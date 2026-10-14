//! A bundled app has no terminal, so write a small log file: ~/Library/Logs/XM5Popup.log

use std::io::Write;
use std::time::{SystemTime, UNIX_EPOCH};

pub fn line(msg: impl AsRef<str>) {
    let Some(home) = std::env::var_os("HOME") else { return };
    let path = std::path::Path::new(&home).join("Library/Logs/XM5Popup.log");
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{secs} {}", msg.as_ref());
    }
}
