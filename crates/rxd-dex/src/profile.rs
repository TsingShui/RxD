//! Process-local DEX stage/counter profiling shared by Rayon workers.
//! Disabled profiling performs no clock reads.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub enum Stage {
    /// Opening and memory-mapping the archive.
    Map,
    /// Walking the central directory into DEX entries.
    ZipIndex,

    Select,

    Inflate,
    /// Parsing the DEX header and section table.
    Header,

    Members,

    Scan,
    /// Rendering hit rows.
    Render,
    /// Parsing a DEX for the Java emitter.
    Engine,

    Emit,

    Manifest,
}

const STAGES: [(&str, Stage); 11] = [
    ("map", Stage::Map),
    ("zip", Stage::ZipIndex),
    ("select", Stage::Select),
    ("inflate", Stage::Inflate),
    ("header", Stage::Header),
    ("members", Stage::Members),
    ("scan", Stage::Scan),
    ("render", Stage::Render),
    ("engine", Stage::Engine),
    ("emit", Stage::Emit),
    ("manifest", Stage::Manifest),
];

impl Stage {
    fn slot(self) -> usize {
        STAGES
            .iter()
            .position(|(_, stage)| std::mem::discriminant(stage) == std::mem::discriminant(&self))
            .expect("every stage is registered")
    }
}

#[derive(Clone, Copy)]
pub enum Counter {
    Entries,
    /// Logical DEX files scanned (a 041 container holds several).
    DexFiles,

    Classes,
    /// Code items (methods with a body) visited.
    CodeItems,

    Hits,
    /// DEX bytes made available to the parser.
    Bytes,

    Strings,
}

const COUNTERS: [(&str, Counter); 7] = [
    ("entries", Counter::Entries),
    ("dex", Counter::DexFiles),
    ("classes", Counter::Classes),
    ("code_items", Counter::CodeItems),
    ("hits", Counter::Hits),
    ("bytes", Counter::Bytes),
    ("strings", Counter::Strings),
];

impl Counter {
    fn slot(self) -> usize {
        COUNTERS
            .iter()
            .position(|(_, counter)| {
                std::mem::discriminant(counter) == std::mem::discriminant(&self)
            })
            .expect("every counter is registered")
    }
}

struct Profile {
    enabled: bool,
    stages: [AtomicU64; 11],
    counters: [AtomicU64; 7],
}

impl Profile {
    fn new(enabled: bool) -> Self {
        Self {
            enabled,
            stages: std::array::from_fn(|_| AtomicU64::new(0)),
            counters: std::array::from_fn(|_| AtomicU64::new(0)),
        }
    }
}

static PROFILE: OnceLock<Profile> = OnceLock::new();

/// Switch profiling on for this process. Called once by the CLI before dispatch.
pub fn enable(enabled: bool) {
    let _ = PROFILE.set(Profile::new(enabled));
}

fn profile() -> &'static Profile {
    PROFILE.get_or_init(|| Profile::new(false))
}

pub fn enabled() -> bool {
    profile().enabled
}

/// Time one stage. Disabled profiling is a branch and an inlined call.
pub fn measure<T>(stage: Stage, action: impl FnOnce() -> T) -> T {
    if !enabled() {
        return action();
    }
    let started = Instant::now();
    let value = action();
    add_nanos(stage, started.elapsed());
    value
}

pub fn add(stage: Stage, elapsed: Duration) {
    if enabled() {
        add_nanos(stage, elapsed);
    }
}

fn add_nanos(stage: Stage, elapsed: Duration) {
    let nanos = u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX);
    profile().stages[stage.slot()].fetch_add(nanos, Ordering::Relaxed);
}

pub fn count(counter: Counter, units: u64) {
    if !enabled() {
        return;
    }
    profile().counters[counter.slot()].fetch_add(units, Ordering::Relaxed);
}

type Totals = Vec<(&'static str, u64)>;

/// Return accumulated stage/counter totals for the shared perf sidecar.
pub fn values() -> (Totals, Totals) {
    if !enabled() {
        return (Vec::new(), Vec::new());
    }
    let profile = profile();
    let stages = STAGES
        .iter()
        .map(|(label, stage)| (*label, profile.stages[stage.slot()].load(Ordering::Relaxed)))
        .collect();
    let counters = COUNTERS
        .iter()
        .map(|(label, counter)| {
            (
                *label,
                profile.counters[counter.slot()].load(Ordering::Relaxed),
            )
        })
        .collect();
    (stages, counters)
}
