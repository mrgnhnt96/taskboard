//! A scratch `taskboardd serve` stops on its own once whatever started it is gone (#1168).
//!
//! Agents start a daemon on a throwaway data folder to try a change. Nothing else ties its life to
//! the agent's, so one that isn't stopped by hand runs on under launchd for weeks. A daemon that
//! launchd started (Taskboard.app's LaunchAgents) is left alone; any other one stops when its data
//! folder is deleted, and, on a folder other than the default one, when the process that started it
//! exits. `--exit-with <pid>` ties any daemon to one more process.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// How often the watch looks.
pub const EVERY: Duration = Duration::from_secs(2);

/// How this process was started, read once at startup.
#[derive(Debug, Clone)]
pub struct Launch {
    pub ppid: u32,
    /// Leads its own process group, as a launchd job does (launchd calls `setsid`).
    pub group_leader: bool,
    /// `XPC_SERVICE_NAME`: launchd sets it to the job's label. Children inherit it (a Midna
    /// terminal's agents carry Midna's), so it only counts with the two checks above.
    pub xpc_label: Option<String>,
}

impl Launch {
    pub fn current() -> Launch {
        let pid = std::process::id();
        // SAFETY: getpgrp has no preconditions and can't fail.
        let pgid = unsafe { libc::getpgrp() } as u32;
        Launch {
            ppid: std::os::unix::process::parent_id(),
            group_leader: pgid == pid,
            xpc_label: std::env::var("XPC_SERVICE_NAME").ok().filter(|l| !l.is_empty() && l != "0"),
        }
    }

    /// Started by launchd as a job: its child from the start, in a group of its own, with a label.
    /// A daemon orphaned by a shell that exited also has parent 1, but stays in that shell's group.
    pub fn by_launchd(&self) -> bool {
        self.ppid == 1 && self.group_leader && self.xpc_label.is_some()
    }
}

/// What a running daemon watches; `None` from [`Watch::new`] when there's nothing to.
#[derive(Debug, Clone, PartialEq)]
pub struct Watch {
    pub data: Option<PathBuf>,
    /// The data folder's device and inode at startup: the daemon's timers recreate subfolders of a
    /// deleted folder, so it counts as gone once it isn't this same folder.
    pub data_id: Option<(u64, u64)>,
    pub parent: Option<u32>,
    pub exit_with: Option<u32>,
}

impl Watch {
    pub fn new(data: &Path, default_data: bool, launch: &Launch, exit_with: Option<u32>) -> Option<Watch> {
        let scratch = !launch.by_launchd();
        let w = Watch {
            data: scratch.then(|| data.to_path_buf()),
            data_id: folder_id(data),
            parent: (scratch && !default_data).then_some(launch.ppid),
            exit_with,
        };
        (w.data.is_some() || w.parent.is_some() || w.exit_with.is_some()).then_some(w)
    }

    /// Why to stop now, if it should: the data folder is gone (or is another folder now), the parent
    /// exited (this process was handed to launchd, or to another reaper), or the `--exit-with`
    /// process exited.
    pub fn reason(&self, ppid: u32, data_there: bool, alive: impl Fn(u32) -> bool) -> Option<String> {
        if let Some(d) = &self.data {
            if !data_there {
                return Some(format!("the data folder {} is gone", d.display()));
            }
        }
        if let Some(p) = self.parent {
            if ppid == 1 || ppid != p {
                return Some(format!("the process that started it ({p}) exited"));
            }
        }
        if let Some(p) = self.exit_with {
            if !alive(p) {
                return Some(format!("process {p} (--exit-with) exited"));
            }
        }
        None
    }

    /// Resolves with the reason once there is one.
    pub async fn wait(self) -> String {
        let mut tick = tokio::time::interval(EVERY);
        loop {
            tick.tick().await;
            let there = self.data.as_deref().is_none_or(|d| self.data_id.is_some() && folder_id(d) == self.data_id);
            if let Some(why) = self.reason(std::os::unix::process::parent_id(), there, alive) {
                return why;
            }
        }
    }
}

/// A folder's device and inode, or `None` when it isn't a folder.
pub fn folder_id(p: &Path) -> Option<(u64, u64)> {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).ok().filter(|m| m.is_dir()).map(|m| (m.dev(), m.ino()))
}

/// Whether a process with this pid is running (one owned by another user counts).
pub fn alive(pid: u32) -> bool {
    let Ok(pid) = i32::try_from(pid) else { return false };
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 only checks that the pid exists and may be signalled; nothing is sent.
    let sent = unsafe { libc::kill(pid, 0) };
    sent == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn launchd() -> Launch {
        Launch { ppid: 1, group_leader: true, xpc_label: Some("com.mrgnhnt.taskboard.daemon".into()) }
    }

    fn shell(ppid: u32) -> Launch {
        Launch { ppid, group_leader: false, xpc_label: Some("com.mrgnhnt.midna.daemon".into()) }
    }

    #[test]
    fn launchd_job_is_left_alone() {
        assert!(launchd().by_launchd());
        assert_eq!(Watch::new(Path::new("/d"), true, &launchd(), None), None);
        assert_eq!(Watch::new(Path::new("/scratch"), false, &launchd(), None), None);
    }

    #[test]
    fn orphan_of_a_shell_isnt_a_launchd_job() {
        let orphan = Launch { ppid: 1, group_leader: false, xpc_label: Some("com.mrgnhnt.midna.daemon".into()) };
        assert!(!orphan.by_launchd());
        let detached = Launch { ppid: 1, group_leader: true, xpc_label: None };
        assert!(!detached.by_launchd());
        let w = Watch::new(Path::new("/scratch"), false, &orphan, None).unwrap();
        assert_eq!(w.reason(1, true, |_| true).as_deref(), Some("the process that started it (1) exited"));
    }

    #[test]
    fn launchd_job_still_honours_exit_with() {
        let w = Watch::new(Path::new("/d"), true, &launchd(), Some(42)).unwrap();
        assert_eq!((w.data.clone(), w.parent, w.exit_with), (None, None, Some(42)));
        assert_eq!(w.reason(1, false, |_| true), None);
        assert_eq!(w.reason(1, true, |_| false).as_deref(), Some("process 42 (--exit-with) exited"));
    }

    #[test]
    fn scratch_daemon_watches_folder_and_parent() {
        let w = Watch::new(Path::new("/scratch"), false, &shell(500), None).unwrap();
        assert_eq!((w.data.clone(), w.parent, w.exit_with), (Some("/scratch".into()), Some(500), None));
        assert_eq!(w.reason(500, true, |_| true), None);
        assert_eq!(w.reason(500, false, |_| true).as_deref(), Some("the data folder /scratch is gone"));
        assert_eq!(w.reason(1, true, |_| true).as_deref(), Some("the process that started it (500) exited"));
        assert_eq!(w.reason(77, true, |_| true).as_deref(), Some("the process that started it (500) exited"));
    }

    #[test]
    fn default_folder_by_hand_only_watches_the_folder() {
        let w = Watch::new(Path::new("/home/.config/taskboard"), true, &shell(500), None).unwrap();
        assert_eq!(w.parent, None);
        assert_eq!(w.reason(1, true, |_| true), None);
        assert!(w.reason(1, false, |_| true).is_some());
    }

    #[test]
    fn folder_id_tells_a_recreated_folder_apart() {
        let tmp = tempfile::tempdir().unwrap();
        let d = tmp.path().join("data");
        std::fs::create_dir(&d).unwrap();
        let before = folder_id(&d);
        assert!(before.is_some());
        std::fs::create_dir(d.join("keep")).unwrap();
        assert_eq!(folder_id(&d), before);
        std::fs::remove_dir_all(&d).unwrap();
        assert_eq!(folder_id(&d), None);
        std::fs::create_dir(&d).unwrap();
        assert_ne!(folder_id(&d), before);
    }

    #[test]
    fn alive_sees_this_process_and_not_a_bad_pid() {
        assert!(alive(std::process::id()));
        assert!(!alive(0));
        assert!(!alive(u32::MAX));
    }
}
