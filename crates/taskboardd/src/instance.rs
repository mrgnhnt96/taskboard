//! One board per data folder: `serve` holds `taskboardd.lock` in it for as long as it runs, so a
//! second `taskboardd serve` on the same folder (another port, a second LaunchAgent) doesn't run a
//! second runner, sync and set of jobs against the same tasks.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, Write};
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The lock, held until it's dropped (or the process ends).
pub struct Instance {
    _file: File,
}

pub enum Refused {
    /// Another board holds the folder: its process id, when it wrote one.
    Held(Option<u32>),
    Io(std::io::Error),
}

impl std::fmt::Display for Refused {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refused::Held(Some(pid)) => write!(f, "another board (process {pid}) is running on this data folder"),
            Refused::Held(None) => f.write_str("another board is running on this data folder"),
            Refused::Io(e) => write!(f, "can't lock the data folder: {e}"),
        }
    }
}

pub fn lock_path(data: &Path) -> PathBuf {
    data.join("taskboardd.lock")
}

/// Takes the data folder's lock, waiting up to `wait` for a board that's still stopping to let it go.
pub fn hold(data: &Path, wait: Duration) -> Result<Instance, Refused> {
    std::fs::create_dir_all(data).map_err(Refused::Io)?;
    let mut file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(lock_path(data)).map_err(Refused::Io)?;
    let until = Instant::now() + wait;
    loop {
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            break;
        }
        let e = std::io::Error::last_os_error();
        if e.raw_os_error() != Some(libc::EWOULDBLOCK) {
            return Err(Refused::Io(e));
        }
        if Instant::now() >= until {
            let mut pid = String::new();
            let _ = file.read_to_string(&mut pid);
            return Err(Refused::Held(pid.trim().parse().ok()));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    // Who holds it, for the one turned away.
    let _ = file.set_len(0).and_then(|_| file.rewind()).and_then(|_| write!(file, "{}", std::process::id()));
    Ok(Instance { _file: file })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_board_on_the_folder_is_refused_until_the_first_lets_go() {
        let dir = tempfile::tempdir().unwrap();
        let first = hold(dir.path(), Duration::ZERO).ok().expect("the first board takes the folder");
        match hold(dir.path(), Duration::from_millis(300)) {
            Err(Refused::Held(pid)) => assert_eq!(pid, Some(std::process::id())),
            Err(e) => panic!("{e}"),
            Ok(_) => panic!("a second board took a held folder"),
        }
        drop(first);
        assert!(hold(dir.path(), Duration::ZERO).is_ok(), "the folder is free once the first board stops");
    }

    #[test]
    fn a_board_waits_for_one_that_is_stopping() {
        let dir = tempfile::tempdir().unwrap();
        let first = hold(dir.path(), Duration::ZERO).ok().unwrap();
        let stopping = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            drop(first);
        });
        assert!(hold(dir.path(), Duration::from_secs(5)).is_ok());
        stopping.join().unwrap();
    }

    #[test]
    fn boards_on_other_folders_both_run() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let _first = hold(a.path(), Duration::ZERO).ok().unwrap();
        assert!(hold(b.path(), Duration::ZERO).is_ok());
    }
}
