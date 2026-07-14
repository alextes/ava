use std::sync::{Arc, Mutex, RwLock};

use futures::future::{AbortHandle, AbortRegistration};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SteerOrigin {
    pub chat_id: i64,
    pub thread_id: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingSteer {
    pub text: String,
    pub origin: SteerOrigin,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StopOutcome {
    Accepted,
    AlreadyStopping,
    Inactive,
}

#[derive(Debug)]
pub struct TurnClose {
    pub stopped: bool,
    pub pending_steers: Vec<PendingSteer>,
}

#[derive(Clone, Default)]
pub struct RuntimeState {
    telegram_display_name: Arc<RwLock<String>>,
    turn: Arc<Mutex<ActiveTurnState>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TurnPhase {
    #[default]
    Idle,
    Running,
    Stopping,
    Completing,
}

#[derive(Default)]
struct ActiveTurnState {
    phase: TurnPhase,
    abort_handle: Option<AbortHandle>,
    pending: Vec<PendingSteer>,
}

impl RuntimeState {
    pub fn new(telegram_display_name: String) -> Self {
        Self {
            telegram_display_name: Arc::new(RwLock::new(telegram_display_name)),
            turn: Arc::new(Mutex::new(ActiveTurnState::default())),
        }
    }

    pub fn telegram_display_name(&self) -> String {
        self.telegram_display_name
            .read()
            .map(|s| s.clone())
            .unwrap_or_default()
    }

    pub fn set_telegram_display_name(&self, name: impl Into<String>) {
        if let Ok(mut guard) = self.telegram_display_name.write() {
            *guard = name.into();
        }
    }

    pub fn begin_turn(&self) -> AbortRegistration {
        let (abort_handle, registration) = AbortHandle::new_pair();
        if let Ok(mut guard) = self.turn.lock() {
            guard.phase = TurnPhase::Running;
            guard.abort_handle = Some(abort_handle);
            guard.pending.clear();
        }
        registration
    }

    pub fn try_stop(&self) -> StopOutcome {
        let Ok(mut guard) = self.turn.lock() else {
            return StopOutcome::Inactive;
        };

        match guard.phase {
            TurnPhase::Running => {
                guard.phase = TurnPhase::Stopping;
                if let Some(handle) = &guard.abort_handle {
                    handle.abort();
                }
                StopOutcome::Accepted
            }
            TurnPhase::Stopping => StopOutcome::AlreadyStopping,
            TurnPhase::Idle | TurnPhase::Completing => StopOutcome::Inactive,
        }
    }

    pub fn try_push_steer(&self, text: impl Into<String>, origin: SteerOrigin) -> bool {
        let text = text.into();
        if text.trim().is_empty() {
            return false;
        }

        let Ok(mut guard) = self.turn.lock() else {
            return false;
        };
        if guard.phase != TurnPhase::Running {
            return false;
        }

        guard.pending.push(PendingSteer { text, origin });
        true
    }

    pub fn drain_steers(&self) -> Vec<String> {
        self.turn
            .lock()
            .map(|mut guard| {
                if guard.phase != TurnPhase::Running {
                    return Vec::new();
                }
                guard.pending.drain(..).map(|steer| steer.text).collect()
            })
            .unwrap_or_default()
    }

    pub fn finish_turn_or_drain_steers(&self) -> Vec<String> {
        self.turn
            .lock()
            .map(|mut guard| {
                if guard.phase != TurnPhase::Running {
                    return Vec::new();
                }
                if guard.pending.is_empty() {
                    guard.phase = TurnPhase::Completing;
                    Vec::new()
                } else {
                    guard.pending.drain(..).map(|steer| steer.text).collect()
                }
            })
            .unwrap_or_default()
    }

    pub fn close_turn(&self) -> TurnClose {
        self.turn
            .lock()
            .map(|mut guard| {
                let stopped = guard.phase == TurnPhase::Stopping;
                guard.phase = TurnPhase::Idle;
                guard.abort_handle = None;
                TurnClose {
                    stopped,
                    pending_steers: guard.pending.drain(..).collect(),
                }
            })
            .unwrap_or(TurnClose {
                stopped: false,
                pending_steers: Vec::new(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn origin() -> SteerOrigin {
        SteerOrigin {
            chat_id: 1,
            thread_id: Some(2),
        }
    }

    #[test]
    fn test_steering_inactive_rejects() {
        let runtime = RuntimeState::new(String::new());
        assert!(!runtime.try_push_steer("be brief", origin()));
        assert!(runtime.drain_steers().is_empty());
    }

    #[test]
    fn test_steering_active_drains_in_order() {
        let runtime = RuntimeState::new(String::new());
        runtime.begin_turn();
        assert!(runtime.try_push_steer("first", origin()));
        assert!(runtime.try_push_steer("second", origin()));

        assert_eq!(runtime.drain_steers(), vec!["first", "second"]);
        assert!(runtime.drain_steers().is_empty());
    }

    #[test]
    fn test_finish_turn_or_drain_steers_drains_before_closing() {
        let runtime = RuntimeState::new(String::new());
        runtime.begin_turn();
        assert!(runtime.try_push_steer("first", origin()));

        assert_eq!(runtime.finish_turn_or_drain_steers(), vec!["first"]);
        assert!(runtime.try_push_steer("second", origin()));
        assert_eq!(runtime.finish_turn_or_drain_steers(), vec!["second"]);

        assert!(runtime.finish_turn_or_drain_steers().is_empty());
        assert!(!runtime.try_push_steer("third", origin()));
    }

    #[test]
    fn test_close_turn_returns_pending_steers_and_closes() {
        let runtime = RuntimeState::new(String::new());
        runtime.begin_turn();
        assert!(runtime.try_push_steer("first", origin()));

        let close = runtime.close_turn();
        assert!(!close.stopped);
        assert_eq!(close.pending_steers.len(), 1);
        assert_eq!(close.pending_steers[0].text, "first");
        assert_eq!(close.pending_steers[0].origin, origin());
        assert!(!runtime.try_push_steer("second", origin()));
    }

    #[test]
    fn test_stop_aborts_active_turn_once() {
        let runtime = RuntimeState::new(String::new());
        let registration = runtime.begin_turn();

        assert_eq!(runtime.try_stop(), StopOutcome::Accepted);
        let aborted = futures::executor::block_on(futures::future::Abortable::new(
            futures::future::pending::<()>(),
            registration,
        ));
        assert!(aborted.is_err());
        assert_eq!(runtime.try_stop(), StopOutcome::AlreadyStopping);
        assert!(!runtime.try_push_steer("too late", origin()));

        let close = runtime.close_turn();
        assert!(close.stopped);
        assert_eq!(runtime.try_stop(), StopOutcome::Inactive);
    }

    #[test]
    fn test_completing_turn_rejects_late_stop() {
        let runtime = RuntimeState::new(String::new());
        runtime.begin_turn();

        assert!(runtime.finish_turn_or_drain_steers().is_empty());
        assert_eq!(runtime.try_stop(), StopOutcome::Inactive);
        assert!(!runtime.close_turn().stopped);
    }
}
