//! FFI Objective-C / CoreGraphics escrita à mão — zero dependências.
//!
//! Este módulo é a borda sancionada de `unsafe` do projeto: o runtime
//! Objective-C é chamado por `objc_msgSend` re-declarado com a assinatura
//! concreta de cada mensagem (em arm64 há UM único entry point para todas
//! as mensagens — structs pequenos vão e voltam em registrador, sem
//! variante `_stret`), e duas classes nascem em runtime via
//! `objc_allocateClassPair`/`class_addMethod`:
//!
//! - `BunnyView` (NSView) — recebe o ciclo completo de ponteiro
//!   (`mouseDown:`/`mouseUp:`/`mouseMoved:`/`mouseDragged:`/entrada e
//!   saída via NSTrackingArea) e converte cada posição para as coordenadas
//!   do layout (AppKit conta de baixo para cima; o flip acontece aqui,
//!   uma vez);
//! - `BunnyDelegate` (NSObject) — `windowDidResize:` re-pinta e
//!   `windowWillClose:` encerra o app (fechar a janela fecha o processo).
//!
//! Os callbacks alcançam o mundo Rust por um handler thread-local (o run
//! loop do AppKit é single-thread, como o resto do motor).

use std::cell::RefCell;
use std::ffi::{CString, c_char, c_void};
use std::sync::Once;

pub type Id = *mut c_void;
pub type Sel = *const c_void;

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CGPoint {
    pub x: f64,
    pub y: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct CGRect {
    pub origin: CGPoint,
    pub size: CGSize,
}

// Redeclarar `objc_msgSend` com a assinatura concreta de cada mensagem é o
// modo de uso desenhado do runtime (o símbolo é um trampolim que preserva a
// ABI da chamada) — o lint de declarações conflitantes não se aplica.
#[allow(clashing_extern_declarations)]
#[link(name = "objc", kind = "dylib")]
unsafe extern "C" {
    fn objc_getClass(name: *const c_char) -> Id;
    fn sel_registerName(name: *const c_char) -> Sel;
    fn objc_autoreleasePoolPush() -> *mut c_void;
    fn objc_autoreleasePoolPop(pool: *mut c_void);
    fn objc_allocateClassPair(superclass: Id, name: *const c_char, extra: usize) -> Id;
    fn objc_registerClassPair(class: Id);
    fn class_addMethod(class: Id, sel: Sel, imp: *const c_void, types: *const c_char) -> i8;

    #[link_name = "objc_msgSend"]
    fn msg_id(obj: Id, sel: Sel) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_void(obj: Id, sel: Sel);
    #[link_name = "objc_msgSend"]
    fn msg_void_id(obj: Id, sel: Sel, a: Id);
    #[link_name = "objc_msgSend"]
    fn msg_void_bool(obj: Id, sel: Sel, a: i8);
    #[link_name = "objc_msgSend"]
    fn msg_void_f64(obj: Id, sel: Sel, a: f64);
    #[link_name = "objc_msgSend"]
    fn msg_f64(obj: Id, sel: Sel) -> f64;
    #[link_name = "objc_msgSend"]
    fn msg_bool_i64(obj: Id, sel: Sel, a: i64) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_id_cstr(obj: Id, sel: Sel, a: *const c_char) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_point(obj: Id, sel: Sel) -> CGPoint;
    #[link_name = "objc_msgSend"]
    fn msg_rect(obj: Id, sel: Sel) -> CGRect;
    #[link_name = "objc_msgSend"]
    fn msg_init_rect(obj: Id, sel: Sel, rect: CGRect) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_init_window(obj: Id, sel: Sel, rect: CGRect, style: u64, backing: u64, defer: i8)
    -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_init_tracking(obj: Id, sel: Sel, rect: CGRect, options: u64, owner: Id, info: Id)
    -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_bool(obj: Id, sel: Sel) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_u16(obj: Id, sel: Sel) -> u16;
    #[link_name = "objc_msgSend"]
    fn msg_u64(obj: Id, sel: Sel) -> u64;
    #[link_name = "objc_msgSend"]
    fn msg_i64(obj: Id, sel: Sel) -> i64;
    #[link_name = "objc_msgSend"]
    fn msg_id_arg(obj: Id, sel: Sel, a: Id) -> Id;
    #[link_name = "objc_msgSend"]
    fn msg_bool_id_id(obj: Id, sel: Sel, a: Id, b: Id) -> i8;
    #[link_name = "objc_msgSend"]
    fn msg_timer(
        obj: Id,
        sel: Sel,
        interval: f64,
        target: Id,
        selector: Sel,
        info: Id,
        repeats: i8,
    ) -> Id;
}

// AppKit/QuartzCore entram pelo runtime ObjC; o link garante as classes.
#[link(name = "AppKit", kind = "framework")]
unsafe extern "C" {
    /// O tipo de string do pasteboard (`public.utf8-plain-text`).
    static NSPasteboardTypeString: Id;
}
#[link(name = "QuartzCore", kind = "framework")]
unsafe extern "C" {}

#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    pub(crate) fn CGColorSpaceCreateDeviceRGB() -> *mut c_void;
    pub(crate) fn CGColorSpaceRelease(space: *mut c_void);
    fn CGDataProviderCreateWithCFData(data: *const c_void) -> *mut c_void;
    fn CGDataProviderRelease(provider: *mut c_void);
    #[allow(clippy::too_many_arguments)]
    fn CGImageCreate(
        width: usize,
        height: usize,
        bits_per_component: usize,
        bits_per_pixel: usize,
        bytes_per_row: usize,
        space: *mut c_void,
        bitmap_info: u32,
        provider: *mut c_void,
        decode: *const f64,
        should_interpolate: bool,
        intent: i32,
    ) -> Id;
    fn CGImageRelease(image: Id);
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFDataCreate(allocator: *const c_void, bytes: *const u8, length: isize) -> *const c_void;
    pub(crate) fn CFRelease(cf: *const c_void);
}

unsafe fn class(name: &str) -> Id {
    let name = CString::new(name).expect("nome de classe sem NUL");
    unsafe { objc_getClass(name.as_ptr()) }
}

unsafe fn sel(name: &str) -> Sel {
    let name = CString::new(name).expect("seletor sem NUL");
    unsafe { sel_registerName(name.as_ptr()) }
}

// MARK: - Eventos

/// O que a plataforma entrega ao mundo Rust. Posições em coordenadas do
/// LAYOUT (origem no topo-esquerda, pontos lógicos) — o flip do AppKit já
/// aconteceu.
pub enum AppEvent {
    MouseDown { x: f64, y: f64 },
    MouseUp { x: f64, y: f64 },
    MouseMoved { x: f64, y: f64 },
    /// O ponteiro saiu da janela — sem este evento o hover ficaria preso
    /// na borda (a razão de usar NSTrackingArea).
    MouseExited,
    /// Rolagem: deltas em pontos (trackpad já vem preciso e com momentum;
    /// roda legada é convertida de linhas para pontos na chegada).
    Wheel { x: f64, y: f64, dx: f64, dy: f64 },
    /// Tecla: keyCode de hardware + modificadores + o texto que o AppKit
    /// traduziu. NOTA de dívida: composição por `characters` commita
    /// direto — IME de verdade (marked text CJK) chega com o
    /// NSTextInputClient.
    Key { code: u16, shift: bool, command: bool, chars: String },
    /// Meio-período do blink do caret (o NSTimer do shell).
    Blink,
    /// A janela mudou de tamanho (ou precisa do primeiro frame).
    Redraw,
}

thread_local! {
    static HANDLER: RefCell<Option<Box<dyn FnMut(AppEvent)>>> = const { RefCell::new(None) };
}

/// Registra quem recebe os eventos (o loop do shell).
pub fn set_handler(handler: Box<dyn FnMut(AppEvent)>) {
    HANDLER.with(|slot| *slot.borrow_mut() = Some(handler));
}

/// Entrega um evento ao handler — usado pelos callbacks e pelo primeiro
/// frame.
pub fn dispatch(event: AppEvent) {
    HANDLER.with(|slot| {
        if let Some(handler) = slot.borrow_mut().as_mut() {
            handler(event);
        }
    });
}

/// A posição do evento em coordenadas do layout — AppKit conta de baixo,
/// o layout conta de cima; o flip mora aqui, uma vez.
unsafe fn event_layout_point(this: Id, event: Id) -> (f64, f64) {
    unsafe {
        let point = msg_point(event, sel("locationInWindow"));
        let bounds = msg_rect(this, sel("bounds"));
        (point.x, bounds.size.height - point.y)
    }
}

extern "C" fn bunny_mouse_down(this: Id, _sel: Sel, event: Id) {
    let (x, y) = unsafe { event_layout_point(this, event) };
    dispatch(AppEvent::MouseDown { x, y });
}

extern "C" fn bunny_mouse_up(this: Id, _sel: Sel, event: Id) {
    let (x, y) = unsafe { event_layout_point(this, event) };
    dispatch(AppEvent::MouseUp { x, y });
}

/// `mouseMoved:`, `mouseDragged:` e `mouseEntered:` caem todos aqui —
/// dragged é OBRIGATÓRIO: com o botão pressionado o AppKit manda dragged,
/// nunca moved (sem ele o visual de pressed não solta ao arrastar fora).
extern "C" fn bunny_mouse_moved(this: Id, _sel: Sel, event: Id) {
    let (x, y) = unsafe { event_layout_point(this, event) };
    dispatch(AppEvent::MouseMoved { x, y });
}

extern "C" fn bunny_mouse_exited(_this: Id, _sel: Sel, _event: Id) {
    dispatch(AppEvent::MouseExited);
}

/// O BunnyView aceita virar first responder — sem isso, keyDown não chega.
extern "C" fn bunny_accepts_first_responder(_this: Id, _sel: Sel) -> i8 {
    1
}

extern "C" fn bunny_key_down(_this: Id, _sel: Sel, event: Id) {
    unsafe {
        let code = msg_u16(event, sel("keyCode"));
        let flags = msg_u64(event, sel("modifierFlags"));
        let shift = flags & (1 << 17) != 0;
        let command = flags & (1 << 20) != 0;
        let ns_chars = msg_id(event, sel("characters"));
        let chars = if ns_chars.is_null() {
            String::new()
        } else {
            let utf8 = msg_id(ns_chars, sel("UTF8String")) as *const c_char;
            if utf8.is_null() {
                String::new()
            } else {
                std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned()
            }
        };
        dispatch(AppEvent::Key { code, shift, command, chars });
    }
}

extern "C" fn bunny_scroll_wheel(this: Id, _sel: Sel, event: Id) {
    unsafe {
        let (x, y) = event_layout_point(this, event);
        let mut dx = msg_f64(event, sel("scrollingDeltaX"));
        let mut dy = msg_f64(event, sel("scrollingDeltaY"));
        // trackpad entrega pontos precisos; roda legada entrega TIQUES de
        // linha — converte para pontos aqui, uma vez
        if msg_bool(event, sel("hasPreciseScrollingDeltas")) == 0 {
            dx *= 16.0;
            dy *= 16.0;
        }
        dispatch(AppEvent::Wheel { x, y, dx, dy });
    }
}

extern "C" fn bunny_window_did_resize(_this: Id, _sel: Sel, _note: Id) {
    dispatch(AppEvent::Redraw);
}

extern "C" fn bunny_blink(_this: Id, _sel: Sel, _timer: Id) {
    dispatch(AppEvent::Blink);
}

extern "C" fn bunny_window_will_close(_this: Id, _sel: Sel, _note: Id) {
    unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        msg_void_id(app, sel("terminate:"), std::ptr::null_mut());
    }
}

static REGISTER_CLASSES: Once = Once::new();

unsafe fn register_classes() {
    REGISTER_CLASSES.call_once(|| unsafe {
        let types = CString::new("v@:@").expect("type encoding");

        let view = objc_allocateClassPair(
            class("NSView"),
            CString::new("BunnyView").expect("nome").as_ptr(),
            0,
        );
        class_addMethod(
            view,
            sel("mouseDown:"),
            bunny_mouse_down as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(view, sel("mouseUp:"), bunny_mouse_up as *const c_void, types.as_ptr());
        class_addMethod(
            view,
            sel("mouseMoved:"),
            bunny_mouse_moved as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            view,
            sel("mouseDragged:"),
            bunny_mouse_moved as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            view,
            sel("mouseEntered:"),
            bunny_mouse_moved as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            view,
            sel("mouseExited:"),
            bunny_mouse_exited as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            view,
            sel("scrollWheel:"),
            bunny_scroll_wheel as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            view,
            sel("keyDown:"),
            bunny_key_down as *const c_void,
            types.as_ptr(),
        );
        let bool_getter = CString::new("c@:").expect("type encoding");
        class_addMethod(
            view,
            sel("acceptsFirstResponder"),
            bunny_accepts_first_responder as *const c_void,
            bool_getter.as_ptr(),
        );
        objc_registerClassPair(view);

        let delegate = objc_allocateClassPair(
            class("NSObject"),
            CString::new("BunnyDelegate").expect("nome").as_ptr(),
            0,
        );
        class_addMethod(
            delegate,
            sel("windowDidResize:"),
            bunny_window_did_resize as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            delegate,
            sel("bunnyBlink:"),
            bunny_blink as *const c_void,
            types.as_ptr(),
        );
        class_addMethod(
            delegate,
            sel("windowWillClose:"),
            bunny_window_will_close as *const c_void,
            types.as_ptr(),
        );
        objc_registerClassPair(delegate);
    });
}

// MARK: - Janela

/// Handles crus da janela — `Copy`, mesma thread, embrulhados pelas
/// operações seguras abaixo.
#[derive(Clone, Copy)]
pub struct WindowHandle {
    window: Id,
    view: Id,
    layer: Id,
}

impl WindowHandle {
    /// Tamanho lógico da área de conteúdo (o viewport do layout).
    pub fn content_size(&self) -> (f64, f64) {
        unsafe {
            let bounds = msg_rect(self.view, sel("bounds"));
            (bounds.size.width, bounds.size.height)
        }
    }

    /// O scale factor da tela (retina = 2).
    pub fn scale(&self) -> usize {
        unsafe { msg_f64(self.window, sel("backingScaleFactor")).round().max(1.0) as usize }
    }

    /// Blita um frame RGBA na layer.
    pub fn set_image(&self, width: usize, height: usize, rgba: &[u8]) {
        unsafe {
            let image = cg_image(width, height, rgba);
            msg_void_f64(self.layer, sel("setContentsScale:"), self.scale() as f64);
            msg_void_id(self.layer, sel("setContents:"), image);
            CGImageRelease(image); // a layer retém
        }
    }

    /// Mão sobre alvo interativo; seta fora. `set` direto — sem cursor
    /// rects por ora (o AppKit pode restaurar nas bordas de resize; glitch
    /// cosmético aceito).
    pub fn set_cursor_pointing(&self, pointing: bool) {
        unsafe {
            let cursor = if pointing {
                msg_id(class("NSCursor"), sel("pointingHandCursor"))
            } else {
                msg_id(class("NSCursor"), sel("arrowCursor"))
            };
            msg_void(cursor, sel("set"));
        }
    }
}

/// `kCGImageAlphaPremultipliedLast` — bytes R,G,B,A, alfa por último.
const ALPHA_PREMULTIPLIED_LAST: u32 = 1;

unsafe fn cg_image(width: usize, height: usize, rgba: &[u8]) -> Id {
    unsafe {
        let data = CFDataCreate(std::ptr::null(), rgba.as_ptr(), rgba.len() as isize);
        let provider = CGDataProviderCreateWithCFData(data);
        let space = CGColorSpaceCreateDeviceRGB();
        let image = CGImageCreate(
            width,
            height,
            8,
            32,
            width * 4,
            space,
            ALPHA_PREMULTIPLIED_LAST,
            provider,
            std::ptr::null(),
            false,
            0,
        );
        CGColorSpaceRelease(space);
        CGDataProviderRelease(provider);
        CFRelease(data);
        image
    }
}

/// Cria o app + a janela com a view de eventos, pronta para blit.
pub fn create_window(title: &str, width: f64, height: f64) -> WindowHandle {
    unsafe {
        let pool = objc_autoreleasePoolPush();
        register_classes();

        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        // Regular: app de terminal ganha janela, dock e foco
        let _ = msg_bool_i64(app, sel("setActivationPolicy:"), 0);

        let rect = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize { width, height },
        };
        // titled | closable | miniaturizable | resizable
        let style: u64 = 1 | 2 | 4 | 8;
        let window = msg_id(class("NSWindow"), sel("alloc"));
        let window = msg_init_window(
            window,
            sel("initWithContentRect:styleMask:backing:defer:"),
            rect,
            style,
            2, // buffered
            0,
        );

        let title = CString::new(title).expect("título sem NUL");
        let ns_title = msg_id_cstr(
            class("NSString"),
            sel("stringWithUTF8String:"),
            title.as_ptr(),
        );
        msg_void_id(window, sel("setTitle:"), ns_title);
        msg_void(window, sel("center"));

        // a view de eventos vira o content view, com layer própria
        let view = msg_id(class("BunnyView"), sel("alloc"));
        let view = msg_init_rect(view, sel("initWithFrame:"), rect);
        msg_void_bool(view, sel("setWantsLayer:"), 1);
        msg_void_id(window, sel("setContentView:"), view);
        let layer = msg_id(view, sel("layer"));

        // moved/entered/exited chegam pelo tracking area — sem dança de
        // first responder, e InVisibleRect acompanha o resize sozinho
        // (o rect passado é ignorado). 0x223 = MouseEnteredAndExited |
        // MouseMoved | ActiveInKeyWindow | InVisibleRect.
        let area = msg_id(class("NSTrackingArea"), sel("alloc"));
        let area = msg_init_tracking(
            area,
            sel("initWithRect:options:owner:userInfo:"),
            CGRect {
                origin: CGPoint { x: 0.0, y: 0.0 },
                size: CGSize { width: 0.0, height: 0.0 },
            },
            0x223,
            view,
            std::ptr::null_mut(),
        );
        msg_void_id(view, sel("addTrackingArea:"), area);

        // delegate: resize re-pinta, fechar encerra
        let delegate = msg_id(msg_id(class("BunnyDelegate"), sel("alloc")), sel("init"));
        msg_void_id(window, sel("setDelegate:"), delegate);

        // o meio-período do blink do caret — o run loop retém o timer
        let _ = msg_timer(
            class("NSTimer"),
            sel("scheduledTimerWithTimeInterval:target:selector:userInfo:repeats:"),
            0.5,
            delegate,
            sel("bunnyBlink:"),
            std::ptr::null_mut(),
            1,
        );

        msg_void_id(window, sel("makeKeyAndOrderFront:"), std::ptr::null_mut());
        // o teclado nasce apontando para a view de eventos
        msg_void_id(window, sel("makeFirstResponder:"), view);
        msg_void_bool(app, sel("activateIgnoringOtherApps:"), 1);
        objc_autoreleasePoolPop(pool);

        WindowHandle { window, view, layer }
    }
}

/// Entra no run loop do AppKit — retorna quando o app termina.
pub fn run() {
    unsafe {
        let app = msg_id(class("NSApplication"), sel("sharedApplication"));
        msg_void(app, sel("run"));
    }
}

// MARK: - Clipboard

/// Escreve texto no pasteboard geral do sistema.
pub fn clipboard_write(text: &str) {
    unsafe {
        let pasteboard = msg_id(class("NSPasteboard"), sel("generalPasteboard"));
        let _ = msg_i64(pasteboard, sel("clearContents"));
        let Ok(text) = CString::new(text) else { return };
        let string = msg_id_cstr(class("NSString"), sel("stringWithUTF8String:"), text.as_ptr());
        let _ = msg_bool_id_id(
            pasteboard,
            sel("setString:forType:"),
            string,
            NSPasteboardTypeString,
        );
    }
}

/// Lê o texto do pasteboard geral (`None` = vazio ou não-texto).
pub fn clipboard_read() -> Option<String> {
    unsafe {
        let pasteboard = msg_id(class("NSPasteboard"), sel("generalPasteboard"));
        let string = msg_id_arg(pasteboard, sel("stringForType:"), NSPasteboardTypeString);
        if string.is_null() {
            return None;
        }
        let utf8 = msg_id(string, sel("UTF8String")) as *const c_char;
        if utf8.is_null() {
            return None;
        }
        Some(std::ffi::CStr::from_ptr(utf8).to_string_lossy().into_owned())
    }
}
