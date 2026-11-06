// stickynote — view / rendering logic.

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};

use crate::app::{App, EditFocus, InputMode, MenuAction, QuitState};
use crate::layout::{self, NoteLayout, TagsLayout};
use crate::note::{Note, THEMES, Theme, parse_hex, parse_md};

// ── Main render entry point ───────────────────────────────────────────────────

/// Render the complete UI for the current application state into `frame`.
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

    if w < 12 || h < 8 {
        render_par(
            frame,
            "Terminal too small. Resize to at least 12x8.",
            Style::new().fg(theme.status_fg),
            area,
        );
        render_quit_prompt(app, frame, theme);
        return;
    }

    // Overlay mode replaces everything.
    if app.show_overlay && app.selected.is_some() {
        render_overlay(app, frame, theme);
        render_quit_prompt(app, frame, theme);
        return;
    }

    if app.count() == 0
        && app.filter_tag.is_empty()
        && app.search_query.is_empty()
        && app.mode != InputMode::SearchInput
        && !app.show_help
    {
        render_welcome(frame, area, theme);
        render_footer(app, frame, theme, h);
        render_file_prompt(app, frame, theme);
        render_quit_prompt(app, frame, theme);
        return;
    }

    // ── Tab bar (1 line) ───────────────────────────────────────────────────────
    render_tabs(app, frame, theme);

    // ── Content area ──────────────────────────────────────────────────────────
    if let Some(rect) = layout::card_rect(w, h, false) {
        if app.selected.is_some() {
            render_selected_note(app, frame, theme, rect);
        } else {
            render_par(
                frame,
                &format!(
                    "No notes match.\nTag: {}   Search: {}\nEsc: clear filters   /: tag   s: search   n: new note",
                    app.filter_tag, app.search_query
                ),
                Style::new().fg(theme.status_fg),
                rect,
            );
        }
    }

    // ── Overlays ──────────────────────────────────────────────────────────────
    if app.menu.visible {
        render_menu(app, frame, theme);
    }
    if app.mode == InputMode::SearchInput {
        render_search_results(app, frame, theme);
    }
    if app.show_help {
        render_help(app, frame, area, theme);
    }

    // ── Footer (2 lines) ──────────────────────────────────────────────────────
    render_footer(app, frame, theme, h);
    render_file_prompt(app, frame, theme);
    render_quit_prompt(app, frame, theme);
}

// Search results follow the same visible selection used by tabs and note actions.
fn render_search_results(app: &App, frame: &mut Frame, theme: &Theme) {
    let Some(rect) = layout::card_rect(frame.area().width, frame.area().height, false) else {
        return;
    };
    let block = Block::bordered()
        .title(format!(" Search / jump · {} results ", app.visible_count()))
        .style(Style::new().bg(theme.hint_bg).fg(theme.hint_fg));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    if inner.is_empty() {
        return;
    }
    let visible = app.visible_note_indices();
    let position = app
        .selected
        .and_then(|selected| visible.iter().position(|&index| index == selected))
        .unwrap_or(0);
    let start = position.saturating_sub(inner.height as usize - 1);
    let lines: Vec<Line> = visible
        .iter()
        .skip(start)
        .take(inner.height as usize)
        .map(|&index| {
            let note = &app.notes[index];
            let label = if note.title.is_empty() {
                note.content.lines().next().unwrap_or("(empty note)")
            } else {
                &note.title
            };
            let selected = Some(index) == app.selected;
            Line::styled(
                format!(
                    "{} {}: {} [{}]",
                    if selected { "▶" } else { " " },
                    index + 1,
                    label,
                    note.tags.join(",")
                ),
                if selected {
                    Style::new().bg(theme.sel_border).fg(Color::Black)
                } else {
                    Style::new().fg(theme.hint_fg).bg(theme.hint_bg)
                },
            )
        })
        .collect();
    if lines.is_empty() {
        frame.render_widget(
            Paragraph::new("No matches. Backspace to revise; Esc clears search.")
                .style(Style::new().fg(theme.hint_fg).bg(theme.hint_bg)),
            inner,
        );
    } else {
        frame.render_widget(Paragraph::new(lines), inner);
    }
}

// ── Tab bar ────────────────────────────────────────────────────────────────────

/// Tabs and their hit regions come from the same cell-width projection.
fn render_tabs(app: &App, frame: &mut Frame, theme: &Theme) {
    let strip = layout::tabs(app, frame.area().width);
    for tab in strip.tabs {
        let style = if Some(tab.index) == app.selected {
            Style::new()
                .bg(theme.sel_border)
                .fg(Color::Rgb(0x1a, 0x1a, 0x1a))
        } else {
            Style::new().fg(parse_hex(&app.notes[tab.index].color))
        };
        frame.render_widget(Paragraph::new(tab.text).style(style), tab.rect);
    }
    if strip.omitted > 0
        && let Some((text, rect)) = strip.overflow
    {
        frame.render_widget(
            Paragraph::new(text).style(Style::new().fg(theme.hint_fg)),
            rect,
        );
    }
}

pub fn note_index_at_tab_x(app: &App, mx: u16) -> Option<usize> {
    layout::tabs(app, app.width)
        .tabs
        .into_iter()
        .find(|tab| tab.rect.contains(ratatui::layout::Position::new(mx, 0)))
        .map(|tab| tab.index)
}

fn render_selected_note(app: &App, frame: &mut Frame, theme: &Theme, rect: Rect) {
    let Some(selected) = app.selected else { return };
    let note = &app.notes[selected];
    let focus = note.editing.then_some(app.edit_focus);
    let tag_focused = focus == Some(EditFocus::Tags);
    let suggestions = if tag_focused {
        app.tag_suggestions()
    } else {
        Vec::new()
    };
    render_full_note(note, theme, focus, &suggestions, rect, frame);
}

// ── Full note render ──────────────────────────────────────────────────────────

fn render_full_note(
    note: &Note,
    theme: &Theme,
    focus: Option<EditFocus>,
    tag_suggestions: &[&str],
    rect: Rect,
    frame: &mut Frame,
) {
    let layout = NoteLayout::new(rect, note, focus);
    let bg = parse_hex(&note.color);
    let fg = Color::Rgb(0x1a, 0x1a, 0x1a);
    let base_style = Style::new().fg(fg);
    let dim_style = base_style.dim();
    let bg_style = Style::new().bg(bg);
    if focus.is_some_and(|focus| !layout.editing_visible(focus)) {
        frame.render_widget(
            Paragraph::new("Resize\nwindow")
                .alignment(Alignment::Center)
                .style(bg_style.fg(fg))
                .block(Block::bordered()),
            layout.card,
        );
        return;
    }
    if note.border_style == "none" || note.border_style == "hidden" && !note.editing {
        frame.render_widget(Block::new().style(bg_style), layout.card);
    } else {
        let border_type = match note.border_style.as_str() {
            "double" => BorderType::Double,
            "thick" => BorderType::Thick,
            "hidden" => BorderType::Plain,
            _ => BorderType::Rounded,
        };
        frame.render_widget(
            Block::bordered()
                .border_type(border_type)
                .border_style(Style::new().fg(if note.editing { Color::Black } else { fg }))
                .style(bg_style),
            layout.card,
        );
    }
    let focus_block = || {
        Block::bordered()
            .border_type(BorderType::Thick)
            .border_style(Style::new().fg(Color::Black))
            .style(bg_style)
    };
    for section in [&layout.header, &layout.body]
        .into_iter()
        .chain(layout.tags.iter())
    {
        if let Some(rect) = section.border {
            frame.render_widget(focus_block(), rect);
        }
    }
    let header_focused = focus == Some(EditFocus::Header);
    let content_focused = focus == Some(EditFocus::Content);
    let tag_focused = focus == Some(EditFocus::Tags);
    let header = layout::header_line(note, header_focused, layout.header.text.width).render(
        header_focused
            .then(|| note.title_selection_range())
            .flatten(),
        if header_focused {
            Style::new().fg(theme.sel_border)
        } else {
            dim_style
        },
    );
    frame.render_widget(Paragraph::new(header).style(bg_style), layout.header.text);
    if !header_focused && !content_focused {
        render_par(
            frame,
            &"─".repeat(layout.inner.width as usize),
            dim_style,
            layout.header_separator,
        );
    }
    let body_lines = if note.editing {
        render_edit_content(note, layout.body.text, content_focused, base_style)
    } else {
        note.content
            .split('\n')
            .map(|text| Line::from(parse_md(text, base_style)))
            .collect()
    };
    let body = Paragraph::new(body_lines).style(bg_style);
    // Reading reflows styled text only. Editing and its source/cell mapping stay unwrapped.
    let body = if note.editing {
        body
    } else {
        body.wrap(ratatui::widgets::Wrap { trim: false })
    };
    frame.render_widget(body, layout.body.text);
    if let Some(tags) = &layout.tags {
        if !content_focused
            && !tag_focused
            && let Some(rect) = layout.tags_separator
        {
            render_par(
                frame,
                &"─".repeat(layout.inner.width as usize),
                dim_style,
                rect,
            );
        }
        render_tags_chips(note, tag_focused, theme, bg_style, tags.text, frame);
    }
    if tag_focused && let Some(popup) = layout.popup(tag_suggestions.len()) {
        let suggestions: Vec<Line> = tag_suggestions
            .iter()
            .take(popup.height.saturating_sub(2) as usize)
            .enumerate()
            .map(|(index, text)| {
                let style = if index == 0 {
                    Style::new()
                        .fg(Color::White)
                        .bg(Color::Rgb(0x55, 0x55, 0x55))
                } else {
                    Style::new().fg(Color::Rgb(0xcc, 0xcc, 0xcc))
                };
                Line::from(Span::styled(format!("  {text}"), style))
            })
            .collect();
        frame.render_widget(Clear, popup);
        frame.render_widget(
            Paragraph::new(suggestions)
                .style(Style::new().bg(Color::Rgb(0x22, 0x22, 0x22)))
                .block(
                    Block::bordered().border_style(Style::new().fg(Color::Rgb(0x88, 0x88, 0x88))),
                ),
            popup,
        );
    }
}

/// Render tags as `[bracketed]` chips, right-aligned within `rect`.
/// Highlights the selected tag when `tag_focused` is true.
fn render_tags_chips(
    note: &Note,
    focused: bool,
    theme: &Theme,
    bg_style: Style,
    rect: Rect,
    frame: &mut Frame,
) {
    let fg = Color::Rgb(0x1a, 0x1a, 0x1a);
    let layout = TagsLayout::new(note, rect, focused);
    for chip in layout.chips {
        let color = if focused && note.tag_cursor == Some(chip.index) {
            theme.sel_border
        } else {
            fg
        };
        frame.render_widget(
            Paragraph::new(chip.text).style(bg_style.fg(color)),
            chip.rect,
        );
    }
    if let Some((line, rect)) = layout.input {
        frame.render_widget(
            Paragraph::new(line.render(None, Style::new().fg(fg))).style(bg_style),
            rect,
        );
    }
}

// ── Overlay (full-screen editing) ─────────────────────────────────────────────

fn render_overlay(app: &App, frame: &mut Frame, theme: &Theme) {
    let area = frame.area();
    let w = area.width;
    let h = area.height;

    if let Some(rect) = layout::card_rect(w, h, true) {
        render_selected_note(app, frame, theme, rect);
    }

    // Overlay status and shortcuts follow the same active focus as inline editing.
    render_bar(
        frame,
        &format!(" Overlay │ {}", app.status_text()),
        Style::new().bg(theme.status_bg).fg(theme.status_fg),
        Rect::new(0, h - 2, w, 1),
    );
    render_bar(
        frame,
        app.hint_bar(),
        Style::new().bg(theme.hint_bg).fg(theme.hint_fg),
        Rect::new(0, h - 1, w, 1),
    );
}

// ── Welcome screen ────────────────────────────────────────────────────────────

fn render_welcome(frame: &mut Frame, area: Rect, theme: &Theme) {
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

    let box_w = 42u16.min(w.saturating_sub(2));
    let box_h = (lines.len() as u16 + 4).min(h.saturating_sub(2));
    let start_x = (w.saturating_sub(box_w)) / 2;
    let start_y = (h.saturating_sub(box_h)) / 3;

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.status_fg).dim())
        .style(Style::new().bg(theme.hint_bg))
        .padding(ratatui::widgets::Padding::horizontal(2));

    let par = Paragraph::new(content)
        .block(block)
        .style(Style::new().fg(theme.status_fg));
    frame.render_widget(par, Rect::new(start_x, start_y, box_w, box_h));
}

// ── Context menu ──────────────────────────────────────────────────────────────

fn render_menu(app: &App, frame: &mut Frame, theme: &Theme) {
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

    let mut items = Vec::new();
    for (i, label) in labels.iter().enumerate() {
        let is_selected = i == app.menu.selected;
        let item_style = if is_selected {
            Style::new().bg(theme.sel_border).fg(theme.status_bg)
        } else {
            Style::new().bg(theme.hint_bg).fg(theme.status_fg)
        };
        items.push(Line::from(Span::styled(
            format!("{label: <width$}", width = inner_w as usize),
            item_style,
        )));
    }

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.sel_border))
        .style(Style::new().bg(theme.hint_bg));

    let par = Paragraph::new(items).block(block);
    frame.render_widget(par, menu_rect(app, frame.area()));
}

/// Menu geometry shared with mouse dispatch; the footer is never a menu target.
pub fn menu_rect(app: &App, area: Rect) -> Rect {
    let available_w = area.width.saturating_sub(2);
    let available_h = area.height.saturating_sub(2);
    let width = 16.min(available_w);
    let height = (app.menu.items.len() as u16 + 2).min(available_h);
    Rect::new(
        app.menu.x.min(available_w - width),
        app.menu.y.min(available_h - height),
        width,
        height,
    )
}

// ── Help overlay ──────────────────────────────────────────────────────────────

pub const HELP_LINES: &[&str] = &[
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
    "  s        Text search / jump (title, body, tags)",
    "  ↑/↓, Tab Search: choose result; Enter jumps",
    "  Esc      Search: clear query; normal: clear filters",
    "  ^z / ^y  Undo / redo board changes (also editing)",
    "  ←/→      Tags: select tag to delete",
    "",
    "  Inline editing (Tab cycles focus):",
    "  Header   Edit note title",
    "  Content  Edit note body",
    "  Tags     Enter:add; Tab:complete then next focus",
    "",
    "  Text selection (Header / Content):",
    "  ^a       Select all",
    "  ^c       Copy selected text",
    "  ^x       Cut selected text",
    "  ^v       Paste clipboard / terminal paste supported",
    "  ←/→, Backspace/Delete: whole graphemes",
    "  ^←/→     Word movement (Shift extends selection)",
    "  ^Backspace/Delete: delete to word boundary",
    "  Words: letters/digits/_, punctuation/emoji runs",
    "  Shift+←/→ Extend selection left/right",
    "  Shift+↑/↓ Extend selection up/down",
    "  Mouse    Click to position, drag to select",
    "  Reading wraps; editing is literal, unwrapped",
    "  Preview click: enter Header, caret at end",
    "",
    "  ←/→      Navigate tabs",
    "  PgUp/Dn  Page help; Home/End jump",
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
    "  ^s       Retry saving unsaved changes",
    "  E / ^E   Export selected / visible notes",
    "  Tab      Export: extension / Markdown / plain text",
    "  B        Open existing board (save first)",
    "  CLI: stickynote restore --yes (validated .bak)",
];
pub fn help_page_height(height: u16) -> usize {
    (HELP_LINES.len() + 2)
        .min(height.saturating_sub(2) as usize)
        .saturating_sub(2)
}

pub fn help_max_scroll(height: u16) -> usize {
    HELP_LINES.len().saturating_sub(help_page_height(height))
}

fn render_help(app: &App, frame: &mut Frame, area: Rect, theme: &Theme) {
    let w = area.width;
    let box_h = (help_page_height(area.height) + 2) as u16;
    let offset = app.help_scroll.min(help_max_scroll(area.height));
    let content = HELP_LINES[offset..].join("\n");
    let box_w = 72u16.min(w.saturating_sub(4));

    let start_x = (w.saturating_sub(box_w)) / 2;
    let start_y = (area.height.saturating_sub(box_h)) / 4;
    let help_rect = Rect::new(start_x, start_y, box_w, box_h);

    // Clear the area behind the help overlay so underlying content doesn't bleed through.
    frame.render_widget(Clear, help_rect);

    let block = Block::bordered()
        .title_bottom(" ↑/↓ PgUp/Dn · Esc/?/q:close ")
        .border_type(BorderType::Double)
        .border_style(Style::new().fg(theme.sel_border))
        .style(Style::new().bg(theme.hint_bg))
        .padding(ratatui::widgets::Padding::horizontal(2));

    let par = Paragraph::new(content)
        .block(block)
        .style(Style::new().fg(theme.status_fg));
    frame.render_widget(par, help_rect);
}

fn render_file_prompt(app: &App, frame: &mut Frame, theme: &Theme) {
    let title = match app.mode {
        InputMode::BoardInput => " Open existing board ".to_string(),
        InputMode::ExportInput { visible, format } => format!(
            " Export {} · {} ",
            if visible {
                "visible notes"
            } else {
                "selected note"
            },
            format
                .map(crate::persistence::ExportFormat::name)
                .unwrap_or(".md/.txt extension")
        ),
        _ => return,
    };
    let area = frame.area();
    let width = 76.min(area.width.saturating_sub(2));
    let height = 14.min(area.height.saturating_sub(2));
    let rect = Rect::new(
        (area.width - width) / 2,
        (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .title(title)
        .style(Style::new().fg(theme.hint_fg).bg(theme.hint_bg));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    if inner.is_empty() {
        return;
    }
    let line = layout::TextLine::following(
        &app.input,
        Some(crate::note::char_len(&app.input)),
        inner.width,
    )
    .render(None, Style::new().fg(theme.hint_fg));
    // The single-line path follows its caret independently of wrapped errors.
    frame.render_widget(line, Rect::new(inner.x, inner.y, inner.width, 1));
    let content = format!(
        "{}\n\n{}\n\nActive board: {}",
        app.hint_bar(),
        app.file_message,
        app.board_path().display()
    );
    frame.render_widget(
        Paragraph::new(content)
            .style(Style::new().fg(theme.hint_fg).bg(theme.hint_bg))
            .wrap(ratatui::widgets::Wrap { trim: false }),
        Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(1),
        ),
    );
}

fn render_quit_prompt(app: &App, frame: &mut Frame, theme: &Theme) {
    let instruction = match app.quit_state {
        QuitState::SaveFailed => {
            "Enter/r: retry   Esc: cancel quit\nd: discard options\n\nSave failed. Changes are still in memory."
        }
        QuitState::ConfirmDiscard => {
            "Discard ALL unsaved changes and quit?\n\ny: discard and quit   n/Esc: go back"
        }
        QuitState::Running | QuitState::Done => return,
    };
    let content = format!("{}\n\n{}", instruction, app.save_error);
    let area = frame.area();
    let width = 72.min(area.width);
    let height = 12.min(area.height);
    let rect = Rect::new(
        (area.width - width) / 2,
        (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(content)
            .style(Style::new().fg(theme.status_fg).bg(theme.hint_bg))
            .block(Block::bordered().title("Unsaved board"))
            .wrap(ratatui::widgets::Wrap { trim: false }),
        rect,
    );
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

// ── Selection rendering ──────────────────────────────────────────────────────

fn render_edit_content(note: &Note, rect: Rect, focused: bool, style: Style) -> Vec<Line<'static>> {
    layout::body_lines(note, rect.width, focused)
        .take(rect.height as usize)
        .map(|(line, source_start)| {
            let selection = note.content_selection_range().map(|(start, end)| {
                (
                    start.saturating_sub(source_start),
                    end.saturating_sub(source_start),
                )
            });
            line.render(selection, style)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::note::char_len;
    use crate::persistence::BoardStore;
    use ratatui::{Terminal, backend::TestBackend, style::Modifier};

    #[test]
    fn file_prompts_render_path_caret_format_errors_and_active_board_with_bounded_geometry() {
        let (_dir, mut app) = test_app();
        app.mode = InputMode::ExportInput {
            visible: true,
            format: Some(crate::persistence::ExportFormat::Text),
        };
        app.input = format!("{}末尾.txt", "long/".repeat(30));
        app.file_message = "choose a new path".into();
        let terminal = draw(&app, 80, 24);
        let contents = screen(&terminal);
        assert!(contents.contains("plain text"));
        assert!(contents.contains(".txt█"));
        symbol_position(&terminal, "末", None);
        symbol_position(&terminal, "尾", None);
        assert!(contents.contains("choose a new path"));
        assert!(contents.contains("Active board:"));
        for (w, h) in [(0, 0), (12, 8), (20, 10), (40, 12)] {
            draw(&app, w, h);
        }
        app.mode = InputMode::BoardInput;
        let contents = screen(&draw(&app, 80, 24));
        assert!(contents.contains("Open existing board"));
        assert!(contents.contains("save and switch"));
    }

    #[test]
    fn reading_markdown_reflows_on_resize_without_changing_source_or_editor_state() {
        let (_dir, mut app) = test_app();
        app.add_note();
        app.notes[0].title = "Reading".into();
        app.notes[0].content = "**alpha beta gamma delta** e\u{301} 🇫🇮 👩🏽‍💻 tail".into();
        app.notes[0].cursor = 3;
        app.notes[0].sel_start = Some(0);
        app.notes[0].sel_end = Some(3);
        app.dirty = false;
        let source = app.to_save_data();
        let narrow = draw_editor(&mut app, 24, 24);
        let wide = draw_editor(&mut app, 80, 24);
        let body_y = NoteLayout::new(
            layout::card_rect(24, 24, false).unwrap(),
            &app.notes[0],
            None,
        )
        .body
        .text
        .y;
        let alpha_narrow = symbol_position(&narrow, "a", Some(body_y));
        let delta_narrow = symbol_position(&narrow, "d", Some(body_y + 1));
        let alpha_wide = symbol_position(&wide, "a", Some(body_y));
        let delta_wide = symbol_position(&wide, "d", Some(body_y));
        assert!(delta_narrow.y > alpha_narrow.y);
        assert_eq!(delta_wide.y, alpha_wide.y);
        for point in [alpha_narrow, delta_narrow] {
            assert!(
                narrow.backend().buffer()[point]
                    .modifier
                    .contains(Modifier::BOLD)
            );
        }
        assert!(!screen(&narrow).contains("**"));
        for cluster in ["e\u{301}", "🇫🇮", "👩🏽‍💻"] {
            symbol_position(&narrow, cluster, None);
        }
        for (width, height) in [(12, 8), (18, 16), (28, 24), (80, 24)] {
            draw_editor(&mut app, width, height);
        }
        assert_eq!(app.to_save_data(), source);
        assert_eq!(
            (
                app.notes[0].cursor,
                app.notes[0].sel_start,
                app.notes[0].sel_end
            ),
            (3, Some(0), Some(3))
        );
        assert!(!app.dirty);
    }

    #[test]
    fn preview_wraps_overlong_unicode_words_but_editing_stays_literal_and_unwrapped() {
        let (_dir, mut app) = test_app();
        app.add_note();
        app.notes[0].content = format!("{}末", "中e\u{301}".repeat(12));
        let preview = draw_editor(&mut app, 24, 24);
        let tail = symbol_position(&preview, "末", None);
        let first = symbol_position(&preview, "中", None);
        assert!(tail.y > first.y);
        app.toggle_edit();
        app.edit_focus = EditFocus::Content;
        app.notes[0].cursor = 0;
        let editing = draw_editor(&mut app, 24, 24);
        let card = layout::card_rect(24, 24, false).unwrap();
        let rect = NoteLayout::new(card, &app.notes[0], Some(EditFocus::Content))
            .body
            .text;
        let buffer = editing.backend().buffer();
        for y in rect.y + 1..rect.bottom() {
            for x in rect.x..rect.right() {
                assert_eq!(buffer[(x, y)].symbol(), " ");
            }
        }
    }

    #[test]
    fn wrapped_preview_click_enters_header_at_field_ends_not_a_wrapped_source_row() {
        let (_dir, mut app) = test_app();
        app.add_note();
        app.notes[0].title = "Header".into();
        app.notes[0].content = "alpha beta gamma delta epsilon zeta eta theta".into();
        app.dirty = false;
        let preview = draw_editor(&mut app, 24, 24);
        let body_y = NoteLayout::new(
            layout::card_rect(24, 24, false).unwrap(),
            &app.notes[0],
            None,
        )
        .body
        .text
        .y;
        let delta = symbol_position(&preview, "d", Some(body_y + 1));
        let source = app.to_save_data();
        mouse(
            &mut app,
            crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            delta,
        );
        assert!(app.notes[0].editing);
        assert_eq!(app.edit_focus, EditFocus::Header);
        assert_eq!(app.notes[0].cursor, char_len(&app.notes[0].content));
        assert_eq!(app.notes[0].title_cursor, char_len(&app.notes[0].title));
        assert!(app.notes[0].content_selection_range().is_none());
        assert_eq!(app.to_save_data(), source);
        assert!(!app.dirty);
    }

    #[test]
    fn none_has_no_outer_border_in_reading_or_editing_and_hidden_remains_compatible() {
        let (_dir, mut app) = test_app();
        let mut note = Note::new();
        note.title = "Border test".into();
        note.content = "body".into();
        note.border_style = "none".into();
        app.notes.push(note);
        app.selected = Some(0);
        for editing in [false, true] {
            app.notes[0].editing = editing;
            for focus in [EditFocus::Header, EditFocus::Content, EditFocus::Tags] {
                app.edit_focus = focus;
                app.prepare_frame(80, 24);
                let card = layout::card_rect(80, 24, false).unwrap();
                let geometry =
                    layout::NoteLayout::new(card, &app.notes[0], editing.then_some(focus));
                let terminal = draw(&app, 80, 24);
                let buffer = terminal.backend().buffer();
                assert_eq!(buffer[(card.x, card.y)].symbol(), " ");
                assert_eq!(buffer[(card.right() - 1, card.bottom() - 1)].symbol(), " ");
                if editing && let Some(border) = geometry.body.border {
                    assert_ne!(buffer[(border.x, border.y)].symbol(), " ");
                }
                if !editing {
                    assert_eq!(geometry.inner, card);
                }
            }
        }
        app.notes[0].border_style = "hidden".into();
        app.edit_focus = EditFocus::Content;
        app.notes[0].editing = true;
        let card = layout::card_rect(80, 24, false).unwrap();
        assert_eq!(
            draw(&app, 80, 24).backend().buffer()[(card.x, card.y)].symbol(),
            "┌"
        );
        app.notes[0].editing = false;
        assert_eq!(
            draw(&app, 80, 24).backend().buffer()[(card.x, card.y)].symbol(),
            " "
        );
    }

    fn test_app() -> (tempfile::TempDir, App) {
        let dir = tempfile::tempdir().unwrap();
        let (board, data) = BoardStore::open(dir.path().join("board.json")).unwrap();
        let app = App::new(data, board);
        (dir, app)
    }

    fn draw(app: &App, width: u16, height: u16) -> Terminal<TestBackend> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| render(app, frame)).unwrap();
        terminal
    }

    fn draw_editor(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
        app.prepare_frame(width, height);
        draw(app, width, height)
    }

    fn mouse(
        app: &mut App,
        kind: crossterm::event::MouseEventKind,
        point: ratatui::layout::Position,
    ) {
        app.handle_mouse(crossterm::event::MouseEvent {
            kind,
            column: point.x,
            row: point.y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        });
    }

    fn editing_app(overlay: bool, focus: EditFocus) -> (tempfile::TempDir, App) {
        let (dir, mut app) = test_app();
        app.width = 80;
        app.height = 24;
        app.notes.push(Note {
            title: "标题e\u{301}🦀".into(),
            content: "ab\n**中e\u{301}🦀**\ncd".into(),
            tags: vec!["work".into()],
            editing: true,
            title_cursor: 5,
            cursor: 14,
            ..Note::new()
        });
        app.selected = Some(0);
        app.edit_focus = focus;
        app.show_overlay = overlay;
        (dir, app)
    }

    fn symbol_position(
        terminal: &Terminal<TestBackend>,
        symbol: &str,
        row: Option<u16>,
    ) -> ratatui::layout::Position {
        let buffer = terminal.backend().buffer();
        for y in 0..buffer.area.height {
            if row.is_some_and(|row| row != y) {
                continue;
            }
            for x in 0..buffer.area.width {
                if buffer[(x, y)].symbol() == symbol {
                    return ratatui::layout::Position::new(x, y);
                }
            }
        }
        panic!("symbol {symbol:?} not drawn on row {row:?}");
    }

    fn screen(terminal: &Terminal<TestBackend>) -> String {
        let buffer = terminal.backend().buffer();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn search_jump_list_draws_matching_results_and_follows_the_selected_result() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (_dir, mut app) = test_app();
        app.notes = (0..40)
            .map(|index| Note {
                title: format!("Result-{index:02}"),
                content: "Phrase in body".into(),
                tags: vec![if index % 2 == 0 { "work" } else { "other" }.into()],
                ..Note::new()
            })
            .collect();
        app.refresh_all_tags();
        app.filter_tag = "work".into();
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        for c in "PHRASE IN BODY".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        let text = screen_text(&draw_editor(&mut app, 80, 24));
        assert!(text.contains("Search / jump") && text.contains("20 results"));
        assert!(text.contains("Result-00") && !text.contains("Result-01"));
        for _ in 0..19 {
            app.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        }
        for (width, height) in [(80, 24), (30, 12), (12, 8), (10, 4)] {
            let terminal = draw_editor(&mut app, width, height);
            if width >= 30 {
                assert!(screen_text(&terminal).contains("Result-38"));
            }
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.selected, Some(38));
        assert_eq!(app.mode, InputMode::Normal);
        let text = screen_text(&draw_editor(&mut app, 80, 24));
        assert!(text.contains("Result-38") && !text.contains("Search / jump"));
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        app.handle_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE));
        assert_eq!(app.selected, None);
        assert!(screen_text(&draw_editor(&mut app, 80, 24)).contains("No matches."));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(app.search_query.is_empty() && app.filter_tag == "work");
        app.notes.clear();
        app.selected = None;
        app.filter_tag.clear();
        app.handle_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::NONE));
        assert!(screen_text(&draw_editor(&mut app, 80, 24)).contains("0 results"));
    }

    #[test]
    fn every_help_line_can_be_read_by_paging_at_eighty_by_twenty_four() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (_dir, mut app) = test_app();
        app.prepare_frame(80, 24);
        app.handle_key(KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE));
        let mut seen = String::new();
        for _ in 0..3 {
            let terminal = draw_editor(&mut app, 80, 24);
            let page = screen(&terminal);
            assert!(page.contains("Esc/?/q:close"));
            seen.push_str(&page);
            app.handle_key(KeyEvent::new(KeyCode::PageDown, KeyModifiers::NONE));
        }
        for line in HELP_LINES.iter().filter(|line| !line.trim().is_empty()) {
            assert!(
                seen.contains(line),
                "help line clipped or unreachable: {line}"
            );
        }
    }

    #[test]
    fn active_tab_window_and_overflow_counts_share_visible_hit_regions() {
        use crossterm::event::{MouseButton, MouseEventKind};
        let (_dir, mut app) = test_app();
        app.notes = (0..20)
            .map(|i| Note {
                title: format!("note-{i:02}-中é🦀"),
                tags: if i % 2 == 0 {
                    vec!["visible".into()]
                } else {
                    vec!["hidden".into()]
                },
                ..Note::new()
            })
            .collect();
        app.filter_tag = "visible".into();
        for width in [12, 20, 40, 80, 400] {
            app.prepare_frame(width, 24);
            for selected in (0..20).step_by(2) {
                app.selected = Some(selected);
                let strip = layout::tabs(&app, width);
                assert!(strip.tabs.iter().any(|tab| tab.index == selected));
                assert_eq!(strip.omitted, 10 - strip.tabs.len());
                for tab in &strip.tabs {
                    assert_eq!(tab.index % 2, 0);
                    assert!(tab.rect.right() < width);
                    for x in tab.rect.x..tab.rect.right() {
                        assert_eq!(note_index_at_tab_x(&app, x), Some(tab.index));
                    }
                }
                if let Some((text, rect)) = &strip.overflow {
                    assert_eq!(text, &format!("+{}", strip.omitted));
                    for x in rect.x..rect.right() {
                        assert_eq!(note_index_at_tab_x(&app, x), None);
                    }
                }
                draw(&app, width, 24);
            }
        }
        app.prepare_frame(40, 24);
        app.selected = Some(18);
        let strip = layout::tabs(&app, 40);
        let target = strip.tabs[0].index;
        let point = ratatui::layout::Position::new(strip.tabs[0].rect.x, 0);
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), point);
        assert_eq!(app.selected, Some(target));
    }

    #[test]
    fn overflowed_selected_chip_is_drawn_and_clicked_or_deleted_not_body_text() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
        for overlay in [false, true] {
            let (_dir, mut app) = editing_app(overlay, EditFocus::Tags);
            app.notes[0].tags = (0..20).map(|i| format!("tag-{i:02}")).collect();
            app.notes[0].tag_cursor = Some(19);
            let terminal = draw_editor(&mut app, 40, 16);
            assert!(screen(&terminal).contains("[tag-19]"));
            let cursor = app.notes[0].cursor;
            let layout = NoteLayout::new(
                layout::card_rect(40, 16, overlay).unwrap(),
                &app.notes[0],
                Some(EditFocus::Tags),
            );
            let chips = TagsLayout::new(&app.notes[0], layout.tags.unwrap().text, true);
            let chip = chips.chips.iter().find(|chip| chip.index == 19).unwrap();
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                ratatui::layout::Position::new(chip.rect.x, chip.rect.y),
            );
            assert_eq!(app.notes[0].tag_cursor, Some(19));
            app.handle_key(KeyEvent::new(KeyCode::Delete, KeyModifiers::NONE));
            assert!(!app.notes[0].tags.contains(&"tag-19".into()));
            assert_eq!(app.notes[0].cursor, cursor);
            assert_eq!(app.edit_focus, EditFocus::Tags);
            assert!(app.dirty);
        }
    }

    #[test]
    fn long_note_viewports_follow_carets_and_map_drawn_unicode_selection() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
        for overlay in [false, true] {
            let (_dir, mut app) = editing_app(overlay, EditFocus::Content);
            app.notes[0].content = (0..29)
                .map(|row| format!("row{row:02}"))
                .chain(std::iter::once(format!("{}中e\u{301}🦀!", "x".repeat(95))))
                .collect::<Vec<_>>()
                .join("\n");
            app.notes[0].cursor = crate::note::char_len(&app.notes[0].content);
            let terminal = draw_editor(&mut app, 80, 24);
            let rect = NoteLayout::new(
                layout::card_rect(80, 24, overlay).unwrap(),
                &app.notes[0],
                Some(EditFocus::Content),
            )
            .body
            .text;
            let caret = symbol_position(&terminal, "█", None);
            assert!(rect.contains(caret));
            assert_eq!(caret.y, rect.bottom() - 1);
            assert_eq!(app.notes[0].viewport.top_line, 30 - rect.height as usize);
            assert!(app.notes[0].viewport.left_cell > 0);
            let top = app.notes[0].viewport.top_line;
            let left = app.notes[0].viewport.left_cell;
            let mut point = symbol_position(&terminal, "中", Some(caret.y));
            point.x += 1; // second cell of the wide source character
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), point);
            assert_eq!(app.notes[0].cursor, app.notes[0].pos_to_cursor(29, 95));
            let terminal = draw_editor(&mut app, 80, 24);
            assert_eq!(app.notes[0].viewport.top_line, top);
            assert_eq!(app.notes[0].viewport.left_cell, left);
            let point = symbol_position(&terminal, "🦀", Some(caret.y));
            mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), point);
            assert_eq!(app.notes[0].selected_text(), "中e\u{301}");
            let terminal = draw_editor(&mut app, 80, 24);
            let point = symbol_position(&terminal, "中", Some(caret.y));
            assert_eq!(
                terminal.backend().buffer()[(point.x, point.y)].bg,
                Color::Rgb(0x55, 0x55, 0x55)
            );
            mouse(&mut app, MouseEventKind::Up(MouseButton::Left), point);
            app.handle_key(KeyEvent::new(KeyCode::Char('Z'), KeyModifiers::NONE));
            let terminal = draw_editor(&mut app, 80, 24);
            assert!(
                app.notes[0]
                    .content
                    .ends_with(&format!("{}Z🦀!", "x".repeat(95)))
            );
            assert!(rect.contains(symbol_position(&terminal, "█", None)));
        }
    }

    #[test]
    fn resize_newline_and_deletion_keep_the_body_cursor_visible() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};
        for overlay in [false, true] {
            let (_dir, mut app) = editing_app(overlay, EditFocus::Content);
            app.notes[0].content = (0..30)
                .map(|_| "中e\u{301}🦀".repeat(30))
                .collect::<Vec<_>>()
                .join("\n");
            app.notes[0].cursor = crate::note::char_len(&app.notes[0].content);
            for (width, height) in [(80, 24), (30, 14), (120, 42)] {
                let terminal = draw_editor(&mut app, width, height);
                let rect = NoteLayout::new(
                    layout::card_rect(width, height, overlay).unwrap(),
                    &app.notes[0],
                    Some(EditFocus::Content),
                )
                .body
                .text;
                assert!(rect.contains(symbol_position(&terminal, "█", None)));
            }
            app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
            let terminal = draw_editor(&mut app, 80, 24);
            assert_eq!(app.notes[0].cursor_pos(), (30, 0));
            assert_eq!(app.notes[0].viewport.left_cell, 0);
            let caret = symbol_position(&terminal, "█", None);
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), caret);
            assert_eq!(
                app.notes[0].cursor,
                crate::note::char_len(&app.notes[0].content)
            );
            app.handle_key(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
            let terminal = draw_editor(&mut app, 80, 24);
            assert_eq!(app.notes[0].cursor_pos().0, 29);
            assert!(app.notes[0].viewport.left_cell > 0);
            assert!(symbol_position(&terminal, "█", None).x < 80);
            app.notes[0].content.clear();
            app.notes[0].cursor = 0;
            app.notes[0].clear_all_selections();
            let terminal = draw_editor(&mut app, 80, 24);
            assert_eq!(app.notes[0].viewport.top_line, 0);
            assert_eq!(app.notes[0].viewport.left_cell, 0);
            assert!(symbol_position(&terminal, "█", None).x < 80);
        }
    }

    #[test]
    fn unavailable_editing_sections_show_resize_message_without_hidden_mouse_targets() {
        use crossterm::event::{MouseButton, MouseEventKind};
        for focus in [EditFocus::Content, EditFocus::Tags] {
            let (_dir, mut app) = editing_app(true, focus);
            let terminal = draw_editor(&mut app, 12, 8);
            assert!(screen_text(&terminal).contains("Resize"));
            let before = (app.notes[0].cursor, app.notes[0].title_cursor);
            mouse(
                &mut app,
                MouseEventKind::Down(MouseButton::Left),
                ratatui::layout::Position::new(3, 2),
            );
            assert_eq!(app.edit_focus, focus);
            assert_eq!((app.notes[0].cursor, app.notes[0].title_cursor), before);
            let terminal = draw_editor(&mut app, 80, 24);
            assert!(symbol_position(&terminal, "█", None).x < 80);
        }
    }

    #[test]
    fn long_tag_input_draws_a_visible_caret_even_with_many_chips() {
        let (_dir, mut app) = editing_app(false, EditFocus::Tags);
        app.notes[0].tags = vec!["verylongtag".into(); 20];
        app.notes[0].tag_input = "中e\u{301}🦀".repeat(30);
        let terminal = draw_editor(&mut app, 40, 24);
        let rect = NoteLayout::new(
            layout::card_rect(40, 24, false).unwrap(),
            &app.notes[0],
            Some(EditFocus::Tags),
        )
        .tags
        .unwrap()
        .text;
        assert!(rect.contains(symbol_position(&terminal, "█", Some(rect.y))));
        assert_eq!(app.notes[0].tag_input, "中e\u{301}🦀".repeat(30));
    }

    #[test]
    fn displayed_body_cells_map_to_literal_source_before_and_after_focus_changes() {
        use crossterm::event::{MouseButton, MouseEventKind};
        for overlay in [false, true] {
            for focus in [EditFocus::Header, EditFocus::Content] {
                for continuation in [0, 1] {
                    let (_dir, mut app) = editing_app(overlay, focus);
                    let terminal = draw(&app, 80, 24);
                    let mut point = symbol_position(&terminal, "中", None);
                    point.x += continuation;
                    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), point);
                    assert_eq!(app.edit_focus, EditFocus::Content);
                    assert_eq!(app.notes[0].cursor, 5, "overlay={overlay} focus={focus:?}");
                    // Clicking the same glyph after the caret appears cannot shift its source index.
                    let terminal = draw(&app, 80, 24);
                    let point = symbol_position(&terminal, "中", None);
                    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), point);
                    assert_eq!(app.notes[0].cursor, 5);
                    let point = symbol_position(&terminal, "e\u{301}", Some(point.y));
                    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), point);
                    assert_eq!(app.notes[0].cursor, 6);
                    let terminal = draw(&app, 80, 24);
                    let point = symbol_position(&terminal, "*", Some(point.y));
                    mouse(&mut app, MouseEventKind::Down(MouseButton::Left), point);
                    assert_eq!(app.notes[0].cursor, 3);
                    let terminal = draw(&app, 80, 24);
                    let point = symbol_position(&terminal, "中", Some(point.y));
                    mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), point);
                    assert_eq!(app.notes[0].selected_text(), "**");
                }
            }
        }
    }

    #[test]
    fn centered_and_cropped_titles_map_to_source_and_support_header_drags() {
        use crossterm::event::{MouseButton, MouseEventKind};
        for overlay in [false, true] {
            let (_dir, mut app) = editing_app(overlay, EditFocus::Content);
            let terminal = draw(&app, 80, 24);
            let geometry = NoteLayout::new(
                layout::card_rect(80, 24, overlay).unwrap(),
                &app.notes[0],
                Some(EditFocus::Content),
            );
            let mut point = symbol_position(&terminal, "标", Some(geometry.header.text.y));
            point.x += 1;
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), point);
            assert_eq!(app.edit_focus, EditFocus::Header);
            assert_eq!(app.notes[0].title_cursor, 0);
            let terminal = draw(&app, 80, 24);
            let point = symbol_position(&terminal, "🦀", Some(point.y));
            mouse(&mut app, MouseEventKind::Drag(MouseButton::Left), point);
            assert_eq!(app.notes[0].title_selection_range(), Some((0, 4)));
            assert_eq!(app.notes[0].selected_title(), "标题e\u{301}");
            assert!(!app.notes[0].has_content_selection());
            // Crossing into another row must not turn a header drag into a body edit.
            mouse(
                &mut app,
                MouseEventKind::Drag(MouseButton::Left),
                ratatui::layout::Position::new(point.x, 22),
            );
            assert_eq!(app.edit_focus, EditFocus::Header);
            assert_eq!(app.notes[0].cursor, 14);
            mouse(&mut app, MouseEventKind::Up(MouseButton::Left), point);
            assert!(!app.mouse_dragging);
            app.width = 40;
            app.notes[0].title = "abcdefghijklmnopqrstuvwx中yz🦀0123456789".into();
            app.notes[0].title_cursor = crate::note::char_len(&app.notes[0].title);
            let terminal = draw(&app, 40, 24);
            let point = symbol_position(&terminal, "中", Some(geometry.header.text.y));
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), point);
            assert_eq!(app.notes[0].title_cursor, 24);
        }
    }

    #[test]
    fn borders_separators_footer_and_tag_chips_do_not_fall_through_to_body_text() {
        use crossterm::event::{MouseButton, MouseEventKind};
        for overlay in [false, true] {
            let (_dir, mut app) = editing_app(overlay, EditFocus::Content);
            app.notes[0].tags = vec!["中🦀e\u{301}".into()];
            app.notes[0].cursor = 1;
            let rect = layout::card_rect(80, 24, overlay).unwrap();
            let geometry = NoteLayout::new(rect, &app.notes[0], Some(EditFocus::Content));
            let before = app.notes[0].cursor;
            for point in [
                ratatui::layout::Position::new(rect.x, rect.y),
                ratatui::layout::Position::new(geometry.body.text.x - 1, geometry.body.text.y),
                ratatui::layout::Position::new(
                    geometry.inner.x,
                    geometry.tags_separator.unwrap().y,
                ),
                ratatui::layout::Position::new(10, 22),
            ] {
                mouse(&mut app, MouseEventKind::Down(MouseButton::Left), point);
                assert_eq!(app.notes[0].cursor, before);
                assert_eq!(app.edit_focus, EditFocus::Content);
            }
            let terminal = draw(&app, 80, 24);
            let point = symbol_position(&terminal, "中", Some(geometry.tags.unwrap().text.y));
            mouse(&mut app, MouseEventKind::Down(MouseButton::Left), point);
            assert_eq!(app.edit_focus, EditFocus::Tags);
            assert_eq!(app.notes[0].tag_cursor, Some(0));
            assert_eq!(app.notes[0].cursor, before);
            app.handle_key(crossterm::event::KeyEvent::new(
                crossterm::event::KeyCode::Backspace,
                crossterm::event::KeyModifiers::NONE,
            ));
            assert!(app.notes[0].tags.is_empty());
            assert_eq!(app.notes[0].cursor, before);
            assert_eq!(app.edit_focus, EditFocus::Tags);
        }
    }

    #[test]
    fn body_drag_clamps_to_body_edges_and_autocomplete_clicks_never_edit_underneath() {
        use crossterm::event::{MouseButton, MouseEventKind};
        let (_dir, mut app) = editing_app(true, EditFocus::Content);
        let terminal = draw(&app, 80, 24);
        let point = symbol_position(&terminal, "a", None);
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), point);
        mouse(
            &mut app,
            MouseEventKind::Drag(MouseButton::Left),
            ratatui::layout::Position::new(79, 23),
        );
        assert_eq!(app.notes[0].content_selection_range(), Some((0, 14)));
        assert_eq!(app.edit_focus, EditFocus::Content);
        app.show_overlay = false;
        app.edit_focus = EditFocus::Tags;
        app.notes[0].tag_input = "wo".into();
        app.all_tags = vec!["work".into()];
        let terminal = draw(&app, 80, 24);
        let rect = layout::card_rect(80, 24, false).unwrap();
        let geometry = NoteLayout::new(rect, &app.notes[0], Some(EditFocus::Tags));
        let popup = geometry.popup(1).unwrap();
        assert_eq!(
            terminal.backend().buffer()[(popup.x + 3, popup.y + 1)].symbol(),
            "w"
        );
        let before = app.notes[0].cursor;
        mouse(
            &mut app,
            MouseEventKind::Down(MouseButton::Left),
            ratatui::layout::Position::new(popup.x + 3, popup.y + 1),
        );
        assert_eq!(app.edit_focus, EditFocus::Tags);
        assert_eq!(app.notes[0].cursor, before);
    }

    #[test]
    fn unicode_tab_hit_regions_match_drawn_cells_and_leave_dividers_unassigned() {
        use crossterm::event::{MouseButton, MouseEventKind};
        let (_dir, mut app) = test_app();
        app.width = 40;
        app.height = 24;
        for title in ["中🦀e\u{301}", "next"] {
            app.notes.push(Note {
                title: title.into(),
                ..Note::new()
            });
        }
        app.selected = Some(0);
        let terminal = draw(&app, 40, 24);
        let point = symbol_position(&terminal, "中", Some(0));
        assert_eq!(note_index_at_tab_x(&app, point.x), Some(0));
        assert_eq!(note_index_at_tab_x(&app, point.x + 1), Some(0));
        let point = symbol_position(&terminal, "n", Some(0));
        mouse(&mut app, MouseEventKind::Down(MouseButton::Left), point);
        assert_eq!(app.selected, Some(1));
        let tabs = layout::tabs(&app, 40).tabs;
        assert_eq!(note_index_at_tab_x(&app, tabs[0].rect.right()), None);
        assert!(tabs.last().unwrap().rect.right() <= 39);
    }

    #[test]
    fn unicode_title_cursor_uses_character_boundaries() {
        let (_dir, mut app) = test_app();
        app.notes.push(Note {
            title: "é🦀".into(),
            editing: true,
            title_cursor: 1,
            ..Note::new()
        });
        app.selected = Some(0);
        app.edit_focus = EditFocus::Header;
        draw(&app, 80, 24);
        app.notes[0].title_cursor = 2;
        draw(&app, 80, 24);
    }

    #[test]
    fn help_fits_a_standard_terminal() {
        let (_dir, mut app) = test_app();
        app.notes.push(Note::new());
        app.selected = Some(0);
        app.show_help = true;
        draw(&app, 80, 24);
    }

    #[test]
    fn narrow_tabs_never_allocate_more_than_the_remaining_width() {
        let (_dir, mut app) = test_app();
        app.notes = (0..12)
            .map(|i| Note {
                title: format!("note-{i:02}"),
                ..Note::new()
            })
            .collect();
        app.selected = Some(0);
        draw(&app, 40, 24);
    }

    #[test]
    fn small_note_layout_and_overlays_remain_inside_the_frame() {
        let (_dir, mut app) = test_app();
        draw(&app, 20, 8); // bounded welcome box
        app.notes.push(Note {
            title: "note".into(),
            tags: vec!["work".into()],
            editing: true,
            ..Note::new()
        });
        app.selected = Some(0);
        draw(&app, 80, 5); // recorded subtraction failure now uses compact view
        for focus in [EditFocus::Header, EditFocus::Content, EditFocus::Tags] {
            app.edit_focus = focus;
            draw(&app, 12, 8);
        }
        app.show_overlay = true;
        draw(&app, 12, 8);
        app.show_overlay = false;
        app.menu.visible = true;
        app.menu.x = 79;
        app.menu.y = 23;
        app.menu.items = vec![MenuAction::Edit, MenuAction::Delete, MenuAction::Close];
        draw(&app, 80, 24);
    }

    #[test]
    fn failed_save_dialog_is_visible_even_over_the_edit_overlay() {
        let (_dir, mut app) = test_app();
        app.notes.push(Note::new());
        app.selected = Some(0);
        app.show_overlay = true;
        app.quit_state = QuitState::SaveFailed;
        app.save_error = "injected save error".into();
        let terminal = draw(&app, 80, 24);
        let buffer = terminal.backend().buffer();
        let text: String = buffer.content.iter().map(|cell| cell.symbol()).collect();
        assert!(text.contains("Enter/r: retry"));
        assert!(text.contains("injected save error"));
        app.quit_state = QuitState::ConfirmDiscard;
        draw(&app, 12, 8);
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn highlighted_text(line: &Line<'_>) -> String {
        line.spans
            .iter()
            .filter(|span| span.style.bg == Some(Color::Rgb(0x55, 0x55, 0x55)))
            .map(|span| span.content.as_ref())
            .collect()
    }

    fn screen_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content
            .iter()
            .map(|cell| cell.symbol())
            .collect()
    }

    #[test]
    fn cropped_and_backward_title_highlights_use_source_character_indices() {
        let mut note = Note {
            title: "0123456789éABC".into(),
            editing: true,
            title_cursor: 12,
            title_sel_start: Some(8),
            title_sel_end: Some(12),
            ..Note::new()
        };
        let line =
            layout::header_line(&note, true, 8).render(note.title_selection_range(), Style::new());
        assert_eq!(line_text(&line), "6789éA█…");
        assert_eq!(highlighted_text(&line), note.selected_title());
        assert_eq!(highlighted_text(&line), "89éA");
        note.title_sel_start = Some(12);
        note.title_sel_end = Some(8);
        note.title_cursor = 8;
        let line =
            layout::header_line(&note, true, 20).render(note.title_selection_range(), Style::new());
        assert!(line_text(&line).contains("67█89éA"));
        assert_eq!(highlighted_text(&line), note.selected_title());
        assert!(!highlighted_text(&line).contains('█'));
    }

    #[test]
    fn content_selection_keeps_the_cursor_and_unicode_highlights_across_lines() {
        for (anchor, cursor) in [(1, 4), (4, 1)] {
            let note = Note {
                content: "aé\n🙂b".into(),
                editing: true,
                cursor,
                sel_start: Some(anchor),
                sel_end: Some(cursor),
                ..Note::new()
            };
            let lines = render_edit_content(&note, Rect::new(0, 0, 20, 10), true, Style::new());
            let text: String = lines.iter().map(line_text).collect();
            let highlighted: String = lines.iter().map(highlighted_text).collect();
            assert_eq!(text.matches('█').count(), 1);
            assert_eq!(highlighted, note.selected_text().replace('\n', ""));
            if cursor == 1 {
                assert_eq!(line_text(&lines[0]), "a█é");
            } else {
                assert_eq!(line_text(&lines[1]), "🙂█b");
            }
        }
    }

    #[test]
    fn editing_keeps_markdown_source_and_preview_still_formats_it() {
        let (_dir, mut app) = test_app();
        app.add_note();
        app.notes[0].title = "Title".into();
        app.notes[0].content = "**bold**".into();
        app.toggle_edit();
        app.edit_focus = EditFocus::Content;
        app.notes[0].cursor = 6;
        app.notes[0].sel_start = Some(2);
        app.notes[0].sel_end = Some(6);
        let text = screen_text(&draw(&app, 80, 24));
        assert!(text.contains("**bold█**"));
        let lines = render_edit_content(&app.notes[0], Rect::new(0, 0, 40, 10), true, Style::new());
        assert_eq!(highlighted_text(&lines[0]), "bold");
        app.edit_focus = EditFocus::Header;
        let text = screen_text(&draw(&app, 80, 24));
        assert!(text.contains("Title█"));
        assert!(!text.contains("bold█"));
        app.toggle_edit();
        let text = screen_text(&draw(&app, 80, 24));
        assert!(text.contains("bold"));
        assert!(!text.contains("**bold**") && !text.contains('█'));
    }

    #[test]
    fn overlay_renders_actual_focus_and_tag_suggestions() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (_dir, mut app) = test_app();
        app.add_note();
        app.notes[0].title = "Title".into();
        app.notes[0].content = "Body".into();
        app.notes.push(Note {
            tags: vec!["weekly".into()],
            ..Note::new()
        });
        app.refresh_all_tags();
        app.toggle_overlay();
        let text = screen_text(&draw(&app, 80, 24));
        assert!(text.contains("Body█") && !text.contains("Title█"));
        assert!(text.contains("Enter:newline"));
        for code in [KeyCode::Tab, KeyCode::Char('w'), KeyCode::Char('e')] {
            app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
        let text = screen_text(&draw(&app, 80, 24));
        assert!(text.contains("we█") && text.contains("weekly"));
        assert!(text.contains("Enter:add"));
        for code in [KeyCode::Enter, KeyCode::Tab] {
            app.handle_key(KeyEvent::new(code, KeyModifiers::NONE));
        }
        let text = screen_text(&draw(&app, 80, 24));
        assert!(text.contains("Title█") && !text.contains("Body█"));
        assert!(text.contains("Enter:content"));
        app.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert!(!app.show_overlay && !app.notes[0].editing);
    }

    #[test]
    fn zero_match_filter_renders_an_empty_result_without_hidden_notes() {
        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let (_dir, mut app) = test_app();
        app.add_note();
        app.notes[0].title = "HiddenTitle".into();
        app.notes[0].content = "HiddenContent".into();
        app.handle_key(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
        for c in "missing".chars() {
            app.handle_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.selected, None);
        let text = screen_text(&draw(&app, 80, 24));
        assert!(text.contains("No notes match.") && text.contains("Esc: clear filters"));
        assert!(
            !text.contains("HiddenTitle")
                && !text.contains("HiddenContent")
                && !text.contains("+1")
        );
    }
}
