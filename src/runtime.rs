use std::sync::{Arc, Mutex, RwLock};

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

#[derive(Clone, Default)]
pub struct RuntimeState {
    telegram_display_name: Arc<RwLock<String>>,
    steering: Arc<Mutex<SteeringState>>,
}

#[derive(Default)]
struct SteeringState {
    active: bool,
    pending: Vec<PendingSteer>,
}

impl RuntimeState {
    pub fn new(telegram_display_name: String) -> Self {
        Self {
            telegram_display_name: Arc::new(RwLock::new(telegram_display_name)),
            steering: Arc::new(Mutex::new(SteeringState::default())),
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

    pub fn begin_turn(&self) {
        if let Ok(mut guard) = self.steering.lock() {
            guard.active = true;
            guard.pending.clear();
        }
    }

    pub fn try_push_steer(&self, text: impl Into<String>, origin: SteerOrigin) -> bool {
        let text = text.into();
        if text.trim().is_empty() {
            return false;
        }

        let Ok(mut guard) = self.steering.lock() else {
            return false;
        };
        if !guard.active {
            return false;
        }

        guard.pending.push(PendingSteer { text, origin });
        true
    }

    pub fn drain_steers(&self) -> Vec<String> {
        self.steering
            .lock()
            .map(|mut guard| guard.pending.drain(..).map(|steer| steer.text).collect())
            .unwrap_or_default()
    }

    pub fn finish_turn_or_drain_steers(&self) -> Vec<String> {
        self.steering
            .lock()
            .map(|mut guard| {
                if guard.pending.is_empty() {
                    guard.active = false;
                    Vec::new()
                } else {
                    guard.pending.drain(..).map(|steer| steer.text).collect()
                }
            })
            .unwrap_or_default()
    }

    pub fn close_turn(&self) -> Vec<PendingSteer> {
        self.steering
            .lock()
            .map(|mut guard| {
                guard.active = false;
                guard.pending.drain(..).collect()
            })
            .unwrap_or_default()
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

        let rejected = runtime.close_turn();
        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].text, "first");
        assert_eq!(rejected[0].origin, origin());
        assert!(!runtime.try_push_steer("second", origin()));
    }
}
