# XM5 Popup

A small macOS menu-bar app for the Sony WH-1000XM5. When the headphones come near or connect, a
glass card drops down under the menu-bar icon (like AirPods do), with an animated 3D headphone
and the Sound Connect style controls.

Unofficial. Not made by or affiliated with Sony. Mac only: it cannot pop up on an Android phone.

## What it does

- **Notices the headphones.** Bluetooth LE adverts ("LE_WH-1000XM5") tell the app they are
  switched on and near, even before they connect.
- **Pops up by itself.**
  - Switched on and auto-connects to this Mac: one card, "Connecting" then "Connected".
  - Switched on near the Mac but connected elsewhere: "Nearby", with Connect and Not Now.
  - It stays quiet when you switch the headphones off, and when the app starts with the
    headphones already connected.
- **Controls** (menu-bar icon, left click): battery, noise cancelling / ambient sound with level
  and Focus on voice, Speak-to-Chat, equalizer (presets and 5 bands plus Clear Bass), pause when
  removed, automatic power off, connection quality, DSEE, touch panel, multipoint.
- Only controls that **your headphones report as supported** are shown. The screen always shows
  what the headphones report back, so a refused change reverts on screen.
- The 3D headphone is interactive: drag to turn it (it keeps a little momentum), hover to tilt,
  tap for a pulse, double-click to reset.

## Build and run

Needs Rust (`brew install rustup && rustup default stable`) and the Xcode command line tools.

```bash
export PATH="/opt/homebrew/opt/rustup/bin:$PATH"   # Homebrew's rustup is keg-only
cargo test --workspace                              # 39 tests, no hardware needed
scripts/bundle-app.sh --run                         # builds target/XM5 Popup.app and starts it
```

It must run as an app bundle: macOS only asks for Bluetooth permission for a bundle with
`NSBluetoothAlwaysUsageDescription`. To keep it, copy `target/XM5 Popup.app` to `/Applications` and
add it in System Settings > General > Login Items.

The headphones must already be paired with this Mac (System Settings > Bluetooth).

Log file: `~/Library/Logs/XM5Popup.log` (every command, every write, every reply).

## Layout

| Path | What |
|---|---|
| `crates/mdr-proto` | Sony's wire protocol, pure Rust, fully unit tested (framing, ACK/retry link, messages) |
| `crates/app` | The Tauri app: Bluetooth (IOBluetooth), BLE presence, popup state machine, windows |
| `ui/` | The popover page (HTML/CSS/JS) and the three.js headphone |
| `crates/probe`, `crates/blescan` | Small hardware tools used to learn what the XM5 does |
| `scripts/` | `bundle-app.sh` (build and sign the app), `make-icons.py` |

## Known limits (honest list)

- **Adaptive Sound Control is not included.** The reference project does not implement it and its
  bytes could not be verified.
- **Writes verified live on a real WH-1000XM5** (the headphones echoed each change back):
  noise control, ambient level, Speak-to-Chat, DSEE, equalizer presets and bands, connection
  quality, automatic power off (the two choices the device offers), and multipoint (the headphones
  ask for confirmation, the app shows the question and sends your answer).
- Some changes make the headphones ask a question first (for example multipoint: "this will briefly
  disconnect the headphones"). The popover shows it over the card, and nothing changes until you
  tap Continue.
- Automatic power off shows only the choices the headphones accept. The XM5 accepts "When removed
  from ears" and "Never". Timed choices are silently ignored by the device, so they are not shown.
- Firmware update and factory reset are deliberately left out.
- The 3D headphone is an original procedural model, not a scan or photo. It was matched by eye to
  reference views of the product, with no Sony model, photo, texture or logo in the app.
- The app is signed ad hoc with a fixed requirement (no developer certificate on this Mac). If
  macOS asks for Bluetooth permission again after a rebuild, allow it.
- Multipoint (System tab) must be ON for the Mac to connect while the phone holds the headphones.
  Switching it makes the headphones reconnect for a second. It is on now on the test unit.

## Contributing

Issues and pull requests are welcome. Please run `cargo test --workspace` and
`cargo clippy --workspace --all-targets` before sending a change. If you have another Sony headphone
and want to help, the `probe` tool (`scripts/run-bundled.sh probe --cmd "F6 0C"`) prints raw replies,
which is how the XM5 support was worked out. Please do not add Sony logos, photos or 3D models.

## Licence and credits

MIT. See `LICENSE`. Third-party work and trademarks are listed in `NOTICE`.
