//! `bunny-ui-shell` — the Trinity shell target of bunny-ui (D723).
//!
//! A raw block is real bunny-ui, compiled for a shell whose platform is a
//! process boundary: the engine (layout and state) runs in a zero-import
//! Wasm sandbox, hands its host the **semantic scene** — never pixels —
//! and takes **events** back. Focus, keyboard, IME and accessibility stay
//! with the shell. What crosses is the **shell protocol** ([`protocol`]):
//! small, versioned, additive, and the one thing the shell keeps loadable
//! forever. A block bundles the engine it was built with, so a bunny-ui
//! change breaks nothing already shipped.
//!
//! - [`protocol`] — the messages, both directions, as bytes; the version
//!   handshake and the capabilities. Std only, always built: a shell that
//!   only speaks the protocol adds this and nothing else.
//! - `guest` (feature `engine`) — a view and its runtime behind the
//!   protocol: [`guest::Guest::call`] takes the host's bytes and answers;
//!   and [`guest::Fixture`], the preview harness — the page a block stands
//!   on, built natively, so its view draws in a preview before it is
//!   built for the shell.
//! - `host` (feature `engine`) — the test host: a scene painted with the
//!   framework's own painter, and [`host::native`], the native render a
//!   scene must equal.
//! - [`block!`] — the zero-import exports that make a crate a block.
//!
//! The contract in full is `docs/trinity-shell.md`; the first block's
//! size and start, measured, are in `docs/measures/`.

pub mod protocol;

#[cfg(feature = "engine")]
pub mod guest;
#[cfg(feature = "engine")]
pub mod host;

#[cfg(feature = "engine")]
#[doc(hidden)]
pub mod abi;

/// Makes the crate a block: the three exports a Trinity shell calls, and
/// nothing it imports.
///
/// ```ignore
/// bunny_ui_shell::block!(hello);            // `fn hello() -> impl View`
/// bunny_ui_shell::block!(page => capacity); // `fn capacity(page: Page) -> impl View`
/// ```
///
/// The `page =>` form hands the view its [`guest::Page`]: what the block's
/// signature lets it receive, read, request and fetch.
///
/// On `wasm32` the crate (a `cdylib`) exports the shape of one
/// component-model function, `call: func(input: list<u8>) -> list<u8>`
/// (`wit/shell.wit`), lowered by hand:
///
/// - `cabi_realloc(old, old_len, align, len) -> ptr` — where the host
///   writes a call's input;
/// - `call(ptr, len) -> ret` — one turn of [`guest::Guest::call`]; `ret`
///   points at the answer's `(ptr, len)`;
/// - `cabi_post_call(ret)` — the host is done reading the answer.
///
/// The guest is built on the first call. Elsewhere the macro expands to
/// nothing, so the crate still builds and tests natively.
#[macro_export]
macro_rules! block {
    (page => $make:path) => {
        $crate::block!(@exports $crate::guest::Guest::over($make));
    };
    ($make:path) => {
        $crate::block!(@exports $crate::guest::Guest::new($make()));
    };
    (@exports $guest:expr) => {
        #[cfg(target_arch = "wasm32")]
        mod __bunny_block {
            #[allow(unused_imports)]
            use super::*;

            ::std::thread_local! {
                static GUEST: ::std::cell::RefCell<Option<$crate::abi::Erased>> =
                    const { ::std::cell::RefCell::new(None) };
            }

            /// # Safety
            /// The canonical ABI's allocator: `old` is null or came from
            /// here with `old_len`.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn cabi_realloc(old: *mut u8, old_len: usize, align: usize, len: usize) -> *mut u8 {
                unsafe { $crate::abi::realloc(old, old_len, align, len) }
            }

            /// # Safety
            /// `input` holds `len` bytes the host wrote through
            /// `cabi_realloc`.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn call(input: *mut u8, len: usize) -> *mut u8 {
                let bytes = unsafe { $crate::abi::take_input(input, len) };
                let answer = GUEST.with(|guest| {
                    let mut guest = guest.borrow_mut();
                    let guest = guest.get_or_insert_with(|| $crate::abi::Erased::new($guest));
                    guest.call(&bytes)
                });
                $crate::abi::give_answer(answer)
            }

            /// # Safety
            /// `ret` is what `call` returned, read and not yet released.
            #[unsafe(no_mangle)]
            pub unsafe extern "C" fn cabi_post_call(ret: *mut u8) {
                unsafe { $crate::abi::release_answer(ret) }
            }
        }
    };
}
