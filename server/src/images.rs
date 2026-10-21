//! Images Ren'Py defines without an `image` statement, and image references.
//!
//! Every image file under `game/images/` (recursively) becomes an image named
//! after the file: lowercased, extension and any `@2x`-style oversampling
//! suffix removed, spaces separating the tag from attributes. So
//! `images/bg/Bg Room@2.png` defines `bg room`. (renpy/common/00images.rpy)

use std::path::{Path, PathBuf};

/// Extensions Ren'Py scans for (`config.image_extensions` in 8.5).
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "avif", "svg"];

/// Images that exist in every project (renpy/common/00definitions.rpy).
pub const BUILTIN_IMAGES: &[&str] = &["black", "text", "vtext"];

pub struct ImageFile {
    /// The image name, e.g. `bg room`.
    pub name: String,
    pub path: PathBuf,
}

/// The image name Ren'Py gives a file, if it is an image file.
pub fn image_name_for(path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return None;
    }
    let stem = path.file_stem()?.to_str()?.to_lowercase();
    let base = stem.split('@').next().unwrap_or("");
    let name = base.split_whitespace().collect::<Vec<_>>().join(" ");
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// All automatically defined images of a project (`<project>/game/images`).
pub fn scan_project_images(project: &Path) -> Vec<ImageFile> {
    let mut out = Vec::new();
    walk(&project.join("game").join("images"), &mut out);
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out
}

fn walk(dir: &Path, out: &mut Vec<ImageFile>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else { continue };
        let path = entry.path();
        if file_type.is_dir() {
            if !entry.file_name().to_string_lossy().starts_with('.') {
                walk(&path, out);
            }
        } else if let Some(name) = image_name_for(&path) {
            out.push(ImageFile { name, path });
        }
    }
}

/// The image tag a `show`/`scene`/`hide` line refers to, as (tag, start, end)
/// byte offsets. None for lines that name no static image: `show screen`,
/// `show layer`, `show expression`, a bare `scene`, ...
pub fn parse_image_reference(line: &str) -> Option<(String, usize, usize)> {
    let indent = line.len() - line.trim_start().len();
    let rest = &line[indent..];
    let keyword = ["show ", "scene ", "hide "].into_iter().find(|kw| rest.starts_with(kw))?;
    let start = indent + keyword.len();
    let start = start + (line[start..].len() - line[start..].trim_start().len());
    let bytes = line.as_bytes();
    let mut end = start;
    while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_') {
        end += 1;
    }
    // A tag directly followed by `(`, `=`, `.` or a quote is not a static
    // image name (`show expression`, Python, ...).
    if end == start || matches!(bytes.get(end), Some(b'(' | b'=' | b'.' | b'"' | b'\'')) {
        return None;
    }
    let tag = &line[start..end];
    if matches!(tag, "screen" | "layer" | "expression" | "onlayer" | "with" | "at" | "as") {
        return None;
    }
    Some((tag.to_string(), start, end))
}

/// `show eileen as eileen2` makes `eileen2` a tag that `hide` may use.
pub fn parse_show_alias(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("show ") && !trimmed.starts_with("scene ") {
        return None;
    }
    let pos = trimmed.find(" as ")?;
    let alias = trimmed[pos + 4..].split_whitespace().next()?;
    let alias = alias.trim_end_matches(':');
    if !alias.is_empty() && alias.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        Some(alias)
    } else {
        None
    }
}

/// Images defined from Python: `renpy.image("name", ...)` defines a known
/// name; a call whose name is not a string literal makes image names
/// unknowable (`Err(())`).
pub fn parse_python_image_definition(line: &str) -> Option<Result<String, ()>> {
    let pos = line.find("renpy.image(")?;
    let arg = line[pos + "renpy.image(".len()..].trim_start();
    let quote = arg.chars().next()?;
    if quote != '"' && quote != '\'' {
        return Some(Err(()));
    }
    let body = &arg[1..];
    match body.find(quote) {
        Some(end) => {
            let name = body[..end].split_whitespace().collect::<Vec<_>>().join(" ");
            if name.is_empty() {
                Some(Err(()))
            } else {
                Some(Ok(name))
            }
        }
        None => Some(Err(())),
    }
}

/// Levenshtein distance, for "did you mean" suggestions.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// The closest candidate to `word`, if it is close enough to be a typo.
pub fn closest<'a>(word: &str, candidates: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let limit = match word.chars().count() {
        0..=3 => 1,
        4..=7 => 2,
        _ => 3,
    };
    candidates
        .into_iter()
        .filter(|c| *c != word)
        .map(|c| (edit_distance(word, c), c))
        .filter(|(d, _)| *d <= limit)
        .min_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)))
        .map(|(_, c)| c)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_image_files_like_renpy() {
        assert_eq!(image_name_for(Path::new("game/images/bg room.png")), Some("bg room".into()));
        assert_eq!(image_name_for(Path::new("game/images/a/Eileen Happy.WEBP")), Some("eileen happy".into()));
        assert_eq!(image_name_for(Path::new("game/images/logo@2.png")), Some("logo".into()));
        assert_eq!(image_name_for(Path::new("game/images/bg  night.jpg")), Some("bg night".into()));
        assert_eq!(image_name_for(Path::new("game/images/notes.txt")), None);
        assert_eq!(image_name_for(Path::new("game/images/.png")), None);
    }

    #[test]
    fn scans_nested_image_directories() {
        let dir = std::env::temp_dir().join(format!("renpy-images-test-{}", std::process::id()));
        let images = dir.join("game/images/characters");
        std::fs::create_dir_all(&images).unwrap();
        std::fs::write(images.join("ciri casual.png"), b"").unwrap();
        std::fs::write(dir.join("game/images/bg forest.jpg"), b"").unwrap();
        std::fs::write(dir.join("game/images/readme.md"), b"").unwrap();
        let mut names: Vec<String> = scan_project_images(&dir).into_iter().map(|i| i.name).collect();
        names.sort();
        assert_eq!(names, vec!["bg forest", "ciri casual"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn finds_image_references() {
        assert_eq!(parse_image_reference("    show eileen happy at left"), Some(("eileen".into(), 9, 15)));
        assert_eq!(parse_image_reference("    scene bg room with fade"), Some(("bg".into(), 10, 12)));
        assert_eq!(parse_image_reference("    hide eileen"), Some(("eileen".into(), 9, 15)));
        assert_eq!(parse_image_reference("    show screen inventory"), None);
        assert_eq!(parse_image_reference("    show layer master at blur"), None);
        assert_eq!(parse_image_reference("    show expression \"x.png\""), None);
        assert_eq!(parse_image_reference("    scene"), None);
        assert_eq!(parse_image_reference("    scene with fade"), None);
        assert_eq!(parse_image_reference("    e \"show me\""), None);
        assert_eq!(parse_show_alias("    show eileen happy as e2 at left"), Some("e2"));
        assert_eq!(parse_show_alias("    show eileen"), None);
    }

    #[test]
    fn python_image_definitions() {
        assert_eq!(parse_python_image_definition("    $ renpy.image(\"bg x\", Solid(\"#000\"))"), Some(Ok("bg x".into())));
        assert_eq!(parse_python_image_definition("    renpy.image(name, d)"), Some(Err(())));
        assert_eq!(parse_python_image_definition("    $ x = 1"), None);
    }

    #[test]
    fn suggests_close_names() {
        assert_eq!(closest("eilen", ["eileen", "lucy", "bg"]), Some("eileen"));
        assert_eq!(closest("xyz", ["eileen", "lucy"]), None);
        assert_eq!(closest("bg", ["bg", "cg"]), Some("cg"));
    }
}
