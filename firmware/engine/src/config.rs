//! Fixed capacities. The XIAO build has 264 KiB of RAM.

/// Eight concurrent owners, because several hosts are expected to be connected
/// at the same time. Held state is the union of all of them; see
/// `protocol-v3.md` for how per-session releases and leases interact.
pub const SESSIONS: usize = 8;
pub const OUTPUT_DEPTH: usize = 32;
pub const RX_DEPTH: usize = 32;
pub const TX_DEPTH: usize = 32;
pub const BOOTLOADER_DELAY_MS: u32 = 120;
pub const LEASE_MS: u32 = 2000;
/// A queued job older than this makes the shared queue stale.
pub const STALE_MS: u32 = 50;
/// Motion axes accumulate up to this magnitude before the queue is faulted.
pub const MOTION_LIMIT: i32 = 8192;

const _: () = assert!(
    TX_DEPTH >= SESSIONS,
    "a global fault must fit every session event"
);
