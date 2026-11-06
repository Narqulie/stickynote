use super::*;

fn text(line: &TextLine) -> String {
    line.render(None, Style::new())
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

#[test]
fn content_projection_preserves_cursor_and_line_split_contracts() {
    for (source, cursor, expected) in [
        ("hello", None, "hello"),
        ("hello", Some(3), "hel█lo"),
        ("hi", Some(2), "hi█"),
        ("ab", Some(100), "ab█"),
    ] {
        assert_eq!(text(&TextLine::new(source, cursor, 20)), expected);
    }
    let note = Note {
        content: "hello\nworld\n".into(),
        editing: true,
        cursor: 12,
        ..Note::new()
    };
    let lines: Vec<_> = body_lines(&note, 20, true).collect();
    assert_eq!(
        lines.iter().map(|(line, _)| text(line)).collect::<Vec<_>>(),
        ["hello", "world", "█"]
    );
    assert_eq!(
        lines.iter().map(|(_, start)| *start).collect::<Vec<_>>(),
        [0, 6, 12]
    );
}

#[test]
fn wide_and_combining_cells_map_to_source_characters_including_the_caret() {
    let line = TextLine::new("中e\u{301}🦀!", None, 20);
    assert_eq!(line.width(), 6);
    assert_eq!(line.render(None, Style::new()).width(), 6);
    assert_eq!(
        (0..=6)
            .map(|cell| line.index_at_cell(cell))
            .collect::<Vec<_>>(),
        [0, 0, 1, 3, 3, 4, 5]
    );
    let line = TextLine::new("中e\u{301}🦀!", Some(3), 20);
    assert_eq!(text(&line), "中e\u{301}█🦀!");
    assert_eq!(
        (0..=7)
            .map(|cell| line.index_at_cell(cell))
            .collect::<Vec<_>>(),
        [0, 0, 1, 3, 3, 3, 4, 5]
    );
    let inside = TextLine::new("e\u{301}x", Some(1), 20);
    assert_eq!(text(&inside), "e█\u{301}x");
    assert_eq!(inside.index_at_cell(1), 1);
}

#[test]
fn clipping_and_title_centering_use_cells_without_splitting_graphemes() {
    let line = TextLine::new("中🦀a", None, 3);
    assert_eq!(text(&line), "中…");
    assert_eq!(line.index_at_cell(2), 1);
    let line = TextLine::title("中e\u{301}🦀", Some(4), 12);
    assert_eq!(text(&line), "   中e\u{301}🦀█");
    assert_eq!(line.index_at_cell(3), 0);
    assert_eq!(line.index_at_cell(5), 1);
    assert_eq!(line.index_at_cell(6), 3);
    assert_eq!(line.index_at_cell(8), 4);
    // A title that fits including its caret must not drop the first glyph.
    assert_eq!(text(&TextLine::title("中a", Some(2), 4)), "中a█");
    let line = TextLine::title("0123456789中e\u{301}🦀xyz", Some(14), 8);
    assert!(line.width() <= 8);
    assert!(text(&line).contains('█'));
}

#[test]
fn windowed_unicode_lines_preserve_source_offsets_and_partial_cell_padding() {
    let line = TextLine::window("a中e\u{301}🦀!", Some(5), 5, 2);
    assert_eq!(text(&line), " e\u{301}🦀█");
    assert_eq!(line.width(), 5);
    assert_eq!(
        (0..=5)
            .map(|cell| line.index_at_cell(cell))
            .collect::<Vec<_>>(),
        [2, 2, 4, 4, 5, 5]
    );
    let line = TextLine::window("a中e\u{301}🦀!", None, 3, 2);
    assert_eq!(text(&line), " e\u{301}…");
    assert_eq!(line.index_at_cell(2), 4);
}

#[test]
fn viewport_follows_only_outside_its_window_and_rechecks_resize_and_short_lines() {
    let mut note = Note {
        content: (0..30)
            .map(|_| "x".repeat(100))
            .collect::<Vec<_>>()
            .join("\n"),
        editing: true,
        ..Note::new()
    };
    note.cursor = char_len(&note.content);
    follow_body(&mut note, Rect::new(0, 0, 12, 5), true);
    assert_eq!(note.viewport.top_line, 25);
    assert_eq!(note.viewport.left_cell, 89);
    assert!(text(&body_line(&note, 4, 12, true).0).contains('█'));
    note.cursor = note.pos_to_cursor(28, 99);
    follow_body(&mut note, Rect::new(0, 0, 12, 5), true);
    assert_eq!(note.viewport.top_line, 25);
    assert_eq!(note.viewport.left_cell, 89);
    note.cursor = note.pos_to_cursor(24, 99);
    follow_body(&mut note, Rect::new(0, 0, 12, 5), true);
    assert_eq!(note.viewport.top_line, 24);
    follow_body(&mut note, Rect::new(0, 0, 6, 2), true);
    assert_eq!(note.viewport.top_line, 24);
    assert!(text(&body_line(&note, 0, 6, true).0).contains('█'));
    follow_body(&mut note, Rect::new(0, 0, 110, 40), true);
    assert_eq!(note.viewport.top_line, 0);
    assert_eq!(note.viewport.left_cell, 0);
    note.viewport.left_cell = 80;
    note.content.push_str("\nshort");
    note.cursor = char_len(&note.content);
    follow_body(&mut note, Rect::new(0, 0, 12, 5), true);
    assert_eq!(note.viewport.left_cell, 0);
    assert_eq!(text(&body_line(&note, 4, 12, true).0), "short█");
}

#[test]
fn tag_input_keeps_its_end_caret_visible_when_chips_and_text_overflow() {
    let mut note = Note::new();
    note.tags = vec!["longtag".into(), "another".into()];
    note.tag_input = "中e\u{301}🦀".repeat(12);
    let rect = Rect::new(3, 20, 8, 1);
    let tags = TagsLayout::new(&note, rect, true);
    assert!(tags.chips.is_empty());
    let (line, input) = tags.input.as_ref().unwrap();
    assert_eq!(*input, rect);
    assert!(text(line).ends_with('█'));
    assert_eq!(line.index_at_cell(8), char_len(&note.tag_input));
    note.tag_input.clear();
    let tags = TagsLayout::new(&note, rect, true);
    assert!(!tags.chips.is_empty());
    assert_eq!(text(&tags.input.as_ref().unwrap().0), "█");
    assert!(
        tags.chips
            .iter()
            .all(|chip| chip.rect.right() <= tags.input.as_ref().unwrap().1.x)
    );
}

#[test]
fn selection_styles_a_whole_display_grapheme_without_changing_source_text() {
    let line = TextLine::new("中e\u{301}🦀", Some(4), 20);
    let rendered = line.render(Some((2, 4)), Style::new());
    let selected: String = rendered
        .spans
        .iter()
        .filter(|span| span.style.bg == Some(Color::Rgb(0x55, 0x55, 0x55)))
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(selected, "e\u{301}🦀");
    assert_eq!(text(&line), "中e\u{301}🦀█");
}

#[test]
fn selected_overflowing_tag_chips_stay_visible_with_bounded_shared_rectangles() {
    let mut note = Note::new();
    note.tags = vec![
        "first".into(),
        "中é".into(),
        "very-long-selected-tag".into(),
        "last".into(),
    ];
    for input in ["", "long input takes the whole row"] {
        note.tag_input = input.into();
        for selected in 0..note.tags.len() {
            note.tag_cursor = Some(selected);
            for width in 1..40 {
                let rect = Rect::new(3, 5, width, 1);
                let tags = TagsLayout::new(&note, rect, true);
                let chip = tags
                    .chips
                    .iter()
                    .find(|chip| chip.index == selected)
                    .unwrap();
                assert!(chip.rect.width > 0);
                for chip in tags.chips {
                    assert!(chip.rect.x >= rect.x && chip.rect.right() <= rect.right());
                }
                if let Some((_, input)) = tags.input {
                    assert!(input.right() <= rect.right());
                }
            }
        }
    }
}
