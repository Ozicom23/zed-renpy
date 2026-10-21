//! A minimal language server for Ren'Py scripts.
//!
//! Indexes every `.rpy`/`.rpym` file in the workspace at startup (and re-indexes
//! open files as they change), plus the image files Ren'Py defines images
//! from, and answers definition, references, rename, completion, hover,
//! signature help, code actions, document colors and workspace symbols.
//!
//! Parsing is line-based: Ren'Py definition statements always start a line, so a
//! full AST is unnecessary for indexing declaration sites.

mod colors;
mod dap;
mod images;
mod renpy_cli;
mod signature;
mod stats;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use lsp_server::{Connection, Message, Notification, Response};
use lsp_types::{
    CodeAction, CodeActionKind, CodeActionOptions, CodeActionOrCommand, CodeActionParams,
    CodeActionProviderCapability, Color, ColorInformation, ColorPresentation,
    ColorPresentationParams, ColorProviderCapability, CompletionItem, CompletionItemKind,
    CompletionOptions, CompletionParams, CompletionResponse, CreateFile, CreateFileOptions,
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DocumentChangeOperation,
    DocumentChanges, DocumentColorParams, Documentation, FileChangeType, GotoDefinitionParams,
    Hover, HoverContents, HoverParams, HoverProviderCapability, InitializeParams, Location,
    MarkupContent, MarkupKind, OneOf, OptionalVersionedTextDocumentIdentifier,
    ParameterInformation, ParameterLabel, Position, PublishDiagnosticsParams, Range,
    ReferenceParams, RenameParams, ResourceOp, ServerCapabilities, SignatureHelp,
    SignatureHelpOptions, SignatureHelpParams, SignatureInformation, SymbolInformation,
    SymbolKind, TextDocumentEdit, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextDocumentSyncOptions, TextDocumentSyncSaveOptions, TextEdit, Url, WorkspaceEdit,
    WorkspaceSymbolParams,
};

#[derive(Clone)]
struct Definition {
    uri: Url,
    range: Range,
    kind: SymbolKind,
    /// The trimmed source line containing the definition.
    detail: String,
    /// Contiguous `#` comment block immediately above the definition.
    doc: Option<String>,
}

/// A `jump`/`call` usage of a label name, or the image tag of a
/// `show`/`scene`/`hide`.
struct Reference {
    name: String,
    range: Range,
}

#[derive(Default)]
struct Index {
    /// symbol name -> every place it is defined
    defs: HashMap<String, Vec<Definition>>,
    /// file -> names it defined (for cheap invalidation on re-index)
    file_names: HashMap<Url, Vec<String>>,
    /// file -> label references (jump/call targets) it contains
    file_refs: HashMap<Url, Vec<Reference>>,
    /// open-buffer contents, overriding what is on disk
    open_docs: HashMap<Url, String>,
    /// file -> image tags its show/scene/hide statements use
    image_refs: HashMap<Url, Vec<Reference>>,
    /// file -> tags it makes up with `show x as alias`, or defines with a
    /// literal `renpy.image("name", ...)`
    extra_image_tags: HashMap<Url, Vec<String>>,
    /// files defining images from Python with a computed name, which makes the
    /// set of image names unknowable
    dynamic_images: HashSet<Url>,
    /// image files currently indexed as definitions (keys of file_names)
    image_files: Vec<Url>,
    /// file -> its dialogue lines, for statistics
    dialogue: HashMap<Url, Vec<stats::Dialogue>>,
}

impl Index {
    fn remove_file(&mut self, uri: &Url) {
        self.file_refs.remove(uri);
        self.image_refs.remove(uri);
        self.extra_image_tags.remove(uri);
        self.dynamic_images.remove(uri);
        self.dialogue.remove(uri);
        if let Some(names) = self.file_names.remove(uri) {
            for name in names {
                if let Some(defs) = self.defs.get_mut(&name) {
                    defs.retain(|d| &d.uri != uri);
                    if defs.is_empty() {
                        self.defs.remove(&name);
                    }
                }
            }
        }
    }

    fn index_file(&mut self, uri: &Url, text: &str) {
        self.remove_file(uri);
        let mut names = Vec::new();
        let mut refs = Vec::new();
        let mut image_refs = Vec::new();
        let mut extra_tags = Vec::new();
        let mut pending_doc: Vec<String> = Vec::new();
        for (line_no, line) in text.lines().enumerate() {
            let trimmed = line.trim();
            if trimmed.starts_with('#') {
                pending_doc.push(trimmed.trim_start_matches('#').trim().to_string());
                continue;
            }
            if let Some((tag, start, end)) = images::parse_image_reference(line) {
                image_refs.push(Reference {
                    name: tag,
                    range: Range {
                        start: Position { line: line_no as u32, character: start as u32 },
                        end: Position { line: line_no as u32, character: end as u32 },
                    },
                });
            }
            if let Some(alias) = images::parse_show_alias(line) {
                extra_tags.push(alias.to_string());
            }
            match images::parse_python_image_definition(line) {
                Some(Ok(name)) => extra_tags.push(name),
                Some(Err(())) => {
                    self.dynamic_images.insert(uri.clone());
                }
                None => {}
            }
            if let Some((name, start, end)) = parse_reference(line) {
                refs.push(Reference {
                    name,
                    range: Range {
                        start: Position { line: line_no as u32, character: start as u32 },
                        end: Position { line: line_no as u32, character: end as u32 },
                    },
                });
            }
            let found = parse_definitions(line);
            if !found.is_empty() {
                let doc = if pending_doc.is_empty() {
                    None
                } else {
                    Some(pending_doc.join("\n"))
                };
                for (name, start, end, kind) in found {
                    let range = Range {
                        start: Position { line: line_no as u32, character: start as u32 },
                        end: Position { line: line_no as u32, character: end as u32 },
                    };
                    self.defs.entry(name.clone()).or_default().push(Definition {
                        uri: uri.clone(),
                        range,
                        kind,
                        detail: trimmed.to_string(),
                        doc: doc.clone(),
                    });
                    names.push(name);
                }
            }
            // Any non-comment line (blank or code) breaks comment attachment.
            pending_doc.clear();
        }
        self.file_names.insert(uri.clone(), names);
        self.file_refs.insert(uri.clone(), refs);
        self.image_refs.insert(uri.clone(), image_refs);
        self.extra_image_tags.insert(uri.clone(), extra_tags);
        // Translations repeat the dialogue; statistics count the original.
        let is_translation = uri.path().contains("/game/tl/");
        if !is_translation {
            self.dialogue.insert(uri.clone(), stats::scan(text));
        }
    }

    /// Replace the indexed image files with `files` (one project scan).
    fn set_image_files(&mut self, files: &[images::ImageFile]) {
        for uri in std::mem::take(&mut self.image_files) {
            self.remove_file(&uri);
        }
        for file in files {
            let Ok(uri) = Url::from_file_path(&file.path) else { continue };
            let mut names = vec![file.name.clone()];
            let tag = file.name.split(' ').next().unwrap_or("").to_string();
            if tag != file.name {
                names.push(tag);
            }
            for name in &names {
                self.defs.entry(name.clone()).or_default().push(Definition {
                    uri: uri.clone(),
                    range: Range::default(),
                    kind: SymbolKind::CONSTANT,
                    detail: format!("image {} = \"{}\"", file.name, image_relative_path(&file.path)),
                    doc: None,
                });
            }
            self.file_names.insert(uri.clone(), names);
            self.image_files.push(uri);
        }
    }

    /// Every image tag the project defines, or None when it defines images
    /// with computed names (then any tag may exist).
    fn known_image_tags(&self) -> Option<HashSet<&str>> {
        if !self.dynamic_images.is_empty() {
            return None;
        }
        let mut tags: HashSet<&str> = images::BUILTIN_IMAGES.iter().copied().collect();
        for (name, defs) in &self.defs {
            if defs.iter().any(|d| d.kind == SymbolKind::CONSTANT) {
                tags.insert(name.split(' ').next().unwrap_or(name));
            }
        }
        for extra in self.extra_image_tags.values().flatten() {
            tags.insert(extra.split(' ').next().unwrap_or(extra));
        }
        Some(tags)
    }

    /// Project-wide checks: jump/call targets that are not defined anywhere,
    /// labels defined more than once, and show/scene/hide of images that do
    /// not exist. Leading-dot local labels are skipped (their resolution
    /// depends on the enclosing global label). Each diagnostic's `data` names
    /// the quick fix that applies (see `code_actions`).
    fn compute_diagnostics(&self) -> HashMap<Url, Vec<Diagnostic>> {
        let mut out: HashMap<Url, Vec<Diagnostic>> = HashMap::new();
        let labels: std::collections::HashSet<&str> = self
            .defs
            .iter()
            .filter(|(_, defs)| defs.iter().any(|d| d.kind == SymbolKind::FUNCTION))
            .map(|(name, _)| name.as_str())
            .collect();
        for (uri, refs) in &self.file_refs {
            for reference in refs {
                if reference.name.starts_with('.') {
                    continue;
                }
                if !labels.contains(reference.name.as_str()) {
                    let suggestion = images::closest(&reference.name, labels.iter().copied());
                    let mut message =
                        format!("label '{}' is not defined anywhere in the project", reference.name);
                    if let Some(s) = suggestion {
                        message.push_str(&format!(" (did you mean '{s}'?)"));
                    }
                    out.entry(uri.clone()).or_default().push(Diagnostic {
                        range: reference.range,
                        severity: Some(DiagnosticSeverity::ERROR),
                        source: Some("renpy".into()),
                        message,
                        data: Some(serde_json::json!({
                            "fix": "undefined_label",
                            "name": reference.name,
                            "suggestion": suggestion,
                        })),
                        ..Default::default()
                    });
                }
            }
        }
        for (name, defs) in &self.defs {
            if name.starts_with('.') {
                continue;
            }
            let sites: Vec<&Definition> =
                defs.iter().filter(|d| d.kind == SymbolKind::FUNCTION).collect();
            if sites.len() > 1 {
                for site in &sites {
                    out.entry(site.uri.clone()).or_default().push(Diagnostic {
                        range: site.range,
                        severity: Some(DiagnosticSeverity::WARNING),
                        source: Some("renpy".into()),
                        message: format!("label '{}' is defined {} times", name, sites.len()),
                        related_information: Some(
                            sites
                                .iter()
                                .filter(|other| other.uri != site.uri || other.range != site.range)
                                .map(|other| lsp_types::DiagnosticRelatedInformation {
                                    location: Location { uri: other.uri.clone(), range: other.range },
                                    message: format!("label '{name}' is also defined here"),
                                })
                                .collect(),
                        ),
                        data: Some(serde_json::json!({ "fix": "duplicate_label", "name": name })),
                        ..Default::default()
                    });
                }
            }
        }
        if let Some(tags) = self.known_image_tags() {
            for (uri, refs) in &self.image_refs {
                for reference in refs {
                    if tags.contains(reference.name.as_str()) {
                        continue;
                    }
                    let suggestion = images::closest(&reference.name, tags.iter().copied());
                    let mut message = format!(
                        "image '{}' is not defined: no image statement, layeredimage or file in game/images uses this tag",
                        reference.name
                    );
                    if let Some(s) = suggestion {
                        message.push_str(&format!(" (did you mean '{s}'?)"));
                    }
                    out.entry(uri.clone()).or_default().push(Diagnostic {
                        range: reference.range,
                        severity: Some(DiagnosticSeverity::WARNING),
                        source: Some("renpy".into()),
                        message,
                        data: Some(serde_json::json!({
                            "fix": "unknown_image",
                            "name": reference.name,
                            "suggestion": suggestion,
                        })),
                        ..Default::default()
                    });
                }
            }
        }
        out
    }

    fn lookup_defs(&self, word: &str) -> Option<&Vec<Definition>> {
        let mut candidates: Vec<&str> = vec![word];
        // `.local` labels and a bare trailing segment as fallbacks.
        if let Some(stripped) = word.strip_prefix('.') {
            candidates.push(stripped);
        }
        candidates.into_iter().find_map(|c| self.defs.get(c))
    }

    fn lookup(&self, word: &str) -> Vec<Location> {
        self.lookup_defs(word)
            .map(|defs| {
                defs.iter()
                    .map(|d| Location { uri: d.uri.clone(), range: d.range })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn def_completions(&self, want: fn(SymbolKind) -> bool) -> Vec<CompletionItem> {
        let mut items = Vec::new();
        for (name, defs) in &self.defs {
            if let Some(def) = defs.iter().find(|d| want(d.kind)) {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(map_completion_kind(def.kind)),
                    detail: Some(def.detail.clone()),
                    ..Default::default()
                });
            }
        }
        items
    }

    /// Precise references to a label: every `jump`/`call` usage in the project.
    fn label_references(&self, name: &str) -> Vec<Location> {
        let mut out = Vec::new();
        for (uri, refs) in &self.file_refs {
            for reference in refs {
                if reference.name == name {
                    out.push(Location { uri: uri.clone(), range: reference.range });
                }
            }
        }
        out
    }

    /// Word-boundary textual occurrences across all indexed files — used for
    /// non-label symbols (speakers, variables), whose usages appear in
    /// dialogue lines and python fragments the indexer doesn't parse.
    fn textual_references(&self, word: &str) -> Vec<Location> {
        let mut out = Vec::new();
        for uri in self.file_names.keys() {
            let Some(text) = self.open_docs.get(uri).cloned().or_else(|| {
                uri.to_file_path().ok().and_then(|p| std::fs::read_to_string(p).ok())
            }) else {
                continue;
            };
            for (line_no, line) in text.lines().enumerate() {
                for (byte, _) in line.match_indices(word) {
                    let before_ok = byte == 0 || !is_ident_byte(line.as_bytes()[byte - 1]);
                    let after = byte + word.len();
                    let after_ok = after >= line.len() || !is_ident_byte(line.as_bytes()[after]);
                    if before_ok && after_ok {
                        out.push(Location {
                            uri: uri.clone(),
                            range: Range {
                                start: Position {
                                    line: line_no as u32,
                                    character: byte_to_utf16_col(line, byte),
                                },
                                end: Position {
                                    line: line_no as u32,
                                    character: byte_to_utf16_col(line, after),
                                },
                            },
                        });
                        if out.len() >= 500 {
                            return out;
                        }
                    }
                }
            }
        }
        out
    }

    /// Variables whose definition constructs a Character() — the speakers.
    fn speaker_completions(&self) -> Vec<CompletionItem> {
        let mut items = Vec::new();
        for (name, defs) in &self.defs {
            if let Some(def) = defs
                .iter()
                .find(|d| d.kind == SymbolKind::VARIABLE && d.detail.contains("Character("))
            {
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(CompletionItemKind::VARIABLE),
                    detail: Some(def.detail.clone()),
                    ..Default::default()
                });
            }
        }
        items
    }
}

/// True for bytes that can appear in a Ren'Py identifier (dotted names included).
fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'.'
}

/// Take an identifier (letters, digits, `_`, `.`) starting at `pos` in `s`.
/// Returns (identifier, end_pos). Leading digits are allowed by the caller's checks.
fn take_ident(s: &str, pos: usize) -> (&str, usize) {
    let bytes = s.as_bytes();
    let mut end = pos;
    while end < bytes.len() && is_ident_byte(bytes[end]) {
        end += 1;
    }
    (&s[pos..end], end)
}

fn skip_spaces(s: &str, pos: usize) -> usize {
    let bytes = s.as_bytes();
    let mut p = pos;
    while p < bytes.len() && (bytes[p] == b' ' || bytes[p] == b'\t') {
        p += 1;
    }
    p
}

/// Match `keyword` at `pos` followed by at least one space.
fn eat_keyword(s: &str, pos: usize, keyword: &str) -> Option<usize> {
    let rest = &s[pos..];
    if rest.starts_with(keyword) {
        let after = pos + keyword.len();
        if s.as_bytes().get(after) == Some(&b' ') {
            return Some(skip_spaces(s, after));
        }
    }
    None
}

/// Extract all definitions declared on one line.
/// Returns (name, start_col, end_col, kind); columns are byte offsets, which
/// equal UTF-16 columns here because everything before a name is ASCII.
fn parse_definitions(line: &str) -> Vec<(String, usize, usize, SymbolKind)> {
    let mut out = Vec::new();
    let indent = line.len() - line.trim_start().len();
    let l = line;

    // label NAME(...):  |  menu NAME:  (a named menu is a jump target, like a label)
    for (kw, kind) in [("label", SymbolKind::FUNCTION), ("menu", SymbolKind::FUNCTION)] {
        if let Some(p) = eat_keyword(l, indent, kw) {
            let (name, end) = take_ident(l, p);
            if !name.is_empty() && l[end..].contains(':') {
                out.push((name.to_string(), p, end, kind));
            }
            return out;
        }
    }

    // call TARGET ... from NAME — the from clause defines a real label.
    // (`call screen ...` invokes a screen and defines nothing.)
    if let Some(p) = eat_keyword(l, indent, "call") {
        let (target, _) = take_ident(l, p);
        if target == "screen" {
            return out;
        }
        if let Some(from_pos) = l.rfind(" from ") {
            let p = skip_spaces(l, from_pos + " from ".len());
            let (name, end) = take_ident(l, p);
            // Only a bare identifier at end-of-line (or before a comment) is a
            // real from-clause; anything else is e.g. " from " inside a string
            // argument like call shop("back from war").
            let rest = l[end..].trim_start();
            if !name.is_empty() && (rest.is_empty() || rest.starts_with('#')) {
                out.push((name.to_string(), p, end, SymbolKind::FUNCTION));
            }
        }
        return out;
    }

    // define NAME = ...  |  default NAME = ...
    for kw in ["define", "default"] {
        if let Some(mut p) = eat_keyword(l, indent, kw) {
            // Skip an optional init-offset number: `define 2 foo = ...`
            let (first, end) = take_ident(l, p);
            let (name, start, end) = if first.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                let p2 = skip_spaces(l, end);
                let (second, end2) = take_ident(l, p2);
                (second, p2, end2)
            } else {
                (first, p, end)
            };
            p = start;
            if !name.is_empty() && l[end..].contains('=') {
                out.push((name.to_string(), p, end, SymbolKind::VARIABLE));
            }
            return out;
        }
    }

    // image NAME PARTS... = ...   |   image NAME PARTS...:   (ATL block)
    if let Some(p) = eat_keyword(l, indent, "image") {
        let mut parts: Vec<(usize, usize)> = Vec::new();
        let mut pos = p;
        loop {
            let (part, end) = take_ident(l, pos);
            if part.is_empty() {
                break;
            }
            parts.push((pos, end));
            pos = skip_spaces(l, end);
        }
        let terminated = l[pos..].trim_start().starts_with('=') || l[pos..].trim_start().starts_with(':');
        if !parts.is_empty() && terminated {
            let full_start = parts[0].0;
            let full_end = parts[parts.len() - 1].1;
            let full_name = l[full_start..full_end].to_string();
            out.push((full_name.clone(), full_start, full_end, SymbolKind::CONSTANT));
            // Also index the first tag alone so `show eileen ...` resolves.
            let first = &l[parts[0].0..parts[0].1];
            if parts.len() > 1 && first != full_name {
                out.push((first.to_string(), parts[0].0, parts[0].1, SymbolKind::CONSTANT));
            }
        }
        return out;
    }

    // layeredimage NAME:  (an image tag with its attributes in the block)
    if let Some(p) = eat_keyword(l, indent, "layeredimage") {
        let (name, end) = take_ident(l, p);
        if !name.is_empty() {
            out.push((name.to_string(), p, end, SymbolKind::CONSTANT));
        }
        return out;
    }

    // screen NAME(...):  |  transform NAME:  |  style NAME:
    for (kw, kind) in [
        ("screen", SymbolKind::CLASS),
        ("transform", SymbolKind::METHOD),
        ("style", SymbolKind::PROPERTY),
    ] {
        if let Some(p) = eat_keyword(l, indent, kw) {
            let (name, end) = take_ident(l, p);
            if !name.is_empty() {
                out.push((name.to_string(), p, end, kind));
            }
            return out;
        }
    }

    out
}

/// A `jump NAME` / `call NAME` usage on this line, as (name, start, end).
/// Dynamic `jump expression ...` / `call expression ...` targets are ignored.
fn parse_reference(line: &str) -> Option<(String, usize, usize)> {
    let indent = line.len() - line.trim_start().len();
    for kw in ["jump", "call"] {
        if let Some(p) = eat_keyword(line, indent, kw) {
            let (name, end) = take_ident(line, p);
            // `call screen X` invokes a screen, not a label, and
            // `jump/call expression ...` targets are dynamic.
            if name.is_empty() || name == "expression" || name == "screen" {
                return None;
            }
            return Some((name.to_string(), p, end));
        }
    }
    None
}

/// Convert an LSP position (UTF-16 column) to a byte offset within `line`.
fn utf16_col_to_byte(line: &str, col: u32) -> usize {
    let mut utf16 = 0u32;
    for (byte_idx, ch) in line.char_indices() {
        if utf16 >= col {
            return byte_idx;
        }
        utf16 += ch.len_utf16() as u32;
    }
    line.len()
}

/// The identifier under the cursor, if any.
fn word_at(text: &str, position: Position) -> Option<String> {
    let line = text.lines().nth(position.line as usize)?;
    let bytes = line.as_bytes();
    let mut idx = utf16_col_to_byte(line, position.character);
    if idx >= bytes.len() || !is_ident_byte(bytes[idx]) {
        // Cursor may sit just past the last character of the word.
        if idx == 0 || !is_ident_byte(bytes[idx - 1]) {
            return None;
        }
        idx -= 1;
    }
    let mut start = idx;
    while start > 0 && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = idx;
    while end < bytes.len() && is_ident_byte(bytes[end]) {
        end += 1;
    }
    let word = line[start..end].trim_matches('.');
    if word.is_empty() {
        None
    } else {
        Some(line[start..end].to_string())
    }
}

/// `images/bg/room.png` for a file in `<project>/game/images/bg/room.png`.
fn image_relative_path(path: &Path) -> String {
    let full = path.to_string_lossy().replace('\\', "/");
    match full.rfind("/game/") {
        Some(pos) => full[pos + "/game/".len()..].to_string(),
        None => full,
    }
}

fn is_image_file(path: &Path) -> bool {
    images::image_name_for(path).is_some()
}

fn is_renpy_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("rpy") | Some("rpym")
    )
}

const SKIP_DIRS: &[&str] = &["cache", "saves", "tmp", "node_modules", "target"];

fn walk_workspace(dir: &Path, files: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else { continue };
        if file_type.is_symlink() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if file_type.is_dir() {
            if !SKIP_DIRS.contains(&name.as_ref()) {
                walk_workspace(&path, files);
            }
        } else if is_renpy_file(&path) {
            files.push(path);
        }
    }
}

/// Ren'Py API documentation bundled from the vscode-language-renpy project
/// (MIT, generated from Ren'Py's own docs; see assets/renpy-docs-LICENSE).
/// Entry shape: name -> [category, kind, signature, "", kind, documentation].
static RENPY_DOCS_JSON: &str = include_str!("../assets/renpy-docs.json");

/// The Ren'Py release the bundled docs were generated from (upstream commit
/// fd12569: "Corresponding to the current development version of Ren'Py 8.3").
/// Update this when re-vendoring assets/renpy-docs.json.
const RENPY_DOCS_VERSION: &str = "8.3";

/// Symbol -> "page.html#anchor" on the official docs site; regenerate with
/// assets/generate_doc_links.py (parses the docs' Sphinx objects.inv).
static RENPY_DOC_LINKS_JSON: &str = include_str!("../assets/renpy-doc-links.json");

/// Doc links always point at the latest published documentation.
const RENPY_DOCS_BASE_URL: &str = "https://www.renpy.org/doc/html/";

#[derive(serde::Deserialize, Default)]
struct BuiltinDocs {
    #[serde(default)]
    config: HashMap<String, Vec<serde_json::Value>>,
    #[serde(default)]
    renpy: HashMap<String, Vec<serde_json::Value>>,
    #[serde(default)]
    internal: HashMap<String, Vec<serde_json::Value>>,
    #[serde(skip)]
    links: HashMap<String, String>,
}

impl BuiltinDocs {
    fn load() -> Self {
        let mut docs: BuiltinDocs = serde_json::from_str(RENPY_DOCS_JSON).unwrap_or_else(|err| {
            eprintln!("renpy-language-server: failed to parse bundled API docs: {err}");
            Self::default()
        });
        docs.links = serde_json::from_str(RENPY_DOC_LINKS_JSON).unwrap_or_else(|err| {
            eprintln!("renpy-language-server: failed to parse bundled doc links: {err}");
            HashMap::new()
        });
        docs
    }

    fn len(&self) -> usize {
        self.config.len() + self.renpy.len() + self.internal.len()
    }

    fn hover_markdown(&self, word: &str) -> Option<String> {
        let entry = self
            .renpy
            .get(word)
            .or_else(|| self.config.get(word))
            .or_else(|| self.internal.get(word))?;
        let kind = entry.get(1).and_then(|v| v.as_str()).unwrap_or("");
        let signature = entry.get(2).and_then(|v| v.as_str()).unwrap_or("");
        let doc = entry.get(5).and_then(|v| v.as_str()).unwrap_or("").trim();
        let code = match kind {
            "function" | "class" => format!("{word}{signature}"),
            _ if !signature.is_empty() => format!("{word} {signature}"),
            _ => word.to_string(),
        };
        let mut text = format!("```renpy\n{code}\n```");
        if !doc.is_empty() {
            text.push_str("\n\n");
            text.push_str(&truncate_chars(doc, 2000));
        }
        if let Some(path) = self.links.get(word) {
            text.push_str(&format!(
                "\n\n[Ren'Py documentation]({RENPY_DOCS_BASE_URL}{path})"
            ));
        }
        if !kind.is_empty() {
            text.push_str(&format!(
                "\n\n*Ren'Py built-in ({kind}) — docs from Ren'Py {RENPY_DOCS_VERSION}*"
            ));
        }
        Some(text)
    }

    /// `Character(name=NotSet, kind=None, **properties)` and its docs, for
    /// built-in functions and classes.
    fn signature(&self, name: &str) -> Option<(String, Option<String>)> {
        let entry = self
            .renpy
            .get(name)
            .or_else(|| self.config.get(name))
            .or_else(|| self.internal.get(name))?;
        let kind = entry.get(1).and_then(|v| v.as_str()).unwrap_or("");
        let signature = entry.get(2).and_then(|v| v.as_str()).unwrap_or("");
        if !matches!(kind, "function" | "class") || !signature.starts_with('(') {
            return None;
        }
        let doc = entry.get(5).and_then(|v| v.as_str()).map(str::trim).filter(|d| !d.is_empty());
        Some((format!("{name}{signature}"), doc.map(String::from)))
    }

    fn completions(&self) -> Vec<CompletionItem> {
        let mut items = Vec::new();
        for section in [&self.renpy, &self.config, &self.internal] {
            for (name, entry) in section {
                let kind = entry.get(1).and_then(|v| v.as_str()).unwrap_or("");
                let signature = entry.get(2).and_then(|v| v.as_str()).unwrap_or("");
                let doc = entry.get(5).and_then(|v| v.as_str()).unwrap_or("").trim();
                items.push(CompletionItem {
                    label: name.clone(),
                    kind: Some(match kind {
                        "function" => CompletionItemKind::FUNCTION,
                        "class" => CompletionItemKind::CLASS,
                        _ => CompletionItemKind::VARIABLE,
                    }),
                    detail: if signature.is_empty() {
                        None
                    } else {
                        Some(signature.to_string())
                    },
                    documentation: if doc.is_empty() {
                        None
                    } else {
                        Some(Documentation::MarkupContent(MarkupContent {
                            kind: MarkupKind::Markdown,
                            value: truncate_chars(doc, 300),
                        }))
                    },
                    ..Default::default()
                });
            }
        }
        items
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let cut: String = s.chars().take(max).collect();
    format!("{cut}…")
}

const STATEMENT_KEYWORDS: &[&str] = &[
    "label", "menu", "jump", "call", "return", "pass", "if", "elif", "else", "while",
    "init", "python", "define", "default", "image", "scene", "show", "hide", "with",
    "at", "play", "queue", "stop", "voice", "pause", "window", "camera", "screen",
    "transform", "style", "translate", "nvl", "$",
];

const BUILTIN_POSITIONS: &[&str] = &[
    "left", "right", "center", "truecenter", "topleft", "top", "topright",
    "offscreenleft", "offscreenright", "default",
];

enum CompletionContext {
    Labels,
    Screens,
    Images { after_at: bool },
    LineStart,
    General,
}

fn completion_context(line_prefix: &str) -> CompletionContext {
    let t = line_prefix.trim_start();
    for kw in ["show screen ", "call screen ", "hide screen "] {
        if t.starts_with(kw) {
            return CompletionContext::Screens;
        }
    }
    for kw in ["jump ", "call "] {
        if let Some(rest) = t.strip_prefix(kw) {
            // After `from`, the user is naming a brand-new label.
            if rest.contains(" from ") {
                return CompletionContext::General;
            }
            return CompletionContext::Labels;
        }
    }
    for kw in ["show ", "scene ", "hide "] {
        if t.starts_with(kw) {
            return CompletionContext::Images { after_at: t.contains(" at ") };
        }
    }
    if !t.contains(' ') {
        return CompletionContext::LineStart;
    }
    CompletionContext::General
}

fn map_completion_kind(kind: SymbolKind) -> CompletionItemKind {
    match kind {
        SymbolKind::FUNCTION => CompletionItemKind::FUNCTION,
        SymbolKind::VARIABLE => CompletionItemKind::VARIABLE,
        SymbolKind::CONSTANT => CompletionItemKind::CONSTANT,
        SymbolKind::CLASS => CompletionItemKind::CLASS,
        SymbolKind::METHOD => CompletionItemKind::METHOD,
        SymbolKind::PROPERTY => CompletionItemKind::PROPERTY,
        _ => CompletionItemKind::TEXT,
    }
}

fn keyword_completions() -> Vec<CompletionItem> {
    STATEMENT_KEYWORDS
        .iter()
        .map(|kw| CompletionItem {
            label: (*kw).to_string(),
            kind: Some(CompletionItemKind::KEYWORD),
            ..Default::default()
        })
        .collect()
}

fn position_completions() -> Vec<CompletionItem> {
    BUILTIN_POSITIONS
        .iter()
        .map(|name| CompletionItem {
            label: (*name).to_string(),
            kind: Some(CompletionItemKind::CONSTANT),
            detail: Some("built-in position".into()),
            ..Default::default()
        })
        .collect()
}

fn project_hover_markdown(defs: &[Definition], roots: &[PathBuf]) -> String {
    let def = &defs[0];
    let mut text = format!("```renpy\n{}\n```", def.detail);
    if let Some(doc) = &def.doc {
        text.push_str("\n\n");
        text.push_str(doc);
    }
    if let Ok(path) = def.uri.to_file_path() {
        let display = roots
            .iter()
            .find_map(|root| path.strip_prefix(root).ok().map(|r| r.display().to_string()))
            .unwrap_or_else(|| path.display().to_string());
        text.push_str(&format!("\n\n*defined in {}:{}*", display, def.range.start.line + 1));
    }
    if defs.len() > 1 {
        text.push_str(&format!("\n\n*+{} more definition(s)*", defs.len() - 1));
    }
    text
}

/// UTF-16 column of a byte offset within `line` (needed for lines containing
/// non-ASCII dialogue text; `byte` must lie on a char boundary).
fn byte_to_utf16_col(line: &str, byte: usize) -> u32 {
    line[..byte].encode_utf16().count() as u32
}

/// Build the WorkspaceEdit for renaming a label. Only labels are renameable:
/// their definition and reference ranges are tracked precisely, so the edit
/// cannot touch prose. Errors are user-facing messages.
fn rename_edits(
    index: &Index,
    word: &str,
    new_name: &str,
) -> Result<HashMap<Url, Vec<TextEdit>>, String> {
    if new_name.is_empty()
        || new_name.bytes().any(|b| !is_ident_byte(b))
        || new_name.as_bytes()[0].is_ascii_digit()
    {
        return Err(format!("'{new_name}' is not a valid label name"));
    }
    let label_defs: Vec<&Definition> = index
        .defs
        .get(word)
        .map(|defs| defs.iter().filter(|d| d.kind == SymbolKind::FUNCTION).collect())
        .unwrap_or_default();
    if label_defs.is_empty() {
        return Err("rename is currently supported for labels only".to_string());
    }
    if index
        .defs
        .get(new_name)
        .is_some_and(|defs| defs.iter().any(|d| d.kind == SymbolKind::FUNCTION))
    {
        return Err(format!("label '{new_name}' already exists"));
    }
    let mut changes: HashMap<Url, Vec<TextEdit>> = HashMap::new();
    for def in label_defs {
        changes
            .entry(def.uri.clone())
            .or_default()
            .push(TextEdit { range: def.range, new_text: new_name.to_string() });
    }
    for (uri, refs) in &index.file_refs {
        for reference in refs {
            if reference.name == word {
                changes
                    .entry(uri.clone())
                    .or_default()
                    .push(TextEdit { range: reference.range, new_text: new_name.to_string() });
            }
        }
    }
    Ok(changes)
}

/// Runs `renpy lint` off-thread on demand, at most one at a time; a save that
/// arrives mid-run queues exactly one follow-up.
struct LintRunner {
    enabled: bool,
    sdk: Option<PathBuf>,
    project: Option<PathBuf>,
    running: bool,
    pending: bool,
    tx: crossbeam_channel::Sender<Result<String, String>>,
}

impl LintRunner {
    fn request(&mut self) {
        if !self.enabled {
            return;
        }
        let (Some(sdk), Some(project)) = (&self.sdk, &self.project) else { return };
        if self.running {
            self.pending = true;
            return;
        }
        let Some(invocation) = renpy_cli::sdk_invocation(sdk) else { return };
        self.running = true;
        let project = project.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let output = std::process::Command::new(&invocation.program)
                .args(&invocation.prefix_args)
                .arg(&project)
                .arg("lint")
                .stdin(std::process::Stdio::null())
                .output();
            let outcome = match output {
                Ok(output) if output.status.success() => {
                    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
                }
                Ok(output) => Err(format!(
                    "renpy lint failed ({}): {}",
                    output.status,
                    String::from_utf8_lossy(&output.stderr).trim()
                )),
                Err(err) => Err(format!("could not run renpy lint: {err}")),
            };
            eprintln!(
                "renpy-language-server: renpy lint finished in {:.1}s",
                started.elapsed().as_secs_f32()
            );
            let _ = tx.send(outcome);
        });
    }
}

/// Lint problems as LSP diagnostics. The end column is deliberately past any
/// real line length: per the LSP spec, clients clamp it to the line end, which
/// underlines the whole line (lint has no column information).
fn lint_diagnostics(project: &Path, report: &str) -> HashMap<Url, Vec<Diagnostic>> {
    let mut out: HashMap<Url, Vec<Diagnostic>> = HashMap::new();
    for problem in renpy_cli::parse_lint_report(report) {
        let Ok(uri) = Url::from_file_path(project.join(&problem.path)) else { continue };
        out.entry(uri).or_default().push(Diagnostic {
            range: Range {
                start: Position { line: problem.line, character: 0 },
                end: Position { line: problem.line, character: u32::MAX },
            },
            severity: Some(DiagnosticSeverity::WARNING),
            source: Some("renpy lint".into()),
            message: problem.message,
            ..Default::default()
        });
    }
    out
}

fn publish_diagnostics(
    connection: &Connection,
    index: &Index,
    lint_diags: &HashMap<Url, Vec<Diagnostic>>,
    previously: &mut std::collections::HashSet<Url>,
) -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    let mut current = index.compute_diagnostics();
    // Merge lint findings, except on lines the indexer already flagged (both
    // tools report undefined jump targets; ours has the more precise range).
    for (uri, diags) in lint_diags {
        let entry = current.entry(uri.clone()).or_default();
        for diag in diags {
            if entry.iter().any(|existing| existing.range.start.line == diag.range.start.line) {
                continue;
            }
            entry.push(diag.clone());
        }
    }
    let cleared: Vec<Url> = previously
        .iter()
        .filter(|uri| !current.contains_key(uri))
        .cloned()
        .collect();
    for uri in cleared {
        previously.remove(&uri);
        let params = PublishDiagnosticsParams { uri, diagnostics: Vec::new(), version: None };
        connection.sender.send(Message::Notification(Notification {
            method: "textDocument/publishDiagnostics".into(),
            params: serde_json::to_value(params)?,
        }))?;
    }
    for (uri, diagnostics) in current {
        previously.insert(uri.clone());
        let params = PublishDiagnosticsParams { uri, diagnostics, version: None };
        connection.sender.send(Message::Notification(Notification {
            method: "textDocument/publishDiagnostics".into(),
            params: serde_json::to_value(params)?,
        }))?;
    }
    Ok(())
}

/// Where a new statement goes at the end of `text`, and the text to insert
/// so it starts on a fresh line after one blank line.
fn end_of_document(text: &str) -> Position {
    if text.is_empty() || text.ends_with('\n') {
        Position { line: text.lines().count() as u32, character: 0 }
    } else {
        let last = text.lines().last().unwrap_or("");
        Position {
            line: text.lines().count().saturating_sub(1) as u32,
            character: last.encode_utf16().count() as u32,
        }
    }
}

fn ranges_overlap(a: &Range, b: &Range) -> bool {
    let before = |x: &Position, y: &Position| (x.line, x.character) < (y.line, y.character);
    !before(&a.end, &b.start) && !before(&b.end, &a.start)
}

fn quick_fix(title: String, uri: &Url, edits: Vec<TextEdit>, diagnostic: &Diagnostic, preferred: bool) -> CodeActionOrCommand {
    CodeActionOrCommand::CodeAction(CodeAction {
        title,
        kind: Some(CodeActionKind::QUICKFIX),
        diagnostics: Some(vec![diagnostic.clone()]),
        edit: Some(WorkspaceEdit {
            changes: Some(HashMap::from([(uri.clone(), edits)])),
            ..Default::default()
        }),
        is_preferred: Some(preferred),
        ..Default::default()
    })
}

/// Quick fixes for the diagnostics under `range`. They are recomputed from
/// the index rather than read from the client's copy of the diagnostics.
fn code_actions(index: &Index, uri: &Url, range: Range, text: &str) -> Vec<CodeActionOrCommand> {
    let mut actions = Vec::new();
    let diagnostics = index.compute_diagnostics();
    let Some(file_diagnostics) = diagnostics.get(uri) else { return actions };
    for diagnostic in file_diagnostics {
        if !ranges_overlap(&diagnostic.range, &range) {
            continue;
        }
        let Some(data) = &diagnostic.data else { continue };
        let name = data["name"].as_str().unwrap_or_default();
        let suggestion = data["suggestion"].as_str();
        match data["fix"].as_str() {
            Some("undefined_label") => {
                if let Some(s) = suggestion {
                    actions.push(quick_fix(
                        format!("Change to '{s}'"),
                        uri,
                        vec![TextEdit { range: diagnostic.range, new_text: s.to_string() }],
                        diagnostic,
                        true,
                    ));
                }
                let separator = if text.is_empty() || text.ends_with("\n\n") {
                    ""
                } else if text.ends_with('\n') {
                    "\n"
                } else {
                    "\n\n"
                };
                let at = end_of_document(text);
                actions.push(quick_fix(
                    format!("Create label '{name}' at the end of this file"),
                    uri,
                    vec![TextEdit {
                        range: Range { start: at, end: at },
                        new_text: format!("{separator}label {name}:\n    return\n"),
                    }],
                    diagnostic,
                    suggestion.is_none(),
                ));
            }
            Some("duplicate_label") => {
                let free = (2..)
                    .map(|n| format!("{name}_{n}"))
                    .find(|candidate| !index.defs.contains_key(candidate))
                    .unwrap_or_default();
                actions.push(quick_fix(
                    format!("Rename this label to '{free}'"),
                    uri,
                    vec![TextEdit { range: diagnostic.range, new_text: free.clone() }],
                    diagnostic,
                    false,
                ));
            }
            Some("unknown_image") => {
                if let Some(s) = suggestion {
                    actions.push(quick_fix(
                        format!("Change to '{s}'"),
                        uri,
                        vec![TextEdit { range: diagnostic.range, new_text: s.to_string() }],
                        diagnostic,
                        true,
                    ));
                }
            }
            _ => {}
        }
    }
    actions
}

/// The source text up to `position`, with up to 20 lines before it, so calls
/// whose arguments span lines are still found.
fn text_before(text: &str, position: Position) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let line_idx = position.line as usize;
    let Some(line) = lines.get(line_idx) else { return String::new() };
    let first = line_idx.saturating_sub(20);
    let mut out = lines[first..line_idx].join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    out.push_str(&line[..utf16_col_to_byte(line, position.character)]);
    out
}

/// Parameter hints for Ren'Py built-ins (`Character(`, `renpy.pause(`,
/// actions) and the project's own labels, screens and transforms.
fn signature_help(index: &Index, docs: &BuiltinDocs, text: &str, position: Position) -> Option<SignatureHelp> {
    let site = signature::call_at(&text_before(text, position))?;
    let (label, documentation) = if let Some((sig, doc)) = docs.signature(&site.name) {
        (sig, doc)
    } else {
        let def = index.defs.get(&site.name)?.iter().find(|d| {
            matches!(d.kind, SymbolKind::FUNCTION | SymbolKind::CLASS | SymbolKind::METHOD)
                && d.detail.contains('(')
        })?;
        (def.detail.trim_end_matches(':').trim_end().to_string(), def.doc.clone())
    };
    let list = signature::parameter_list(&label)?;
    let parameters = signature::split_parameters(list);
    // Parameter labels as UTF-16 offsets into `label`, so repeated names are
    // highlighted at the right place.
    let list_start = label.find(list).unwrap_or(0);
    let mut search_from = list_start;
    let mut infos = Vec::new();
    for parameter in &parameters {
        let Some(found) = label[search_from..].find(parameter.as_str()) else { break };
        let start = search_from + found;
        let end = start + parameter.len();
        search_from = end;
        infos.push(ParameterInformation {
            label: ParameterLabel::LabelOffsets([
                label[..start].encode_utf16().count() as u32,
                label[..end].encode_utf16().count() as u32,
            ]),
            documentation: None,
        });
    }
    let active = signature::active_parameter(&parameters, &site);
    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation: documentation.map(|doc| {
                Documentation::MarkupContent(MarkupContent {
                    kind: MarkupKind::Markdown,
                    value: truncate_chars(&doc, 1500),
                })
            }),
            parameters: Some(infos),
            active_parameter: active,
        }],
        active_signature: Some(0),
        active_parameter: active,
    })
}

/// Hover text for an image defined by a file: its name and a preview.
fn image_file_hover(def: &Definition) -> Option<String> {
    let path = def.uri.to_file_path().ok()?;
    if !is_image_file(&path) {
        return None;
    }
    let name = images::image_name_for(&path)?;
    Some(format!(
        "```renpy\nimage {name}\n```\n\n![{name}]({})\n\n*defined by the file {}*",
        def.uri,
        image_relative_path(&path)
    ))
}

/// Dialogue statistics for a speaker or a label, shown under its hover.
fn stats_hover(index: &Index, word: &str) -> Option<String> {
    let defs = index.defs.get(word)?;
    let all = || index.dialogue.values().flatten();
    if defs.iter().any(|d| d.kind == SymbolKind::VARIABLE && d.detail.contains("Character(")) {
        let mine = stats::by_speaker(all().filter(|d| d.speaker.as_deref() == Some(word)));
        let tally = mine.get(word).copied().unwrap_or_default();
        let total: usize = all().map(|d| d.words).sum();
        let share = if total == 0 { 0.0 } else { tally.words as f64 * 100.0 / total as f64 };
        return Some(format!(
            "**Dialogue:** {} lines · {} words · {:.1}% of all words",
            stats::thousands(tally.lines),
            stats::thousands(tally.words),
            share
        ));
    }
    if defs.iter().any(|d| d.kind == SymbolKind::FUNCTION) {
        let mut tally = stats::Tally::default();
        for d in all().filter(|d| d.label.as_deref() == Some(word)) {
            tally.add(d.words);
        }
        if tally.lines > 0 {
            return Some(format!(
                "**Dialogue in this label:** {} lines · {} words · about {} to read",
                stats::thousands(tally.lines),
                stats::thousands(tally.words),
                stats::reading_time(tally.words)
            ));
        }
    }
    None
}

/// The Markdown report written by the "script statistics" source action.
fn stats_report(index: &Index, roots: &[PathBuf]) -> String {
    let all: Vec<&stats::Dialogue> = index.dialogue.values().flatten().collect();
    let total_words: usize = all.iter().map(|d| d.words).sum();
    let mut out = String::from("# Script statistics\n\n");
    out.push_str(&format!(
        "{} lines of dialogue, {} words: about {} to read at 200 words a minute.\n\n",
        stats::thousands(all.len()),
        stats::thousands(total_words),
        stats::reading_time(total_words)
    ));
    let share = |words: usize| {
        if total_words == 0 { 0.0 } else { words as f64 * 100.0 / total_words as f64 }
    };

    out.push_str("## By character\n\n| Character | Lines | Words | Share |\n|---|---:|---:|---:|\n");
    let mut speakers: Vec<(String, stats::Tally)> = stats::by_speaker(all.iter().copied()).into_iter().collect();
    speakers.sort_by(|a, b| b.1.words.cmp(&a.1.words).then_with(|| a.0.cmp(&b.0)));
    for (who, tally) in &speakers {
        let display = index
            .defs
            .get(who)
            .and_then(|defs| defs.iter().find_map(|d| stats::character_display_name(&d.detail)))
            .map(|name| format!("{name} (`{who}`)"))
            .unwrap_or_else(|| format!("`{who}`"));
        out.push_str(&format!(
            "| {display} | {} | {} | {:.1}% |\n",
            stats::thousands(tally.lines),
            stats::thousands(tally.words),
            share(tally.words)
        ));
    }

    out.push_str("\n## By file\n\n| File | Lines | Words |\n|---|---:|---:|\n");
    let mut files: Vec<(String, stats::Tally)> = index
        .dialogue
        .iter()
        .filter(|(_, lines)| !lines.is_empty())
        .map(|(uri, lines)| {
            let path = uri.to_file_path().unwrap_or_default();
            let display = roots
                .iter()
                .find_map(|root| path.strip_prefix(root).ok().map(|r| r.display().to_string()))
                .unwrap_or_else(|| path.display().to_string());
            let mut tally = stats::Tally::default();
            for d in lines {
                tally.add(d.words);
            }
            (display, tally)
        })
        .collect();
    files.sort_by(|a, b| b.1.words.cmp(&a.1.words).then_with(|| a.0.cmp(&b.0)));
    for (file, tally) in &files {
        out.push_str(&format!(
            "| {file} | {} | {} |\n",
            stats::thousands(tally.lines),
            stats::thousands(tally.words)
        ));
    }

    out.push_str("\n## Longest labels\n\n| Label | Lines | Words | Reading time |\n|---|---:|---:|---:|\n");
    let mut labels: HashMap<&str, stats::Tally> = HashMap::new();
    for d in &all {
        if let Some(label) = &d.label {
            labels.entry(label.as_str()).or_default().add(d.words);
        }
    }
    let mut labels: Vec<(&str, stats::Tally)> = labels.into_iter().collect();
    labels.sort_by(|a, b| b.1.words.cmp(&a.1.words).then_with(|| a.0.cmp(b.0)));
    for (label, tally) in labels.iter().take(25) {
        out.push_str(&format!(
            "| `{label}` | {} | {} | {} |\n",
            stats::thousands(tally.lines),
            stats::thousands(tally.words),
            stats::reading_time(tally.words)
        ));
    }
    out.push_str(
        "\n*Counted by the Ren'Py extension for Zed: dialogue and narration lines, \
         without text tags; translations under game/tl are not counted.*\n",
    );
    out
}

/// The source action that writes `script_stats.md` into the project.
fn stats_action(index: &Index, roots: &[PathBuf], project: &Path) -> Option<CodeActionOrCommand> {
    let path = project.join("script_stats.md");
    let uri = Url::from_file_path(&path).ok()?;
    let report = stats_report(index, roots);
    let mut operations = Vec::new();
    let range = match std::fs::read_to_string(&path) {
        Ok(existing) => Range { start: Position::default(), end: end_of_document(&existing) },
        Err(_) => {
            operations.push(DocumentChangeOperation::Op(ResourceOp::Create(CreateFile {
                uri: uri.clone(),
                options: Some(CreateFileOptions { overwrite: Some(false), ignore_if_exists: Some(true) }),
                annotation_id: None,
            })));
            Range::default()
        }
    };
    operations.push(DocumentChangeOperation::Edit(TextDocumentEdit {
        text_document: OptionalVersionedTextDocumentIdentifier { uri, version: None },
        edits: vec![OneOf::Left(TextEdit { range, new_text: report })],
    }));
    Some(CodeActionOrCommand::CodeAction(CodeAction {
        title: "Write Ren'Py script statistics to script_stats.md".into(),
        kind: Some(CodeActionKind::SOURCE),
        edit: Some(WorkspaceEdit {
            document_changes: Some(DocumentChanges::Operations(operations)),
            ..Default::default()
        }),
        ..Default::default()
    }))
}

/// Re-read image files of every project after something under them changed.
fn rescan_images(index: &mut Index, projects: &[PathBuf]) {
    let files: Vec<images::ImageFile> =
        projects.iter().flat_map(|project| images::scan_project_images(project)).collect();
    index.set_image_files(&files);
}

fn main() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    match std::env::args().nth(1).as_deref() {
        Some("dap") => return dap::run(),
        Some("--version") | Some("-V") => {
            println!("renpy-language-server {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        Some(other) => {
            eprintln!(
                "renpy-language-server: unknown argument '{other}' \
                 (no arguments = language server, 'dap' = debug adapter)"
            );
            std::process::exit(2);
        }
        None => {}
    }
    run_lsp()
}

fn run_lsp() -> Result<(), Box<dyn std::error::Error + Sync + Send>> {
    let (connection, io_threads) = Connection::stdio();

    let capabilities = serde_json::to_value(ServerCapabilities {
        text_document_sync: Some(TextDocumentSyncCapability::Options(TextDocumentSyncOptions {
            open_close: Some(true),
            change: Some(TextDocumentSyncKind::FULL),
            save: Some(TextDocumentSyncSaveOptions::Supported(true)),
            ..Default::default()
        })),
        definition_provider: Some(OneOf::Left(true)),
        workspace_symbol_provider: Some(OneOf::Left(true)),
        hover_provider: Some(HoverProviderCapability::Simple(true)),
        completion_provider: Some(CompletionOptions {
            trigger_characters: Some(vec![".".to_string()]),
            ..Default::default()
        }),
        references_provider: Some(OneOf::Left(true)),
        rename_provider: Some(OneOf::Left(true)),
        signature_help_provider: Some(SignatureHelpOptions {
            trigger_characters: Some(vec!["(".to_string(), ",".to_string()]),
            retrigger_characters: None,
            work_done_progress_options: Default::default(),
        }),
        code_action_provider: Some(CodeActionProviderCapability::Options(CodeActionOptions {
            code_action_kinds: Some(vec![CodeActionKind::QUICKFIX, CodeActionKind::SOURCE]),
            ..Default::default()
        })),
        color_provider: Some(ColorProviderCapability::Simple(true)),
        ..Default::default()
    })?;
    let init_value = connection.initialize(capabilities)?;
    let init: InitializeParams = serde_json::from_value(init_value)?;

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(folders) = &init.workspace_folders {
        roots.extend(folders.iter().filter_map(|f| f.uri.to_file_path().ok()));
    }
    #[allow(deprecated)]
    if roots.is_empty() {
        if let Some(root) = init.root_uri.as_ref().and_then(|u| u.to_file_path().ok()) {
            roots.push(root);
        }
    }

    let options = init.initialization_options.unwrap_or(serde_json::Value::Null);
    let sdk_override = options.get("sdk").and_then(|v| v.as_str()).map(PathBuf::from);
    if let Some(path) = &sdk_override {
        if !renpy_cli::is_sdk_dir(path) {
            eprintln!(
                "renpy-language-server: configured sdk '{}' does not look like a Ren'Py SDK \
                 (no renpy.py inside); ignoring it",
                path.display()
            );
        }
    }
    let projects: Vec<PathBuf> = roots.iter().filter_map(|root| renpy_cli::find_project(root)).collect();
    // The script statistics action creates a file, which needs the client to
    // apply resource operations.
    let can_create_files = init
        .capabilities
        .workspace
        .as_ref()
        .and_then(|w| w.workspace_edit.as_ref())
        .is_some_and(|edit| {
            edit.document_changes == Some(true)
                && edit
                    .resource_operations
                    .as_ref()
                    .is_some_and(|ops| ops.contains(&lsp_types::ResourceOperationKind::Create))
        });
    let can_watch_files = init
        .capabilities
        .workspace
        .as_ref()
        .and_then(|w| w.did_change_watched_files.as_ref())
        .and_then(|c| c.dynamic_registration)
        .unwrap_or(false);
    let (lint_tx, lint_rx) = crossbeam_channel::unbounded();
    let mut lint = LintRunner {
        enabled: options.get("lint").and_then(|v| v.as_bool()).unwrap_or(true),
        sdk: sdk_override.filter(|p| renpy_cli::is_sdk_dir(p)).or_else(renpy_cli::find_sdk),
        project: projects.first().cloned(),
        running: false,
        pending: false,
        tx: lint_tx,
    };
    let mut lint_diags: HashMap<Url, Vec<Diagnostic>> = HashMap::new();

    let builtin_docs = BuiltinDocs::load();
    let mut index = Index::default();
    let mut published: std::collections::HashSet<Url> = Default::default();
    let mut file_count = 0usize;
    for root in &roots {
        let mut files = Vec::new();
        walk_workspace(root, &mut files);
        for path in files {
            if let (Ok(text), Ok(uri)) = (std::fs::read_to_string(&path), Url::from_file_path(&path)) {
                index.index_file(&uri, &text);
                file_count += 1;
            }
        }
    }
    rescan_images(&mut index, &projects);
    eprintln!(
        "renpy-language-server: indexed {} symbols from {} files and {} image files in {} workspace root(s); {} built-in API docs loaded (Ren'Py {})",
        index.defs.len(),
        file_count,
        index.image_files.len(),
        roots.len(),
        builtin_docs.len(),
        RENPY_DOCS_VERSION
    );
    if !lint.enabled {
        eprintln!("renpy-language-server: renpy lint disabled by settings");
    } else {
        match (&lint.sdk, &lint.project) {
            (Some(sdk), Some(project)) => eprintln!(
                "renpy-language-server: renpy lint on save — SDK {} / project {}",
                sdk.display(),
                project.display()
            ),
            (None, _) => eprintln!(
                "renpy-language-server: renpy lint inactive — no Ren'Py SDK found \
                 (set initialization_options.sdk or $RENPY_SDK)"
            ),
            (_, None) => eprintln!(
                "renpy-language-server: renpy lint inactive — workspace has no game/ directory"
            ),
        }
    }

    // `Connection::initialize` has already consumed the client's `initialized`
    // notification, so post-handshake work starts here, not in the loop.
    publish_diagnostics(&connection, &index, &lint_diags, &mut published)?;
    lint.request();
    if can_watch_files {
        // Scripts edited outside Zed, and image files added or renamed.
        let watchers: Vec<serde_json::Value> = ["**/*.rpy", "**/*.rpym", "**/game/images/**"]
            .iter()
            .map(|glob| serde_json::json!({ "globPattern": glob }))
            .collect();
        connection.sender.send(Message::Request(lsp_server::Request {
            id: lsp_server::RequestId::from("renpy-watch-files".to_string()),
            method: "client/registerCapability".into(),
            params: serde_json::json!({
                "registrations": [{
                    "id": "renpy-watch-files",
                    "method": "workspace/didChangeWatchedFiles",
                    "registerOptions": { "watchers": watchers },
                }]
            }),
        }))?;
    }

    enum Incoming {
        Client(Message),
        Lint(Result<String, String>),
    }
    loop {
        let received = crossbeam_channel::select! {
            recv(connection.receiver) -> msg => match msg {
                Ok(msg) => Incoming::Client(msg),
                Err(_) => break,
            },
            recv(lint_rx) -> outcome => match outcome {
                Ok(outcome) => Incoming::Lint(outcome),
                Err(_) => continue,
            },
        };
        let msg = match received {
            Incoming::Lint(outcome) => {
                lint.running = false;
                match outcome {
                    Ok(report) => {
                        if let Some(project) = lint.project.clone() {
                            lint_diags = lint_diagnostics(&project, &report);
                            publish_diagnostics(&connection, &index, &lint_diags, &mut published)?;
                        }
                    }
                    Err(message) => eprintln!("renpy-language-server: {message}"),
                }
                if lint.pending {
                    lint.pending = false;
                    lint.request();
                }
                continue;
            }
            Incoming::Client(msg) => msg,
        };
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    break;
                }
                match req.method.as_str() {
                    "textDocument/definition" => {
                        let resp = match serde_json::from_value::<GotoDefinitionParams>(req.params) {
                            Ok(params) => {
                                let doc = &params.text_document_position_params;
                                let uri = &doc.text_document.uri;
                                let text = index.open_docs.get(uri).cloned().or_else(|| {
                                    uri.to_file_path().ok().and_then(|p| std::fs::read_to_string(p).ok())
                                });
                                let locations = text
                                    .and_then(|t| word_at(&t, doc.position))
                                    .map(|word| index.lookup(&word))
                                    .unwrap_or_default();
                                let result = if locations.is_empty() {
                                    serde_json::Value::Null
                                } else {
                                    serde_json::to_value(locations)?
                                };
                                Response::new_ok(req.id, result)
                            }
                            Err(err) => Response::new_err(req.id, -32602, err.to_string()),
                        };
                        connection.sender.send(Message::Response(resp))?;
                    }
                    "textDocument/completion" => {
                        let resp = match serde_json::from_value::<CompletionParams>(req.params) {
                            Ok(params) => {
                                let doc_pos = &params.text_document_position;
                                let uri = &doc_pos.text_document.uri;
                                let text = index.open_docs.get(uri).cloned().or_else(|| {
                                    uri.to_file_path().ok().and_then(|p| std::fs::read_to_string(p).ok())
                                });
                                let mut items: Vec<CompletionItem> = Vec::new();
                                if let Some(text) = text {
                                    let line =
                                        text.lines().nth(doc_pos.position.line as usize).unwrap_or("");
                                    let byte = utf16_col_to_byte(line, doc_pos.position.character);
                                    match completion_context(&line[..byte]) {
                                        CompletionContext::Labels => {
                                            items = index.def_completions(|k| k == SymbolKind::FUNCTION);
                                        }
                                        CompletionContext::Screens => {
                                            items = index.def_completions(|k| k == SymbolKind::CLASS);
                                        }
                                        CompletionContext::Images { after_at } => {
                                            if after_at {
                                                items = index.def_completions(|k| k == SymbolKind::METHOD);
                                                items.extend(position_completions());
                                            } else {
                                                items = index.def_completions(|k| k == SymbolKind::CONSTANT);
                                            }
                                        }
                                        CompletionContext::LineStart => {
                                            items = keyword_completions();
                                            items.extend(index.speaker_completions());
                                        }
                                        CompletionContext::General => {
                                            items = builtin_docs.completions();
                                            items.extend(
                                                index.def_completions(|k| k == SymbolKind::VARIABLE),
                                            );
                                        }
                                    }
                                }
                                Response::new_ok(
                                    req.id,
                                    serde_json::to_value(CompletionResponse::Array(items))?,
                                )
                            }
                            Err(err) => Response::new_err(req.id, -32602, err.to_string()),
                        };
                        connection.sender.send(Message::Response(resp))?;
                    }
                    "textDocument/hover" => {
                        let resp = match serde_json::from_value::<HoverParams>(req.params) {
                            Ok(params) => {
                                let doc_pos = &params.text_document_position_params;
                                let uri = &doc_pos.text_document.uri;
                                let text = index.open_docs.get(uri).cloned().or_else(|| {
                                    uri.to_file_path().ok().and_then(|p| std::fs::read_to_string(p).ok())
                                });
                                let markdown = text
                                    .and_then(|t| word_at(&t, doc_pos.position))
                                    .and_then(|word| {
                                        let project = index.lookup_defs(&word).map(|defs| {
                                            let mut text = image_file_hover(&defs[0])
                                                .unwrap_or_else(|| project_hover_markdown(defs, &roots));
                                            if let Some(stats) = stats_hover(&index, &word) {
                                                text.push_str("\n\n");
                                                text.push_str(&stats);
                                            }
                                            text
                                        });
                                        project.or_else(|| builtin_docs.hover_markdown(&word))
                                    });
                                let result = match markdown {
                                    Some(value) => serde_json::to_value(Hover {
                                        contents: HoverContents::Markup(MarkupContent {
                                            kind: MarkupKind::Markdown,
                                            value,
                                        }),
                                        range: None,
                                    })?,
                                    None => serde_json::Value::Null,
                                };
                                Response::new_ok(req.id, result)
                            }
                            Err(err) => Response::new_err(req.id, -32602, err.to_string()),
                        };
                        connection.sender.send(Message::Response(resp))?;
                    }
                    "textDocument/references" => {
                        let resp = match serde_json::from_value::<ReferenceParams>(req.params) {
                            Ok(params) => {
                                let doc_pos = &params.text_document_position;
                                let uri = &doc_pos.text_document.uri;
                                let text = index.open_docs.get(uri).cloned().or_else(|| {
                                    uri.to_file_path().ok().and_then(|p| std::fs::read_to_string(p).ok())
                                });
                                let mut locations: Vec<Location> = Vec::new();
                                if let Some(word) = text.and_then(|t| word_at(&t, doc_pos.position)) {
                                    let is_label = index.defs.get(&word).is_some_and(|defs| {
                                        defs.iter().any(|d| d.kind == SymbolKind::FUNCTION)
                                    });
                                    if is_label {
                                        locations = index.label_references(&word);
                                        if params.context.include_declaration {
                                            if let Some(defs) = index.lookup_defs(&word) {
                                                locations.extend(
                                                    defs.iter()
                                                        .filter(|d| d.kind == SymbolKind::FUNCTION)
                                                        .map(|d| Location {
                                                            uri: d.uri.clone(),
                                                            range: d.range,
                                                        }),
                                                );
                                            }
                                        }
                                    } else {
                                        // Textual scan naturally includes definition lines.
                                        locations = index.textual_references(&word);
                                    }
                                    locations.sort_by(|a, b| {
                                        (a.uri.as_str(), a.range.start.line, a.range.start.character)
                                            .cmp(&(b.uri.as_str(), b.range.start.line, b.range.start.character))
                                    });
                                }
                                let result = if locations.is_empty() {
                                    serde_json::Value::Null
                                } else {
                                    serde_json::to_value(locations)?
                                };
                                Response::new_ok(req.id, result)
                            }
                            Err(err) => Response::new_err(req.id, -32602, err.to_string()),
                        };
                        connection.sender.send(Message::Response(resp))?;
                    }
                    "textDocument/rename" => {
                        let resp = match serde_json::from_value::<RenameParams>(req.params) {
                            Ok(params) => {
                                let doc_pos = &params.text_document_position;
                                let uri = &doc_pos.text_document.uri;
                                let text = index.open_docs.get(uri).cloned().or_else(|| {
                                    uri.to_file_path().ok().and_then(|p| std::fs::read_to_string(p).ok())
                                });
                                match text.and_then(|t| word_at(&t, doc_pos.position)) {
                                    Some(word) => match rename_edits(&index, &word, &params.new_name) {
                                        Ok(changes) => Response::new_ok(
                                            req.id,
                                            serde_json::to_value(WorkspaceEdit {
                                                changes: Some(changes),
                                                ..Default::default()
                                            })?,
                                        ),
                                        Err(message) => Response::new_err(req.id, -32803, message),
                                    },
                                    None => Response::new_err(
                                        req.id,
                                        -32803,
                                        "nothing to rename here".to_string(),
                                    ),
                                }
                            }
                            Err(err) => Response::new_err(req.id, -32602, err.to_string()),
                        };
                        connection.sender.send(Message::Response(resp))?;
                    }
                    "textDocument/signatureHelp" => {
                        let resp = match serde_json::from_value::<SignatureHelpParams>(req.params) {
                            Ok(params) => {
                                let doc_pos = &params.text_document_position_params;
                                let uri = &doc_pos.text_document.uri;
                                let text = index.open_docs.get(uri).cloned().or_else(|| {
                                    uri.to_file_path().ok().and_then(|p| std::fs::read_to_string(p).ok())
                                });
                                let help = text.and_then(|t| {
                                    signature_help(&index, &builtin_docs, &t, doc_pos.position)
                                });
                                Response::new_ok(req.id, serde_json::to_value(help)?)
                            }
                            Err(err) => Response::new_err(req.id, -32602, err.to_string()),
                        };
                        connection.sender.send(Message::Response(resp))?;
                    }
                    "textDocument/codeAction" => {
                        let resp = match serde_json::from_value::<CodeActionParams>(req.params) {
                            Ok(params) => {
                                let uri = &params.text_document.uri;
                                let text = index.open_docs.get(uri).cloned().or_else(|| {
                                    uri.to_file_path().ok().and_then(|p| std::fs::read_to_string(p).ok())
                                });
                                let mut actions = text
                                    .map(|t| code_actions(&index, uri, params.range, &t))
                                    .unwrap_or_default();
                                let wants = |kind: &CodeActionKind| {
                                    params.context.only.as_ref().is_none_or(|only| {
                                        only.iter().any(|o| kind.as_str().starts_with(o.as_str()))
                                    })
                                };
                                actions.retain(|action| match action {
                                    CodeActionOrCommand::CodeAction(a) => a.kind.as_ref().is_none_or(wants),
                                    CodeActionOrCommand::Command(_) => true,
                                });
                                if can_create_files && wants(&CodeActionKind::SOURCE) {
                                    if let Some(project) = projects.first() {
                                        actions.extend(stats_action(&index, &roots, project));
                                    }
                                }
                                Response::new_ok(req.id, serde_json::to_value(actions)?)
                            }
                            Err(err) => Response::new_err(req.id, -32602, err.to_string()),
                        };
                        connection.sender.send(Message::Response(resp))?;
                    }
                    "textDocument/documentColor" => {
                        let resp = match serde_json::from_value::<DocumentColorParams>(req.params) {
                            Ok(params) => {
                                let uri = &params.text_document.uri;
                                let text = index.open_docs.get(uri).cloned().or_else(|| {
                                    uri.to_file_path().ok().and_then(|p| std::fs::read_to_string(p).ok())
                                });
                                let found: Vec<ColorInformation> = colors::find_colors(&text.unwrap_or_default())
                                    .into_iter()
                                    .map(|c| ColorInformation {
                                        range: Range {
                                            start: Position { line: c.line, character: c.start },
                                            end: Position { line: c.line, character: c.end },
                                        },
                                        color: Color {
                                            red: c.rgba[0],
                                            green: c.rgba[1],
                                            blue: c.rgba[2],
                                            alpha: c.rgba[3],
                                        },
                                    })
                                    .collect();
                                Response::new_ok(req.id, serde_json::to_value(found)?)
                            }
                            Err(err) => Response::new_err(req.id, -32602, err.to_string()),
                        };
                        connection.sender.send(Message::Response(resp))?;
                    }
                    "textDocument/colorPresentation" => {
                        let resp = match serde_json::from_value::<ColorPresentationParams>(req.params) {
                            Ok(params) => {
                                let c = params.color;
                                let label = colors::presentation([c.red, c.green, c.blue, c.alpha]);
                                let presentation = ColorPresentation {
                                    text_edit: Some(TextEdit { range: params.range, new_text: label.clone() }),
                                    label,
                                    additional_text_edits: None,
                                };
                                Response::new_ok(req.id, serde_json::to_value(vec![presentation])?)
                            }
                            Err(err) => Response::new_err(req.id, -32602, err.to_string()),
                        };
                        connection.sender.send(Message::Response(resp))?;
                    }
                    "workspace/symbol" => {
                        let resp = match serde_json::from_value::<WorkspaceSymbolParams>(req.params) {
                            Ok(params) => {
                                let query = params.query.to_lowercase();
                                let mut symbols: Vec<SymbolInformation> = Vec::new();
                                'outer: for (name, defs) in &index.defs {
                                    if !query.is_empty() && !name.to_lowercase().contains(&query) {
                                        continue;
                                    }
                                    for def in defs {
                                        #[allow(deprecated)]
                                        symbols.push(SymbolInformation {
                                            name: name.clone(),
                                            kind: def.kind,
                                            tags: None,
                                            deprecated: None,
                                            location: Location {
                                                uri: def.uri.clone(),
                                                range: def.range,
                                            },
                                            container_name: None,
                                        });
                                        if symbols.len() >= 256 {
                                            break 'outer;
                                        }
                                    }
                                }
                                symbols.sort_by(|a, b| a.name.cmp(&b.name));
                                Response::new_ok(req.id, serde_json::to_value(symbols)?)
                            }
                            Err(err) => Response::new_err(req.id, -32602, err.to_string()),
                        };
                        connection.sender.send(Message::Response(resp))?;
                    }
                    _ => {
                        let resp = Response::new_err(
                            req.id,
                            -32601,
                            format!("method not supported: {}", req.method),
                        );
                        connection.sender.send(Message::Response(resp))?;
                    }
                }
            }
            Message::Notification(note) => match note.method.as_str() {
                "textDocument/didOpen" => {
                    if let Ok(params) = serde_json::from_value::<DidOpenTextDocumentParams>(note.params) {
                        let uri = params.text_document.uri;
                        index.index_file(&uri, &params.text_document.text);
                        index.open_docs.insert(uri, params.text_document.text);
                        publish_diagnostics(&connection, &index, &lint_diags, &mut published)?;
                    }
                }
                "textDocument/didChange" => {
                    if let Ok(params) = serde_json::from_value::<DidChangeTextDocumentParams>(note.params) {
                        // FULL sync: the last change carries the entire document.
                        if let Some(change) = params.content_changes.into_iter().last() {
                            let uri = params.text_document.uri;
                            index.index_file(&uri, &change.text);
                            index.open_docs.insert(uri, change.text);
                            publish_diagnostics(&connection, &index, &lint_diags, &mut published)?;
                        }
                    }
                }
                "textDocument/didSave" => {
                    lint.request();
                }
                "workspace/didChangeWatchedFiles" => {
                    if let Ok(params) = serde_json::from_value::<DidChangeWatchedFilesParams>(note.params) {
                        let mut images_changed = false;
                        for change in params.changes {
                            let Ok(path) = change.uri.to_file_path() else { continue };
                            if is_renpy_file(&path) {
                                if index.open_docs.contains_key(&change.uri) {
                                    continue; // the buffer is the source of truth
                                }
                                match (change.typ, std::fs::read_to_string(&path)) {
                                    (FileChangeType::DELETED, _) | (_, Err(_)) => index.remove_file(&change.uri),
                                    (_, Ok(text)) => index.index_file(&change.uri, &text),
                                }
                            } else {
                                // an image, or a directory of them
                                images_changed = true;
                            }
                        }
                        if images_changed {
                            rescan_images(&mut index, &projects);
                        }
                        publish_diagnostics(&connection, &index, &lint_diags, &mut published)?;
                    }
                }
                "textDocument/didClose" => {
                    if let Ok(params) = serde_json::from_value::<DidCloseTextDocumentParams>(note.params) {
                        let uri = params.text_document.uri;
                        index.open_docs.remove(&uri);
                        // Fall back to on-disk contents (buffer may have been discarded).
                        match uri.to_file_path().ok().and_then(|p| std::fs::read_to_string(p).ok()) {
                            Some(text) => index.index_file(&uri, &text),
                            None => index.remove_file(&uri),
                        }
                        publish_diagnostics(&connection, &index, &lint_diags, &mut published)?;
                    }
                }
                _ => {}
            },
            Message::Response(_) => {}
        }
    }

    // The writer thread only stops once the connection's channels are dropped.
    drop(connection);
    io_threads.join()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(line: &str) -> Vec<(String, SymbolKind)> {
        parse_definitions(line).into_iter().map(|(n, _, _, k)| (n, k)).collect()
    }

    #[test]
    fn parses_definition_statements() {
        assert_eq!(names("label start:"), vec![("start".into(), SymbolKind::FUNCTION)]);
        assert_eq!(names("label shop(x=1):"), vec![("shop".into(), SymbolKind::FUNCTION)]);
        assert_eq!(names("    label .local:"), vec![(".local".into(), SymbolKind::FUNCTION)]);
        assert_eq!(names("menu trade_menu:"), vec![("trade_menu".into(), SymbolKind::FUNCTION)]);
        assert_eq!(names("menu:"), Vec::<(String, SymbolKind)>::new());
        assert_eq!(
            names("define e = Character(\"Eileen\")"),
            vec![("e".into(), SymbolKind::VARIABLE)]
        );
        assert_eq!(
            names("define config.name = \"Demo\""),
            vec![("config.name".into(), SymbolKind::VARIABLE)]
        );
        assert_eq!(names("default points = 0"), vec![("points".into(), SymbolKind::VARIABLE)]);
        assert_eq!(
            names("image eileen happy = \"eileen_happy.png\""),
            vec![
                ("eileen happy".into(), SymbolKind::CONSTANT),
                ("eileen".into(), SymbolKind::CONSTANT)
            ]
        );
        assert_eq!(names("image bg room:"), vec![
            ("bg room".into(), SymbolKind::CONSTANT),
            ("bg".into(), SymbolKind::CONSTANT)
        ]);
        assert_eq!(names("screen inventory(items):"), vec![("inventory".into(), SymbolKind::CLASS)]);
        assert_eq!(names("transform slide_in:"), vec![("slide_in".into(), SymbolKind::METHOD)]);
        assert_eq!(names("style big_text:"), vec![("big_text".into(), SymbolKind::PROPERTY)]);
        // Non-definitions must not be indexed.
        assert_eq!(names("    jump shop"), Vec::<(String, SymbolKind)>::new());
        assert_eq!(names("    e \"label start: is not a definition\""), Vec::<(String, SymbolKind)>::new());
        assert_eq!(names("# label commented:"), Vec::<(String, SymbolKind)>::new());
    }

    #[test]
    fn word_extraction() {
        let text = "label start:\n    jump shop\n    e \"hi\"\n";
        assert_eq!(word_at(text, Position { line: 1, character: 10 }), Some("shop".into()));
        assert_eq!(word_at(text, Position { line: 1, character: 13 }), Some("shop".into()));
        assert_eq!(word_at(text, Position { line: 2, character: 4 }), Some("e".into()));
        assert_eq!(word_at(text, Position { line: 0, character: 0 }), Some("label".into()));
    }

    #[test]
    fn parses_references_and_call_from_definitions() {
        assert_eq!(parse_reference("    jump shop"), Some(("shop".into(), 9, 13)));
        assert_eq!(parse_reference("    call shop(1) from _c1"), Some(("shop".into(), 9, 13)));
        assert_eq!(parse_reference("    jump expression dest"), None);
        assert_eq!(parse_reference("    e \"jump nowhere\""), None);
        assert_eq!(parse_reference("label start:"), None);
        assert_eq!(
            parse_reference("    call screen dice_roll(15, 0, \"test_end\", \"\") with Dissolve(0.3)"),
            None
        );
        assert_eq!(
            names("    call shop from _call_shop"),
            vec![("_call_shop".into(), SymbolKind::FUNCTION)]
        );
        assert_eq!(
            names("    call shop(points) from _c1  # note"),
            vec![("_c1".into(), SymbolKind::FUNCTION)]
        );
        // No phantom definitions from `call screen` or from-inside-a-string.
        assert_eq!(names("    call screen dice_roll(15)"), Vec::<(String, SymbolKind)>::new());
        assert_eq!(
            names("    call shop(\"back from war\")"),
            Vec::<(String, SymbolKind)>::new()
        );
    }

    #[test]
    fn diagnostics_for_missing_and_duplicate_labels() {
        let mut index = Index::default();
        let a = Url::parse("file:///tmp/a.rpy").unwrap();
        let b = Url::parse("file:///tmp/b.rpy").unwrap();
        // `default start` shares the name with the two labels; only labels count.
        index.index_file(
            &a,
            "default start = 0\nlabel start:\n    jump shop\n    jump missing\n    jump .local\n",
        );
        index.index_file(&b, "label shop:\n    return\nlabel start:\n    return\n");
        let diags = index.compute_diagnostics();
        let a_diags = &diags[&a];
        assert!(a_diags.iter().any(|d| d.message.contains("'missing' is not defined")));
        assert!(!a_diags.iter().any(|d| d.message.contains(".local")));
        assert!(a_diags.iter().any(|d| d.message.contains("defined 2 times")));
        assert!(diags[&b].iter().any(|d| d.message.contains("defined 2 times")));
        assert!(!diags[&b].iter().any(|d| d.severity == Some(DiagnosticSeverity::ERROR)));
        // Fixing the duplicate clears it.
        index.index_file(&b, "label shop:\n    return\n");
        let diags = index.compute_diagnostics();
        assert!(!diags.contains_key(&b));
        assert!(!diags[&a].iter().any(|d| d.message.contains("defined 2 times")));
    }

    #[test]
    fn references_and_rename() {
        let mut index = Index::default();
        let a = Url::parse("file:///tmp/a.rpy").unwrap();
        let b = Url::parse("file:///tmp/b.rpy").unwrap();
        let a_text = "define e = Character(\"Eileen\")\nlabel start:\n    e \"hi\"\n    jump shop\n";
        let b_text = "label shop:\n    e \"welcome\"\n    call shop from _c1\n";
        index.index_file(&a, a_text);
        index.index_file(&b, b_text);
        index.open_docs.insert(a.clone(), a_text.to_string());
        index.open_docs.insert(b.clone(), b_text.to_string());

        // Precise label references: the jump in a + the self-call in b.
        assert_eq!(index.label_references("shop").len(), 2);

        // Textual speaker references: define line + two say lines; nothing
        // matched inside words like "Eileen", "define", or "welcome".
        assert_eq!(index.textual_references("e").len(), 3);

        let changes = rename_edits(&index, "shop", "bazaar").unwrap();
        assert_eq!(changes[&b].len(), 2); // definition + self-call
        assert_eq!(changes[&a].len(), 1); // the jump
        assert!(rename_edits(&index, "shop", "start").unwrap_err().contains("already exists"));
        assert!(rename_edits(&index, "e", "f").unwrap_err().contains("labels only"));
        assert!(rename_edits(&index, "shop", "9bad").unwrap_err().contains("not a valid"));
        assert!(rename_edits(&index, "shop", "so bad").unwrap_err().contains("not a valid"));
    }

    #[test]
    fn completion_contexts() {
        assert!(matches!(completion_context("    jump s"), CompletionContext::Labels));
        assert!(matches!(completion_context("    call "), CompletionContext::Labels));
        assert!(matches!(completion_context("    call screen inv"), CompletionContext::Screens));
        assert!(matches!(
            completion_context("    show eileen at r"),
            CompletionContext::Images { after_at: true }
        ));
        assert!(matches!(
            completion_context("    scene bg "),
            CompletionContext::Images { after_at: false }
        ));
        assert!(matches!(completion_context("    sc"), CompletionContext::LineStart));
        assert!(matches!(completion_context(""), CompletionContext::LineStart));
        assert!(matches!(completion_context("define e = Cha"), CompletionContext::General));
        assert!(matches!(
            completion_context("    call shop from _c"),
            CompletionContext::General
        ));
    }

    #[test]
    fn doc_comments_attach_to_definitions() {
        let mut index = Index::default();
        let uri = Url::parse("file:///tmp/doc.rpy").unwrap();
        index.index_file(
            &uri,
            "# The shopkeeper.\n# Sells things.\ndefine s = Character(\"Shopkeeper\")\n\n# Unattached (blank line below).\n\nlabel start:\n",
        );
        let defs = index.lookup_defs("s").unwrap();
        assert_eq!(defs[0].detail, "define s = Character(\"Shopkeeper\")");
        assert_eq!(defs[0].doc.as_deref(), Some("The shopkeeper.\nSells things."));
        let defs = index.lookup_defs("start").unwrap();
        assert_eq!(defs[0].doc, None);
    }

    #[test]
    fn builtin_docs_cover_common_api() {
        let docs = BuiltinDocs::load();
        assert!(docs.len() > 1000, "expected >1000 bundled entries, got {}", docs.len());
        let transform = docs.hover_markdown("Transform").unwrap();
        assert!(transform.contains("Transform(child=None"), "{transform}");
        assert!(transform.contains("docs from Ren'Py 8.3"), "{transform}");
        assert!(
            transform.contains("(https://www.renpy.org/doc/html/transforms.html#Transform)"),
            "{transform}"
        );
        let character = docs.hover_markdown("Character").unwrap();
        assert!(character.contains("Character(name="), "{character}");
        assert!(docs.hover_markdown("definitely_not_a_builtin_xyz").is_none());
    }

    #[test]
    fn unknown_images_are_flagged_with_suggestions() {
        let mut index = Index::default();
        let a = Url::parse("file:///tmp/a.rpy").unwrap();
        index.index_file(
            &a,
            "image eileen happy = \"e.png\"\nlayeredimage lucy:\n    always:\n        \"lucy\"\nlabel start:\n    scene bg room\n    show eileen happy\n    show eilen sad\n    show lucy\n    show black\n    show ghost as spooky\n    hide spooky\n    show screen hud\n    $ renpy.image(\"bg night\", Solid(\"#000\"))\n    scene bg night\n",
        );
        index.set_image_files(&[images::ImageFile {
            name: "bg room".into(),
            path: PathBuf::from("/tmp/game/images/bg room.png"),
        }]);
        let diags = index.compute_diagnostics();
        let messages: Vec<&str> = diags[&a].iter().map(|d| d.message.as_str()).collect();
        assert_eq!(messages.len(), 2, "{messages:?}");
        assert!(messages[0].starts_with("image 'eilen' is not defined") && messages[0].ends_with("(did you mean 'eileen'?)"));
        assert!(messages[1].starts_with("image 'ghost' is not defined"));
        // The image file is a definition with a path, and resolves the tag.
        let defs = index.lookup_defs("bg room").unwrap();
        assert_eq!(defs[0].detail, "image bg room = \"images/bg room.png\"");
        assert!(index.lookup_defs("bg").is_some());
        // Rescanning replaces old image files.
        index.set_image_files(&[]);
        assert!(index.lookup_defs("bg room").is_none());
        // A computed renpy.image() name makes every tag possible.
        let b = Url::parse("file:///tmp/b.rpy").unwrap();
        index.index_file(&b, "init python:\n    for n in names:\n        renpy.image(n, Solid(\"#000\"))\n");
        assert!(!index.compute_diagnostics().contains_key(&a));
    }

    #[test]
    fn quick_fixes_for_labels_and_images() {
        let mut index = Index::default();
        let a = Url::parse("file:///tmp/a.rpy").unwrap();
        let text = "image eileen = \"e.png\"\nlabel start:\n    jump shopp\n    jump nowhere\n    show eilen\nlabel shop:\n    return\nlabel shop:\n    return";
        index.index_file(&a, text);
        let at = |line: u32| Range {
            start: Position { line, character: 0 },
            end: Position { line, character: 99 },
        };
        let titles = |line: u32| -> Vec<String> {
            code_actions(&index, &a, at(line), text)
                .into_iter()
                .map(|action| match action {
                    CodeActionOrCommand::CodeAction(a) => a.title,
                    CodeActionOrCommand::Command(c) => c.title,
                })
                .collect()
        };
        assert_eq!(titles(2), vec!["Change to 'shop'", "Create label 'shopp' at the end of this file"]);
        assert_eq!(titles(3), vec!["Create label 'nowhere' at the end of this file"]);
        assert_eq!(titles(4), vec!["Change to 'eileen'"]);
        assert_eq!(titles(5), vec!["Rename this label to 'shop_2'"]);
        assert!(titles(1).is_empty());
        // The new label goes after the last line, on a fresh line.
        let CodeActionOrCommand::CodeAction(create) = code_actions(&index, &a, at(3), text).remove(0) else {
            panic!()
        };
        let edit = &create.edit.unwrap().changes.unwrap()[&a][0];
        assert_eq!(edit.range.start, Position { line: 8, character: 10 });
        assert_eq!(edit.new_text, "\n\nlabel nowhere:\n    return\n");
    }

    #[test]
    fn parameter_hints_for_builtins_and_project_symbols() {
        let mut index = Index::default();
        let a = Url::parse("file:///tmp/a.rpy").unwrap();
        index.index_file(&a, "label shop(items, discount=0.1):\n    return\nscreen hud(gold):\n    text \"[gold]\"\n");
        let docs = BuiltinDocs::load();
        let help = |text: &str| {
            let line = text.lines().count() as u32 - 1;
            let character = text.lines().last().unwrap().encode_utf16().count() as u32;
            signature_help(&index, &docs, text, Position { line, character })
        };
        let h = help("define e = Character(\"Eileen\", ").unwrap();
        let sig = &h.signatures[0];
        assert!(sig.label.starts_with("Character(name="), "{}", sig.label);
        assert_eq!(h.active_parameter, Some(1));
        let h = help("    call shop([], discount=").unwrap();
        assert_eq!(h.signatures[0].label, "label shop(items, discount=0.1)");
        assert_eq!(h.active_parameter, Some(1));
        let ParameterLabel::LabelOffsets([start, end]) = h.signatures[0].parameters.as_ref().unwrap()[1].label else {
            panic!()
        };
        assert_eq!(&h.signatures[0].label[start as usize..end as usize], "discount=0.1");
        assert_eq!(help("    show screen hud(").unwrap().signatures[0].label, "screen hud(gold)");
        assert!(help("    $ unknown_function(").is_none());
    }

    #[test]
    fn dialogue_statistics_in_hover_and_report() {
        let mut index = Index::default();
        let a = Url::parse("file:///tmp/game/a.rpy").unwrap();
        let tl = Url::parse("file:///tmp/game/tl/french/a.rpy").unwrap();
        index.index_file(
            &a,
            "define e = Character(\"Eileen\")\nlabel start:\n    e \"One two three.\"\n    \"Four five.\"\nlabel other:\n    e \"Six.\"\n",
        );
        index.index_file(&tl, "translate french start_1:\n    e \"Un deux trois.\"\n");
        let hover = stats_hover(&index, "e").unwrap();
        assert_eq!(hover, "**Dialogue:** 2 lines · 4 words · 66.7% of all words");
        let hover = stats_hover(&index, "start").unwrap();
        assert!(hover.starts_with("**Dialogue in this label:** 2 lines · 5 words"), "{hover}");
        let report = stats_report(&index, &[PathBuf::from("/tmp")]);
        assert!(report.contains("3 lines of dialogue, 6 words"), "{report}");
        assert!(report.contains("| Eileen (`e`) | 2 | 4 | 66.7% |"), "{report}");
        assert!(report.contains("| `narrator` | 1 | 2 | 33.3% |"), "{report}");
        assert!(report.contains("| game/a.rpy | 3 | 6 |"), "{report}");
        assert!(report.contains("| `start` | 2 | 5 |"), "{report}");
    }

    #[test]
    fn index_and_lookup_across_files() {
        let mut index = Index::default();
        let a = Url::parse("file:///tmp/a.rpy").unwrap();
        let b = Url::parse("file:///tmp/b.rpy").unwrap();
        index.index_file(&a, "label start:\n    jump shop\n");
        index.index_file(&b, "label shop:\n    return\n");
        let locs = index.lookup("shop");
        assert_eq!(locs.len(), 1);
        assert_eq!(locs[0].uri, b);
        assert_eq!(locs[0].range.start.line, 0);
        // Re-indexing a file replaces its old symbols.
        index.index_file(&b, "label shop_v2:\n    return\n");
        assert!(index.lookup("shop").is_empty());
        assert_eq!(index.lookup("shop_v2").len(), 1);
    }
}
