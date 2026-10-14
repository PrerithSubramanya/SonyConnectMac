//! Sony "MDR" wire protocol, as used by the Sony Sound Connect app (V2 family, e.g. WH-1000XM5).
//!
//! This crate is pure Rust: no Bluetooth, no I/O. The transport only moves bytes in and out.
//!
//! The protocol facts here (framing, escaping, checksum, command ids) come from the
//! MIT-licensed reverse engineering work in <https://github.com/mos9527/SonyHeadphonesClient>
//! (Copyright (c) 2026 mos9527, Amr Satrio and other contributors). Credit to them.

use std::collections::VecDeque;

pub const START: u8 = 0x3E; // '>'
pub const END: u8 = 0x3C; // '<'
const ESCAPE: u8 = 0x3D; // '='

/// RFCOMM service UUID that XM5 and newer headphones expose for MDR.
pub const SERVICE_UUID_V2: &str = "956C7B26-D49A-4BA8-B03F-B17D393CB6E2";
/// RFCOMM service UUID used by older models (XM3, XM4 era).
pub const SERVICE_UUID_LEGACY: &str = "96CC203E-5068-46AD-B32D-E316F5E069BA";

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DataType {
    Data,
    Ack,
    /// Table 1 commands.
    DataMdr,
    /// Table 2 commands.
    DataMdrNo2,
    Other(u8),
}

impl DataType {
    pub fn from_u8(b: u8) -> Self {
        match b {
            0 => Self::Data,
            1 => Self::Ack,
            12 => Self::DataMdr,
            14 => Self::DataMdrNo2,
            x => Self::Other(x),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Self::Data => 0,
            Self::Ack => 1,
            Self::DataMdr => 12,
            Self::DataMdrNo2 => 14,
            Self::Other(x) => x,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Packet {
    pub ty: DataType,
    pub seq: u8,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UnpackError {
    BadMarker,
    BadEscape,
    TooShort,
    BadLength,
    BadChecksum,
}

pub fn checksum(data: &[u8]) -> u8 {
    data.iter().fold(0u8, |sum, b| sum.wrapping_add(*b))
}

fn escape(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 4);
    for &b in data {
        match b {
            END | ESCAPE | START => out.extend_from_slice(&[ESCAPE, b & 0xEF]),
            _ => out.push(b),
        }
    }
    out
}

fn unescape(data: &[u8]) -> Result<Vec<u8>, UnpackError> {
    let mut out = Vec::with_capacity(data.len());
    let mut it = data.iter();
    while let Some(&b) = it.next() {
        if b == ESCAPE {
            match it.next() {
                Some(&x @ 0x2C..=0x2E) => out.push(x | 0x10),
                _ => return Err(UnpackError::BadEscape),
            }
        } else {
            out.push(b);
        }
    }
    Ok(out)
}

/// Frame layout: `START escape(type seq len_be32 payload checksum) END`.
pub fn pack(ty: DataType, seq: u8, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(payload.len() + 7);
    body.push(ty.to_u8());
    body.push(seq);
    body.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    body.extend_from_slice(payload);
    body.push(checksum(&body));

    let mut frame = Vec::with_capacity(body.len() + 2);
    frame.push(START);
    frame.extend(escape(&body));
    frame.push(END);
    frame
}

/// Unpack one complete frame (including the START and END markers).
pub fn unpack(frame: &[u8]) -> Result<Packet, UnpackError> {
    if frame.len() < 2 || frame[0] != START || frame[frame.len() - 1] != END {
        return Err(UnpackError::BadMarker);
    }
    let body = unescape(&frame[1..frame.len() - 1])?;
    if body.len() < 7 {
        return Err(UnpackError::TooShort);
    }
    let (data, sum) = body.split_at(body.len() - 1);
    if checksum(data) != sum[0] {
        return Err(UnpackError::BadChecksum);
    }
    let len = u32::from_be_bytes([data[2], data[3], data[4], data[5]]) as usize;
    if data.len() - 6 != len {
        return Err(UnpackError::BadLength);
    }
    Ok(Packet {
        ty: DataType::from_u8(data[0]),
        seq: data[1],
        payload: data[6..].to_vec(),
    })
}

/// Collects raw RFCOMM bytes and splits them into frames.
/// A frame can arrive in several chunks, and one chunk can hold several frames.
#[derive(Default)]
pub struct FrameReader {
    buf: Vec<u8>,
}

impl FrameReader {
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Result<Packet, UnpackError>> {
        self.buf.extend_from_slice(bytes);
        let mut out = Vec::new();
        loop {
            // Throw away anything before the next START marker.
            match self.buf.iter().position(|&b| b == START) {
                Some(0) => {}
                Some(n) => {
                    self.buf.drain(..n);
                }
                None => {
                    self.buf.clear();
                    break;
                }
            }
            // START and END bytes never appear unescaped inside a frame body.
            match self.buf.iter().skip(1).position(|&b| b == END) {
                Some(rel) => {
                    let end = rel + 1;
                    let frame: Vec<u8> = self.buf.drain(..=end).collect();
                    out.push(unpack(&frame));
                }
                None => break,
            }
        }
        out
    }
}

/// What came out of feeding bytes to the [`Link`].
#[derive(Default, Debug)]
pub struct LinkOutput {
    /// Bytes to write to the RFCOMM channel now (ACKs and the next queued command).
    pub tx: Vec<u8>,
    /// Data packets received from the headphones (ACK packets are handled inside the link).
    pub rx: Vec<Packet>,
    /// Frames that could not be decoded.
    pub errors: Vec<UnpackError>,
}

/// Stop-and-wait link. One command is on the wire at a time. The headphones stop answering
/// if a data packet is not ACKed, and they expect an ACK before the next command.
///
/// - We send data with our current `seq`; the matching ACK carries `1 - seq`.
/// - For every data packet received with sequence `s`, we reply with an ACK of `1 - s`.
#[derive(Default)]
pub struct Link {
    seq: u8,
    in_flight: bool,
    /// The frame in flight, kept so it can be sent again if no ACK arrives.
    frame: Vec<u8>,
    /// `tick()` calls since the frame in flight was sent.
    age: u8,
    retries: u8,
    queue: VecDeque<(DataType, Vec<u8>)>,
    reader: FrameReader,
}

/// How many `tick()` calls a command may wait for its ACK before it is sent again.
pub const ACK_WAIT_TICKS: u8 = 2;

/// What [`Link::tick`] did.
#[derive(Default, Debug)]
pub struct LinkTick {
    /// Bytes to write now: the same frame again, or the next queued command.
    pub tx: Vec<u8>,
    pub retried: bool,
    /// The command never got an ACK, even after one retry, and was given up on.
    pub dropped: bool,
}

impl Link {
    pub fn new() -> Self {
        Self::default()
    }

    /// Queue a command. Returns the bytes to write now (empty if another command is in flight).
    pub fn send(&mut self, ty: DataType, payload: Vec<u8>) -> Vec<u8> {
        self.queue.push_back((ty, payload));
        self.next_tx()
    }

    /// For debugging a stuck link: (a command is waiting for its ACK, our current seq,
    /// number of commands still queued).
    pub fn status(&self) -> (bool, u8, usize) {
        (self.in_flight, self.seq, self.queue.len())
    }

    pub fn on_bytes(&mut self, bytes: &[u8]) -> LinkOutput {
        let mut out = LinkOutput::default();
        for item in self.reader.push(bytes) {
            match item {
                Ok(p) if p.ty == DataType::Ack => {
                    if self.in_flight && p.seq == 1 - self.seq {
                        self.in_flight = false;
                        self.seq = 1 - self.seq;
                        out.tx.extend(self.next_tx());
                    }
                    // Otherwise it is a stale ACK. Ignore it.
                }
                Ok(p) => {
                    out.tx.extend(pack(DataType::Ack, 1 - (p.seq & 1), &[]));
                    out.rx.push(p);
                }
                Err(e) => out.errors.push(e),
            }
        }
        out
    }

    /// Call at a steady rate (about every 1.5 s). If the command in flight has waited too long
    /// for its ACK, send it once more; if that also fails, drop it so the queue is not stuck
    /// behind it forever. The sequence number is left alone, so the next command reuses it.
    pub fn tick(&mut self) -> LinkTick {
        let mut out = LinkTick::default();
        if !self.in_flight {
            return out;
        }
        self.age += 1;
        if self.age < ACK_WAIT_TICKS {
            return out;
        }
        self.age = 0;
        if self.retries == 0 {
            self.retries = 1;
            out.retried = true;
            out.tx = self.frame.clone();
        } else {
            self.in_flight = false;
            out.dropped = true;
            out.tx = self.next_tx();
        }
        out
    }

    fn next_tx(&mut self) -> Vec<u8> {
        if self.in_flight {
            return Vec::new();
        }
        match self.queue.pop_front() {
            Some((ty, payload)) => {
                self.in_flight = true;
                self.age = 0;
                self.retries = 0;
                self.frame = pack(ty, self.seq, &payload);
                self.frame.clone()
            }
            None => Vec::new(),
        }
    }
}

/// Table 1 command payloads (sent with [`DataType::DataMdr`]) and reply parsing.
pub mod msg {
    pub fn get_protocol_info() -> Vec<u8> {
        vec![0x00, 0x00]
    }

    pub fn get_capability_info() -> Vec<u8> {
        vec![0x02, 0x00]
    }

    #[derive(Copy, Clone, Debug)]
    pub enum DeviceInfoType {
        ModelName = 0x01,
        FwVersion = 0x02,
        SeriesAndColor = 0x03,
    }

    pub fn get_device_info(kind: DeviceInfoType) -> Vec<u8> {
        vec![0x04, kind as u8]
    }

    pub fn get_support_function() -> Vec<u8> {
        vec![0x06, 0x00]
    }

    pub fn get_battery() -> Vec<u8> {
        vec![0x22, 0x00]
    }

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub enum Charging {
        NotCharging,
        Charging,
        Unknown,
        Charged,
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum Event {
        /// `table1` / `table2` are the raw bytes. 0x00 is the "ENABLE" value in the reference code.
        ProtocolInfo { version: i32, table1: u8, table2: u8 },
        DeviceInfo { kind: u8, text: String },
        /// Reply to the series/colour query. `color` is the headphone's own colour code.
        SeriesAndColor { series: u8, color: u8 },
        /// Function ids the headphones support, from the table 1 support-function reply.
        SupportFunctions(Vec<u8>),
        Battery { level: u8, charging: Charging },
        /// Noise cancelling / ambient sound state (NCASM, XM5 "dual mode switch" variant).
        NoiseControl(NoiseControl),
        /// Speak-to-Chat on/off. `preview` is the raw "preview mode" byte, kept so a SET can echo it.
        SpeakToChat { enabled: bool, preview: u8 },
        /// Speak-to-Chat detail: sensitivity (0 auto, 1 high, 2 low) and how long it stays on
        /// (0 fast, 1 medium, 2 slow, 3 never leave).
        SpeakToChatExt { sensitivity: u8, timeout: u8 },
        /// Pause playback when the headphones are taken off.
        WearPause(bool),
        /// Auto power-off timer. See [`auto_off`] for the codes.
        AutoPowerOff { current: u8, last: u8 },
        /// The headphones want to tell the user something before applying a change, for example
        /// "multipoint will briefly disconnect the headphones". `needs_answer` means they wait for
        /// a yes or no (see [`alert_reply`]); otherwise it is only a notice.
        Alert { message: u8, needs_answer: bool },
        /// The auto power-off codes the headphones accept. Other codes are silently ignored.
        AutoPowerOffOptions(Vec<u8>),
        /// 0 prefers sound quality, 1 prefers a stable connection.
        ConnectionMode(u8),
        /// DSEE upscaling set to Auto.
        Dsee(bool),
        /// Equalizer: preset id and the raw band bytes (value + 10 = step).
        Eq { preset: u8, steps: Vec<u8> },
        /// Equalizer capability: band byte count, steps per band, and the preset ids on offer.
        EqCapability { bands: u8, steps: u8, presets: Vec<u8> },
        /// A "general setting" the headphones describe themselves (touch panel, multipoint...).
        /// `key` is the headphones' own label key, for example `MULTIPOINT_SETTING`.
        GsCapability { id: u8, key: String, summary: String },
        GsParam { id: u8, on: bool },
        Unknown(Vec<u8>),
    }

    /// Parse a Table 1 payload received from the headphones.
    pub fn parse(p: &[u8]) -> Event {
        match p {
            [0x01, 0x00, a, b, c, d, t1, t2, ..] => Event::ProtocolInfo {
                version: i32::from_be_bytes([*a, *b, *c, *d]),
                table1: *t1,
                table2: *t2,
            },
            [0x05, 0x03, series, color] => Event::SeriesAndColor { series: *series, color: *color },
            // 0x07 is CONNECT_RET_SUPPORT_FUNCTION: type, count, then (function id, extra byte) pairs.
            [0x07, 0x00, count, rest @ ..] if rest.len() >= *count as usize * 2 => Event::SupportFunctions(
                rest.as_chunks::<2>().0.iter().take(*count as usize).map(|pair| pair[0]).collect(),
            ),
            [0x05, kind, len, rest @ ..] if rest.len() >= *len as usize => Event::DeviceInfo {
                kind: *kind,
                text: String::from_utf8_lossy(&rest[..*len as usize]).into_owned(),
            },
            // 0x23 is POWER_RET_STATUS, 0x25 is POWER_NTFY_STATUS. Same layout for battery.
            [0x23 | 0x25, 0x00, level, charging] => Event::Battery {
                level: *level,
                charging: match charging {
                    0 => Charging::NotCharging,
                    1 => Charging::Charging,
                    3 => Charging::Charged,
                    _ => Charging::Unknown,
                },
            },
            // System table: 0xF7 RET_PARAM, 0xF9 NTFY_PARAM, 0xFB RET_EXT_PARAM, 0xFD NTFY_EXT_PARAM.
            // Note this table says 0x00 = ON and 0x01 = OFF, the opposite of the NC table.
            [0xF7 | 0xF9, SYS_SMART_TALKING2, on, preview, ..] => {
                Event::SpeakToChat { enabled: *on == 0, preview: *preview }
            }
            [0xFB | 0xFD, SYS_SMART_TALKING2, sensitivity, timeout, ..] => {
                Event::SpeakToChatExt { sensitivity: *sensitivity, timeout: *timeout }
            }
            [0xF7 | 0xF9, SYS_WEAR_PAUSE, v, ..] => Event::WearPause(*v == 0),
            // Power table: 0x27 RET_PARAM, 0x29 NTFY_PARAM.
            [0x27 | 0x29, POWER_AUTO_OFF_WEAR, current, last, ..] => {
                Event::AutoPowerOff { current: *current, last: *last }
            }
            // 0x99 is ALERT_NTFY_PARAM for the "fixed message" kind (type 0x00).
            [0x99, 0x00, message, action, ..] => Event::Alert { message: *message, needs_answer: *action == 1 },
            // 0x21 is POWER_RET_CAPABILITY: type, then a count-prefixed list of allowed codes.
            [0x21, POWER_AUTO_OFF_WEAR, count, rest @ ..] if rest.len() >= *count as usize => {
                Event::AutoPowerOffOptions(rest[..*count as usize].to_vec())
            }
            // Audio table: 0xE7 RET_PARAM, 0xE9 NTFY_PARAM.
            [0xE7 | 0xE9, 0x00, v, ..] => Event::ConnectionMode(*v),
            [0xE7 | 0xE9, 0x01, v, ..] => Event::Dsee(*v == 1),
            // EQ table: 0x51 RET_CAPABILITY, 0x57 RET_PARAM, 0x59 NTFY_PARAM.
            [0x57 | 0x59, 0x00, preset, count, rest @ ..] if rest.len() >= *count as usize => {
                Event::Eq { preset: *preset, steps: rest[..*count as usize].to_vec() }
            }
            [0x51, 0x00, bands, steps, count, rest @ ..] => {
                // Each preset is its id followed by a length-prefixed name. This device sends
                // empty names, so the names come from the id table in the app.
                let mut presets = Vec::new();
                let mut r = rest;
                for _ in 0..*count {
                    let [id, len, tail @ ..] = r else { break };
                    if tail.len() < *len as usize {
                        break;
                    }
                    presets.push(*id);
                    r = &tail[*len as usize..];
                }
                Event::EqCapability { bands: *bands, steps: *steps, presets }
            }
            // General settings: 0xD1 RET_CAPABILITY, 0xD7 RET_PARAM, 0xD9 NTFY_PARAM. Boolean type only.
            [0xD1, id @ 0xD1..=0xD4, 0x00, _format, rest @ ..] => match prefixed(rest) {
                Some((key, rest)) => Event::GsCapability {
                    id: *id,
                    key,
                    summary: prefixed(rest).map(|(s, _)| s).unwrap_or_default(),
                },
                None => Event::Unknown(p.to_vec()),
            },
            [0xD7 | 0xD9, id @ 0xD1..=0xD4, 0x00, v] => Event::GsParam { id: *id, on: *v == 0 },
            // 0x67 is NCASM_RET_PARAM, 0x69 is NCASM_NTFY_PARAM.
            [0x67 | 0x69, NC_TYPE_DUAL, _status, total, mode, ambient, level] => {
                Event::NoiseControl(NoiseControl {
                    enabled: *total == 1,
                    ambient: *mode == 1,
                    voice_focus: *ambient == 1,
                    ambient_level: *level,
                })
            }
            other => Event::Unknown(other.to_vec()),
        }
    }

    /// A length-prefixed string. Returns the text and what follows it.
    fn prefixed(b: &[u8]) -> Option<(String, &[u8])> {
        let (&len, rest) = b.split_first()?;
        let len = len as usize;
        (rest.len() >= len).then(|| (String::from_utf8_lossy(&rest[..len]).into_owned(), &rest[len..]))
    }

    const SYS_WEAR_PAUSE: u8 = 0x01;
    const SYS_SMART_TALKING2: u8 = 0x0C;
    const POWER_AUTO_OFF_WEAR: u8 = 0x05;

    /// Auto power-off codes (the "with wearing detection" variant the XM5 uses).
    pub mod auto_off {
        pub const MIN_5: u8 = 0x00;
        pub const MIN_30: u8 = 0x01;
        pub const MIN_60: u8 = 0x02;
        pub const MIN_180: u8 = 0x03;
        pub const MIN_15: u8 = 0x04;
        pub const WHEN_REMOVED: u8 = 0x10;
        pub const DISABLED: u8 = 0x11;
    }

    /// Ask the headphones to send alerts. Without this, changes that need a confirmation, such
    /// as multipoint, are silently ignored instead of being offered to the user.
    pub fn enable_alerts() -> Vec<u8> {
        vec![0x94, 0x00, 0x00]
    }
    /// Answer an alert that `needs_answer`.
    pub fn alert_reply(message: u8, positive: bool) -> Vec<u8> {
        vec![0x98, 0x00, message, positive as u8]
    }

    // ---- reads ----
    pub fn get_speak_to_chat() -> Vec<u8> {
        vec![0xF6, SYS_SMART_TALKING2]
    }
    pub fn get_speak_to_chat_ext() -> Vec<u8> {
        vec![0xFA, SYS_SMART_TALKING2]
    }
    pub fn get_wear_pause() -> Vec<u8> {
        vec![0xF6, SYS_WEAR_PAUSE]
    }
    pub fn get_auto_power_off() -> Vec<u8> {
        vec![0x26, POWER_AUTO_OFF_WEAR]
    }
    /// Which auto power-off settings this device accepts.
    pub fn get_auto_power_off_options() -> Vec<u8> {
        vec![0x20, POWER_AUTO_OFF_WEAR]
    }
    pub fn get_connection_mode() -> Vec<u8> {
        vec![0xE6, 0x00]
    }
    pub fn get_dsee() -> Vec<u8> {
        vec![0xE6, 0x01]
    }
    /// Asks for the preset list in English. This device answers with ids only.
    pub fn get_eq_capability() -> Vec<u8> {
        vec![0x50, 0x00, 0x01]
    }
    pub fn get_eq() -> Vec<u8> {
        vec![0x56, 0x00]
    }
    pub fn get_gs_capability(id: u8) -> Vec<u8> {
        vec![0xD0, id, 0x01]
    }
    pub fn get_gs_param(id: u8) -> Vec<u8> {
        vec![0xD6, id]
    }

    /// Everything worth asking after the support-function list arrives, for the functions this
    /// app implements. Read-only.
    pub fn queries_for(support: &[u8]) -> Vec<Vec<u8>> {
        let has = |id: u8| support.contains(&id);
        let mut q = Vec::new();
        if has(0x90) {
            q.push(enable_alerts());
        }
        if has(0xFC) {
            q.push(get_speak_to_chat());
            q.push(get_speak_to_chat_ext());
        }
        if has(0xF1) {
            q.push(get_wear_pause());
        }
        if has(0x25) {
            q.push(get_auto_power_off_options());
            q.push(get_auto_power_off());
        }
        if has(0xE1) {
            q.push(get_connection_mode());
        }
        if has(0xE2) {
            q.push(get_dsee());
        }
        if has(0x50) {
            q.push(get_eq_capability());
            q.push(get_eq());
        }
        for id in [0xD1u8, 0xD2, 0xD3, 0xD4] {
            if has(id) {
                q.push(get_gs_capability(id));
                q.push(get_gs_param(id));
            }
        }
        q
    }

    // ---- writes ----
    fn on_off(on: bool) -> u8 {
        if on { 0x00 } else { 0x01 }
    }
    /// `preview` is the "preview mode" byte last reported by the headphones.
    pub fn set_speak_to_chat(enabled: bool) -> Vec<u8> {
        // The reference app always sends preview mode OFF (0x01).
        vec![0xF8, SYS_SMART_TALKING2, on_off(enabled), 0x01]
    }
    pub fn set_speak_to_chat_ext(sensitivity: u8, timeout: u8) -> Vec<u8> {
        vec![0xFC, SYS_SMART_TALKING2, sensitivity, timeout]
    }
    pub fn set_wear_pause(on: bool) -> Vec<u8> {
        vec![0xF8, SYS_WEAR_PAUSE, on_off(on)]
    }
    pub fn set_auto_power_off(code: u8) -> Vec<u8> {
        // Like the reference app, "last selected" is always sent as 5 minutes.
        vec![0x28, POWER_AUTO_OFF_WEAR, code, auto_off::MIN_5]
    }
    pub fn set_connection_mode(stable: bool) -> Vec<u8> {
        vec![0xE8, 0x00, stable as u8]
    }
    pub fn set_dsee(auto: bool) -> Vec<u8> {
        vec![0xE8, 0x01, auto as u8]
    }
    /// Pick a preset. The headphones then report that preset's band values.
    pub fn set_eq_preset(preset: u8) -> Vec<u8> {
        vec![0x58, 0x00, preset, 0x00]
    }
    /// Set the five bands (400 Hz, 1k, 2.5k, 6.3k, 16k) and Clear Bass, each -10 to +10.
    pub fn set_eq_bands(preset: u8, clear_bass: i8, bands: [i8; 5]) -> Vec<u8> {
        let step = |v: i8| (v.clamp(-10, 10) + 10) as u8;
        let mut p = vec![0x58, 0x00, preset, 6, step(clear_bass)];
        p.extend(bands.iter().map(|b| step(*b)));
        p
    }
    pub fn set_gs(id: u8, on: bool) -> Vec<u8> {
        vec![0xD8, id, 0x00, on_off(on)]
    }

    /// NCASM inquired type for the XM5: MODE_NC_ASM_DUAL_NC_MODE_SWITCH_AND_ASM_SEAMLESS.
    /// This is the variant the headphones announce with support function 0x6B.
    pub const NC_TYPE_DUAL: u8 = 0x17;

    #[derive(Copy, Clone, Debug, PartialEq, Eq)]
    pub struct NoiseControl {
        /// Noise control as a whole is on. When false, both NC and ambient are off.
        pub enabled: bool,
        /// With `enabled`: false means noise cancelling, true means ambient sound.
        pub ambient: bool,
        /// Ambient sound "focus on voice".
        pub voice_focus: bool,
        /// Ambient sound level, 0 to 20.
        pub ambient_level: u8,
    }

    pub fn get_noise_control() -> Vec<u8> {
        vec![0x66, NC_TYPE_DUAL]
    }

    /// `settled` is false while a slider is still being dragged, true for the final value.
    pub fn set_noise_control(nc: NoiseControl, settled: bool) -> Vec<u8> {
        vec![
            0x68,
            NC_TYPE_DUAL,
            settled as u8,
            nc.enabled as u8,
            nc.ambient as u8,
            nc.voice_focus as u8,
            nc.ambient_level,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_info_request_matches_hand_computed_frame() {
        // type 0x0C, seq 0, len 2, payload 00 00, checksum 0x0C + 0x02 = 0x0E
        let frame = pack(DataType::DataMdr, 0, &msg::get_protocol_info());
        assert_eq!(
            frame,
            [0x3E, 0x0C, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x0E, 0x3C]
        );
    }

    #[test]
    fn ack_frame_matches_hand_computed_frame() {
        let frame = pack(DataType::Ack, 1, &[]);
        assert_eq!(frame, [0x3E, 0x01, 0x01, 0x00, 0x00, 0x00, 0x00, 0x02, 0x3C]);
    }

    #[test]
    fn special_bytes_are_escaped_and_round_trip() {
        let payload = [0x3C, 0x3D, 0x3E, 0x00, 0xFF];
        let frame = pack(DataType::DataMdr, 1, &payload);
        // No raw 0x3C / 0x3E may appear between the markers.
        assert!(!frame[1..frame.len() - 1].iter().any(|&b| b == START || b == END));
        let p = unpack(&frame).unwrap();
        assert_eq!(p.payload, payload);
        assert_eq!(p.seq, 1);
        assert_eq!(p.ty, DataType::DataMdr);
    }

    #[test]
    fn escape_table_is_exact() {
        assert_eq!(escape(&[0x3C, 0x3D, 0x3E]), [0x3D, 0x2C, 0x3D, 0x2D, 0x3D, 0x2E]);
    }

    #[test]
    fn checksum_byte_that_needs_escaping_still_round_trips() {
        // Find a payload byte that makes the checksum 0x3C.
        let target: u8 = 0x3C;
        let base = checksum(&[0x0C, 0x00, 0x00, 0x00, 0x00, 0x01]);
        let b = target.wrapping_sub(base);
        let frame = pack(DataType::DataMdr, 0, &[b]);
        assert_eq!(unpack(&frame).unwrap().payload, [b]);
    }

    #[test]
    fn bad_checksum_is_rejected() {
        let mut frame = pack(DataType::DataMdr, 0, &[1, 2, 3]);
        let n = frame.len();
        frame[n - 2] ^= 0x01;
        assert_eq!(unpack(&frame), Err(UnpackError::BadChecksum));
    }

    #[test]
    fn reader_handles_split_frames_joined_frames_and_garbage() {
        let a = pack(DataType::DataMdr, 0, &[0x23, 0x00, 80, 0]);
        let b = pack(DataType::Ack, 1, &[]);
        let mut all = vec![0xAA, 0xBB]; // garbage before the first frame
        all.extend(&a);
        all.extend(&b);

        let mut r = FrameReader::default();
        let mut got = Vec::new();
        for chunk in all.chunks(3) {
            got.extend(r.push(chunk));
        }
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].as_ref().unwrap().payload, [0x23, 0x00, 80, 0]);
        assert_eq!(got[1].as_ref().unwrap().ty, DataType::Ack);
    }

    #[test]
    fn a_missing_ack_is_retried_once_and_then_the_queue_moves_on() {
        let mut link = Link::new();
        let first = link.send(DataType::DataMdr, msg::get_protocol_info());
        assert!(link.send(DataType::DataMdr, msg::get_battery()).is_empty());

        // No ACK. The first wait passes quietly, the second sends the same frame again.
        assert!(link.tick().tx.is_empty());
        let t = link.tick();
        assert!(t.retried && !t.dropped);
        assert_eq!(t.tx, first);

        // Still nothing: give up on it and release the next command with the same seq.
        assert!(link.tick().tx.is_empty());
        let t = link.tick();
        assert!(t.dropped);
        assert_eq!(t.tx, pack(DataType::DataMdr, 0, &msg::get_battery()));
    }

    #[test]
    fn tick_does_nothing_when_idle_or_acked() {
        let mut link = Link::new();
        assert!(link.tick().tx.is_empty());
        link.send(DataType::DataMdr, msg::get_battery());
        link.on_bytes(&pack(DataType::Ack, 1, &[]));
        for _ in 0..5 {
            let t = link.tick();
            assert!(t.tx.is_empty() && !t.retried && !t.dropped);
        }
    }

    #[test]
    fn link_acks_incoming_data_with_flipped_seq() {
        let mut link = Link::new();
        let incoming = pack(DataType::DataMdr, 0, &[0x01, 0x00]);
        let out = link.on_bytes(&incoming);
        assert_eq!(out.rx.len(), 1);
        assert_eq!(out.tx, pack(DataType::Ack, 1, &[]));

        let incoming = pack(DataType::DataMdr, 1, &[0x01, 0x00]);
        let out = link.on_bytes(&incoming);
        assert_eq!(out.tx, pack(DataType::Ack, 0, &[]));
    }

    #[test]
    fn link_waits_for_ack_before_sending_next_command() {
        let mut link = Link::new();
        let first = link.send(DataType::DataMdr, msg::get_protocol_info());
        assert_eq!(first, pack(DataType::DataMdr, 0, &msg::get_protocol_info()));

        // Second command must wait.
        assert!(link.send(DataType::DataMdr, msg::get_battery()).is_empty());

        // Stale ACK (wrong seq) changes nothing.
        let stale = link.on_bytes(&pack(DataType::Ack, 0, &[]));
        assert!(stale.tx.is_empty());

        // Correct ACK releases the next command, which now uses seq 1.
        let out = link.on_bytes(&pack(DataType::Ack, 1, &[]));
        assert_eq!(out.tx, pack(DataType::DataMdr, 1, &msg::get_battery()));
    }

    #[test]
    fn noise_control_get_and_set_bytes() {
        assert_eq!(msg::get_noise_control(), [0x66, 0x17]);
        let nc = msg::NoiseControl { enabled: true, ambient: true, voice_focus: false, ambient_level: 12 };
        assert_eq!(msg::set_noise_control(nc, true), [0x68, 0x17, 0x01, 0x01, 0x01, 0x00, 12]);
        let off = msg::NoiseControl { enabled: false, ambient: false, voice_focus: false, ambient_level: 0 };
        assert_eq!(msg::set_noise_control(off, false), [0x68, 0x17, 0x00, 0x00, 0x00, 0x00, 0]);
    }

    #[test]
    fn noise_control_reply_and_notify_parse_the_same() {
        for cmd in [0x67u8, 0x69] {
            assert_eq!(
                msg::parse(&[cmd, 0x17, 0x01, 0x01, 0x00, 0x01, 7]),
                msg::Event::NoiseControl(msg::NoiseControl {
                    enabled: true, ambient: false, voice_focus: true, ambient_level: 7
                })
            );
        }
    }

    #[test]
    fn parses_real_xm5_support_function_and_colour_replies() {
        // Captured from a real WH-1000XM5 (firmware 2.5.1) on 2026-10-14.
        let support: Vec<u8> = vec![
            0x07, 0x00, 0x21, 0x10, 0xFF, 0x12, 0xFF, 0x13, 0xFF, 0x14, 0xFF, 0x23, 0xFF, 0x25, 0x21, 0x20,
            0xFF, 0x27, 0xFF, 0xD1, 0x19, 0x32, 0x26, 0x50, 0x0A, 0x6B, 0x06, 0x70, 0x01, 0x90, 0xFF, 0xA1,
            0x0B, 0xC1, 0xFF, 0xC2, 0xFF, 0xE1, 0x0D, 0xE2, 0x0F, 0xF4, 0x14, 0x93, 0xFF, 0x69, 0x1A, 0xD2,
            0x12, 0xF9, 0x27, 0xB0, 0x28, 0xB2, 0x2B, 0xB6, 0x2A, 0xF1, 0x23, 0xFD, 0x1B, 0xFC, 0x09, 0xB8,
            0xFF, 0xB7, 0xFF, 0x19, 0xFF,
        ];
        let msg::Event::SupportFunctions(ids) = msg::parse(&support) else { panic!("not support list") };
        assert_eq!(ids.len(), 33);
        for wanted in [0x20u8, 0x6B, 0x70, 0x50, 0xFC, 0xF1, 0x25, 0xE1, 0x19] {
            assert!(ids.contains(&wanted), "missing function {wanted:#04x}");
        }
        assert_eq!(msg::parse(&[0x05, 0x03, 0x30, 0x01]), msg::Event::SeriesAndColor { series: 0x30, color: 0x01 });
    }

    #[test]
    fn parses_real_xm5_feature_replies() {
        use msg::Event::*;
        // All captured from a real WH-1000XM5 (firmware 2.5.1) on 2026-10-14.
        assert_eq!(msg::parse(&[0xF7, 0x0C, 0x01, 0x01]), SpeakToChat { enabled: false, preview: 1 });
        assert_eq!(msg::parse(&[0xFB, 0x0C, 0x00, 0x01]), SpeakToChatExt { sensitivity: 0, timeout: 1 });
        assert_eq!(msg::parse(&[0xF7, 0x01, 0x00]), WearPause(true));
        assert_eq!(msg::parse(&[0x27, 0x05, 0x10, 0x00]), AutoPowerOff { current: 0x10, last: 0 });
        assert_eq!(msg::parse(&[0xE7, 0x00, 0x00]), ConnectionMode(0));
        assert_eq!(msg::parse(&[0xE7, 0x01, 0x01]), Dsee(true));
        assert_eq!(
            msg::parse(&[0x57, 0x00, 0xA0, 0x06, 0x14, 0x0A, 0x0A, 0x0A, 0x0A, 0x0A]),
            Eq { preset: 0xA0, steps: vec![20, 10, 10, 10, 10, 10] }
        );
        assert_eq!(
            msg::parse(&[
                0x51, 0x00, 0x06, 0x15, 0x0C, 0x00, 0x00, 0x10, 0x00, 0x11, 0x00, 0x12, 0x00, 0x13, 0x00, 0x14,
                0x00, 0x15, 0x00, 0x16, 0x00, 0x17, 0x00, 0xA0, 0x00, 0xA1, 0x00, 0xA2, 0x00
            ]),
            EqCapability {
                bands: 6,
                steps: 21,
                presets: vec![0x00, 0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0xA0, 0xA1, 0xA2]
            }
        );
        let mut touch = vec![0xD1, 0xD1, 0x00, 0x01, 0x13];
        touch.extend(b"TOUCH_PANEL_SETTING");
        touch.push(0x00);
        assert_eq!(
            msg::parse(&touch),
            GsCapability { id: 0xD1, key: "TOUCH_PANEL_SETTING".into(), summary: String::new() }
        );
        assert_eq!(msg::parse(&[0xD7, 0xD2, 0x00, 0x01]), GsParam { id: 0xD2, on: false });
        assert_eq!(msg::parse(&[0xD7, 0xD1, 0x00, 0x00]), GsParam { id: 0xD1, on: true });
    }

    #[test]
    fn alerts_are_enabled_parsed_and_answered() {
        assert_eq!(msg::enable_alerts(), [0x94, 0x00, 0x00]);
        assert!(msg::queries_for(&[0x90]).contains(&vec![0x94, 0x00, 0x00]));
        // 0x07 = "changing multipoint", asks yes/no.
        assert_eq!(msg::parse(&[0x99, 0x00, 0x07, 0x01]), msg::Event::Alert { message: 7, needs_answer: true });
        // Confirmation only: a notice, no answer wanted.
        assert_eq!(msg::parse(&[0x99, 0x00, 0x08, 0x00]), msg::Event::Alert { message: 8, needs_answer: false });
        assert_eq!(msg::alert_reply(7, true), [0x98, 0x00, 0x07, 0x01]);
        assert_eq!(msg::alert_reply(7, false), [0x98, 0x00, 0x07, 0x00]);
    }

    #[test]
    fn parses_the_auto_power_off_capability_layout() {
        // Layout from the reference code: count-prefixed list of allowed codes.
        assert_eq!(
            msg::parse(&[0x21, 0x05, 0x02, 0x10, 0x11]),
            msg::Event::AutoPowerOffOptions(vec![0x10, 0x11])
        );
        assert_eq!(msg::get_auto_power_off_options(), [0x20, 0x05]);
    }

    #[test]
    fn feature_writes_match_reference_bytes() {
        assert_eq!(msg::set_speak_to_chat(true), [0xF8, 0x0C, 0x00, 0x01]);
        assert_eq!(msg::set_speak_to_chat(false), [0xF8, 0x0C, 0x01, 0x01]);
        assert_eq!(msg::set_wear_pause(false), [0xF8, 0x01, 0x01]);
        assert_eq!(msg::set_auto_power_off(msg::auto_off::MIN_30), [0x28, 0x05, 0x01, 0x00]);
        assert_eq!(msg::set_connection_mode(true), [0xE8, 0x00, 0x01]);
        assert_eq!(msg::set_dsee(true), [0xE8, 0x01, 0x01]);
        assert_eq!(msg::set_eq_preset(0x10), [0x58, 0x00, 0x10, 0x00]);
        assert_eq!(
            msg::set_eq_bands(0xA0, 3, [-10, 0, 10, 1, 2]),
            [0x58, 0x00, 0xA0, 6, 13, 0, 10, 20, 11, 12]
        );
        assert_eq!(msg::set_gs(0xD2, true), [0xD8, 0xD2, 0x00, 0x00]);
    }

    #[test]
    fn queries_cover_the_functions_the_xm5_reports() {
        let q = msg::queries_for(&[0x20, 0xFC, 0xF1, 0x25, 0xE1, 0xE2, 0x50, 0xD1, 0xD2]);
        assert_eq!(q.len(), 2 + 1 + 2 + 1 + 1 + 2 + 4);
        assert!(q.contains(&vec![0xD6, 0xD2]));
        assert!(msg::queries_for(&[0x20]).is_empty());
    }

    #[test]
    fn parses_protocol_info_battery_and_device_info() {
        assert_eq!(
            msg::parse(&[0x01, 0x00, 0x00, 0x00, 0x00, 0x05, 0x00, 0x01]),
            msg::Event::ProtocolInfo { version: 5, table1: 0, table2: 1 }
        );
        assert_eq!(
            msg::parse(&[0x23, 0x00, 73, 1]),
            msg::Event::Battery { level: 73, charging: msg::Charging::Charging }
        );
        assert_eq!(
            msg::parse(&[0x05, 0x01, 0x03, b'X', b'M', b'5']),
            msg::Event::DeviceInfo { kind: 1, text: "XM5".into() }
        );
        assert!(matches!(msg::parse(&[0x99]), msg::Event::Unknown(_)));
    }
}
