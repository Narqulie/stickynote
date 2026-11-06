// Shared terminal-cell layout for drawing and mouse hit-testing.
use std::ops::Range;

use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, EditFocus};
use crate::note::{Note, char_idx_to_byte, char_len, color_name, strip_md};

/// Match Ratatui's per-grapheme cell widths, not bytes or scalar counts.
pub fn cell_width(text: &str) -> usize {
    text.graphemes(true).map(UnicodeWidthStr::width).sum()
}

struct Glyph {
    text: String,
    source: Range<usize>,
    caret: Option<usize>,
}

impl Glyph {
    fn width(&self) -> usize {
        cell_width(&self.text)
    }
    fn index(&self) -> usize {
        self.caret.unwrap_or(self.source.start)
    }
}

/// Visible graphemes with their original character offsets, including the caret.
pub struct TextLine {
    glyphs: Vec<Glyph>,
    padding: usize,
    end: usize,
}

impl TextLine {
    fn glyphs(text: &str, cursor: Option<usize>) -> Vec<Glyph> {
        let cursor = cursor.map(|index| index.min(char_len(text)));
        let mut display = text.to_string();
        if let Some(index) = cursor {
            display.insert(char_idx_to_byte(text, index), '█');
        }
        let mut display_start = 0;
        let mut source_start = 0;
        display
            .graphemes(true)
            .map(|text| {
                let count = char_len(text);
                let caret =
                    cursor.filter(|&index| index >= display_start && index < display_start + count);
                let end = source_start + count - usize::from(caret.is_some());
                let glyph = Glyph {
                    text: text.into(),
                    source: source_start..end,
                    caret,
                };
                display_start += count;
                source_start = end;
                glyph
            })
            .collect()
    }

    fn crop(glyphs: Vec<Glyph>, start: usize, width: u16) -> Self {
        let mut visible = Vec::new();
        let mut used = 0;
        let mut end = glyphs.last().map_or(0, |glyph| glyph.source.end);
        for glyph in glyphs.into_iter().skip(start) {
            if used + glyph.width() > width as usize {
                end = glyph.source.start;
                // Leave room for an ellipsis without evicting the visible caret.
                while used + 1 > width as usize {
                    if visible.last().is_some_and(|g: &Glyph| g.caret.is_some()) {
                        break;
                    }
                    let Some(last) = visible.pop() else { break };
                    used -= last.width();
                    end = last.source.start;
                }
                if used < width as usize {
                    visible.push(Glyph {
                        text: "…".into(),
                        source: end..end,
                        caret: None,
                    });
                }
                break;
            }
            used += glyph.width();
            visible.push(glyph);
        }
        Self {
            glyphs: visible,
            padding: 0,
            end,
        }
    }

    pub fn new(text: &str, cursor: Option<usize>, width: u16) -> Self {
        Self::crop(Self::glyphs(text, cursor), 0, width)
    }

    pub fn source(text: &str) -> Self {
        Self {
            glyphs: Self::glyphs(text, None),
            padding: 0,
            end: char_len(text),
        }
    }

    pub fn following(text: &str, cursor: Option<usize>, width: u16) -> Self {
        let glyphs = Self::glyphs(text, cursor);
        let mut start = 0;
        if glyphs.iter().map(Glyph::width).sum::<usize>() > width as usize
            && let Some(caret) = glyphs.iter().position(|g| g.caret.is_some())
        {
            let mut used: usize = glyphs[..=caret].iter().map(Glyph::width).sum();
            let budget = width.saturating_sub(1) as usize;
            while used > budget && start < caret {
                used -= glyphs[start].width();
                start += 1;
            }
        }
        Self::crop(glyphs, start, width)
    }

    pub fn title(text: &str, cursor: Option<usize>, width: u16) -> Self {
        let mut line = Self::following(text, cursor, width);
        line.padding = (width as usize).saturating_sub(line.width()) / 2;
        line
    }

    /// Skip cells without drawing a partial wide glyph at the left edge.
    pub fn window(text: &str, cursor: Option<usize>, width: u16, left: usize) -> Self {
        let glyphs = Self::glyphs(text, cursor);
        let mut start = 0;
        let mut cell = 0;
        while start < glyphs.len() && cell < left {
            cell += glyphs[start].width();
            start += 1;
        }
        let padding = cell.saturating_sub(left).min(width as usize);
        let mut line = Self::crop(glyphs, start, width - padding as u16);
        line.padding = padding;
        line
    }

    pub fn width(&self) -> usize {
        self.padding + self.glyphs.iter().map(Glyph::width).sum::<usize>()
    }

    pub fn index_at_cell(&self, cell: usize) -> usize {
        if cell < self.padding {
            return self.glyphs.first().map_or(self.end, Glyph::index);
        }
        let mut x = self.padding;
        for glyph in &self.glyphs {
            if cell < x + glyph.width() {
                return glyph.index();
            }
            x += glyph.width();
        }
        self.end
    }

    pub fn render(&self, selection: Option<(usize, usize)>, style: Style) -> Line<'static> {
        let mut spans = Vec::new();
        if self.padding > 0 {
            spans.push(Span::styled(" ".repeat(self.padding), style));
        }
        for glyph in &self.glyphs {
            // A terminal cannot independently color the combining scalars in one cell.
            let selected = glyph.caret.is_none()
                && selection.is_some_and(|(start, end)| {
                    start < glyph.source.end && end > glyph.source.start
                });
            let glyph_style = if selected {
                Style::new()
                    .bg(Color::Rgb(0x55, 0x55, 0x55))
                    .fg(Color::White)
            } else {
                style
            };
            spans.push(Span::styled(glyph.text.clone(), glyph_style));
        }
        Line::from(spans)
    }
}

pub fn card_rect(width: u16, height: u16, overlay: bool) -> Option<Rect> {
    if width < 12 || height < 8 {
        return None;
    }
    let (y, h) = if overlay {
        let h = (height as f64 * 0.6) as u16;
        ((height - h) / 3, h)
    } else {
        (1, height - 3)
    };
    Some(Rect::new(1, y, width - 2, h))
}

pub struct Section {
    pub text: Rect,
    pub border: Option<Rect>,
}

pub struct NoteLayout {
    pub card: Rect,
    pub inner: Rect,
    pub header: Section,
    pub body: Section,
    pub tags: Option<Section>,
    pub header_separator: Rect,
    pub tags_separator: Option<Rect>,
}

impl NoteLayout {
    pub fn new(card: Rect, note: &Note, focus: Option<EditFocus>) -> Self {
        // Editing reserves room for section focus borders even with no outer border.
        let bordered = !matches!(note.border_style.as_str(), "hidden" | "none") || note.editing;
        let inner = if bordered {
            card.inner(ratatui::layout::Margin::new(1, 1))
        } else {
            card
        };
        let header_focused = focus == Some(EditFocus::Header);
        let body_focused = focus == Some(EditFocus::Content);
        let tags_focused = focus == Some(EditFocus::Tags);
        let show_tags = inner.height >= 4
            && (!note.tags.is_empty()
                || note.editing && (tags_focused || !note.tag_input.is_empty()));
        let body_h = inner
            .height
            .saturating_sub(2 + if show_tags { 2 } else { 0 });
        let inset = |focused: bool, y, height| {
            Rect::new(
                inner.x + u16::from(focused),
                y,
                inner.width.saturating_sub(if focused { 2 } else { 0 }),
                height,
            )
        };
        let header = Section {
            text: inset(header_focused, inner.y, 1),
            border: header_focused.then(|| Rect::new(inner.x, inner.y - 1, inner.width, 3)),
        };
        let mut body_text = inset(body_focused, inner.y + 2, body_h);
        body_text.width = body_text.width.saturating_sub(2);
        let body = Section {
            text: body_text,
            border: body_focused.then(|| Rect::new(inner.x, inner.y + 1, inner.width, body_h + 2)),
        };
        let tags_y = inner.y + 3 + body_h;
        let tags = show_tags.then(|| Section {
            text: inset(tags_focused, tags_y, 1),
            border: tags_focused.then(|| Rect::new(inner.x, tags_y - 1, inner.width, 3)),
        });
        Self {
            card,
            inner,
            header,
            body,
            tags,
            header_separator: Rect::new(inner.x, inner.y + 1, inner.width, 1),
            tags_separator: show_tags.then(|| Rect::new(inner.x, tags_y - 1, inner.width, 1)),
        }
    }

    pub fn editing_visible(&self, focus: EditFocus) -> bool {
        match focus {
            EditFocus::Header => !self.header.text.is_empty(),
            EditFocus::Content => !self.body.text.is_empty(),
            EditFocus::Tags => self.tags.as_ref().is_some_and(|tags| !tags.text.is_empty()),
        }
    }

    pub fn focus_at(&self, point: Position) -> Option<EditFocus> {
        if self.header.text.contains(point) {
            Some(EditFocus::Header)
        } else if self.body.text.contains(point) {
            Some(EditFocus::Content)
        } else if self
            .tags
            .as_ref()
            .is_some_and(|section| section.text.contains(point))
        {
            Some(EditFocus::Tags)
        } else {
            None
        }
    }

    pub fn popup(&self, count: usize) -> Option<Rect> {
        let tags = self.tags.as_ref()?;
        let bottom = tags.text.y - 1;
        let height =
            (count + 2).min(bottom.saturating_sub(self.body.text.y).min(6) as usize) as u16;
        (count > 0 && height >= 3)
            .then(|| Rect::new(self.inner.x, bottom - height, self.inner.width, height))
    }
}

pub fn header_line(note: &Note, focused: bool, width: u16) -> TextLine {
    if focused {
        TextLine::title(&note.title, Some(note.title_cursor), width)
    } else {
        let title = if note.title.is_empty() {
            format!(" ● {}  [{}]", color_name(&note.color), note.border_style)
        } else {
            note.title.clone()
        };
        TextLine::title(&title, None, width)
    }
}

/// Follow only when the caret leaves the existing window. Focus changes keep its rows.
pub fn follow_body(note: &mut Note, rect: Rect, focused: bool) {
    if rect.is_empty() {
        return;
    }
    let (row, col) = note.cursor_pos();
    let lines: Vec<&str> = note.content.split('\n').collect();
    let width = rect.width as usize;
    let height = rect.height as usize;
    note.viewport.top_line = note
        .viewport
        .top_line
        .min(lines.len().saturating_sub(height));
    let cursor_line = TextLine::glyphs(lines[row], focused.then_some(col));
    let max_width = lines
        .iter()
        .enumerate()
        .map(|(index, text)| {
            if index == row {
                cursor_line.iter().map(Glyph::width).sum::<usize>()
            } else {
                cell_width(text)
            }
        })
        .max()
        .unwrap_or(0);
    note.viewport.left_cell = note.viewport.left_cell.min(max_width.saturating_sub(width));
    if focused {
        if cursor_line.iter().map(Glyph::width).sum::<usize>() <= width {
            note.viewport.left_cell = 0;
        }
        if row < note.viewport.top_line {
            note.viewport.top_line = row;
        } else if row >= note.viewport.top_line + height {
            note.viewport.top_line = row + 1 - height;
        }
        let mut cell = 0;
        for glyph in cursor_line {
            if glyph.caret.is_some() {
                if cell < note.viewport.left_cell {
                    note.viewport.left_cell = cell;
                } else if cell + glyph.width() > note.viewport.left_cell + width {
                    note.viewport.left_cell = cell + glyph.width() - width;
                }
                break;
            }
            cell += glyph.width();
        }
    }
}

/// The same windowed line and source offset drive drawing and mouse mapping.
pub fn body_lines(
    note: &Note,
    width: u16,
    focused: bool,
) -> impl Iterator<Item = (TextLine, usize)> + '_ {
    let cursor = focused.then(|| note.cursor.min(char_len(&note.content)));
    note.content
        .split('\n')
        .scan(0, |start, text| {
            let end = *start + char_len(text);
            let line = (text, *start, end);
            *start = end + 1;
            Some(line)
        })
        .skip(note.viewport.top_line)
        .map(move |(text, start, end)| {
            let cursor = cursor
                .filter(|&index| index >= start && index <= end)
                .map(|index| index - start);
            (
                TextLine::window(text, cursor, width, note.viewport.left_cell),
                start,
            )
        })
}

pub fn body_line(note: &Note, row: usize, width: u16, focused: bool) -> (TextLine, usize) {
    body_lines(note, width, focused)
        .nth(row)
        .unwrap_or_else(|| (TextLine::new("", None, width), char_len(&note.content)))
}

pub struct TagChip {
    pub index: usize,
    pub text: String,
    pub rect: Rect,
}
pub struct TagsLayout {
    pub chips: Vec<TagChip>,
    pub input: Option<(TextLine, Rect)>,
}

impl TagsLayout {
    pub fn new(note: &Note, rect: Rect, focused: bool) -> Self {
        let input_shown = focused || !note.tag_input.is_empty();
        let caret = (focused && note.tag_cursor.is_none()).then_some(char_len(&note.tag_input));
        let tags_width: usize = note.tags.iter().map(|tag| cell_width(tag) + 3).sum();
        let total = tags_width
            + if input_shown {
                cell_width(&note.tag_input) + usize::from(caret.is_some())
            } else {
                0
            };
        let selected_width = if focused {
            note.tag_cursor
                .and_then(|i| note.tags.get(i))
                .map_or(0, |tag| cell_width(tag) + 2)
        } else {
            0
        };
        let input_width = if input_shown {
            (cell_width(&note.tag_input) + usize::from(caret.is_some()))
                .min((rect.width as usize).saturating_sub(selected_width.min(rect.width as usize)))
        } else {
            0
        };
        let chip_right = rect.width as usize - input_width;
        // Keep the selected chip in the shared drawing/hit-testing window.
        let mut first = 0;
        if focused && let Some(selected) = note.tag_cursor.filter(|&i| i < note.tags.len()) {
            while first < selected
                && note.tags[first..=selected]
                    .iter()
                    .map(|tag| cell_width(tag) + 3)
                    .sum::<usize>()
                    .saturating_sub(1)
                    > chip_right
            {
                first += 1;
            }
        }
        let mut x = (rect.width as usize).saturating_sub(total);
        let mut chips = Vec::new();
        for (index, tag) in note.tags.iter().enumerate().skip(first) {
            let text = format!("[{tag}]");
            let width = cell_width(&text);
            if x < chip_right {
                chips.push(TagChip {
                    index,
                    text,
                    rect: Rect::new(
                        rect.x + x as u16,
                        rect.y,
                        width.min(chip_right - x) as u16,
                        1,
                    ),
                });
            }
            x += width + 1;
        }
        x = x.min(chip_right);
        let input = (input_shown && x < rect.width as usize).then(|| {
            let rect = Rect::new(rect.x + x as u16, rect.y, rect.width - x as u16, 1);
            (
                TextLine::following(&note.tag_input, caret, rect.width),
                rect,
            )
        });
        Self { chips, input }
    }
}

pub struct Tab {
    pub index: usize,
    pub text: Line<'static>,
    pub rect: Rect,
}
pub struct TabStrip {
    pub tabs: Vec<Tab>,
    pub omitted: usize,
    pub overflow: Option<(String, Rect)>,
}

/// Follow the active visible tab; omitted counts never include filtered-out notes.
/// Counter cells are not clickable tabs, and every hit region is the rendered rect.
pub fn tabs(app: &App, width: u16) -> TabStrip {
    let visible = app.visible_note_indices();
    let labels: Vec<_> = visible
        .iter()
        .map(|&index| {
            let note = &app.notes[index];
            let raw = if !note.title.is_empty() {
                note.title.clone()
            } else if note.first_line().is_empty() {
                "empty".into()
            } else {
                strip_md(note.first_line())
            };
            let marker = if note.tags.is_empty() { "●" } else { "#" };
            format!(" {marker} {raw} ")
        })
        .collect();
    let available = width.saturating_sub(2) as usize;
    let total = labels
        .iter()
        .map(|label| cell_width(label) + 1)
        .sum::<usize>()
        .saturating_sub(1);
    let counter_width = if total > available && visible.len() > 1 {
        (visible.len().to_string().len() + 2).min(available.saturating_sub(1))
    } else {
        0
    };
    let budget = available.saturating_sub(counter_width);
    let active = app
        .selected
        .and_then(|index| visible.iter().position(|&i| i == index))
        .unwrap_or(0);
    let sizes: Vec<_> = labels
        .iter()
        .map(|label| cell_width(label).min(budget))
        .collect();
    let mut first = 0;
    while first < active
        && sizes[first..=active]
            .iter()
            .map(|size| size + 1)
            .sum::<usize>()
            .saturating_sub(1)
            > budget
    {
        first += 1;
    }
    let mut tabs = Vec::new();
    let mut used = 0;
    for pos in first..visible.len() {
        let size = sizes[pos];
        if size == 0 || used + size > budget {
            break;
        }
        tabs.push(Tab {
            index: visible[pos],
            text: TextLine::new(&labels[pos], None, size as u16).render(None, Style::new()),
            rect: Rect::new(1 + used as u16, 0, size as u16, 1),
        });
        used += size + 1;
    }
    let omitted = visible.len() - tabs.len();
    let overflow = (omitted > 0 && counter_width > 0).then(|| {
        (
            format!("+{omitted}"),
            Rect::new(width - 1 - counter_width as u16, 0, counter_width as u16, 1),
        )
    });
    TabStrip {
        tabs,
        omitted,
        overflow,
    }
}

#[cfg(test)]
mod tests;
