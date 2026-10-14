//! Notify the lane owner after enqueueing wire data, without owning its event loop.
use super::WakeHook;
use bunny_ui_preview::wire::{Reader, Received};
use std::io::Read;
use std::sync::{Arc, Mutex, PoisonError, mpsc::Sender};

#[derive(Clone, Default)]
pub(super) struct Signal(Arc<Mutex<Option<WakeHook>>>);
impl Signal {
    pub(super) fn current(&self) -> Option<WakeHook> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    pub(super) fn set(&self, hook: Option<WakeHook>) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = hook;
        // Registration itself wakes: messages may precede installation.
        self.notify();
    }
    pub(super) fn notify(&self) {
        if let Some(hook) = self.current() {
            hook();
        }
    }
}
struct Closed<'a>(&'a Signal);
impl Drop for Closed<'_> {
    fn drop(&mut self) {
        self.0.notify();
    }
}

pub(super) fn read(mut reading: impl Read, sender: &Sender<Received>, signal: &Signal) {
    let _closed = Closed(signal);
    let mut reader = Reader::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = match reading.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        reader.feed(&buf[..read]);
        loop {
            match reader.pull() {
                Ok(Some(received)) => {
                    if sender.send(received).is_err() {
                        return;
                    }
                    signal.notify();
                }
                Ok(None) => break,
                Err(_) => return,
            }
        }
    }
}
