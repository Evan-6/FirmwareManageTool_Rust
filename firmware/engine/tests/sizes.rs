//! Static size budget. The XIAO has 264 KiB of SRAM and the shipping C++ engine
//! uses 29,712 bytes of static RAM in total; the Rust engine must stay in the
//! same order of magnitude before it can replace it.
//!
//! Measured with `arm-none-eabi-g++ -mcpu=cortex-m0plus -mthumb -Os` on the C++
//! engine (`char probe[sizeof(T)]` + `nm -S`) versus `size_of` here:
//!
//! | type | C++ | Rust |
//! | --- | ---: | ---: |
//! | InputState | 31 | 31 |
//! | Session | 60 | 56 |
//! | OutputScheduler | 10,072 | — |
//! | InputEngine | 12,600 | 12,564 |
//!
//! These are layout assertions, not measurements: they hold on every target and
//! fail the build when a struct grows without review.

use firmware_engine::{
    BootloaderGate, InputEngine, InputState, LedState, Report, Session, StatusIndicator, config,
    scheduler::{Completion, InputReport},
};
use std::mem::size_of;

#[test]
fn engine_static_footprint_stays_within_budget() {
    let engine = size_of::<InputEngine>();
    let state = size_of::<InputState>();
    let session = size_of::<Session>();
    let job_sources = size_of::<InputState>() * config::SESSIONS;

    println!("InputState={state} Session={session} InputEngine={engine}");
    println!("output job sources={job_sources} (per job)");

    // Wire-visible layouts are fixed by the protocol.
    assert_eq!(state, 31, "31-byte snapshot is part of the wire format");
    assert_eq!(size_of::<Report>(), 63, "reports are 63-byte payloads");
    assert_eq!(size_of::<Completion>(), 8);
    assert_eq!(size_of::<InputReport>(), 31, "id + length + 29 data bytes");
    assert_eq!(size_of::<LedState>(), 1);
    assert_eq!(size_of::<BootloaderGate>(), 8);
    assert_eq!(size_of::<StatusIndicator>(), 12);

    // Budgets with headroom over the current layout; raise them only with a
    // measured reason (the C++ engine's 29,712-byte total is the reference).
    assert!(session <= 64, "session slot grew unexpectedly: {session}");
    assert!(engine <= 64 * 1024, "engine static size: {engine}");
    assert!(
        engine + 16 * 1024 <= 264 * 1024,
        "must leave room for USB and stack"
    );
}
