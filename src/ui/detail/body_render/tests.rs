use super::*;
use crate::ui::detail::body_cache::Capture;
use crate::ui::detail::body_cache::fixtures::{media, partial_capture, source};
use crate::ui::style::theme::Theme;
use eframe::egui::Galley;
use http_inspector_analysis::search::Query;
use http_inspector_content::format::tree;

fn run(cache: &mut BodyViewCache, ctx: &egui::Context, source: &BodySource<'_>, mode: ViewMode) {
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        let _ = cache.show(ui, source, mode, Language::English);
    });
    output.textures_delta.clear();
}

#[test]
fn css_preview_caches_gutter_and_warns_when_truncated() {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();
    let css = BodySource {
        media_type: media("text/css"),
        capture: Capture::Truncated,
        ..source(1, b"a{color:red}")
    };
    run(&mut cache, &ctx, &css, ViewMode::Pretty);
    run(&mut cache, &ctx, &css, ViewMode::Pretty);
    assert_eq!(total_builds(&cache), 1);
    if let Some(Content::Ready {
        job,
        lines,
        notices,
        ..
    }) = &cache.response.shown
    {
        assert_eq!(job.text, "a{\n  color:red\n}");
        let texts: Vec<_> = lines
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(texts, ["a{", "  color:red", "}"]);
        assert_eq!(notices, &[partial_capture()]);
    } else {
        assert_eq!(total_builds(&cache), 0, "expected prepared CSS content");
    }
    run(&mut cache, &ctx, &css, ViewMode::Raw);
    if let Some(Content::Ready { job, lines, .. }) = &cache.response.shown {
        assert_eq!(job.text.as_bytes(), css.bytes);
        let texts: Vec<_> = lines
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|line| line.text.as_str())
            .collect();
        assert_eq!(texts, ["a{color:red}"]);
    }
}

#[test]
fn rebuilds_only_when_the_body_or_view_changes() {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();
    let first = 1;
    let body = br#"{"a":[1,2]}"#;
    run(&mut cache, &ctx, &source(first, body), ViewMode::Pretty);
    run(&mut cache, &ctx, &source(first, body), ViewMode::Pretty);
    assert_eq!(total_builds(&cache), 1);
    run(&mut cache, &ctx, &source(first, body), ViewMode::Raw);
    assert_eq!(total_builds(&cache), 2);
    let request = BodySource {
        side: BodySide::Request,
        ..source(first, body)
    };
    run(&mut cache, &ctx, &request, ViewMode::Raw);
    run(&mut cache, &ctx, &source(first, body), ViewMode::Raw);
    assert_eq!(total_builds(&cache), 3, "each side keeps its own entry");
    run(&mut cache, &ctx, &source(2, body), ViewMode::Raw);
    assert_eq!(total_builds(&cache), 4);
    // The theme buttons switch the theme in the middle of a frame.
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        ui.ctx().set_theme(Theme::Light);
        let _ = cache.show(ui, &source(2, body), ViewMode::Raw, Language::English);
    });
    output.textures_delta.clear();
    assert_eq!(total_builds(&cache), 5);
    let text_color = match &cache.response.shown {
        Some(Content::Ready { job, .. }) => job.sections.first().map(|s| s.format.color),
        _ => None,
    };
    assert_eq!(
        text_color,
        Some(ctx.style_of(Theme::Light).visuals.text_color()),
        "colors follow the new theme in the frame it is switched"
    );
}

fn total_builds(cache: &BodyViewCache) -> u64 {
    cache.request.cache.builds + cache.response.cache.builds
}

#[test]
fn trees_keep_parse_and_open_rows_across_modes_and_themes() -> Result<(), String> {
    for (content_type, body) in [
        ("application/json", &br#"{"b":{"c":1},"a":2,"b":1e2}"#[..]),
        ("application/xml", b"<a><b><c>1</c></b><d/></a>"),
    ] {
        let ctx = egui::Context::default();
        ctx.set_theme(Theme::Dark);
        let mut cache = BodyViewCache::default();
        let body = BodySource {
            media_type: media(content_type),
            ..source(1, body)
        };
        run(&mut cache, &ctx, &body, ViewMode::Tree);
        assert!(matches!(&cache.response.shown, Some(Content::Tree { .. })));
        let open = |cache: &mut BodyViewCache| {
            cache
                .response
                .tree
                .as_ref()
                .map_or_else(Vec::new, TreeView::open_rows)
        };
        let initial = open(&mut cache);
        cache
            .response
            .tree
            .as_mut()
            .ok_or("missing tree")?
            .collapse_all();
        run(&mut cache, &ctx, &body, ViewMode::Raw);
        ctx.set_theme(Theme::Light);
        run(&mut cache, &ctx, &body, ViewMode::Tree);
        assert_eq!(
            cache.response.cache.tree_builds, 1,
            "{content_type} parsed once"
        );
        assert_eq!(
            total_builds(&cache),
            3,
            "the text view still follows the theme"
        );
        assert!(open(&mut cache).iter().all(|open| !open));
        assert_ne!(
            open(&mut cache),
            initial,
            "{content_type} kept its rows closed"
        );
        let longer = BodySource {
            capture: Capture::Truncated,
            ..body
        };
        run(&mut cache, &ctx, &longer, ViewMode::Tree);
        run(&mut cache, &ctx, &source(2, body.bytes), ViewMode::Tree);
        assert_eq!(
            cache.response.cache.tree_builds, 3,
            "other bytes build a new tree"
        );
    }
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();
    run(&mut cache, &ctx, &source(3, b"[1,"), ViewMode::Tree);
    assert!(matches!(
        &cache.response.shown,
        Some(Content::Ready { notices, .. })
            if notices.iter().any(|notice| notice.text.starts_with("Showing Pretty"))
    ));
    Ok(())
}

#[test]
fn numbers_script_lines_and_warns_about_partial_capture() {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();
    let script = BodySource {
        media_type: media("application/javascript"),
        capture: Capture::Truncated,
        ..source(1, b"f(function(){a();b()}); x()")
    };
    run(&mut cache, &ctx, &script, ViewMode::Pretty);
    let (lines, notices) = match &cache.response.shown {
        Some(Content::Ready { lines, notices, .. }) => (lines.as_deref(), notices.clone()),
        _ => (None, Vec::new()),
    };
    let texts: Vec<_> = lines
        .unwrap_or_default()
        .iter()
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(texts, ["f(function(){", "  a();", "  b()", "});", "x()"]);
    assert_eq!(notices, [partial_capture()]);
    run(&mut cache, &ctx, &script, ViewMode::Raw);
    assert!(matches!(
        &cache.response.shown,
        Some(Content::Ready { lines: Some(lines), .. }) if lines.len() == 1
    ));
    run(
        &mut cache,
        &ctx,
        &source(2, br#"{"a":1}"#),
        ViewMode::Pretty,
    );
    assert!(matches!(
        &cache.response.shown,
        Some(Content::Ready { lines: None, .. })
    ));
}

fn glyph_x(galley: &Galley, text: &str, needle: &str) -> Result<f32, String> {
    let byte_index = text
        .find(needle)
        .ok_or_else(|| format!("needle {needle:?} not found in {text:?}"))?;
    let char_index = text
        .get(..byte_index)
        .ok_or_else(|| format!("invalid byte offset {byte_index}"))?
        .chars()
        .count();
    Ok(galley
        .pos_from_cursor(egui::text::CCursor::new(char_index))
        .min
        .x)
}

fn cached_text_galley(cache: &BodyViewCache) -> Result<(&LayoutJob, Arc<Galley>), String> {
    match &cache.response.shown {
        Some(Content::Ready {
            job,
            galley: Some((_, galley)),
            ..
        }) => Ok((job, Arc::clone(galley))),
        _ => Err("expected laid-out text body".to_owned()),
    }
}

fn assert_three_lines_aligned(
    galley: &Galley,
    text: &str,
    lines: [&str; 3],
    next_field: &str,
) -> Result<(), String> {
    let first_x = glyph_x(galley, text, lines[0])?;
    let second_x = glyph_x(galley, text, lines[1])?;
    let third_x = glyph_x(galley, text, lines[2])?;
    let next_x = glyph_x(galley, text, next_field)?;
    assert!((first_x - second_x).abs() < 0.5, "{first_x} vs {second_x}");
    assert!((first_x - third_x).abs() < 0.5, "{first_x} vs {third_x}");
    assert!(first_x > next_x, "value {first_x} <= next field {next_x}");
    Ok(())
}

#[test]
fn aligns_nested_xml_html_and_json_multiline_values() -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();

    let xml_bytes =
        b"<root><item title=\"attr1\nattr2\"><note>line1\nline2\nline3</note><next>ok</next></item></root>";
    let xml = BodySource {
        media_type: media("application/xml"),
        ..source(10, xml_bytes)
    };
    run(&mut cache, &ctx, &xml, ViewMode::Pretty);
    let (job, galley) = cached_text_galley(&cache)?;
    assert!(job.text.contains("line1\nline2\nline3"), "{}", job.text);
    assert!(job.text.contains("\"attr1\nattr2\""), "{}", job.text);
    assert_three_lines_aligned(&galley, &job.text, ["line1", "line2", "line3"], "<next>")?;
    let attr_first_x = glyph_x(&galley, &job.text, "attr1")?;
    let attr_second_x = glyph_x(&galley, &job.text, "attr2")?;
    assert!((attr_first_x - attr_second_x).abs() < 0.5);

    let html_bytes =
        b"<section><textarea name=\"bio\">alpha\nbeta\ngamma</textarea><p>after</p></section>";
    let html = BodySource {
        media_type: media("text/html"),
        ..source(11, html_bytes)
    };
    run(&mut cache, &ctx, &html, ViewMode::Pretty);
    let (job, galley) = cached_text_galley(&cache)?;
    assert!(job.text.contains("alpha\nbeta\ngamma"), "{}", job.text);
    assert_three_lines_aligned(&galley, &job.text, ["alpha", "beta", "gamma"], "<p>")?;

    let json_bytes = b"{\"outer\":{\"note\":\"one\ntwo\nthree\",\"next\":1}}";
    let json = BodySource {
        media_type: media("application/json"),
        ..source(12, json_bytes)
    };
    run(&mut cache, &ctx, &json, ViewMode::Pretty);
    let (job, galley) = cached_text_galley(&cache)?;
    assert!(job.text.contains("\"one\ntwo\nthree\""), "{}", job.text);
    assert_three_lines_aligned(&galley, &job.text, ["one", "two", "three"], "\"next\"")?;
    Ok(())
}

#[test]
fn preserves_raw_and_tree_copy_values() -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();

    let xml_bytes = b"<root><note>line1\nline2\nline3</note></root>";
    let xml = BodySource {
        media_type: media("application/xml"),
        ..source(13, xml_bytes)
    };
    run(&mut cache, &ctx, &xml, ViewMode::Raw);
    let (raw_job, _) = cached_text_galley(&cache)?;
    assert_eq!(raw_job.text.as_bytes(), xml_bytes);
    assert!(
        raw_job
            .sections
            .iter()
            .all(|section| section.leading_space == 0.0)
    );

    let xml_tree = tree::build(
        BodyFormat::Xml,
        std::str::from_utf8(xml_bytes).map_err(|e| e.to_string())?,
    )
    .ok_or("missing XML tree")?
    .map_err(|e| e.to_string())?;
    assert!(
        xml_tree
            .nodes()
            .iter()
            .any(|node| node.value == "<note>line1\nline2\nline3</note>")
    );

    let valid_json = r#"{"note":"line1\n  line2\nline3"}"#;
    let json_tree = tree::build(BodyFormat::Json, valid_json)
        .ok_or("missing JSON tree")?
        .map_err(|e| e.to_string())?;
    assert!(
        json_tree
            .nodes()
            .iter()
            .any(|node| node.value == r#""line1\n  line2\nline3""#)
    );
    Ok(())
}

#[test]
fn aligns_js_line_numbered_multiline_values_and_keeps_raw_unpadded() -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();

    let js_bytes = b"const cfg={a:{msg:`row1\nrow2\nrow3`,next:1}};";
    let js = BodySource {
        media_type: media("application/javascript"),
        ..source(14, js_bytes)
    };
    run(&mut cache, &ctx, &js, ViewMode::Pretty);
    let js_lines = match &cache.response.shown {
        Some(Content::Ready {
            lines: Some(lines), ..
        }) => lines.clone(),
        _ => return Err("expected line-numbered JS body".to_owned()),
    };
    let row1_idx = js_lines
        .iter()
        .position(|line| line.text.contains("row1"))
        .ok_or("missing row1")?;
    let first_line = js_lines.get(row1_idx).ok_or("missing row1 line")?;
    let second_line = js_lines.get(row1_idx + 1).ok_or("missing row2 line")?;
    let third_line = js_lines.get(row1_idx + 2).ok_or("missing row3 line")?;
    assert_eq!(second_line.text, "row2");
    assert_eq!(third_line.text, "row3`,");
    let mut check: Result<(), String> = Ok(());
    let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
        check = (|| {
            let g1 = ui.fonts_mut(|f| f.layout_job(first_line.clone()));
            let g2 = ui.fonts_mut(|f| f.layout_job(second_line.clone()));
            let g3 = ui.fonts_mut(|f| f.layout_job(third_line.clone()));
            let first_x = glyph_x(&g1, &first_line.text, "row1")?;
            let second_x = glyph_x(&g2, &second_line.text, "row2")?;
            let third_x = glyph_x(&g3, &third_line.text, "row3")?;
            assert!((first_x - second_x).abs() < 0.5, "{first_x} vs {second_x}");
            assert!((first_x - third_x).abs() < 0.5, "{first_x} vs {third_x}");
            Ok(())
        })();
    });
    output.textures_delta.clear();
    check?;

    run(&mut cache, &ctx, &js, ViewMode::Raw);
    let Some(Content::Ready { job, lines, .. }) = &cache.response.shown else {
        return Err("expected raw JS body".to_owned());
    };
    assert_eq!(job.text.as_bytes(), js_bytes);
    assert!(
        lines
            .as_deref()
            .unwrap_or_default()
            .iter()
            .flat_map(|line| &line.sections)
            .all(|section| section.leading_space == 0.0)
    );
    Ok(())
}

#[test]
fn preserves_multiline_alignment_with_active_search_highlights() -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();
    let query = Query {
        generation: 1,
        matcher: http_inspector_analysis::search::Matcher::new(
            "line",
            http_inspector_analysis::search::MatchOptions::default(),
        )
        .map_err(|e| e.to_string())?,
        scope: http_inspector_analysis::search::SearchScope::All,
    };
    let xml = BodySource {
        media_type: media("application/xml"),
        search: Some(&query),
        ..source(
            20,
            b"<root><note>line1\nline2\nline3</note><other>line4</other></root>",
        )
    };
    run(&mut cache, &ctx, &xml, ViewMode::Pretty);
    let (job, galley) = cached_text_galley(&cache)?;
    assert!(job.text.contains("line1\nline2\nline3"));
    let x1 = glyph_x(&galley, &job.text, "line1")?;
    let x2 = glyph_x(&galley, &job.text, "line2")?;
    let x3 = glyph_x(&galley, &job.text, "line3")?;
    assert!((x1 - x2).abs() < 0.5, "x1={x1}, x2={x2}");
    assert!((x1 - x3).abs() < 0.5, "x1={x1}, x3={x3}");
    Ok(())
}

#[test]
fn clamps_multiline_continuation_indent_in_narrow_viewport() -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();
    let xml = BodySource {
        media_type: media("application/xml"),
        ..source(
            21,
            b"<root><very_long_nested_field_name_for_narrow_window>first\nsecond\nthird</very_long_nested_field_name_for_narrow_window></root>",
        )
    };
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(640.0, 420.0),
        )),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        ui.set_max_width(240.0);
        let _ = cache.show(ui, &xml, ViewMode::Pretty, Language::English);
    });
    output.textures_delta.clear();

    let (job, galley) = cached_text_galley(&cache)?;
    let x_second = glyph_x(&galley, &job.text, "second")?;
    let x_third = glyph_x(&galley, &job.text, "third")?;
    assert!(
        x_second > 0.0,
        "continuation should remain visibly indented"
    );
    assert!(
        x_second <= 120.5,
        "indent {x_second} should be clamped to at most half of narrow width"
    );
    assert!(
        (x_second - x_third).abs() < 0.5,
        "clamped continuations must still align: x_second={x_second}, x_third={x_third}"
    );
    Ok(())
}

#[test]
fn trailing_newline_does_not_enable_break_anywhere_on_wide_viewport() -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();
    let truncated_xml = BodySource {
        media_type: media("application/xml"),
        capture: Capture::Truncated,
        ..source(
            22,
            b"<root><note>first\nsecond\nalpha beta gamma delta epsilon zeta\n",
        )
    };
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1000.0, 600.0),
        )),
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, |ui| {
        ui.set_max_width(300.0);
        let _ = cache.show(ui, &truncated_xml, ViewMode::Pretty, Language::English);
    });
    output.textures_delta.clear();

    let (job, galley) = cached_text_galley(&cache)?;
    assert!(
        !galley.job.wrap.break_anywhere,
        "normal trailing newline row must not trigger break_anywhere"
    );
    let first_x = glyph_x(&galley, &job.text, "first")?;
    let second_x = glyph_x(&galley, &job.text, "second")?;
    assert!((first_x - second_x).abs() < 0.5, "{first_x} vs {second_x}");
    let row_texts: Vec<String> = galley.rows.iter().map(|row| row.text()).collect();
    assert!(
        row_texts.iter().any(|row| row.contains("epsilon")),
        "words should stay intact on word-boundary wraps: {row_texts:?}"
    );
    Ok(())
}

#[test]
fn aligns_closing_tag_when_source_tail_indent_is_shorter_than_depth() -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();

    let xml = BodySource {
        media_type: media("application/xml"),
        ..source(
            23,
            b"<root><outer><note>first\nsecond\nthird\n  </note><next>ok</next></outer></root>",
        )
    };
    run(&mut cache, &ctx, &xml, ViewMode::Pretty);
    let (job, galley) = cached_text_galley(&cache)?;
    assert!(job.text.contains("\n  </note>"), "{}", job.text);
    let open_x = glyph_x(&galley, &job.text, "<note>")?;
    let close_x = glyph_x(&galley, &job.text, "</note>")?;
    let next_x = glyph_x(&galley, &job.text, "<next>")?;
    assert!((open_x - close_x).abs() < 0.5, "{open_x} vs {close_x}");
    assert!((open_x - next_x).abs() < 0.5, "{open_x} vs {next_x}");

    let html = BodySource {
        media_type: media("text/html"),
        ..source(
            24,
            b"<div><section><textarea>\nline1\nline2\n  </textarea><p>next</p></section></div>",
        )
    };
    run(&mut cache, &ctx, &html, ViewMode::Pretty);
    let (job, galley) = cached_text_galley(&cache)?;
    assert!(job.text.contains("\n  </textarea>"), "{}", job.text);
    let html_open_x = glyph_x(&galley, &job.text, "<textarea>")?;
    let html_close_x = glyph_x(&galley, &job.text, "</textarea>")?;
    let html_next_x = glyph_x(&galley, &job.text, "<p>")?;
    assert!(
        (html_open_x - html_close_x).abs() < 0.5,
        "{html_open_x} vs {html_close_x}"
    );
    assert!(
        (html_open_x - html_next_x).abs() < 0.5,
        "{html_open_x} vs {html_next_x}"
    );
    Ok(())
}

fn find_text_shape(
    shapes: &[egui::epaint::ClippedShape],
    predicate: &impl Fn(&str) -> bool,
) -> Option<(egui::Pos2, Arc<Galley>)> {
    fn walk(
        shape: &egui::Shape,
        predicate: &impl Fn(&str) -> bool,
    ) -> Option<(egui::Pos2, Arc<Galley>)> {
        match shape {
            egui::Shape::Text(text) if predicate(text.galley.text()) => {
                Some((text.pos, Arc::clone(&text.galley)))
            }
            egui::Shape::Vec(items) => items.iter().find_map(|item| walk(item, predicate)),
            _ => None,
        }
    }
    shapes
        .iter()
        .find_map(|clipped| walk(&clipped.shape, predicate))
}

fn cursor_screen_pos(
    origin: egui::Pos2,
    galley: &Galley,
    byte_index: usize,
) -> Result<egui::Pos2, String> {
    let char_index = galley
        .text()
        .get(..byte_index)
        .ok_or_else(|| format!("invalid byte index {byte_index}"))?
        .chars()
        .count();
    let rect = galley.pos_from_cursor(egui::text::CCursor::new(char_index));
    Ok(egui::pos2(
        origin.x + rect.min.x,
        origin.y + rect.center().y,
    ))
}

fn run_frame(
    ctx: &egui::Context,
    time: f64,
    events: Vec<egui::Event>,
    body: impl FnMut(&mut egui::Ui),
) -> egui::FullOutput {
    let input = egui::RawInput {
        time: Some(time),
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(800.0, 900.0),
        )),
        events,
        ..Default::default()
    };
    let mut output = ctx.run_ui(input, body);
    output.textures_delta.clear();
    output
}

fn copied_texts(output: &egui::FullOutput) -> Vec<String> {
    output
        .platform_output
        .commands
        .iter()
        .filter_map(|cmd| match cmd {
            egui::OutputCommand::CopyText(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn drag_select(
    ctx: &egui::Context,
    time: &mut f64,
    from: egui::Pos2,
    to: egui::Pos2,
    mut body: impl FnMut(&mut egui::Ui),
) {
    for events in [
        vec![egui::Event::PointerMoved(from)],
        vec![egui::Event::PointerButton {
            pos: from,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        }],
        vec![egui::Event::PointerMoved(to)],
        vec![egui::Event::PointerButton {
            pos: to,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
    ] {
        *time += 0.05;
        let _ = run_frame(ctx, *time, events, &mut body);
    }
}

fn click_at(
    ctx: &egui::Context,
    time: &mut f64,
    pos: egui::Pos2,
    mut body: impl FnMut(&mut egui::Ui),
) {
    *time += 0.05;
    let _ = run_frame(
        ctx,
        *time,
        vec![
            egui::Event::PointerMoved(pos),
            egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ],
        &mut body,
    );
    *time += 0.02;
    let _ = run_frame(
        ctx,
        *time,
        vec![egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }],
        &mut body,
    );
}

fn key_press(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

fn drag_copy_slice(
    ctx: &egui::Context,
    time: &mut f64,
    pos: egui::Pos2,
    galley: &Galley,
    slice: &str,
    mut draw: impl FnMut(&mut egui::Ui),
) -> Result<Vec<String>, String> {
    let start = galley
        .text()
        .find(slice)
        .ok_or_else(|| format!("missing slice {slice:?}"))?;
    let end = start + slice.len();
    drag_select(
        ctx,
        time,
        cursor_screen_pos(pos, galley, start)?,
        cursor_screen_pos(pos, galley, end)?,
        &mut draw,
    );
    *time += 0.05;
    let out = run_frame(ctx, *time, vec![egui::Event::Copy], &mut draw);
    Ok(copied_texts(&out))
}

#[test]
fn copies_pretty_request_and_response_with_drag_select_all_and_clicks() -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();
    let mut time = 0.0;
    let req_source = BodySource {
        side: BodySide::Request,
        ..source(30, br#"{"req":  "alpha  beta","id":1}"#)
    };
    let resp_source = BodySource {
        side: BodySide::Response,
        ..source(30, br#"{"resp":  "gamma  delta","ok":true}"#)
    };
    let mut draw_pretty = |ui: &mut egui::Ui| {
        let _ = cache.show(ui, &req_source, ViewMode::Pretty, Language::English);
        ui.separator();
        let _ = cache.show(ui, &resp_source, ViewMode::Pretty, Language::English);
    };

    time += 0.05;
    let initial = run_frame(&ctx, time, Vec::new(), &mut draw_pretty);
    let expected_req = "{\n  \"req\": \"alpha  beta\",\n  \"id\": 1\n}";
    let expected_resp = "{\n  \"resp\": \"gamma  delta\",\n  \"ok\": true\n}";
    let (req_pos, req_galley) = find_text_shape(&initial.shapes, &|t| t == expected_req)
        .ok_or("missing Pretty request body galley")?;
    let (resp_pos, resp_galley) = find_text_shape(&initial.shapes, &|t| t == expected_resp)
        .ok_or("missing Pretty response body galley")?;

    let req_slice = "  \"req\": \"alpha  beta\",\n  \"id\": 1";
    let got_req = drag_copy_slice(
        &ctx,
        &mut time,
        req_pos,
        &req_galley,
        req_slice,
        &mut draw_pretty,
    )?;
    assert_eq!(got_req, vec![req_slice.to_owned()]);

    let resp_slice = "  \"resp\": \"gamma  delta\",\n  \"ok\": true";
    let got_resp = drag_copy_slice(
        &ctx,
        &mut time,
        resp_pos,
        &resp_galley,
        resp_slice,
        &mut draw_pretty,
    )?;
    assert_eq!(got_resp, vec![resp_slice.to_owned()]);

    // Ctrl+A in request body selects only the request body, not the response body.
    let req_click = expected_req.find("alpha").ok_or("missing alpha")?;
    click_at(
        &ctx,
        &mut time,
        cursor_screen_pos(req_pos, &req_galley, req_click)?,
        &mut draw_pretty,
    );
    time += 0.05;
    let select_all = run_frame(
        &ctx,
        time,
        vec![
            key_press(egui::Key::A, egui::Modifiers::COMMAND),
            egui::Event::Copy,
        ],
        &mut draw_pretty,
    );
    assert_eq!(copied_texts(&select_all), vec![expected_req.to_owned()]);

    // Double-click selects a word ("gamma") and triple-click selects the line in response body.
    time += 1.0;
    let gamma_byte = expected_resp.find("gamma").ok_or("missing gamma")?;
    let gamma_pos = cursor_screen_pos(resp_pos, &resp_galley, gamma_byte + 2)?;
    click_at(&ctx, &mut time, gamma_pos, &mut draw_pretty);
    click_at(&ctx, &mut time, gamma_pos, &mut draw_pretty);
    time += 0.05;
    let dbl_copy = run_frame(&ctx, time, vec![egui::Event::Copy], &mut draw_pretty);
    assert_eq!(copied_texts(&dbl_copy), vec!["gamma".to_owned()]);

    click_at(&ctx, &mut time, gamma_pos, &mut draw_pretty);
    time += 0.05;
    let triple_copy = run_frame(&ctx, time, vec![egui::Event::Copy], &mut draw_pretty);
    assert_eq!(
        copied_texts(&triple_copy),
        vec!["  \"resp\": \"gamma  delta\",".to_owned()]
    );
    Ok(())
}

#[test]
fn copies_raw_request_and_response_preserving_original_spacing() -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();
    let mut time = 0.0;

    let req_raw = br#"{"req":  "alpha  beta","id":1}"#;
    let resp_raw = br#"{"resp":  "gamma  delta","ok":true}"#;
    let req_source = BodySource {
        side: BodySide::Request,
        ..source(36, req_raw)
    };
    let resp_source = BodySource {
        side: BodySide::Response,
        ..source(36, resp_raw)
    };
    let mut draw_raw = |ui: &mut egui::Ui| {
        let _ = cache.show(ui, &req_source, ViewMode::Raw, Language::English);
        ui.separator();
        let _ = cache.show(ui, &resp_source, ViewMode::Raw, Language::English);
    };

    time += 0.05;
    let initial = run_frame(&ctx, time, Vec::new(), &mut draw_raw);
    let expected_req = std::str::from_utf8(req_raw).map_err(|e| e.to_string())?;
    let expected_resp = std::str::from_utf8(resp_raw).map_err(|e| e.to_string())?;
    let (req_pos, req_galley) = find_text_shape(&initial.shapes, &|t| t == expected_req)
        .ok_or("missing Raw request body galley")?;
    let (resp_pos, resp_galley) = find_text_shape(&initial.shapes, &|t| t == expected_resp)
        .ok_or("missing Raw response body galley")?;

    let raw_req_slice = "\"req\":  \"alpha  beta\"";
    assert_eq!(
        drag_copy_slice(
            &ctx,
            &mut time,
            req_pos,
            &req_galley,
            raw_req_slice,
            &mut draw_raw
        )?,
        vec![raw_req_slice.to_owned()]
    );

    let raw_resp_slice = "\"resp\":  \"gamma  delta\"";
    assert_eq!(
        drag_copy_slice(
            &ctx,
            &mut time,
            resp_pos,
            &resp_galley,
            raw_resp_slice,
            &mut draw_raw
        )?,
        vec![raw_resp_slice.to_owned()]
    );
    Ok(())
}

#[test]
fn read_only_body_rejects_text_edits_backspace_paste_and_cut() -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();
    let mut time = 0.0;
    let json = source(31, br#"{"name":"Alice","role":"admin"}"#);
    let expected = "{\n  \"name\": \"Alice\",\n  \"role\": \"admin\"\n}";

    time += 0.05;
    let initial = run_frame(&ctx, time, Vec::new(), |ui| {
        let _ = cache.show(ui, &json, ViewMode::Pretty, Language::English);
    });
    let (pos, galley) =
        find_text_shape(&initial.shapes, &|t| t == expected).ok_or("missing JSON galley")?;

    let start = expected.find("Alice").ok_or("missing Alice")?;
    drag_select(
        &ctx,
        &mut time,
        cursor_screen_pos(pos, &galley, start)?,
        cursor_screen_pos(pos, &galley, start + "Alice".len())?,
        |ui| {
            let _ = cache.show(ui, &json, ViewMode::Pretty, Language::English);
        },
    );

    // Attempt every mutating input event while focused with an active selection.
    time += 0.05;
    let _ = run_frame(
        &ctx,
        time,
        vec![
            egui::Event::Text("Bob".to_owned()),
            key_press(egui::Key::Backspace, egui::Modifiers::NONE),
            key_press(egui::Key::Delete, egui::Modifiers::NONE),
            key_press(egui::Key::Enter, egui::Modifiers::NONE),
            egui::Event::Paste("INJECTED".to_owned()),
            egui::Event::Cut,
        ],
        |ui| {
            let _ = cache.show(ui, &json, ViewMode::Pretty, Language::English);
        },
    );

    let (job, _) = cached_text_galley(&cache)?;
    assert_eq!(job.text, expected, "body text must remain read-only");

    time += 0.05;
    let verify = run_frame(
        &ctx,
        time,
        vec![
            key_press(egui::Key::A, egui::Modifiers::COMMAND),
            egui::Event::Copy,
        ],
        |ui| {
            let _ = cache.show(ui, &json, ViewMode::Pretty, Language::English);
        },
    );
    assert_eq!(copied_texts(&verify), vec![expected.to_owned()]);
    Ok(())
}

#[test]
fn copies_multiline_values_without_visual_leading_space() -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();
    let mut time = 0.0;
    let xml = BodySource {
        media_type: media("application/xml"),
        ..source(
            32,
            b"<root><item><note>line1\nline2\n  line3</note><next>ok</next></item></root>",
        )
    };

    time += 0.05;
    let initial = run_frame(&ctx, time, Vec::new(), |ui| {
        let _ = cache.show(ui, &xml, ViewMode::Pretty, Language::English);
    });
    let (job, _) = cached_text_galley(&cache)?;
    let expected_text = job.text.clone();
    assert!(
        job.sections
            .iter()
            .any(|section| section.leading_space > 0.0),
        "expected visual leading_space on multiline continuation lines"
    );

    let (pos, galley) =
        find_text_shape(&initial.shapes, &|t| t == expected_text).ok_or("missing XML galley")?;
    let needle = "line1\nline2\n  line3";
    let copied = drag_copy_slice(&ctx, &mut time, pos, &galley, needle, |ui| {
        let _ = cache.show(ui, &xml, ViewMode::Pretty, Language::English);
    })?;
    assert_eq!(
        copied,
        vec![needle.to_owned()],
        "visual leading_space must not leak into copied text"
    );
    Ok(())
}

#[test]
fn resets_selection_on_record_and_mode_switch_without_panic() -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();
    let mut time = 0.0;
    let long_body = source(
        33,
        br#"{"first":"value-one","second":"value-two","third":"value-three"}"#,
    );
    let short_body = source(34, br#"{"x":1}"#);

    time += 0.05;
    let initial = run_frame(&ctx, time, Vec::new(), |ui| {
        let _ = cache.show(ui, &long_body, ViewMode::Pretty, Language::English);
    });
    let (job, _) = cached_text_galley(&cache)?;
    let full_pretty = job.text.clone();
    let (pos, galley) = find_text_shape(&initial.shapes, &|t| t == full_pretty)
        .ok_or("missing long Pretty galley")?;
    let tail_start = full_pretty.find("\"third\"").ok_or("missing third field")?;
    drag_select(
        &ctx,
        &mut time,
        cursor_screen_pos(pos, &galley, tail_start)?,
        cursor_screen_pos(pos, &galley, full_pretty.len())?,
        |ui| {
            let _ = cache.show(ui, &long_body, ViewMode::Pretty, Language::English);
        },
    );
    assert!(ctx.memory(egui::Memory::focused).is_some());

    // Switch Pretty -> Raw: selection and focus reset cleanly.
    time += 0.05;
    let switched_mode = run_frame(&ctx, time, vec![egui::Event::Copy], |ui| {
        let _ = cache.show(ui, &long_body, ViewMode::Raw, Language::English);
    });
    assert_eq!(copied_texts(&switched_mode), Vec::<String>::new());
    assert!(ctx.memory(egui::Memory::focused).is_none());

    // Select near the end of long Raw body, then switch to a much shorter record.
    let (raw_job, _) = cached_text_galley(&cache)?;
    let full_raw = raw_job.text.clone();
    let (raw_pos, raw_galley) = find_text_shape(&switched_mode.shapes, &|t| t == full_raw)
        .ok_or("missing long Raw galley")?;
    drag_select(
        &ctx,
        &mut time,
        cursor_screen_pos(raw_pos, &raw_galley, full_raw.len() - 15)?,
        cursor_screen_pos(raw_pos, &raw_galley, full_raw.len())?,
        |ui| {
            let _ = cache.show(ui, &long_body, ViewMode::Raw, Language::English);
        },
    );

    time += 0.05;
    let switched_record = run_frame(&ctx, time, vec![egui::Event::Copy], |ui| {
        let _ = cache.show(ui, &short_body, ViewMode::Raw, Language::English);
    });
    assert_eq!(copied_texts(&switched_record), Vec::<String>::new());
    assert!(ctx.memory(egui::Memory::focused).is_none());
    Ok(())
}

#[test]
fn numbered_lines_view_copies_code_without_line_numbers() -> Result<(), String> {
    let ctx = egui::Context::default();
    let mut cache = BodyViewCache::default();
    let mut time = 0.0;
    let css = BodySource {
        media_type: media("text/css"),
        ..source(35, b"a{color:red;margin:0}")
    };

    let mut draw = |ui: &mut egui::Ui| {
        let _ = cache.show(ui, &css, ViewMode::Pretty, Language::English);
    };
    time += 0.05;
    let initial = run_frame(&ctx, time, Vec::new(), &mut draw);
    let (line1_pos, line1_galley) =
        find_text_shape(&initial.shapes, &|t| t == "a{").ok_or("missing first CSS line")?;
    let (line2_pos, line2_galley) = find_text_shape(&initial.shapes, &|t| t == "  color:red;")
        .ok_or("missing second CSS line")?;

    drag_select(
        &ctx,
        &mut time,
        cursor_screen_pos(line1_pos, &line1_galley, 0)?,
        cursor_screen_pos(line2_pos, &line2_galley, "  color:red;".len())?,
        &mut draw,
    );
    time += 0.05;
    let copied = run_frame(&ctx, time, vec![egui::Event::Copy], &mut draw);
    assert_eq!(copied_texts(&copied), vec!["a{\n  color:red;".to_owned()]);
    Ok(())
}
