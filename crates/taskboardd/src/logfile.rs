//! A log file that rolls over: `server.log` and the app's `app.log` grew without end. Once the file
//! reaches its cap it moves to `<name>.1`, the one before to `<name>.2`, the oldest is dropped, and a
//! fresh file starts.

use std::fs::File;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Where a log rolls over: a few days of a busy board.
pub const CAP: u64 = 4 * 1024 * 1024;
/// Old files kept beside the live one.
const KEEP: usize = 2;

pub struct Log {
    path: PathBuf,
    file: Option<File>,
    cap: u64,
}

impl Log {
    pub fn open(path: PathBuf, cap: u64) -> Log {
        let file = append(&path);
        Log { path, file, cap }
    }

    /// Appends `line`, rolling the file over once it's at the cap. A line that can't be written is dropped.
    pub fn write(&mut self, line: &str) {
        let Some(f) = self.file.as_mut() else { return };
        let _ = f.write_all(line.as_bytes());
        if f.metadata().is_ok_and(|m| m.len() >= self.cap) {
            rotate(&self.path);
            self.file = append(&self.path);
        }
    }
}

fn append(path: &Path) -> Option<File> {
    std::fs::OpenOptions::new().create(true).append(true).open(path).ok()
}

/// `<path>.1` moves to `<path>.2` (dropping the one there), then `path` to `<path>.1`.
fn rotate(path: &Path) {
    for n in (1..KEEP).rev() {
        let _ = std::fs::rename(numbered(path, n), numbered(path, n + 1));
    }
    let _ = std::fs::rename(path, numbered(path, 1));
}

fn numbered(path: &Path, n: usize) -> PathBuf {
    let mut p = path.as_os_str().to_owned();
    p.push(format!(".{n}"));
    PathBuf::from(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(p: &Path) -> String {
        std::fs::read_to_string(p).unwrap_or_default()
    }

    #[test]
    fn rolls_over_at_the_cap_keeping_two_old_files() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("server.log");
        let mut log = Log::open(path.clone(), 10);
        for line in ["one 12345\n", "two 12345\n", "three 123\n", "four 1234\n"] {
            log.write(line);
        }
        assert_eq!(read(&path), "", "the fourth line filled the file, so a fresh one started");
        assert_eq!(read(&numbered(&path, 1)), "four 1234\n");
        assert_eq!(read(&numbered(&path, 2)), "three 123\n");
        assert!(!numbered(&path, 3).exists(), "older files are dropped");
        log.write("five\n");
        assert_eq!(read(&path), "five\n");
    }

    #[test]
    fn a_file_under_the_cap_stays_put() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("app.log");
        std::fs::write(&path, "before\n").unwrap();
        let mut log = Log::open(path.clone(), 1024);
        log.write("after\n");
        assert_eq!(read(&path), "before\nafter\n");
        assert!(!numbered(&path, 1).exists());
    }
}
