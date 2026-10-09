//! Follow-ups found retesting v0.1.0-beta.5: stacked PRs (#53), the author review and steps (#55),
//! `tb done` (#56) and the PR bar (#57).

use std::path::Path;
use std::process::Command;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{PrLink, Row, RowExt};
use taskboardd::{board, dispatch, midna, prflow, reports, stack};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn new_board_with(config: &str, tweak: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.pr.gh = fake_gh(dir.path()).to_string_lossy().to_string();
    tweak(&mut cfg);
    std::fs::write(&cfg.config_path, config).unwrap();
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

fn new_board() -> Board {
    new_board_with("", |_| {})
}

impl Board {
    fn get(&self, path: &str) -> Value {
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn post_err(&self, path: &str, body: Value) -> (u16, String) {
        let e = api::dispatch(&self.app, "POST", path, &Query::new(), &body).expect_err("should fail");
        (e.status, e.message)
    }
    fn report(&self, event: &str, extra: Value) -> Result<Value, (u16, String)> {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| (e.status, e.message))
    }
    fn repo(&self) -> String {
        self.dir.path().join("webapp").to_string_lossy().to_string()
    }
    fn session(&self) {
        midna::sync(&self.app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": self.repo(), "status": {"state": "working"}})], &[]).unwrap();
    }
    fn task(&self, title: &str, extra: Value) -> i64 {
        let mut body = json!({"title": title, "detail": "Do it.", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body)["id"].as_i64().unwrap()
    }
    fn take(&self, id: i64) {
        self.session();
        self.report("tb.take", json!({"task": format!("T{id}")})).unwrap();
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn card(&self, id: i64) -> Value {
        self.get(&format!("tasks/T{id}"))
    }
    fn flow(&self, id: i64) -> Value {
        Value::Object(taskboardd::util::jloads_obj(self.row(id).s("pr_flow")))
    }
    fn link(&self, id: i64, num: i64, branch: &str) {
        let pr = PrLink { host: "github".into(), repo: "acme/webapp".into(), num, url: format!("https://github.com/acme/webapp/pull/{num}") };
        let t = self.row(id);
        self.app.db.tx(|| prflow::link_pr(&self.app, &t, &pr, "test")).unwrap();
        let rec = json!({"state": "OPEN", "head": format!("{num:0>40}"), "branch": branch, "base": "main", "checks": [], "failed": [], "running": 0,
                         "comments": 0, "approvals": 0, "review_decision": "", "requested": 1});
        prflow::merge_flow(&self.app, id, vec![("rec", rec)]).unwrap();
        board::update_task(&self.app, id, vec![("status", json!("done"))]).unwrap();
    }
    fn fail_gh(&self, on: bool) {
        let marker = self.dir.path().join("gh.fail");
        if on {
            std::fs::write(marker, "").unwrap();
        } else {
            let _ = std::fs::remove_file(marker);
        }
    }
}

/// A `gh` that records its arguments and answers like `gh pr edit`, or fails while `gh.fail` exists.
fn fake_gh(dir: &Path) -> std::path::PathBuf {
    let gh = dir.join("gh");
    let log = dir.join("gh.log");
    let fail = dir.join("gh.fail");
    std::fs::write(
        &gh,
        format!(
            "#!/bin/sh\nif [ -f '{}' ]; then echo 'HTTP 502: bad gateway' >&2; exit 1; fi\necho \"$@\" >> '{}'\necho https://github.com/acme/webapp/pull/77\n",
            fail.display(),
            log.display()
        ),
    )
    .unwrap();
    Command::new("chmod").arg("+x").arg(&gh).status().unwrap();
    gh
}

fn alerts_for(b: &Board, id: i64) -> Vec<Value> {
    dispatch::alerts(&b.app).into_iter().filter(|a| a["task_id"].as_i64() == Some(id)).collect()
}

// ------------------------------------------------------------------ #53 stacked PRs

#[test]
fn a_failed_retarget_is_tried_again_and_its_alert_clears() {
    let b = new_board();
    let parent = b.task("Add the endpoint", json!({}));
    let child = b.task("Use the endpoint", json!({"stack_on": format!("T{parent}")}));
    b.link(parent, 12, "feat/endpoint");
    b.link(child, 13, "feat/use");

    b.fail_gh(true);
    b.post(&format!("/tasks/T{parent}/pr/merged"), json!({}));
    let f = b.flow(child);
    assert!(f.get("retargeted").is_none(), "{f}");
    assert!(f["retarget_error"].as_str().unwrap().contains("502"), "{f}");
    assert_eq!(f["retarget_tries"], 1);
    assert!(f["retarget_at"].is_string());
    let up = alerts_for(&b, child);
    assert_eq!(up.len(), 1, "{up:?}");
    assert!(up[0]["text"].as_str().unwrap().contains("tries again"), "{up:?}");
    dispatch::prune_alerts(&b.app).unwrap();
    assert_eq!(alerts_for(&b, child).len(), 1, "the alert stays while the PR points at a dead branch");
    assert_eq!(b.card(child)["pr"]["bar"]["retarget_error"], f["retarget_error"]);

    // Too soon: nothing is tried.
    b.fail_gh(false);
    assert_eq!(stack::retarget(&b.app).unwrap(), 0);
    // Still failing after the wait: one more try, no second alert.
    b.fail_gh(true);
    prflow::merge_flow(&b.app, child, vec![("retarget_at", json!("2000-01-01T00:00:00"))]).unwrap();
    assert_eq!(stack::retarget(&b.app).unwrap(), 0);
    assert_eq!(b.flow(child)["retarget_tries"], 2);
    assert_eq!(alerts_for(&b, child).len(), 1);

    // It works next time: done, and the alert is gone.
    b.fail_gh(false);
    prflow::merge_flow(&b.app, child, vec![("retarget_at", json!("2000-01-01T00:00:00"))]).unwrap();
    assert_eq!(stack::retarget(&b.app).unwrap(), 1);
    let f = b.flow(child);
    assert_eq!(f["retargeted"], "main", "{f}");
    assert!(f.get("retarget_error").is_none() && f.get("retarget_at").is_none(), "{f}");
    assert!(alerts_for(&b, child).is_empty());
    assert_eq!(stack::retarget(&b.app).unwrap(), 0, "only once");
}

#[test]
fn an_old_failed_retarget_is_tried_again() {
    let b = new_board();
    let parent = b.task("Add the endpoint", json!({}));
    let child = b.task("Use the endpoint", json!({"stack_on": format!("T{parent}")}));
    b.link(parent, 12, "feat/endpoint");
    b.link(child, 13, "feat/use");
    board::update_task(&b.app, parent, vec![("pr_state", json!("MERGED"))]).unwrap();
    prflow::merge_flow(&b.app, child, vec![("retargeted", json!("failed: HTTP 502"))]).unwrap();
    assert_eq!(stack::retarget(&b.app).unwrap(), 1);
    assert_eq!(b.flow(child)["retargeted"], "main");
}

#[test]
fn the_stack_parent_is_listed_as_a_wait() {
    let b = new_board();
    let other = b.task("Schema", json!({}));
    let parent = b.task("Add the endpoint", json!({}));
    let child = b.task("Use the endpoint", json!({"stack_on": format!("T{parent}"), "waits_for": format!("T{other}")}));
    let w = b.card(child)["waits_for_state"].clone();
    let refs: Vec<&str> = w.as_array().unwrap().iter().map(|x| x["ref"].as_str().unwrap()).collect();
    assert_eq!(refs, vec![format!("T{other}"), format!("T{parent}")], "{w}");
    assert_eq!(w[1]["stack"], true);
    assert_eq!(w[0]["stack"], false);
    assert_eq!(w[1]["done"], false);
}

#[test]
fn a_task_others_stack_on_must_end_in_a_pr() {
    let b = new_board();
    let parent = b.task("Add the endpoint", json!({}));
    let child = b.task("Use the endpoint", json!({"stack_on": format!("T{parent}")}));
    let (code, why) = b.post_err(&format!("/tasks/T{parent}"), json!({"ships_pr": "no"}));
    assert_eq!(code, 409, "{why}");
    assert!(why.contains(&format!("T{child} stacks on T{parent}")), "{why}");

    b.take(parent);
    let (code, why) = b.report("tb.done", json!({"summary": "Done", "no_pr": "not needed"})).unwrap_err();
    assert_eq!(code, 409, "{why}");
    assert!(why.contains(&format!("T{child}")), "{why}");

    // Once the child moves off it, either is fine.
    b.post(&format!("/tasks/T{child}"), json!({"stack_on": "none"}));
    b.post(&format!("/tasks/T{parent}"), json!({"ships_pr": "no"}));
    assert_eq!(b.row(parent).i("ships_pr"), Some(0));
}

// ------------------------------------------------------------------ #55 the author review and steps

const REVIEW: &str = r#"
[[steps]]
name = "Author review"
prompt = "Run the review"
check = "true"
per_head = true
min_gap_mins = 10
bar = "WD"
"#;

fn round(b: &Board, head: &str, ok: bool, result: Value, output: &str) {
    b.report("tb.step", json!({"name": "Author review", "via": "done", "ok": ok, "head": head, "result": result, "output": output})).unwrap();
}

fn next_round_at(b: &Board) -> Value {
    let mut q = Query::new();
    q.insert("session".into(), "s1".into());
    api::dispatch(&b.app, "GET", "/steps", &q, &Value::Null).unwrap()["steps"][0]["next_round_at"].clone()
}

#[test]
fn an_unreviewable_or_unfinished_round_keeps_no_gap() {
    let b = new_board_with(REVIEW, |_| {});
    let id = b.task("Add login", json!({}));
    b.take(id);
    let head = "a".repeat(40);
    round(&b, &head, true, json!({"verdict": "skip"}), "");
    assert_eq!(next_round_at(&b), Value::Null, "a skipped round doesn't hold the next");
    round(&b, &head, false, json!(null), "(Check stopped after 600s)");
    assert_eq!(next_round_at(&b), Value::Null, "nor does one that didn't finish");
    round(&b, &head, false, json!({"verdict": "fail"}), "");
    assert!(next_round_at(&b).is_string(), "a real round does");
}

#[test]
fn cards_show_the_review_step_in_the_pr_bar() {
    let b = new_board_with(REVIEW, |_| {});
    let id = b.task("Add login", json!({}));
    b.take(id);
    round(&b, &"b".repeat(40), true, json!({"verdict": "pass"}), "");
    b.link(id, 21, "feat/login");
    let card = board::task_card(&b.app, &b.row(id)).unwrap();
    assert_eq!(card["pr"]["bar"]["wd"]["bar"], "WD", "{}", card["pr"]["bar"]);
    assert_eq!(card["pr"]["bar"]["wd"]["headline"], "No findings");
}

#[test]
fn one_bad_step_is_left_out_and_named_in_an_alert() {
    let config = format!("{REVIEW}\n[[steps]]\nname = \"Lint\"\nrun = \"make lint\"\nchek = \"oops\"\n\n[[steps]]\nname = \"Sign-off\"\nowner = true\n");
    let b = new_board_with(&config, |_| {});
    let id = b.task("Add login", json!({}));
    let names: Vec<String> = taskboardd::steps::for_task(&b.app, &b.row(id)).iter().map(|s| s.name.clone()).collect();
    assert_eq!(names, vec!["Author review", "Sign-off"]);
    let up: Vec<Value> = dispatch::alerts(&b.app).into_iter().filter(|a| a["key"] == "steps:lint").collect();
    assert_eq!(up.len(), 1);
    let text = up[0]["text"].as_str().unwrap();
    assert!(text.contains("Step “Lint”") && text.contains("chek"), "{text}");
    dispatch::prune_alerts(&b.app).unwrap();
    taskboardd::steps::load(&b.app);
    assert_eq!(dispatch::alerts(&b.app).iter().filter(|a| a["key"] == "steps:lint").count(), 1, "one alert, kept up");

    // Fixed: the alert goes.
    std::fs::write(&b.app.cfg.config_path, REVIEW).unwrap();
    assert_eq!(taskboardd::steps::load(&b.app).len(), 1);
    assert!(dispatch::alerts(&b.app).iter().all(|a| !a["key"].as_str().unwrap_or("").starts_with("steps:")));
}
