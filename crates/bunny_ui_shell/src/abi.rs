//! The memory half of [`block!`](crate::block): the canonical ABI's
//! allocator, the input taken back as a `Vec`, and the answer handed out
//! as a `(ptr, len)` pair until the host releases it. Only the macro
//! calls these.

use std::alloc::{self, Layout};

use bunny_ui_core::view::View;

use crate::guest::Guest;

/// A guest with its view's type forgotten — what the macro keeps.
pub struct Erased(Box<dyn FnMut(&[u8]) -> Vec<u8>>);

impl Erased {
    pub fn new<V: View + 'static>(mut guest: Guest<V>) -> Erased {
        Erased(Box::new(move |input| guest.call(input)))
    }

    pub fn call(&mut self, input: &[u8]) -> Vec<u8> {
        (self.0)(input)
    }
}

/// # Safety
/// `old` is null, or a block of `old_len` bytes at `align` this function
/// returned.
pub unsafe fn realloc(old: *mut u8, old_len: usize, align: usize, len: usize) -> *mut u8 {
    let align = align.max(1);
    if len == 0 {
        if !old.is_null() && old_len != 0 {
            // SAFETY: the caller's promise about `old`
            unsafe { alloc::dealloc(old, Layout::from_size_align_unchecked(old_len, align)) };
        }
        return align as *mut u8;
    }
    let Ok(layout) = Layout::from_size_align(len, align) else { alloc::handle_alloc_error(Layout::new::<u8>()) };
    // SAFETY: `layout` has a nonzero size; `old` is the caller's promise
    let fresh = unsafe {
        if old.is_null() || old_len == 0 {
            alloc::alloc(layout)
        } else {
            alloc::realloc(old, Layout::from_size_align_unchecked(old_len, align), len)
        }
    };
    if fresh.is_null() {
        alloc::handle_alloc_error(layout);
    }
    fresh
}

/// # Safety
/// `input` is `len` bytes from [`realloc`] at alignment 1.
pub unsafe fn take_input(input: *mut u8, len: usize) -> Vec<u8> {
    if len == 0 || input.is_null() {
        return Vec::new();
    }
    // SAFETY: allocated by `realloc` with the layout a `Vec<u8>` of
    // capacity `len` has
    unsafe { Vec::from_raw_parts(input, len, len) }
}

/// The answer, out until [`release_answer`]: the returned pointer holds
/// its `(ptr, len)`.
pub fn give_answer(answer: Vec<u8>) -> *mut u8 {
    let answer = Box::into_raw(answer.into_boxed_slice());
    let len = answer.len();
    Box::into_raw(Box::new([answer as *mut u8 as usize, len])) as *mut u8
}

/// # Safety
/// `ret` came from [`give_answer`] and was not released.
pub unsafe fn release_answer(ret: *mut u8) {
    // SAFETY: the caller's promise; both boxes were made by `give_answer`
    unsafe {
        let pair = Box::from_raw(ret as *mut [usize; 2]);
        let slice = std::ptr::slice_from_raw_parts_mut(pair[0] as *mut u8, pair[1]);
        drop(Box::from_raw(slice));
    }
}
