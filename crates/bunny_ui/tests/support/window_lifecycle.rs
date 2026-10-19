//! Public lifecycle contract shared by native shell witnesses.
use crate::shell::{App, CloseAction, WindowError, WindowId, WindowSpec, WindowVisibility};
use bunny_ui::prelude::*;
use std::{
    cell::Cell,
    rc::Rc,
    time::{Duration, Instant},
};

type NativeClose = fn(&App, WindowId, &str);

pub fn run(native_close: NativeClose) {
    if let Some(mode) = std::env::var_os("BUNNY_LIFECYCLE_CHILD") {
        match mode.to_str().expect("fixed test mode") {
            "policies" => policies(native_close),
            "wake" => wake(),
            "default" => final_close(native_close),
            "quit" => quit(),
            mode => panic!("unknown lifecycle mode {mode}"),
        }
        return;
    }
    for mode in ["policies", "wake", "default", "quit"] {
        run_child(mode);
    }

    println!(
        "Lifecycle subprocesses passed: policies, all-hidden channel wake, default close and explicit Quit"
    );
}

pub fn run_child(mode: &str) {
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .env("BUNNY_LIFECYCLE_CHILD", mode)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "lifecycle mode {mode} failed: {status}");
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("lifecycle mode {mode} did not terminate");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn policies(native_close: NativeClose) {
    let app = App::new();
    let guard = app.open(
        WindowSpec::titled("Lifecycle guard"),
        Rc::new(app.runtime()),
        text("guard"),
    );
    let value = State::new(String::from("retained draft"));
    let runtime = Rc::new(app.runtime());
    let id = app.open(
        WindowSpec::titled("Lifecycle subject"),
        Rc::clone(&runtime),
        text_field("Draft", value.binding()).id("draft"),
    );
    let feed = bunny_ui::image_engine::ImageFeed::new();
    let observer = app
        .observe_feed(id, feed.key(), 8)
        .expect("owned live window");
    assert!(!observer.snapshot().closed);
    assert!(runtime.focus_named("draft"));
    assert_eq!(app.visibility(id), Ok(WindowVisibility::Shown));
    let foreign = App::new();
    assert_eq!(foreign.hide(id), Err(WindowError::StaleWindow));
    assert!(foreign.observe_feed(id, feed.key(), 8).is_none());
    let focus = runtime.focused();
    for _ in 0..3 {
        app.hide(id).unwrap();
        app.hide(id).unwrap();
        assert_eq!(app.visibility(id), Ok(WindowVisibility::Hidden));
        assert!(app.windows().contains(&id));
        app.show(id).unwrap();
        app.show(id).unwrap();
        assert_eq!(app.visibility(id), Ok(WindowVisibility::Shown));
        assert_eq!(value.get(), "retained draft");
        assert_eq!(runtime.focused(), focus);
        assert!(!observer.snapshot().closed);
    }
    let calls = Rc::new(Cell::new(0));
    app.on_close_requested(id, {
        let calls = Rc::clone(&calls);
        move |_| {
            calls.set(calls.get() + 1);
            CloseAction::Cancel
        }
    })
    .unwrap();
    native_close(&app, id, "Lifecycle subject");
    assert_eq!(calls.get(), 1, "close request consults policy once");
    assert!(!observer.snapshot().closed);
    assert_eq!(app.visibility(id), Ok(WindowVisibility::Shown));
    app.on_close_requested(id, |_| CloseAction::Hide).unwrap();
    native_close(&app, id, "Lifecycle subject");
    assert_eq!(app.visibility(id), Ok(WindowVisibility::Hidden));
    app.show(id).unwrap();
    app.on_close_requested(id, {
        let app = app.clone();
        move |same| {
            assert_eq!(app.request_close(same), Err(WindowError::CloseInProgress));
            app.on_close_requested(same, |_| CloseAction::Hide).unwrap();
            CloseAction::Cancel
        }
    })
    .unwrap();
    assert_eq!(app.request_close(id), Ok(CloseAction::Cancel));
    assert_eq!(app.request_close(id), Ok(CloseAction::Hide));
    app.show(id).unwrap();
    app.clear_close_handler(id).unwrap();
    assert_eq!(app.request_close(id), Ok(CloseAction::Close));
    assert!(observer.snapshot().closed);
    assert!(app.observe_feed(id, feed.key(), 8).is_none());
    assert_eq!(app.visibility(id), Err(WindowError::StaleWindow));
    assert_eq!(app.show(id), Err(WindowError::StaleWindow));
    assert_eq!(app.hide(id), Err(WindowError::StaleWindow));
    assert_eq!(app.request_close(id), Err(WindowError::StaleWindow));
    let replacement = app.open(
        WindowSpec::titled("Lifecycle replacement"),
        Rc::new(app.runtime()),
        text("replacement"),
    );
    assert_ne!(id, replacement);
    app.close(id);
    assert!(app.windows().contains(&replacement));
    app.on_close_requested(replacement, {
        let app = app.clone();
        move |same| {
            app.close(same);
            CloseAction::Close
        }
    })
    .unwrap();
    assert_eq!(app.request_close(replacement), Ok(CloseAction::Close));
    assert_eq!(app.windows(), vec![guard]);
    let bypass = app.open(
        WindowSpec::titled("Lifecycle explicit close"),
        Rc::new(app.runtime()),
        text("explicit"),
    );
    app.on_close_requested(bypass, |_| {
        panic!("explicit teardown must bypass close policy")
    })
    .unwrap();
    let bypass_observer = app.observe_feed(bypass, feed.key(), 8).unwrap();
    app.close(bypass);
    assert!(bypass_observer.snapshot().closed);
    assert_eq!(app.windows(), vec![guard]);
    println!(
        "native lifecycle policies, retained draft, repeated hide/show and stale handles passed"
    );
    // Release the final native window while its platform services still live.
    // Dedicated subprocesses separately witness default-close exit and wake.
    app.quit();
    app.run();
    assert!(app.windows().is_empty());
}

fn wake() {
    let app = App::new();
    let value = State::new(String::from("before wake"));
    let id = app.open(
        WindowSpec::titled("Lifecycle hidden wake"),
        Rc::new(app.runtime()),
        text_field("Draft", value.binding()),
    );
    app.on_close_requested(id, |_| CloseAction::Hide).unwrap();
    app.hide(id).unwrap();
    let (sender, reader) = task::channel();
    let awakened = app.clone();
    task::spawn(async move {
        assert_eq!(reader.recv().await, Some("worker arrived"));
        assert_eq!(awakened.visibility(id), Ok(WindowVisibility::Hidden));
        value.set("after wake".into());
        task::sleep(Duration::from_millis(200)).await;
        assert_eq!(awakened.visibility(id), Ok(WindowVisibility::Hidden));
        awakened.show(id).unwrap();
        assert_eq!(awakened.visibility(id), Ok(WindowVisibility::Shown));
        assert_eq!(value.get(), "after wake");
        // Allow the remap protocol and a real present to run before final Quit.
        task::sleep(Duration::from_millis(200)).await;
        awakened.hide(id).unwrap();
        println!("All-hidden background channel woke the retained scene; explicit Quit follows");
        awakened.quit();
    })
    .detach();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        sender.send("worker arrived").unwrap();
    });
    app.run();
    assert!(app.windows().is_empty());
}

fn final_close(native_close: NativeClose) {
    let app = App::new();
    let id = app.open(
        WindowSpec::titled("Lifecycle default close"),
        Rc::new(app.runtime()),
        text("last"),
    );
    let feed = bunny_ui::image_engine::ImageFeed::new();
    let observer = app.observe_feed(id, feed.key(), 8).unwrap();
    println!("Requesting default close on the final owned window");
    native_close(&app, id, "Lifecycle default close");
    assert!(observer.snapshot().closed);
    app.run();
    assert!(app.windows().is_empty());
}

fn quit() {
    let app = App::new();
    for title in ["Lifecycle quit one", "Lifecycle quit two"] {
        let id = app.open(
            WindowSpec::titled(title),
            Rc::new(app.runtime()),
            text("retained"),
        );
        app.on_close_requested(id, |_| panic!("Quit must bypass policy"))
            .unwrap();
        app.hide(id).unwrap();
    }
    println!("Explicit Quit with every window hidden and non-closing policies installed");
    app.quit();
    app.run();
    assert!(app.windows().is_empty());
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
pub fn child_surfaces(visible_children: fn(&str) -> usize) {
    use bunny_ui::layout::DialogSpec;
    use bunny_ui::prelude::*;
    use std::{rc::Rc, time::Duration};
    let app = App::new();
    let dialog = State::new(true);
    let popup = State::new(true);
    let value = State::new(String::from("visible draft"));
    let runtime = Rc::new(app.runtime());
    let id = app.open(
        WindowSpec::titled("Lifecycle parent"),
        Rc::clone(&runtime),
        text("parent").dialog(
            dialog.binding(),
            DialogSpec::titled("Lifecycle child dialog"),
            move |_| {
                erased(text_field("Draft", value.binding()).popover(
                    popup.binding(),
                    Side::Bottom,
                    |_| erased(text("retained popup")),
                ))
            },
        ),
    );
    let running = app.clone();
    task::spawn(async move {
        task::sleep(Duration::from_millis(200)).await;
        eprintln!("Lifecycle children: initial frame settled");
        let before = visible_children("Lifecycle parent");
        assert!(before >= 2, "dialog and its popup are actually visible: {before}");
        eprintln!("Lifecycle children: hiding parent with {before} visible surfaces");
        running.hide(id).unwrap();
        assert_eq!(visible_children("Lifecycle parent"), 0, "hiding the owner hides every descendant");
        eprintln!("Lifecycle children: hidden; mutating draft and awaiting timer");
        value.set("changed while hidden".into());
        runtime.request_frame();
        task::sleep(Duration::from_millis(200)).await;
        assert_eq!(visible_children("Lifecycle parent"), 0, "background work must not reveal an overlay");
        eprintln!("Lifecycle children: timer woke; showing parent");
        running.show(id).unwrap();
        task::sleep(Duration::from_millis(200)).await;
        assert_eq!(visible_children("Lifecycle parent"), before, "show restores retained child surfaces");
        assert_eq!(value.get(), "changed while hidden");
        println!("Native dialog and nested popup remain hidden during background updates and restore on show");
        running.quit();
    }).detach();
    app.run();
}
