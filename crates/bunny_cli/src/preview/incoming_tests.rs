use super::incoming::{Signal, read};
use super::*;
use std::io::{Cursor, Read};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn message() -> Message {
    Message::ToIde(ToIde::Ready {
        generation: 1,
        previews: Vec::new(),
        missing: Vec::new(),
    })
}

#[test]
fn reader_enqueues_before_waking_and_notifies_closure() {
    let signal = Signal::default();
    let (sender, inbox) = mpsc::channel();
    let inbox = Arc::new(Mutex::new(inbox));
    let observed = Arc::new(Mutex::new(Vec::new()));
    let read_inbox = inbox.clone();
    let saved = observed.clone();
    signal.set(Some(Arc::new(move || {
        saved
            .lock()
            .unwrap()
            .push(read_inbox.lock().unwrap().try_recv());
    })));
    read(Cursor::new(wire::encode(&message())), sender, &signal);
    let events = observed.lock().unwrap();
    assert_eq!(events.len(), 3, "registration, message, closure");
    assert!(matches!(events[0], Err(mpsc::TryRecvError::Empty)));
    assert!(matches!(&events[1], Ok(Received::Message(value)) if value == &message()));
    assert!(matches!(events[2], Err(mpsc::TryRecvError::Disconnected)));
}

#[test]
fn registration_covers_preexisting_messages_and_replacement_can_reenter() {
    let signal = Signal::default();
    let (sender, inbox) = mpsc::channel();
    read(Cursor::new(wire::encode(&message())), sender, &signal);
    let count = Arc::new(AtomicUsize::new(0));
    let counted = count.clone();
    let setting = signal.clone();
    signal.set(Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        setting.set(None); // callback runs outside its registration lock.
    })));
    assert_eq!(count.load(Ordering::SeqCst), 1);
    assert!(matches!(inbox.try_recv(), Ok(Received::Message(value)) if value == message()));
    signal.notify();
    assert_eq!(count.load(Ordering::SeqCst), 1);
    let counted = count.clone();
    signal.set(Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    })));
    signal.notify();
    assert_eq!(count.load(Ordering::SeqCst), 3);
}

#[test]
fn read_failure_still_wakes_the_owner() {
    struct Broken;
    impl Read for Broken {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("closed"))
        }
    }
    let signal = Signal::default();
    let count = Arc::new(AtomicUsize::new(0));
    let counted = count.clone();
    signal.set(Some(Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
    })));
    let (sender, _) = mpsc::channel();
    read(Broken, sender, &signal);
    assert_eq!(count.load(Ordering::SeqCst), 2);
}

#[test]
fn socket_data_and_disconnect_wake_without_owner_polling() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let mut writer = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    let (socket, _) = listener.accept().unwrap();
    let signal = Signal::default();
    let (wake, awake) = mpsc::channel();
    signal.set(Some(Arc::new(move || {
        let _ = wake.send(());
    })));
    awake.recv_timeout(Duration::from_secs(2)).unwrap();
    let (sender, inbox) = mpsc::channel();
    let reading = signal.clone();
    let thread = std::thread::spawn(move || read(socket, sender, &reading));
    writer.write_all(&wire::encode(&message())).unwrap();
    awake
        .recv_timeout(Duration::from_secs(2))
        .expect("message wakes the parked owner");
    assert!(matches!(inbox.try_recv(), Ok(Received::Message(value)) if value == message()));
    drop(writer);
    awake
        .recv_timeout(Duration::from_secs(2))
        .expect("EOF wakes the parked owner");
    thread.join().unwrap();
    signal.set(None);
}
