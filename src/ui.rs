// stickynote — view / rendering logic.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::app::{App, EditFocus, MenuAction};
use crate::note::{Note, THEMES, Theme, color_name, parse_hex, parse_md};

// ── Main render entry point ───────────────────────────────────────────────────

pub fn render(app: &App, frame: &mut Frame) {
    let area = frame.area();
    let w = area.width;
    let h = area.height;

    if w == 0 || h == 0 {
        return;
    }

    // Clear entire terminal before rendering — prevents ghosted characters
    // in the 1-grid-unit gutter around inset note cards.
    frame.render_widget(Clear, area);
    let theme = &THEMES[app.theme_idx];

    // Overlay mode replaces everything.
    if app.show_overlay && app.count() > 0 && app.selected < app.count() {
        render_overlay(app, frame, theme);
        return;
    }

    if app.count() == 0 && !app.show_help {
        render_welcome(frame, area);
        render_footer(app, frame, theme, h);
        return;
    }

    // ── Tab bar (1 line) ───────────────────────────────────────────────────────
    let footer_h = 2u16;
    let inset_w = w.saturating_sub(2);
    let tab_h = 1u16;
    render_tabs(app, frame, theme, inset_w);

    // ── Content area ──────────────────────────────────────────────────────────
    let content_h = h.saturating_sub(tab_h + footer_h);
    if content_h > 0 && app.count() > 0 {
        let note = &app.notes[app.selected];
        let header_focused = note.editing && app.edit_focus == EditFocus::Header;
        let tag_focused = note.editing && app.edit_focus == EditFocus::Tags;
        let tag_suggestions: Vec<&str> = if tag_focused && !note.tag_input.is_empty() {
            let input = note.tag_input.to_lowercase();
            app.all_tags
                .iter()
                .filter(|t| t.to_lowercase().contains(&input))
                .take(5)
                .map(|s| s.as_str())
                .collect()
        } else {
            Vec::new()
        };
        render_full_note(
            note,
            theme,
            header_focused,
            tag_focused,
            &tag_suggestions,
            Rect::new(1, tab_h, inset_w, content_h),
            frame,
        );
    }

    // ── Overlays ──────────────────────────────────────────────────────────────
    if app.menu.visible {
        render_menu(app, frame, theme);
    }
    if app.show_help {
        render_help(frame, area, theme);
    }

    // ── Footer (2 lines) ──────────────────────────────────────────────────────
    render_footer(app, frame, theme, h);
}

// ── Tab bar ────────────────────────────────────────────────────────────────────

/// Build a tab label string for a note, truncated to fit `remaining` columns.
/// Format: " ● title " (untagged) or " # title " (tagged), truncated if needed.
fn tab_label(note: &Note, remaining: u16) -> String {
    let raw = if !note.title.is_empty() {
        note.title.as_str()
    } else {
        note.first_line()
    };
    let raw = if raw.is_empty() { "empty" } else { raw };
    let marker = if !note.tags.is_empty() { "#" } else { "●" };
    let label = format!(" {} {} ", marker, raw);
    if label.len() as u16 > remaining {
        let max_title = remaining.saturating_sub(4) as usize;
        format!(" {}… ", &raw[..max_title.max(1)])
    } else {
        label
    }
}

/// Render a single-line tab bar showing all notes as colored tabs.
/// The active note's tab is highlighted with the theme's selection border color.
fn render_tabs(app: &App, frame: &mut Frame, theme: &Theme, inset_w: u16) {
    let visible_indices = app.visible_note_indices();
    let mut x = 1u16; // start at the inset column

    for (i, &note_idx) in visible_indices.iter().enumerate() {
        if x >= inset_w {
            break;
        }
        let note = &app.notes[note_idx];
        let remaining = inset_w.saturating_sub(x);
        let label = tab_label(note, remaining);
        let w = label.len() as u16;

        let is_selected = note_idx == app.selected;
        let note_color = parse_hex(&note.color);

        let style = if is_selected {
            Style::new()
                .bg(theme.sel_border)
                .fg(Color::Rgb(0x1a, 0x1a, 0x1a))
        } else {
            Style::new().fg(note_color)
        };

        render_par(frame, &label, style, Rect::new(x, 0, w, 1));
        x += w;

        // Divider between tabs (single space).
        if i < visible_indices.len() - 1 && x < inset_w {
            render_par(frame, " ", Style::new(), Rect::new(x, 0, 1, 1));
            x += 1;
        }
    }

    // Overflow indicator — only when we have more notes than could fit.
    let rendered = visible_indices.len();
    let total = app.notes.len();
    if rendered < total && x < inset_w {
        let more = format!(" +{}", total - rendered);
        render_par(
            frame,
            &more,
            Style::new().fg(Color::Rgb(0x88, 0x88, 0x88)),
            Rect::new(x, 0, more.len() as u16, 1),
        );
    }
}

/// Given an x-coordinate, return the note index whose tab is at that position,
/// or `None` if no tab is at that x.
pub fn note_index_at_tab_x(app: &App, mx: u16) -> Option<usize> {
    let inset_w = app.width.saturating_sub(2);
    let mut x = 1u16;
    let visible = app.visible_note_indices();

    for &note_idx in &visible {
        if x >= inset_w {
            break;
        }
        let note = &app.notes[note_idx];
        let remaining = inset_w.saturating_sub(x);
        let label = tab_label(note, remaining);
        let w = label.len() as u16;

        if mx >= x && mx < x + w {
            return Some(note_idx);
        }
        x += w + 1; // +1 for divider space
    }
    None
}

// ── Full note render ──────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn render_full_note(
    note: &Note,
    theme: &Theme,
    header_focused: bool,
    tag_focused: bool,
    tag_suggestions: &[&str],
    rect: Rect,
    frame: &mut Frame,
) {
    let bg = parse_hex(&note.color);
    let fg = Color::Rgb(0x1a, 0x1a, 0x1a);
    let base_style = Style::new().fg(fg);
    let dim_style = Style::new().fg(fg).dim();

    // ── Border config ───────────────────────────────────────────────────────────
    let border_type = match note.border_style.as_str() {
        "double" => BorderType::Double,
        "thick" => BorderType::Thick,
        "hidden" | "none" => BorderType::Plain,
        _ => BorderType::Rounded,
    };

    let border_fg = if note.editing {
        theme.sel_border
    } else if note.border_style == "hidden" && !note.editing {
        bg
    } else {
        fg
    };

    // ── Inner area ──────────────────────────────────────────────────────────────
    let has_border = note.border_style != "hidden" || note.editing;
    let (inner_x, inner_y, inner_w, inner_h) = if has_border {
        (
            rect.x + 1,
            rect.y + 1,
            rect.width.saturating_sub(2),
            rect.height.saturating_sub(2),
        )
    } else {
        (rect.x, rect.y, rect.width, rect.height)
    };
    let max_line = inner_w.saturating_sub(2); // 1-char padding inside

    // ── Header bar (1 line) — center-justified editable title ──────────────────
    let title_width = if header_focused {
        inner_w.saturating_sub(2)
    } else {
        inner_w
    };
    let has_title = !note.title.is_empty() || header_focused;
    let header_display = if has_title {
        let title_with_cursor = if header_focused {
            let mut t = note.title.clone();
            t.insert(note.title_cursor, '▋');
            t
        } else {
            note.title.clone()
        };
        let title_len = title_with_cursor.len() as u16;
        if title_len <= title_width {
            // Center-justified within inner width.
            let pad = (title_width - title_len) / 2;
            format!("{:pad$}{}", "", title_with_cursor, pad = pad as usize)
        } else {
            // Too long — truncate with cursor priority.
            let max = title_width as usize;
            if header_focused && note.title_cursor >= max {
                // Cursor at/after cutoff — show end with cursor.
                let offset = note.title_cursor.saturating_sub(max.saturating_sub(1));
                let tail: String = note
                    .title
                    .chars()
                    .skip(offset)
                    .take(max.saturating_sub(1))
                    .collect();
                format!("{}▋", tail)
            } else {
                let max_show = max.saturating_sub(1);
                format!("{}…", &title_with_cursor[..max_show])
            }
        }
    } else {
        // Fallback: show color + border info.
        let info = format!(" ● {}  [{}]", color_name(&note.color), note.border_style);
        if info.len() as u16 > title_width {
            let max = title_width.saturating_sub(1) as usize;
            format!("{}…", &info[..max])
        } else {
            // Center-justified.
            let pad = (inner_w - info.len() as u16) / 2;
            format!("{:pad$}{}", "", info, pad = pad as usize)
        }
    };
    let header_style = if header_focused {
        Style::new().fg(theme.sel_border)
    } else {
        Style::new().fg(fg).dim()
    };
    let header_line = Line::from(Span::styled(header_display, header_style));

    // ── Tags section visibility ──────────────────────────────────────────────────
    let has_tags_area =
        !note.tags.is_empty() || note.editing && (tag_focused || !note.tag_input.is_empty());

    // ── Layout with next_y tracking ─────────────────────────────────────────────
    let bg_style = Style::new().bg(bg);

    // Focused section gets a highlighted separator line in sel_border colour.
    let focus_sep = Style::new().fg(theme.sel_border);
    let content_focused = note.editing && !header_focused && !tag_focused;
    let sep_header = if content_focused {
        focus_sep
    } else {
        dim_style
    };
    let sep_tags = if tag_focused { focus_sep } else { dim_style };

    // Separator lines with conditional styles.
    let header_sep_line = Span::styled("─".repeat(inner_w as usize), sep_header);
    let tags_sep_line = Span::styled("─".repeat(inner_w as usize), sep_tags);

    let mut next_y = inner_y;

    // 1. Header
    let header_rect = Rect::new(inner_x, next_y, inner_w, 1);
    next_y += 1;

    // 2. Header separator (─ line runs full inner width)
    let header_sep_y = next_y;
    next_y += 1;

    // Tags section at bottom: separator (1) + tags line (1)
    let tags_section_h = if has_tags_area && inner_y + inner_h > header_sep_y + 1 {
        2u16
    } else {
        0u16
    };
    let body_h = (inner_y + inner_h - next_y).saturating_sub(tags_section_h);

    // 3. Body (content area)
    let body_rect = Rect::new(inner_x, next_y, inner_w, body_h);
    next_y += body_h;

    // 4. Tags separator
    let tags_sep_y = next_y;
    next_y += 1; // only used if tags_section_h > 0

    // 5. Tags line
    let tags_rect = Rect::new(inner_x, next_y, inner_w, 1);

    // ── Focus border (per-section) ────────────────────────────────────────────
    let focus_block = Block::bordered()
        .border_type(BorderType::Plain)
        .border_style(Style::new().fg(fg));

    let inset_w = inner_w.saturating_sub(2); // width inside focus border
    let header_render = if header_focused {
        Rect::new(inner_x + 1, inner_y, inset_w, 1)
    } else {
        header_rect
    };
    let body_render = if content_focused {
        Rect::new(inner_x + 1, body_rect.y, inset_w, body_rect.height)
    } else {
        body_rect
    };
    let tags_render = if tag_focused {
        Rect::new(inner_x + 1, tags_rect.y, inset_w, 1)
    } else {
        tags_rect
    };
    let content_max_line = if content_focused {
        inset_w.saturating_sub(2)
    } else {
        max_line
    };

    // ── Content lines (rebuilt with correct max_line) ─────────────────────────
    let content_lines = note.content_lines();
    let body_lines: Vec<Line> = content_lines
        .iter()
        .map(|l| {
            let display = if l.len() as u16 > content_max_line {
                let max_len = content_max_line as usize;
                format!("{}…", &l[..max_len])
            } else {
                l.clone()
            };
            Line::from(parse_md(&display, base_style))
        })
        .collect();

    // ── Render ──────────────────────────────────────────────────────────────────
    if note.border_style == "hidden" && !note.editing {
        frame.render_widget(Paragraph::new(header_line).style(bg_style), header_render);
        frame.render_widget(
            Paragraph::new(header_sep_line.clone()),
            Rect::new(inner_x, header_sep_y, inner_w, 1),
        );
        frame.render_widget(Paragraph::new(body_lines).style(bg_style), body_render);
        if tags_section_h > 0 {
            frame.render_widget(
                Paragraph::new(tags_sep_line.clone()),
                Rect::new(inner_x, tags_sep_y, inner_w, 1),
            );
            render_tags_chips(
                note,
                tag_focused,
                theme,
                bg_style,
                tags_render,
                frame,
                tags_render.width,
            );
        }
    } else {
        let block = Block::bordered()
            .border_type(border_type)
            .border_style(Style::new().fg(border_fg))
            .style(bg_style);
        frame.render_widget(&block, rect);

        // ── Per-section focus borders ───────────────────
        if header_focused {
            frame.render_widget(&focus_block, header_rect);
        }
        if content_focused {
            frame.render_widget(&focus_block, body_rect);
        }
        if tag_focused {
            frame.render_widget(&focus_block, tags_rect);
        }

        frame.render_widget(Paragraph::new(header_line).style(bg_style), header_render);
        frame.render_widget(
            Paragraph::new(header_sep_line.clone()),
            Rect::new(inner_x, header_sep_y, inner_w, 1),
        );
        frame.render_widget(Paragraph::new(body_lines).style(bg_style), body_render);
        if tags_section_h > 0 {
            frame.render_widget(
                Paragraph::new(tags_sep_line.clone()),
                Rect::new(inner_x, tags_sep_y, inner_w, 1),
            );
            render_tags_chips(
                note,
                tag_focused,
                theme,
                bg_style,
                tags_render,
                frame,
                tags_render.width,
            );
        }
    }

    // ── Autocomplete popup (above tags, overlaying content) ─────────────────────
    if tag_focused && !tag_suggestions.is_empty() && tags_section_h > 0 {
        // Each suggestion = 1 row, plus 2 for the border (top + bottom).
        let max_popup_h = tags_sep_y.saturating_sub(body_rect.y).min(6);
        let popup_h = ((tag_suggestions.len() as u16) + 2).min(max_popup_h);
        if popup_h >= 3 {
            let content_rows = (popup_h - 2) as usize;
            let popup_rect = Rect::new(inner_x, tags_sep_y - popup_h, inner_w, popup_h);
            let visible_suggestions = &tag_suggestions[..tag_suggestions.len().min(content_rows)];

            let suggestion_lines: Vec<Line> = visible_suggestions
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let style = if i == 0 {
                        Style::new()
                            .fg(Color::White)
                            .bg(Color::Rgb(0x55, 0x55, 0x55))
                    } else {
                        Style::new().fg(Color::Rgb(0xcc, 0xcc, 0xcc))
                    };
                    Line::from(Span::styled(format!("  {}", s), style))
                })
                .collect();

            frame.render_widget(Clear, popup_rect);
            frame.render_widget(
                Paragraph::new(suggestion_lines)
                    .style(Style::new().bg(Color::Rgb(0x22, 0x22, 0x22)))
                    .block(
                        Block::bordered()
                            .border_type(BorderType::Plain)
                            .border_style(Style::new().fg(Color::Rgb(0x88, 0x88, 0x88))),
                    ),
                popup_rect,
            );
        }
    }
}

/// Render tags as `[bracketed]` chips, right-aligned within `rect`.
/// Highlights the selected tag when `tag_focused` is true.
#[allow(clippy::too_many_arguments)]
fn render_tags_chips(
    note: &Note,
    tag_focused: bool,
    theme: &Theme,
    bg_style: Style,
    rect: Rect,
    frame: &mut Frame,
    inner_w: u16,
) {
    let fg = Color::Rgb(0x1a, 0x1a, 0x1a);

    // No tags and not focused → dim placeholder, right-aligned.
    if note.tags.is_empty() && note.tag_input.is_empty() && !tag_focused {
        let placeholder = format!("{:>width$}", "(no tags)", width = inner_w as usize);
        let line = Line::from(Span::styled(placeholder, Style::new().fg(fg).dim()));
        frame.render_widget(Paragraph::new(line).style(bg_style), rect);
        return;
    }

    let mut spans: Vec<Span> = Vec::new();

    // Render existing tags as [bracketed] chips.
    for (i, tag) in note.tags.iter().enumerate() {
        let is_selected = tag_focused && note.tag_cursor == Some(i);
        let tag_style = if is_selected {
            Style::new().fg(theme.sel_border)
        } else {
            Style::new().fg(fg)
        };
        spans.push(Span::styled(format!("[{}]", tag), tag_style));
        spans.push(Span::styled(" ", Style::new()));
    }

    // Tag input area (typed-but-not-yet-committed text).
    if tag_focused || !note.tag_input.is_empty() {
        let show_cursor = tag_focused && note.tag_cursor.is_none();
        let input_text = if show_cursor {
            format!("{}▋", note.tag_input)
        } else {
            note.tag_input.clone()
        };
        spans.push(Span::styled(input_text, Style::new().fg(fg)));
    }

    let line = Line::from(spans);
    let line_w = line.width() as u16;

    // Right-align by prepending spaces.
    let display = if line_w < inner_w {
        let pad = (inner_w - line_w) as usize;
        let mut padded = vec![Span::styled(" ".repeat(pad), Style::new())];
        padded.extend(line.spans);
        Line::from(padded)
    } else {
        line
    };

    frame.render_widget(Paragraph::new(display).style(bg_style), rect);
}

// ── Overlay (full-screen editing) ─────────────────────────────────────────────

fn render_overlay(app: &App, frame: &mut Frame, theme: &Theme) {
    let area = frame.area();
    let w = area.width;
    let h = area.height;

    if app.count() == 0 || app.selected >= app.count() {
        return;
    }

    let note = &app.notes[app.selected];
    // The note should already be in editing mode, but ensure cursor is set.
    let note_clone = Note {
        cursor: if !note.content.is_empty() || note.editing {
            note.cursor
        } else {
            note.content.len()
        },
        ..note.clone()
    };

    // Centre the note vertically with a small top margin.
    let render_h = (h as f64 * 0.6) as u16;
    let render_y = (h - render_h) / 3;
    let note_rect = Rect::new(1, render_y, w.saturating_sub(2), render_h);

    render_full_note(&note_clone, theme, false, false, &[], note_rect, frame);

    // Footer with overlay-specific hints.
    render_bar(
        frame,
        " Esc:close  │  full-screen overlay",
        Style::new().bg(theme.status_bg).fg(theme.status_fg),
        Rect::new(0, h - 2, w, 1),
    );
    render_bar(
        frame,
        " Tab:focus  Enter:newline  Backspace:delete  ←/→:move cursor",
        Style::new().bg(theme.hint_bg).fg(theme.hint_fg),
        Rect::new(0, h - 1, w, 1),
    );
}

// ── Welcome screen ────────────────────────────────────────────────────────────

fn render_welcome(frame: &mut Frame, area: Rect) {
    let w = area.width;
    let h = area.height;

    let lines = [
        "  Stickynote Board  ",
        "",
        "  n   new note         d  delete note",
        "  e   toggle edit      c  cycle color",
        "  b   cycle border    ^d  duplicate",
        "  t   add tag          T  filter by tag",
        "  /   search tags      O  overlay view",
        "  ←/→ navigate tabs   ^R  cycle theme",
        "",
        "  Click a tab to select, double-click to edit",
        "  Right-click a tab for options",
        "  Middle-click a tab to delete",
    ];
    let content = lines.join("\n");

    let box_w = 42u16;
    let box_h = lines.len() as u16 + 4; // padding + borders
    let start_x = (w.saturating_sub(box_w)) / 2;
    let start_y = (h.saturating_sub(box_h)) / 3;

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Rgb(0x88, 0x88, 0x88)))
        .style(Style::new())
        .padding(ratatui::widgets::Padding::horizontal(2));

    let par = Paragraph::new(content)
        .block(block)
        .style(Style::new().fg(Color::Rgb(0xcc, 0xcc, 0xcc)));
    frame.render_widget(par, Rect::new(start_x, start_y, box_w, box_h));
}

// ── Context menu ──────────────────────────────────────────────────────────────

fn render_menu(app: &App, frame: &mut Frame, _theme: &Theme) {
    let labels: Vec<&str> = app
        .menu
        .items
        .iter()
        .map(|a| match a {
            MenuAction::Edit => "Edit",
            MenuAction::Color => "Color",
            MenuAction::Border => "Border",
            MenuAction::Tag => "Tag",
            MenuAction::Delete => "Delete",
            MenuAction::NewNote => "New note",
            MenuAction::Close => "Close",
        })
        .collect();

    let inner_w = 14u16;
    let menu_h = labels.len() as u16 + 2; // border top/bottom
    let menu_w = inner_w + 2; // border left/right

    let mut items = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        let is_selected = i == app.menu.selected;
        let item_style = if is_selected {
            Style::new()
                .bg(Color::Rgb(0x55, 0x55, 0x55))
                .fg(Color::White)
        } else {
            Style::new()
                .bg(Color::Rgb(0x22, 0x22, 0x22))
                .fg(Color::White)
        };
        items.push(Line::from(Span::styled(
            format!("{label: <width$}", width = inner_w as usize),
            item_style,
        )));
    }

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::Rgb(0xaa, 0xaa, 0xaa)))
        .style(Style::new().bg(Color::Rgb(0x11, 0x11, 0x11)));

    let par = Paragraph::new(items).block(block);
    let menu_rect = Rect::new(app.menu.x, app.menu.y, menu_w, menu_h);
    frame.render_widget(par, menu_rect);
}

// ── Help overlay ──────────────────────────────────────────────────────────────

fn render_help(frame: &mut Frame, area: Rect, theme: &Theme) {
    let w = area.width;

    let lines = [
        "  Stickynote  --  Key Bindings  ",
        "",
        "  n        New note",
        "  ^d       Duplicate note",
        "  e/enter  Toggle edit mode",
        "  d        Delete (confirm)",
        "  c        Cycle colour",
        "  b        Cycle border style",
        "  t        Add tag",
        "  ^t       Clear all tags (confirm)",
        "  T        Toggle tag filter",
        "  /        Filter by tag",
        "  ←/→      Tags: select tag to delete",
        "",
        "  Inline editing (Tab cycles focus):",
        "  Header   Edit note title",
        "  Tags     Type + Enter to add | ←/→ select | Del",
        "  ←/→      Navigate tabs",
        "  ?        Toggle this help",
        "  ^R       Cycle theme",
        "  esc      Cancel / close",
        "  q        Quit",
        "",
        "  Click tab       Select note",
        "  Double-click    Select + edit",
        "  Right-click     Context menu",
        "  Middle-click    Delete note",
        "  Scroll wheel    Navigate tabs",
        "",
        "  Press any key or click to close",
    ];
    let content = lines.join("\n");

    let box_h = lines.len() as u16 + 2;
    let box_w = 44u16.min(w.saturating_sub(4));

    let start_x = (w.saturating_sub(box_w)) / 2;
    let start_y = (area.height.saturating_sub(box_h)) / 4;
    let help_rect = Rect::new(start_x, start_y, box_w, box_h);

    // Clear the area behind the help overlay so underlying content doesn't bleed through.
    frame.render_widget(Clear, help_rect);

    let block = Block::bordered()
        .border_type(BorderType::Double)
        .border_style(Style::new().fg(theme.sel_border))
        .style(Style::new().bg(theme.hint_bg))
        .padding(ratatui::widgets::Padding::horizontal(2));

    let par = Paragraph::new(content)
        .block(block)
        .style(Style::new().fg(theme.status_fg));
    frame.render_widget(par, help_rect);
}

// ── Footer ────────────────────────────────────────────────────────────────────

fn render_footer(app: &App, frame: &mut Frame, theme: &Theme, h: u16) {
    let w = frame.area().width;

    // Status bar (line h-1).
    if h >= 2 {
        render_bar(
            frame,
            &app.status_text(),
            Style::new().bg(theme.status_bg).fg(theme.status_fg),
            Rect::new(0, h - 2, w, 1),
        );
    }

    // Hint bar (line h).
    if h >= 1 {
        render_bar(
            frame,
            app.hint_bar(),
            Style::new().bg(theme.hint_bg).fg(theme.hint_fg),
            Rect::new(0, h - 1, w, 1),
        );
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Render a single-line paragraph into a rect with the given text and style.
fn render_par(frame: &mut Frame, text: &str, style: Style, rect: Rect) {
    frame.render_widget(Paragraph::new(text).style(style), rect);
}

/// Render a bar (full-width single line) with text and style.
fn render_bar(frame: &mut Frame, text: &str, style: Style, rect: Rect) {
    frame.render_widget(Paragraph::new(text).style(style), rect);
}
