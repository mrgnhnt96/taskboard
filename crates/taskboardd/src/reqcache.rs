//! What one read request loads once and shares across every task it shows: the settings rows and the
//! steps file each task card reads (#866). A goal page or `/state` builds a card per task, and each card
//! asked for the same PR rules, alerts and steps again. Inside [`scope`] the first read is kept for the
//! rest of the request; outside one (writes, the timers, tests) every read goes to the source as before.
//! The writes of a kept value call [`forget`], so a request that changes one reads it fresh after.

use std::any::Any;
use std::cell::RefCell;
use std::collections::HashMap;

use crate::app::App;
use crate::util::Result;

thread_local! {
    static KEPT: RefCell<Option<HashMap<String, Box<dyn Any>>>> = const { RefCell::new(None) };
}

/// Runs `f` with one request's reads kept. A scope inside a scope shares the outer one.
pub fn scope<T>(f: impl FnOnce() -> T) -> T {
    if KEPT.with(|k| k.borrow().is_some()) {
        return f();
    }
    // The blocking pool reuses its threads: the next request on this one starts empty, even after a panic.
    struct End;
    impl Drop for End {
        fn drop(&mut self) {
            KEPT.with(|k| k.borrow_mut().take());
        }
    }
    KEPT.with(|k| *k.borrow_mut() = Some(HashMap::new()));
    let _end = End;
    f()
}

/// The value kept under `key` in this request, loading it the first time.
pub fn get<T: Clone + 'static>(key: &str, load: impl FnOnce() -> T) -> T {
    if let Some(v) = KEPT.with(|k| k.borrow().as_ref()?.get(key)?.downcast_ref::<T>().cloned()) {
        return v;
    }
    let v = load();
    KEPT.with(|k| {
        if let Some(m) = k.borrow_mut().as_mut() {
            m.insert(key.to_string(), Box::new(v.clone()));
        }
    });
    v
}

/// [`get`] for a load that can fail: only a value it got is kept, so a failed read is tried again.
pub fn try_get<T: Clone + 'static>(key: &str, load: impl FnOnce() -> Result<T>) -> Result<T> {
    if let Some(v) = KEPT.with(|k| k.borrow().as_ref()?.get(key)?.downcast_ref::<T>().cloned()) {
        return Ok(v);
    }
    let v = load()?;
    KEPT.with(|k| {
        if let Some(m) = k.borrow_mut().as_mut() {
            m.insert(key.to_string(), Box::new(v.clone()));
        }
    });
    Ok(v)
}

/// A settings row, read once per request.
pub fn setting(app: &App, key: &str) -> Result<Option<String>> {
    try_get(&format!("setting:{key}"), || app.db.get_setting(key))
}

/// Drops what this request kept under `key` (a setting is `setting:<key>`), after a write changed it.
pub fn forget(key: &str) {
    KEPT.with(|k| {
        if let Some(m) = k.borrow_mut().as_mut() {
            m.remove(key);
        }
    });
}

/// [`forget`] for a settings row.
pub fn forget_setting(key: &str) {
    forget(&format!("setting:{key}"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn a_scope_loads_once_and_outside_one_loads_every_time() {
        let loads = Cell::new(0);
        let load = || {
            loads.set(loads.get() + 1);
            7
        };
        get("k", load);
        get("k", load);
        assert_eq!(loads.get(), 2);
        loads.set(0);
        scope(|| {
            assert_eq!(get("k", load), 7);
            assert_eq!(get("k", load), 7);
            scope(|| get("k", load));
        });
        assert_eq!(loads.get(), 1);
        scope(|| get("k", load));
        assert_eq!(loads.get(), 2, "the next request starts empty");
    }

    #[test]
    fn forget_and_a_failed_load_read_again() {
        let loads = Cell::new(0);
        scope(|| {
            let ok = || -> Result<i32> {
                loads.set(loads.get() + 1);
                Ok(1)
            };
            try_get("k", ok).unwrap();
            forget("k");
            try_get("k", ok).unwrap();
            assert_eq!(loads.get(), 2);
            assert!(try_get::<i32>("bad", || Err(crate::util::ApiError::new(500, "no"))).is_err());
            assert_eq!(try_get("bad", ok).unwrap(), 1);
        });
    }
}
