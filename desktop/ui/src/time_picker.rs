use knotq_model::TimeFormat;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimePickerWidget {
    pub hour: u32,
    pub minute: u32,
    pub format: TimeFormat,
}
