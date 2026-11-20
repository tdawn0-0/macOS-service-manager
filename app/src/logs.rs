//! Bounded reads of the end of a log file, ported from `internal/logs`.
//!
//! A job's output can grow without bound, so the inspector only ever reads
//! the last [`MAX_BYTES`] bytes and keeps the last [`MAX_LINES`] lines. The
//! read runs on a blocking thread, never in render code.

use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
    time::SystemTime,
};

/// The most bytes read from the end of a log.
pub(crate) const MAX_BYTES: u64 = 64 * 1024;

/// The most lines shown at once.
pub(crate) const MAX_LINES: usize = 200;

/// The end of a log file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Tail {
    pub lines: Vec<String>,
    /// The file holds more before these lines.
    pub truncated: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// Reads the end of `path`: at most [`MAX_LINES`] lines, from its last
/// [`MAX_BYTES`] bytes. A file that cannot be read answers its error text.
pub(crate) fn tail(path: &Path) -> Result<Tail, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let info = file.metadata().map_err(|error| error.to_string())?;
    let size = info.len();
    let offset = size.saturating_sub(MAX_BYTES);
    let mut snapshot = Tail {
        truncated: size > MAX_BYTES,
        size,
        modified: info.modified().ok(),
        ..Tail::default()
    };
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| error.to_string())?;
    let mut buffer = Vec::with_capacity((size - offset) as usize);
    file.take(size - offset)
        .read_to_end(&mut buffer)
        .map_err(|error| error.to_string())?;
    if offset > 0 {
        // The first line is probably cut: drop it.
        if let Some(index) = buffer.iter().position(|byte| *byte == b'\n') {
            buffer.drain(..=index);
        }
    }
    let text = String::from_utf8_lossy(&buffer);
    let text = text.trim_end_matches('\n').replace("\r\n", "\n");
    if text.is_empty() {
        return Ok(snapshot);
    }
    let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
    if lines.len() > MAX_LINES {
        snapshot.truncated = true;
        lines.drain(..lines.len() - MAX_LINES);
    }
    snapshot.lines = lines;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    #[test]
    fn tail_reads_the_end_and_reports_the_cut() {
        let dir = std::env::temp_dir().join(format!("devbase-logs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("out.log");
        let mut file = std::fs::File::create(&path).unwrap();
        for i in 0..(MAX_LINES + 20) {
            writeln!(file, "line {i}").unwrap();
        }
        drop(file);
        let snapshot = tail(&path).unwrap();
        assert_eq!(snapshot.lines.len(), MAX_LINES);
        assert!(snapshot.truncated);
        assert_eq!(
            snapshot.lines.last().map(String::as_str),
            Some("line 219")
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_log_has_no_lines() {
        let dir = std::env::temp_dir().join(format!("devbase-logs-empty-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("empty.log");
        std::fs::write(&path, "").unwrap();
        let snapshot = tail(&path).unwrap();
        assert!(snapshot.lines.is_empty());
        assert!(!snapshot.truncated);
        std::fs::remove_dir_all(&dir).ok();
    }
}
