//! Presence probe: does the WH-1000XM5 advertise over Bluetooth LE, and in which states?
//!
//! Scans for a while and prints a summary of every device seen, with the Sony-looking ones
//! marked. Run it in three headphone states and compare:
//!   A) headphones connected to this Mac
//!   B) headphones connected to the phone only
//!   C) headphones just powered on (not connected to anything yet)
//!
//! Must run from a signed .app bundle (see scripts/run-bundled.sh), like the RFCOMM probe.
//!
//! Usage: blescan [--secs 30]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use btleplug::api::{Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use futures::StreamExt;

const SONY_COMPANY_ID: u16 = 0x012D;
const FAST_PAIR: &str = "0000fe2c-0000-1000-8000-00805f9b34fb";
const MDR_BLE: &str = "5b833e20-6bc7-4802-8e9a-723ceca4bd8f";

#[derive(Default)]
struct Seen {
    name: Option<String>,
    best_rssi: Option<i16>,
    events: u32,
    mfr_ids: Vec<u16>,
    /// "company:first bytes" for each manufacturer data entry.
    mfr: Vec<String>,
    services: Vec<String>,
    /// "uuid:first bytes" for each service data entry.
    service_data: Vec<String>,
}

fn hex(b: &[u8]) -> String {
    b.iter().take(8).map(|x| format!("{x:02X}")).collect()
}

fn why_interesting(s: &Seen) -> Vec<&'static str> {
    let mut why = Vec::new();
    if s.mfr_ids.contains(&SONY_COMPANY_ID) {
        why.push("sony-company-id");
    }
    if s.name.as_deref().is_some_and(|n| n.contains("1000XM") || n.to_uppercase().contains("SONY")) {
        why.push("sony-name");
    }
    if s.services.iter().any(|u| u == FAST_PAIR) {
        why.push("fast-pair");
    }
    if s.services.iter().any(|u| u == MDR_BLE) {
        why.push("sony-mdr-ble-service");
    }
    why
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let secs: u64 = args
        .iter()
        .position(|a| a == "--secs")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.parse().ok())
        .unwrap_or(30);

    let manager = Manager::new().await?;
    let Some(adapter) = manager.adapters().await?.into_iter().next() else {
        println!("BLE: FAIL no Bluetooth adapter");
        return Ok(());
    };
    let mut events = adapter.events().await?;
    adapter.start_scan(ScanFilter::default()).await?;
    println!("BLE: scanning for {secs}s ...");

    let mut seen: HashMap<String, Seen> = HashMap::new();
    let deadline = Instant::now() + Duration::from_secs(secs);
    while let Ok(Some(event)) = tokio::time::timeout(
        deadline.saturating_duration_since(Instant::now()),
        events.next(),
    )
    .await
    {
        let id = match event {
            CentralEvent::DeviceDiscovered(id)
            | CentralEvent::DeviceUpdated(id)
            | CentralEvent::ManufacturerDataAdvertisement { id, .. }
            | CentralEvent::ServiceDataAdvertisement { id, .. }
            | CentralEvent::ServicesAdvertisement { id, .. } => id,
            _ => continue,
        };
        let Ok(peripheral) = adapter.peripheral(&id).await else { continue };
        let Ok(Some(props)) = peripheral.properties().await else { continue };
        let entry = seen.entry(id.to_string()).or_default();
        entry.events += 1;
        if props.local_name.is_some() {
            entry.name = props.local_name;
        }
        if let Some(r) = props.rssi {
            entry.best_rssi = Some(entry.best_rssi.map_or(r, |b| b.max(r)));
        }
        for (k, v) in &props.manufacturer_data {
            if !entry.mfr_ids.contains(k) {
                entry.mfr_ids.push(*k);
            }
            let item = format!("{k:04X}:{}", hex(v));
            if !entry.mfr.contains(&item) {
                entry.mfr.push(item);
            }
        }
        for u in props.services.iter().map(|u| u.to_string()) {
            if !entry.services.contains(&u) {
                entry.services.push(u);
            }
        }
        for (u, v) in &props.service_data {
            let item = format!("{u}:{}", hex(v));
            if !entry.service_data.contains(&item) {
                entry.service_data.push(item);
            }
            let u = u.to_string();
            if !entry.services.contains(&u) {
                entry.services.push(u);
            }
        }
    }
    adapter.stop_scan().await?;

    println!("BLE: ---- summary: {} devices seen ----", seen.len());
    let mut rows: Vec<_> = seen.iter().collect();
    rows.sort_by_key(|(_, s)| std::cmp::Reverse(s.best_rssi.unwrap_or(-127)));
    let mut sony_hits = 0;
    for (id, s) in rows {
        let why = why_interesting(s);
        if !why.is_empty() {
            sony_hits += 1;
        }
        println!(
            "BLE: {id} name={:?} rssi={:?} events={} mfr={:?} services={:?} service_data={:?} {}",
            s.name,
            s.best_rssi,
            s.events,
            s.mfr,
            s.services,
            s.service_data,
            if why.is_empty() { String::new() } else { format!("<== {}", why.join(",")) },
        );
    }
    println!(
        "BLE: RESULT {}",
        if sony_hits > 0 { format!("{sony_hits} Sony-looking advertiser(s) seen") } else { "no Sony-looking advertiser seen".into() }
    );
    Ok(())
}
