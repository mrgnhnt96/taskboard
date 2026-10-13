//! Built-in focus for an iOS simulator or an Android emulator that has a target, used when neither the
//! device nor `[devices] focus` gives a command (the Python board needed no setup for these):
//!
//! - iOS: the target is a simulator UDID. Its name comes from `xcrun simctl list devices --json`, and the
//!   Simulator window whose title starts with "<name> – " is raised. With no such window, Simulator is
//!   opened on that UDID and the raise is tried again.
//! - Android: the target is `emulator-<port>`, and the `qemu-system…` window whose title ends with
//!   ":<port>" is raised.
//!
//! Windows are raised through System Events, which needs the Accessibility permission.

use std::path::Path;

use serde_json::Value;

use crate::proc::{run_with, RunError};

#[derive(Debug, PartialEq)]
pub enum Builtin {
    Ios,
    Android,
}

/// The built-in focus for a device's kind and target: an iOS simulator with a UDID, or an Android
/// emulator with an `emulator-<port>` target.
pub fn builtin_for(kind: &str, target: &str) -> Option<Builtin> {
    if target.trim().is_empty() {
        return None;
    }
    match crate::devices::kind_label(kind).as_str() {
        "iOS simulator" => Some(Builtin::Ios),
        "Android emulator" => Some(Builtin::Android),
        _ => None,
    }
}

/// Raises the device's window. The error is a sentence for whoever asked.
pub fn focus(name: &str, kind: &str, target: &str, timeout: f64) -> Result<(), String> {
    match builtin_for(kind, target) {
        Some(Builtin::Ios) => focus_ios(name, target.trim(), timeout),
        Some(Builtin::Android) => focus_android(name, target.trim(), timeout),
        None => Err(format!("The board can't find a window for {name}.")),
    }
}

fn focus_ios(name: &str, udid: &str, timeout: f64) -> Result<(), String> {
    let list = run_with(Path::new("/usr/bin/xcrun"), &args(&["simctl", "list", "devices", "--json"]), None, timeout, &[], None);
    let sim = match list {
        Ok(o) if o.code == Some(0) => sim_name(&o.stdout, udid),
        _ => None,
    };
    let Some(sim) = sim else { return Err(format!("{name}'s simulator isn't on this Mac.")) };
    let script = raise_script("name is \"Simulator\"", &format!("starts with \"{}\"", quoted(&format!("{sim} – "))));
    if raise(name, &script, timeout)? {
        return Ok(());
    }
    let _ = run_with(Path::new("/usr/bin/open"), &args(&["-a", "Simulator", "--args", "-CurrentDeviceUDID", udid]), None, timeout, &[], None);
    for _ in 0..5 {
        std::thread::sleep(std::time::Duration::from_secs(1));
        if raise(name, &script, timeout)? {
            return Ok(());
        }
    }
    Err(format!("{name} is running, but its window didn't open."))
}

fn focus_android(name: &str, target: &str, timeout: f64) -> Result<(), String> {
    let Some(port) = emulator_port(target) else { return Err(format!("The board can't find a window for {name}.")) };
    let script = raise_script("name starts with \"qemu-system\"", &format!("ends with \":{port}\""));
    if raise(name, &script, timeout)? {
        return Ok(());
    }
    Err(format!("{name} isn't running."))
}

/// Runs a raise script: true when it raised a window, false when it found none.
fn raise(name: &str, script: &str, timeout: f64) -> Result<bool, String> {
    match run_with(Path::new("/usr/bin/osascript"), &args(&["-e", script]), None, timeout, &[], None) {
        Ok(o) if o.code == Some(0) => Ok(o.stdout.trim() == "ok"),
        Ok(o) => Err(osascript_error(name, &o.stderr)),
        Err(RunError::TimedOut) => Err(format!("Raising {name}'s window took over {timeout} seconds.")),
        Err(RunError::Spawn(e)) => Err(format!("The board can't find a window for {name}: {e}.")),
    }
}

/// What a failed raise means: no Accessibility permission, or a window that couldn't be found.
pub fn osascript_error(name: &str, stderr: &str) -> String {
    let low = stderr.to_lowercase();
    if low.contains("assistive access") || low.contains("-1719") || low.contains("-25211") || low.contains("not allowed to send keystrokes") {
        return format!(
            "Taskboard needs the Accessibility permission to raise {name}'s window. Grant it in System Settings › Privacy & Security › Accessibility."
        );
    }
    let said = crate::util::one_line(stderr, 200);
    if said.is_empty() {
        format!("The board can't find a window for {name}.")
    } else {
        format!("The board can't find a window for {name}: {said}")
    }
}

/// An AppleScript that raises the first window, in the processes `procs` picks, whose name matches
/// `title` (both System Events conditions), and prints "ok", or "none" when there's no such window.
pub fn raise_script(procs: &str, title: &str) -> String {
    format!(
        "tell application \"System Events\"\n\
         repeat with p in (every process whose {procs})\n\
         repeat with w in (windows of p)\n\
         if name of w {title} then\n\
         perform action \"AXRaise\" of w\n\
         set frontmost of p to true\n\
         return \"ok\"\n\
         end if\n\
         end repeat\n\
         end repeat\n\
         end tell\n\
         return \"none\""
    )
}

/// A string as it goes between AppleScript quotes.
fn quoted(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// The simulator's name for a UDID, from `xcrun simctl list devices --json`.
pub fn sim_name(json: &str, udid: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json).ok()?;
    v["devices"].as_object()?.values().filter_map(|l| l.as_array()).flatten().find(|d| d["udid"].as_str().is_some_and(|u| u.eq_ignore_ascii_case(udid))).and_then(|d| d["name"].as_str().map(str::to_string))
}

/// The console port in an `emulator-<port>` target.
pub fn emulator_port(target: &str) -> Option<u16> {
    target.trim().strip_prefix("emulator-")?.parse().ok()
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_simulator_or_an_emulator_with_a_target_has_a_built_in_focus() {
        assert_eq!(builtin_for("ios", "A1B2-UDID"), Some(Builtin::Ios));
        assert_eq!(builtin_for("simulator", "A1B2-UDID"), Some(Builtin::Ios));
        assert_eq!(builtin_for("android", "emulator-5554"), Some(Builtin::Android));
        assert_eq!(builtin_for("android", ""), None, "no target, nothing to find");
        assert_eq!(builtin_for("device", "R58M"), None, "a real phone has no window to raise");
        assert_eq!(builtin_for("", "emulator-5554"), None);
    }

    #[test]
    fn it_finds_the_simulator_s_name_and_the_emulator_s_port() {
        let json = r#"{"devices": {"com.apple.CoreSimulator.SimRuntime.iOS-17-5": [
            {"udid": "AAAA", "name": "iPhone 15", "state": "Booted"},
            {"udid": "BBBB", "name": "iPad \"Air\"", "state": "Shutdown"}]}}"#;
        assert_eq!(sim_name(json, "aaaa").as_deref(), Some("iPhone 15"));
        assert_eq!(sim_name(json, "BBBB").as_deref(), Some("iPad \"Air\""));
        assert_eq!(sim_name(json, "CCCC"), None);
        assert_eq!(sim_name("not json", "AAAA"), None);
        assert_eq!(emulator_port("emulator-5554"), Some(5554));
        assert_eq!(emulator_port("R58M123"), None);
    }

    #[test]
    fn the_raise_script_matches_the_window_and_quotes_the_name() {
        let s = raise_script("name is \"Simulator\"", &format!("starts with \"{}\"", quoted("iPad \"Air\" – ")));
        assert!(s.contains("every process whose name is \"Simulator\""), "{s}");
        assert!(s.contains("if name of w starts with \"iPad \\\"Air\\\" – \" then"), "{s}");
        assert!(s.contains("perform action \"AXRaise\" of w"));
        assert!(s.ends_with("return \"none\""));
    }

    #[test]
    fn a_missing_permission_says_where_to_grant_it() {
        let e = osascript_error("sim-a", "execution error: System Events got an error: osascript is not allowed assistive access. (-1719)");
        assert_eq!(e, "Taskboard needs the Accessibility permission to raise sim-a's window. Grant it in System Settings › Privacy & Security › Accessibility.");
        assert_eq!(osascript_error("sim-a", ""), "The board can't find a window for sim-a.");
        assert_eq!(osascript_error("sim-a", "boom"), "The board can't find a window for sim-a: boom");
    }
}
