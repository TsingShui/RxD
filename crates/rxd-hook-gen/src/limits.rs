use crate::{Diagnostic, ErrorKind, Result, Stage};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub input_bytes: usize,
    pub expanded_bytes: usize,
    pub target_bytes: usize,
    pub source_bytes: usize,
    pub artifact_bytes: usize,
    pub artifact_files: usize,
    pub analysis_steps: u64,
    pub analysis_timeout_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            input_bytes: rxd_core::MAX_INPUT_BYTES,
            expanded_bytes: 128 * 1024 * 1024,
            target_bytes: 16 * 1024,
            source_bytes: 1024 * 1024,
            artifact_bytes: 2 * 1024 * 1024,
            artifact_files: 8,
            analysis_steps: 1_000_000,
            analysis_timeout_ms: 1_000,
        }
    }
}

impl Limits {
    pub fn validate(self) -> Result<()> {
        if self.input_bytes == 0
            || self.expanded_bytes == 0
            || self.target_bytes == 0
            || self.source_bytes == 0
            || self.artifact_bytes == 0
            || self.artifact_files == 0
            || self.analysis_steps == 0
            || self.analysis_timeout_ms == 0
        {
            return Err(Diagnostic::new(
                ErrorKind::InvalidArguments,
                Stage::Input,
                "budgets must be nonzero",
            ));
        }
        Ok(())
    }

    pub fn check_input(self, bytes: usize) -> Result<()> {
        check(
            bytes,
            self.input_bytes,
            Stage::Input,
            "input byte budget exceeded",
        )
    }

    pub fn check_target(self, target: &str) -> Result<()> {
        check(
            target.len(),
            self.target_bytes,
            Stage::Selection,
            "target byte budget exceeded",
        )
    }

    pub fn check_source(self, bytes: usize) -> Result<()> {
        check(
            bytes,
            self.source_bytes,
            Stage::Render,
            "source byte budget exceeded",
        )
    }
}

pub(crate) fn check(actual: usize, maximum: usize, stage: Stage, message: &str) -> Result<()> {
    if actual > maximum {
        Err(Diagnostic::new(ErrorKind::ResourceLimit, stage, message))
    } else {
        Ok(())
    }
}

/// Cooperative accounting, not preemption. Providers must also pass these ceilings to engines
/// and check at bounded work intervals; a single uninterruptible engine call is not time-limited by this guard.
pub struct AnalysisBudget {
    started: Instant,
    timeout: Duration,
    remaining_steps: u64,
}

impl AnalysisBudget {
    pub fn new(limits: Limits) -> Result<Self> {
        limits.validate()?;
        Ok(Self {
            started: Instant::now(),
            timeout: Duration::from_millis(limits.analysis_timeout_ms),
            remaining_steps: limits.analysis_steps,
        })
    }

    pub fn consume(&mut self, steps: u64) -> Result<()> {
        if self.started.elapsed() >= self.timeout {
            return Err(Diagnostic::new(
                ErrorKind::AnalysisTimeout,
                Stage::Analysis,
                "analysis time budget exceeded",
            ));
        }
        self.remaining_steps = self.remaining_steps.checked_sub(steps).ok_or_else(|| {
            Diagnostic::new(
                ErrorKind::ResourceLimit,
                Stage::Analysis,
                "analysis step budget exceeded",
            )
        })?;
        Ok(())
    }
}
