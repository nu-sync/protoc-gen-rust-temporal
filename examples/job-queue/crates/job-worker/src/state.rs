//! Pure data shape backing the `RunJob` workflow. Kept in a separate module so
//! it can be unit-tested without instantiating any Temporal machinery.

use jobs_proto::jobs::v1::{CancelJobInput, JobInput, JobOutput, JobStatusOutput};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stage {
    #[default]
    Pending,
    Preparing,
    Executing,
    Collecting,
    Done,
    Cancelled,
}

impl Stage {
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Preparing => "preparing",
            Self::Executing => "executing",
            Self::Collecting => "collecting",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
}

/// Plain-data representation of `RunJob`'s in-memory state. Useful in tests —
/// the real workflow struct lives in lib.rs and is locked behind the
/// `#[workflow]` macro.
#[derive(Debug, Clone)]
pub struct JobState {
    pub input: JobInput,
    pub stage: Stage,
    pub progress_pct: u32,
    pub cancelled: bool,
    pub cancel_reason: Option<String>,
}

impl JobState {
    pub fn new(input: JobInput) -> Self {
        Self {
            input,
            stage: Stage::Pending,
            progress_pct: 0,
            cancelled: false,
            cancel_reason: None,
        }
    }

    pub fn status(&self) -> JobStatusOutput {
        JobStatusOutput {
            stage: self.stage.as_wire().to_string(),
            progress_pct: self.progress_pct,
        }
    }

    pub fn handle_cancel(&mut self, input: CancelJobInput) {
        self.cancelled = true;
        self.cancel_reason = Some(input.reason);
    }

    /// Drives one stage transition. Returns `None` while the workflow is still
    /// running, or `Some(output)` once it terminates.
    pub fn tick(&mut self) -> Option<JobOutput> {
        if self.cancelled && self.stage != Stage::Done && self.stage != Stage::Cancelled {
            self.stage = Stage::Cancelled;
            return Some(JobOutput {
                exit_code: 130,
                stdout: String::new(),
                stderr: format!(
                    "cancelled: {}",
                    self.cancel_reason.as_deref().unwrap_or("(no reason)")
                ),
            });
        }
        match self.stage {
            Stage::Pending => {
                self.stage = Stage::Preparing;
                self.progress_pct = 10;
                None
            }
            Stage::Preparing => {
                self.stage = Stage::Executing;
                self.progress_pct = 50;
                None
            }
            Stage::Executing => {
                self.stage = Stage::Collecting;
                self.progress_pct = 90;
                None
            }
            Stage::Collecting => {
                self.stage = Stage::Done;
                self.progress_pct = 100;
                Some(JobOutput {
                    exit_code: 0,
                    stdout: format!(
                        "[stub] ran `{}` (name={})",
                        self.input.command, self.input.name
                    ),
                    stderr: String::new(),
                })
            }
            Stage::Done | Stage::Cancelled => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ji(name: &str) -> JobInput {
        JobInput {
            name: name.into(),
            command: "echo hi".into(),
            timeout_seconds: 60,
        }
    }

    #[test]
    fn ticks_through_natural_path() {
        let mut state = JobState::new(ji("a"));
        assert_eq!(state.stage, Stage::Pending);
        assert!(state.tick().is_none());
        assert_eq!(state.stage, Stage::Preparing);
        assert!(state.tick().is_none());
        assert_eq!(state.stage, Stage::Executing);
        assert!(state.tick().is_none());
        assert_eq!(state.stage, Stage::Collecting);
        let out = state.tick().expect("collecting -> done returns output");
        assert_eq!(out.exit_code, 0);
        assert_eq!(state.stage, Stage::Done);
        assert_eq!(state.progress_pct, 100);
        assert!(state.tick().is_none(), "done is terminal");
    }

    #[test]
    fn cancel_short_circuits_during_executing() {
        let mut state = JobState::new(ji("b"));
        state.tick(); // -> Preparing
        state.tick(); // -> Executing
        state.handle_cancel(CancelJobInput {
            reason: "user".into(),
        });
        let out = state.tick().expect("cancellation produces an output");
        assert_eq!(out.exit_code, 130);
        assert!(out.stderr.contains("user"));
        assert_eq!(state.stage, Stage::Cancelled);
    }

    #[test]
    fn status_reflects_current_stage() {
        let mut state = JobState::new(ji("c"));
        let s0 = state.status();
        assert_eq!(s0.stage, "pending");
        assert_eq!(s0.progress_pct, 0);
        state.tick();
        let s1 = state.status();
        assert_eq!(s1.stage, "preparing");
        assert_eq!(s1.progress_pct, 10);
    }
}
