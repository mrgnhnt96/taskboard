//! A poll's answer (`GET /state`, `GET /goals`) boiled down to a stamp, clocks aside, so a poller
//! that already holds it gets `{"unchanged": true}` back instead of the whole board again.
use serde_json::{json, Value};
use std::hash::{DefaultHasher, Hash, Hasher};

use crate::util::now_iso;

/// Keys that move on their own while work runs: the board's clock, when a terminal was last heard
/// from, and a goal's newest activity. A poller shows them as ages and asks in full now and then
/// to catch up on them.
pub const CLOCKS: &[&str] = &["now", "seen_at", "last_activity", "idle_secs", "last_update"];

/// `v` minus its [`CLOCKS`], as a short hex string.
pub fn of(v: &Value) -> String {
    let mut h = DefaultHasher::new();
    print(v, &mut h);
    format!("{:016x}", h.finish())
}

/// `v` stamped, or just `{"unchanged": true, "stamp", "now"}` when that stamp is `since`.
pub fn answer(mut v: Value, since: Option<&str>) -> Value {
    let stamp = of(&v);
    if since == Some(stamp.as_str()) {
        let now = v.get("now").cloned().unwrap_or_else(|| json!(now_iso()));
        return json!({"unchanged": true, "stamp": stamp, "now": now});
    }
    if let Some(o) = v.as_object_mut() {
        o.insert("stamp".into(), json!(stamp));
    }
    v
}

fn print(v: &Value, h: &mut DefaultHasher) {
    match v {
        Value::Null => h.write_u8(0),
        Value::Bool(b) => (1u8, b).hash(h),
        Value::Number(n) => (2u8, n).hash(h),
        Value::String(s) => (3u8, s).hash(h),
        Value::Array(a) => {
            (4u8, a.len()).hash(h);
            a.iter().for_each(|x| print(x, h));
        }
        Value::Object(o) => {
            // Some objects are built from hash maps, so their key order can change between answers.
            let mut kept: Vec<_> = o.iter().filter(|(k, _)| !CLOCKS.contains(&k.as_str())).collect();
            kept.sort_unstable_by_key(|(k, _)| *k);
            h.write_u8(5);
            for (k, x) in kept {
                k.hash(h);
                print(x, h);
            }
            h.write_u8(6);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clocks_and_key_order_leave_the_stamp_alone() {
        let a = json!({"now": "1", "sessions": [{"id": "s1", "seen_at": "1", "idle_secs": 4}], "goals": [{"id": 1, "last_update": "a"}]});
        let b = json!({"goals": [{"last_update": "b", "id": 1}], "sessions": [{"idle_secs": 9, "seen_at": "2", "id": "s1"}], "now": "2"});
        assert_eq!(of(&a), of(&b));
        assert_ne!(of(&a), of(&json!({"now": "1", "sessions": [{"id": "s2"}], "goals": [{"id": 1}]})));
    }

    #[test]
    fn answers_in_full_with_its_stamp_until_asked_since_it() {
        let v = json!({"now": "2026-10-26T10:00:00Z", "columns": {"queued": []}});
        let full = answer(v.clone(), None);
        let stamp = full["stamp"].as_str().unwrap().to_string();
        assert_eq!(full["columns"], v["columns"]);
        assert_eq!(answer(v.clone(), Some("other"))["columns"], v["columns"]);
        assert_eq!(answer(v, Some(&stamp)), json!({"unchanged": true, "stamp": stamp, "now": "2026-10-26T10:00:00Z"}));
    }
}
