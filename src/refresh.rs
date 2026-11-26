//! Correlated refresh feedback. Scheduled data updates never create a generation.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RefreshPhase {
    Waiting,
    Running,
    Succeeded,
    Failed,
    Disabled,
}
impl RefreshPhase {
    pub fn active(self) -> bool {
        matches!(self, Self::Waiting | Self::Running)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Waiting => "Waiting for provider interval or backoff",
            Self::Running => "Refreshing",
            Self::Succeeded => "Refresh complete",
            Self::Failed => "Refresh failed",
            Self::Disabled => "Refresh cancelled: provider hidden",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefreshSnapshot {
    pub generation: u64,
    pub startup: bool,
    pub providers: BTreeMap<String, RefreshPhase>,
}
impl RefreshSnapshot {
    pub fn active(&self) -> bool {
        self.providers.values().any(|phase| phase.active())
    }
    pub fn summary(&self) -> &'static str {
        if self.providers.is_empty() {
            "No enabled providers"
        } else if self
            .providers
            .values()
            .any(|phase| *phase == RefreshPhase::Running)
        {
            "Refreshing providers"
        } else if self.active() {
            "Refresh waiting for provider interval or backoff"
        } else if self
            .providers
            .values()
            .any(|phase| *phase == RefreshPhase::Failed)
        {
            "Refresh finished with errors"
        } else if self
            .providers
            .values()
            .any(|phase| *phase == RefreshPhase::Disabled)
        {
            "Refresh finished; some providers were hidden"
        } else {
            "Refresh complete"
        }
    }
}
