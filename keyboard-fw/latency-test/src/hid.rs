//! macOS HID capture via IOKit. We register an *input-report-with-timestamp*
//! callback on the emulated keyboard, so every report the host receives is
//! delivered with the kernel's mach-time stamp — the most precise "the computer
//! got the signal" instant available without a kernel extension.
//!
//! Requires the "Input Monitoring" privacy permission (System Settings →
//! Privacy & Security → Input Monitoring) for the terminal running this tool.
//! `--seize` additionally takes exclusive control of the device so the
//! keystrokes do not also reach the OS, but needs `sudo`.

use std::os::raw::c_void;
use std::ptr;
use std::sync::mpsc::Sender;

use core_foundation::base::TCFType;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;

use core_foundation_sys::base::{CFIndex, CFRelease, CFTypeRef};
use core_foundation_sys::dictionary::{
    kCFTypeDictionaryKeyCallBacks, kCFTypeDictionaryValueCallBacks, CFDictionaryCreate,
    CFDictionaryRef,
};
use core_foundation_sys::number::{kCFNumberSInt32Type, CFNumberGetValue, CFNumberRef};
use core_foundation_sys::runloop::{
    kCFRunLoopDefaultMode, CFRunLoopGetCurrent, CFRunLoopRef, CFRunLoopRun,
};
use core_foundation_sys::set::{CFSetGetCount, CFSetGetValues, CFSetRef};
use core_foundation_sys::string::CFStringRef;

use crate::macos_time::mach_to_ns;

type IOHIDManagerRef = *mut c_void;
type IOHIDDeviceRef = *mut c_void;
type IOReturn = i32;
type IOOptionBits = u32;
type IOHIDReportType = u32;

const SEIZE_DEVICE: IOOptionBits = 1; // kIOHIDOptionsTypeSeizeDevice

type ReportWithTsCallback = extern "C" fn(
    context: *mut c_void,
    result: IOReturn,
    sender: *mut c_void,
    report_type: IOHIDReportType,
    report_id: u32,
    report: *mut u8,
    report_len: CFIndex,
    timestamp: u64,
);

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDManagerCreate(allocator: *const c_void, options: IOOptionBits) -> IOHIDManagerRef;
    fn IOHIDManagerSetDeviceMatching(manager: IOHIDManagerRef, matching: CFDictionaryRef);
    fn IOHIDManagerOpen(manager: IOHIDManagerRef, options: IOOptionBits) -> IOReturn;
    fn IOHIDManagerCopyDevices(manager: IOHIDManagerRef) -> CFSetRef;
    fn IOHIDManagerScheduleWithRunLoop(
        manager: IOHIDManagerRef,
        run_loop: CFRunLoopRef,
        run_loop_mode: CFStringRef,
    );
    fn IOHIDDeviceRegisterInputReportWithTimeStampCallback(
        device: IOHIDDeviceRef,
        report: *mut u8,
        report_length: CFIndex,
        callback: ReportWithTsCallback,
        context: *mut c_void,
    );
    fn IOHIDDeviceGetProperty(device: IOHIDDeviceRef, key: CFStringRef) -> CFTypeRef;
}

pub struct HidEvent {
    pub report: Vec<u8>,
    pub host_ts_ns: i64,
}

pub enum Startup {
    Ready(usize),
    Error(String),
}

struct CbCtx {
    tx: Sender<HidEvent>,
}

extern "C" fn input_report_cb(
    context: *mut c_void,
    _result: IOReturn,
    _sender: *mut c_void,
    _report_type: IOHIDReportType,
    _report_id: u32,
    report: *mut u8,
    report_len: CFIndex,
    timestamp: u64,
) {
    if context.is_null() || report.is_null() || report_len <= 0 {
        return;
    }
    let ctx = unsafe { &*(context as *const CbCtx) };
    let bytes = unsafe { std::slice::from_raw_parts(report, report_len as usize) }.to_vec();
    let _ = ctx.tx.send(HidEvent {
        report: bytes,
        host_ts_ns: mach_to_ns(timestamp),
    });
}

/// Build a +0-retain matching dictionary from (CF key string, i32) pairs.
/// Caller owns the returned ref (Create rule) and must `CFRelease` it.
fn make_match(pairs: &[(&'static str, i32)]) -> CFDictionaryRef {
    let keys: Vec<CFString> = pairs
        .iter()
        .map(|(k, _)| CFString::from_static_string(k))
        .collect();
    let vals: Vec<CFNumber> = pairs.iter().map(|(_, v)| CFNumber::from(*v)).collect();
    let key_refs: Vec<*const c_void> = keys
        .iter()
        .map(|k| k.as_concrete_TypeRef() as *const c_void)
        .collect();
    let val_refs: Vec<*const c_void> = vals
        .iter()
        .map(|v| v.as_concrete_TypeRef() as *const c_void)
        .collect();
    unsafe {
        CFDictionaryCreate(
            ptr::null(),
            key_refs.as_ptr() as *mut *const c_void,
            val_refs.as_ptr() as *mut *const c_void,
            pairs.len() as CFIndex,
            &kCFTypeDictionaryKeyCallBacks,
            &kCFTypeDictionaryValueCallBacks,
        )
    }
    // keys/vals drop here, releasing their refs; the dict has retained copies.
}

unsafe fn get_int_prop(device: IOHIDDeviceRef, key: &'static str) -> Option<i32> {
    let k = CFString::from_static_string(key);
    let v = IOHIDDeviceGetProperty(device, k.as_concrete_TypeRef());
    if v.is_null() {
        return None;
    }
    let mut out: i32 = 0;
    let ok = CFNumberGetValue(
        v as CFNumberRef,
        kCFNumberSInt32Type,
        &mut out as *mut i32 as *mut c_void,
    );
    if ok {
        Some(out)
    } else {
        None
    }
}

unsafe fn get_str_prop(device: IOHIDDeviceRef, key: &'static str) -> Option<String> {
    let k = CFString::from_static_string(key);
    let v = IOHIDDeviceGetProperty(device, k.as_concrete_TypeRef());
    if v.is_null() {
        return None;
    }
    Some(CFString::wrap_under_get_rule(v as CFStringRef).to_string())
}

/// Enumerate connected HID keyboards (usage page 1 / usage 6).
pub fn list_keyboards() -> Vec<(u16, u16, String)> {
    let mut out = Vec::new();
    unsafe {
        let manager = IOHIDManagerCreate(ptr::null(), 0);
        if manager.is_null() {
            return out;
        }
        let dict = make_match(&[("DeviceUsagePage", 1), ("DeviceUsage", 6)]);
        IOHIDManagerSetDeviceMatching(manager, dict);
        CFRelease(dict as CFTypeRef);
        if IOHIDManagerOpen(manager, 0) != 0 {
            CFRelease(manager as CFTypeRef);
            return out;
        }
        let set = IOHIDManagerCopyDevices(manager);
        if !set.is_null() {
            let count = CFSetGetCount(set);
            let mut values: Vec<*const c_void> = vec![ptr::null(); count as usize];
            CFSetGetValues(set, values.as_mut_ptr());
            for &dev in &values {
                if dev.is_null() {
                    continue;
                }
                let vid = get_int_prop(dev as IOHIDDeviceRef, "VendorID").unwrap_or(0) as u16;
                let pid = get_int_prop(dev as IOHIDDeviceRef, "ProductID").unwrap_or(0) as u16;
                let name =
                    get_str_prop(dev as IOHIDDeviceRef, "Product").unwrap_or_else(|| "?".into());
                out.push((vid, pid, name));
            }
            CFRelease(set as CFTypeRef);
        }
        CFRelease(manager as CFTypeRef);
    }
    out
}

/// Spawn the HID capture thread. It runs a CFRunLoop for the life of the
/// process; startup status is reported on `ready_tx`.
pub fn spawn_capture(
    vid: u16,
    pid: u16,
    seize: bool,
    ev_tx: Sender<HidEvent>,
    ready_tx: Sender<Startup>,
) {
    std::thread::Builder::new()
        .name("sigtap-hid".into())
        .spawn(move || unsafe {
            let manager = IOHIDManagerCreate(ptr::null(), 0);
            if manager.is_null() {
                let _ = ready_tx.send(Startup::Error("IOHIDManagerCreate failed".into()));
                return;
            }
            let dict = make_match(&[("VendorID", vid as i32), ("ProductID", pid as i32)]);
            IOHIDManagerSetDeviceMatching(manager, dict);
            CFRelease(dict as CFTypeRef);

            let opts = if seize { SEIZE_DEVICE } else { 0 };
            let r = IOHIDManagerOpen(manager, opts);
            if r != 0 {
                let hint = if seize {
                    " (seize needs sudo)"
                } else {
                    " (grant Terminal 'Input Monitoring' in System Settings)"
                };
                let _ = ready_tx.send(Startup::Error(format!(
                    "IOHIDManagerOpen failed: 0x{:08x}{hint}",
                    r as u32
                )));
                return;
            }

            IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetCurrent(), kCFRunLoopDefaultMode);

            let ctx = Box::into_raw(Box::new(CbCtx { tx: ev_tx })) as *mut c_void;
            let mut n_dev = 0usize;
            let set = IOHIDManagerCopyDevices(manager);
            if !set.is_null() {
                let count = CFSetGetCount(set);
                let mut values: Vec<*const c_void> = vec![ptr::null(); count as usize];
                CFSetGetValues(set, values.as_mut_ptr());
                for &dev in &values {
                    if dev.is_null() {
                        continue;
                    }
                    // Per-device buffer the system fills; leaked for process life.
                    let buf = Box::into_raw(Box::new([0u8; 64])) as *mut u8;
                    IOHIDDeviceRegisterInputReportWithTimeStampCallback(
                        dev as IOHIDDeviceRef,
                        buf,
                        64,
                        input_report_cb,
                        ctx,
                    );
                    n_dev += 1;
                }
                CFRelease(set as CFTypeRef);
            }

            let _ = ready_tx.send(Startup::Ready(n_dev));
            // `manager` is intentionally leaked: it stays open and scheduled for
            // the life of the process; the run loop drives the callbacks.
            CFRunLoopRun();
        })
        .expect("spawn hid thread");
}
