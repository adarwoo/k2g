use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

// Generates schema-compatible UUIDv7-like identifiers.
pub fn new_uuid_v7_like() -> String {
    // UUIDv7 layout: xxxxxxxx-xxxx-7xxx-yxxx-xxxxxxxxxxxx
    // Uses Unix milliseconds + process-local sequence for monotonicity.
    static ID_SEQ: AtomicU64 = AtomicU64::new(0);

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let seq = ID_SEQ.fetch_add(1, Ordering::Relaxed);

    let ts48 = now_ms & 0x0000_FFFF_FFFF_FFFF;
    let rand_a = (seq & 0x0FFF) as u16;
    let variant = 0x8 | ((seq >> 12) as u16 & 0x3);
    let rand_b = (now_ms.rotate_left(13) ^ seq.rotate_left(7) ^ 0xA5A5_5A5A_1357_2468)
        & 0x0000_FFFF_FFFF_FFFF;

    format!(
        "{:08x}-{:04x}-7{:03x}-{:x}{:03x}-{:012x}",
        ((ts48 >> 16) & 0xFFFF_FFFF) as u32,
        (ts48 & 0xFFFF) as u16,
        rand_a,
        variant,
        ((seq >> 20) & 0x0FFF) as u16,
        rand_b
    )
}
