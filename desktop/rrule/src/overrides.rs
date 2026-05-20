use knotq_model::{Item, Occurrence, OccurrenceOverride};

pub(crate) fn apply_override(
    occurrence: &mut Occurrence,
    item: &Item,
    override_: &OccurrenceOverride,
) {
    let original_start = occurrence.start;
    if let Some(start) = override_.start {
        occurrence.start = Some(start);
        if override_.end.is_none() {
            if let (Some(original_start), Some(original_end)) = (item.start, item.end) {
                occurrence.end = Some(start + (original_end - original_start));
            }
        }
    }
    if let Some(end) = override_.end {
        occurrence.end = Some(end);
    }
    if let Some(available) = override_.available {
        occurrence.available = Some(available);
    }
    if occurrence.start != original_start {
        occurrence.state = item.state_for_occurrence(&occurrence.id);
    }
}

pub fn apply_overrides(
    mut occurrences: Vec<Occurrence>,
    item: &Item,
    overrides: &[OccurrenceOverride],
) -> Vec<Occurrence> {
    for occurrence in &mut occurrences {
        if let Some(override_) = overrides
            .iter()
            .find(|override_| override_.occurrence == occurrence.id)
        {
            apply_override(occurrence, item, override_);
        }
    }
    occurrences
}
