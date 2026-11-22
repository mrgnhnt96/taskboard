//! This Mac's facts, shared by "Report an issue" (`report.rs`) and the debug export (`export.rs`):
//! macOS, hardware, architecture, how the binary was installed, and how long something has run.
use std::process::Command;

/// What [`gather`] reads. Each string is empty when it couldn't be read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Machine {
    /// "15.1".
    pub macos: String,
    /// "24B83".
    pub macos_build: String,
    /// "Mac15,6".
    pub model: String,
    /// "Apple M3 Pro".
    pub cpu: String,
    pub memory_gb: Option<u64>,
    /// Running translated by Rosetta.
    pub rosetta: bool,
    /// "/Applications", "dev build (cargo)", ...
    pub install: String,
}

/// Runs `sysctl` and `sw_vers`, so it belongs off the UI thread.
pub fn gather(home: &str) -> Machine {
    let sysctl = |k: &str| output("/usr/sbin/sysctl", &["-n", k]);
    let exe = std::env::current_exe().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    Machine {
        macos: output("/usr/bin/sw_vers", &["-productVersion"]),
        macos_build: output("/usr/bin/sw_vers", &["-buildVersion"]),
        model: sysctl("hw.model"),
        cpu: sysctl("machdep.cpu.brand_string"),
        memory_gb: sysctl("hw.memsize").parse::<u64>().ok().map(|b| b >> 30),
        rosetta: sysctl("sysctl.proc_translated") == "1",
        install: install_kind(&exe, home),
    }
}

/// A command's trimmed stdout, empty when it fails or can't run.
pub fn output(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Where the binary runs from, without the home folder's name.
pub fn install_kind(exe: &str, home: &str) -> String {
    if exe.starts_with("/Applications/") {
        "/Applications".into()
    } else if !home.is_empty() && exe.starts_with(&format!("{home}/Applications/")) {
        "~/Applications".into()
    } else if exe.contains("/target/debug/") || exe.contains("/target/release/") {
        "dev build (cargo)".into()
    } else if exe.contains(".app/") {
        "app bundle elsewhere".into()
    } else {
        "other".into()
    }
}

/// "42m", "3h 2m", "2d 5h".
pub fn uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_kind_hides_the_home_folder() {
        assert_eq!(install_kind("/Applications/Taskboard.app/Contents/MacOS/tb", "/Users/x"), "/Applications");
        assert_eq!(install_kind("/Users/x/Applications/Taskboard.app/Contents/MacOS/tb", "/Users/x"), "~/Applications");
        assert_eq!(install_kind("/Users/x/dev/taskboard/target/debug/tb", "/Users/x"), "dev build (cargo)");
        assert_eq!(install_kind("/Volumes/T/Taskboard.app/Contents/MacOS/tb", "/Users/x"), "app bundle elsewhere");
        assert_eq!(install_kind("/usr/local/bin/tb", ""), "other");
    }

    #[test]
    fn uptime_reads_in_its_largest_units() {
        assert_eq!(uptime(59), "0m");
        assert_eq!(uptime(4000), "1h 6m");
        assert_eq!(uptime(2 * 86_400 + 5 * 3600 + 60), "2d 5h");
    }
}
