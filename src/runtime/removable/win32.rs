//! Windows removable-media detection and eject.
//!
//! **The only `unsafe` in this crate lives here**, behind the safe API in the parent
//! module. Every block carries a `SAFETY:` note naming the invariant it relies on; the
//! four that recur are NUL-terminated wide strings, out-parameter buffers whose length is
//! passed in the unit the callee documents, handle validity, and a `#[repr(C)]` struct
//! passed as an input buffer.
//!
//! ## Known limitation: USB hard drives do not appear
//!
//! `GetDriveTypeW` reports `DRIVE_REMOVABLE` from the *device's* removable-media bit.
//! Flash sticks and card readers set it; external USB hard drives and most USB SSDs do
//! not — the medium inside them is not removable, the enclosure is — so they come back as
//! `DRIVE_FIXED` and are not offered here. Doing better means `IOCTL_STORAGE_QUERY_PROPERTY`
//! or SetupAPI for a materially larger unsafe surface, and ejecting a fixed volume also
//! wants elevation this app does not have and should not ask for.
//!
//! That is a missing convenience, never a dead end: the ordinary Save button stays
//! visible and unconditional, so a USB hard drive is saved to through the normal dialog
//! and removed with Windows' own Safely Remove Hardware.
//!
//! The converse false positive — a card reader with no card, which *is* reported as
//! `DRIVE_REMOVABLE` — is filtered by the `GetVolumeInformationW` failure in [`scan`],
//! not by a special case.

use std::path::PathBuf;
use std::ptr;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    CloseHandle, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FlushFileBuffers, GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives,
    GetVolumeInformationW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows_sys::Win32::System::Diagnostics::Debug::{SetThreadErrorMode, SEM_FAILCRITICALERRORS};
use windows_sys::Win32::System::Ioctl::{
    FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, IOCTL_STORAGE_EJECT_MEDIA,
    IOCTL_STORAGE_MEDIA_REMOVAL, PREVENT_MEDIA_REMOVAL,
};
use windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOVABLE;
use windows_sys::Win32::System::IO::DeviceIoControl;

use super::{EjectError, EjectOutcome, RemovableMedium, UNLABELLED_MEDIUM};

/// `MAX_PATH + 1`, the buffer size `GetVolumeInformationW` documents for a volume name.
const VOLUME_NAME_CAPACITY: usize = 261;

/// How many times to try locking the volume before giving up.
///
/// The sequence and the retry are the long-standing Win32 recipe for ejecting removable
/// media (KB165721), which uses twenty attempts; ten keeps the whole thing inside about
/// five seconds. The lock is what actually fails in practice — Explorer with the drive
/// open, a shell thumbnail worker, or an antivirus scan of the file just written will all
/// hold a handle for a second or two after a save.
const LOCK_ATTEMPTS: u32 = 10;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(500);

/// Suppresses the hard-error dialog for the calling thread.
///
/// Without this, probing a removable drive with no media in it can raise the Windows hard
/// error handler and pop a modal *"There is no disk in the drive. Please insert a disk
/// into drive E:"* — from a background thread, over a WebView, with no way to dismiss it
/// programmatically. `SetThreadErrorMode` (rather than the process-wide `SetErrorMode`)
/// is thread-local, so setting it on the watcher thread cannot affect the UI thread or
/// any other crate's error handling.
pub(super) fn prepare_thread() {
    // SAFETY: both arguments are plain values — a flag and a null out-pointer, which the
    // API documents as "do not return the previous mode". No memory is shared.
    let ok = unsafe { SetThreadErrorMode(SEM_FAILCRITICALERRORS, ptr::null_mut()) };
    if ok == 0 {
        // Not fatal: the worst case is a modal box on a rare configuration, which is the
        // state every version before this one shipped in.
        log::debug!(
            "could not suppress hard-error dialogs on the removable-media thread: {}",
            std::io::Error::last_os_error()
        );
    }
}

/// Every mounted removable volume, in drive-letter order.
///
/// The order is A→Z by construction (the drive bitmask is walked in bit order) and the
/// parent module's change detection compares the two lists positionally, so this ordering
/// is part of the contract rather than an accident.
pub(super) fn scan() -> Vec<RemovableMedium> {
    // SAFETY: no arguments, no memory shared with the callee.
    let mask = unsafe { GetLogicalDrives() };
    if mask == 0 {
        // A zero mask is indistinguishable from "the call failed" — the API has no other
        // failure signal — but either way there is nothing to offer.
        log::debug!("GetLogicalDrives reported no drives: {}", std::io::Error::last_os_error());
        return Vec::new();
    }

    let mut media = Vec::new();
    for index in 0..26u32 {
        if mask & (1 << index) == 0 {
            continue;
        }
        let letter = b'A' + index as u8;
        let root = root_path_wide(letter);

        // First, because it is the cheap device-free one: a mount-manager lookup that
        // costs nothing and keeps the rest of the scan off spun-down disks and dead
        // network mounts entirely.
        //
        // SAFETY: `root` is a live local array whose last element is 0, so `as_ptr()` is
        // a valid NUL-terminated PCWSTR for the length of the call.
        if unsafe { GetDriveTypeW(root.as_ptr()) } != DRIVE_REMOVABLE {
            continue;
        }

        let Some(label) = volume_label(&root) else {
            continue; // no media in the slot — see the module header
        };
        let Some(free_bytes) = free_space(&root) else {
            continue; // pulled between the two calls; skipping is the right answer
        };

        media.push(RemovableMedium {
            root: PathBuf::from(format!("{}:\\", letter as char)),
            label,
            drive_letter: letter as char,
            free_bytes,
        });
    }
    media
}

/// The volume label, or `None` when the drive has no media in it.
///
/// The expected failure is `ERROR_NOT_READY` from a card reader with an empty slot, which
/// Windows still reports as `DRIVE_REMOVABLE` — so this call is what filters empty slots
/// (and any surviving A:/B: floppy letter) out of the picker.
fn volume_label(root: &[u16]) -> Option<String> {
    let mut name = [0u16; VOLUME_NAME_CAPACITY];
    // SAFETY: `root` is a NUL-terminated wide string that outlives the call. `name` is
    // `[u16; VOLUME_NAME_CAPACITY]` and the length is passed as a count of *characters*,
    // which is the unit `nVolumeNameSize` is specified in, so the callee writes at most
    // the space that exists. The five null pointers are the optional out-parameters this
    // caller does not want, which the API documents as skippable.
    let ok = unsafe {
        GetVolumeInformationW(
            root.as_ptr(),
            name.as_mut_ptr(),
            name.len() as u32,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            0,
        )
    };
    if ok == 0 {
        return None;
    }

    let end = name.iter().position(|&unit| unit == 0).unwrap_or(name.len());
    // Lossy: a label with an unpaired surrogate in it is a curiosity, not a reason to
    // drop the drive from the list.
    let label = String::from_utf16_lossy(&name[..end]).trim().to_string();
    Some(if label.is_empty() { UNLABELLED_MEDIUM.to_string() } else { label })
}

/// Free bytes on the volume, or `None` if it went away mid-scan.
///
/// Reads `lpFreeBytesAvailableToCaller` rather than the volume's total free space: on a
/// quota'd volume the two differ, and the question being asked is "will my program fit".
fn free_space(root: &[u16]) -> Option<u64> {
    let mut free: u64 = 0;
    // SAFETY: `root` is a NUL-terminated wide string that outlives the call; `free` is a
    // live local the callee writes exactly one `u64` into. The two null pointers are the
    // totals this caller does not want.
    let ok = unsafe {
        GetDiskFreeSpaceExW(root.as_ptr(), &mut free, ptr::null_mut(), ptr::null_mut())
    };
    (ok != 0).then_some(free)
}

/// Flushes, locks, dismounts and ejects the volume.
pub(super) fn eject(medium: &RemovableMedium) -> Result<EjectOutcome, EjectError> {
    let letter = medium.drive_letter as u8;
    let path = device_path_wide(letter);

    // SAFETY: `path` is a live local NUL-terminated wide string. The security-attributes
    // and template-file arguments are null, which the API documents as "defaults, and no
    // template" respectively.
    let handle = unsafe {
        CreateFileW(
            path.as_ptr(),
            // Write access is required by both `FlushFileBuffers` and `FSCTL_LOCK_VOLUME`.
            GENERIC_READ | GENERIC_WRITE,
            // Must share: the filesystem itself still has the volume mounted at this point.
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            ptr::null(),
            OPEN_EXISTING, // a device is never created
            // 0, not FILE_ATTRIBUTE_NORMAL: that flag is documented as valid only when
            // used alone and is meaningless for a device open.
            0,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        // Captured immediately: the thread's last error is overwritten by the very next
        // Win32 call, `CloseHandle` included.
        return Err(std::io::Error::last_os_error().into());
    }
    let volume = VolumeHandle(handle);

    // Redundant on the happy path — the dismount below flushes as part of dismounting —
    // and there entirely for the unhappy one. `fs::write` returns when the data reaches
    // the *filesystem cache*, and whether Windows write-caches a removable volume is a
    // per-device policy ("Quick removal" is only the default). If the lock never succeeds,
    // this is the only thing that pushed the program onto the stick, and it is why the
    // busy message can honestly lead with "Saved to E:".
    //
    // SAFETY: the handle came from a `CreateFileW` that returned something other than
    // `INVALID_HANDLE_VALUE` and is owned by the live `volume`, so it is open here.
    if unsafe { FlushFileBuffers(volume.0) } == 0 {
        // A flush that cannot run is a reason to be less confident, not a reason to
        // abandon a dismount that would flush anyway.
        log::warn!(
            "could not flush {}: {}",
            medium.display_name(),
            std::io::Error::last_os_error()
        );
    }

    let mut locked = false;
    for attempt in 1..=LOCK_ATTEMPTS {
        if control(&volume, FSCTL_LOCK_VOLUME, ptr::null(), 0) {
            locked = true;
            break;
        }
        if attempt < LOCK_ATTEMPTS {
            std::thread::sleep(LOCK_RETRY_DELAY);
        }
    }
    if !locked {
        // `volume` drops here, closing a handle that never held a lock. The data is
        // already flushed, which is exactly why the flush comes first.
        return Err(EjectError::Busy {
            attempts: LOCK_ATTEMPTS,
            seconds: (LOCK_ATTEMPTS as u64 * LOCK_RETRY_DELAY.as_millis() as u64) / 1000,
        });
    }

    // Fatal on failure: a volume that is still mounted has a live filesystem on it, and
    // calling that safe to remove would be a lie.
    if !control(&volume, FSCTL_DISMOUNT_VOLUME, ptr::null(), 0) {
        return Err(std::io::Error::last_os_error().into());
    }

    // Clears the *software* media lock. Non-fatal by design: most USB sticks do not
    // implement it and answer `ERROR_INVALID_FUNCTION`, so treating that as a failure
    // would make the common case look broken.
    let prevent = PREVENT_MEDIA_REMOVAL { PreventMediaRemoval: false };
    if !control(
        &volume,
        IOCTL_STORAGE_MEDIA_REMOVAL,
        std::ptr::addr_of!(prevent).cast(),
        std::mem::size_of::<PREVENT_MEDIA_REMOVAL>() as u32,
    ) {
        log::debug!(
            "{} does not support a software media lock: {}",
            medium.display_name(),
            std::io::Error::last_os_error()
        );
    }

    if control(&volume, IOCTL_STORAGE_EJECT_MEDIA, ptr::null(), 0) {
        return Ok(EjectOutcome::Ejected);
    }

    let error = std::io::Error::last_os_error();
    match error.raw_os_error() {
        // ERROR_INVALID_FUNCTION / ERROR_NOT_SUPPORTED: the device has no eject
        // mechanism, which is extremely common for plain sticks. The dismount above has
        // already happened, so it genuinely *is* safe to pull — reporting an error here
        // would train users to ignore the one message that matters.
        Some(1) | Some(50) => Ok(EjectOutcome::SafeToRemove),
        _ => Err(error.into()),
    }
    // `volume` drops: `CloseHandle` releases the volume lock with it. There is no
    // explicit unlock anywhere in this function because the lock is a property of the
    // handle, so every exit path — success or failure — releases it.
}

/// One `DeviceIoControl` with no output buffer, which is every call this module makes.
///
/// Wrapped so the six constant arguments are written once rather than five times, and so
/// each call site reads as the operation it performs.
fn control(volume: &VolumeHandle, code: u32, input: *const core::ffi::c_void, input_len: u32) -> bool {
    let mut returned: u32 = 0;
    // SAFETY: the handle is open for the length of the call (owned by the live `volume`).
    // `input` is either null with a zero length, or a pointer to a live `#[repr(C)]`
    // local whose length is `size_of` of that same type, so the callee reads exactly the
    // bytes that exist. `returned` is a live local; the overlapped pointer is null, which
    // requests a synchronous call on a handle opened without `FILE_FLAG_OVERLAPPED`.
    unsafe {
        DeviceIoControl(
            volume.0,
            code,
            input,
            input_len,
            ptr::null_mut(),
            0,
            &mut returned,
            ptr::null_mut(),
        ) != 0
    }
}

/// Owns a volume handle for the length of one eject attempt.
///
/// Every step of [`eject`] is an early return, and a leaked handle keeps the volume
/// *locked* — a worse outcome than the failed eject, because the user then cannot open
/// the drive in Explorer either. Tying the close to the scope means no exit path can
/// forget it.
struct VolumeHandle(HANDLE);

impl Drop for VolumeHandle {
    fn drop(&mut self) {
        // SAFETY: the handle came from a `CreateFileW` that returned something other than
        // `INVALID_HANDLE_VALUE`; this type is its sole owner (no `Copy`, no `Clone`,
        // constructed only in `eject`) and `drop` runs at most once, so this is neither a
        // double close nor the close of a foreign handle.
        unsafe { CloseHandle(self.0) };
    }
}

/// The volume's *root directory*: `E:\`, **with** the trailing backslash.
///
/// `GetDriveTypeW`, `GetVolumeInformationW` and `GetDiskFreeSpaceExW` all want this form;
/// drop the backslash and they operate on the process's current directory *on* that drive
/// instead of on the volume.
///
/// A fixed-size array rather than `encode_wide().collect()` because the string is ASCII of
/// known length: no allocation, and no chance of the classic dangling-pointer bug where a
/// temporary `Vec<u16>` is dropped before the call it was built for. (If a path here ever
/// becomes dynamic, the rule is: bind the `Vec<u16>` to a `let` that outlives the call.)
fn root_path_wide(letter: u8) -> [u16; 4] {
    [letter as u16, b':' as u16, b'\\' as u16, 0]
}

/// The volume's *device* path: `\\.\E:`, deliberately **without** a trailing backslash.
///
/// This is the single most common mistake in this API: `\\.\E:\` opens the root
/// *directory* rather than the volume device, after which every FSCTL fails with
/// `ERROR_INVALID_FUNCTION` and the drive looks like it has no eject support, rather than
/// like a malformed path.
fn device_path_wide(letter: u8) -> [u16; 7] {
    [
        b'\\' as u16,
        b'\\' as u16,
        b'.' as u16,
        b'\\' as u16,
        letter as u16,
        b':' as u16,
        0,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two path forms differ by exactly one separator, and getting that wrong fails
    /// in a way that looks like an unsupported device rather than a bad path — so both
    /// are pinned here.
    #[test]
    fn the_two_path_forms_are_what_win32_expects() {
        assert_eq!(String::from_utf16_lossy(&root_path_wide(b'E')[..3]), "E:\\");
        assert_eq!(root_path_wide(b'E')[3], 0, "root path must be NUL-terminated");

        assert_eq!(String::from_utf16_lossy(&device_path_wide(b'E')[..6]), r"\\.\E:");
        assert_eq!(device_path_wide(b'E')[6], 0, "device path must be NUL-terminated");
    }

    /// A scan on a machine with nothing plugged in must be empty rather than panicking —
    /// the one thing about the Win32 path that CI can actually assert.
    #[test]
    fn scanning_never_panics() {
        prepare_thread();
        let _ = scan();
    }
}
