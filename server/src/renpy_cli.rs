//! Locating and invoking the Ren'Py SDK, and parsing `renpy lint` reports.
//! Shared by the language server (lint diagnostics) and the debug adapter
//! (game launching).

use std::path::{Path, PathBuf};

/// A resolved way to start Ren'Py: the program to exec and the arguments that
/// must come before the project directory.
pub struct RenpyInvocation {
    pub program: PathBuf,
    pub prefix_args: Vec<String>,
}

/// True if `dir` looks like a Ren'Py SDK checkout.
pub fn is_sdk_dir(dir: &Path) -> bool {
    dir.join("renpy.py").is_file()
}

/// How to invoke the SDK at `sdk`. On Windows the SDK has no shell launcher,
/// so the bundled CPython runs renpy.py directly.
pub fn sdk_invocation(sdk: &Path) -> Option<RenpyInvocation> {
    if !is_sdk_dir(sdk) {
        return None;
    }
    if cfg!(windows) {
        let python = sdk.join("lib").join("py3-windows-x86_64").join("python.exe");
        if python.is_file() {
            return Some(RenpyInvocation {
                program: python,
                prefix_args: vec![sdk.join("renpy.py").to_string_lossy().into_owned()],
            });
        }
        return None;
    }
    let sh = sdk.join("renpy.sh");
    if sh.is_file() {
        return Some(RenpyInvocation { program: sh, prefix_args: Vec::new() });
    }
    None
}

/// Digit runs in a name, for "renpy-8.10.1-sdk" > "renpy-8.5.3-sdk" ordering.
fn version_key(name: &str) -> Vec<u64> {
    let mut key = Vec::new();
    let mut current: Option<u64> = None;
    for ch in name.chars() {
        if let Some(d) = ch.to_digit(10) {
            current = Some(current.unwrap_or(0) * 10 + d as u64);
        } else if let Some(n) = current.take() {
            key.push(n);
        }
    }
    if let Some(n) = current {
        key.push(n);
    }
    key
}

/// Find an SDK without configuration: `$RENPY_SDK`, then directories that look
/// like `renpy-*-sdk` in the usual download spots, newest version first.
pub fn find_sdk() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("RENPY_SDK") {
        let path = PathBuf::from(path);
        if is_sdk_dir(&path) {
            return Some(path);
        }
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .ok()?;
    let mut candidates: Vec<PathBuf> = Vec::new();
    for parent in [home.clone(), home.join("Documents"), home.join("Downloads"), home.join("Desktop")] {
        let Ok(entries) = std::fs::read_dir(&parent) else { continue };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_lowercase();
            if name.contains("renpy") && is_sdk_dir(&entry.path()) {
                candidates.push(entry.path());
            }
        }
    }
    candidates.sort_by_key(|p| version_key(&p.file_name().unwrap_or_default().to_string_lossy()));
    candidates.pop()
}

/// The Ren'Py project base directory (the one containing `game/`) at or above
/// `start`, if any.
pub fn find_project(start: &Path) -> Option<PathBuf> {
    if start.join("game").is_dir() {
        return Some(start.to_path_buf());
    }
    // Opened the game/ directory itself, or a subdirectory of it.
    for ancestor in start.ancestors().skip(1) {
        if ancestor.join("game").is_dir() {
            return Some(ancestor.to_path_buf());
        }
    }
    None
}

/// Ren'Py wants warp targets as `<project-relative path>:<line>`; editors have
/// absolute paths, so translate when the file lives under the project.
pub fn normalize_warp(warp: &str, project: &Path) -> String {
    let Some((path, line)) = warp.rsplit_once(':') else {
        return warp.to_string();
    };
    let path_buf = Path::new(path);
    if path_buf.is_absolute() {
        if let Ok(relative) = path_buf.strip_prefix(project) {
            let relative = relative.to_string_lossy().replace('\\', "/");
            return format!("{relative}:{line}");
        }
    }
    format!("{}:{line}", path.replace('\\', "/"))
}

/// One problem from a lint report: project-relative path, 0-based line, message.
#[derive(Debug, PartialEq)]
pub struct LintProblem {
    pub path: String,
    pub line: u32,
    pub message: String,
}

/// `game/script.rpy:4 'eileen happy' is not an image.` — the start of one
/// problem; continuation lines follow until a blank line.
fn parse_problem_start(line: &str) -> Option<(String, u32, String)> {
    let (path, rest) = line.split_once(':')?;
    if !(path.ends_with(".rpy") || path.ends_with(".rpym")) || path.contains(' ') {
        return None;
    }
    let (number, message) = rest.split_once(' ')?;
    let number: u32 = number.parse().ok()?;
    Some((path.to_string(), number.saturating_sub(1), message.trim().to_string()))
}

/// Parse the human-readable `renpy lint` report. The report opens with a BOM
/// and header, lists problems (with multi-line continuations) separated by
/// blank lines, and ends with a "Statistics:" section.
pub fn parse_lint_report(report: &str) -> Vec<LintProblem> {
    let mut problems: Vec<LintProblem> = Vec::new();
    for raw in report.trim_start_matches('\u{feff}').lines() {
        let line = raw.trim_end();
        if line.trim() == "Statistics:" {
            break;
        }
        if line.is_empty() {
            continue;
        }
        if let Some((path, line_no, message)) = parse_problem_start(line) {
            problems.push(LintProblem { path, line: line_no, message });
        } else if let Some(last) = problems.last_mut() {
            // Continuation of the previous problem's message — but only when
            // directly adjacent to it (headers before the first problem and
            // stray prose after a blank line are not continuations).
            if !last.message.is_empty() {
                last.message.push(' ');
                last.message.push_str(line.trim());
            }
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lint_report_with_continuations() {
        let report = "\u{feff}Ren'Py 8.5.3.26051504 lint report, generated at: Sun Jul 19 2026\n\n\
            game/script.rpy:4 'eileen happy' is not an image.\n\n\
            game/script.rpy:6 The jump is to nonexistent label 'missing_label'.\n\n\
            game/script.rpy:10 Could not evaluate 'q' in the who part of a say statement.\n\
            Perhaps you forgot to define a character?\n\n\n\
            Statistics:\n\n\
            The game contains 2 dialogue blocks.\n";
        let problems = parse_lint_report(report);
        assert_eq!(problems.len(), 3);
        assert_eq!(problems[0], LintProblem {
            path: "game/script.rpy".into(),
            line: 3,
            message: "'eileen happy' is not an image.".into(),
        });
        assert_eq!(problems[1].line, 5);
        assert_eq!(
            problems[2].message,
            "Could not evaluate 'q' in the who part of a say statement. Perhaps you forgot to define a character?"
        );
    }

    #[test]
    fn lint_report_ignores_prose_and_statistics() {
        let problems = parse_lint_report("No problems found.\n\nStatistics:\n\ngame/x.rpy:1 not a problem\n");
        assert!(problems.is_empty());
    }

    #[test]
    fn warp_paths_are_made_project_relative() {
        let project = if cfg!(windows) { Path::new("C:\\proj") } else { Path::new("/proj") };
        let absolute = if cfg!(windows) { "C:\\proj\\game\\script.rpy:42" } else { "/proj/game/script.rpy:42" };
        assert_eq!(normalize_warp(absolute, project), "game/script.rpy:42");
        assert_eq!(normalize_warp("game/script.rpy:7", project), "game/script.rpy:7");
        let outside = if cfg!(windows) { "D:\\other\\a.rpy:1" } else { "/other/a.rpy:1" };
        assert_eq!(normalize_warp(outside, project), outside.replace('\\', "/"));
    }

    #[test]
    fn version_ordering_prefers_newest_sdk() {
        assert!(version_key("renpy-8.10.1-sdk") > version_key("renpy-8.5.3-sdk"));
        assert!(version_key("renpy-8.5.3-sdk") > version_key("renpy-7.9.9-sdk"));
    }

    #[test]
    fn finds_project_from_game_subdirectory() {
        let base = std::env::temp_dir().join(format!("renpy-cli-test-{}", std::process::id()));
        let game = base.join("game");
        std::fs::create_dir_all(&game).unwrap();
        assert_eq!(find_project(&base), Some(base.clone()));
        assert_eq!(find_project(&game), Some(base.clone()));
        assert_eq!(find_project(&game.join("sub")), Some(base.clone()));
        let _ = std::fs::remove_dir_all(&base);
    }
}
