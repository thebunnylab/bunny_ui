//! A detached local document crosses a blank-navigation barrier before reuse.
//!
//! The reserve owns an extra retain only during cleaning/availability. Routing is
//! revoked before touching WebKit, and a different mount uses different channels.
use super::{
    CHANNELS, CleaningEngine, EngineOrigin, EnginePool, HTML_PREPARATIONS, Id, LETTERS, MOUNTS,
    PAGES, SpareEngine, bridge, class, current_url, msg_bool, msg_id, msg_id_id_id,
    msg_init_script, msg_void, msg_void_id, ns, null_mut, sel,
};
use bunny_ui::host::{Document, NetworkPolicy};
use std::rc::{Rc, Weak};

/// Retire a detached host before its owner releases its last native retain.
/// Only local, read-only, network-denied documents can enter a live preparation
/// reserve. Every other host keeps its ordinary destruction path.
///
/// # Safety
/// `view` must be a live WKWebView on the main thread, detached by its owning
/// host, with that host's retain held until this call returns.
pub unsafe fn retire(path: &str, view: Id) {
    if !owns_host(path, view) {
        return;
    }
    PAGES.with(|pages| pages.borrow_mut().remove(path));
    let eligible = LETTERS.with(|letters| {
        letters
            .borrow_mut()
            .remove(path)
            .is_some_and(|letter| letter.reusable)
    });
    let generation = MOUNTS.with(|mounts| mounts.borrow_mut().remove(&(view as usize)));
    if !eligible {
        return;
    }
    let pool = HTML_PREPARATIONS.with(|pending| {
        pending
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .find(|pool| pool.borrow().cleaning.len() < 2)
    });
    let Some(pool) = pool else {
        return;
    };
    let discarded = {
        let mut reserve = pool.borrow_mut();
        if reserve.len() == 2 {
            reserve.ready.pop()
        } else {
            None
        }
    };
    drop(discarded);
    unsafe {
        msg_id(view, sel("retain"));
        msg_void(view, sel("stopLoading"));
        msg_void(view, sel("removeFromSuperview"));
        msg_void_id(view, sel("setUIDelegate:"), null_mut());
        let controller = msg_id(
            msg_id(view, sel("configuration")),
            sel("userContentController"),
        );
        msg_void(controller, sel("removeAllUserScripts"));
        if let Some(generation) = generation {
            for channel in CHANNELS {
                msg_void_id(
                    controller,
                    sel("removeScriptMessageHandlerForName:"),
                    ns(&format!("{channel}_{generation}")),
                );
            }
        }
    }
    begin_cleaning(&pool, SpareEngine(view), EngineOrigin::Retired);
}
pub(super) fn begin_cleaning(pool: &Rc<EnginePool>, engine: SpareEngine, origin: EngineOrigin) {
    let view = engine.0;
    pool.borrow_mut().cleaning.push(CleaningEngine {
        engine,
        navigation: null_mut(),
        origin,
    });
    unsafe {
        msg_void_id(view, sel("setNavigationDelegate:"), bridge());
        // Prime the script runtime on the sealed blank, never on user content.
        // No handlers are registered, and readiness removes this inert script.
        let controller = msg_id(
            msg_id(view, sel("configuration")),
            sel("userContentController"),
        );
        let primer = msg_init_script(
            msg_id(class("WKUserScript"), sel("alloc")),
            sel("initWithSource:injectionTime:forMainFrameOnly:"),
            ns("void 0;"),
            0,
            1,
        );
        msg_void_id(controller, sel("addUserScript:"), primer);
        msg_void(primer, sel("release"));
        let blank = Document::new(
            "<!doctype html><html><body></body></html>",
            "",
            NetworkPolicy::Deny,
        );
        let navigation = msg_id_id_id(
            view,
            sel("loadHTMLString:baseURL:"),
            ns(&blank.sealed()),
            null_mut(),
        );
        if let Some(cleaning) = pool
            .borrow_mut()
            .cleaning
            .iter_mut()
            .find(|engine| engine.engine.0 == view)
        {
            cleaning.navigation = navigation;
        }
    }
}

pub(super) fn is_cleaning(view: Id) -> bool {
    HTML_PREPARATIONS.with(|pending| {
        pending
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .any(|pool| {
                pool.borrow()
                    .cleaning
                    .iter()
                    .any(|engine| engine.engine.0 == view)
            })
    })
}
pub(super) fn finish_cleaning(view: Id, navigation: Id) {
    if navigation.is_null() {
        return;
    }
    let pool = HTML_PREPARATIONS.with(|pending| {
        pending
            .borrow()
            .iter()
            .filter_map(Weak::upgrade)
            .find(|pool| {
                pool.borrow()
                    .cleaning
                    .iter()
                    .any(|engine| engine.engine.0 == view && engine.navigation == navigation)
            })
    });
    let Some(pool) = pool else {
        return;
    };
    let mut pool = pool.borrow_mut();
    let Some(index) = pool
        .cleaning
        .iter()
        .position(|engine| engine.engine.0 == view && engine.navigation == navigation)
    else {
        return;
    };
    let cleaned = pool.cleaning.remove(index);
    unsafe {
        msg_void_id(view, sel("setNavigationDelegate:"), null_mut());
        let controller = msg_id(
            msg_id(view, sel("configuration")),
            sel("userContentController"),
        );
        msg_void(controller, sel("removeAllUserScripts"));
        if current_url(view).as_deref() != Some("about:blank")
            || msg_bool(view, sel("canGoBack")) != 0
            || msg_bool(view, sel("canGoForward")) != 0
        {
            return;
        }
    }
    if matches!(cleaned.origin, EngineOrigin::Retired) {
        pool.reused += 1;
    }
    pool.ready.push(cleaned.engine);
}

pub(super) fn owns_host(path: &str, view: Id) -> bool {
    PAGES.with(|pages| pages.borrow().get(path).copied() == Some(view))
}
