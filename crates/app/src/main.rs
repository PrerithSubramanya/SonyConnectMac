//! XM5 Popup: an AirPods-style popover and Sound Connect-style controls for the Sony WH-1000XM5,
//! dropping down from the menu bar.
//!
//! Protocol knowledge: https://github.com/mos9527/SonyHeadphonesClient (MIT).

mod bt;
mod engine;
mod log;
mod machine;
mod model;
mod presence;
mod windows;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{ActivationPolicy, AppHandle};

use crate::engine::try_get;
use crate::machine::Input;
use crate::model::Xm5State;

// The engine only exists when the headphones are paired. Without it every command is a no-op.

#[tauri::command]
fn get_state() -> Xm5State {
    try_get().map(|e| e.snapshot()).unwrap_or_else(Xm5State::new)
}

/// The page reports its own errors here, because a bundled app has no console to look at.
#[tauri::command]
fn ui_log(msg: String) {
    log::line(format!("ui: {msg}"));
}

#[tauri::command]
fn connect() {
    if let Some(e) = try_get() {
        e.input(Input::UserConnect);
    }
}

#[tauri::command]
fn dismiss_popup() {
    if let Some(e) = try_get() {
        e.input(Input::UserDismiss);
    }
}

/// The page sets this while a dropdown list is open, so losing focus does not close the popover.
#[tauri::command]
fn hold_popup(on: bool) {
    windows::hold_open(on);
}

#[tauri::command]
fn resize_popup(app: AppHandle, height: f64) {
    windows::resize_popup(&app, height);
}

/// JS sends `{ mode, ambientLevel, voiceFocus }`. Tauri maps camelCase keys to these names.
#[tauri::command]
fn set_nc(mode: Option<String>, ambient_level: Option<u8>, voice_focus: Option<bool>) {
    log::line(format!("command set_nc mode={mode:?} level={ambient_level:?} voice={voice_focus:?}"));
    if let Some(e) = try_get() {
        e.set_nc(mode, ambient_level, voice_focus);
    }
}

#[tauri::command]
fn set_toggle(key: String, value: bool) {
    log::line(format!("command set_toggle {key}={value}"));
    if let Some(e) = try_get() {
        e.set_toggle(&key, value);
    }
}

#[tauri::command]
fn set_choice(key: String, value: String) {
    log::line(format!("command set_choice {key}={value}"));
    if let Some(e) = try_get() {
        e.set_choice(&key, &value);
    }
}

/// JS sends one of `{ preset }`, `{ band, value }` or `{ clearBass }`.
#[tauri::command]
fn set_eq(preset: Option<u8>, band: Option<usize>, value: Option<i8>, clear_bass: Option<i8>) {
    log::line(format!("command set_eq preset={preset:?} band={band:?} value={value:?} clear_bass={clear_bass:?}"));
    if let Some(e) = try_get() {
        e.set_eq(preset, band, value, clear_bass);
    }
}

/// The user's answer to a question from the headphones (for example "switch multipoint?").
#[tauri::command]
fn answer_alert(positive: bool) {
    log::line(format!("command answer_alert {positive}"));
    if let Some(e) = try_get() {
        e.answer_alert(positive);
    }
}

#[tauri::command]
fn set_setting(id: u8, on: bool) {
    log::line(format!("command set_setting {id:#04x}={on}"));
    if let Some(e) = try_get() {
        e.set_setting(id, on);
    }
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_state, ui_log, connect, dismiss_popup, hold_popup, resize_popup, set_nc, set_toggle, set_choice, set_eq, set_setting, answer_alert
        ])
        .setup(|app| {
            app.set_activation_policy(ActivationPolicy::Accessory);
            windows::create_popup(app.handle())?;

            // Left click opens the popover under the icon. Right click shows the menu.
            let quit = MenuItem::with_id(app, "quit", "Quit XM5 Popup", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&quit])?;
            TrayIconBuilder::with_id("tray")
                .icon(tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))?)
                .icon_as_template(true)
                .tooltip("WH-1000XM5")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id.as_ref() == "quit" {
                        let _ = app.run_on_main_thread(bt::close_mdr);
                        app.exit(0);
                    }
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, rect, .. } = event {
                        let scale = tray.app_handle().primary_monitor().ok().flatten().map(|m| m.scale_factor()).unwrap_or(2.0);
                        windows::remember_tray_rect(&rect, scale);
                        if let Some(e) = try_get() {
                            e.tray_clicked();
                        }
                    }
                })
                .build(app)?;

            // On the main thread: find the paired headphones and start the engine.
            match bt::find_device("WH-1000XM5") {
                Some(address) => {
                    log::line("found paired WH-1000XM5");
                    engine::start(app.handle().clone(), address);
                }
                None => log::line("no paired WH-1000XM5 found; pair it in System Settings first"),
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the app");
}
