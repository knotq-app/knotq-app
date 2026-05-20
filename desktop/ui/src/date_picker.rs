use chrono::{DateTime, Utc};

#[derive(Clone, Debug, PartialEq)]
pub struct DatePickerWidget {
    pub value: Option<DateTime<Utc>>,
}
