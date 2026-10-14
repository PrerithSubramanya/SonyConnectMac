//! The popover: a glass card that drops down from the menu-bar icon, like a system popup.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use objc2::rc::Retained;
use objc2::runtime::AnyClass;
use objc2::{msg_send, MainThreadMarker};
use objc2_app_kit::{
    NSAutoresizingMaskOptions, NSGlassEffectView, NSGlassEffectViewStyle, NSView, NSVisualEffectBlendingMode,
    NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView, NSWindow, NSWindowCollectionBehavior,
    NSWindowOrderingMode,
};
use objc2_foundation::NSRect;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

use crate::machine::Input;
use crate::{engine, log};

pub const POPUP_W: f64 = 400.0;
/// Corner radius of the glass. The page's CSS uses the same value.
const CARD_RADIUS: f64 = 28.0;
/// Gap under the menu-bar icon / from the screen edge, in points.
const GAP: f64 = 6.0;
/// macOS NSStatusWindowLevel: above normal windows and the Dock, like system popups.
const STATUS_WINDOW_LEVEL: isize = 25;
/// A click on the menu-bar icon that closes the popover also blurs it. Ignore the follow-up
/// "open" for this long so the popover does not close and instantly reopen.
const REOPEN_GUARD: Duration = Duration::from_millis(350);

/// Bumped on every show, so a fade-out that is still pending cannot hide a newer popup.
static SHOW_GENERATION: AtomicU64 = AtomicU64::new(0);
/// True once the user has clicked into the popover. Clicking anywhere else then closes it.
static ENGAGED: AtomicBool = AtomicBool::new(false);
/// While a dropdown list is open the window loses focus. That must not close the popover.
static HOLD_OPEN: AtomicBool = AtomicBool::new(false);
/// Menu-bar icon rectangle in physical pixels: x, y, width, height.
static TRAY_RECT: Mutex<Option<(f64, f64, f64, f64)>> = Mutex::new(None);
static LAST_HIDE: Mutex<Option<Instant>> = Mutex::new(None);

#[derive(Serialize, Clone)]
struct ShowPayload {
    interactive: bool,
}

/// The popup is created once, hidden, so showing it later is instant. Call on the main thread.
pub fn create_popup(app: &AppHandle) -> tauri::Result<()> {
    let window = WebviewWindowBuilder::new(app, "popup", WebviewUrl::App("popup.html".into()))
        .title("XM5")
        .inner_size(POPUP_W, 320.0)
        .decorations(false)
        .transparent(true)
        .shadow(true)
        .resizable(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .accept_first_mouse(true)
        .build()?;
    style_popup(&window);
    install_glass(&window);
    window.on_window_event(|event| {
        if let WindowEvent::Focused(focused) = event {
            if *focused {
                ENGAGED.store(true, Ordering::SeqCst);
            } else if ENGAGED.load(Ordering::SeqCst) && !HOLD_OPEN.load(Ordering::SeqCst) {
                // Clicked somewhere else: close, like a menu.
                if let Some(e) = engine::try_get() {
                    e.input(Input::UserDismiss);
                }
            }
        }
    });
    Ok(())
}

fn ns_window(window: &WebviewWindow) -> Option<&'static NSWindow> {
    let ptr = window.ns_window().ok()?;
    // SAFETY: the pointer is this window's NSWindow. Callers run on the main thread, and the
    // window lives as long as the app.
    Some(unsafe { &*(ptr as *mut NSWindow) })
}

fn style_popup(window: &WebviewWindow) {
    let Some(ns) = ns_window(window) else { return };
    ns.setLevel(STATUS_WINDOW_LEVEL);
    ns.setHasShadow(true);
    ns.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
}

/// Put the system glass material behind the web view, so the popup matches macOS.
/// macOS 26 and later: Liquid Glass (NSGlassEffectView). Earlier: blurred vibrancy.
fn install_glass(window: &WebviewWindow) {
    let Some(ns) = ns_window(window) else { return };
    let Some(content) = ns.contentView() else { return };
    let Some(mtm) = MainThreadMarker::new() else { return };

    let glass: Retained<NSView> = if AnyClass::get(c"NSGlassEffectView").is_some() {
        let view = NSGlassEffectView::new(mtm);
        view.setCornerRadius(CARD_RADIUS);
        view.setStyle(NSGlassEffectViewStyle::Regular);
        log::line("popup glass: NSGlassEffectView (Liquid Glass)");
        Retained::into_super(view)
    } else {
        let view = NSVisualEffectView::new(mtm);
        view.setMaterial(NSVisualEffectMaterial::HUDWindow);
        view.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
        view.setState(NSVisualEffectState::Active);
        view.setWantsLayer(true);
        // SAFETY: plain CALayer property sets on the view's own layer.
        unsafe {
            if let Some(layer) = view.layer() {
                let _: () = msg_send![&*layer, setCornerRadius: CARD_RADIUS];
                let _: () = msg_send![&*layer, setMasksToBounds: true];
            }
        }
        log::line("popup glass: NSVisualEffectView (vibrancy fallback)");
        Retained::into_super(view)
    };
    glass.setFrame(content.bounds());
    glass.setAutoresizingMask(NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable);
    // Below the web view, which draws the text and the headphones on top.
    content.addSubview_positioned_relativeTo(&glass, NSWindowOrderingMode::Below, None);
}

/// Remember where the menu-bar icon is (physical pixels), from a tray click event.
pub fn remember_tray_rect(rect: &tauri::Rect, scale: f64) {
    let p = rect.position.to_physical::<f64>(scale);
    let s = rect.size.to_physical::<f64>(scale);
    *TRAY_RECT.lock().unwrap() = Some((p.x, p.y, s.width, s.height));
}

/// Ask for the icon's current position (it can move), and fall back to the last click's.
fn tray_rect(app: &AppHandle) -> Option<(f64, f64, f64, f64)> {
    let fresh = app.tray_by_id("tray").and_then(|t| t.rect().ok().flatten()).map(|rect| {
        let scale = app.primary_monitor().ok().flatten().map(|m| m.scale_factor()).unwrap_or(2.0);
        let p = rect.position.to_physical::<f64>(scale);
        let s = rect.size.to_physical::<f64>(scale);
        (p.x, p.y, s.width, s.height)
    });
    fresh.or(*TRAY_RECT.lock().unwrap())
}

/// Directly under the menu-bar icon, kept inside the screen. The window grows downward.
fn place_popup(app: &AppHandle, window: &WebviewWindow) {
    let anchor = tray_rect(app);
    let monitor = anchor
        .and_then(|(x, y, w, h)| app.monitor_from_point(x + w / 2.0, y + h / 2.0).ok().flatten())
        .or_else(|| app.cursor_position().ok().and_then(|p| app.monitor_from_point(p.x, p.y).ok().flatten()))
        .or_else(|| app.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else { return };
    let area = monitor.work_area();
    let gap = (GAP * monitor.scale_factor()) as i32;
    let size = window.outer_size().unwrap_or_default();
    let (left, right) = (area.position.x + gap, area.position.x + area.size.width as i32 - size.width as i32 - gap);
    let (x, y) = match anchor {
        Some((ax, ay, aw, ah)) => {
            let x = ((ax + aw / 2.0) as i32 - size.width as i32 / 2).clamp(left, right.max(left));
            (x, ((ay + ah) as i32 + gap / 2).max(area.position.y))
        }
        None => (right, area.position.y + gap),
    };
    let _ = window.set_position(PhysicalPosition::new(x, y));
}

/// `interactive` is true when the user opened it from the menu bar: it then stays until clicked
/// away. Otherwise it is an announcement that hides itself, and is shown without taking focus.
pub fn show_popup(app: &AppHandle, interactive: bool) {
    log::line(format!("popup show interactive={interactive}"));
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let Some(window) = app.get_webview_window("popup") else { return };
        SHOW_GENERATION.fetch_add(1, Ordering::SeqCst);
        ENGAGED.store(interactive, Ordering::SeqCst);
        place_popup(&app, &window);
        let _ = app.emit_to("popup", "xm5-popup-show", ShowPayload { interactive });
        let Some(ns) = ns_window(&window) else { return };
        // Fade in. Only the alpha is animated: animating the frame too would finish at a stale
        // height whenever the card resizes while it is fading in, and the card would be cropped.
        ns.setAlphaValue(0.0);
        ns.orderFrontRegardless();
        animate(ns, 1.0, None, 0.3);
        if interactive {
            let _ = window.set_focus();
        }
    });
}

pub fn hide_popup(app: &AppHandle) {
    log::line("popup hide");
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let Some(window) = app.get_webview_window("popup") else { return };
        let Some(ns) = ns_window(&window) else { return };
        ENGAGED.store(false, Ordering::SeqCst);
        *LAST_HIDE.lock().unwrap() = Some(Instant::now());
        let generation = SHOW_GENERATION.load(Ordering::SeqCst);
        animate(ns, 0.0, None, 0.2);
        let app2 = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(240));
            let _ = app2.clone().run_on_main_thread(move || {
                if SHOW_GENERATION.load(Ordering::SeqCst) == generation {
                    if let Some(w) = app2.get_webview_window("popup") {
                        let _ = w.hide();
                        let _ = app2.emit_to("popup", "xm5-popup-hidden", ());
                    }
                }
            });
        });
    });
}

pub fn hold_open(on: bool) {
    HOLD_OPEN.store(on, Ordering::SeqCst);
}

/// True if the popover was closed a moment ago, so a menu-bar click should not reopen it.
pub fn just_hidden() -> bool {
    LAST_HIDE.lock().unwrap().is_some_and(|t| t.elapsed() < REOPEN_GUARD)
}

/// Animate the window's alpha (and optionally its frame) with AppKit's own animator.
fn animate(ns: &NSWindow, alpha: f64, frame: Option<NSRect>, seconds: f64) {
    use objc2_app_kit::NSAnimationContext;
    NSAnimationContext::beginGrouping();
    NSAnimationContext::currentContext().setDuration(seconds);
    // SAFETY: `animator` returns a proxy of the same window; the selectors are standard NSWindow API.
    unsafe {
        let proxy: Retained<NSWindow> = msg_send![ns, animator];
        let _: () = msg_send![&*proxy, setAlphaValue: alpha];
        if let Some(frame) = frame {
            let _: () = msg_send![&*proxy, setFrame: frame, display: true];
        }
    }
    NSAnimationContext::endGrouping();
}

/// Fit the popup window to its card exactly, so the glass is the card and nothing invisible
/// blocks clicks beneath it.
pub fn resize_popup(app: &AppHandle, height: f64) {
    let app = app.clone();
    let _ = app.clone().run_on_main_thread(move || {
        let Some(window) = app.get_webview_window("popup") else { return };
        let _ = window.set_size(tauri::LogicalSize::new(POPUP_W, height.clamp(120.0, 800.0)));
        place_popup(&app, &window);
        if let Some(ns) = ns_window(&window) {
            ns.invalidateShadow();
        }
    });
}
