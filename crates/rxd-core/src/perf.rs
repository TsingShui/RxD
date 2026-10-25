//! Opt-in metrics. Timings are nanoseconds; RSS is bytes, never a tree-wide sum.

use std::{collections::BTreeMap, io::Write, time::Instant};

use serde::Serialize;

use crate::Limits;

pub const PERF_VERSION: u32 = 1;

#[derive(Default, Serialize)]
pub struct Counters {
    pub instructions: Option<usize>,
    pub input_bytes: Option<usize>,
    pub entities: Option<usize>,
    pub matches: Option<usize>,
    /// Query result records; JSONL header and summary are excluded.
    pub records: Option<usize>,
    pub output_bytes: Option<usize>,
    pub semantic_nodes: Option<usize>,
    pub semantic_symbols: Option<usize>,
    pub semantic_references: Option<usize>,
    pub work: Option<usize>,
    pub ast_depth: Option<usize>,
    /// Selected EVM reconstruction work, independent of whole-input navigation.
    pub reconstructed_instructions: Option<usize>,
    pub reconstructed_blocks: Option<usize>,
    pub reconstructed_contexts: Option<usize>,
    pub reconstruction_values: Option<usize>,
    pub reconstruction_work: Option<usize>,
    pub structured_contexts: Option<usize>,
    pub structured_edges: Option<usize>,
    pub structuring_work: Option<usize>,
    pub structured_loops: Option<usize>,
    pub structured_backedges: Option<usize>,
}

#[derive(Serialize)]
pub struct Perf {
    record: &'static str,
    perf_version: u32,
    scope: &'static str,
    backend: &'static str,
    command: &'static str,
    pid: u32,
    status: &'static str,
    error_code: Option<&'static str>,
    pub source_id: Option<String>,
    pub counters: Counters,
    pub limits: Option<Limits>,
    pub resource_limits: Option<BTreeMap<&'static str, usize>>,
    /// Backend counters with no shared meaning (DEX entry counts, string totals, ...).
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<&'static str, usize>,
    stages_ns: BTreeMap<&'static str, u64>,
    total_ns: u64,
    peak_rss_bytes: Option<u64>,
    #[serde(skip)]
    start: Option<Instant>,
}

impl Perf {
    pub fn new(
        enabled: bool,
        scope: &'static str,
        backend: &'static str,
        command: &'static str,
    ) -> Self {
        let stages = if scope == "analysis" {
            &[
                "read",
                "hash",
                "utf8",
                "parse",
                "visit",
                "query",
                "serialize",
                "write",
            ][..]
        } else {
            &["args", "spawn_wait", "forward"][..]
        };
        Self {
            record: "perf",
            perf_version: PERF_VERSION,
            scope,
            backend,
            command,
            pid: std::process::id(),
            status: "pending",
            error_code: None,
            source_id: None,
            counters: Counters::default(),
            limits: None,
            resource_limits: None,
            extra: BTreeMap::new(),
            stages_ns: if enabled {
                stages.iter().map(|&s| (s, 0)).collect()
            } else {
                BTreeMap::new()
            },
            total_ns: 0,
            peak_rss_bytes: None,
            start: enabled.then(Instant::now),
        }
    }

    pub fn enabled(&self) -> bool {
        self.start.is_some()
    }

    /// The closure executes without clock reads or map access when metrics are off.
    pub fn measure<T>(&mut self, stage: &'static str, action: impl FnOnce() -> T) -> T {
        let start = self.enabled().then(Instant::now);
        let result = action();
        self.record(stage, start);
        result
    }

    pub fn start_stage(&self) -> Option<Instant> {
        self.enabled().then(Instant::now)
    }

    pub fn record(&mut self, stage: &'static str, start: Option<Instant>) {
        if let Some(start) = start {
            *self.stages_ns.entry(stage).or_default() += ns(start.elapsed());
        }
    }

    pub fn include_args(&mut self, process_start: Instant, args_ns: u64) {
        if self.enabled() {
            self.start = Some(process_start);
            self.stages_ns.insert("args", args_ns);
        }
    }

    /// Sum shard counters/timings; retain maximum RSS for the shared process.
    pub fn merge(&mut self, other: &Self) {
        macro_rules! merge_counters {
            ($($field:ident),* $(,)?) => {
                $(
                    self.counters.$field = match (self.counters.$field, other.counters.$field) {
                        (None, None) => None,
                        (left, right) => {
                            Some(left.unwrap_or(0).saturating_add(right.unwrap_or(0)))
                        }
                    };
                )*
            };
        }
        merge_counters!(
            instructions,
            input_bytes,
            entities,
            matches,
            records,
            output_bytes,
            semantic_nodes,
            semantic_symbols,
            semantic_references,
            work,
            ast_depth,
            reconstructed_instructions,
            reconstructed_blocks,
            reconstructed_contexts,
            reconstruction_values,
            reconstruction_work,
            structured_contexts,
            structured_edges,
            structuring_work,
            structured_loops,
            structured_backedges,
        );
        for (stage, elapsed) in &other.stages_ns {
            *self.stages_ns.entry(stage).or_default() += elapsed;
        }
        self.peak_rss_bytes = match (self.peak_rss_bytes, other.peak_rss_bytes) {
            (None, None) => None,
            (left, right) => Some(left.unwrap_or(0).max(right.unwrap_or(0))),
        };
    }

    pub fn finish(&mut self, error_code: Option<&'static str>) {
        if let Some(start) = self.start {
            self.total_ns = ns(start.elapsed());
            self.peak_rss_bytes = peak_rss_bytes();
        }
        self.status = if error_code.is_some() { "error" } else { "ok" };
        self.error_code = error_code;
    }

    /// Metrics emission is outside the measured total. Diagnostics cannot change query success.
    pub fn emit(&self) {
        let mut stderr = std::io::stderr().lock();
        let _ = serde_json::to_writer(&mut stderr, self);
        let _ = stderr.write_all(b"\n");
    }

    pub fn stage_ns(&mut self, stage: &'static str, nanos: u64) {
        if !self.enabled() {
            return;
        }
        *self.stages_ns.entry(stage).or_insert(0) += nanos;
    }

    pub fn extra(&mut self, name: &'static str, units: usize) {
        if !self.enabled() {
            return;
        }
        *self.extra.entry(name).or_insert(0) += units;
    }
}

pub fn ns(duration: std::time::Duration) -> u64 {
    duration.as_nanos().min(u128::from(u64::MAX)) as u64
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn peak_rss_bytes() -> Option<u64> {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::uninit();
    // SAFETY: getrusage initializes the supplied rusage on success.
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) } != 0 {
        return None;
    }
    // SAFETY: the successful call initialized usage.
    let usage = unsafe { usage.assume_init() };
    let peak = u64::try_from(usage.ru_maxrss).ok()?;
    // Darwin's current XNU uses resident_size_max in bytes; Linux reports KiB.
    Some(if cfg!(target_os = "linux") {
        peak.saturating_mul(1024)
    } else {
        peak
    })
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn peak_rss_bytes() -> Option<u64> {
    None
}
