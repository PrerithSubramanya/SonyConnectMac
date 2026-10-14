//! Glue. Owns the shared state, feeds the popup state machine, and carries out its effects
//! (windows, Bluetooth link, MDR channel).

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use mdr_proto::msg::{self, NoiseControl};
use objc2_foundation::NSString;
use objc2_io_bluetooth::IOBluetoothDevice;
use tauri::{AppHandle, Emitter};

use crate::machine::{Effect, Input, Machine};
use crate::model::{
    alert_text, auto_off_from_ui, auto_off_to_ui, color_name, eq_preset_name, features_for, nc_to_ui, setting_text, Alert,
    Battery, EqPreset, Setting, Xm5State,
};
use crate::{bt, log, presence, windows};

static ENGINE: OnceLock<Engine> = OnceLock::new();

pub fn get() -> &'static Engine {
    ENGINE.get().expect("engine not started")
}

/// None when the headphones are not paired and the engine never started.
pub fn try_get() -> Option<&'static Engine> {
    ENGINE.get()
}

struct Inner {
    machine: Machine,
    state: Xm5State,
    nc: NoiseControl,
    support_known: bool,
    nc_known: bool,
    ready_sent: bool,
}

pub struct Engine {
    app: AppHandle,
    address: String,
    inner: Mutex<Inner>,
}

/// Start the engine. Call on the main thread, after the paired headphones were found.
pub fn start(app: AppHandle, address: String) {
    let nc = NoiseControl { enabled: false, ambient: false, voice_focus: false, ambient_level: 10 };
    let engine = Engine {
        app: app.clone(),
        address,
        inner: Mutex::new(Inner {
            machine: Machine::new(),
            state: Xm5State::new(),
            nc,
            support_known: false,
            nc_known: false,
            ready_sent: false,
        }),
    };
    if ENGINE.set(engine).is_err() {
        return;
    }

    // Once a second: popup timing.
    std::thread::spawn(|| loop {
        std::thread::sleep(Duration::from_secs(1));
        get().input(Input::Tick);
    });
    // Every 1.5 s: ask macOS if the headphones are connected, and keep the MDR channel open.
    let poll_app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_millis(1500));
        let _ = poll_app.run_on_main_thread(|| {
            bt::tick();
            let linked = bt::is_connected();
            get().input(Input::MacLink(linked));
            if linked {
                bt::open_mdr();
            }
        });
    });
    tauri::async_runtime::spawn(presence::run());
}

impl Engine {
    pub fn input(&self, input: Input) {
        let (effects, before, after) = {
            let mut g = self.inner.lock().unwrap();
            let before = g.machine.phase;
            let fx = g.machine.step(input.clone(), Instant::now());
            (fx, before, g.machine.phase)
        };
        if before != after {
            log::line(format!("phase {before:?} -> {after:?} (after {input:?})"));
        }
        self.run(effects);
    }

    /// The menu-bar icon was clicked: open the popover, or close it if it is open.
    pub fn tray_clicked(&self) {
        let visible = self.inner.lock().unwrap().machine.popup_visible();
        if visible {
            self.input(Input::UserDismiss);
        } else if !windows::just_hidden() {
            self.input(Input::OpenFromTray);
        }
    }

    pub fn snapshot(&self) -> Xm5State {
        let g = self.inner.lock().unwrap();
        let mut s = g.state.clone();
        s.presence = g.machine.phase;
        s.error = g.machine.error.clone();
        s.rssi = g.machine.rssi;
        s
    }

    fn publish(&self) {
        let _ = self.app.emit("xm5-state", self.snapshot());
    }

    fn run(&self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::ShowPopup => windows::show_popup(&self.app, false),
                Effect::ShowPopupInteractive => windows::show_popup(&self.app, true),
                Effect::HidePopup => windows::hide_popup(&self.app),
                Effect::StartConnect => self.start_connect(),
                Effect::OpenMdr => {
                    let _ = self.app.run_on_main_thread(bt::open_mdr);
                }
                Effect::CloseMdr => {
                    self.reset_session_state();
                    let _ = self.app.run_on_main_thread(bt::close_mdr);
                }
                Effect::Publish => self.publish(),
            }
        }
    }

    /// The headphones disconnected: forget what we knew about the session.
    fn reset_session_state(&self) {
        let mut g = self.inner.lock().unwrap();
        g.support_known = false;
        g.nc_known = false;
        g.ready_sent = false;
        g.state.battery = None;
        g.state.auto_power_off_options.clear();
        g.state.alert = None;
        g.state.features.clear();
        g.state.settings.clear();
    }

    fn start_connect(&self) {
        let address = self.address.clone();
        log::line("user asked to connect");
        std::thread::spawn(move || {
            let device = unsafe { IOBluetoothDevice::deviceWithAddressString(Some(&NSString::from_str(&address))) };
            let Some(device) = device else {
                get().input(Input::ConnectFailed("The headphones are not paired with this Mac.".into()));
                return;
            };
            if unsafe { device.isConnected() } {
                get().input(Input::MacLink(true));
                return;
            }
            // Blocks until connected or failed (about five seconds at most).
            let r = unsafe { device.openConnection() };
            log::line(format!("openConnection -> 0x{r:x}"));
            if r == 0 {
                get().input(Input::MacLink(true));
            } else {
                get().input(Input::ConnectFailed(
                    "Could not connect. If the headphones are in use by your phone, turn on \"Connect to 2 devices\" in Sony Sound Connect, or disconnect them from the phone first.".into(),
                ));
            }
        });
    }

    // ---- called from bt.rs on the main thread ----

    pub fn mdr_open_failed(&self, code: i32) {
        log::line(format!("MDR channel open failed 0x{code:x}; will retry"));
    }

    pub fn mdr_closed(&self) {
        self.reset_session_state();
        self.publish();
    }

    pub fn on_mdr_event(&self, event: msg::Event) {
        log::line(format!("mdr event: {event:?}"));
        let ready = {
            let mut g = self.inner.lock().unwrap();
            match event {
                msg::Event::Battery { level, charging } => {
                    g.state.battery = Some(Battery { level, charging: charging == msg::Charging::Charging });
                }
                msg::Event::DeviceInfo { kind: 1, text } => g.state.name = text,
                msg::Event::DeviceInfo { kind: 2, text } => g.state.firmware = Some(text),
                msg::Event::SeriesAndColor { color, .. } => g.state.color = color_name(color).into(),
                msg::Event::SupportFunctions(ids) => {
                    g.state.features = features_for(&ids);
                    g.support_known = true;
                }
                msg::Event::NoiseControl(nc) => {
                    g.nc = nc;
                    g.state.nc = nc_to_ui(&nc);
                    g.nc_known = true;
                }
                msg::Event::SpeakToChat { enabled, .. } => g.state.speak_to_chat = enabled,
                msg::Event::WearPause(on) => g.state.wear_pause = on,
                msg::Event::AutoPowerOff { current, .. } => g.state.auto_power_off = auto_off_to_ui(current).into(),
                msg::Event::Alert { message, needs_answer } => {
                    g.state.alert = Some(Alert { message, text: alert_text(message).into(), choice: needs_answer });
                }
                msg::Event::AutoPowerOffOptions(codes) => {
                    g.state.auto_power_off_options = codes.iter().map(|&c| auto_off_to_ui(c).to_string()).collect();
                }
                msg::Event::ConnectionMode(v) => g.state.connection_quality = if v == 1 { "stable" } else { "sound" }.into(),
                msg::Event::Dsee(on) => g.state.dsee = on,
                msg::Event::EqCapability { presets, .. } => {
                    g.state.eq.presets = presets.iter().map(|&id| EqPreset { id, name: eq_preset_name(id) }).collect();
                }
                msg::Event::Eq { preset, steps } => {
                    g.state.eq.preset = preset;
                    if steps.len() == 6 {
                        let v = |i: usize| steps[i] as i8 - 10;
                        g.state.eq.clear_bass = v(0);
                        g.state.eq.bands = [v(1), v(2), v(3), v(4), v(5)];
                    }
                }
                msg::Event::GsCapability { id, key, summary: _ } => {
                    let (label, summary) = setting_text(&key);
                    match g.state.settings.iter_mut().find(|s| s.id == id) {
                        Some(s) => (s.key, s.label, s.summary) = (key, label, summary),
                        None => g.state.settings.push(Setting { id, key, label, summary, on: false }),
                    }
                    g.state.settings.sort_by_key(|s| s.id);
                }
                msg::Event::GsParam { id, on } => {
                    if let Some(s) = g.state.settings.iter_mut().find(|s| s.id == id) {
                        s.on = on;
                    }
                }
                _ => {}
            }
            let wants_nc = g.state.features.iter().any(|f| f == "nc");
            let ready = !g.ready_sent
                && g.support_known
                && g.state.battery.is_some()
                && (!wants_nc || g.nc_known);
            if ready {
                g.ready_sent = true;
            }
            ready
        };
        self.publish();
        if ready {
            self.input(Input::MdrReady);
        }
    }

    // ---- called from UI commands ----

    pub fn set_nc(&self, mode: Option<String>, ambient_level: Option<u8>, voice_focus: Option<bool>) {
        let nc = {
            let mut g = self.inner.lock().unwrap();
            match mode.as_deref() {
                Some("nc") => (g.nc.enabled, g.nc.ambient) = (true, false),
                Some("ambient") => (g.nc.enabled, g.nc.ambient) = (true, true),
                Some("off") => g.nc.enabled = false,
                _ => {}
            }
            if let Some(level) = ambient_level {
                g.nc.ambient_level = level.min(20);
            }
            if let Some(v) = voice_focus {
                g.nc.voice_focus = v;
            }
            g.state.nc = nc_to_ui(&g.nc);
            g.nc
        };
        self.publish();
        self.write(msg::set_noise_control(nc, true), msg::get_noise_control());
    }

    /// Send a write to the headphones. The headphones confirm with a notification, which then
    /// corrects the state if they refused.
    /// Every write is followed by a read of the same setting, so the screen shows what the
    /// headphones really accepted, even if they send no notification back.
    fn write(&self, set: Vec<u8>, read_back: Vec<u8>) {
        log::line(format!("write: set {:02X?} then read {:02X?}", set, read_back));
        let _ = self.app.run_on_main_thread(move || bt::send_many(vec![set, read_back]));
    }

    pub fn set_toggle(&self, key: &str, value: bool) {
        let payload = {
            let mut g = self.inner.lock().unwrap();
            match key {
                "speakToChat" => {
                    g.state.speak_to_chat = value;
                    (msg::set_speak_to_chat(value), msg::get_speak_to_chat())
                }
                "wearPause" => {
                    g.state.wear_pause = value;
                    (msg::set_wear_pause(value), msg::get_wear_pause())
                }
                "dsee" => {
                    g.state.dsee = value;
                    (msg::set_dsee(value), msg::get_dsee())
                }
                _ => return,
            }
        };
        self.publish();
        self.write(payload.0, payload.1);
    }

    pub fn set_choice(&self, key: &str, value: &str) {
        let payload = {
            let mut g = self.inner.lock().unwrap();
            match key {
                "autoPowerOff" => {
                    let Some(code) = auto_off_from_ui(value) else { return };
                    // The headphones silently ignore codes they do not offer.
                    if !g.state.auto_power_off_options.iter().any(|o| o == value) {
                        log::line(format!("auto power off {value} is not offered by the headphones; ignored"));
                        return;
                    }
                    g.state.auto_power_off = value.into();
                    (msg::set_auto_power_off(code), msg::get_auto_power_off())
                }
                "connectionQuality" => {
                    g.state.connection_quality = if value == "stable" { "stable" } else { "sound" }.into();
                    (msg::set_connection_mode(value == "stable"), msg::get_connection_mode())
                }
                _ => return,
            }
        };
        self.publish();
        self.write(payload.0, payload.1);
    }

    /// Pick a preset, or move one band. Moving a band switches to the "Manual" preset, like the
    /// Sony app does, unless a custom preset is already selected.
    pub fn set_eq(&self, preset: Option<u8>, band: Option<usize>, value: Option<i8>, clear_bass: Option<i8>) {
        let payload = {
            let mut g = self.inner.lock().unwrap();
            if let Some(id) = preset {
                g.state.eq.preset = id;
                (msg::set_eq_preset(id), msg::get_eq())
            } else {
                if let (Some(i), Some(v)) = (band, value) {
                    if i < 5 {
                        g.state.eq.bands[i] = v.clamp(-10, 10);
                    }
                }
                if let Some(v) = clear_bass {
                    g.state.eq.clear_bass = v.clamp(-10, 10);
                }
                if !matches!(g.state.eq.preset, 0xA0..=0xA2) {
                    g.state.eq.preset = 0xA0;
                }
                (msg::set_eq_bands(g.state.eq.preset, g.state.eq.clear_bass, g.state.eq.bands), msg::get_eq())
            }
        };
        self.publish();
        self.write(payload.0, payload.1);
    }

    pub fn set_setting(&self, id: u8, on: bool) {
        {
            let mut g = self.inner.lock().unwrap();
            let Some(s) = g.state.settings.iter_mut().find(|s| s.id == id) else { return };
            s.on = on;
        }
        self.publish();
        self.write(msg::set_gs(id, on), msg::get_gs_param(id));
    }

    /// The user answered the headphones' question. A change they asked for is only applied when
    /// the answer is yes. The settings are read again afterwards, so the screen shows the result.
    pub fn answer_alert(&self, positive: bool) {
        let alert = self.inner.lock().unwrap().state.alert.take();
        self.publish();
        let Some(alert) = alert else { return };
        if !alert.choice {
            return; // only a notice, nothing to answer
        }
        log::line(format!("alert {:#04x} answered {}", alert.message, if positive { "yes" } else { "no" }));
        let _ = self.app.run_on_main_thread(move || {
            bt::send_many(vec![msg::alert_reply(alert.message, positive), msg::get_gs_param(0xD1), msg::get_gs_param(0xD2)]);
        });
    }
}
