//! Dialogue statistics: who says how much, where.
//!
//! Line-based like the indexer: a dialogue line is a string at statement
//! position, optionally preceded by a speaker and image attributes
//! (`e happy "Hi."`, `"Narration."`). Screens, transforms, styles, python and
//! other non-script blocks are skipped, as are menu choices (`"Go":`).

use std::collections::HashMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Dialogue {
    /// 0-based line of the statement.
    pub line: u32,
    /// The speaker variable (`e`), or None for narration.
    pub speaker: Option<String>,
    /// The label the line belongs to (the nearest `label` above it).
    pub label: Option<String>,
    pub words: usize,
}

/// Keywords that start a statement which is not dialogue.
const STATEMENT_KEYWORDS: &[&str] = &[
    "label", "menu", "jump", "call", "return", "pass", "if", "elif", "else", "while", "for",
    "init", "python", "define", "default", "image", "scene", "show", "hide", "with", "at",
    "play", "queue", "stop", "voice", "pause", "window", "camera", "screen", "transform",
    "style", "translate", "nvl", "layeredimage", "testcase", "old", "new", "rpy", "use",
    "text", "textbutton", "add", "key", "timer", "on",
];

/// Block openers whose bodies hold no dialogue.
const NON_SCRIPT_BLOCKS: &[&str] = &[
    "screen", "transform", "style", "python", "layeredimage", "image", "show", "scene",
    "camera", "testcase",
];

fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

fn first_word(s: &str) -> &str {
    let end = s
        .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
        .unwrap_or(s.len());
    &s[..end]
}

/// Does this line open a block whose body holds no dialogue?
fn opens_non_script_block(trimmed: &str) -> bool {
    let code = trimmed.split(" #").next().unwrap_or(trimmed).trim_end();
    if !code.ends_with(':') {
        return false;
    }
    let word = first_word(trimmed);
    if NON_SCRIPT_BLOCKS.contains(&word) {
        return true;
    }
    match word {
        // init python:, init -1 python early:, but not init:
        "init" => code.contains(" python"),
        // translate xx strings: / translate xx python: / translate xx style ...:
        "translate" => {
            let parts: Vec<&str> = code.trim_end_matches(':').split_whitespace().collect();
            matches!(parts.get(2), Some(&"strings") | Some(&"python") | Some(&"style"))
        }
        _ => false,
    }
}

/// The text of the string literal starting at `start` (its opening quote) and
/// the byte offset after it, continuing onto following lines while it is
/// open. Returns (text, line index where it ends, byte offset after it).
fn read_string(lines: &[&str], line_idx: usize, start: usize) -> Option<(String, usize, usize)> {
    let line = lines[line_idx];
    let rest = &line[start..];
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let delim: String = if rest.starts_with(&quote.to_string().repeat(3)) {
        quote.to_string().repeat(3)
    } else {
        quote.to_string()
    };
    let mut text = String::new();
    let mut idx = line_idx;
    let mut pos = start + delim.len();
    loop {
        let current = lines[idx];
        let bytes = current.as_bytes();
        let mut i = pos;
        // Byte-wise: only ASCII quotes are sliced at, which are always char
        // boundaries, whatever the dialogue's script.
        while i < bytes.len() {
            if bytes[i] == b'\\' {
                i += 2;
                continue;
            }
            if bytes[i..].starts_with(delim.as_bytes()) {
                text.push_str(&current[pos..i]);
                return Some((text, idx, i + delim.len()));
            }
            i += 1;
        }
        text.push_str(&current[pos.min(current.len())..]);
        text.push(' ');
        idx += 1;
        if idx >= lines.len() || idx > line_idx + 50 {
            return None;
        }
        pos = 0;
    }
}

/// Words a player reads: text tags (`{b}`) are not words, `[[`/`{{` escapes
/// are plain brackets.
pub fn count_words(text: &str) -> usize {
    let mut plain = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '{' {
            if chars.peek() == Some(&'{') {
                chars.next();
                plain.push('{');
                continue;
            }
            for d in chars.by_ref() {
                if d == '}' {
                    break;
                }
            }
            plain.push(' ');
            continue;
        }
        if c == '\\' {
            match chars.next() {
                Some('n') => plain.push(' '),
                Some(d) => plain.push(d),
                None => {}
            }
            continue;
        }
        plain.push(c);
    }
    plain.split_whitespace().filter(|w| w.chars().any(|c| c.is_alphanumeric())).count()
}

/// Is `prefix` a speaker plus image attributes: `e`, `e happy`, `e @ angry -sad`,
/// `mc.name`?
fn speaker_prefix(prefix: &str) -> Option<Option<String>> {
    let prefix = prefix.trim();
    if prefix.is_empty() {
        return Some(None);
    }
    let mut parts = prefix.split_whitespace();
    let who = parts.next()?;
    let ident = |s: &str| {
        !s.is_empty()
            && s.chars().next().is_some_and(|c| c.is_alphabetic() || c == '_')
            && s.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '.')
    };
    if !ident(who) || STATEMENT_KEYWORDS.contains(&who) {
        return None;
    }
    for attr in parts {
        let attr = attr.trim_start_matches('-');
        if attr != "@" && !ident(attr) {
            return None;
        }
    }
    Some(Some(who.to_string()))
}

/// All dialogue lines of one script.
pub fn scan(text: &str) -> Vec<Dialogue> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut label: Option<String> = None;
    // Indentation of the non-script block being skipped.
    let mut skip_until_indent: Option<usize> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        let indent = indent_of(line);
        if let Some(block_indent) = skip_until_indent {
            if indent > block_indent {
                i += 1;
                continue;
            }
            skip_until_indent = None;
        }
        if let Some(rest) = trimmed.strip_prefix("label ") {
            let name = first_word(rest.trim_start());
            if !name.is_empty() {
                label = Some(name.to_string());
            }
            i += 1;
            continue;
        }
        if opens_non_script_block(trimmed) {
            skip_until_indent = Some(indent);
            i += 1;
            continue;
        }
        let quote = trimmed.find(['"', '\'']);
        let Some(quote) = quote else {
            i += 1;
            continue;
        };
        let Some(speaker) = speaker_prefix(&trimmed[..quote]) else {
            i += 1;
            continue;
        };
        let start = indent + quote;
        let Some((said, end_line, after)) = read_string(&lines, i, start) else {
            i += 1;
            continue;
        };
        let tail = lines[end_line][after..].trim();
        let is_choice = tail.ends_with(':') && !tail.starts_with('#');
        if !is_choice && !tail.starts_with('"') && !tail.starts_with('\'') {
            out.push(Dialogue {
                line: i as u32,
                speaker,
                label: label.clone(),
                words: count_words(&said),
            });
        }
        i = end_line + 1;
    }
    out
}

/// `define e = Character("Eileen", ...)` / `Character(_("Eileen"))` → "Eileen".
pub fn character_display_name(define_line: &str) -> Option<String> {
    let pos = define_line.find("Character(")?;
    let mut rest = define_line[pos + "Character(".len()..].trim_start();
    if let Some(r) = rest.strip_prefix("_(") {
        rest = r.trim_start();
    }
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let body = &rest[1..];
    let end = body.find(quote)?;
    let name = body[..end].trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[derive(Default, Clone, Copy)]
pub struct Tally {
    pub lines: usize,
    pub words: usize,
}

impl Tally {
    pub fn add(&mut self, words: usize) {
        self.lines += 1;
        self.words += words;
    }
}

/// "1,234"
pub fn thousands(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// Reading time at 200 words a minute: "3 h 20 min", "12 min", "< 1 min".
pub fn reading_time(words: usize) -> String {
    let minutes = (words as f64 / 200.0).round() as usize;
    if minutes == 0 {
        "< 1 min".into()
    } else if minutes < 60 {
        format!("{minutes} min")
    } else {
        format!("{} h {} min", minutes / 60, minutes % 60)
    }
}

/// Speaker variable → tally, narration under "narrator".
pub fn by_speaker<'a>(dialogue: impl IntoIterator<Item = &'a Dialogue>) -> HashMap<String, Tally> {
    let mut out: HashMap<String, Tally> = HashMap::new();
    for d in dialogue {
        let who = d.speaker.clone().unwrap_or_else(|| "narrator".into());
        out.entry(who).or_default().add(d.words);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_dialogue_and_skips_everything_else() {
        let script = r#"define e = Character("Eileen")
label start:
    "It was a dark night."
    e "Hello, [name]! {b}Welcome{/b}."
    e happy -sad @ angry "Fine."
    menu:
        "Go left":
            jump left
        "Go right" if brave:
            e "Brave."
    $ e = 1
    show eileen happy
    "Multi
     line."
screen hud():
    text "Not dialogue"
    textbutton "No" action Return()
label .local:
    centered "A centered line."
init python:
    x = "not dialogue"
"#;
        let lines = scan(script);
        let summary: Vec<(u32, Option<&str>, Option<&str>, usize)> = lines
            .iter()
            .map(|d| (d.line, d.speaker.as_deref(), d.label.as_deref(), d.words))
            .collect();
        assert_eq!(
            summary,
            vec![
                (2, None, Some("start"), 5),
                (3, Some("e"), Some("start"), 3),
                (4, Some("e"), Some("start"), 1),
                (9, Some("e"), Some("start"), 1),
                (12, None, Some("start"), 2),
                (18, Some("centered"), Some(".local"), 3),
            ]
        );
    }

    #[test]
    fn handles_non_latin_dialogue() {
        let lines = scan("label start:\n    e \"Рада тебе \\\"знову\\\" бачити.\"\n    \"Ще\n     рядок.\"\n");
        let words: Vec<usize> = lines.iter().map(|d| d.words).collect();
        assert_eq!(words, vec![4, 2]);
    }

    #[test]
    fn counts_words_players_read() {
        assert_eq!(count_words("Hello, {b}big{/b} world!"), 3);
        assert_eq!(count_words("Wait...{w=0.5} what?"), 2);
        assert_eq!(count_words("{{literal} -- ok"), 2);
        assert_eq!(count_words("Line one\\nline two"), 4);
    }

    #[test]
    fn character_names_and_formatting() {
        assert_eq!(character_display_name("define e = Character(\"Eileen\", color=\"#fff\")"), Some("Eileen".into()));
        assert_eq!(character_display_name("define c = Character(_(\"Ciri\"))"), Some("Ciri".into()));
        assert_eq!(character_display_name("define n = Character(None, kind=nvl)"), None);
        assert_eq!(thousands(1234567), "1,234,567");
        assert_eq!(thousands(12), "12");
        assert_eq!(reading_time(40_000), "3 h 20 min");
        assert_eq!(reading_time(50), "< 1 min");
    }
}
