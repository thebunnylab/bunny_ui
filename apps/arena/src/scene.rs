//! Observe the scene produced by the native window at script boundaries.
//!
//! The observer uses the same root and runtime as the window. Its extra
//! layout occurs before the first input or after completion, outside the
//! steady CPU window; it never replaces the window's rendering path.
use std::cell::{Cell, OnceCell};
use std::rc::Rc;

use bunny_ui::layout::{DisplayList, DrawCommand, LayoutResult, Proposal};
use bunny_ui::prelude::*;
use bunny_ui_macos::{App, CoreGraphicsImageEngine, CoreTextEngine, WindowSpec};
use serde_json::{Value, json};

use crate::{Args, Step, WINDOW, scripted, unix_ms};

type Observer = Box<dyn Fn(&str)>;

/// Open an ordinary arena window and report actual layout at input boundaries.
/// `content` supplies the fixture's stored item count and completion proof.
pub fn run<V: View<Arity = Single>>(
    title: &str,
    kind: &'static str,
    view: V,
    args: Args,
    on_step: impl Fn(Step) + 'static,
    content: impl Fn() -> Value + 'static,
) {
    let runtime = Rc::new(
        Runtime::new()
            .text_engine(Rc::new(CoreTextEngine::new()))
            .image_engine(Rc::new(CoreGraphicsImageEngine::new())),
    );
    let observer: Rc<OnceCell<Observer>> = Rc::new(OnceCell::new());
    let weak = Rc::downgrade(&observer);
    let ready = Cell::new(false);
    let root = scripted(view, args, (WINDOW.0 / 2.0, WINDOW.1 / 2.0), move |step| {
        if let Step::Checkpoint(point) = step {
            emit(&weak, point.marker());
            return;
        }
        let done = matches!(step, Step::Done);
        if !done && !ready.replace(true) {
            emit(&weak, "SCENE_READY");
        }
        on_step(step);
        if done {
            emit(&weak, "SCENE_DONE");
        }
    });
    let copy = root.clone();
    let observed = runtime.clone();
    let _ = observer.set(Box::new(move |marker| {
        let Some(size) = observed.last_viewport() else {
            eprintln!("SCENE_STATE_ERROR no native viewport");
            return;
        };
        let layout = observed.settled_layout(&copy, Proposal::exact(size));
        let mut value = geometry(kind, &layout, &observed);
        value["content"] = content();
        println!("{marker} {} {value}", unix_ms());
    }));
    let app = App::new();
    app.open(
        WindowSpec::titled(title).size(WINDOW.0, WINDOW.1),
        runtime,
        root,
    );
    app.run();
}

fn emit(weak: &std::rc::Weak<OnceCell<Observer>>, marker: &str) {
    if let Some(holder) = weak.upgrade() {
        let report = holder.get();
        if let Some(report) = report {
            report(marker);
        }
    }
}

fn geometry(kind: &str, layout: &LayoutResult, runtime: &Runtime) -> Value {
    let mut value = json!({
        "protocol": "scene-geometry-v1", "kind": kind,
        "content_size": [layout.size.width, layout.size.height],
    });
    if let Some(scroll) = layout.scrolls.first() {
        let rect = scroll.frame;
        let offset = runtime.scroll_offset(&scroll.path);
        value["viewport"] = json!([
            rect.origin.x,
            rect.origin.y,
            rect.size.width,
            rect.size.height
        ]);
        value["scroll_y"] = json!(offset.y);
        value["row_pitch"] = json!(scroll.row_extent);
        value["content_height"] = json!(scroll.content.height);
        let fields = visible_fields(
            &layout.display,
            rect.origin.y,
            rect.origin.y + rect.size.height,
        );
        value["fields"] = json!(fields);
    }
    if kind == "canvas" {
        if let Some(canvas) = layout.customs.iter().find(|item| item.live.is_some()) {
            let r = canvas.frame;
            value["drawing_rect"] = json!([r.origin.x, r.origin.y, r.size.width, r.size.height]);
        }
        value["fields"] = json!(visible_fields(&layout.display, 0.0, 40.0));
    }
    value
}

fn visible_fields(display: &DisplayList, top: f64, bottom: f64) -> Vec<Value> {
    display
        .iter()
        .filter_map(|command| {
            let DrawCommand::TextLine {
                origin,
                content,
                range,
                font,
                ..
            } = command
            else {
                return None;
            };
            (origin.y >= top && origin.y < bottom).then(|| {
                json!({
                    "origin": [origin.x, origin.y], "text": &content[range.0..range.1],
                    "size": font.size, "family": font.family.name().as_deref(),
                })
            })
        })
        .take(12)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bunny_ui::layout::{Color, Point};
    use bunny_ui::text_engine::{Family, FontSpec};

    #[test]
    fn fields_use_drawn_text_positions_and_exclude_the_header() {
        let mut commands = Vec::new();
        for (text, x, y) in [
            ("header", 12.0, 10.0),
            ("first", 19.0, 44.0),
            ("next", 101.0, 44.0),
            ("below", 19.0, 801.0),
        ] {
            commands.push(DrawCommand::TextLine {
                origin: Point { x, y },
                content: text.into(),
                range: (0, text.len()),
                color: Color::WHITE,
                font: FontSpec {
                    size: 12.5,
                    family: Family::named("Menlo"),
                    ..FontSpec::DEFAULT
                },
            });
        }
        let fields = visible_fields(&DisplayList::from(commands), 40.0, 800.0);
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0]["text"], "first");
        assert_eq!(fields[0]["origin"], json!([19.0, 44.0]));
        assert_eq!(fields[1]["origin"], json!([101.0, 44.0]));
        assert_eq!(fields[0]["size"], 12.5);
        assert_eq!(fields[0]["family"], "Menlo");
    }
}
