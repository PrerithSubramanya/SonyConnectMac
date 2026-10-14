//! Decides when the popup shows, and what phase the headphones are in.
//!
//! Pure logic: no Bluetooth, no windows, no clock of its own. The engine feeds it [`Input`]s
//! and carries out the [`Effect`]s it returns. That keeps every popup rule testable.

use std::time::{Duration, Instant};

use serde::Serialize;

/// "Close enough to pop up" RSSI, like AirPods needing the case near the phone.
const NEAR_ENTER_DBM: f32 = -68.0;
/// Once near, stay near until the signal drops below this (hysteresis stops flicker).
const NEAR_LEAVE_DBM: f32 = -80.0;
/// No advertisement for this long means the headphones are gone.
const SEEN_TIMEOUT: Duration = Duration::from_secs(8);
/// The headphones must stay near this long before the "nearby" card shows. Headphones that
/// were just switched on often connect to this Mac by themselves a second or two later, and
/// then the "connected" card is the right one to show, not a flash of "nearby".
const NEAR_SETTLE: Duration = Duration::from_millis(2500);
/// If the link is up but the headphones' control channel has not answered after this long,
/// say so instead of showing "Connecting" forever.
const CONNECT_PATIENCE: Duration = Duration::from_secs(25);
/// After "Not Now" the popup stays quiet until the headphones have been away this long.
const AWAY_RESET: Duration = Duration::from_secs(20);

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Phase {
    Absent,
    Nearby,
    Connecting,
    Connected,
    Error,
}

#[derive(Clone, Debug)]
pub enum Input {
    /// A BLE advertisement from the headphones was seen.
    BleSeen { rssi: i16 },
    /// Once a second.
    Tick,
    /// Result of asking macOS whether the headphones are connected to this Mac.
    MacLink(bool),
    /// The MDR channel is open and the first battery/noise-control data has arrived.
    MdrReady,
    UserConnect,
    UserDismiss,
    ConnectFailed(String),
    /// The user clicked the menu-bar icon to open the popover.
    OpenFromTray,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Announce the headphones: the popup hides itself and takes no keyboard focus.
    ShowPopup,
    /// Opened by the user from the menu bar: stays until clicked away.
    ShowPopupInteractive,
    HidePopup,
    /// Ask macOS to open a Bluetooth link to the headphones.
    StartConnect,
    OpenMdr,
    CloseMdr,
    /// The phase, error or popup state changed. Tell the UI.
    Publish,
}

pub struct Machine {
    pub phase: Phase,
    pub error: Option<String>,
    pub rssi: Option<i16>,
    popup_visible: bool,
    last_seen: Option<Instant>,
    rssi_ema: f32,
    near: bool,
    /// When the headphones last came near (cleared when they leave).
    near_since: Option<Instant>,
    /// The "nearby" card was already shown for this visit.
    announced: bool,
    /// When the phase last became Connecting.
    connecting_since: Option<Instant>,
    /// None until macOS has told us once. The first answer never triggers a popup.
    mac_linked: Option<bool>,
    mdr_ready: bool,
    dismissed: bool,
    away_since: Option<Instant>,
}

impl Machine {
    pub fn new() -> Self {
        Self {
            phase: Phase::Absent,
            error: None,
            rssi: None,
            popup_visible: false,
            last_seen: None,
            rssi_ema: -100.0,
            near: false,
            near_since: None,
            announced: false,
            connecting_since: None,
            mac_linked: None,
            mdr_ready: false,
            dismissed: false,
            away_since: None,
        }
    }

    pub fn popup_visible(&self) -> bool {
        self.popup_visible
    }

    pub fn step(&mut self, input: Input, now: Instant) -> Vec<Effect> {
        let before = (self.phase, self.popup_visible, self.error.clone());
        let mut fx = Vec::new();

        match input {
            Input::BleSeen { rssi } => {
                self.last_seen = Some(now);
                self.rssi = Some(rssi);
                let r = rssi as f32;
                self.rssi_ema = if self.rssi_ema <= -99.0 { r } else { self.rssi_ema * 0.6 + r * 0.4 };
            }
            Input::Tick => self.tick(now, &mut fx),
            Input::MacLink(linked) => self.on_link(linked, now, &mut fx),
            Input::MdrReady => {
                self.mdr_ready = true;
                if self.mac_linked == Some(true) {
                    let newly_connected = self.phase != Phase::Connected;
                    self.phase = Phase::Connected;
                    self.error = None;
                    // Headphones that connected by themselves: announce them. A user-started
                    // connect already has the popup open.
                    if newly_connected && !self.popup_visible && !self.dismissed {
                        self.popup_visible = true;
                        fx.push(Effect::ShowPopup);
                    }
                }
            }
            Input::UserConnect => {
                self.error = None;
                self.phase = Phase::Connecting;
                self.connecting_since = Some(now);
                if self.mac_linked == Some(true) {
                    fx.push(Effect::OpenMdr);
                } else {
                    fx.push(Effect::StartConnect);
                }
            }
            Input::UserDismiss => {
                self.dismissed = true;
                if self.popup_visible {
                    self.popup_visible = false;
                    fx.push(Effect::HidePopup);
                }
                if matches!(self.phase, Phase::Error | Phase::Connecting) && self.mac_linked != Some(true) {
                    self.phase = if self.near { Phase::Nearby } else { Phase::Absent };
                }
            }
            Input::ConnectFailed(msg) => {
                self.phase = Phase::Error;
                self.error = Some(msg);
                if !self.popup_visible {
                    self.popup_visible = true;
                    fx.push(Effect::ShowPopup);
                }
            }
            Input::OpenFromTray => {
                self.popup_visible = true;
                fx.push(Effect::ShowPopupInteractive);
            }
        }

        if before != (self.phase, self.popup_visible, self.error.clone()) || !fx.is_empty() {
            fx.push(Effect::Publish);
        }
        fx
    }

    fn tick(&mut self, now: Instant, fx: &mut Vec<Effect>) {
        if self.phase == Phase::Connecting
            && self.mac_linked == Some(true)
            && self.connecting_since.is_some_and(|t| now.duration_since(t) >= CONNECT_PATIENCE)
        {
            self.phase = Phase::Error;
            self.error = Some(
                "Connected to this Mac, but the headphone controls did not answer. Try switching the headphones off and on, then Try Again.".into(),
            );
            if !self.popup_visible {
                self.popup_visible = true;
                fx.push(Effect::ShowPopup);
            }
        }
        let fresh = self.last_seen.is_some_and(|t| now.duration_since(t) < SEEN_TIMEOUT);
        let was_near = self.near;
        self.near = if !fresh {
            false
        } else if was_near {
            self.rssi_ema >= NEAR_LEAVE_DBM
        } else {
            self.rssi_ema >= NEAR_ENTER_DBM
        };

        if self.near && !was_near {
            self.near_since = Some(now);
        }
        if !self.near {
            self.near_since = None;
            self.announced = false;
        }
        if self.near {
            self.away_since = None;
        } else {
            let since = *self.away_since.get_or_insert(now);
            if self.dismissed && now.duration_since(since) >= AWAY_RESET {
                self.dismissed = false;
            }
        }

        // Nearby/Absent only matter while not connected to this Mac. Until macOS has answered
        // once, we do not know yet, so stay quiet instead of flashing a wrong popup.
        if self.mac_linked != Some(false) {
            return;
        }

        if self.near {
            let settled = self.near_since.is_some_and(|t| now.duration_since(t) >= NEAR_SETTLE);
            if settled {
                if self.phase == Phase::Absent {
                    self.phase = Phase::Nearby;
                }
                if !self.announced && !self.dismissed && !self.popup_visible {
                    self.announced = true;
                    self.popup_visible = true;
                    fx.push(Effect::ShowPopup);
                }
            }
        } else if was_near && matches!(self.phase, Phase::Nearby | Phase::Error) {
            self.phase = Phase::Absent;
            if self.popup_visible {
                self.popup_visible = false;
                fx.push(Effect::HidePopup);
            }
        }
    }

    fn on_link(&mut self, linked: bool, now: Instant, fx: &mut Vec<Effect>) {
        let previous = self.mac_linked;
        if previous == Some(linked) {
            return;
        }
        self.mac_linked = Some(linked);

        if linked {
            self.error = None;
            self.phase = Phase::Connecting;
            self.connecting_since = Some(now);
            fx.push(Effect::OpenMdr);
            if previous.is_none() {
                // The headphones were already connected when the app started. Stay quiet.
                self.dismissed = true;
            } else if !self.popup_visible && !self.dismissed {
                // They just connected by themselves. Show the card at once as "Connecting",
                // so there is feedback even if the control channel is slow to answer.
                self.popup_visible = true;
                fx.push(Effect::ShowPopup);
            }
        } else {
            self.mdr_ready = false;
            fx.push(Effect::CloseMdr);
            self.phase = if self.near { Phase::Nearby } else { Phase::Absent };
            if self.popup_visible && self.phase == Phase::Absent {
                self.popup_visible = false;
                fx.push(Effect::HidePopup);
            }
            // A disconnect should not leave the popup muted for the next power-on.
            self.dismissed = false;
            // Headphones that were just switched off keep advertising for a few seconds. Do
            // not announce them as "nearby" for that. The flag resets once they are away.
            if previous == Some(true) {
                self.announced = true;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(secs: u64, base: Instant) -> Instant {
        base + Duration::from_secs(secs)
    }

    fn has(fx: &[Effect], e: Effect) -> bool {
        fx.contains(&e)
    }

    /// Headphones are seen and stay near for a while, so the "nearby" card is due.
    fn settle_near(m: &mut Machine, from: u64, t0: Instant) -> Vec<Effect> {
        let mut all = Vec::new();
        for s in from..from + 4 {
            m.step(Input::BleSeen { rssi: -50 }, t(s, t0));
            all.extend(m.step(Input::Tick, t(s, t0)));
        }
        all
    }

    #[test]
    fn popup_shows_once_when_headphones_stay_near_and_are_not_linked() {
        let t0 = Instant::now();
        let mut m = Machine::new();
        m.step(Input::MacLink(false), t0);
        let fx = settle_near(&mut m, 1, t0);
        assert_eq!(fx.iter().filter(|e| **e == Effect::ShowPopup).count(), 1);
        assert_eq!(m.phase, Phase::Nearby);
        // Still near on later ticks: no second popup.
        m.step(Input::BleSeen { rssi: -50 }, t(8, t0));
        assert!(!has(&m.step(Input::Tick, t(8, t0)), Effect::ShowPopup));
    }

    #[test]
    fn a_brief_sighting_does_not_pop_up() {
        let t0 = Instant::now();
        let mut m = Machine::new();
        m.step(Input::MacLink(false), t0);
        m.step(Input::BleSeen { rssi: -50 }, t(1, t0));
        assert!(!has(&m.step(Input::Tick, t(1, t0)), Effect::ShowPopup));
        assert!(!has(&m.step(Input::Tick, t(2, t0)), Effect::ShowPopup));
        assert_eq!(m.phase, Phase::Absent);
    }

    #[test]
    fn headphones_that_auto_connect_right_after_power_on_skip_the_nearby_card() {
        let t0 = Instant::now();
        let mut m = Machine::new();
        m.step(Input::MacLink(false), t0);
        // Powered on: the BLE advert is seen first, the Mac connects a second later.
        m.step(Input::BleSeen { rssi: -50 }, t(1, t0));
        assert!(!has(&m.step(Input::Tick, t(1, t0)), Effect::ShowPopup));
        // One card, shown at once as "Connecting", never a "nearby" card first.
        let fx = m.step(Input::MacLink(true), t(2, t0));
        assert!(has(&fx, Effect::ShowPopup));
        assert_eq!(m.phase, Phase::Connecting);
        assert!(!has(&m.step(Input::Tick, t(3, t0)), Effect::ShowPopup));
        // When the control channel is ready it turns into the connected card, same popup.
        let fx = m.step(Input::MdrReady, t(4, t0));
        assert!(!has(&fx, Effect::ShowPopup));
        assert_eq!(m.phase, Phase::Connected);
    }

    #[test]
    fn far_headphones_do_not_pop_up() {
        let t0 = Instant::now();
        let mut m = Machine::new();
        m.step(Input::MacLink(false), t0);
        for s in 1..8 {
            m.step(Input::BleSeen { rssi: -85 }, t(s, t0));
            assert!(!has(&m.step(Input::Tick, t(s, t0)), Effect::ShowPopup));
        }
        assert_eq!(m.phase, Phase::Absent);
    }

    #[test]
    fn not_now_stays_quiet_until_headphones_have_been_away() {
        let t0 = Instant::now();
        let mut m = Machine::new();
        m.step(Input::MacLink(false), t0);
        settle_near(&mut m, 1, t0);
        let fx = m.step(Input::UserDismiss, t(6, t0));
        assert!(has(&fx, Effect::HidePopup));

        // Signal flickers away and back: still dismissed, no popup.
        m.step(Input::Tick, t(20, t0)); // not seen for >8s, near = false
        let fx = settle_near(&mut m, 21, t0);
        assert!(!has(&fx, Effect::ShowPopup));

        // Away long enough, then back for a while: popup returns.
        m.step(Input::Tick, t(40, t0));
        m.step(Input::Tick, t(70, t0));
        let fx = settle_near(&mut m, 71, t0);
        assert!(has(&fx, Effect::ShowPopup));
    }

    #[test]
    fn user_connect_flow_keeps_the_same_popup() {
        let t0 = Instant::now();
        let mut m = Machine::new();
        m.step(Input::MacLink(false), t0);
        settle_near(&mut m, 1, t0);

        let fx = m.step(Input::UserConnect, t(6, t0));
        assert!(has(&fx, Effect::StartConnect));
        assert_eq!(m.phase, Phase::Connecting);

        let fx = m.step(Input::MacLink(true), t(7, t0));
        assert!(has(&fx, Effect::OpenMdr));

        let fx = m.step(Input::MdrReady, t(8, t0));
        assert_eq!(m.phase, Phase::Connected);
        assert!(!has(&fx, Effect::ShowPopup), "popup is already open");
    }

    #[test]
    fn headphones_that_connect_by_themselves_get_a_connected_popup() {
        let t0 = Instant::now();
        let mut m = Machine::new();
        m.step(Input::MacLink(false), t0); // app started with headphones off
        let fx = m.step(Input::MacLink(true), t(5, t0)); // powered on, auto-connected
        assert!(has(&fx, Effect::ShowPopup));
        let fx = m.step(Input::MdrReady, t(6, t0));
        assert!(!has(&fx, Effect::ShowPopup), "already showing");
        assert_eq!(m.phase, Phase::Connected);
    }

    #[test]
    fn switching_the_headphones_off_does_not_announce_them_as_nearby() {
        let t0 = Instant::now();
        let mut m = Machine::new();
        m.step(Input::MacLink(true), t0);
        m.step(Input::MdrReady, t(1, t0));
        settle_near(&mut m, 2, t0); // BLE adverts keep coming
        // The headphones are switched off: the link drops, but the adverts linger a few seconds.
        let fx = m.step(Input::MacLink(false), t(6, t0));
        assert!(!has(&fx, Effect::ShowPopup));
        let fx = settle_near(&mut m, 7, t0);
        assert!(!has(&fx, Effect::ShowPopup), "no ghost popup while the signal fades");
    }

    #[test]
    fn a_control_channel_that_never_answers_becomes_an_error() {
        let t0 = Instant::now();
        let mut m = Machine::new();
        m.step(Input::MacLink(false), t0);
        m.step(Input::MacLink(true), t(1, t0));
        assert_eq!(m.phase, Phase::Connecting);
        m.step(Input::Tick, t(10, t0));
        assert_eq!(m.phase, Phase::Connecting, "still patient");
        m.step(Input::Tick, t(30, t0));
        assert_eq!(m.phase, Phase::Error);
        assert!(m.error.as_deref().unwrap_or("").contains("did not answer"));
        // Try Again re-opens the control channel.
        let fx = m.step(Input::UserConnect, t(31, t0));
        assert!(has(&fx, Effect::OpenMdr));
    }

    #[test]
    fn already_connected_at_startup_stays_quiet() {
        let t0 = Instant::now();
        let mut m = Machine::new();
        m.step(Input::MacLink(true), t0);
        let fx = m.step(Input::MdrReady, t(1, t0));
        assert!(!has(&fx, Effect::ShowPopup));
        assert_eq!(m.phase, Phase::Connected);
    }

    #[test]
    fn no_popup_flash_at_startup_when_headphones_are_already_connected() {
        let t0 = Instant::now();
        let mut m = Machine::new();
        // Ticks run before macOS has answered whether the headphones are connected.
        let fx = settle_near(&mut m, 1, t0);
        assert!(!has(&fx, Effect::ShowPopup), "must stay quiet while the link state is unknown");
        let fx = m.step(Input::MacLink(true), t(6, t0));
        assert!(!has(&fx, Effect::ShowPopup));
        let fx = m.step(Input::MdrReady, t(7, t0));
        assert!(!has(&fx, Effect::ShowPopup));
        assert_eq!(m.phase, Phase::Connected);
    }

    #[test]
    fn near_but_not_connected_at_startup_is_announced_once() {
        let t0 = Instant::now();
        let mut m = Machine::new();
        settle_near(&mut m, 1, t0); // link unknown: quiet
        let fx = m.step(Input::MacLink(false), t(5, t0));
        assert!(!has(&fx, Effect::ShowPopup));
        m.step(Input::BleSeen { rssi: -50 }, t(6, t0));
        let fx = m.step(Input::Tick, t(6, t0));
        assert!(has(&fx, Effect::ShowPopup));
        assert_eq!(m.phase, Phase::Nearby);
        m.step(Input::BleSeen { rssi: -50 }, t(7, t0));
        assert!(!has(&m.step(Input::Tick, t(7, t0)), Effect::ShowPopup));
    }

    #[test]
    fn failed_connect_shows_error_and_retry_works() {
        let t0 = Instant::now();
        let mut m = Machine::new();
        m.step(Input::MacLink(false), t0);
        settle_near(&mut m, 1, t0);
        m.step(Input::UserConnect, t(6, t0));
        m.step(Input::ConnectFailed("busy".into()), t(7, t0));
        assert_eq!(m.phase, Phase::Error);
        assert_eq!(m.error.as_deref(), Some("busy"));
        let fx = m.step(Input::UserConnect, t(8, t0));
        assert!(has(&fx, Effect::StartConnect));
        assert_eq!(m.phase, Phase::Connecting);
        assert!(m.error.is_none());
    }

    #[test]
    fn menu_bar_click_opens_interactively_even_when_nothing_is_near() {
        let t0 = Instant::now();
        let mut m = Machine::new();
        let fx = m.step(Input::OpenFromTray, t0);
        assert!(has(&fx, Effect::ShowPopupInteractive));
        assert!(m.popup_visible());
        let fx = m.step(Input::UserDismiss, t(1, t0));
        assert!(has(&fx, Effect::HidePopup));
        assert!(!m.popup_visible());
    }

    #[test]
    fn disconnect_closes_mdr_and_hides_popup_when_headphones_are_gone() {
        let t0 = Instant::now();
        let mut m = Machine::new();
        m.step(Input::MacLink(true), t0);
        m.step(Input::MdrReady, t(1, t0));
        m.step(Input::OpenFromTray, t(2, t0));
        let fx = m.step(Input::MacLink(false), t(3, t0));
        assert!(has(&fx, Effect::CloseMdr));
        assert!(has(&fx, Effect::HidePopup));
        assert_eq!(m.phase, Phase::Absent);
    }
}
