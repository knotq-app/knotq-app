use knotq_model::ItemMarker;

pub fn marker_prefix(marker: ItemMarker, number: usize) -> String {
    match marker {
        ItemMarker::Blank => String::new(),
        ItemMarker::Checkbox => "- [ ] ".to_string(),
        ItemMarker::Bullet => "- ".to_string(),
        ItemMarker::Numbered => format!("{}. ", number.max(1)),
    }
}

pub fn parse_marker_prefix(line: &str) -> (ItemMarker, usize) {
    let leading = line.len() - line.trim_start().len();
    let trimmed = &line[leading..];
    if trimmed.starts_with("- [ ] ") {
        return (ItemMarker::Checkbox, leading + 6);
    }
    if trimmed.starts_with("- ") {
        return (ItemMarker::Bullet, leading + 2);
    }
    if let Some((prefix, _rest)) = trimmed.split_once(". ") {
        if !prefix.is_empty() && prefix.chars().all(|ch| ch.is_ascii_digit()) {
            return (ItemMarker::Numbered, leading + prefix.len() + 2);
        }
    }
    (ItemMarker::Blank, 0)
}

pub fn parse_marker_content(line: &str) -> (ItemMarker, &str) {
    let (marker, offset) = parse_marker_prefix(line);
    (marker, &line[offset..])
}
