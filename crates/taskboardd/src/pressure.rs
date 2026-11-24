//! The Mac's load and memory, checked before a task's terminal opens. While the 1-minute load average
//! per core is at or above `[terminals] hold_load_per_core`, or macOS's memory pressure reaches
//! `[terminals] hold_memory`, or less memory is free than `[terminals] hold_free_memory_gb`, new task
//! starts wait, however many terminals the caps leave free. A midnad using a whole core or more shows
//! in why they wait.
//!
//! The reading comes from Midna's `system.load` while Midna is up (the same signal its own busy gate
//! uses), else from the kernel: `getloadavg` and `kern.memorystatus_vm_pressure_level`. Free memory and
//! midnad's own use come only from Midna; 0 there (an older Midna, or the kernel's reading) is unknown
//! and never holds. The runner refreshes it once per tick (`refresh`), outside any transaction; the
//! start checks only read it.
//!
//! Once a hold clears, held starts open one per fresh reading (`pace_hold`), like Midna's own
//! admission: the load average lags a minute, so letting them all open at once would push the load
//! straight back up. The pacing ends at the first reading after one on which no start opened.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::app::App;
use crate::config::TerminalsConfig;

/// How long a reading stands before the runner reads again.
const FRESH: Duration = Duration::from_secs(15);
/// How old a reading may get before a start check reads the kernel itself.
const STALE: Duration = Duration::from_secs(60);
const MIDNA_TIMEOUT: f64 = 2.0;
/// midnad using this much CPU (percent of one core) or more shows in why starts wait.
const HOT_MIDNAD_CPU: u32 = 100;
const GB: f64 = (1u64 << 30) as f64;
/// Why a start waits while held starts open one per reading.
pub const PACED: &str = "Waits its turn: the Mac just calmed down, so held starts open one per load reading";

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    pub load1: f64,
    pub cpus: u32,
    /// macOS's memory pressure: `normal`, `warning` or `critical`.
    pub memory: String,
    /// Memory apps can still get, bytes (Midna's `memory_available_bytes`; 0: unknown).
    pub memory_free: u64,
    /// midnad's own CPU, percent of one core, and memory footprint, bytes (0: unknown).
    pub midnad_cpu: u32,
    pub midnad_memory: u64,
    /// `midna` or `mac`.
    pub from: &'static str,
}

impl Reading {
    pub fn json(&self) -> Value {
        json!({"load1": self.load1, "cpus": self.cpus, "memory": self.memory, "memory_free_bytes": self.memory_free,
               "midnad": {"cpu_percent": self.midnad_cpu, "memory_bytes": self.midnad_memory}, "from": self.from})
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

fn size(bytes: u64) -> String {
    let gb = bytes as f64 / GB;
    if gb < 1.0 { format!("{} MB", (bytes >> 20).max(1)) } else { format!("{} GB", num((gb * 10.0).round() / 10.0)) }
}

/// Why `r` holds new starts back under `cfg`, if it does, with a hot midnad named after it.
pub fn why(cfg: &TerminalsConfig, r: &Reading) -> Option<String> {
    let why = held_by(cfg, r)?;
    Some(match hot_midnad(r) {
        Some(hot) => format!("{why}; {hot}"),
        None => why,
    })
}

fn held_by(cfg: &TerminalsConfig, r: &Reading) -> Option<String> {
    if memory_reaches(&r.memory, &cfg.hold_memory) {
        return Some(format!("Waits for free memory: macOS memory pressure is {}", r.memory));
    }
    let floor = (cfg.hold_free_memory_gb.max(0.0) * GB) as u64;
    if floor > 0 && r.memory_free > 0 && r.memory_free < floor {
        return Some(format!("Waits for free memory: {} free (starts wait under {})", size(r.memory_free), size(floor)));
    }
    if cfg.hold_load_per_core > 0.0 && r.cpus > 0 {
        let at = cfg.hold_load_per_core * r.cpus as f64;
        if r.load1 >= at {
            return Some(format!("Waits for the Mac to calm down: load {:.1} on {} cores (starts wait from {})", r.load1, r.cpus, num(at)));
        }
    }
    None
}

/// midnad's own use when it runs hot (a whole core or more), else None.
fn hot_midnad(r: &Reading) -> Option<String> {
    if r.midnad_cpu < HOT_MIDNAD_CPU {
        return None;
    }
    let memory = if r.midnad_memory > 0 { format!(", {}", size(r.midnad_memory)) } else { String::new() };
    Some(format!("midnad itself runs hot ({}% CPU{memory})", r.midnad_cpu))
}

fn off(cfg: &TerminalsConfig) -> bool {
    cfg.hold_load_per_core <= 0.0 && rank(&cfg.hold_memory) == 0 && cfg.hold_free_memory_gb <= 0.0
}

/// Why a task's start waits on the pace held starts open at after a hold cleared: one already opened
/// on this reading, or `waiting` other starts are already in line for it.
pub fn pace_hold(app: &App, waiting: usize) -> Option<String> {
    let opened = app.shared.lock().pressure_pace?;
    (opened || waiting > 0).then(|| PACED.to_string())
}

/// A task's start opened its terminal: while pacing, the next waits for a fresh reading.
pub fn opened(app: &App) {
    if let Some(p) = app.shared.lock().pressure_pace.as_mut() {
        *p = true;
    }
}

/// Keeps `r` as the latest reading.
fn store(s: &mut crate::app::Shared, r: Reading) {
    s.pressure = Some((Instant::now(), r));
    fresh(s);
}

/// A fresh reading lets one more held start open while pacing, or ends the pacing when none opened
/// on the last one.
fn fresh(s: &mut crate::app::Shared) {
    s.pressure_pace = s.pressure_pace.filter(|&opened| opened).map(|_| false);
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
    store(&mut app.shared.lock(), r.clone());
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
            store(&mut app.shared.lock(), r);
        }
    }
    let Some(r) = reading(app) else { return };
    let now = why(&app.cfg.terminals, &r);
    let kind = now.as_deref().map(|w| w.split(':').next().unwrap_or(w).to_string());
    let changed = {
        let mut s = app.shared.lock();
        let changed = s.pressure_held != kind;
        if kind.is_some() {
            s.pressure_pace = None;
        } else if changed {
            s.pressure_pace = Some(false);
        }
        s.pressure_held = kind;
        changed
    };
    if changed {
        match now {
            Some(w) => app.info(format!("task starts held: {w}")),
            None => app.info(format!(
                "task starts go on, one per reading: load {:.1} on {} cores, memory pressure {}",
                r.load1, r.cpus, r.memory
            )),
        }
    }
}

/// Puts a reading in place of the Mac's, for tests (None: the Mac's again).
/// Each call counts as a fresh reading.
pub fn fix_for_tests(app: &App, r: Option<Reading>) {
    let mut s = app.shared.lock();
    s.pressure_fixed = r;
    fresh(&mut s);
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
    let memory_free = v["memory_available_bytes"].as_u64().unwrap_or(0);
    let midnad_cpu = v["daemon"]["cpu_percent"].as_u64().unwrap_or(0).min(u32::MAX as u64) as u32;
    let midnad_memory = v["daemon"]["memory_bytes"].as_u64().unwrap_or(0);
    Some(Reading { load1, cpus, memory, memory_free, midnad_cpu, midnad_memory, from: "midna" })
}

fn local() -> Option<Reading> {
    let mut avg = [0f64; 3];
    // SAFETY: getloadavg writes at most `nelem` doubles into the buffer it's given.
    if unsafe { libc::getloadavg(avg.as_mut_ptr(), 3) } < 1 {
        return None;
    }
    let cpus = std::thread::available_parallelism().map(|n| n.get() as u32).unwrap_or(1);
    Some(Reading { load1: avg[0], cpus, memory: memory_level().into(), from: "mac", ..Default::default() })
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
        TerminalsConfig { hold_load_per_core: load, hold_memory: memory.into(), hold_free_memory_gb: 0.0, ..Default::default() }
    }

    fn r(load1: f64, cpus: u32, memory: &str) -> Reading {
        Reading { load1, cpus, memory: memory.into(), from: "mac", ..Default::default() }
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
    fn little_free_memory_holds_and_unknown_never_does() {
        let c = TerminalsConfig { hold_free_memory_gb: 2.0, ..cfg(1.5, "warning") };
        let low = Reading { memory_free: 3 << 29, ..r(1.0, 12, "normal") };
        assert_eq!(why(&c, &low).as_deref(), Some("Waits for free memory: 1.5 GB free (starts wait under 2 GB)"));
        let tiny = Reading { memory_free: 60 << 20, ..low.clone() };
        assert_eq!(why(&c, &tiny).as_deref(), Some("Waits for free memory: 60 MB free (starts wait under 2 GB)"));
        assert_eq!(why(&c, &Reading { memory_free: 24 << 30, ..low.clone() }), None);
        // An older Midna, or the kernel's reading, says 0: unknown, so it never holds.
        assert_eq!(why(&c, &Reading { memory_free: 0, ..low.clone() }), None);
        assert_eq!(why(&TerminalsConfig { hold_free_memory_gb: 0.0, ..c.clone() }, &low), None);
        assert!(!off(&TerminalsConfig { hold_free_memory_gb: 2.0, ..cfg(0.0, "off") }));
    }

    #[test]
    fn a_hot_midnad_shows_in_why_starts_wait() {
        let c = cfg(1.5, "warning");
        let hot = Reading { midnad_cpu: 140, midnad_memory: 1 << 30, ..r(30.0, 12, "normal") };
        assert_eq!(
            why(&c, &hot).as_deref(),
            Some("Waits for the Mac to calm down: load 30.0 on 12 cores (starts wait from 18); midnad itself runs hot (140% CPU, 1 GB)")
        );
        assert_eq!(why(&c, &Reading { midnad_cpu: 31, ..hot.clone() }).as_deref(), Some("Waits for the Mac to calm down: load 30.0 on 12 cores (starts wait from 18)"));
        // A hot midnad alone holds nothing; 0 (an older Midna) is unknown.
        assert_eq!(why(&c, &Reading { load1: 2.0, ..hot.clone() }), None);
        assert_eq!(hot_midnad(&Reading { midnad_memory: 0, ..hot }).as_deref(), Some("midnad itself runs hot (140% CPU)"));
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
        assert_eq!(parse_midna(&v), Some(Reading { load1: 41.5, cpus: 12, memory: "critical".into(), from: "midna", ..Default::default() }));
        let v = json!({"load1": 3.0, "cpus": 12, "memory_available_bytes": 3u64 << 30,
                       "daemon": {"cpu_percent": 140, "memory_bytes": 1u64 << 30, "uptime_secs": 4695}});
        let got = parse_midna(&v).unwrap();
        assert_eq!((got.memory_free, got.midnad_cpu, got.midnad_memory), (3 << 30, 140, 1 << 30));
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
