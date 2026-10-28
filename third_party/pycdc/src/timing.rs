//! Opt-in recovery phase timings.
//!
//! State is thread-local so parallel module workers stay independent. The
//! adapter enables collection only for `--perf`; otherwise `scope` is a thin
//! branch around the measured action.

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    time::{Duration, Instant},
};

thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(false) };
    static DEPTH: Cell<usize> = const { Cell::new(0) };
    static STAGES: RefCell<BTreeMap<&'static str, Duration>> = const { RefCell::new(BTreeMap::new()) };
}

pub fn enable(enabled: bool) {
    STAGES.with(|stages| stages.borrow_mut().clear());
    ENABLED.with(|state| state.set(enabled));
}

pub struct Frame;

pub fn enter() -> Frame {
    DEPTH.with(|depth| depth.set(depth.get() + 1));
    Frame
}

impl Drop for Frame {
    fn drop(&mut self) {
        DEPTH.with(|depth| depth.set(depth.get().saturating_sub(1)));
    }
}

fn root() -> bool {
    ENABLED.with(Cell::get) && DEPTH.with(Cell::get) == 1
}

pub fn scope<T>(name: &'static str, action: impl FnOnce() -> T) -> T {
    if !root() {
        return action();
    }
    let start = Instant::now();
    let result = action();
    STAGES.with(|stages| {
        let mut stages = stages.borrow_mut();
        *stages.entry(name).or_default() += start.elapsed();
    });
    result
}

/// Add one externally timed scope.
pub fn mark(name: &'static str, elapsed: Duration) {
    if !root() {
        return;
    }
    STAGES.with(|stages| {
        let mut stages = stages.borrow_mut();
        *stages.entry(name).or_default() += elapsed;
    });
}

/// Drain accumulated nanoseconds for the current thread.
pub fn take() -> Vec<(&'static str, u64)> {
    STAGES.with(|stages| {
        std::mem::take(&mut *stages.borrow_mut())
            .into_iter()
            .map(|(name, elapsed)| {
                (
                    name,
                    elapsed.as_nanos().min(u128::from(u64::MAX)) as u64,
                )
            })
            .collect()
    })
}
