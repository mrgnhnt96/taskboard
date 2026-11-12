//! `tb backlog fixed B<n> --by T<n>|<PR link>`: open issues close as fixed by finished work, all or none.
//! They show as fixed (not won't do), link the task and its PR, and the task's log lists them.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    Board { app, _dir: dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        let (p, qs) = path.split_once('?').unwrap_or((path, ""));
        let q: Query = qs.split('&').filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
        api::dispatch(&self.app, "GET", p, &q, &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn post_err(&self, path: &str, body: Value) -> (u16, String) {
        let e = api::dispatch(&self.app, "POST", path, &Query::new(), &body).expect_err("should fail");
        (e.status, e.message)
    }
    fn issue(&self, project: &str, title: &str) -> i64 {
        self.app.db.insert("issues", vec![("project", json!(project)), ("kind", json!("bug")), ("title", json!(title)), ("state", json!("open"))]).unwrap()
    }
    fn task(&self, status: &str, pr: Option<(i64, &str)>) -> i64 {
        let id = self.post("/tasks", json!({"title": "Fix saving", "detail": "Do it.", "project": "webapp"}))["id"].as_i64().unwrap();
        let mut f = vec![("status", json!(status))];
        if let Some((num, url)) = pr {
            f.extend([("pr_num", json!(num)), ("pr_url", json!(url)), ("pr_repo", json!("acme/webapp")), ("pr_host", json!("github"))]);
        }
        self.app.db.update("tasks", &json!(id), f).unwrap();
        id
    }
    fn issue_json(&self, id: i64) -> Value {
        self.get(&format!("/backlog/B{id}"))
    }
    fn task_log(&self, id: i64) -> Vec<String> {
        self.app.db.q("SELECT text FROM events WHERE task_id = ? ORDER BY id", taskboardd::p![id]).unwrap().iter().map(|r| r.st("text")).collect()
    }
    fn closed_line(&self, id: i64) -> Option<String> {
        self.app.db.q1("SELECT text FROM issue_events WHERE issue_id = ? AND kind = 'drop'", taskboardd::p![id]).unwrap().map(|r| r.st("text"))
    }
}

#[test]
fn issues_close_as_fixed_by_a_done_task_link_it_and_land_in_its_log() {
    let b = new_board();
    let t = b.task("done", Some((45, "https://github.com/acme/webapp/pull/45")));
    let (x, y) = (b.issue("webapp", "Save drops the title"), b.issue("webapp", "Save is slow"));

    let r = b.post("/backlog/fixed", json!({"ids": [format!("B{x}"), format!("B{y}")], "by": format!("T{t}"), "who": "terminal abcd1234"}));
    assert_eq!(r["count"], 2);
    assert_eq!(r["by"], format!("T{t} (PR #45)"));

    let v = b.issue_json(x);
    assert_eq!((v["state"].as_str(), v["closed_as"].as_str()), (Some("drop"), Some("fixed")));
    assert_eq!(v["fixed_by_ref"], format!("T{t}"));
    assert_eq!(v["fixed_pr"], "https://github.com/acme/webapp/pull/45");
    assert_eq!(b.closed_line(y).unwrap(), format!("Closed as fixed by T{t} (PR #45)"));
    assert!(b.task_log(t).contains(&format!("Closed B{x}, B{y} as fixed")), "{:?}", b.task_log(t));

    let listed = b.get("/backlog?project=webapp&state=all");
    assert!(listed["issues"].as_array().unwrap().iter().all(|i| i["closed_as"] == "fixed"), "{listed}");
}

#[test]
fn a_pr_link_finds_the_boards_task_for_it_or_stands_on_its_own() {
    let b = new_board();
    let t = b.task("done", Some((9, "https://github.com/acme/webapp/pull/9")));
    let x = b.issue("webapp", "Login flickers");
    b.post("/backlog/fixed", json!({"ids": [format!("B{x}")], "by": "https://github.com/acme/webapp/pull/9"}));
    let v = b.issue_json(x);
    assert_eq!((v["fixed_by_ref"].as_str(), v["fixed_pr"].as_str()), (Some(format!("T{t}").as_str()), Some("https://github.com/acme/webapp/pull/9")));
    assert!(b.task_log(t).contains(&format!("Closed B{x} as fixed")));

    let y = b.issue("webapp", "Logout flickers");
    let r = b.post("/backlog/fixed", json!({"ids": [format!("B{y}")], "by": "https://github.com/acme/other/pull/3"}));
    assert_eq!(r["task"], Value::Null);
    let v = b.issue_json(y);
    assert_eq!((v["closed_as"].as_str(), v["fixed_by"].clone(), v["fixed_pr"].as_str()), (Some("fixed"), Value::Null, Some("https://github.com/acme/other/pull/3")));
    assert_eq!(b.closed_line(y).unwrap(), "Closed as fixed by https://github.com/acme/other/pull/3");
}

#[test]
fn unfinished_failed_or_other_project_work_closes_nothing() {
    let b = new_board();
    let x = b.issue("webapp", "Save drops the title");
    let open = b.task("working", None);
    let (code, why) = b.post_err("/backlog/fixed", json!({"ids": [format!("B{x}")], "by": format!("T{open}")}));
    assert_eq!(code, 409);
    assert!(why.contains("isn't done yet") && why.contains("--fixes"), "{why}");

    let failed = b.task("done", None);
    b.app.db.update("tasks", &json!(failed), vec![("failed", json!(1))]).unwrap();
    assert_eq!(b.post_err("/backlog/fixed", json!({"ids": [format!("B{x}")], "by": format!("T{failed}")})).0, 409);

    let done = b.task("done", None);
    let elsewhere = b.issue("mobile", "Phone save");
    let (code, why) = b.post_err("/backlog/fixed", json!({"ids": [format!("B{x}"), format!("B{elsewhere}")], "by": format!("T{done}")}));
    assert_eq!(code, 400, "{why}");
    assert_eq!(b.issue_json(x)["state"], "open", "all or none");

    assert_eq!(b.post_err("/backlog/fixed", json!({"ids": [format!("B{x}")], "by": "soon"})).0, 400);
    assert_eq!(b.post_err("/backlog/fixed", json!({"ids": [format!("B{x}")], "by": ""})).0, 400);
    assert_eq!(b.post_err("/backlog/fixed", json!({"ids": [], "by": format!("T{done}")})).0, 400);

    b.post(&format!("/backlog/B{x}/drop"), json!({}));
    let (code, why) = b.post_err("/backlog/fixed", json!({"ids": [format!("B{x}")], "by": format!("T{done}")}));
    assert_eq!(code, 409, "{why}");
    assert_eq!(b.issue_json(x)["closed_as"], "wont_do");
}

#[test]
fn reopening_a_fixed_issue_forgets_what_fixed_it() {
    let b = new_board();
    let t = b.task("done", None);
    let x = b.issue("webapp", "Save drops the title");
    let r = b.post("/backlog/fixed", json!({"ids": [format!("B{x}")], "by": format!("T{t}")}));
    assert_eq!(r["by"], format!("T{t}"));
    b.post(&format!("/backlog/B{x}/reopen"), json!({}));
    let v = b.issue_json(x);
    assert_eq!((v["state"].as_str(), v["closed_as"].clone(), v["fixed_by"].clone(), v["fixed_pr"].clone()), (Some("open"), Value::Null, Value::Null, Value::Null));
    b.post(&format!("/backlog/B{x}/drop"), json!({}));
    assert_eq!(b.issue_json(x)["closed_as"], "wont_do", "a won't-do close after a fixed one isn't fixed");
}
