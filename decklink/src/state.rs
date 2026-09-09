use crate::error::{Error, ErrorKind, Result};

/// Session lifecycle used by capture and playout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionState {
    Idle,
    Configuring,
    Enabled,
    Prerolling,
    Running,
    Stopping,
    Stopped,
    Failed,
    Disconnected,
}

impl SessionState {
    pub fn can_enable(self) -> bool {
        matches!(self, Self::Idle | Self::Stopped)
    }

    pub fn can_start(self) -> bool {
        matches!(self, Self::Enabled | Self::Prerolling)
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Stopped | Self::Failed | Self::Disconnected)
    }

    pub fn transition(self, next: Self) -> Result<Self> {
        let allowed = matches!(
            (self, next),
            (Self::Idle, Self::Configuring)
                | (Self::Configuring, Self::Enabled)
                | (Self::Configuring, Self::Failed)
                | (Self::Enabled, Self::Prerolling)
                | (Self::Enabled, Self::Running)
                | (Self::Enabled, Self::Stopping)
                | (Self::Prerolling, Self::Running)
                | (Self::Prerolling, Self::Stopping)
                | (Self::Running, Self::Stopping)
                | (Self::Stopping, Self::Stopped)
                | (Self::Stopping, Self::Failed)
                | (Self::Idle, Self::Failed)
                | (_, Self::Disconnected)
                | (_, Self::Failed)
        );
        if allowed {
            Ok(next)
        } else {
            Err(Error::new(
                ErrorKind::InvalidState,
                "state",
                format!("cannot move from {self:?} to {next:?}"),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_can_configure() {
        assert_eq!(
            SessionState::Idle.transition(SessionState::Configuring).unwrap(),
            SessionState::Configuring
        );
    }

    #[test]
    fn running_cannot_skip_stop() {
        assert!(SessionState::Running.transition(SessionState::Idle).is_err());
    }
}
