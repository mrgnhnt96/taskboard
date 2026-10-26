//! A refresh's answer boiled down to one number, so a poll that brings nothing new can skip the
//! redraw (`MainWindow::landed`). Worked out off the UI thread, beside the fetch.
use serde_json::Value;
use std::hash::{DefaultHasher, Hash, Hasher};

/// Keys that move on their own every poll while work runs: the board's clock, and when a terminal
/// was last heard from. They show only as ages, which the window redraws on its own
/// (`app::AGES`).
pub const CLOCKS: &[&str] = &["now", "seen_at", "last_activity", "idle_secs"];

/// Likewise a `GET /days` answer's running totals and open bars, which grow by the minute.
pub const DAY_CLOCKS: &[&str] = &["now", "agent_min", "by_project", "to"];

pub struct Fingerprint {
    h: DefaultHasher,
    /// The keys left out of the answer being added.
    pub skip: &'static [&'static str],
}

impl Fingerprint {
    pub fn new(seed: impl Hash) -> Fingerprint {
        let mut h = DefaultHasher::new();
        seed.hash(&mut h);
        Fingerprint { h, skip: CLOCKS }
    }

    pub fn add(&mut self, x: &impl Print) -> &mut Self {
        x.print(self);
        self
    }

    pub fn finish(&self) -> u64 {
        self.h.finish()
    }
}

/// What goes into a [`Fingerprint`].
pub trait Print {
    fn print(&self, f: &mut Fingerprint);
}

impl Print for Value {
    fn print(&self, f: &mut Fingerprint) {
        match self {
            Value::Null => f.h.write_u8(0),
            Value::Bool(b) => (1u8, b).hash(&mut f.h),
            Value::Number(n) => (2u8, n).hash(&mut f.h),
            Value::String(s) => (3u8, s).hash(&mut f.h),
            Value::Array(a) => {
                (4u8, a.len()).hash(&mut f.h);
                a.iter().for_each(|x| x.print(f));
            }
            Value::Object(o) => {
                // Key order isn't the board's to keep: some objects are built from hash maps.
                let mut kept: Vec<_> = o.iter().filter(|(k, _)| !f.skip.contains(&k.as_str())).collect();
                kept.sort_unstable_by_key(|(k, _)| *k);
                f.h.write_u8(5);
                for (k, x) in kept {
                    k.hash(&mut f.h);
                    x.print(f);
                }
                f.h.write_u8(6);
            }
        }
    }
}

impl Print for String {
    fn print(&self, f: &mut Fingerprint) {
        self.hash(&mut f.h);
    }
}

impl Print for i64 {
    fn print(&self, f: &mut Fingerprint) {
        self.hash(&mut f.h);
    }
}

impl Print for crate::backend::CallError {
    fn print(&self, f: &mut Fingerprint) {
        (self.status, &self.message).hash(&mut f.h);
    }
}

impl<T: Print> Print for Option<T> {
    fn print(&self, f: &mut Fingerprint) {
        f.h.write_u8(self.is_some() as u8);
        if let Some(x) = self {
            x.print(f);
        }
    }
}

impl<T: Print, E: Print> Print for Result<T, E> {
    fn print(&self, f: &mut Fingerprint) {
        match self {
            Ok(x) => {
                f.h.write_u8(1);
                x.print(f);
            }
            Err(e) => {
                f.h.write_u8(0);
                e.print(f);
            }
        }
    }
}

impl<T: Print> Print for Vec<T> {
    fn print(&self, f: &mut Fingerprint) {
        self.len().hash(&mut f.h);
        self.iter().for_each(|x| x.print(f));
    }
}

impl<A: Print, B: Print> Print for (A, B) {
    fn print(&self, f: &mut Fingerprint) {
        self.0.print(f);
        self.1.print(f);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn print(v: &Value) -> u64 {
        Fingerprint::new(()).add(v).finish()
    }

    #[::core::prelude::v1::test]
    fn the_clocks_dont_count() {
        let a = json!({"now": "2026-10-26T10:00:00Z", "sessions": [{"id": "s1", "status": "working", "seen_at": "2026-10-26T09:59:58Z", "idle_secs": 2}]});
        let b = json!({"now": "2026-10-26T10:00:03Z", "sessions": [{"id": "s1", "status": "working", "seen_at": "2026-10-26T10:00:01Z", "idle_secs": 0}]});
        assert_eq!(print(&a), print(&b));
        let mut c = b.clone();
        c["sessions"][0]["status"] = json!("idle");
        assert_ne!(print(&b), print(&c), "what else the answer says does");
    }

    #[::core::prelude::v1::test]
    fn a_days_answer_leaves_out_its_running_totals() {
        let day = |min: f64, to: &str| json!({"day": {"agent_min": min, "by_project": {"web": min}, "done": 2, "lanes": [{"bars": [{"ref": "T1", "to": to, "open": true}]}]}});
        let print = |v: &Value| {
            let mut f = Fingerprint::new(());
            f.skip = DAY_CLOCKS;
            f.add(v).finish()
        };
        assert_eq!(print(&day(10.0, "10:00")), print(&day(10.05, "10:03")));
        let mut done = day(10.0, "10:00");
        done["day"]["done"] = json!(3);
        assert_ne!(print(&day(10.0, "10:00")), print(&done));
    }

    #[::core::prelude::v1::test]
    fn the_shape_counts() {
        assert_ne!(print(&json!([["a"], []])), print(&json!([[], ["a"]])));
        assert_ne!(print(&json!({"a": null})), print(&json!({"a": false})));
        let mut ab = serde_json::Map::new();
        ab.insert("a".into(), json!(1));
        ab.insert("b".into(), json!(2));
        let mut ba = serde_json::Map::new();
        ba.insert("b".into(), json!(2));
        ba.insert("a".into(), json!(1));
        assert_eq!(print(&Value::Object(ab)), print(&Value::Object(ba)), "key order doesn't");
        assert_ne!(Fingerprint::new(1).finish(), Fingerprint::new(2).finish(), "so does the seed (what was asked)");
    }
}
