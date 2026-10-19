//! Stage profiling for `--debug`.
//!
//! One process runs one DEX command, so the profiler is a process-global that the CLI
//! switches on before dispatch. Every call site is a `measure(stage, ..)` wrapper: when
//! profiling is off it costs one relaxed load and no clock reads, which keeps the normal
//! path free of instrumentation. Rayon workers update the same atomics, so a parallel
//! scan aggregates into one report instead of one line per thread.
//!
//! The output is deliberately fixed-width and deterministic: one line for stage totals in
//! declaration order, one for counters, one for wall time. Nothing is omitted when it is
//! zero, so two reports are always comparable line by line.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Clone, Copy)]
pub enum Stage {
    /// Opening and memory-mapping the archive.
    Map,
    /// Walking the central directory into DEX entries.
    ZipIndex,
    /// Ordering the selected entries.
    Select,
    /// Inflating one entry.
    Inflate,
    /// Parsing the DEX header and section table.
    Header,
    /// Resolving class/member/string identities.
    Members,
    /// Scanning code items for reference hits.
    Scan,
    /// Rendering hit rows.
    Render,
    /// Parsing a DEX for the Java emitter.
    Engine,
    /// Emitting Java for one class.
    Emit,
    /// Decoding AndroidManifest.xml.
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
    /// DEX entries the archive selected.
    Entries,
    /// Logical DEX files scanned (a 041 container holds several).
    DexFiles,
    /// Class definitions visited.
    Classes,
    /// Code items (methods with a body) visited.
    CodeItems,
    /// Reference hits found.
    Hits,
    /// DEX bytes made available to the parser.
    Bytes,
    /// String-table entries visited.
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

/// Count one kind of work unit.
pub fn count(counter: Counter, units: u64) {
    if !enabled() {
        return;
    }
    profile().counters[counter.slot()].fetch_add(units, Ordering::Relaxed);
}

/// Emit the stage/counter report, one deterministic line per category.
pub fn report(command: &str, wall: Duration) {
    if !enabled() {
        return;
    }
    let profile = profile();
    let stages: Vec<String> = STAGES
        .iter()
        .map(|(label, stage)| {
            let nanos = profile.stages[stage.slot()].load(Ordering::Relaxed);
            format!("{label}={:.3} ms", nanos as f64 / 1e6)
        })
        .collect();
    let counters: Vec<String> = COUNTERS
        .iter()
        .map(|(label, counter)| {
            format!(
                "{label}={}",
                profile.counters[counter.slot()].load(Ordering::Relaxed)
            )
        })
        .collect();
    crate::diag::diagnose(format_args!("[dex] {command} stages {}", stages.join(" ")));
    crate::diag::diagnose(format_args!(
        "[dex] {command} counters {}",
        counters.join(" ")
    ));
    crate::diag::diagnose(format_args!(
        "[dex] {command} wall {:.3} ms",
        wall.as_secs_f64() * 1e3
    ));
}
