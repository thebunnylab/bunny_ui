//! Owner-attached `AppKit` sheets that never enter a nested application loop.
//!
//! ## Wiring
//! The safe shell reserves a core request and queues it here. Presentation and
//! teardown run after dispatch borrows end. `AppKit` copies a POD-token block;
//! its callback queues a receipt rather than waking application code inside an
//! Objective-C completion. Native objects remain retained until that receipt.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;
use std::rc::Rc;

use bunny_ui::window::{
    ConfirmationError, ConfirmationRequest, ConfirmationResponse, WindowError, WindowVisibility,
};
use bunny_ui::words::{Word, Words};

use super::{Id, Sel, class, msg_id, msg_void, msg_void_id, ns_string, sel};

#[allow(
    clashing_extern_declarations,
    reason = "Each Objective-C selector uses its concrete ABI"
)]
#[link(name = "objc")]
unsafe extern "C" {
    #[link_name = "objc_msgSend"]
    fn add_button(alert: Id, selector: Sel, title: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn begin_sheet(alert: Id, selector: Sel, owner: Id, block: *const SheetBlock);
    #[link_name = "objc_msgSend"]
    fn end_sheet(owner: Id, selector: Sel, sheet: Id, response: isize);
    #[link_name = "objc_msgSend"]
    fn monitor_events(class: Id, selector: Sel, mask: usize, block: *const KeyBlock) -> Id;
    #[link_name = "objc_msgSend"]
    fn key_code(event: Id, selector: Sel) -> u16;
    #[link_name = "objc_msgSend"]
    fn event_flags(event: Id, selector: Sel) -> usize;
}

/// A main-thread Objective-C retain; never shared across threads.
struct Retained(Id);

impl Retained {
    fn retain(object: Id) -> Self {
        unsafe { msg_id(object, sel("retain")) };
        Self(object)
    }
}

impl Drop for Retained {
    fn drop(&mut self) {
        unsafe { msg_void(self.0, sel("release")) };
    }
}

struct Pending {
    owner: Retained,
    request: ConfirmationRequest,
    yes: String,
    no: String,
}

struct Presentation {
    _monitor: Monitor,
    owner: Retained,
    alert: Retained,
    request: ConfirmationRequest,
}

/// Local to this process, removed before releasing the sheet's native objects.
struct Monitor(Retained);

impl Drop for Monitor {
    fn drop(&mut self) {
        unsafe { msg_void_id(class("NSEvent"), sel("removeMonitor:"), self.0.0) };
    }
}

enum Operation {
    Present(Pending),
    Cancel(usize),
    Completed(usize, isize),
}

thread_local! {
    static QUEUE: RefCell<VecDeque<Operation>> = const { RefCell::new(VecDeque::new()) };
    static ACTIVE: RefCell<HashMap<usize, Rc<Presentation>>> = RefCell::default();
    static DRAINING: Cell<bool> = const { Cell::new(false) };
}

fn enqueue(operation: Operation) {
    QUEUE.with(|queue| queue.borrow_mut().push_back(operation));
    super::wake_from_any_thread();
}

pub fn available(owner: usize) -> Result<(), ConfirmationError> {
    if super::window_visibility(owner)? == WindowVisibility::Hidden {
        return Err(WindowError::HiddenWindow.into());
    }
    if super::MODAL_WINDOWS.with(|state| state.borrow().scopes.top(owner).is_some())
        || !unsafe { msg_id(owner as Id, sel("attachedSheet")) }.is_null()
    {
        return Err(ConfirmationError::AlreadyOpen);
    }
    Ok(())
}

pub fn queue(owner: usize, request: ConfirmationRequest, words: &Words) {
    let token = request.token();
    if request.on_cancel(move || enqueue(Operation::Cancel(token))) {
        enqueue(Operation::Present(Pending {
            owner: Retained::retain(owner as Id),
            request,
            yes: words.get(Word::Yes).into_owned(),
            no: words.get(Word::No).into_owned(),
        }));
    }
}

struct Draining;
impl Drop for Draining {
    fn drop(&mut self) {
        DRAINING.set(false);
    }
}

pub(super) fn drain() {
    if super::DISPATCHING.with(Cell::get)
        || super::LENDING.with(Cell::get)
        || DRAINING.with(|busy| busy.replace(true))
    {
        return;
    }
    let _draining = Draining;
    loop {
        let operation = QUEUE.with(|queue| queue.borrow_mut().pop_front());
        match operation {
            Some(Operation::Present(pending)) => present(pending),
            Some(Operation::Cancel(token)) => cancel(token),
            Some(Operation::Completed(token, response)) => complete(token, response),
            None => break,
        }
    }
}

fn present(pending: Pending) {
    let request = pending.request.clone();
    if !request.can_present() {
        let _ = request.complete(Ok(ConfirmationResponse::Dismissed));
        return;
    }
    if let Err(error) = available(pending.owner.0 as usize) {
        let _ = request.complete(Err(error));
        return;
    }
    let Some(spec) = request.spec() else { return };
    let alert = unsafe { msg_id(msg_id(class("NSAlert"), sel("alloc")), sel("init")) };
    if alert.is_null() {
        let _ = request.complete(Err(WindowError::NativeFailure {
            operation: "NSAlert.init",
            code: 0,
        }
        .into()));
        return;
    }
    let alert = Retained(alert);
    let key_block = KeyBlock {
        isa: (&raw const _NSConcreteStackBlock).cast(),
        flags: 0,
        reserved: 0,
        invoke: sheet_key,
        descriptor: &raw const KEY_DESCRIPTOR,
        token: request.token(),
    };
    let monitor = unsafe {
        monitor_events(
            class("NSEvent"),
            sel("addLocalMonitorForEventsMatchingMask:handler:"),
            1 << 10,
            &raw const key_block,
        )
    };
    if monitor.is_null() {
        let _ = request.complete(Err(WindowError::NativeFailure {
            operation: "NSEvent.addLocalMonitor",
            code: 0,
        }
        .into()));
        return;
    }
    let presentation = Rc::new(Presentation {
        _monitor: Monitor(Retained::retain(monitor)),
        owner: pending.owner,
        alert,
        request,
    });
    let alert = presentation.alert.0;
    unsafe {
        msg_void_id(alert, sel("setMessageText:"), ns_string(spec.question()));
        msg_void_id(alert, sel("setInformativeText:"), ns_string(spec.detail()));
        let yes = add_button(alert, sel("addButtonWithTitle:"), ns_string(&pending.yes));
        let no = add_button(alert, sel("addButtonWithTitle:"), ns_string(&pending.no));
        // Setting Return establishes the default. The local event monitor keeps
        // Return, keypad Enter and Escape on that answer for this sheet alone.
        let _window = msg_id(alert, sel("window"));
        msg_void_id(yes, sel("setKeyEquivalent:"), ns_string(""));
        msg_void_id(no, sel("setKeyEquivalent:"), ns_string("\r"));
    }
    let token = presentation.request.token();
    ACTIVE.with(|active| active.borrow_mut().insert(token, Rc::clone(&presentation)));
    let block = SheetBlock {
        isa: (&raw const _NSConcreteStackBlock).cast(),
        flags: 0,
        reserved: 0,
        invoke: sheet_completed,
        descriptor: &raw const DESCRIPTOR,
        token,
    };
    // NSAlert copies this block before returning; its only capture is a token.
    // The local Rc also protects both native objects if completion is immediate.
    unsafe {
        begin_sheet(
            alert,
            sel("beginSheetModalForWindow:completionHandler:"),
            presentation.owner.0,
            &raw const block,
        );
    }
}

fn cancel(token: usize) {
    let presentation = ACTIVE.with(|active| active.borrow().get(&token).cloned());
    if let Some(presentation) = presentation {
        unsafe {
            let sheet = msg_id(presentation.alert.0, sel("window"));
            end_sheet(
                presentation.owner.0,
                sel("endSheet:returnCode:"),
                sheet,
                -1000,
            );
        }
    }
}

fn complete(token: usize, response: isize) {
    let presentation = ACTIVE.with(|active| active.borrow_mut().remove(&token));
    if let Some(presentation) = presentation {
        let request = presentation.request.clone();
        unsafe {
            msg_void_id(
                msg_id(presentation.alert.0, sel("window")),
                sel("orderOut:"),
                std::ptr::null_mut(),
            );
        }
        drop(presentation);
        let _ = request.complete(Ok(match response {
            1000 => ConfirmationResponse::Yes,
            1001 => ConfirmationResponse::No,
            _ => ConfirmationResponse::Dismissed,
        }));
    }
}

// A token-only block follows the existing Apple snapshot boundary. No Rust
// object is byte-copied; AppKit retains its heap copy until sheet completion.
#[repr(C)]
struct SheetBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: extern "C" fn(*mut Self, isize),
    descriptor: *const BlockDescriptor,
    token: usize,
}

#[repr(C)]
struct BlockDescriptor {
    reserved: usize,
    size: usize,
}

static DESCRIPTOR: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<SheetBlock>(),
};

#[link(name = "System", kind = "dylib")]
unsafe extern "C" {
    static _NSConcreteStackBlock: [*const c_void; 32];
}

extern "C" fn sheet_completed(block: *mut SheetBlock, response: isize) {
    // AppKit invokes its copied block on the same main thread used to present.
    let token = unsafe { (*block).token };
    enqueue(Operation::Completed(token, response));
}

#[repr(C)]
struct KeyBlock {
    isa: *const c_void,
    flags: i32,
    reserved: i32,
    invoke: extern "C" fn(*mut Self, Id) -> Id,
    descriptor: *const BlockDescriptor,
    token: usize,
}

static KEY_DESCRIPTOR: BlockDescriptor = BlockDescriptor {
    reserved: 0,
    size: std::mem::size_of::<KeyBlock>(),
};

extern "C" fn sheet_key(block: *mut KeyBlock, event: Id) -> Id {
    // NSEvent copies the token-only block and calls it on the presenting thread.
    let token = unsafe { (*block).token };
    let presentation = ACTIVE.with(|active| active.borrow().get(&token).cloned());
    let Some(presentation) = presentation else {
        return event;
    };
    unsafe {
        let sheet = msg_id(presentation.alert.0, sel("window"));
        let modifiers = event_flags(event, sel("modifierFlags")) & (0b1111 << 17);
        if msg_id(event, sel("window")) == sheet
            && modifiers == 0
            && matches!(key_code(event, sel("keyCode")), 36 | 53 | 76)
        {
            end_sheet(
                presentation.owner.0,
                sel("endSheet:returnCode:"),
                sheet,
                1001,
            );
            std::ptr::null_mut()
        } else {
            event
        }
    }
}
