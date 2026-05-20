pub const DAILY_QUEUE_TITLE: &str = "Daily";
pub const DAILY_QUEUE_COLOR_INDEX: u8 = 0;
pub const PAGE_DAYS: i64 = 31;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DailyQueueConfig;

impl DailyQueueConfig {
    pub const TITLE: &'static str = DAILY_QUEUE_TITLE;
    pub const COLOR_INDEX: u8 = DAILY_QUEUE_COLOR_INDEX;
    pub const PAGE_DAYS: i64 = PAGE_DAYS;
}
