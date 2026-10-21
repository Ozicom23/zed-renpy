//! Parameter hints: which call the cursor is inside, and which parameter.

#[derive(Debug, PartialEq)]
pub struct CallSite {
    /// The called name, dots included: `renpy.pause`, `Character`, `shop`.
    pub name: String,
    /// 0-based index of the argument being typed (commas before the cursor).
    pub argument: u32,
    /// `name=` already typed for the current argument.
    pub keyword: Option<String>,
}

struct Frame {
    open: char,
    /// Byte offset of the opening bracket.
    pos: usize,
    commas: u32,
    /// Byte offset where the current argument starts.
    arg_start: usize,
}

/// The innermost unclosed call around the end of `text` (the source up to
/// the cursor). Strings and comments are skipped.
pub fn call_at(text: &str) -> Option<CallSite> {
    let bytes = text.as_bytes();
    let mut stack: Vec<Frame> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            b'"' | b'\'' => {
                let triple = text[i..].starts_with(if c == b'"' { "\"\"\"" } else { "'''" });
                let delim_len = if triple { 3 } else { 1 };
                let mut j = i + delim_len;
                loop {
                    if j >= bytes.len() {
                        return None; // cursor inside a string
                    }
                    if bytes[j] == b'\\' {
                        j += 2;
                        continue;
                    }
                    if bytes[j] == c && (!triple || text[j..].starts_with(&text[i..i + 3])) {
                        j += delim_len;
                        break;
                    }
                    if bytes[j] == b'\n' && !triple {
                        j += 1;
                        break;
                    }
                    j += 1;
                }
                i = j;
                continue;
            }
            b'#' => {
                while i < bytes.len() && bytes[i] != b'\n' {
                    i += 1;
                }
                continue;
            }
            b'(' | b'[' | b'{' => stack.push(Frame {
                open: c as char,
                pos: i,
                commas: 0,
                arg_start: i + 1,
            }),
            b')' | b']' | b'}' => {
                stack.pop();
            }
            b',' => {
                if let Some(top) = stack.last_mut() {
                    top.commas += 1;
                    top.arg_start = i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    let frame = stack.iter().rev().find(|f| f.open == '(')?;
    // Only the innermost bracket counts: inside a list argument, no hint.
    if !std::ptr::eq(frame, stack.last()?) {
        return None;
    }
    let before = text[..frame.pos].trim_end();
    let name_start = before
        .rfind(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.'))
        .map(|p| p + 1)
        .unwrap_or(0);
    let name = &before[name_start..];
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let current = text[frame.arg_start..].trim_start();
    let keyword = current.find('=').and_then(|eq| {
        let candidate = current[..eq].trim();
        let is_ident = !candidate.is_empty()
            && candidate.chars().all(|c| c.is_alphanumeric() || c == '_')
            && !current[eq..].starts_with("==");
        is_ident.then(|| candidate.to_string())
    });
    Some(CallSite { name: name.to_string(), argument: frame.commas, keyword })
}

/// The parameter list inside a signature or definition line: everything
/// between the first `(` and its matching `)`.
pub fn parameter_list(text: &str) -> Option<&str> {
    let open = text.find('(')?;
    let mut depth = 0;
    for (i, c) in text[open..].char_indices() {
        match c {
            '(' | '[' | '{' => depth += 1,
            ')' | ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[open + 1..open + i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// Split a parameter list at top-level commas.
pub fn split_parameters(list: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for c in list.chars() {
        if let Some(q) = quote {
            current.push(c);
            if c == q {
                quote = None;
            }
            continue;
        }
        match c {
            '"' | '\'' => {
                quote = Some(c);
                current.push(c);
            }
            '(' | '[' | '{' => {
                depth += 1;
                current.push(c);
            }
            ')' | ']' | '}' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => {
                out.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(c),
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

/// The parameter a call site fills: by keyword when one is typed, else by
/// position (a `*args` parameter absorbs every further positional argument).
pub fn active_parameter(parameters: &[String], site: &CallSite) -> Option<u32> {
    let bare = |p: &str| {
        p.trim_start_matches('*')
            .split(['=', ':'])
            .next()
            .unwrap_or("")
            .trim()
            .to_string()
    };
    if let Some(keyword) = &site.keyword {
        if let Some(i) = parameters.iter().position(|p| bare(p) == *keyword) {
            return Some(i as u32);
        }
        return parameters.iter().position(|p| p.starts_with("**")).map(|i| i as u32);
    }
    // `self` is never passed explicitly.
    let offset = usize::from(parameters.first().is_some_and(|p| p == "self"));
    let wanted = site.argument as usize + offset;
    for (i, p) in parameters.iter().enumerate() {
        if p.starts_with("**") || p == "/" {
            continue;
        }
        if p.starts_with('*') && i <= wanted {
            return Some(i as u32);
        }
        if i == wanted {
            return Some(i as u32);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(name: &str, argument: u32, keyword: Option<&str>) -> Option<CallSite> {
        Some(CallSite { name: name.into(), argument, keyword: keyword.map(String::from) })
    }

    #[test]
    fn finds_the_enclosing_call() {
        assert_eq!(call_at("    $ renpy.pause("), site("renpy.pause", 0, None));
        assert_eq!(call_at("define e = Character(\"Eileen, (x)\", col"), site("Character", 1, None));
        assert_eq!(call_at("define e = Character(\"E\", color="), site("Character", 1, Some("color")));
        assert_eq!(call_at("    call shop(1, Jump(\"a\"), "), site("shop", 2, None));
        assert_eq!(call_at("    $ f(a == "), site("f", 0, None));
        assert_eq!(call_at("    $ f(x)"), None);
        assert_eq!(call_at("    $ f([1, "), None);
        assert_eq!(call_at("    e \"Hello (there"), None);
        assert_eq!(call_at("Character(\n    \"E\",\n    "), site("Character", 1, None));
    }

    #[test]
    fn splits_and_picks_parameters() {
        let params = split_parameters(parameter_list("Character(name=NotSet, kind=None, **properties)").unwrap());
        assert_eq!(params, vec!["name=NotSet", "kind=None", "**properties"]);
        assert_eq!(active_parameter(&params, &site("C", 1, None).unwrap()), Some(1));
        assert_eq!(active_parameter(&params, &site("C", 0, Some("kind")).unwrap()), Some(1));
        assert_eq!(active_parameter(&params, &site("C", 0, Some("color")).unwrap()), Some(2));
        let params = split_parameters("a, *args, b=(1, 2)");
        assert_eq!(params, vec!["a", "*args", "b=(1, 2)"]);
        assert_eq!(active_parameter(&params, &site("f", 4, None).unwrap()), Some(1));
        let params = split_parameters("self, x");
        assert_eq!(active_parameter(&params, &site("f", 0, None).unwrap()), Some(1));
        assert_eq!(parameter_list("label shop(items, discount=0.1):"), Some("items, discount=0.1"));
    }
}
