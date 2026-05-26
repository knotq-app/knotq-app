pub(super) use knotq_rrule::weekday_util::{
    default_weekday_for_item as default_repeat_weekday, repeat_weekday_initial,
    repeat_weekday_rrule_code, weekday_index as repeat_weekday_index, ALL_WEEKDAYS_FROM_SUNDAY,
};
use knotq_model::RepeatWeekday;

pub(super) fn repeat_weekdays_for_popup() -> [RepeatWeekday; 7] {
    ALL_WEEKDAYS_FROM_SUNDAY
}
