use chrono::prelude::*;

#[derive(Debug, PartialEq, Clone)]
pub struct UpdateStatus {
    pub message: String,
    pub succeed: bool,
    pub time: DateTime<Utc>,
}

impl UpdateStatus {
    pub fn new_ok(time: DateTime<Utc>) -> Self {
        Self {
            message: String::new(),
            succeed: true,
            time,
        }
    }

    pub fn new_fail(time: DateTime<Utc>, message: String) -> Self {
        Self {
            message,
            succeed: false,
            time,
        }
    }

    pub fn format_time(&self) -> String {
        self.time.format("%F %T").to_string()
    }
}
