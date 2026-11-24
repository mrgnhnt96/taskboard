//! The Mac's load and memory, checked before a task's terminal opens. While the 1-minute load average
//! per core is at or above `[terminals] hold_load_per_core`, or macOS's memory pressure reaches
//! `[terminals] hold_memory`, new task starts wait, however many terminals the caps leave free.
//!
//! The reading comes from Midna's `system.load` while Midna is up (the same signal its own busy gate
//! uses), else from the kernel: `getloadavg` and `kern.memorystatus_vm_pressure_level`. The runner
//! refreshes it once per tick (`refresh`), outside any transaction; the start checks only read it.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::app::App;
use crate::config::TerminalsConfig;

/// How long a reading stands before the runner reads again.
const FRESH: Duration = Duration::from_secs(15);
/// How old a reading may get before a start check reads the kernel itself.
const STALE: Duration = Duration::from_secs(60);
const MIDNA_TIMEOUT: f64 = 2.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Reading {
    pub load1: f64,
    pub cpus: u32,
    /// macOS's memory pressure: `normal`, `warning` or `critical`.
    pub memory: String,
    /// `midna` or `mac`.
    pub from: &'static str,
}

impl Reading {
    pub fn json(&self) -> Value {
        json!({"load1": self.load1, "cpus": self.cpus, "memory": self.memory, "from": self.from})
    }
}

fn rank(level: &str) -> u8 {
    match level {
        "critical" => 2,
        "warning" => 1,
        _ => 0,
    }
}

/// Whether `memory` reaches `hold_at` (`warning` or `critical`; anything else never holds).
fn memory_reaches(memory: &str, hold_at: &str) -> bool {
    rank(hold_at) > 0 && rank(memory) >= rank(hold_at)
}

fn num(x: f64) -> String {
    if x.fract() == 0.0 { format!("{x:.0}") } else { format!("{x:.1}") }
}

/// Why `r` holds new starts back under `cfg`, if it does.
pub fn why(cfg: &TerminalsConfig, r: &Reading) -> Option<String> {
    if memory_reaches(&r.memory, &cfg.hold_memory) {
        return Some(format!("Waits for free memory: macOS memory pressure is {}", r.memory));
    }
    if cfg.hold_load_per_core > 0.0 && r.cpus > 0 {
        let at = cfg.hold_load_per_core * r.cpus as f64;
        if r.load1 >= at {
            return Some(format!("Waits for the Mac to calm down: load {:.1} on {} cores (starts wait from {})", r.load1, r.cpus, num(at)));
        }
    }
    None
}

fn off(cfg: &TerminalsConfig) -> bool {
    cfg.hold_load_per_core <= 0.0 && rank(&cfg.hold_memory) == 0
}

/// Why new task starts wait on the Mac's load or memory right now, if they do.
pub fn hold(app: &App) -> Option<String> {
    if off(&app.cfg.terminals) {
        return None;
    }
    why(&app.cfg.terminals, &reading(app)?)
}

/// The last reading, or the kernel's when that's older than a minute.
pub fn reading(app: &App) -> Option<Reading> {
    {
        let s = app.shared.lock();
        if let Some(r) = &s.pressure_fixed {
            return Some(r.clone());
        }
        if let Some((at, r)) = &s.pressure {
            if at.elapsed() < STALE {
                return Some(r.clone());
            }
        }
    }
    let r = local()?;
    app.shared.lock().pressure = Some((Instant::now(), r.clone()));
    Some(r)
}

/// Reads the Mac again when the last reading is older than `FRESH` (Midna's while it's up), and logs
/// when starts begin or stop waiting on it. The runner calls it each tick, outside any transaction.
pub fn refresh(app: &App) {
    if off(&app.cfg.terminals) {
        return;
    }
    let due = {
        let s = app.shared.lock();
        s.pressure_fixed.is_none() && s.pressure.as_ref().is_none_or(|(at, _)| at.elapsed() >= FRESH)
    };
    if due {
        let r = if crate::midna::up(app) { from_midna(app).or_else(local) } else { local() };
        if let Some(r) = r {
            app.shared.lock().pressure = Some((Instant::now(), r));
        }
    }
    let Some(r) = reading(app) else { return };
    let now = why(&app.cfg.terminals, &r);
    let kind = now.as_deref().map(|w| w.split(':').next().unwrap_or(w).to_string());
    let changed = {
        let mut s = app.shared.lock();
        let changed = s.pressure_held != kind;
        s.pressure_held = kind;
        changed
    };
    if changed {
        match now {
            Some(w) => app.info(format!("task starts held: {w}")),
            None => app.info(format!("task starts go on: load {:.1} on {} cores, memory pressure {}", r.load1, r.cpus, r.memory)),
        }
    }
}

/// Puts a reading in place of the Mac's, for tests (None: the Mac's again).
pub fn fix_for_tests(app: &App, r: Option<Reading>) {
    app.shared.lock().pressure_fixed = r;
}

fn from_midna(app: &App) -> Option<Reading> {
    let v = crate::midna::call_timeout(app, "system.load", json!({}), MIDNA_TIMEOUT).ok()?;
    parse_midna(&v)
}

/// Midna's `system.load` as a reading.
pub fn parse_midna(v: &Value) -> Option<Reading> {
    let load1 = v["load1"].as_f64()?;
    let cpus = v["cpus"].as_u64().filter(|&c| c > 0)? as u32;
    let memory = v["memory_pressure"].as_str().unwrap_or("normal").to_string();
    Some(Reading { load1, cpus, memory, from: "midna" })
}

fn local() -> Option<Reading> {
    let mut avg = [0f64; 3];
    // SAFETY: getloadavg writes at most `nelem` doubles into the buffer it's given.
    if unsafe { libc::getloadavg(avg.as_mut_ptr(), 3) } < 1 {
        return None;
    }
    let cpus = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
    Some(Reading { load1: avg[0], cpus, memory: memory_level().into(), from: "mac" })
}

#[cfg(target_os = "macos")]
fn memory_level() -> &'static str {
    let mut level: libc::c_int = 0;
    let mut len = std::mem::size_of::<libc::c_int>();
    let name = c"kern.memorystatus_vm_pressure_level";
    // SAFETY: the name is NUL-terminated and `level`/`len` describe a c_int-sized buffer.
    let rc = unsafe { libc::sysctlbyname(name.as_ptr(), (&mut level as *mut libc::c_int).cast(), &mut len, std::ptr::null_mut(), 0) };
    match (rc, level) {
        (0, l) if l >= 4 => "critical",
        (0, 2 | 3) => "warning",
        _ => "normal",
    }
}

#[cfg(not(target_os = "macos"))]
fn memory_level() -> &'static str {
    "normal"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(load: f64, memory: &str) -> TerminalsConfig {
        TerminalsConfig { hold_load_per_core: load, hold_memory: memory.into(), ..Default::default() }
    }

    fn r(load1: f64, cpus: u32, memory: &str) -> Reading {
        Reading { load1, cpus, memory: memory.into(), from: "mac" }
    }

    #[test]
    fn load_at_the_threshold_per_core_holds() {
        let c = cfg(1.5, "warning");
        assert_eq!(why(&c, &r(17.9, 12, "normal")), None);
        assert_eq!(why(&c, &r(18.0, 12, "normal")).as_deref(), Some("Waits for the Mac to calm down: load 18.0 on 12 cores (starts wait from 18)"));
        assert_eq!(why(&cfg(1.25, "off"), &r(30.0, 10, "normal")).as_deref(), Some("Waits for the Mac to calm down: load 30.0 on 10 cores (starts wait from 12.5)"));
    }

    #[test]
    fn memory_pressure_holds_from_its_level() {
        assert_eq!(why(&cfg(1.5, "warning"), &r(1.0, 12, "warning")).as_deref(), Some("Waits for free memory: macOS memory pressure is warning"));
        assert_eq!(why(&cfg(1.5, "critical"), &r(1.0, 12, "warning")), None);
        assert!(why(&cfg(1.5, "critical"), &r(1.0, 12, "critical")).is_some());
        assert_eq!(why(&cfg(1.5, "off"), &r(1.0, 12, "critical")), None);
    }

    #[test]
    fn zero_turns_the_load_check_off() {
        assert_eq!(why(&cfg(0.0, "off"), &r(280.0, 12, "normal")), None);
        assert!(off(&cfg(0.0, "off")));
        assert!(!off(&cfg(0.0, "warning")));
    }

    #[test]
    fn midna_s_system_load_reads_as_a_reading() {
        let v = json!({"load1": 41.5, "cpus": 12, "memory_pressure": "critical", "memory_held": true});
        assert_eq!(parse_midna(&v), Some(Reading { load1: 41.5, cpus: 12, memory: "critical".into(), from: "midna" }));
        assert_eq!(parse_midna(&json!({"load1": 2.0, "cpus": 8})).unwrap().memory, "normal");
        assert_eq!(parse_midna(&json!({"ok": false})), None);
    }

    #[test]
    fn the_kernel_gives_a_reading() {
        let r = local().expect("getloadavg");
        assert!(r.cpus >= 1 && r.load1 >= 0.0, "{r:?}");
        assert!(["normal", "warning", "critical"].contains(&r.memory.as_str()));
    }
}
