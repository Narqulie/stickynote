// stickynote — terminal-based sticky notes board.
// Note model, theme palette, and style helpers.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

// ── Themes ────────────────────────────────────────────────────────────────────

/// A colour theme defining the palette for status bars, borders, and hints.
pub struct Theme {
    pub name: &'static str,
    pub status_bg: Color,
    pub status_fg: Color,
    pub sel_border: Color,
    pub hint_bg: Color,
    pub hint_fg: Color,
}

pub const THEMES: [Theme; 3] = [
    Theme {
        name: "dark",
        status_bg: Color::Rgb(0x22, 0x22, 0x22),
        status_fg: Color::Rgb(0xcc, 0xcc, 0xcc),
        sel_border: Color::Rgb(0xff, 0xff, 0xff),
        hint_bg: Color::Rgb(0x1a, 0x1a, 0x1a),
        hint_fg: Color::Rgb(0x88, 0x88, 0x88),
    },
    Theme {
        name: "light",
        status_bg: Color::Rgb(0xdd, 0xdd, 0xdd),
        status_fg: Color::Rgb(0x22, 0x22, 0x22),
        sel_border: Color::Rgb(0x00, 0x00, 0x00),
        hint_bg: Color::Rgb(0xe8, 0xe8, 0xe8),
        hint_fg: Color::Rgb(0x55, 0x55, 0x55),
    },
    Theme {
        name: "mono",
        status_bg: Color::Rgb(0x11, 0x11, 0x11),
        status_fg: Color::Rgb(0x33, 0xff, 0x33),
        sel_border: Color::Rgb(0x33, 0xff, 0x33),
        hint_bg: Color::Rgb(0x00, 0x00, 0x00),
        hint_fg: Color::Rgb(0x33, 0xff, 0x33),
    },
];

// ── Note colours ───────────────────────────────────────────────────────────────

pub const NOTE_COLORS: [&str; 10] = [
    "#ffe066", "#98d8c8", "#a8d8ea", "#ffb3ba", "#ffcc99", "#ff6b6b", "#c9b1ff", "#7ec8e3",
    "#b5e7a0", "#d4d4d4",
];

pub const COLOR_NAMES: [&str; 10] = [
    "yellow", "mint", "blue", "pink", "orange", "red", "purple", "teal", "green", "gray",
];

pub const BORDER_STYLES: [&str; 5] = ["rounded", "double", "thick", "hidden", "none"];

// ── Note ──────────────────────────────────────────────────────────────────────

/// A single sticky note with content, styling, tags, and editing state.
/// Transient fields (`cursor`, `title_cursor`, `tag_input`, `tag_cursor`) are not persisted.
#[derive(Debug, Clone)]
pub struct Note {
    pub content: String,
    pub tags: Vec<String>,
    pub title: String,
    pub title_cursor: usize,
    pub color: String,
    pub font_style: String,
    pub border_style: String,
    pub editing: bool,
    pub cursor: usize,
    /// Inline tag input buffer — transient, not persisted.
    pub tag_input: String,
    /// Selected tag index during tag navigation (None = no tag selected).
    pub tag_cursor: Option<usize>,
}

impl Note {
    /// Create a new empty note with default colour, border, and font style.
    pub fn new() -> Self {
        Note {
            content: String::new(),
            tags: Vec::new(),
            title: String::new(),
            title_cursor: 0,
            color: NOTE_COLORS[0].to_string(),
            font_style: "normal".to_string(),
            border_style: "rounded".to_string(),
            editing: false,
            cursor: 0,
            tag_input: String::new(),
            tag_cursor: None,
        }
    }

    /// First line of content (everything before the first `\n`).
    pub fn first_line(&self) -> &str {
        self.content.lines().next().unwrap_or("")
    }

    /// Split content into lines, inserting a `|` cursor marker when editing.
    pub fn content_lines(&self) -> Vec<String> {
        let display = if self.editing {
            let pos = self.cursor.min(self.content.len());
            let mut s = self.content.clone();
            s.insert(pos, '|');
            s
        } else {
            self.content.clone()
        };
        display.lines().map(|l| l.to_string()).collect()
    }

    /// Check if the note has a specific tag (case-insensitive).
    pub fn has_tag(&self, tag: &str) -> bool {
        let lower = tag.to_lowercase();
        self.tags.iter().any(|t| t.to_lowercase() == lower)
    }

    /// Normalize a raw tag string to canonical form (lowercased, trimmed).
    pub fn normalize_tag(raw: &str) -> String {
        raw.trim().to_lowercase()
    }

    /// Return the (line_index, column) of the content cursor.
    /// Both are 0-based; `line_index` counts `\n`-separated lines.
    pub fn cursor_pos(&self) -> (usize, usize) {
        let pos = self.cursor.min(self.content.len());
        let before = &self.content[..pos];
        let lines_before = before.chars().filter(|&c| c == '\n').count();
        let line_start = before.rfind('\n').map(|i| i + 1).unwrap_or(0);
        let col = self.cursor - line_start;
        (lines_before, col)
    }

    /// Convert a (line_index, column) pair back to a flat byte position.
    /// Column is clamped to the line length.
    pub fn pos_to_cursor(&self, line_idx: usize, col: usize) -> usize {
        let mut pos = 0usize;
        for (i, line) in self.content.split('\n').enumerate() {
            if i == line_idx {
                return pos + col.min(line.len());
            }
            pos += line.len() + 1; // +1 for the '\n' separator
        }
        self.content.len()
    }
}

// ── Colour parsing ────────────────────────────────────────────────────────────

/// Parse a hex colour string (`#rrggbb`) into a Ratatui `Color`.
pub fn parse_hex(hex: &str) -> Color {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6
        && let (Ok(r), Ok(g), Ok(b)) = (
            u8::from_str_radix(&hex[0..2], 16),
            u8::from_str_radix(&hex[2..4], 16),
            u8::from_str_radix(&hex[4..6], 16),
        )
    {
        return Color::Rgb(r, g, b);
    }
    Color::White
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Find `current` in `slice` and return the next element, wrapping around.
pub fn cycle_str<'a>(slice: &'a [&'a str], current: &'a str) -> &'a str {
    for (i, s) in slice.iter().enumerate() {
        if *s == current {
            return slice[(i + 1) % slice.len()];
        }
    }
    current
}

/// Absolute difference between two `u16` values, saturating.
pub fn abs_diff_u16(a: u16, b: u16) -> u16 {
    a.abs_diff(b)
}

/// Look up the human-readable name for a note colour hex value.
pub fn color_name(hex: &str) -> &'static str {
    for (i, c) in NOTE_COLORS.iter().enumerate() {
        if *c == hex {
            return COLOR_NAMES[i];
        }
    }
    COLOR_NAMES[0]
}

// ── Markdown parsing ──────────────────────────────────────────────────────────

/// Parse inline markdown syntax from `text` and produce styled `Span`s layered
/// over a base `style`. Supports: `**bold**`, `__bold__`, `*italic*`, `_italic_`,
/// `~~strikethrough~~`, `` `code` ``.
pub fn parse_md(text: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut remaining = text;
    let mut plain = String::new();

    macro_rules! flush_plain {
        () => {
            if !plain.is_empty() {
                spans.push(Span::styled(std::mem::take(&mut plain), base));
            }
        };
    }

    while !remaining.is_empty() {
        // Macro: try to match a markdown pattern, flush plain text before it,
        // push the styled span, and advance `remaining`.
        macro_rules! try_capture {
            ($open:expr, $close:expr, $style_fn:expr) => {
                if let Some((inner, tail, new_style)) =
                    try_md_capture(remaining, $open, $close, base, $style_fn)
                {
                    flush_plain!();
                    spans.push(Span::styled(inner.to_string(), new_style));
                    remaining = tail;
                    continue;
                }
            };
        }

        try_capture!("~~", "~~", |s| s.add_modifier(Modifier::CROSSED_OUT));
        try_capture!("**", "**", |s| s.add_modifier(Modifier::BOLD));
        try_capture!("__", "__", |s| s.add_modifier(Modifier::BOLD));
        try_capture!("*", "*", |s| s.add_modifier(Modifier::ITALIC));
        try_capture!("_", "_", |s| s.add_modifier(Modifier::ITALIC));
        try_capture!("`", "`", |s| s
            .fg(Color::Rgb(0xe6, 0x9b, 0x3b))
            .bg(Color::Rgb(0x2d, 0x2d, 0x2d)));

        // No markdown — accumulate plain character.
        let c = remaining.chars().next().unwrap();
        plain.push(c);
        remaining = &remaining[c.len_utf8()..];
    }

    flush_plain!();
    spans
}

/// Try to match inline markdown `open` … `close` at the start of `text`.
/// Returns `(inner_text, tail, styled_style)` on match, `None` otherwise.
fn try_md_capture<'a>(
    text: &'a str,
    open: &str,
    close: &str,
    base: Style,
    style_fn: impl Fn(Style) -> Style,
) -> Option<(&'a str, &'a str, Style)> {
    let after_open = text.strip_prefix(open)?;
    let end = after_open.find(close)?;
    let inner = &after_open[..end];
    let tail = &after_open[end + close.len()..];
    Some((inner, tail, style_fn(base)))
}

/// Strip Markdown syntax delimiters from `text`, returning plain text.
/// Useful for tab previews where formatting markers would look cluttered.
pub fn strip_md(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut remaining = text;

    while !remaining.is_empty() {
        // Check multi-char delimiters first; if the open delimiter matches but
        // no close is found, emit the open delimiter as plain text.
        if let Some(rest) = remaining.strip_prefix("**") {
            if let Some(end) = rest.find("**") {
                out.push_str(&rest[..end]);
                remaining = &rest[end + 2..];
                continue;
            }
            out.push_str("**");
            remaining = rest;
            continue;
        }
        if let Some(rest) = remaining.strip_prefix("__") {
            if let Some(end) = rest.find("__") {
                out.push_str(&rest[..end]);
                remaining = &rest[end + 2..];
                continue;
            }
            out.push_str("__");
            remaining = rest;
            continue;
        }
        if let Some(rest) = remaining.strip_prefix("~~") {
            if let Some(end) = rest.find("~~") {
                out.push_str(&rest[..end]);
                remaining = &rest[end + 2..];
                continue;
            }
            out.push_str("~~");
            remaining = rest;
            continue;
        }
        if let Some(rest) = remaining.strip_prefix('*')
            && let Some(end) = rest.find('*')
        {
            out.push_str(&rest[..end]);
            remaining = &rest[end + 1..];
            continue;
        }
        if let Some(rest) = remaining.strip_prefix('_')
            && let Some(end) = rest.find('_')
        {
            out.push_str(&rest[..end]);
            remaining = &rest[end + 1..];
            continue;
        }
        if let Some(rest) = remaining.strip_prefix('`')
            && let Some(end) = rest.find('`')
        {
            out.push_str(&rest[..end]);
            remaining = &rest[end + 1..];
            continue;
        }

        let c = remaining.chars().next().unwrap();
        out.push(c);
        remaining = &remaining[c.len_utf8()..];
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_note_defaults() {
        let n = Note::new();
        assert_eq!(n.font_style, "normal");
        assert_eq!(n.border_style, "rounded");
        assert_eq!(n.color, NOTE_COLORS[0]);
        assert!(!n.editing);
        assert!(n.tags.is_empty());
        assert!(n.content.is_empty());
    }

    #[test]
    fn test_first_line() {
        let n = Note {
            content: "hello".into(),
            ..Note::new()
        };
        assert_eq!(n.first_line(), "hello");

        let n = Note {
            content: "hello\nworld".into(),
            ..Note::new()
        };
        assert_eq!(n.first_line(), "hello");

        let n = Note {
            content: "".into(),
            ..Note::new()
        };
        assert_eq!(n.first_line(), "");
    }

    #[test]
    fn test_content_lines_no_cursor() {
        let n = Note {
            content: "hello\nworld".into(),
            editing: false,
            ..Note::new()
        };
        let lines = n.content_lines();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0], "hello");
        assert_eq!(lines[1], "world");
    }

    #[test]
    fn test_content_lines_with_cursor() {
        let n = Note {
            content: "hello".into(),
            editing: true,
            cursor: 3,
            ..Note::new()
        };
        let lines = n.content_lines();
        assert_eq!(lines[0], "hel|lo");
    }

    #[test]
    fn test_content_lines_cursor_at_end() {
        let n = Note {
            content: "hi".into(),
            editing: true,
            cursor: 2,
            ..Note::new()
        };
        let lines = n.content_lines();
        assert_eq!(lines[0], "hi|");
    }

    #[test]
    fn test_content_lines_cursor_past_end() {
        let n = Note {
            content: "ab".into(),
            editing: true,
            cursor: 100,
            ..Note::new()
        };
        let lines = n.content_lines();
        assert_eq!(lines[0], "ab|");
    }

    #[test]
    fn test_has_tag() {
        let n = Note {
            tags: vec!["urgent".into(), "todo".into()],
            ..Note::new()
        };
        assert!(n.has_tag("urgent"));
        assert!(n.has_tag("todo"));
        assert!(!n.has_tag("missing"));
        assert!(!n.has_tag(""));
    }

    #[test]
    fn test_parse_hex() {
        let c = parse_hex("#ffe066");
        assert_eq!(c, Color::Rgb(0xff, 0xe0, 0x66));

        let c = parse_hex("ff0000");
        assert_eq!(c, Color::Rgb(0xff, 0x00, 0x00));
    }

    #[test]
    fn test_cycle_str() {
        let slice = ["a", "b", "c"];
        assert_eq!(cycle_str(&slice, "a"), "b");
        assert_eq!(cycle_str(&slice, "c"), "a");
        assert_eq!(cycle_str(&slice, "x"), "x");
    }

    #[test]
    fn test_abs_diff_u16() {
        assert_eq!(abs_diff_u16(5, 3), 2);
        assert_eq!(abs_diff_u16(3, 5), 2);
        assert_eq!(abs_diff_u16(0, 0), 0);
    }

    #[test]
    fn test_color_name() {
        assert_eq!(color_name("#ffe066"), "yellow");
        assert_eq!(color_name("#98d8c8"), "mint");
        assert_eq!(color_name("#nonexist"), "yellow"); // fallback
    }

    // ── Markdown ──────────────────────────────────────────────────────────

    #[test]
    fn test_strip_md_bold() {
        assert_eq!(strip_md("**bold**"), "bold");
        assert_eq!(strip_md("__bold__"), "bold");
        assert_eq!(strip_md("before **bold** after"), "before bold after");
    }

    #[test]
    fn test_strip_md_italic() {
        assert_eq!(strip_md("*italic*"), "italic");
        assert_eq!(strip_md("_italic_"), "italic");
    }

    #[test]
    fn test_strip_md_strikethrough() {
        assert_eq!(strip_md("~~strike~~"), "strike");
    }

    #[test]
    fn test_strip_md_code() {
        assert_eq!(strip_md("`code`"), "code");
    }

    #[test]
    fn test_strip_md_mixed() {
        assert_eq!(strip_md("**bold** and *italic*"), "bold and italic");
        assert_eq!(strip_md("no markup"), "no markup");
        assert_eq!(strip_md(""), "");
    }

    #[test]
    fn test_strip_md_unmatched() {
        // Unmatched delimiters should remain as-is.
        assert_eq!(strip_md("**unclosed"), "**unclosed");
        assert_eq!(strip_md("*nope"), "*nope");
    }

    #[test]
    fn test_parse_md_plain() {
        let spans = parse_md("hello", Style::new());
        assert_eq!(spans.len(), 1); // single accumulated span
        assert_eq!(span_style(&spans[0]), Style::new());
    }

    /// Helper: extract the style from a span's content for comparison.
    fn span_style(s: &Span) -> Style {
        s.style
    }

    #[test]
    fn test_parse_md_bold() {
        let spans = parse_md("**bold**", Style::new());
        assert_eq!(spans.len(), 1);
        assert!(spans[0].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[0].content, "bold");
    }

    #[test]
    fn test_parse_md_italic() {
        let spans = parse_md("*italic*", Style::new());
        assert_eq!(spans.len(), 1);
        assert!(spans[0].style.add_modifier.contains(Modifier::ITALIC));
    }

    #[test]
    fn test_parse_md_mixed() {
        let spans = parse_md("a **b** c", Style::new());
        assert_eq!(spans.len(), 3); // "a ", **bold**, " c"
        assert_eq!(spans[0].content, "a ");
        assert_eq!(spans[1].content, "b");
        assert!(spans[1].style.add_modifier.contains(Modifier::BOLD));
        assert_eq!(spans[2].content, " c");
    }
}
