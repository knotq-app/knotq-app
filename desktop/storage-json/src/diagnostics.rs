//! Append-only diagnostic logs, with a ceiling.
//!
//! `knotq-notif.log` and `knotq-google.log` are the two files support asks for
//! when a notification does not fire or a calendar import goes wrong, so they
//! have to stay on disk between sessions — but they were opened in append mode
//! and never trimmed. Observed on a real install: 4.9 MB and 3.7 MB, growing for
//! as long as the app has been used, in a data directory the user never looks
//! at. Rotation keeps the recent history (which is the useful part) and puts a
//! bound on the rest.

use std::fs;
use std::io::Write;
use std::path::Path;

/// Rotate at this size, keeping one previous generation — so a log costs at most
/// twice this on disk. Large enough to hold a long session's worth of lines,
/// which is as far back as these logs are ever read.
const MAX_BYTES: u64 = 1024 * 1024;

/// Append one line to a diagnostic log in `dir`, rotating it if it has grown
/// past the ceiling.
///
/// Never fails: a diagnostic log that cannot be written must not disturb the
/// thing it is describing.
pub fn append_diagnostic_line(dir: &Path, file_name: &str, line: &str) {
    let _ = fs::create_dir_all(dir);
    let path = dir.join(file_name);
    rotate_if_large(&path, MAX_BYTES);
    if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "{line}");
    }
}

/// Move the log aside to `<name>.1` once it passes `max_bytes`, replacing any
/// previous generation. Rename rather than truncate: a reader holding the file
/// open keeps reading a complete file instead of watching it empty underneath
/// them.
fn rotate_if_large(path: &Path, max_bytes: u64) {
    let Ok(metadata) = fs::metadata(path) else {
        return;
    };
    if metadata.len() < max_bytes {
        return;
    }
    let previous = path.with_extension(match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) => format!("{ext}.1"),
        None => "1".to_string(),
    });
    let _ = fs::rename(path, previous);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "knotq-diag-{name}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_log_that_grows_past_the_ceiling_is_rotated_not_lost() {
        let dir = temp_dir("rotate");
        let path = dir.join("knotq-notif.log");

        // Fill past the ceiling, then write one more line.
        fs::write(&path, vec![b'x'; MAX_BYTES as usize + 1]).unwrap();
        append_diagnostic_line(&dir, "knotq-notif.log", "after rotation");

        let rotated = dir.join("knotq-notif.log.1");
        assert!(rotated.exists(), "the previous generation must be kept");
        assert_eq!(
            fs::metadata(&rotated).unwrap().len(),
            MAX_BYTES + 1,
            "the rotated file must be the whole previous log"
        );
        assert_eq!(fs::read_to_string(&path).unwrap(), "after rotation\n");

        // A second rotation replaces the previous generation rather than
        // accumulating `.2`, `.3`, ... — the point is a bounded ceiling.
        fs::write(&path, vec![b'y'; MAX_BYTES as usize + 1]).unwrap();
        append_diagnostic_line(&dir, "knotq-notif.log", "after the second rotation");
        assert_eq!(
            fs::read_dir(&dir)
                .unwrap()
                .flatten()
                .filter(|entry| entry.file_name().to_string_lossy().contains("knotq-notif"))
                .count(),
            2,
            "at most the live log and one previous generation"
        );
        assert!(fs::read(&rotated).unwrap().starts_with(b"yyy"));

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn a_small_log_is_appended_to_in_place() {
        let dir = temp_dir("append");
        for line in ["first", "second", "third"] {
            append_diagnostic_line(&dir, "knotq-google.log", line);
        }
        assert_eq!(
            fs::read_to_string(dir.join("knotq-google.log")).unwrap(),
            "first\nsecond\nthird\n"
        );
        assert!(!dir.join("knotq-google.log.1").exists());
        let _ = fs::remove_dir_all(dir);
    }
}
