use chrono::Datelike;
use knotq_model::{Item, RepeatWeekday};

pub const ALL_WEEKDAYS_FROM_SUNDAY: [RepeatWeekday; 7] = [
    RepeatWeekday::Sun,
    RepeatWeekday::Mon,
    RepeatWeekday::Tue,
    RepeatWeekday::Wed,
    RepeatWeekday::Thu,
    RepeatWeekday::Fri,
    RepeatWeekday::Sat,
];

pub fn repeat_weekday_from_index(index: u32) -> RepeatWeekday {
    match index % 7 {
        0 => RepeatWeekday::Mon,
        1 => RepeatWeekday::Tue,
        2 => RepeatWeekday::Wed,
        3 => RepeatWeekday::Thu,
        4 => RepeatWeekday::Fri,
        5 => RepeatWeekday::Sat,
        _ => RepeatWeekday::Sun,
    }
}

pub fn weekday_index(weekday: RepeatWeekday) -> u32 {
    match weekday {
        RepeatWeekday::Mon => 0,
        RepeatWeekday::Tue => 1,
        RepeatWeekday::Wed => 2,
        RepeatWeekday::Thu => 3,
        RepeatWeekday::Fri => 4,
        RepeatWeekday::Sat => 5,
        RepeatWeekday::Sun => 6,
    }
}

pub fn default_weekday_for_item(item: &Item) -> RepeatWeekday {
    item.start
        .or(item.end)
        .or(item.available)
        .map(|dt| dt.weekday().num_days_from_monday())
        .map(repeat_weekday_from_index)
        .unwrap_or(RepeatWeekday::Mon)
}

pub fn repeat_weekday_initial(weekday: RepeatWeekday) -> &'static str {
    match weekday {
        RepeatWeekday::Mon => "M",
        RepeatWeekday::Tue => "T",
        RepeatWeekday::Wed => "W",
        RepeatWeekday::Thu => "T",
        RepeatWeekday::Fri => "F",
        RepeatWeekday::Sat => "S",
        RepeatWeekday::Sun => "S",
    }
}

pub fn repeat_weekday_rrule_code(weekday: RepeatWeekday) -> &'static str {
    match weekday {
        RepeatWeekday::Mon => "MO",
        RepeatWeekday::Tue => "TU",
        RepeatWeekday::Wed => "WE",
        RepeatWeekday::Thu => "TH",
        RepeatWeekday::Fri => "FR",
        RepeatWeekday::Sat => "SA",
        RepeatWeekday::Sun => "SU",
    }
}
