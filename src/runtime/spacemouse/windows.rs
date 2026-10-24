//! Reading the 3D mouse on Windows, through raw input.
//!
//! **Raw input, not `hidapi`.** Opening the device's HID handle and reading it is the
//! portable way, but on Windows the 3DxWare driver is installed wherever the mouse is,
//! and it is reported to leave nothing for another reader (WebHID sees no input with it
//! running). Raw input is the window-message route the OS itself offers for a multi-axis
//! controller, and it is what PrusaSlicer uses on Windows alongside the driver.
//!
//! **A window of our own.** Raw input is delivered as `WM_INPUT` to a window. k2g's real
//! window belongs to the UI framework, and subclassing it would mean reaching into tao's
//! message handling. So this thread makes a message-only window — invisible, never in the
//! taskbar, there only to receive messages — and registers *it* for the device, with
//! `RIDEV_INPUTSINK` so input arrives whichever window has focus. Whether it is acted on is
//! then decided here: only while a window of this process is in front, so the mouse steers
//! k2g's view only while the operator is looking at k2g.
//!
//! Everything — the window, its messages, the per-device state — lives on this one thread,
//! because a window's messages are delivered to the thread that created it.
//!
//! This and `removable/win32.rs` are the crate's only `unsafe`, all of it FFI.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::mem::{size_of, zeroed};
use std::ptr;

use windows_sys::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Threading::GetCurrentProcessId;
use windows_sys::Win32::UI::Input::{
    GetRawInputData, GetRawInputDeviceInfoW, RegisterRawInputDevices, HRAWINPUT, RAWINPUT,
    RAWINPUTDEVICE, RAWINPUTHEADER, RIDEV_INPUTSINK, RIDI_DEVICEINFO, RID_DEVICE_INFO, RID_INPUT,
    RIM_TYPEHID,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, GetForegroundWindow, GetMessageW,
    GetWindowThreadProcessId, RegisterClassW, HWND_MESSAGE, MSG, WM_INPUT, WNDCLASSW,
};

use super::{publish, Deflection, Device, Input, VENDORS};

/// HID usage page and usage for a multi-axis controller — the class every 3Dconnexion
/// device reports itself as, and what raw input is asked for.
const USAGE_PAGE_GENERIC_DESKTOP: u16 = 0x01;
const USAGE_MULTI_AXIS_CONTROLLER: u16 = 0x08;

/// Starts the raw-input thread. A failure to start is logged and leaves the mouse unused;
/// it never stops the application.
pub(super) fn start() {
    if let Err(err) = std::thread::Builder::new().name("spacemouse".into()).spawn(run) {
        log::warn!("3D mouse: could not start its input thread: {err}");
    }
}

/// What the thread knows: each device it has seen, and whether the last motion it passed
/// on was a moving one.
#[derive(Default)]
struct State {
    /// By raw-input device handle. `None` for a device that is not a 3Dconnexion one — the
    /// answer is cached so it is asked of the OS once, not on every report.
    devices: BTreeMap<usize, Option<Device>>,
    /// The view was last told the cap is pushed. When k2g loses the front, the view is told
    /// it is at rest instead — once — so a held push does not carry on steering a view the
    /// operator has looked away from.
    moving: bool,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// The thread: make the window, register it, then pump its messages until the process
/// ends.
fn run() {
    // SAFETY: plain Win32 calls with valid arguments. Every pointer passed is either null
    // where the API allows it, or to a local that outlives the call (`class_name` is kept
    // alive for the life of the thread, which is the life of the window class).
    unsafe {
        let instance = GetModuleHandleW(ptr::null());
        let class_name = wide("k2g-spacemouse");
        let class = WNDCLASSW {
            style: 0,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: ptr::null_mut(),
            hCursor: ptr::null_mut(),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_name.as_ptr(),
        };
        if RegisterClassW(&class) == 0 {
            log::warn!("3D mouse: could not register its window class");
            return;
        }
        let hwnd = CreateWindowExW(
            0,
            class_name.as_ptr(),
            ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            ptr::null_mut(),
            instance,
            ptr::null(),
        );
        if hwnd.is_null() {
            log::warn!("3D mouse: could not create its message window");
            return;
        }

        let device = RAWINPUTDEVICE {
            usUsagePage: USAGE_PAGE_GENERIC_DESKTOP,
            usUsage: USAGE_MULTI_AXIS_CONTROLLER,
            dwFlags: RIDEV_INPUTSINK,
            hwndTarget: hwnd,
        };
        if RegisterRawInputDevices(&device, 1, size_of::<RAWINPUTDEVICE>() as u32) == 0 {
            log::warn!("3D mouse: could not register for raw input");
            return;
        }
        log::info!("3D mouse: listening for multi-axis controllers");

        let mut msg: MSG = zeroed();
        while GetMessageW(&mut msg, ptr::null_mut(), 0, 0) > 0 {
            DispatchMessageW(&msg);
        }
    }
}

/// The window's message handler: raw input is read here, and everything — `WM_INPUT`
/// included — still goes to `DefWindowProcW`, which is what lets the system release the
/// input's buffer.
unsafe extern "system" fn window_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_INPUT {
        read_input(lparam as HRAWINPUT);
    }
    // SAFETY: forwarding this window's own message, unchanged.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

/// Reads one `WM_INPUT` and passes on what its device said.
fn read_input(handle: HRAWINPUT) {
    let Some(buffer) = raw_input(handle) else {
        return;
    };
    // SAFETY: `raw_input` returned a buffer of at least a header's size, 8-byte aligned,
    // holding what GetRawInputData wrote — a RAWINPUT.
    let raw = unsafe { &*(buffer.as_ptr() as *const RAWINPUT) };
    if raw.header.dwType != RIM_TYPEHID {
        return;
    }
    // SAFETY: the type says this is the HID arm of the union.
    let hid = unsafe { &raw.data.hid };
    let report_size = hid.dwSizeHid as usize;
    let length = report_size * hid.dwCount as usize;
    let offset = (&raw const hid.bRawData) as usize - buffer.as_ptr() as usize;
    if report_size == 0 || offset + length > buffer.len() * size_of::<u64>() {
        return;
    }
    // SAFETY: bounds checked just above against what was written.
    let reports =
        unsafe { std::slice::from_raw_parts((&raw const hid.bRawData).cast::<u8>(), length) };
    let device = raw.header.hDevice;

    let in_front = k2g_in_front();
    STATE.with_borrow_mut(|state| {
        if !in_front {
            if state.moving {
                state.moving = false;
                publish(Input::Motion { deflection: Deflection::default() });
            }
            return;
        }
        let entry = state.devices.entry(device as usize).or_insert_with(|| identify(device));
        let Some(tracked) = entry.as_mut() else {
            return;
        };
        for report in reports.chunks_exact(report_size) {
            if let Some(input) = tracked.read(report) {
                if let Input::Motion { deflection } = input {
                    state.moving = deflection != Deflection::default();
                }
                publish(input);
            }
        }
    });
}

/// The `RAWINPUT` behind `handle`, copied into an 8-byte-aligned buffer (`RAWINPUT` holds
/// pointer-sized fields, so a byte vector would not do).
fn raw_input(handle: HRAWINPUT) -> Option<Vec<u64>> {
    let header = size_of::<RAWINPUTHEADER>() as u32;
    let mut size = 0u32;
    // SAFETY: a null buffer asks for the size only.
    let asked = unsafe { GetRawInputData(handle, RID_INPUT, ptr::null_mut(), &mut size, header) };
    if asked == u32::MAX || (size as usize) < size_of::<RAWINPUTHEADER>() {
        return None;
    }
    let mut buffer = vec![0u64; (size as usize).div_ceil(size_of::<u64>())];
    // SAFETY: the buffer holds at least `size` bytes.
    let written =
        unsafe { GetRawInputData(handle, RID_INPUT, buffer.as_mut_ptr().cast(), &mut size, header) };
    (written != u32::MAX && written as usize >= size_of::<RAWINPUTHEADER>()).then_some(buffer)
}

/// A tracker for `device` if it is a 3Dconnexion multi-axis controller, `None` if it is
/// anything else. Says which device it found, once, in the log — the first thing to check
/// when the mouse does nothing.
fn identify(device: HANDLE) -> Option<Device> {
    // SAFETY: an all-zero RID_DEVICE_INFO is valid; cbSize is set before the call as the
    // API requires.
    let mut info: RID_DEVICE_INFO = unsafe { zeroed() };
    info.cbSize = size_of::<RID_DEVICE_INFO>() as u32;
    let mut size = info.cbSize;
    // SAFETY: `info` is a RID_DEVICE_INFO of the size passed.
    let got = unsafe {
        GetRawInputDeviceInfoW(device, RIDI_DEVICEINFO, (&raw mut info).cast(), &mut size)
    };
    if got == u32::MAX || got == 0 || info.dwType != RIM_TYPEHID {
        return None;
    }
    // SAFETY: the type says this is the HID arm of the union.
    let hid = unsafe { info.Anonymous.hid };
    if !VENDORS.contains(&hid.dwVendorId) {
        log::info!(
            "3D mouse: ignoring a multi-axis controller from vendor {:04x} (product {:04x})",
            hid.dwVendorId,
            hid.dwProductId
        );
        return None;
    }
    log::info!(
        "3D mouse: found 3Dconnexion device {:04x}:{:04x}",
        hid.dwVendorId,
        hid.dwProductId
    );
    Some(Device::default())
}

/// Whether the window in front belongs to this process — k2g's own window, or one of its
/// dialogs.
fn k2g_in_front() -> bool {
    // SAFETY: both calls take and return plain values; a null window is checked for.
    unsafe {
        let front = GetForegroundWindow();
        if front.is_null() {
            return false;
        }
        let mut process = 0u32;
        GetWindowThreadProcessId(front, &mut process);
        process == GetCurrentProcessId()
    }
}

/// `text` as a nul-terminated UTF-16 string, for the `W` APIs.
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
