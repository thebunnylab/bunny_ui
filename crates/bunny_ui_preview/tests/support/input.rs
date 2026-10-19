use super::*;
use bunny_ui::text_input::{CaretState, EditingStrategy};
use wire::{InstanceCommand, InstanceSelection, SceneInstanceId};

#[derive(Clone, Copy)]
struct Form {
    value: State<String>,
    submitted: State<String>,
    intercepted: State<usize>,
    guarded: bool,
    multiline: bool,
}
impl Component for Form {
    fn body(self) -> impl View {
        let strategy = self
            .guarded
            .then(|| Rc::new(Guard(self.intercepted)) as Rc<dyn EditingStrategy>);
        let field = if self.multiline {
            text_editor("Title", self.value.binding())
        } else {
            text_field("Title", self.value.binding())
        };
        vstack!(
            field
                .editing_strategy(strategy)
                .on_submit(move || self.submitted.set(self.value.get())),
            text!("value={}", self.value),
            text!("submitted={}", self.submitted),
            text!("intercepted={}", self.intercepted),
        )
    }
}
struct Guard(State<usize>);
impl EditingStrategy for Guard {
    fn takes_text(&self) -> bool {
        true
    }
    fn key(
        &self,
        stroke: &bunny_ui::action::Stroke,
        text: &mut String,
        caret: &mut CaretState,
    ) -> bool {
        match stroke.pattern.key {
            Key::Char('!') => {
                self.0.add(1);
                text.push('!');
                *caret = CaretState {
                    caret: text.len(),
                    ..CaretState::default()
                };
                true
            }
            Key::Backspace => {
                self.0.add(1);
                true
            }
            Key::Char('a') if stroke.pattern.command => {
                self.0.add(1);
                true
            }
            _ => false,
        }
    }
}
fn form(guarded: bool, multiline: bool) -> Output {
    Output::of(Form {
        value: State::new(String::new()),
        submitted: State::new(String::new()),
        intercepted: State::new(0),
        guarded,
        multiline,
    })
}
static FORM: PreviewEntry = PreviewEntry::new(
    Meta {
        ident: "input_form",
        name: "Form",
        scope: "Input",
        example: "input",
        module: "host",
        file: "src/lib.rs",
        line: 1,
    },
    || form(false, false),
);
static GUARDED: PreviewEntry = PreviewEntry::new(
    Meta {
        ident: "guarded_form",
        name: "Guarded",
        scope: "Input",
        example: "input",
        module: "host",
        file: "src/lib.rs",
        line: 2,
    },
    || form(true, false),
);
static EDITOR: PreviewEntry = PreviewEntry::new(
    Meta {
        ident: "input_editor",
        name: "Editor",
        scope: "Input",
        example: "input",
        module: "host",
        file: "src/lib.rs",
        line: 3,
    },
    || form(false, true),
);
#[unsafe(export_name = "bunny_preview__input_editor")]
pub fn editor_entry() -> &'static PreviewEntry {
    &EDITOR
}
#[unsafe(export_name = "bunny_preview__input_form")]
pub fn form_entry() -> &'static PreviewEntry {
    &FORM
}
#[unsafe(export_name = "bunny_preview__guarded_form")]
pub fn guarded_entry() -> &'static PreviewEntry {
    &GUARDED
}

#[derive(Clone)]
enum Target {
    Legacy(&'static str),
    Instance { id: SceneInstanceId, variant: u32 },
}
impl Target {
    fn send(&self, host: &mut Host, command: InstanceCommand) {
        let message = match self {
            Self::Instance { id, variant } => ToHost::Instance {
                instance: id.clone(),
                variant: *variant,
                command,
            },
            Self::Legacy(ident) => {
                let id = format!("src/lib.rs#{ident}");
                match command {
                    InstanceCommand::Pointer {
                        kind,
                        x,
                        y,
                        modifiers,
                    } => ToHost::Pointer {
                        id,
                        kind,
                        x,
                        y,
                        modifiers,
                    },
                    InstanceCommand::Key { key, modifiers } => ToHost::Key { id, key, modifiers },
                    InstanceCommand::Text { text } => ToHost::Text { id, text },
                    _ => panic!("unsupported test input"),
                }
            }
        };
        host.handle(message);
    }
    fn take(&self, seen: &Rc<RefCell<Vec<ToIde>>>) -> Option<wire::Frame> {
        let mut result = None;
        for event in seen.borrow_mut().drain(..) {
            match (self, event) {
                (Self::Legacy(_), ToIde::Frame(frame)) => result = Some(frame),
                (Self::Instance { id, variant }, ToIde::InstanceFrame { instance, frame })
                    if *id == instance && *variant == frame.variant =>
                {
                    result = Some(*frame)
                }
                (_, other) => panic!("input affected another scene: {other:?}"),
            }
        }
        result
    }
    fn focus(&self, host: &mut Host, frame: &wire::Frame) {
        let field = frame
            .sites
            .iter()
            .find(|site| frame.strings[site.kind as usize] == "text_field")
            .unwrap();
        for kind in [
            wire::PointerKind::Move,
            wire::PointerKind::Down,
            wire::PointerKind::Up,
        ] {
            self.send(
                host,
                InstanceCommand::Pointer {
                    kind,
                    x: field.frame.origin.x + field.frame.size.width / 2.0,
                    y: field.frame.origin.y + field.frame.size.height / 2.0,
                    modifiers: 0,
                },
            );
        }
    }
    fn key(&self, host: &mut Host, key: &str, modifiers: u32) {
        self.send(
            host,
            InstanceCommand::Key {
                key: key.into(),
                modifiers,
            },
        );
    }
    fn text(&self, host: &mut Host, text: &str) {
        self.send(host, InstanceCommand::Text { text: text.into() });
    }
    fn value(&self, host: &mut Host, seen: &Rc<RefCell<Vec<ToIde>>>, value: &str) {
        assert!(host.tick(), "editing should publish a frame");
        let frame = self
            .take(seen)
            .expect("the addressed scene publishes its frame");
        assert!(
            says(&frame, &format!("value={value}")),
            "expected value={value}; got {:?}",
            frame.strings
        );
    }
}

fn edit(target: &Target, host: &mut Host, seen: &Rc<RefCell<Vec<ToIde>>>, first: &wire::Frame) {
    target.focus(host, first);
    target.text(host, "hé👩‍💻");
    target.value(host, seen, "hé👩‍💻");
    target.key(host, "Backspace", 0);
    target.value(host, seen, "hé");
    target.key(host, "Left", 1);
    target.text(host, "ã");
    target.value(host, seen, "hã");
    target.key(host, "Home", 0);
    target.key(host, "Delete", 0);
    target.key(host, "End", 0);
    target.text(host, " ok");
    target.value(host, seen, "ã ok");
    target.key(host, "a", 8);
    target.text(host, "next");
    target.value(host, seen, "next");
    target.key(host, "a", 2);
    target.text(host, "done");
    target.key(host, "Enter", 0);
    assert!(host.tick());
    let frame = target.take(seen).unwrap();
    assert!(says(&frame, "value=done"));
    assert!(says(&frame, "submitted=done"));
}

#[test]
fn legacy_field_accepts_unicode_selection_deletion_and_submission() {
    let (mut host, seen) = host();
    host.handle(select("input_form", fixed(300.0, 160.0)));
    host.tick();
    let target = Target::Legacy("input_form");
    let first = target.take(&seen).unwrap();
    edit(&target, &mut host, &seen, &first);
}

#[test]
fn instance_field_editing_is_scoped_to_its_instance_and_variant() {
    let (mut host, seen) = host();
    let a = SceneInstanceId::parse("field-a").unwrap();
    let b = SceneInstanceId::parse("field-b").unwrap();
    host.handle(ToHost::SelectInstances(vec![
        InstanceSelection {
            instance: a.clone(),
            ident: "input_form".into(),
            traits: Traits {
                scheme: wire::Scheme::Both,
                ..fixed(300.0, 160.0)
            },
        },
        InstanceSelection {
            instance: b.clone(),
            ident: "input_form".into(),
            traits: fixed(300.0, 160.0),
        },
    ]));
    host.tick();
    let initial = instance_frames(&seen);
    assert_eq!(initial.len(), 3);
    let target = Target::Instance {
        id: a.clone(),
        variant: 1,
    };
    let first = &initial
        .iter()
        .find(|(id, frame)| *id == a && frame.variant == 1)
        .unwrap()
        .1;
    edit(&target, &mut host, &seen, first);
    for id in [a, b] {
        Target::Instance { id, variant: 0 }.send(
            &mut host,
            InstanceCommand::Resize(wire::Size {
                width: 320.0,
                height: 160.0,
            }),
        );
    }
    host.tick();
    let untouched = instance_frames(&seen);
    assert_eq!(untouched.len(), 2);
    assert!(
        untouched
            .iter()
            .all(|(_, frame)| says(frame, "value=") && says(frame, "submitted="))
    );
}

#[test]
fn field_strategy_consumes_strokes_once_before_default_editing() {
    let (mut host, seen) = host();
    host.handle(select("guarded_form", fixed(300.0, 160.0)));
    host.tick();
    let target = Target::Legacy("guarded_form");
    let first = target.take(&seen).unwrap();
    target.focus(&mut host, &first);
    target.text(&mut host, "!");
    target.key(&mut host, "Backspace", 0);
    target.key(&mut host, "a", 2);
    target.text(&mut host, "ok");
    host.tick();
    let frame = target.take(&seen).unwrap();
    assert!(says(&frame, "value=!ok"), "{:?}", frame.strings);
    assert!(says(&frame, "intercepted=3"));
}

fn instance_frames(seen: &Rc<RefCell<Vec<ToIde>>>) -> Vec<(SceneInstanceId, wire::Frame)> {
    seen.borrow_mut()
        .drain(..)
        .map(|event| match event {
            ToIde::InstanceFrame { instance, frame } => (instance, *frame),
            other => panic!("expected instance frame: {other:?}"),
        })
        .collect()
}

#[test]
fn multiline_enter_arrows_and_committed_text_follow_native_field_semantics() {
    let (mut host, seen) = host();
    host.handle(select("input_editor", fixed(300.0, 240.0)));
    host.tick();
    let target = Target::Legacy("input_editor");
    let first = target.take(&seen).unwrap();
    target.focus(&mut host, &first);
    // A physical printable key and its committed text must insert once.
    target.key(&mut host, "a", 0);
    target.text(&mut host, "ab");
    target.key(&mut host, "Enter", 0);
    target.text(&mut host, "cd");
    host.tick();
    target.take(&seen).unwrap();
    target.key(&mut host, "Up", 0);
    target.key(&mut host, "Home", 0);
    target.key(&mut host, "Right", 0);
    target.key(&mut host, "Down", 1);
    target.text(&mut host, "!");
    target.key(&mut host, "Enter", 2);
    host.tick();
    let frame = target.take(&seen).unwrap();
    assert!(says(&frame, "value=a!d"), "{:?}", frame.strings);
    assert!(says(&frame, "submitted=a!d"));
    target.key(&mut host, "Escape", 0);
    target.text(&mut host, "ignored");
    target.value(&mut host, &seen, "a!d");
}
