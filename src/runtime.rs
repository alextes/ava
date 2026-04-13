use std::sync::{Arc, RwLock};

#[derive(Clone, Default)]
pub struct RuntimeState {
    telegram_display_name: Arc<RwLock<String>>,
}

impl RuntimeState {
    pub fn new(telegram_display_name: String) -> Self {
        Self {
            telegram_display_name: Arc::new(RwLock::new(telegram_display_name)),
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
}
