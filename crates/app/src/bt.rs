#![allow(non_snake_case)] // ObjC protocol method names are fixed by IOBluetooth.
//! IOBluetooth side: finds the paired headphones, opens the MDR RFCOMM channel and talks the
//! protocol through `mdr_proto`.
//!
//! Everything in here runs on the main thread. IOBluetooth delivers its delegate callbacks on
//! the run loop of the thread that opened the channel, and in a Tauri app only the main thread
//! runs a run loop. The state lives in thread-locals for that reason.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::time::{Duration, Instant};

use mdr_proto::{msg, DataType, Link, SERVICE_UUID_V2};
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, AnyThread, Message};
use objc2_foundation::{NSObject, NSObjectProtocol};
use objc2_io_bluetooth::{
    IOBluetoothDevice, IOBluetoothRFCOMMChannel, IOBluetoothRFCOMMChannelDelegate, IOBluetoothSDPUUID,
};

use crate::{engine, log};

const RETRY_EVERY: Duration = Duration::from_secs(3);
/// Right after the headphones are switched on, macOS may never finish opening the channel, or
/// the channel opens but the headphones do not answer yet. Give up on such an attempt after this
/// long and start a fresh one.
const OPEN_TIMEOUT: Duration = Duration::from_secs(8);

thread_local! {
    static DEVICE: RefCell<Option<Retained<IOBluetoothDevice>>> = const { RefCell::new(None) };
    static CHANNEL: RefCell<Option<Retained<IOBluetoothRFCOMMChannel>>> = const { RefCell::new(None) };
    static DELEGATE: RefCell<Option<Retained<Delegate>>> = const { RefCell::new(None) };
    static LINK: RefCell<Link> = RefCell::new(Link::new());
    static LAST_TRY: Cell<Option<Instant>> = const { Cell::new(None) };
    static SDP_ASKED: Cell<bool> = const { Cell::new(false) };
    /// When the current channel open was started; cleared once it completes or fails.
    static OPEN_SINCE: Cell<Option<Instant>> = const { Cell::new(None) };
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and Delegate has no Drop and no ivars.
    #[unsafe(super = NSObject)]
    struct Delegate;

    // SAFETY: NSObjectProtocol has no safety requirements.
    unsafe impl NSObjectProtocol for Delegate {}

    // SAFETY: the signatures match the protocol declaration.
    unsafe impl IOBluetoothRFCOMMChannelDelegate for Delegate {
        #[unsafe(method(rfcommChannelOpenComplete:status:))]
        unsafe fn rfcommChannelOpenComplete_status(&self, channel: Option<&IOBluetoothRFCOMMChannel>, error: i32) {
            log::line(format!("rfcomm open complete status=0x{error:x}"));
            if error != 0 {
                OPEN_SINCE.set(None);
                CHANNEL.with(|c| c.borrow_mut().take());
                engine::get().mdr_open_failed(error);
                return;
            }
            CHANNEL.with(|c| {
                let mut c = c.borrow_mut();
                if c.is_none() {
                    *c = channel.map(|ch| ch.retain());
                }
            });
            // A fresh link, so nothing left over from an earlier session can block the handshake.
            reset_link();
            // First command of the handshake. The rest is queued when its reply arrives.
            let tx = LINK.with(|l| l.borrow_mut().send(DataType::DataMdr, msg::get_protocol_info()));
            write(&tx);
        }

        #[unsafe(method(rfcommChannelData:data:length:))]
        unsafe fn rfcommChannelData_data_length(
            &self,
            _channel: Option<&IOBluetoothRFCOMMChannel>,
            data: *mut c_void,
            length: usize,
        ) {
            // SAFETY: IOBluetooth passes a valid buffer of `length` bytes for the call.
            let bytes = unsafe { std::slice::from_raw_parts(data as *const u8, length) };
            handle_rx(bytes);
        }

        #[unsafe(method(rfcommChannelClosed:))]
        unsafe fn rfcommChannelClosed(&self, _channel: Option<&IOBluetoothRFCOMMChannel>) {
            // Ignore the echo of our own close_mdr().
            if CHANNEL.with(|c| c.borrow_mut().take()).is_some() {
                log::line("rfcomm channel closed by the headphones");
                reset_link();
                engine::get().mdr_closed();
            }
        }
    }
);

impl Delegate {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

fn reset_link() {
    OPEN_SINCE.set(None);
    LINK.with(|l| *l.borrow_mut() = Link::new());
    SDP_ASKED.set(false);
    LAST_TRY.set(None);
}

/// Write bytes to the channel. No RefCell borrow may be held by the caller, because
/// IOBluetooth can call back into the delegate while the write runs.
fn write(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let Some(channel) = CHANNEL.with(|c| c.borrow().clone()) else {
        log::line("write skipped: no channel");
        return;
    };
    let mut buf = bytes.to_vec();
    // SAFETY: `buf` is valid for the whole synchronous call.
    let r = unsafe { channel.writeSync_length(buf.as_mut_ptr() as *mut c_void, buf.len() as u16) };
    if r != 0 {
        log::line(format!("writeSync failed 0x{r:x}"));
    }
}

fn handle_rx(bytes: &[u8]) {
    let (mut tx, packets) = LINK.with(|l| {
        let out = l.borrow_mut().on_bytes(bytes);
        (out.tx, out.rx)
    });
    for p in packets {
        if p.ty != DataType::DataMdr {
            continue; // Table 2 is not used yet.
        }
        let event = msg::parse(&p.payload);
        if matches!(event, msg::Event::ProtocolInfo { .. }) {
            OPEN_SINCE.set(None); // the headphones answered: the handshake is alive
            // The link sends these one at a time, each after the previous ACK.
            for payload in [
                msg::get_capability_info(),
                msg::get_device_info(msg::DeviceInfoType::ModelName),
                msg::get_device_info(msg::DeviceInfoType::FwVersion),
                msg::get_device_info(msg::DeviceInfoType::SeriesAndColor),
                msg::get_support_function(),
                msg::get_battery(),
                msg::get_noise_control(),
            ] {
                tx.extend(LINK.with(|l| l.borrow_mut().send(DataType::DataMdr, payload)));
            }
        }
        if let msg::Event::SupportFunctions(ids) = &event {
            for payload in msg::queries_for(ids) {
                tx.extend(LINK.with(|l| l.borrow_mut().send(DataType::DataMdr, payload)));
            }
        }
        engine::get().on_mdr_event(event);
    }
    write(&tx);
}

/// Find the paired headphones by name and remember them. Returns the Bluetooth address.
pub fn find_device(name_part: &str) -> Option<String> {
    let list = unsafe { IOBluetoothDevice::pairedDevices() }?;
    for item in list.iter() {
        let Ok(dev) = item.downcast::<IOBluetoothDevice>() else { continue };
        if unsafe { dev.name() }.to_string().contains(name_part) {
            let addr = unsafe { dev.addressString() }.map(|s| s.to_string())?;
            DEVICE.with(|d| *d.borrow_mut() = Some(dev));
            return Some(addr);
        }
    }
    None
}

pub fn is_connected() -> bool {
    DEVICE.with(|d| d.borrow().as_ref().is_some_and(|dev| unsafe { dev.isConnected() }))
}

fn rfcomm_channel(dev: &IOBluetoothDevice) -> Option<u8> {
    let h: String = SERVICE_UUID_V2.chars().filter(|c| *c != '-').collect();
    let bytes: Vec<u8> = (0..16).map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap()).collect();
    let uuid = unsafe { IOBluetoothSDPUUID::uuidWithBytes_length(bytes.as_ptr() as *const c_void, 16) }?;
    let record = unsafe { dev.getServiceRecordForUUID(Some(&uuid)) }?;
    let mut id: u8 = 0;
    (unsafe { record.getRFCOMMChannelID(&mut id) } == 0).then_some(id)
}

/// Open the MDR channel if it is not open. Safe to call often: it retries every few seconds.
pub fn open_mdr() {
    if CHANNEL.with(|c| c.borrow().is_some()) {
        return;
    }
    if LAST_TRY.get().is_some_and(|t| t.elapsed() < RETRY_EVERY) {
        return;
    }
    LAST_TRY.set(Some(Instant::now()));
    let Some(dev) = DEVICE.with(|d| d.borrow().clone()) else { return };
    let Some(channel_id) = rfcomm_channel(&dev) else {
        // Service records are not cached yet. Ask once; the next retry will find them.
        if !SDP_ASKED.replace(true) {
            log::line("no MDR service record yet, running SDP query");
            unsafe { dev.performSDPQuery(None) };
        }
        return;
    };
    let delegate = Delegate::new();
    let mut channel = None;
    let any: &AnyObject = &delegate;
    let r = unsafe { dev.openRFCOMMChannelAsync_withChannelID_delegate(Some(&mut channel), channel_id, Some(any)) };
    log::line(format!("openRFCOMMChannelAsync(channel {channel_id}) -> 0x{r:x}"));
    if r != 0 {
        engine::get().mdr_open_failed(r);
        return;
    }
    CHANNEL.with(|c| *c.borrow_mut() = channel);
    DELEGATE.with(|d| *d.borrow_mut() = Some(delegate));
    OPEN_SINCE.set(Some(Instant::now()));
}

pub fn close_mdr() {
    if let Some(ch) = CHANNEL.with(|c| c.borrow_mut().take()) {
        unsafe { ch.closeChannel() };
    }
    DELEGATE.with(|d| d.borrow_mut().take());
    reset_link();
}

/// Queue table 1 commands. The first goes out now and each next one after the previous is ACKed.
/// With no open channel this does nothing: a queued command would wait forever for an ACK and
/// block the handshake when the channel opens again.
pub fn send_many(payloads: Vec<Vec<u8>>) {
    if CHANNEL.with(|c| c.borrow().is_none()) {
        log::line("write ignored: MDR channel is not open");
        return;
    }
    let mut tx = Vec::new();
    for payload in payloads {
        tx.extend(LINK.with(|l| l.borrow_mut().send(DataType::DataMdr, payload)));
    }
    write(&tx);
}

/// Called every 1.5 s: resend a command that got no ACK, or drop it so it cannot block the queue.
pub fn tick() {
    if OPEN_SINCE.get().is_some_and(|t| t.elapsed() > OPEN_TIMEOUT) {
        log::line("no handshake answer from the headphones, closing the channel to try again");
        close_mdr();
        return;
    }
    if CHANNEL.with(|c| c.borrow().is_none()) {
        return;
    }
    let t = LINK.with(|l| l.borrow_mut().tick());
    if t.retried {
        log::line("no ACK from the headphones, sending the command again");
    }
    if t.dropped {
        log::line("no ACK after a retry, command dropped");
    }
    write(&t.tx);
}
