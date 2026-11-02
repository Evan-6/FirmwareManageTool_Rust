//! Millisecond-precision sleeps.
//!
//! `std::thread::sleep` on Windows rounds the request up to the system timer
//! tick — 15.625 ms by default — so a 25 ms key hold actually lasts ~31 ms and
//! every delay in a run lands on the same grid. That is both wrong (the SDK
//! documents holds in milliseconds) and visible: any program that records input
//! timestamps can see the quantisation. This module waits on a high-resolution
//! waitable timer instead, which resolves to roughly half a millisecond.

use std::time::Duration;

/// Sleeps for `duration`, with sub-tick resolution where the platform has it.
pub fn sleep(duration: Duration) {
    let millis = duration.as_secs_f64() * 1000.0;
    if millis <= 0.0 {
        return;
    }
    #[cfg(windows)]
    if win::sleep_millis(millis) {
        return;
    }
    std::thread::sleep(duration);
}

/// Sleeps for `millis`, which may be fractional.
pub fn sleep_ms(millis: f64) {
    if millis > 0.0 {
        sleep(Duration::from_secs_f64(millis / 1000.0));
    }
}

#[cfg(windows)]
mod win {
    use std::cell::RefCell;
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows_sys::Win32::System::Threading::{
        CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, SetWaitableTimer,
        TIMER_ALL_ACCESS, WaitForSingleObject,
    };

    /// One high-resolution timer per thread, closed when the thread ends.
    struct Timer(HANDLE);

    impl Drop for Timer {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    fn timer() -> Option<&'static Timer> {
        thread_local! {
            static TIMER: RefCell<Option<Box<Timer>>> = const { RefCell::new(None) };
        }
        // The handle is owned by the thread-local for the life of the thread, so
        // handing out a reference past the borrow is sound.
        TIMER
            .with(|slot| {
                let mut slot = slot.borrow_mut();
                if slot.is_none() {
                    let handle = unsafe {
                        CreateWaitableTimerExW(
                            std::ptr::null(),
                            std::ptr::null(),
                            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                            TIMER_ALL_ACCESS,
                        )
                    };
                    if handle.is_null() {
                        return None;
                    }
                    *slot = Some(Box::new(Timer(handle)));
                }
                slot.as_deref().map(|timer| timer as *const Timer)
            })
            .map(|pointer| unsafe { &*pointer })
    }

    /// Returns false when the high-resolution timer is unavailable, so the
    /// caller can fall back to the coarse sleep.
    pub fn sleep_millis(millis: f64) -> bool {
        let Some(timer) = timer() else {
            return false;
        };
        // Negative means relative, in 100 ns units.
        let due: i64 = -((millis * 10_000.0).round() as i64).max(1);
        let set = unsafe { SetWaitableTimer(timer.0, &due, 0, None, std::ptr::null(), 0) };
        if set == 0 {
            return false;
        }
        unsafe { WaitForSingleObject(timer.0, u32::MAX) == WAIT_OBJECT_0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn short_sleeps_do_not_round_up_to_the_system_tick() {
        // A 4 ms request that takes 15 ms is the coarse timer: Windows rounds
        // every sleep up to the next 15.625 ms tick.
        let mut fast = 0;
        for _ in 0..20 {
            let started = Instant::now();
            sleep_ms(4.0);
            if started.elapsed() < Duration::from_millis(9) {
                fast += 1;
            }
        }
        assert!(
            fast >= 10,
            "only {fast}/20 four-millisecond sleeps finished under 9 ms; the coarse timer is in use"
        );
    }

    #[test]
    fn zero_and_negative_waits_return_immediately() {
        let started = Instant::now();
        sleep(Duration::ZERO);
        sleep_ms(-5.0);
        assert!(started.elapsed() < Duration::from_millis(5));
    }
}
