//! A long list the board sends a page at a time (`?limit=&before=`, answered with the page, the
//! `total` that match and the `next` page's cursor): the rows held so far, by id. A refresh asks
//! for the first page again plus the held rows that may have changed, and the list nears its end
//! before the next older page is asked for. The Tasks page keeps one; the other long lists can too.
use crate::backend::Backend;
use crate::fmt::arr;
use serde_json::Value;
use std::collections::HashSet;
use std::rc::Rc;

/// What a refresh fetched: the first page, and the held rows outside it asked for again by id.
pub struct Fresh {
    first: Value,
    asked: Vec<i64>,
    again: Option<Value>,
}

impl Fresh {
    /// The first page as the board sent it, for what else it says about the list.
    pub fn first(&self) -> &Value {
        &self.first
    }
}

#[derive(Default)]
pub struct Paged {
    /// In the board's order as the pages came, each once.
    pub rows: Rc<Vec<Value>>,
    /// The first page's ids at the last refresh.
    first: Vec<i64>,
    /// An older page landed since the list started, so the first page's cursor no longer applies.
    older: bool,
    /// Where the next older page starts; None once the board has nothing older.
    pub next: Option<String>,
    pub total: usize,
    /// The first page has answered.
    pub loaded: bool,
    pub loading: bool,
    /// Bumped whenever `rows` change, so a view built from them knows to rebuild.
    pub version: u64,
}

fn id(v: &Value) -> i64 {
    v["id"].as_i64().unwrap_or_default()
}

impl Paged {
    /// The held rows a refresh asks for again: the last first page (a row can move out of it) and
    /// the ones `stale` says can still change.
    pub fn recheck(&self, stale: impl Fn(&Value) -> bool) -> Vec<i64> {
        self.rows.iter().filter(|v| self.first.contains(&id(v)) || stale(v)).map(id).collect()
    }

    /// `GET path?limit=`, then `?ids=` for the rows in `recheck` the first page didn't bring.
    pub fn fetch(be: &dyn Backend, path: &str, key: &str, query: &[(&str, String)], limit: usize, recheck: Vec<i64>) -> Result<Fresh, String> {
        let mut q = query.to_vec();
        q.push(("limit", limit.to_string()));
        let first = be.get(path, &q).map_err(|e| e.message)?;
        let came: HashSet<i64> = arr(&first, key).iter().map(id).collect();
        let asked: Vec<i64> = recheck.into_iter().filter(|i| !came.contains(i)).collect();
        let again = if asked.is_empty() {
            None
        } else {
            let mut q = query.to_vec();
            q.push(("ids", asked.iter().map(i64::to_string).collect::<Vec<_>>().join(",")));
            Some(be.get(path, &q).map_err(|e| e.message)?)
        };
        Ok(Fresh { first, asked, again })
    }

    /// Land a refresh: its first page on top, then the older rows held, swapped for what came back
    /// by id (a row asked for that didn't come back is gone).
    pub fn land(&mut self, key: &str, f: Fresh) {
        let page = arr(&f.first, key);
        let again: Vec<&Value> = f.again.as_ref().map(|a| arr(a, key).iter().collect()).unwrap_or_default();
        let ids: HashSet<i64> = page.iter().map(id).collect();
        let mut rows: Vec<Value> = page.to_vec();
        for v in self.rows.iter().filter(|v| !ids.contains(&id(v))) {
            if !f.asked.contains(&id(v)) {
                rows.push(v.clone());
            } else if let Some(n) = again.iter().find(|n| id(n) == id(v)) {
                rows.push((*n).clone());
            }
        }
        self.first = page.iter().map(id).collect();
        self.total = f.first["total"].as_u64().unwrap_or(rows.len() as u64) as usize;
        if !self.older {
            self.next = f.first["next"].as_str().map(String::from);
        }
        self.loaded = true;
        self.set(rows);
    }

    /// Land the older page that started at `before`; one for a cursor since replaced is dropped.
    pub fn land_older(&mut self, key: &str, before: &str, got: Result<Value, String>) {
        self.loading = false;
        if self.next.as_deref() != Some(before) {
            return;
        }
        let v = match got {
            Ok(v) => v,
            // Stop asking; the rows already here stay.
            Err(_) => {
                self.next = None;
                return;
            }
        };
        let page = arr(&v, key);
        let ids: HashSet<i64> = page.iter().map(id).collect();
        let mut rows: Vec<Value> = self.rows.iter().filter(|r| !ids.contains(&id(r))).cloned().collect();
        rows.extend(page.iter().cloned());
        self.older = true;
        self.next = v["next"].as_str().map(String::from);
        self.set(rows);
    }

    fn set(&mut self, rows: Vec<Value>) {
        if *self.rows != rows {
            self.rows = Rc::new(rows);
            self.version += 1;
        }
    }
}
