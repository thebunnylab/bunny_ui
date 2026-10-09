use bunny_ui::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

struct OwnedLabel {
    text: String,
    drops: Rc<Cell<usize>>,
}

impl OwnedLabel {
    fn as_str(&self) -> &str {
        &self.text
    }
}

impl Drop for OwnedLabel {
    fn drop(&mut self) {
        self.drops.set(self.drops.get() + 1);
    }
}

#[test]
fn an_async_task_borrows_a_non_clone_capture_across_await() {
    let runtime = Runtime::scene("async_capture");
    let drops = Rc::new(Cell::new(0));
    let label = OwnedLabel {
        text: "Log".into(),
        drops: Rc::clone(&drops),
    };
    let sender = Rc::new(RefCell::new(None));
    let task_sender = Rc::clone(&sender);
    let output = State::new(String::new());
    let view = text(output).task(async move || {
        let prefix = label.as_str();
        let (send, reader) = task::channel::<String>();
        *task_sender.borrow_mut() = Some(send);
        if let Some(line) = reader.recv().await {
            output.set(format!("{prefix}: {line}"));
        }
    });

    runtime.render_stable(&view);
    assert_eq!(
        drops.get(),
        0,
        "the pending future owns its borrowed capture"
    );
    sender
        .borrow()
        .as_ref()
        .unwrap()
        .send("ready".into())
        .unwrap();
    assert!(runtime.render_stable(&view).contains("Log: ready"));
    assert_eq!(motor::task::pending(), 0);

    drop(view);
    runtime.render_stable(&empty());
    assert_eq!(drops.get(), 1, "unmount releases the retained factory too");
}

struct Running(Rc<Cell<usize>>);

impl Drop for Running {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[derive(Clone)]
struct Details {
    visible: State<bool>,
    key: State<usize>,
    revision: State<usize>,
    output: State<String>,
    starts: Rc<Cell<usize>>,
    drops: Rc<Cell<usize>>,
    senders: Rc<RefCell<Vec<task::Sender<()>>>>,
}

impl Component for Details {
    fn body(self) -> impl View {
        if self.visible.get() {
            let prefix = format!("file {}", self.key.get());
            Either::First(text(format!("revision {}", self.revision.get())).task_id(
                self.key.get(),
                async move || {
                    let _running = Running(Rc::clone(&self.drops));
                    self.starts.set(self.starts.get() + 1);
                    let borrowed = prefix.as_str();
                    let (sender, reader) = task::channel();
                    self.senders.borrow_mut().push(sender);
                    if reader.recv().await.is_some() {
                        self.output.set(borrowed.to_owned());
                    }
                },
            ))
        } else {
            Either::Second(empty())
        }
    }
}

#[test]
fn async_task_identity_controls_restart_completion_and_cancellation() {
    let runtime = Runtime::scene("async_identity");
    let view = Details {
        visible: State::new(true),
        key: State::new(1),
        revision: State::new(0),
        output: State::new(String::new()),
        starts: Rc::new(Cell::new(0)),
        drops: Rc::new(Cell::new(0)),
        senders: Rc::new(RefCell::new(Vec::new())),
    };
    runtime.render_stable(&view);
    assert_eq!(view.starts.get(), 1);

    view.revision.set(1);
    runtime.render_stable(&view);
    assert_eq!(view.starts.get(), 1, "a new body does not restart the task");
    assert_eq!(
        view.drops.get(),
        0,
        "the old factory remains valid across a rebuild"
    );

    view.key.set(2);
    runtime.render_stable(&view);
    assert_eq!(view.starts.get(), 2);
    assert_eq!(view.drops.get(), 1, "the old future was cancelled");
    assert!(view.senders.borrow()[0].send(()).is_err());
    view.senders.borrow()[1].send(()).unwrap();
    runtime.render_stable(&view);
    assert_eq!(
        view.output.get(),
        "file 2",
        "only the current task completes"
    );
    assert_eq!(view.drops.get(), 2);
    assert_eq!(motor::task::pending(), 0);

    view.revision.set(2);
    runtime.render_stable(&view);
    assert_eq!(
        view.starts.get(),
        2,
        "a completed task stays complete for its id"
    );

    view.key.set(3);
    runtime.render_stable(&view);
    assert_eq!(view.starts.get(), 3);
    view.visible.set(false);
    runtime.render_stable(&view);
    assert_eq!(view.drops.get(), 3, "unmount cancels the pending task");
    assert!(view.senders.borrow()[2].send(()).is_err());
    assert_eq!(motor::task::pending(), 0);
}

#[test]
fn ordinary_future_factories_remain_compatible() {
    fn with_task<F>(start: F) -> impl View
    where
        F: AsyncFn() + 'static,
    {
        text("legacy").task(start)
    }

    let runtime = Runtime::scene("async_compatibility");
    let calls = State::new(0);
    let view = vstack((
        with_task(move || async move { calls.add(1) }),
        text("explicit site").task_keyed("async_site", None, async move || calls.add(1)),
    ));
    runtime.render_stable(&view);
    assert_eq!(calls.get(), 2);
    runtime.render_stable(&view);
    assert_eq!(calls.get(), 2);
}
