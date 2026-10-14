//! The state shared with the web UI. Field names are camelCase on the wire.

use mdr_proto::msg::{auto_off, NoiseControl};
use serde::Serialize;

use crate::machine::Phase;

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Battery {
    pub level: u8,
    pub charging: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Nc {
    /// "nc", "ambient" or "off".
    pub mode: String,
    pub ambient_level: u8,
    pub voice_focus: bool,
}

#[derive(Serialize, Clone)]
pub struct EqPreset {
    pub id: u8,
    pub name: String,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Eq {
    /// Preset id as the headphones use it.
    pub preset: u8,
    pub presets: Vec<EqPreset>,
    /// 400 Hz, 1 kHz, 2.5 kHz, 6.3 kHz, 16 kHz, each -10 to +10.
    pub bands: [i8; 5],
    pub clear_bass: i8,
}

/// A toggle the headphones describe themselves (touch panel, multipoint...).
#[derive(Serialize, Clone)]
pub struct Setting {
    pub id: u8,
    pub key: String,
    pub label: String,
    pub summary: String,
    pub on: bool,
}

/// Something the headphones want to tell the user, or ask, before applying a change.
#[derive(Serialize, Clone)]
pub struct Alert {
    pub message: u8,
    pub text: String,
    /// True when the headphones wait for a yes or no.
    pub choice: bool,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct Xm5State {
    pub presence: Phase,
    pub rssi: Option<i16>,
    pub error: Option<String>,
    pub name: String,
    pub color: String,
    pub firmware: Option<String>,
    pub battery: Option<Battery>,
    pub nc: Nc,
    pub speak_to_chat: bool,
    pub wear_pause: bool,
    /// "5m", "15m", "30m", "60m", "180m", "removed" or "off".
    pub auto_power_off: String,
    /// The auto power-off choices this headphone really accepts, as UI codes.
    pub auto_power_off_options: Vec<String>,
    /// "sound" or "stable".
    pub connection_quality: String,
    pub dsee: bool,
    pub eq: Eq,
    pub settings: Vec<Setting>,
    pub alert: Option<Alert>,
    /// Controls the UI may show: supported by the headphones AND implemented by this app.
    pub features: Vec<String>,
}

impl Xm5State {
    pub fn new() -> Self {
        Self {
            presence: Phase::Absent,
            rssi: None,
            error: None,
            name: "WH-1000XM5".into(),
            color: "black".into(),
            firmware: None,
            battery: None,
            nc: nc_to_ui(&NoiseControl { enabled: false, ambient: false, voice_focus: false, ambient_level: 10 }),
            speak_to_chat: false,
            wear_pause: false,
            auto_power_off: "off".into(),
            auto_power_off_options: Vec::new(),
            connection_quality: "sound".into(),
            dsee: false,
            eq: Eq { preset: 0, presets: Vec::new(), bands: [0; 5], clear_bass: 0 },
            settings: Vec::new(),
            alert: None,
            features: Vec::new(),
        }
    }
}

pub fn nc_to_ui(nc: &NoiseControl) -> Nc {
    Nc {
        mode: if !nc.enabled { "off" } else if nc.ambient { "ambient" } else { "nc" }.into(),
        ambient_level: nc.ambient_level,
        voice_focus: nc.voice_focus,
    }
}

/// Colour code from the headphones (0x01 black, 0x02 white, 0x03 silver, 0x05 blue, 0x06 pink)
/// to the palette names the UI draws.
pub fn color_name(code: u8) -> &'static str {
    match code & 0x0F {
        0x02 | 0x03 => "silver",
        0x05 => "blue",
        0x06 => "pink",
        _ => "black",
    }
}

/// Which UI features to offer, given the function ids the headphones reported.
/// Only features this app can really drive are listed.
pub fn features_for(support: &[u8]) -> Vec<String> {
    let has = |id: u8| support.contains(&id);
    let mut out: Vec<&str> = Vec::new();
    if has(0x20) {
        out.push("battery");
    }
    if has(0x6B) {
        out.extend(["nc", "ambient-level", "voice-focus"]);
    }
    for (id, name) in [
        (0xFC, "speak-to-chat"),
        (0xF1, "wear-pause"),
        (0x25, "auto-power-off"),
        (0xE1, "connection-quality"),
        (0xE2, "dsee"),
        (0x50, "eq"),
    ] {
        if has(id) {
            out.push(name);
        }
    }
    out.into_iter().map(String::from).collect()
}

/// Equalizer preset names, from the headphones' own preset ids.
pub fn eq_preset_name(id: u8) -> String {
    match id {
        0x00 => "Off",
        0x10 => "Bright",
        0x11 => "Excited",
        0x12 => "Mellow",
        0x13 => "Relaxed",
        0x14 => "Vocal",
        0x15 => "Treble Boost",
        0x16 => "Bass Boost",
        0x17 => "Speech",
        0xA0 => "Manual",
        0xA1 => "Custom 1",
        0xA2 => "Custom 2",
        other => return format!("Preset {other:#04x}"),
    }
    .into()
}

pub fn auto_off_to_ui(code: u8) -> &'static str {
    match code {
        auto_off::MIN_5 => "5m",
        auto_off::MIN_15 => "15m",
        auto_off::MIN_30 => "30m",
        auto_off::MIN_60 => "60m",
        auto_off::MIN_180 => "180m",
        auto_off::WHEN_REMOVED => "removed",
        _ => "off",
    }
}

pub fn auto_off_from_ui(s: &str) -> Option<u8> {
    Some(match s {
        "5m" => auto_off::MIN_5,
        "15m" => auto_off::MIN_15,
        "30m" => auto_off::MIN_30,
        "60m" => auto_off::MIN_60,
        "180m" => auto_off::MIN_180,
        "removed" => auto_off::WHEN_REMOVED,
        "off" => auto_off::DISABLED,
        _ => return None,
    })
}

/// What the headphones are asking, in plain words. Message ids come from the protocol's list.
pub fn alert_text(message: u8) -> &'static str {
    match message {
        0x00 => "Changing the connection mode will briefly disconnect the headphones.",
        0x06 => "Switching multipoint will briefly disconnect the headphones. The high-quality LDAC codec is turned off while two devices are connected.",
        0x07 => "Switching multipoint will briefly disconnect the headphones.",
        0x08 => "Using the equalizer and DSEE together uses more battery.",
        0x0A | 0x0B => "Touch gestures on the ear cup will stop working.",
        0x0C | 0x0D => "Touch gestures on the ear cup will work again.",
        0x1C => "Sound quality priority can make the connection less stable.",
        _ => "The headphones need your confirmation to apply this change.",
    }
}

/// Friendly label and one-line explanation for the headphones' own setting keys.
pub fn setting_text(key: &str) -> (String, String) {
    match key {
        "TOUCH_PANEL_SETTING" => (
            "Touch sensor control panel".into(),
            "Use the touch panel on the right ear cup for playback and volume.".into(),
        ),
        "MULTIPOINT_SETTING" => (
            "Connect to 2 devices simultaneously".into(),
            "Keep the headphones connected to two devices at once, such as your phone and this Mac.".into(),
        ),
        other => {
            let words: Vec<String> = other
                .split('_')
                .filter(|w| !w.is_empty())
                .enumerate()
                .map(|(i, w)| {
                    let w = w.to_lowercase();
                    if i == 0 {
                        let mut c = w.chars();
                        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
                    } else {
                        w
                    }
                })
                .collect();
            (words.join(" "), String::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_off_codes_round_trip() {
        for s in ["5m", "15m", "30m", "60m", "180m", "removed", "off"] {
            assert_eq!(auto_off_to_ui(auto_off_from_ui(s).unwrap()), s);
        }
        assert!(auto_off_from_ui("tomorrow").is_none());
    }

    #[test]
    fn xm5_features_follow_the_reported_support_list() {
        let f = features_for(&[0x20, 0x6B, 0xFC, 0xF1, 0x25, 0xE1, 0xE2, 0x50, 0x70, 0xF9]);
        for want in ["battery", "nc", "speak-to-chat", "wear-pause", "auto-power-off", "connection-quality", "dsee", "eq"] {
            assert!(f.iter().any(|x| x == want), "missing {want}");
        }
        // 0x70 (adaptive control) and 0xF9 (factory reset) are deliberately not offered.
        assert_eq!(f.len(), 10);
    }

    #[test]
    fn alert_text_knows_the_multipoint_messages() {
        assert!(alert_text(0x07).contains("multipoint"));
        assert!(alert_text(0xEE).contains("confirmation"));
    }

    #[test]
    fn setting_text_knows_the_xm5_keys() {
        assert_eq!(setting_text("MULTIPOINT_SETTING").0, "Connect to 2 devices simultaneously");
        assert_eq!(setting_text("SOME_NEW_THING").0, "Some new thing");
    }
}
