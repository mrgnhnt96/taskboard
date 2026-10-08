//! The reader's own UI state that outlives a launch: the web board kept it in `localStorage`
//! under the same keys (`taskboard.filter`, `tb.rail.w`, `tb.rail.shut`, `tb.rail.recent`,
//! `tb.linked`, `tb.fold.<k>`, `tb.draft.<k>`, `tb.sessCollapsed`), and so do we.
//!
//! Stored as one JSON object in `app-state.json`: under `$TASKBOARD_DATA` when set (Taskboard Dev
//! sets it), else `~/Library/Application Support/com.mrgnhnt.taskboard/`. Values are JSON.
//! The fake backend and tests keep it in memory only, so they never touch the real file.
use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Mutex;

struct Store {
    path: Option<PathBuf>,
    map: Map<String, Value>,
}

static STORE: Mutex<Option<Store>> = Mutex::new(None);

// Tests run in parallel, each on its own thread: give each its own in-memory store.
#[cfg(test)]
thread_local! {
    static TEST_STORE: std::cell::RefCell<Store> = std::cell::RefCell::new(Store { path: None, map: Map::new() });
}

fn default_path() -> PathBuf {
    match std::env::var("TASKBOARD_DATA") {
        Ok(d) if !d.trim().is_empty() => taskboardd::util::expand_home(&d).join("app-state.json"),
        _ => taskboardd::util::expand_home("~/Library/Application Support/com.mrgnhnt.taskboard/app-state.json"),
    }
}

/// Load the file (call once at startup). `persist: false` keeps everything in memory.
pub fn init(persist: bool) {
    let path = persist.then(default_path);
    let map = path
        .as_ref()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|t| serde_json::from_str::<Value>(&t).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default();
    *STORE.lock().unwrap() = Some(Store { path, map });
}

#[cfg(test)]
fn with<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    TEST_STORE.with(|s| f(&mut s.borrow_mut()))
}

#[cfg(not(test))]
fn with<R>(f: impl FnOnce(&mut Store) -> R) -> R {
    let mut g = STORE.lock().unwrap_or_else(|e| e.into_inner());
    let s = g.get_or_insert_with(|| Store { path: None, map: Map::new() });
    f(s)
}

fn save(s: &Store) {
    let Some(p) = &s.path else { return };
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec_pretty(&Value::Object(s.map.clone())).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, p);
    }
}

pub fn get(key: &str) -> Option<Value> {
    with(|s| s.map.get(key).cloned())
}

pub fn get_str(key: &str) -> Option<String> {
    get(key).and_then(|v| v.as_str().map(str::to_string))
}

pub fn set(key: &str, value: Value) {
    with(|s| {
        if s.map.get(key) != Some(&value) {
            s.map.insert(key.to_string(), value);
            save(s);
        }
    })
}

pub fn remove(key: &str) {
    with(|s| {
        if s.map.remove(key).is_some() {
            save(s);
        }
    })
}

/// Forget everything (tests).
#[cfg(test)]
pub fn reset() {
    with(|s| s.map.clear());
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[::core::prelude::v1::test]
    fn round_trip_in_memory() {
        set("tb.rail.w", json!(320));
        assert_eq!(get("tb.rail.w"), Some(json!(320)));
        remove("tb.rail.w");
        assert_eq!(get("tb.rail.w"), None);
    }
}
