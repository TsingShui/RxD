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
    static STAGES: RefCell<BTreeMap<&'static str, Duration>> = const { RefCell::new(BTreeMap::new()) };
}

pub fn enable(enabled: bool) {
    STAGES.with(|stages| stages.borrow_mut().clear());
    ENABLED.with(|state| state.set(enabled));
}

pub fn scope<T>(name: &'static str, action: impl FnOnce() -> T) -> T {
    if !ENABLED.with(Cell::get) {
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
