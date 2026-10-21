//! Color swatches: hex colors in string literals, as Ren'Py writes them
//! (`"#f00"`, `"#ff000080"`, `color="#c8c8ff"`, `Solid("#000")`).

#[derive(Debug, PartialEq)]
pub struct ColorAt {
    pub line: u32,
    /// UTF-16 columns of the `#rrggbb` text, quotes excluded.
    pub start: u32,
    pub end: u32,
    /// Components in 0..=1.
    pub rgba: [f32; 4],
}

fn hex(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

/// Parse `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa` (without the `#`).
pub fn parse_hex(digits: &str) -> Option<[f32; 4]> {
    let b = digits.as_bytes();
    let channel = |hi: u8, lo: u8| Some(((hex(hi)? << 4) | hex(lo)?) as f32 / 255.0);
    let short = |c: u8| Some((hex(c)? * 17) as f32 / 255.0);
    match b.len() {
        3 => Some([short(b[0])?, short(b[1])?, short(b[2])?, 1.0]),
        4 => Some([short(b[0])?, short(b[1])?, short(b[2])?, short(b[3])?]),
        6 => Some([channel(b[0], b[1])?, channel(b[2], b[3])?, channel(b[4], b[5])?, 1.0]),
        8 => Some([channel(b[0], b[1])?, channel(b[2], b[3])?, channel(b[4], b[5])?, channel(b[6], b[7])?]),
        _ => None,
    }
}

/// Every quoted hex color in `text`.
pub fn find_colors(text: &str) -> Vec<ColorAt> {
    let mut out = Vec::new();
    for (line_no, line) in text.lines().enumerate() {
        let bytes = line.as_bytes();
        let mut i = 0;
        while i + 1 < bytes.len() {
            let q = bytes[i];
            if (q == b'"' || q == b'\'') && bytes[i + 1] == b'#' {
                let start = i + 1;
                let mut end = start + 1;
                while end < bytes.len() && bytes[end].is_ascii_hexdigit() {
                    end += 1;
                }
                if bytes.get(end) == Some(&q) {
                    if let Some(rgba) = parse_hex(&line[start + 1..end]) {
                        out.push(ColorAt {
                            line: line_no as u32,
                            start: line[..start].encode_utf16().count() as u32,
                            end: line[..end].encode_utf16().count() as u32,
                            rgba,
                        });
                    }
                    i = end + 1;
                    continue;
                }
            }
            i += 1;
        }
    }
    out
}

/// How a picked color is written back: `#rrggbb`, or `#rrggbbaa` when it is
/// not opaque.
pub fn presentation(rgba: [f32; 4]) -> String {
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let [r, g, b, a] = rgba.map(byte);
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_quoted_hex_colors() {
        let text = "define e = Character(\"Eileen\", color=\"#c8c8ff\")\nimage black = Solid('#000')\n$ x = \"#notacolor\"\nstyle s color \"#ff000080\" # \"#123\" too\n$ y = \"Café #fff\"\n";
        let found = find_colors(text);
        let spans: Vec<(u32, u32, u32)> = found.iter().map(|c| (c.line, c.start, c.end)).collect();
        assert_eq!(spans, vec![(0, 38, 45), (1, 21, 25), (3, 15, 24), (3, 29, 33)]);
        assert_eq!(found[1].rgba, [0.0, 0.0, 0.0, 1.0]);
        assert!((found[2].rgba[3] - 128.0 / 255.0).abs() < 1e-6);
    }

    #[test]
    fn writes_colors_back() {
        assert_eq!(presentation([1.0, 0.0, 0.5, 1.0]), "#ff0080");
        assert_eq!(presentation([1.0, 1.0, 1.0, 0.5]), "#ffffff80");
        assert_eq!(parse_hex("fc0"), Some([1.0, 0.8, 0.0, 1.0]));
        assert_eq!(parse_hex("12345"), None);
    }
}
