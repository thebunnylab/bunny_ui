//! The local executor — the engine's own task queue.
//!
//! One queue, on the thread that owns the views. A task is a future that
//! may touch `State`, so it never leaves that thread; what crosses a
//! thread boundary is a SIGNAL, never data. A [`Sender`] marks its task
//! ready and asks the shell for one more turn through the wake hook.
//!
//! The framework does no I/O. The app opens the thread, the process or
//! the fetch and hands results over a [`channel`]; this module is the
//! queue and the signal between the two.
//!
//! Nothing here is unsafe: the waker comes from `Arc<impl Wake>` and the
//! future rides a `Pin<Box<…>>`.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Wake, Waker};

/// Identity of a spawned task.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct TaskId(u64);

/// A lock that survives a panicking worker: the data behind it is a
/// queue and a flag, and neither breaks an invariant halfway.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

// MARK: - The signal that crosses threads

/// What senders and wakers hold. It carries no task and no value — a
/// list of ids to poll and the shell's way to ask for a turn.
#[derive(Default)]
struct Shared {
    ready: Mutex<Vec<TaskId>>,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl Shared {
    /// Marks a task for the next poll and asks for a turn. Called from
    /// ANY thread; the shell's hook only signals its run loop, which is
    /// why a wake during a frame lands on the next turn instead of
    /// re-entering this one.
    fn mark(&self, id: TaskId) {
        {
            let mut ready = lock(&self.ready);
            if ready.contains(&id) {
                // already queued: the hook fired when it was queued
                return;
            }
            ready.push(id);
        }
        let hook = lock(&self.wake).clone();
        if let Some(hook) = hook {
            hook();
        }
    }

    fn take_ready(&self) -> Vec<TaskId> {
        std::mem::take(&mut *lock(&self.ready))
    }

    fn has_ready(&self) -> bool {
        !lock(&self.ready).is_empty()
    }
}

struct TaskWaker {
    id: TaskId,
    shared: Arc<Shared>,
}

impl Wake for TaskWaker {
    fn wake(self: Arc<Self>) {
        self.shared.mark(self.id);
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.shared.mark(self.id);
    }
}

// MARK: - The queue

type BoxedTask = Pin<Box<dyn Future<Output = ()>>>;

#[derive(Default)]
struct Executor {
    /// `None` in the slot = the task is being polled right now. The
    /// entry itself is the proof it is alive: a cancel removes it, and
    /// the poll that finishes finds nothing to put back.
    tasks: HashMap<TaskId, Option<BoxedTask>>,
    next: u64,
    shared: Arc<Shared>,
}

thread_local! {
    static EXECUTOR: RefCell<Executor> = RefCell::new(Executor::default());
}

/// Puts a future on the queue. It runs on the next [`poll_ready`], never
/// inside this call — a spawn from a body or an event handler never
/// re-enters the pass that made it.
///
/// The returned [`Spawned`] OWNS the task: drop it and the task is
/// cancelled. Call [`Spawned::detach`] for a task that must finish on
/// its own.
#[must_use = "a dropped Spawned cancels its task — call detach() to let it run alone"]
pub fn spawn(future: impl Future<Output = ()> + 'static) -> Spawned {
    let (id, shared) = EXECUTOR.with(|executor| {
        let mut executor = executor.borrow_mut();
        executor.next += 1;
        let id = TaskId(executor.next);
        executor.tasks.insert(id, Some(Box::pin(future)));
        (id, Arc::clone(&executor.shared))
    });
    // a task that has never been polled is ready by definition
    shared.mark(id);
    Spawned { id: Some(id) }
}

/// Polls every task marked ready and answers whether any of them ran.
/// Tasks woken DURING this drain wait for the next call — the caller's
/// loop (or the shell's next turn) is what bounds the work.
pub fn poll_ready() -> bool {
    let (ready, shared) = EXECUTOR.with(|executor| {
        let executor = executor.borrow();
        (executor.shared.take_ready(), Arc::clone(&executor.shared))
    });
    let mut ran = false;
    for id in ready {
        // out of the map while it runs: the future may spawn, cancel or
        // send, and all of those come back through this same registry
        let taken = EXECUTOR
            .with(|executor| executor.borrow_mut().tasks.get_mut(&id).and_then(Option::take));
        let Some(mut future) = taken else { continue };
        ran = true;
        let waker = Waker::from(Arc::new(TaskWaker { id, shared: Arc::clone(&shared) }));
        let finished = future.as_mut().poll(&mut Context::from_waker(&waker)).is_ready();
        // the entry is GONE when the task was cancelled while it ran;
        // either way the future drops outside the borrow
        let dropped = EXECUTOR.with(|executor| {
            let mut executor = executor.borrow_mut();
            if finished {
                executor.tasks.remove(&id);
                Some(future)
            } else {
                match executor.tasks.get_mut(&id) {
                    Some(slot) => {
                        *slot = Some(future);
                        None
                    }
                    None => Some(future),
                }
            }
        });
        drop(dropped);
    }
    ran
}

/// Is there anything waiting for the next poll? The frame loop asks
/// before declaring the scene settled.
pub fn has_ready() -> bool {
    EXECUTOR.with(|executor| executor.borrow().shared.has_ready())
}

/// How many tasks are alive — instrumentation for tests.
pub fn pending() -> usize {
    EXECUTOR.with(|executor| executor.borrow().tasks.len())
}

/// Installs the shell's way to ask for a turn: a task woken from a
/// worker thread (or a browser callback) calls this, and the shell
/// answers with the present it already knows how to do.
pub fn set_wake_hook(hook: Arc<dyn Fn() + Send + Sync>) {
    EXECUTOR.with(|executor| {
        *lock(&executor.borrow().shared.wake) = Some(hook);
    });
}

/// Ends a task now. The future drops where it stands, which is what
/// tells whoever waits on the other side of a [`channel`] to stop.
pub fn cancel(id: TaskId) {
    // `try_with`: a handle held in a thread-local slot is dropped while
    // the thread tears its locals down, and the queue may already be
    // gone by then — there is nothing left to cancel in that case.
    // Out first, drop after: the future's own Drop may reach back here.
    let task = EXECUTOR
        .try_with(|executor| executor.borrow_mut().tasks.remove(&id))
        .ok()
        .flatten();
    drop(task);
}

/// The handle of a spawned task. Dropping it cancels; that is what makes
/// a task tied to a view die with the view.
pub struct Spawned {
    id: Option<TaskId>,
}

impl Spawned {
    pub fn id(&self) -> Option<TaskId> {
        self.id
    }

    /// Lets the task finish on its own — nothing will cancel it.
    pub fn detach(mut self) {
        self.id = None;
    }

    /// Ends the task now (what the drop does, spelled out).
    pub fn cancel(self) {}
}

impl Drop for Spawned {
    fn drop(&mut self) {
        if let Some(id) = self.id.take() {
            cancel(id);
        }
    }
}

impl std::fmt::Debug for Spawned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.id {
            Some(TaskId(id)) => write!(f, "Spawned({id})"),
            None => f.write_str("Spawned(detached)"),
        }
    }
}

// MARK: - The channel: one value or a whole stream

struct Chan<T> {
    queue: VecDeque<T>,
    /// The waker of the task parked in `recv`.
    waker: Option<Waker>,
    senders: usize,
    receiver_alive: bool,
}

/// The writing end. It is `Send`, so it is what crosses to a worker
/// thread, and it is `Clone`, so several producers can feed one task.
pub struct Sender<T> {
    chan: Arc<Mutex<Chan<T>>>,
}

/// The reading end, awaited from inside a task.
pub struct Receiver<T> {
    chan: Arc<Mutex<Chan<T>>>,
}

/// The value came back because the reader is gone — the task was
/// cancelled, or its view died. Whoever holds the sender stops working.
#[derive(Debug, PartialEq)]
pub struct SendError<T>(pub T);

impl<T> std::fmt::Display for SendError<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the receiver is gone")
    }
}

/// A channel from anywhere to one task. Send once for a result; send
/// again for a stream — `while let Some(item) = rx.recv().await` reads
/// until the last sender hangs up.
pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let chan = Arc::new(Mutex::new(Chan {
        queue: VecDeque::new(),
        waker: None,
        senders: 1,
        receiver_alive: true,
    }));
    (Sender { chan: Arc::clone(&chan) }, Receiver { chan })
}

impl<T> Sender<T> {
    /// Hands a value over and wakes the task waiting for it. `Err` means
    /// nobody is reading any more: stop producing.
    pub fn send(&self, value: T) -> Result<(), SendError<T>> {
        let waker = {
            let mut chan = lock(&self.chan);
            if !chan.receiver_alive {
                return Err(SendError(value));
            }
            chan.queue.push_back(value);
            chan.waker.take()
        };
        // outside the lock: waking runs the shell's hook
        if let Some(waker) = waker {
            waker.wake();
        }
        Ok(())
    }

    /// Is anyone still reading? A long job asks between steps.
    pub fn is_connected(&self) -> bool {
        lock(&self.chan).receiver_alive
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        lock(&self.chan).senders += 1;
        Sender { chan: Arc::clone(&self.chan) }
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        // the last sender leaving ENDS the stream: the parked `recv`
        // wakes to answer `None`
        let waker = {
            let mut chan = lock(&self.chan);
            chan.senders -= 1;
            if chan.senders == 0 { chan.waker.take() } else { None }
        };
        if let Some(waker) = waker {
            waker.wake();
        }
    }
}

impl<T> Receiver<T> {
    /// Waits for the next value. `None` = every sender hung up.
    pub fn recv(&self) -> Recv<'_, T> {
        Recv { receiver: self }
    }

    /// Takes a value if one is already there, without waiting.
    pub fn try_recv(&self) -> Option<T> {
        lock(&self.chan).queue.pop_front()
    }
}

impl<T> Drop for Receiver<T> {
    fn drop(&mut self) {
        lock(&self.chan).receiver_alive = false;
    }
}

/// The future of [`Receiver::recv`].
pub struct Recv<'a, T> {
    receiver: &'a Receiver<T>,
}

impl<T> Future for Recv<'_, T> {
    type Output = Option<T>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<T>> {
        let mut chan = lock(&self.receiver.chan);
        if let Some(value) = chan.queue.pop_front() {
            return Poll::Ready(Some(value));
        }
        if chan.senders == 0 {
            return Poll::Ready(None);
        }
        chan.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Every test shares the thread-local queue — this clears what an
    /// earlier one left behind.
    fn fresh() {
        EXECUTOR.with(|executor| {
            let mut executor = executor.borrow_mut();
            executor.tasks.clear();
            executor.shared = Arc::default();
        });
    }

    #[test]
    fn a_spawned_future_runs_on_the_next_poll_and_not_before() {
        fresh();
        let landed = Rc::new(Cell::new(false));
        let task = {
            let landed = Rc::clone(&landed);
            spawn(async move { landed.set(true) })
        };
        assert!(!landed.get(), "the spawn itself never runs the body");
        assert_eq!(pending(), 1);

        assert!(poll_ready());
        assert!(landed.get());
        assert_eq!(pending(), 0, "a finished task leaves the queue");
        assert!(!poll_ready(), "nothing left to run");
        task.detach();
    }

    #[test]
    fn a_value_crosses_from_a_real_thread_and_the_hook_asks_for_a_turn() {
        fresh();
        let turns = Arc::new(AtomicUsize::new(0));
        {
            let turns = Arc::clone(&turns);
            set_wake_hook(Arc::new(move || {
                turns.fetch_add(1, Ordering::SeqCst);
            }));
        }
        let (sender, receiver) = channel::<u32>();
        let got = Rc::new(Cell::new(0));
        let task = {
            let got = Rc::clone(&got);
            spawn(async move {
                if let Some(value) = receiver.recv().await {
                    got.set(value);
                }
            })
        };
        poll_ready();
        assert_eq!(got.get(), 0, "the task parks on the empty channel");
        let asked = turns.load(Ordering::SeqCst);

        std::thread::spawn(move || sender.send(7).expect("the task is reading"))
            .join()
            .expect("the worker");
        assert!(
            turns.load(Ordering::SeqCst) > asked,
            "the send asked the shell for a turn"
        );
        assert!(has_ready(), "the task is queued for the next poll");

        poll_ready();
        assert_eq!(got.get(), 7);
        task.detach();
    }

    #[test]
    fn a_stream_reads_until_the_last_sender_hangs_up() {
        fresh();
        let (sender, receiver) = channel::<u32>();
        let lines = Rc::new(RefCell::new(Vec::new()));
        let task = {
            let lines = Rc::clone(&lines);
            spawn(async move {
                while let Some(value) = receiver.recv().await {
                    lines.borrow_mut().push(value);
                }
                lines.borrow_mut().push(99);
            })
        };
        poll_ready();

        std::thread::spawn(move || {
            for value in 1..=3 {
                sender.send(value).expect("the task is reading");
            }
            // the sender leaving is what ends the stream
        })
        .join()
        .expect("the worker");

        // one poll per value plus the one that sees the hang-up
        for _ in 0..5 {
            poll_ready();
        }
        assert_eq!(*lines.borrow(), vec![1, 2, 3, 99]);
        assert_eq!(pending(), 0, "the task finished with the stream");
        task.detach();
    }

    #[test]
    fn dropping_the_handle_cancels_and_the_sender_learns() {
        fresh();
        let (sender, receiver) = channel::<u32>();
        let task = spawn(async move {
            let _ = receiver.recv().await;
        });
        poll_ready();
        assert!(sender.is_connected());

        drop(task);
        assert_eq!(pending(), 0, "the cancel took it off the queue");
        assert_eq!(
            sender.send(1),
            Err(SendError(1)),
            "the value comes back: nobody is reading"
        );
        assert!(!sender.is_connected());
    }

    #[test]
    fn waking_a_task_that_is_gone_does_nothing() {
        fresh();
        let (sender, receiver) = channel::<u32>();
        let task = spawn(async move {
            let _ = receiver.recv().await;
        });
        poll_ready();
        let sender_from_afar = sender.clone();
        drop(task);
        // the queue marks an id nobody owns any more
        let _ = sender_from_afar.send(1);
        assert!(!poll_ready(), "the poll finds nothing to run");
        assert_eq!(pending(), 0);
    }

    #[test]
    fn a_task_may_spawn_another_from_inside_its_own_poll() {
        fresh();
        let depth = Rc::new(Cell::new(0));
        let inner_depth = Rc::clone(&depth);
        let task = spawn(async move {
            inner_depth.set(1);
            let deeper = Rc::clone(&inner_depth);
            spawn(async move { deeper.set(2) }).detach();
        });
        poll_ready();
        assert_eq!(depth.get(), 1, "the child waits for the next poll");
        poll_ready();
        assert_eq!(depth.get(), 2);
        task.detach();
    }
}
