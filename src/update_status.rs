use chrono::prelude::*;
use std::time::UNIX_EPOCH;

#[derive(Debug, PartialEq, Clone)]
pub struct UpdateStatus {
    pub message: String,
    pub succeed: bool,
    pub time: DateTime<Utc>,
    pub last_modified: DateTime<Utc>,
}

impl UpdateStatus {
    pub fn new_ok(time: DateTime<Utc>, last_modified: DateTime<Utc>) -> Self {
        Self {
            message: String::new(),
            succeed: true,
            time,
            last_modified,
        }
    }

    pub fn new_fail(time: DateTime<Utc>, message: String) -> Self {
        Self {
            message,
            succeed: false,
            time,
            last_modified: UNIX_EPOCH.into(),
        }
    }

    pub fn format_time(&self) -> String {
        self.time.format("%F %T").to_string()
    }
}
