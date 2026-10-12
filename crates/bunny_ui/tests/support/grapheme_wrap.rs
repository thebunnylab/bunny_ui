//! The same public wrapping/editing contract runs against deterministic and native engines.
use bunny_ui::layout::{DrawCommand, Proposal, Size};
use bunny_ui::prelude::*;
use bunny_ui::text_engine::{FontSpec, MeasureCache, TextEngine, break_lines};
use bunny_ui::text_input::{EditCommand, utf16_to_byte};
use std::rc::Rc;

const CLUSTERS: [&str; 8] = ["a", "e\u{301}", "👨‍👩‍👧‍👦", "🇧🇷", "👍🏽", "1️⃣", "क्\u{200d}ष", "z"];

fn boundaries(clusters: &[&str]) -> Vec<usize> {
    std::iter::once(0)
        .chain(clusters.iter().scan(0, |at, cluster| {
            *at += cluster.len();
            Some(*at)
        }))
        .collect()
}

fn check_ranges(text: &str, allowed: &[usize], lines: &[(usize, usize)]) {
    assert!(!lines.is_empty());
    let mut next = 0;
    for &(start, end) in lines {
        assert_eq!(start, next, "lost or duplicated bytes in {text:?}");
        assert!(
            start < end || text.is_empty(),
            "spurious empty soft line: {lines:?}"
        );
        assert!(
            allowed.contains(&start) && allowed.contains(&end),
            "soft break inside a cluster: {start}..{end} of {text:?}, expected {allowed:?}"
        );
        next = end;
    }
    assert_eq!(next, text.len());
}

pub(crate) fn clusters_stay_whole(engine: &dyn TextEngine) {
    let font = FontSpec::DEFAULT;
    for clusters in [
        &CLUSTERS[..],
        &["a", " \u{301}", "b"],
        &["\u{600} ", "x"],
        &["e\u{301}"],
    ] {
        let text = clusters.concat();
        let allowed = boundaries(clusters);
        for width in [0.0, 1.0, 8.0, 32.0, 80.0] {
            let lines = break_lines(&text, &font, width, engine, &MeasureCache::default());
            check_ranges(&text, &allowed, &lines);
        }
    }
    for cluster in CLUSTERS {
        let width = engine.measure_line(cluster, &font).width;
        assert!(
            width.is_finite() && width > 0.0,
            "native measurement unavailable for {cluster:?}"
        );
        let lines = break_lines(
            cluster,
            &font,
            width / 2.0,
            engine,
            &MeasureCache::default(),
        );
        assert_eq!(
            lines,
            [(0, cluster.len())],
            "one oversized cluster must remain one line: {cluster:?}"
        );
    }
}

pub(crate) fn retained_editor_keeps_atomic_lines(engine: Rc<dyn TextEngine>) {
    let runtime = Runtime::new().text_engine(engine);
    let content = State::new(CLUSTERS.concat());
    let view = text_editor("", content.binding())
        .font_size(15.0)
        .frame(40.0, 500.0)
        .id("editor");
    let size = Size {
        width: 40.0,
        height: 500.0,
    };
    for clusters in [&CLUSTERS[..], &["z", " \u{301}", "👨‍👩‍👧‍👦", "e\u{301}", "🇧🇷"]]
    {
        let text = clusters.concat();
        let allowed = boundaries(clusters);
        content.set(text.clone());
        runtime.display_frame(&view, size);
        let layout = runtime.layout(&view, Proposal::exact(size));
        let ranges: Vec<_> = layout
            .display
            .iter()
            .filter_map(|command| match command {
                DrawCommand::TextLine { content, range, .. } if content.as_ref() == text => {
                    Some(*range)
                }
                _ => None,
            })
            .collect();
        check_ranges(&text, &allowed, &ranges);
        assert!(runtime.focus_named("editor"));
        runtime.key(EditCommand::Home(false));
        for _ in 0..ranges.len() + 1 {
            let caret = runtime.ime_snapshot().unwrap().selected.0;
            assert!(
                allowed.contains(&utf16_to_byte(&text, caret)),
                "vertical motion split a cluster"
            );
            runtime.key(EditCommand::Down(false));
        }
        runtime.key(EditCommand::SelectAll);
        assert_eq!(
            runtime.key(EditCommand::Copy).output.as_deref(),
            Some(text.as_str())
        );
    }
}
