#![allow(dead_code)]

pub mod addon_vm;
pub mod fake_t3;
pub mod shots;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Sets `stop` when dropped. Inside `thread::scope` a failed check would
/// otherwise wait forever for the bridge thread instead of failing the test.
pub struct StopOnDrop<'a>(pub &'a AtomicBool);

impl Drop for StopOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub fn wait_for<T>(what: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}
