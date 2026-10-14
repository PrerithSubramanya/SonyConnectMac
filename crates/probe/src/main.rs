#![allow(non_snake_case)]
//! Hardware probe for the Sony WH-1000XM5.
//!
//! Finds the paired headphones, lists the SDP services it can see, opens the MDR RFCOMM
//! channel, runs the init handshake and prints everything that comes back.
//!
//! It must run from a signed .app bundle with NSBluetoothAlwaysUsageDescription (see
//! scripts/run-bundled.sh), otherwise macOS blocks or mis-attributes the Bluetooth permission.
//!
//! Usage: probe [--name WH-1000XM5] [--connect]
//!   --connect   open a Bluetooth link first if the headphones are not connected.

use std::cell::RefCell;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use mdr_proto::{msg, DataType, Link, SERVICE_UUID_LEGACY, SERVICE_UUID_V2};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, AnyThread, DefinedClass, Message};
use objc2_foundation::{NSDate, NSObject, NSObjectProtocol, NSRunLoop};
use objc2_io_bluetooth::{
    IOBluetoothDevice, IOBluetoothRFCOMMChannel, IOBluetoothRFCOMMChannelDelegate,
    IOBluetoothSDPServiceRecord, IOBluetoothSDPUUID,
};

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ")
}

#[derive(Default)]
struct State {
    link: Link,
    channel: Option<Retained<IOBluetoothRFCOMMChannel>>,
    open_status: Option<i32>,
    closed: bool,
    commands_queued: bool,
    battery: Option<(u8, msg::Charging)>,
    device_info: Vec<(u8, String)>,
    /// Extra read-only table 1 payloads from `--cmd "F6 0C"`, sent after the handshake.
    extra: Vec<Vec<u8>>,
}

#[derive(Default)]
struct Ivars {
    st: RefCell<State>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and Delegate has no Drop.
    #[unsafe(super = NSObject)]
    #[ivars = Ivars]
    struct Delegate;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: the signatures match the protocol declaration.
    unsafe impl IOBluetoothRFCOMMChannelDelegate for Delegate {
        #[unsafe(method(rfcommChannelOpenComplete:status:))]
        unsafe fn rfcommChannelOpenComplete_status(
            &self,
            channel: Option<&IOBluetoothRFCOMMChannel>,
            error: i32,
        ) {
            println!("PROBE: rfcommChannelOpenComplete status=0x{error:x}");
            let tx = {
                let mut st = self.ivars().st.borrow_mut();
                st.open_status = Some(error);
                if error != 0 {
                    return;
                }
                if st.channel.is_none() {
                    st.channel = channel.map(|c| c.retain());
                }
                // First command of the handshake. The rest is queued after the reply.
                st.link.send(DataType::DataMdr, msg::get_protocol_info())
            };
            self.write(&tx);
        }

        #[unsafe(method(rfcommChannelData:data:length:))]
        unsafe fn rfcommChannelData_data_length(
            &self,
            _channel: Option<&IOBluetoothRFCOMMChannel>,
            data: *mut c_void,
            length: usize,
        ) {
            // SAFETY: IOBluetooth gives a valid buffer of `length` bytes for this call.
            let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, length) };
            self.handle_rx(bytes);
        }

        #[unsafe(method(rfcommChannelClosed:))]
        unsafe fn rfcommChannelClosed(&self, _channel: Option<&IOBluetoothRFCOMMChannel>) {
            println!("PROBE: rfcommChannelClosed");
            self.ivars().st.borrow_mut().closed = true;
        }
    }
);

impl Delegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars::default());
        unsafe { msg_send![super(this), init] }
    }

    fn handle_rx(&self, bytes: &[u8]) {
        println!("PROBE: RX raw {} bytes: {}", bytes.len(), hex(bytes));
        let mut tx = Vec::new();
        {
            let mut st = self.ivars().st.borrow_mut();
            let out = st.link.on_bytes(bytes);
            tx.extend(out.tx);
            for e in &out.errors {
                println!("PROBE: RX decode error {e:?}");
            }
            for p in out.rx {
                println!("PROBE: RX {:?} seq={} payload={}", p.ty, p.seq, hex(&p.payload));
                if p.ty != DataType::DataMdr {
                    continue;
                }
                let event = msg::parse(&p.payload);
                println!("PROBE:    -> {event:?}");
                match event {
                    msg::Event::ProtocolInfo { .. } if !st.commands_queued => {
                        st.commands_queued = true;
                        for payload in [
                            msg::get_capability_info(),
                            msg::get_device_info(msg::DeviceInfoType::ModelName),
                            msg::get_device_info(msg::DeviceInfoType::FwVersion),
                            msg::get_device_info(msg::DeviceInfoType::SeriesAndColor),
                            msg::get_support_function(),
                            msg::get_battery(),
                        ]
                        .into_iter()
                        .chain(st.extra.clone())
                        {
                            tx.extend(st.link.send(DataType::DataMdr, payload));
                        }
                    }
                    msg::Event::Battery { level, charging } => st.battery = Some((level, charging)),
                    msg::Event::DeviceInfo { kind, text } => st.device_info.push((kind, text)),
                    _ => {}
                }
            }
        }
        self.write(&tx);
    }

    /// Write bytes to the channel. The RefCell borrow is released before this is called, because
    /// IOBluetooth may call back into the delegate while the write runs.
    fn write(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let channel = self.ivars().st.borrow().channel.clone();
        let Some(channel) = channel else {
            println!("PROBE: no channel yet, cannot write");
            return;
        };
        println!("PROBE: TX {}", hex(bytes));
        let mut buf = bytes.to_vec();
        // SAFETY: `buf` is valid for the whole synchronous call.
        let r = unsafe { channel.writeSync_length(buf.as_mut_ptr() as *mut c_void, buf.len() as u16) };
        if r != 0 {
            println!("PROBE: writeSync failed 0x{r:x}");
        }
    }
}

fn pump(secs: f64) {
    NSRunLoop::currentRunLoop().runUntilDate(&NSDate::dateWithTimeIntervalSinceNow(secs));
}

fn uuid_from_str(s: &str) -> Retained<IOBluetoothSDPUUID> {
    let h: String = s.chars().filter(|c| *c != '-').collect();
    let bytes: Vec<u8> = (0..16).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap()).collect();
    unsafe { IOBluetoothSDPUUID::uuidWithBytes_length(bytes.as_ptr() as *const c_void, 16) }
        .expect("valid 16 byte uuid")
}

fn find_device(name_part: &str) -> Option<Retained<IOBluetoothDevice>> {
    let list = unsafe { IOBluetoothDevice::pairedDevices() }?;
    for item in list.iter() {
        let Ok(dev) = item.downcast::<IOBluetoothDevice>() else { continue };
        let name = unsafe { dev.name() }.to_string();
        let addr = unsafe { dev.addressString() }.map(|s| s.to_string()).unwrap_or_default();
        println!("PROBE: paired device: {name} [{addr}]");
        if name.contains(name_part) {
            return Some(dev);
        }
    }
    None
}

fn rfcomm_channel_for(dev: &IOBluetoothDevice, uuid: &str) -> Option<u8> {
    let record = unsafe { dev.getServiceRecordForUUID(Some(&uuid_from_str(uuid))) }?;
    let mut id: u8 = 0;
    let r = unsafe { record.getRFCOMMChannelID(&mut id) };
    (r == 0).then_some(id)
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let want_connect = args.iter().any(|a| a == "--connect");
    let name_part = args
        .iter()
        .position(|a| a == "--name")
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
        .unwrap_or("WH-1000XM5");

    let Some(dev) = find_device(name_part) else {
        println!("PROBE: FAIL no paired device with a name containing {name_part:?}");
        return;
    };
    let connected = unsafe { dev.isConnected() };
    println!("PROBE: found device, connected={connected}");

    if !connected {
        if !want_connect {
            println!("PROBE: FAIL headphones are not connected to this Mac. Connect them in Bluetooth settings, or run with --connect.");
            return;
        }
        let r = unsafe { dev.openConnection() };
        println!("PROBE: openConnection -> 0x{r:x}");
        pump(3.0);
    }

    // SDP: list what we can see, then look up the MDR service by UUID.
    let mut channel_id = None;
    for attempt in 0..2 {
        let services = unsafe { dev.services() };
        match &services {
            None => println!("PROBE: no SDP records cached yet"),
            Some(arr) => {
                println!("PROBE: {} SDP records cached", arr.len());
                for item in arr.iter() {
                    if let Ok(rec) = item.downcast::<IOBluetoothSDPServiceRecord>() {
                        let n = unsafe { rec.getServiceName() }.map(|s| s.to_string());
                        println!("PROBE:   service: {n:?}");
                    }
                }
            }
        }
        for (label, uuid) in [("V2/XM5", SERVICE_UUID_V2), ("legacy", SERVICE_UUID_LEGACY)] {
            let ch = rfcomm_channel_for(&dev, uuid);
            println!("PROBE: {label} uuid {uuid} -> rfcomm channel {ch:?}");
            if channel_id.is_none() {
                channel_id = ch;
            }
        }
        if channel_id.is_some() || attempt == 1 {
            break;
        }
        println!("PROBE: running SDP query");
        let r = unsafe { dev.performSDPQuery(None) };
        println!("PROBE: performSDPQuery -> 0x{r:x}");
        pump(5.0);
    }
    let Some(channel_id) = channel_id else {
        println!("PROBE: FAIL no MDR RFCOMM service found");
        return;
    };

    let delegate = Delegate::new();
    delegate.ivars().st.borrow_mut().extra = args
        .windows(2)
        .filter(|w| w[0] == "--cmd")
        .map(|w| w[1].split_whitespace().map(|h| u8::from_str_radix(h, 16).expect("hex byte")).collect())
        .collect();
    let mut channel: Option<Retained<IOBluetoothRFCOMMChannel>> = None;
    let any: &AnyObject = &delegate;
    let r = unsafe { dev.openRFCOMMChannelAsync_withChannelID_delegate(Some(&mut channel), channel_id, Some(any)) };
    println!("PROBE: openRFCOMMChannelAsync(channel {channel_id}) -> 0x{r:x}");
    if r != 0 {
        println!("PROBE: FAIL could not start opening the channel");
        return;
    }
    delegate.ivars().st.borrow_mut().channel = channel;

    // Pump the run loop so delegate callbacks arrive. Stop early once we have what we need.
    let deadline = Instant::now() + Duration::from_secs(15);
    while Instant::now() < deadline {
        pump(0.1);
        let st = delegate.ivars().st.borrow();
        if st.closed || matches!(st.open_status, Some(s) if s != 0) {
            break;
        }
        if st.battery.is_some() && st.device_info.len() >= 2 {
            let wait = if st.extra.is_empty() { 1.0 } else { 4.0 };
            drop(st);
            pump(wait); // let the remaining replies arrive
            break;
        }
    }

    let st = delegate.ivars().st.borrow();
    println!("PROBE: ---- summary ----");
    println!("PROBE: channel open status: {:?}", st.open_status);
    println!("PROBE: device info: {:?}", st.device_info);
    println!("PROBE: battery: {:?}", st.battery);
    let (waiting_ack, seq, queued) = st.link.status();
    println!("PROBE: link: waiting_for_ack={waiting_ack} seq={seq} (expects ack seq {}) queued={queued}", 1 - seq);
    println!(
        "PROBE: RESULT {}",
        if st.battery.is_some() { "OK - protocol works end to end" } else { "INCOMPLETE - no battery reply" }
    );
    if let Some(ch) = &st.channel {
        unsafe { ch.closeChannel() };
    }
}
