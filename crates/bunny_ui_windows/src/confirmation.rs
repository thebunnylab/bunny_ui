//! Run owner-bound message boxes after the UI dispatcher releases its borrows.
//!
//! ## Wiring
//! One native loop runs at a time; requests for other owners remain queued.
//! Thread-local CBT and message-filter hooks identify only that loop's dialog.
//! Every hook is removed before completing its weak response endpoint.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use bunny_ui::window::{
    ConfirmationError, ConfirmationRequest, ConfirmationResponse, WindowError, WindowVisibility,
};

use super::{GetWindow, Hwnd, Msg, PostMessageW, wide};

type HookProc = unsafe extern "system" fn(i32, usize, isize) -> isize;

#[link(name = "user32", kind = "raw-dylib")]
unsafe extern "system" {
    fn MessageBoxW(owner: Hwnd, text: *const u16, caption: *const u16, flags: u32) -> i32;
    fn SetWindowsHookExW(kind: i32, callback: HookProc, module: isize, thread: u32) -> isize;
    fn UnhookWindowsHookEx(hook: isize) -> i32;
    fn CallNextHookEx(hook: isize, code: i32, wparam: usize, lparam: isize) -> isize;
    fn GetClassNameW(window: Hwnd, name: *mut u16, capacity: i32) -> i32;
    fn GetDlgItem(window: Hwnd, id: i32) -> Hwnd;
    fn IsWindowEnabled(window: Hwnd) -> i32;
    fn IsChild(parent: Hwnd, child: Hwnd) -> i32;
}

#[link(name = "kernel32", kind = "raw-dylib")]
unsafe extern "system" {
    fn GetCurrentThreadId() -> u32;
    fn GetLastError() -> u32;
}

struct Pending {
    owner: Hwnd,
    request: ConfirmationRequest,
}

struct Active {
    owner: Hwnd,
    dialog: Cell<Option<Hwnd>>,
    request: ConfirmationRequest,
}

thread_local! {
    static QUEUE: RefCell<VecDeque<Pending>> = const { RefCell::new(VecDeque::new()) };
    static CANCELLED: RefCell<VecDeque<ConfirmationRequest>> = const { RefCell::new(VecDeque::new()) };
    static ACTIVE: RefCell<Option<Rc<Active>>> = const { RefCell::new(None) };
    static DRAINING: Cell<bool> = const { Cell::new(false) };
    static COMPLETING: Cell<bool> = const { Cell::new(false) };
    static DISPATCH_DEPTH: Cell<usize> = const { Cell::new(0) };
}

/// Also covers a slot's first frame, which bypasses the global dispatcher.
pub struct DispatchGuard;

pub fn defer() -> DispatchGuard {
    DISPATCH_DEPTH.set(DISPATCH_DEPTH.get() + 1);
    DispatchGuard
}

impl Drop for DispatchGuard {
    fn drop(&mut self) {
        DISPATCH_DEPTH.set(DISPATCH_DEPTH.get() - 1);
    }
}

struct Hook(isize);

impl Hook {
    fn install(kind: i32, callback: HookProc) -> Result<Self, ConfirmationError> {
        let hook = unsafe { SetWindowsHookExW(kind, callback, 0, GetCurrentThreadId()) };
        if hook == 0 {
            return Err(failure("SetWindowsHookExW"));
        }
        Ok(Self(hook))
    }
}

impl Drop for Hook {
    fn drop(&mut self) {
        unsafe { UnhookWindowsHookEx(self.0) };
    }
}

fn failure(operation: &'static str) -> ConfirmationError {
    WindowError::NativeFailure {
        operation,
        code: i64::from(unsafe { GetLastError() }),
    }
    .into()
}

pub fn available(owner: usize) -> Result<(), ConfirmationError> {
    if super::window_visibility(owner)? == WindowVisibility::Hidden {
        return Err(WindowError::HiddenWindow.into());
    }
    if unsafe { IsWindowEnabled(Hwnd::from_ne_bytes(owner.to_ne_bytes())) } == 0 {
        return Err(ConfirmationError::AlreadyOpen);
    }
    Ok(())
}

pub fn queue(owner: usize, request: ConfirmationRequest) {
    let token = request.token();
    if request.on_cancel(move || cancel(token)) {
        QUEUE.with(|queue| {
            queue.borrow_mut().push_back(Pending {
                owner: Hwnd::from_ne_bytes(owner.to_ne_bytes()),
                request,
            });
        });
        super::wake_from_any_thread();
    }
}

fn cancel(token: usize) {
    let pending = QUEUE.with(|queue| {
        let mut queue = queue.borrow_mut();
        let index = queue
            .iter()
            .position(|pending| pending.request.token() == token)?;
        queue.remove(index)
    });
    if let Some(pending) = pending {
        CANCELLED.with(|queue| queue.borrow_mut().push_back(pending.request));
    } else {
        let active = ACTIVE.with(|active| active.borrow().clone());
        if let Some(active) = active
            && active.request.token() == token
            && let Some(dialog) = active.dialog.get()
        {
            dismiss(dialog);
        }
    }
    super::wake_from_any_thread();
}

fn dismiss(dialog: Hwnd) {
    // The HWND is retired by HCBT_DESTROYWND before it can be reused.
    unsafe { PostMessageW(dialog, 0x0111, 7, 0) };
}

struct Draining;
impl Drop for Draining {
    fn drop(&mut self) {
        DRAINING.set(false);
    }
}

pub(super) fn drain() {
    if DISPATCH_DEPTH.get() != 0
        || COMPLETING.get()
        || !super::HANDLER.with(|handler| handler.try_borrow_mut().is_ok())
    {
        return;
    }
    COMPLETING.set(true);
    loop {
        let request = CANCELLED.with(|queue| queue.borrow_mut().pop_front());
        let Some(request) = request else { break };
        let _ = request.complete(Ok(ConfirmationResponse::Dismissed));
    }
    COMPLETING.set(false);
    if DRAINING.replace(true) {
        return;
    }
    let _draining = Draining;
    loop {
        let pending = QUEUE.with(|queue| queue.borrow_mut().pop_front());
        let Some(pending) = pending else { break };
        present(&pending);
    }
}

fn present(pending: &Pending) {
    if !pending.request.can_present() {
        let _ = pending
            .request
            .complete(Ok(ConfirmationResponse::Dismissed));
        return;
    }
    if let Err(error) = available(usize::from_ne_bytes(pending.owner.to_ne_bytes())) {
        let _ = pending.request.complete(Err(error));
        return;
    }
    let Some(spec) = pending.request.spec() else {
        return;
    };
    let text = wide(&format!("{}\n\n{}", spec.question(), spec.detail()));
    let caption = wide(spec.question());
    let result = run_box(pending, &text, &caption);
    let _ = pending.request.complete(result);
}

fn run_box(
    pending: &Pending,
    text: &[u16],
    caption: &[u16],
) -> Result<ConfirmationResponse, ConfirmationError> {
    let _cbt = Hook::install(5, cbt)?;
    let _filter = Hook::install(-1, filter)?;
    ACTIVE.with(|active| {
        active.replace(Some(Rc::new(Active {
            owner: pending.owner,
            dialog: Cell::new(None),
            request: pending.request.clone(),
        })))
    });
    // MB_YESNO | MB_ICONWARNING | MB_DEFBUTTON2. Native button labels follow
    // Windows, independently of the application's scene locale.
    let answer = unsafe {
        MessageBoxW(
            pending.owner,
            text.as_ptr(),
            caption.as_ptr(),
            0x0004 | 0x0030 | 0x0100,
        )
    };
    let result = match answer {
        0 => Err(failure("MessageBoxW")),
        6 => Ok(ConfirmationResponse::Yes),
        7 => Ok(ConfirmationResponse::No),
        _ => Ok(ConfirmationResponse::Dismissed),
    };
    ACTIVE.with(RefCell::take);
    result
}

unsafe extern "system" fn cbt(code: i32, wparam: usize, lparam: isize) -> isize {
    if code >= 0 {
        let active = ACTIVE.with(|active| active.borrow().clone());
        if let Some(active) = active {
            let window = Hwnd::from_ne_bytes(wparam.to_ne_bytes());
            if code == 4 && active.dialog.get() == Some(window) {
                active.dialog.set(None);
            } else if code == 5
                && active.dialog.get().is_none()
                && is_message_box(window, active.owner)
            {
                active.dialog.set(Some(window));
                if !active.request.can_present() {
                    dismiss(window);
                }
            }
        }
    }
    unsafe { CallNextHookEx(0, code, wparam, lparam) }
}

fn is_message_box(window: Hwnd, owner: Hwnd) -> bool {
    let mut name = [0_u16; 16];
    unsafe {
        GetWindow(window, 4) == owner
            && GetClassNameW(window, name.as_mut_ptr(), 16) == 6
            && name[..6] == [35, 51, 50, 55, 55, 48]
            && GetDlgItem(window, 6) != 0
            && GetDlgItem(window, 7) != 0
    }
}

unsafe extern "system" fn filter(code: i32, wparam: usize, lparam: isize) -> isize {
    if code == 0 && lparam != 0 {
        let message = unsafe { &*(lparam as *const Msg) };
        let dialog = ACTIVE.with(|active| {
            active
                .borrow()
                .as_ref()
                .and_then(|active| active.dialog.get())
        });
        if let Some(dialog) = dialog
            && message.message == 0x0100
            && message.wparam == 0x1b
            && (message.hwnd == dialog || unsafe { IsChild(dialog, message.hwnd) } != 0)
        {
            dismiss(dialog);
            return 1;
        }
    }
    unsafe { CallNextHookEx(0, code, wparam, lparam) }
}
