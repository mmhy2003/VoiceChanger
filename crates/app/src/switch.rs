//! Switching library models: what a switch needs and which pitch the new model gets.

use std::collections::BTreeMap;
use std::sync::mpsc::Receiver;

use vc_engine::Generator;

use crate::model::LoadResult;

/// How a model switch is carried out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchPlan {
    /// No pipeline is loaded: load ContentVec, RMVPE and the generator.
    LoadAll,
    /// Stopped with a pipeline: load the new generator and swap it in.
    Swap,
    /// Running: load the new generator while the old voice plays, then stop, swap and start.
    SwapWithRestart,
}

pub fn plan_switch(has_pipeline: bool, running: bool) -> SwitchPlan {
    if !has_pipeline {
        SwitchPlan::LoadAll
    } else if running {
        SwitchPlan::SwapWithRestart
    } else {
        SwitchPlan::Swap
    }
}

/// The saved pitch for `id`, else the model's default.
pub fn pitch_for(id: &str, saved: &BTreeMap<String, i32>, default_pitch: i32) -> i32 {
    saved.get(id).copied().unwrap_or(default_pitch)
}

/// A generator loading in the background for the model `id`.
pub struct PendingSwitch {
    pub id: String,
    pub name: String,
    pub rx: Receiver<LoadResult<Generator>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_switch_cases() {
        assert_eq!(plan_switch(false, false), SwitchPlan::LoadAll);
        assert_eq!(plan_switch(false, true), SwitchPlan::LoadAll);
        assert_eq!(plan_switch(true, false), SwitchPlan::Swap);
        assert_eq!(plan_switch(true, true), SwitchPlan::SwapWithRestart);
    }

    #[test]
    fn pitch_for_saved_else_default() {
        let saved: BTreeMap<String, i32> = [("ana".to_string(), -4)].into();
        assert_eq!(pitch_for("ana", &saved, 7), -4);
        assert_eq!(pitch_for("bob", &saved, 7), 7);
        assert_eq!(pitch_for("bob", &BTreeMap::new(), 0), 0);
    }
}
