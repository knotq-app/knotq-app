use thiserror::Error;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorkspaceNodeNameKind {
    Folder,
    Scheme,
}

#[derive(Clone, Debug, Eq, Error, PartialEq)]
pub enum WorkspaceNodeNameError {
    #[error("name cannot be empty")]
    Empty,
    #[error("name cannot start or end with whitespace")]
    OuterWhitespace,
    #[error("name cannot be . or ..")]
    DotSegment,
    #[error("name contains a path separator")]
    PathSeparator,
    #[error("name contains reserved filesystem character {0:?}")]
    ReservedCharacter(char),
    #[error("name contains control character U+{0:04X}")]
    ControlCharacter(u32),
    #[error("name {0:?} is reserved")]
    ReservedName(String),
    #[error("scheme names cannot end in .knotq")]
    SchemeExtension,
}

pub fn validate_workspace_node_name(
    name: &str,
    kind: WorkspaceNodeNameKind,
) -> Result<(), WorkspaceNodeNameError> {
    if name.is_empty() {
        return Err(WorkspaceNodeNameError::Empty);
    }
    if name.trim() != name {
        return Err(WorkspaceNodeNameError::OuterWhitespace);
    }
    if name == "." || name == ".." {
        return Err(WorkspaceNodeNameError::DotSegment);
    }
    for ch in name.chars() {
        match ch {
            '/' | '\\' => return Err(WorkspaceNodeNameError::PathSeparator),
            ':' | '*' | '?' | '"' | '<' | '>' | '|' => {
                return Err(WorkspaceNodeNameError::ReservedCharacter(ch));
            }
            ch if ch.is_control() => {
                return Err(WorkspaceNodeNameError::ControlCharacter(ch as u32));
            }
            _ => {}
        }
    }

    let normalized = name.trim_end_matches('.').to_ascii_uppercase();
    if matches!(
        normalized.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    ) {
        return Err(WorkspaceNodeNameError::ReservedName(name.to_string()));
    }

    if name.starts_with('.') {
        return Err(WorkspaceNodeNameError::ReservedName(name.to_string()));
    }

    if kind == WorkspaceNodeNameKind::Scheme && name.to_ascii_lowercase().ends_with(".knotq") {
        return Err(WorkspaceNodeNameError::SchemeExtension);
    }

    Ok(())
}
