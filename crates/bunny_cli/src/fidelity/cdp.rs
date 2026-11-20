//! The Chrome debugging protocol over one web socket.
//!
//! A command goes out with its number, the answer with the same number
//! comes back, and the events that arrive in between wait in a queue for
//! whoever asks.

use std::collections::{BTreeSet, VecDeque};
use std::time::{Duration, Instant};

use super::ws::Socket;
use crate::error::{Error, Result};
use crate::json::{self, Value};

/// A JSON object of `members`.
#[must_use]
pub fn object(members: &[(&str, Value)]) -> Value {
    Value::Object(members.iter().map(|(name, value)| ((*name).to_owned(), value.clone())).collect())
}

#[must_use]
pub fn text(value: &str) -> Value {
    Value::String(value.to_owned())
}

#[must_use]
pub fn number(value: impl std::fmt::Display) -> Value {
    Value::Number(value.to_string())
}

/// One connection: to the browser, or to one page.
pub struct Session {
    socket: Socket,
    next: u64,
    events: VecDeque<Value>,
    /// The requests the page sent that have not finished — the network
    /// is quiet when this is empty (needs `Network.enable`).
    pending: BTreeSet<String>,
}

impl Session {
    /// Connects to the debugging socket at `url`.
    ///
    /// # Errors
    ///
    /// The socket refused, or its handshake failed.
    pub fn open(url: &str) -> Result<Self> {
        let socket = Socket::connect(url).map_err(|error| Error::new(format!("the browser's debugging socket: {error}")))?;
        Ok(Self { socket, next: 0, events: VecDeque::new(), pending: BTreeSet::new() })
    }

    /// The next message, waiting at most until `deadline`.
    fn read(&mut self, deadline: Instant) -> Result<Option<Value>> {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(None);
        }
        self.socket.set_timeout(Some(left)).map_err(|error| Error::new(error.to_string()))?;
        match self.socket.receive() {
            Ok(Some(text)) => json::parse(&text).map(Some).map_err(Error::new),
            Ok(None) => Err(Error::new("the browser closed the debugging socket")),
            Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => Ok(None),
            Err(error) => Err(Error::new(format!("the debugging socket: {error}"))),
        }
    }

    /// An event noted on its way past: the network's ledger is kept here.
    fn note(&mut self, event: Value) {
        let id = || event.str_at(&["params", "requestId"]).map(str::to_owned);
        match event.str_at(&["method"]) {
            Some("Network.requestWillBeSent") => {
                if let Some(id) = id() {
                    self.pending.insert(id);
                }
            }
            Some("Network.loadingFinished" | "Network.loadingFailed") => {
                if let Some(id) = id() {
                    self.pending.remove(&id);
                }
            }
            _ => {}
        }
        self.events.push_back(event);
    }

    /// Sends `method` and waits a minute for its answer's `result`.
    ///
    /// # Errors
    ///
    /// The browser's own error for the command, no answer in time, or a
    /// broken socket.
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value> {
        self.call_within(method, params, Duration::from_secs(60))
    }

    /// [`Session::call`] with a deadline of its own.
    ///
    /// # Errors
    ///
    /// As [`Session::call`].
    pub fn call_within(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        self.next += 1;
        let id = self.next;
        let message = object(&[("id", number(id)), ("method", text(method)), ("params", params)]);
        self.socket.send(&json::write(&message)).map_err(|error| Error::new(format!("the debugging socket: {error}")))?;
        let deadline = Instant::now() + timeout;
        loop {
            let Some(message) = self.read(deadline)? else {
                return Err(Error::new(format!("the browser did not answer {method} within {}s", timeout.as_secs())));
            };
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                if let Some(error) = message.get("error") {
                    return Err(Error::new(format!("{method}: {}", error.str_at(&["message"]).unwrap_or("failed"))));
                }
                return Ok(message.get("result").cloned().unwrap_or(Value::Null));
            }
            if message.get("method").is_some() {
                self.note(message);
            }
        }
    }

    /// Waits for the event `method`, taking it out of the queue.
    ///
    /// # Errors
    ///
    /// No such event in time, or a broken socket.
    pub fn wait_for(&mut self, method: &str, timeout: Duration) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(at) = self.events.iter().position(|event| event.str_at(&["method"]) == Some(method)) {
                return Ok(self.events.remove(at).unwrap_or(Value::Null));
            }
            match self.read(deadline)? {
                Some(message) if message.get("method").is_some() => self.note(message),
                Some(_) => {}
                None => return Err(Error::new(format!("no {method} within {}s", timeout.as_secs()))),
            }
        }
    }

    /// Reads what already arrived, without waiting long.
    ///
    /// # Errors
    ///
    /// A broken socket.
    pub fn pump(&mut self) -> Result<()> {
        let deadline = Instant::now() + Duration::from_millis(5);
        while let Some(message) = self.read(deadline)? {
            if message.get("method").is_some() {
                self.note(message);
            }
        }
        Ok(())
    }

    /// Requests sent and not finished.
    #[must_use]
    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }

    /// Takes every queued event named `method`.
    pub fn take(&mut self, method: &str) -> Vec<Value> {
        let mut taken = Vec::new();
        self.events.retain(|event| {
            let named = event.str_at(&["method"]) == Some(method);
            if named {
                taken.push(event.clone());
            }
            !named
        });
        taken
    }

    /// Evaluates `expression` in the page, awaiting a promise, and answers
    /// its value.
    ///
    /// # Errors
    ///
    /// What the page threw, in its words, or the session's own failure.
    pub fn evaluate(&mut self, expression: &str) -> Result<Value> {
        let result = self.call(
            "Runtime.evaluate",
            object(&[("expression", text(expression)), ("awaitPromise", Value::Bool(true)), ("returnByValue", Value::Bool(true))]),
        )?;
        if let Some(details) = result.get("exceptionDetails") {
            let words = details
                .str_at(&["exception", "description"])
                .or_else(|| details.str_at(&["text"]))
                .unwrap_or("an exception");
            return Err(Error::new(format!("the page threw: {words}")));
        }
        Ok(result.path(&["result", "value"]).cloned().unwrap_or(Value::Null))
    }
}
