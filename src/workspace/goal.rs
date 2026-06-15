//! Goal-loop + real-time judging aggregation for one workspace.
//!
//! The sidecar emulates code_puppy's CLI goal loop and streams structured
//! events (`goal_state` / `judge_run_started` / `judge_started` /
//! `judge_verdict` / `goal_iteration` / `goal_done`). This folds that stream
//! into the per-session state the GOALS HUD + the real-time judging view
//! render from. Bounded: only the last `MAX_ROUNDS` rounds are retained.

use crate::backend::{
    GoalDoneMsg, GoalIterationMsg, GoalStateMsg, JudgeRunStarted, JudgeStartedMsg, JudgeVerdictMsg,
};

/// Keep a short scrollback of prior judging rounds (older dropped).
const MAX_ROUNDS: usize = 8;

/// Live status of one judge within a round.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JudgeStatus {
    Pending,
    Running,
    Pass,
    Fail,
    Abstain,
}

impl JudgeStatus {
    pub fn label(self) -> &'static str {
        match self {
            JudgeStatus::Pending => "pending",
            JudgeStatus::Running => "running",
            JudgeStatus::Pass => "PASS",
            JudgeStatus::Fail => "FAIL",
            JudgeStatus::Abstain => "ABSTAIN",
        }
    }

    /// Still waiting on this judge's verdict (drives the pulse animation).
    pub fn is_running(self) -> bool {
        matches!(self, JudgeStatus::Pending | JudgeStatus::Running)
    }
}

/// One judge's live row in a round.
#[derive(Clone, Debug)]
pub struct JudgeLive {
    pub name: String,
    pub model: String,
    pub status: JudgeStatus,
    pub notes: String,
}

/// A finished round's snapshot (the judging scrollback).
#[derive(Clone, Debug)]
pub struct RoundSummary {
    pub iteration: u64,
    pub all_complete: bool,
    pub verdicts: Vec<JudgeLive>,
}

/// One workspace's goal-loop state, folded from the event stream.
#[derive(Clone, Debug, Default)]
pub struct GoalRun {
    pub active: bool,
    pub prompt: String,
    pub loop_count: u64,
    pub max: u64,
    pub iteration: u64,
    pub remediation: String,
    /// Set once the loop finishes (completed / loops / reason).
    pub done: Option<GoalDoneMsg>,
    /// The current round's live judge rows (resolve as verdicts arrive).
    pub judges: Vec<JudgeLive>,
    /// Prior rounds, newest last, bounded to `MAX_ROUNDS`.
    pub rounds: Vec<RoundSummary>,
    /// The current round's all-complete flag (stashed for the archive).
    last_all_complete: bool,
}

impl GoalRun {
    /// Is a goal actively running right now?
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Any goal data worth showing (active, mid-round, or a finished result)?
    pub fn has_activity(&self) -> bool {
        self.active || self.done.is_some() || !self.judges.is_empty() || !self.rounds.is_empty()
    }

    pub fn on_state(&mut self, m: GoalStateMsg) {
        // A fresh goal beginning (active flips on) resets the round data so a
        // prior run's verdicts don't bleed into the new HUD.
        if m.active && !self.active {
            self.done = None;
            self.judges.clear();
            self.rounds.clear();
            self.iteration = 0;
            self.remediation.clear();
            self.last_all_complete = false;
        }
        self.active = m.active;
        if !m.prompt.is_empty() {
            self.prompt = m.prompt;
        }
        self.loop_count = m.loop_count;
        if m.max > 0 {
            self.max = m.max;
        }
    }

    pub fn on_run_started(&mut self, m: JudgeRunStarted) {
        // Archive the previous round before the new one's rows replace it.
        self.archive_round();
        self.iteration = m.iteration;
        if m.max > 0 {
            self.max = m.max;
        }
        self.judges = m
            .judges
            .into_iter()
            .map(|j| JudgeLive {
                name: j.name,
                model: j.model,
                status: JudgeStatus::Pending,
                notes: String::new(),
            })
            .collect();
    }

    pub fn on_judge_started(&mut self, m: JudgeStartedMsg) {
        if let Some(row) = self.judges.iter_mut().find(|r| r.name == m.judge_name) {
            row.status = JudgeStatus::Running;
        }
    }

    pub fn on_verdict(&mut self, m: JudgeVerdictMsg) {
        if let Some(row) = self.judges.iter_mut().find(|r| r.name == m.judge_name) {
            row.status = if m.abstained {
                JudgeStatus::Abstain
            } else if m.complete {
                JudgeStatus::Pass
            } else {
                JudgeStatus::Fail
            };
            row.notes = m.notes;
        }
    }

    pub fn on_iteration(&mut self, m: GoalIterationMsg) {
        self.remediation = m.remediation_notes;
        self.loop_count = m.loop_count;
        if m.max > 0 {
            self.max = m.max;
        }
        self.last_all_complete = m.all_complete;
    }

    pub fn on_done(&mut self, m: GoalDoneMsg) {
        self.archive_round();
        self.active = false;
        self.done = Some(m);
    }

    /// Move the current round's rows into the bounded history.
    fn archive_round(&mut self) {
        if self.judges.is_empty() {
            return;
        }
        self.rounds.push(RoundSummary {
            iteration: self.iteration,
            all_complete: self.last_all_complete,
            verdicts: std::mem::take(&mut self.judges),
        });
        if self.rounds.len() > MAX_ROUNDS {
            let drop = self.rounds.len() - MAX_ROUNDS;
            self.rounds.drain(..drop);
        }
        self.last_all_complete = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::JudgeRosterEntry;

    fn roster(names: &[&str]) -> Vec<JudgeRosterEntry> {
        names
            .iter()
            .map(|n| JudgeRosterEntry {
                name: (*n).into(),
                model: "gpt-5".into(),
            })
            .collect()
    }

    #[test]
    fn full_round_resolves_rows_live() {
        let mut g = GoalRun::default();
        g.on_state(GoalStateMsg {
            active: true,
            prompt: "ship it".into(),
            loop_count: 0,
            max: 2,
            mode: "goal".into(),
        });
        assert!(g.is_active());
        g.on_run_started(JudgeRunStarted {
            goal: "ship it".into(),
            iteration: 1,
            max: 2,
            judges: roster(&["a", "b"]),
        });
        assert_eq!(g.judges.len(), 2);
        assert_eq!(g.judges[0].status, JudgeStatus::Pending);
        g.on_judge_started(JudgeStartedMsg {
            judge_name: "a".into(),
            iteration: 1,
        });
        assert_eq!(g.judges[0].status, JudgeStatus::Running);
        g.on_verdict(JudgeVerdictMsg {
            judge_name: "a".into(),
            iteration: 1,
            complete: true,
            abstained: false,
            notes: "looks good".into(),
        });
        assert_eq!(g.judges[0].status, JudgeStatus::Pass);
        assert_eq!(g.judges[0].notes, "looks good");
        g.on_verdict(JudgeVerdictMsg {
            judge_name: "b".into(),
            iteration: 1,
            complete: false,
            abstained: true,
            notes: "endpoint error".into(),
        });
        assert_eq!(g.judges[1].status, JudgeStatus::Abstain);
        g.on_done(GoalDoneMsg {
            completed: true,
            loops: 1,
            reason: "all_pass".into(),
        });
        assert!(!g.is_active());
        assert_eq!(g.rounds.len(), 1); // archived on done
        assert_eq!(g.rounds[0].verdicts.len(), 2);
        assert_eq!(g.done.as_ref().unwrap().reason, "all_pass");
    }

    #[test]
    fn new_run_resets_prior_round() {
        let mut g = GoalRun::default();
        g.on_state(GoalStateMsg {
            active: true,
            prompt: "x".into(),
            loop_count: 0,
            max: 5,
            mode: "goal".into(),
        });
        g.on_run_started(JudgeRunStarted {
            goal: "x".into(),
            iteration: 1,
            max: 5,
            judges: roster(&["a"]),
        });
        g.on_done(GoalDoneMsg {
            completed: false,
            loops: 1,
            reason: "stopped".into(),
        });
        assert_eq!(g.rounds.len(), 1);
        // A brand-new goal clears the prior run's scrollback.
        g.on_state(GoalStateMsg {
            active: true,
            prompt: "y".into(),
            loop_count: 0,
            max: 5,
            mode: "goal".into(),
        });
        assert!(g.rounds.is_empty());
        assert!(g.done.is_none());
        assert_eq!(g.prompt, "y");
    }

    #[test]
    fn round_history_is_bounded() {
        let mut g = GoalRun::default();
        g.on_state(GoalStateMsg {
            active: true,
            prompt: "x".into(),
            loop_count: 0,
            max: 100,
            mode: "goal".into(),
        });
        for i in 1..=12 {
            g.on_run_started(JudgeRunStarted {
                goal: "x".into(),
                iteration: i,
                max: 100,
                judges: roster(&["a"]),
            });
        }
        // archive_round fires on each on_run_started after the first.
        assert!(g.rounds.len() <= MAX_ROUNDS);
    }
}
