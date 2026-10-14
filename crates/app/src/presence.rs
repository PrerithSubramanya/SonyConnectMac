//! Bluetooth LE presence. The XM5 advertises as "LE_WH-1000XM5" whenever it is on, whether or
//! not it is connected to this Mac (checked on real hardware). That is what lets the app say
//! "nearby" before any connection exists.

use btleplug::api::{Central, CentralEvent, Manager as _, Peripheral as _, ScanFilter};
use btleplug::platform::Manager;
use futures::StreamExt;

use crate::{engine, log, machine::Input};

pub async fn run() {
    loop {
        if let Err(e) = scan().await {
            log::line(format!("BLE scan stopped: {e}. Retrying in 5s"));
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

async fn scan() -> Result<(), Box<dyn std::error::Error>> {
    let manager = Manager::new().await?;
    let adapter = manager.adapters().await?.into_iter().next().ok_or("no Bluetooth adapter")?;
    let mut events = adapter.events().await?;
    adapter.start_scan(ScanFilter::default()).await?;
    log::line("BLE scan started");

    // Some advertisements leave the name out. Remember the id once we have matched by name.
    let mut known: Option<String> = None;
    while let Some(event) = events.next().await {
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
        let by_name = props.local_name.as_deref().is_some_and(|n| n.contains("WH-1000XM5"));
        if by_name {
            known = Some(id.to_string());
        } else if known.as_deref() != Some(id.to_string().as_str()) {
            continue;
        }
        if let Some(rssi) = props.rssi {
            engine::get().input(Input::BleSeen { rssi });
        }
    }
    Err("BLE event stream ended".into())
}
