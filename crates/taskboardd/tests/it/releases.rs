//! Releases: a project that turns them on cuts named releases, each holding the done work since the
//! one before; what isn't released yet sits in Unreleased, and a task can move between them.

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::board;
use taskboardd::config::Config;

fn new_board() -> (tempfile::TempDir, std::sync::Arc<App>) {
    let dir = tempfile::tempdir().unwrap();
    let app = App::for_tests(Config::for_tests(dir.path()));
    let projects: Vec<Value> = ["webapp", "api"]
        .iter()
        .map(|n| {
            let repo = dir.path().join(n);
            std::fs::create_dir_all(&repo).unwrap();
            json!({"name": n, "path": repo.to_string_lossy()})
        })
        .collect();
    app.db.set_setting("midna_projects", Some(&Value::Array(projects).to_string())).unwrap();
    (dir, app)
}

fn try_call(app: &App, method: &str, path: &str, query: &[(&str, &str)], body: Value) -> Result<Value, (u16, String)> {
    let q: Query = query.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    api::dispatch(app, method, path, &q, &body).map_err(|e| (e.status, e.message))
}

fn call(app: &App, method: &str, path: &str, body: Value) -> Value {
    try_call(app, method, path, &[], body).unwrap()
}

fn get(app: &App, path: &str, project: &str) -> Value {
    try_call(app, "GET", path, &[("project", project)], json!({})).unwrap()
}

fn task(app: &App, project: &str, title: &str, goal: Option<i64>) -> i64 {
    let mut b = json!({"title": title, "detail": "Do it.", "project": project});
    if let Some(g) = goal {
        b["goal_id"] = json!(g);
    }
    call(app, "POST", "/tasks", b)["id"].as_i64().unwrap()
}

fn done(app: &App, id: i64, pr: Option<(i64, &str)>) {
    let mut f = vec![("status", json!("done")), ("finished_at", json!(format!("2026-10-15T10:00:{id:02}Z")))];
    if let Some((n, phase)) = pr {
        f.push(("pr_num", json!(n)));
        f.push(("pr_url", json!(format!("https://github.com/o/webapp/pull/{n}"))));
        f.push(("pr_phase", json!(phase)));
    }
    board::update_task(app, id, f).unwrap();
}

fn releases_on(app: &App, project: &str) {
    call(app, "POST", &format!("/projects/{project}"), json!({"releases": "on"}));
}

fn refs(v: &Value) -> Vec<String> {
    v["groups"].as_array().unwrap().iter().flat_map(|g| g["tasks"].as_array().unwrap().iter().map(|t| t["ref"].as_str().unwrap().to_string())).collect()
}

fn project(app: &App, name: &str) -> Value {
    call(app, "GET", "/projects", json!({}))["projects"].as_array().unwrap().iter().find(|p| p["name"] == name).unwrap().clone()
}

#[test]
fn releases_are_off_until_the_project_turns_them_on() {
    let (_d, app) = new_board();
    assert_eq!(project(&app, "webapp")["releases"], false);
    assert_eq!(project(&app, "webapp")["releases_set"], Value::Null);
    let v = get(&app, "/releases", "webapp");
    assert_eq!(v["on"], false);
    let (code, msg) = try_call(&app, "POST", "/releases", &[], json!({"name": "v1", "project": "webapp"})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("tb project set webapp --releases on"), "{msg}");
    assert_eq!(try_call(&app, "GET", "/releases/unreleased", &[("project", "webapp")], json!({})).unwrap_err().0, 409);

    releases_on(&app, "webapp");
    assert_eq!(project(&app, "webapp")["releases"], true);
    assert_eq!(project(&app, "webapp")["releases_set"], true);
    assert_eq!(project(&app, "api")["releases"], false, "only the project that said");
    assert_eq!(get(&app, "/releases", "webapp")["on"], true);

    call(&app, "POST", "/projects/webapp", json!({"releases": "off"}));
    assert_eq!(project(&app, "webapp")["releases_set"], false);
    call(&app, "POST", "/projects/webapp", json!({"releases": "default"}));
    assert_eq!(project(&app, "webapp")["releases_set"], Value::Null);
    assert_eq!(project(&app, "webapp")["releases"], false);
}

#[test]
fn a_cut_takes_the_done_work_whose_pr_merged_or_had_none() {
    let (_d, app) = new_board();
    releases_on(&app, "webapp");
    let g = call(&app, "POST", "/goals", json!({"name": "Ship login", "project": "webapp"}))["id"].as_i64().unwrap();
    let merged = task(&app, "webapp", "Login form", Some(g));
    let no_pr = task(&app, "webapp", "Tidy the README", None);
    let open_pr = task(&app, "webapp", "Password reset", Some(g));
    let not_done = task(&app, "webapp", "Sign up", Some(g));
    let failed = task(&app, "webapp", "Broken one", None);
    let other = task(&app, "api", "Login endpoint", None);
    done(&app, merged, Some((12, "merged")));
    done(&app, no_pr, None);
    done(&app, open_pr, Some((13, "review")));
    done(&app, failed, None);
    board::update_task(&app, failed, vec![("failed", json!(1))]).unwrap();
    releases_on(&app, "api");
    done(&app, other, None);
    let _ = not_done;

    let un = get(&app, "/releases/unreleased", "webapp");
    assert_eq!(refs(&un), vec![format!("T{merged}"), format!("T{no_pr}")]);

    let r = call(&app, "POST", "/releases", json!({"name": "v1.0", "project": "webapp", "notes": "First cut."}));
    assert_eq!(r["ref"], "R1");
    assert_eq!(r["tasks"], 2);
    assert_eq!(r["groups"][0]["goal"], format!("G{g}"));
    assert_eq!(r["groups"][1]["goal"], Value::Null, "tasks with no goal come last");
    let md = r["markdown"].as_str().unwrap();
    assert!(md.starts_with("## v1.0 ("), "{md}");
    assert!(md.contains("First cut."), "{md}");
    assert!(md.contains("### Ship login\n\n- Login form ([#12](https://github.com/o/webapp/pull/12))"), "{md}");
    assert!(md.contains("### Other changes\n\n- Tidy the README\n"), "{md}");

    assert_eq!(get(&app, "/releases/unreleased", "webapp")["tasks"], 0);
    assert_eq!(get(&app, "/releases/unreleased", "api")["tasks"], 1, "another project's work stays out");
    let (code, _) = try_call(&app, "POST", "/releases", &[], json!({"name": "v1.1", "project": "webapp"})).unwrap_err();
    assert_eq!(code, 409, "nothing to release");
    assert_eq!(try_call(&app, "POST", "/releases", &[], json!({"name": "V1.0", "project": "api"})).unwrap()["ref"], "R2", "names are per project");

    board::update_task(&app, open_pr, vec![("pr_phase", json!("merged"))]).unwrap();
    assert_eq!(try_call(&app, "POST", "/releases", &[], json!({"name": "v1.0", "project": "webapp"})).unwrap_err().0, 409, "the name is taken");
    let r3 = call(&app, "POST", "/releases", json!({"name": "v1.1", "project": "webapp"}));
    assert_eq!(refs(&r3), vec![format!("T{open_pr}")], "a second cut takes only the new work");

    let list = get(&app, "/releases", "webapp");
    let names: Vec<&str> = list["releases"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap()).collect();
    assert_eq!(names, vec!["v1.1", "v1.0"], "newest first");
    assert_eq!(list["unreleased"], 0);
}

#[test]
fn a_done_task_moves_between_releases_in_its_project() {
    let (_d, app) = new_board();
    releases_on(&app, "webapp");
    releases_on(&app, "api");
    let a = task(&app, "webapp", "Login form", None);
    done(&app, a, None);
    call(&app, "POST", "/releases", json!({"name": "v1", "project": "webapp"}));
    let b = task(&app, "webapp", "Logout", None);
    done(&app, b, None);
    call(&app, "POST", "/releases", json!({"name": "v2", "project": "webapp"}));
    let c = task(&app, "api", "Endpoint", None);
    done(&app, c, None);
    call(&app, "POST", "/releases", json!({"name": "v1", "project": "api"}));

    let v = call(&app, "POST", "/releases/move", json!({"task": format!("T{a}"), "release": "R2"}));
    assert_eq!(v["release"], "R2");
    assert_eq!(refs(&get(&app, "/releases/R2", "webapp")), vec![format!("T{a}"), format!("T{b}")]);
    assert_eq!(get(&app, "/releases/R1", "webapp")["tasks"], 0);
    assert_eq!(try_call(&app, "POST", "/releases/move", &[], json!({"task": format!("T{a}"), "release": "R2"})).unwrap_err().0, 409, "already there");
    let (code, msg) = try_call(&app, "POST", "/releases/move", &[], json!({"task": format!("T{a}"), "release": "R3"})).unwrap_err();
    assert_eq!(code, 409, "{msg}");

    let v = call(&app, "POST", "/releases/move", json!({"task": format!("T{a}"), "release": "unreleased"}));
    assert_eq!(v["release"], Value::Null);
    assert_eq!(refs(&get(&app, "/releases/unreleased", "webapp")), vec![format!("T{a}")]);

    let open = task(&app, "webapp", "Not yet", None);
    assert_eq!(try_call(&app, "POST", "/releases/move", &[], json!({"task": format!("T{open}"), "release": "R2"})).unwrap_err().0, 409, "only done work");

    let s = call(&app, "POST", "/releases/R2", json!({"name": "v2.0", "notes": "Logout lands."}));
    assert_eq!((s["name"].as_str(), s["notes"].as_str()), (Some("v2.0"), Some("Logout lands.")));
    assert_eq!(try_call(&app, "POST", "/releases/R2", &[], json!({"name": "v1"})).unwrap_err().0, 409, "the name is taken");
}

#[test]
fn a_release_script_is_a_project_setting_the_cut_check_hands_back() {
    let (_d, app) = new_board();
    releases_on(&app, "webapp");
    assert_eq!(project(&app, "webapp")["release_script"], Value::Null);
    let v = call(&app, "POST", "/projects/webapp", json!({"release_script": "  scripts/release.sh  "}));
    assert_eq!(v["release_script"], "scripts/release.sh");
    assert_eq!(project(&app, "webapp")["release_script"], "scripts/release.sh");
    assert_eq!(project(&app, "api")["release_script"], Value::Null);
    assert_eq!(try_call(&app, "POST", "/projects/webapp", &[], json!({"release_script": "a\nb"})).unwrap_err().0, 400);

    // The check refuses a doomed cut before any script runs.
    let (code, msg) = try_call(&app, "POST", "/releases", &[], json!({"name": "v1", "project": "webapp", "check": true})).unwrap_err();
    assert_eq!(code, 409, "{msg}");
    let a = task(&app, "webapp", "Login form", None);
    done(&app, a, None);
    let v = call(&app, "POST", "/releases", json!({"name": "v1", "project": "webapp", "check": true}));
    assert_eq!(v["script"], "scripts/release.sh");
    assert_eq!(v["name"], "v1");
    assert!(v["dir"].as_str().unwrap().ends_with("webapp"), "{v}");
    assert_eq!(get(&app, "/releases", "webapp")["releases"].as_array().unwrap().len(), 0, "a check cuts nothing");

    call(&app, "POST", "/releases", json!({"name": "v1", "project": "webapp"}));
    let b = task(&app, "webapp", "Logout", None);
    done(&app, b, None);
    assert_eq!(try_call(&app, "POST", "/releases", &[], json!({"name": "v1", "project": "webapp", "check": true})).unwrap_err().0, 409, "the name is taken");

    let v = call(&app, "POST", "/projects/webapp", json!({"release_script": "none"}));
    assert_eq!(v["release_script"], Value::Null);
}

#[test]
fn a_failed_release_hands_its_work_and_name_back_for_the_next_cut() {
    let (_d, app) = new_board();
    releases_on(&app, "webapp");
    call(&app, "POST", "/projects/webapp", json!({"release_script": "scripts/release.sh"}));
    let a = task(&app, "webapp", "Login form", None);
    done(&app, a, None);
    call(&app, "POST", "/releases", json!({"name": "v1.0", "project": "webapp"}));
    let b = task(&app, "webapp", "Logout", None);
    done(&app, b, None);
    let r = call(&app, "POST", "/releases", json!({"name": "v1.1", "project": "webapp"}));
    assert_eq!(state_project(&app, "webapp")["latest_release"]["name"], "v1.1");

    let f = call(&app, "POST", &format!("/releases/{}/fail", r["ref"].as_str().unwrap()), json!({"reason": "Notarizing timed out."}));
    assert!(f["failed_at"].is_string(), "{f}");
    assert_eq!(f["failed_reason"], "Notarizing timed out.");
    assert_eq!(f["tasks"], 0);
    assert_eq!(refs(&get(&app, "/releases/unreleased", "webapp")), vec![format!("T{b}")], "its work waits again");
    assert_eq!(state_project(&app, "webapp")["latest_release"]["name"], "v1.0", "a failed release isn't the newest");
    let log = call(&app, "GET", &format!("/tasks/{b}/log"), json!({}));
    assert!(log.to_string().contains("release v1.1 (R2) failed"), "{log}");
    assert_eq!(try_call(&app, "POST", "/releases/R2/fail", &[], json!({})).unwrap_err().0, 409, "already failed");
    assert_eq!(try_call(&app, "POST", "/releases/move", &[], json!({"task": format!("T{b}"), "release": "R2"})).unwrap_err().0, 409, "no work goes in a failed release");

    // The check tells the script it replaces a failed release, and the name cuts again.
    let v = call(&app, "POST", "/releases", json!({"name": "v1.1", "project": "webapp", "check": true}));
    assert_eq!(v["replaces"], true);
    assert_eq!(try_call(&app, "POST", "/releases", &[], json!({"name": "V1.0", "project": "webapp", "check": true})).unwrap_err().0, 409, "a release that went out keeps its name");
    let again = call(&app, "POST", "/releases", json!({"name": "v1.1", "project": "webapp"}));
    assert_eq!(again["ref"], "R3");
    assert_eq!(refs(&again), vec![format!("T{b}")]);
    assert_eq!(again["failed_at"], Value::Null);
    let names: Vec<(String, bool)> = get(&app, "/releases", "webapp")["releases"].as_array().unwrap().iter()
        .map(|r| (r["name"].as_str().unwrap().to_string(), r["failed_at"].is_string())).collect();
    assert_eq!(names, vec![("v1.1".into(), false), ("v1.1".into(), true), ("v1.0".into(), false)]);
}

fn state_project(app: &App, name: &str) -> Value {
    call(app, "GET", "/state", json!({}))["projects"].as_array().unwrap().iter().find(|p| p["name"] == name).unwrap().clone()
}

#[test]
fn the_state_carries_each_projects_release_waits_and_backlog() {
    let (_d, app) = new_board();
    let web = state_project(&app, "webapp");
    assert!(web["path"].as_str().unwrap().ends_with("webapp"));
    assert_eq!(web["releases"], false);
    assert_eq!(web["latest_release"], Value::Null, "nothing about releases while they're off");
    assert_eq!(web["unreleased"], Value::Null);
    assert_eq!(web["waits_on"], json!([]));
    assert_eq!(web["backlog_open"], 0);

    releases_on(&app, "webapp");
    call(&app, "POST", "/projects/webapp", json!({"waits_on": ["api"]}));
    let first = task(&app, "webapp", "Login form", None);
    done(&app, first, None);
    let web = state_project(&app, "webapp");
    assert_eq!(web["releases"], true);
    assert_eq!(web["latest_release"], Value::Null, "no cut yet");
    assert_eq!(web["unreleased"], 1);
    assert_eq!(web["waits_on"], json!(["api"]));

    call(&app, "POST", "/releases", json!({"name": "v1.0", "project": "webapp"}));
    done(&app, task(&app, "webapp", "Password reset", None), None);
    let r = call(&app, "POST", "/releases", json!({"name": "v1.1", "project": "webapp"}));
    done(&app, task(&app, "webapp", "Sign up", None), None);
    done(&app, task(&app, "api", "Login endpoint", None), None);
    let web = state_project(&app, "webapp");
    assert_eq!(web["latest_release"]["name"], "v1.1", "the newest cut");
    assert_eq!(web["latest_release"]["ref"], r["ref"]);
    assert_eq!(web["latest_release"]["cut_at"], r["cut_at"]);
    assert_eq!(web["unreleased"], 1, "only what came after the cut");

    for (title, project) in [("Crash on login", "webapp"), ("Slow sync", "webapp"), ("Bad header", "api")] {
        call(&app, "POST", "/backlog", json!({"title": title, "project": project, "kind": "bug"}));
    }
    let dropped = call(&app, "POST", "/backlog", json!({"title": "Old idea", "project": "webapp", "kind": "bug"}))["id"].as_i64().unwrap();
    call(&app, "POST", &format!("/backlog/{dropped}/drop"), json!({"reason": "Not needed."}));
    assert_eq!(state_project(&app, "webapp")["backlog_open"], 2, "open issues only");
    let api = state_project(&app, "api");
    assert_eq!(api["backlog_open"], 1);
    assert_eq!(api["waits_on"], json!([]), "waiting is one way");
    assert_eq!(api["unreleased"], Value::Null, "api's releases are still off");
}

#[test]
fn the_release_list_pages_and_counts_the_whole_project() {
    let (_d, app) = new_board();
    releases_on(&app, "webapp");
    for n in 0..5 {
        let t = task(&app, "webapp", &format!("Work {n}"), None);
        done(&app, t, None);
        call(&app, "POST", "/releases", json!({"name": format!("v1.{n}"), "project": "webapp"}));
    }
    call(&app, "POST", "/releases/R4/fail", json!({}));
    let page = |q: &[(&str, &str)]| {
        let mut q = q.to_vec();
        q.push(("project", "webapp"));
        let v = try_call(&app, "GET", "/releases", &q, json!({})).unwrap();
        let names: Vec<String> = v["releases"].as_array().unwrap().iter().map(|r| r["name"].as_str().unwrap().to_string()).collect();
        (names, v["more"].as_bool().unwrap(), v["counts"].clone())
    };
    let (names, more, counts) = page(&[("limit", "2")]);
    assert_eq!((names, more), (vec!["v1.4".to_string(), "v1.3".to_string()], true));
    assert_eq!(counts, json!({"total": 5, "recent": 5, "failed": 1}), "counts are the whole project's");
    let (names, more, _) = page(&[("limit", "2"), ("before", "4")]);
    assert_eq!((names, more), (vec!["v1.2".to_string(), "v1.1".to_string()], true));
    let (names, more, _) = page(&[("limit", "2"), ("before", "2")]);
    assert_eq!((names, more), (vec!["v1.0".to_string()], false));
    let (names, more, _) = page(&[]);
    assert_eq!((names.len(), more), (5, false), "without a limit, every release");

    // A release cut over 30 days ago isn't recent.
    app.db.update("releases", &json!(1), vec![("cut_at", json!("2020-01-01T00:00:00Z"))]).unwrap();
    let (_, _, counts) = page(&[("limit", "1")]);
    assert_eq!(counts, json!({"total": 5, "recent": 4, "failed": 1}));
}

fn goal(app: &App, project: &str) -> i64 {
    call(app, "POST", "/goals", json!({"name": "Ship login", "project": project}))["id"].as_i64().unwrap()
}

fn pending(app: &App, project: &str) -> Value {
    get(app, "/releases", project)["pending"].clone()
}

fn alert_texts(app: &App) -> Vec<String> {
    taskboardd::dispatch::alerts(app).iter().filter_map(|a| a["text"].as_str().map(str::to_string)).collect()
}

#[test]
fn a_pending_cut_waits_for_its_tasks_and_goals_then_cuts_itself() {
    let (_d, app) = new_board();
    releases_on(&app, "webapp");
    let shipped = task(&app, "webapp", "Shipped", None);
    done(&app, shipped, None);
    let t = task(&app, "webapp", "Login form", None);
    let g = goal(&app, "webapp");
    let gt = task(&app, "webapp", "Login API", Some(g));

    let v = call(&app, "POST", "/releases", json!({"name": "v1.5", "project": "webapp", "after": [format!("T{t}"), format!("g{g}"), format!("T{t}")], "no_script": true}));
    assert_eq!(v["name"], "v1.5");
    let after: Vec<&str> = v["after"].as_array().unwrap().iter().map(|a| a["ref"].as_str().unwrap()).collect();
    assert_eq!(after, vec![format!("T{t}"), format!("G{g}")]);
    assert_eq!(v["after"][0]["state"], "waiting");
    assert_eq!((v["after"][0]["title"].as_str(), v["after"][0]["status"].as_str()), (Some("Login form"), Some("queued")));
    assert_eq!((v["after"][1]["title"].as_str(), v["after"][1]["done"].as_i64(), v["after"][1]["total"].as_i64()), (Some("Ship login"), Some(0), Some(1)));
    assert_eq!(pending(&app, "webapp")["name"], "v1.5");
    assert_eq!(pending(&app, "api"), Value::Null);

    // Nothing cuts while the work is open, or once the task is done but its PR isn't merged.
    assert!(taskboardd::pending_cut::check(&app).unwrap().is_empty());
    done(&app, t, Some((7, "review")));
    done(&app, gt, None);
    let p = pending(&app, "webapp");
    assert_eq!(p["after"][1]["state"], "met");
    assert_eq!(p["after"][1]["done"], 1);
    assert_eq!((p["after"][0]["status"].as_str(), p["after"][0]["pr_phase"].as_str()), (Some("done"), Some("review")), "done, its PR in review");
    assert!(taskboardd::pending_cut::check(&app).unwrap().is_empty());

    done(&app, t, Some((7, "merged")));
    let ready = taskboardd::pending_cut::check(&app).unwrap();
    assert_eq!(ready.len(), 1);
    assert_eq!(pending(&app, "webapp")["running"], true);
    // Marked running, so the next look doesn't cut it twice, and it can't be cancelled mid-cut.
    assert!(taskboardd::pending_cut::check(&app).unwrap().is_empty());
    assert_eq!(try_call(&app, "POST", "/releases/pending/cancel", &[], json!({"project": "webapp"})).unwrap_err().0, 409);

    let rel = taskboardd::pending_cut::fire(&app, &ready[0]).unwrap().unwrap();
    let v = get(&app, &format!("/releases/{rel}"), "webapp");
    assert_eq!(v["name"], "v1.5");
    let mut got = refs(&v);
    got.sort();
    let mut want = vec![format!("T{shipped}"), format!("T{t}"), format!("T{gt}")];
    want.sort();
    assert_eq!(got, want);
    assert_eq!(pending(&app, "webapp"), Value::Null);
}

#[test]
fn a_project_holds_one_pending_cut_and_its_name_until_cancelled() {
    let (_d, app) = new_board();
    releases_on(&app, "webapp");
    let t = task(&app, "webapp", "Login form", None);
    let shipped = task(&app, "webapp", "Shipped", None);
    done(&app, shipped, None);
    let other = task(&app, "api", "Elsewhere", None);
    let make = |name: &str, after: Value| try_call(&app, "POST", "/releases", &[], json!({"name": name, "project": "webapp", "after": after}));

    assert_eq!(make("v2", json!([])).unwrap_err().0, 400);
    assert_eq!(make("v2", json!(["nope"])).unwrap_err().0, 400);
    assert_eq!(make("v2", json!([format!("T{other}")])).unwrap_err().0, 409);
    assert_eq!(make("v2", json!(["T9999"])).unwrap_err().0, 404);
    make("v2", json!(format!("T{t}"))).unwrap();

    let (code, msg) = make("v3", json!([format!("T{t}")])).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("tb release cancel"), "{msg}");
    // The pending name is held from a cut by hand, in any case.
    let (code, msg) = try_call(&app, "POST", "/releases", &[], json!({"name": "V2", "project": "webapp"})).unwrap_err();
    assert_eq!(code, 409);
    assert!(msg.contains("pending cut"), "{msg}");

    assert_eq!(call(&app, "POST", "/releases/pending/cancel", json!({"project": "webapp"}))["cancelled"], true);
    assert_eq!(pending(&app, "webapp"), Value::Null);
    assert_eq!(try_call(&app, "POST", "/releases/pending/cancel", &[], json!({"project": "webapp"})).unwrap_err().0, 404);
    call(&app, "POST", "/releases", json!({"name": "v2", "project": "webapp"}));
}

#[test]
fn a_pending_cut_drops_and_tells_the_owner_when_its_work_fails() {
    let (_d, app) = new_board();
    releases_on(&app, "webapp");
    let t = task(&app, "webapp", "Login form", None);
    call(&app, "POST", "/releases", json!({"name": "v1.5", "project": "webapp", "after": [format!("T{t}")]}));
    board::update_task(&app, t, vec![("failed", json!(1))]).unwrap();
    assert!(taskboardd::pending_cut::check(&app).unwrap().is_empty());
    assert_eq!(pending(&app, "webapp"), Value::Null);
    assert!(alert_texts(&app).iter().any(|a| a.contains("v1.5") && a.contains(&format!("T{t} failed"))), "{:?}", alert_texts(&app));

    // A failed task can't hold a new one, and a PR closed unmerged drops one.
    assert_eq!(try_call(&app, "POST", "/releases", &[], json!({"name": "v1.5", "project": "webapp", "after": [format!("T{t}")]})).unwrap_err().0, 409);
    let t2 = task(&app, "webapp", "Signup form", None);
    call(&app, "POST", "/releases", json!({"name": "v1.6", "project": "webapp", "after": [format!("T{t2}")]}));
    done(&app, t2, Some((8, "declined")));
    taskboardd::pending_cut::check(&app).unwrap();
    assert_eq!(pending(&app, "webapp"), Value::Null);
    assert!(alert_texts(&app).iter().any(|a| a.contains("v1.6") && a.contains("closed without merging")));
}

#[test]
fn a_pending_cut_with_nothing_to_release_drops_instead_of_cutting() {
    let (_d, app) = new_board();
    releases_on(&app, "webapp");
    let t = task(&app, "webapp", "Login form", None);
    call(&app, "POST", "/releases", json!({"name": "v1.5", "project": "webapp", "after": [format!("T{t}")], "no_script": true}));
    done(&app, t, None);
    // Someone cut by hand in the meantime.
    call(&app, "POST", "/releases", json!({"name": "v1.4", "project": "webapp"}));
    let ready = taskboardd::pending_cut::check(&app).unwrap();
    assert_eq!(taskboardd::pending_cut::fire(&app, &ready[0]).unwrap(), None);
    assert_eq!(pending(&app, "webapp"), Value::Null);
    assert!(alert_texts(&app).iter().any(|a| a.contains("v1.5") && a.contains("Unreleased")));
}

#[test]
fn a_pending_cut_is_hidden_and_held_while_releases_are_off() {
    let (_d, app) = new_board();
    releases_on(&app, "webapp");
    let t = task(&app, "webapp", "Login form", None);
    call(&app, "POST", "/releases", json!({"name": "v1.5", "project": "webapp", "after": [format!("T{t}")], "no_script": true}));
    call(&app, "POST", "/projects/webapp", json!({"releases": "off"}));
    done(&app, t, None);
    assert_eq!(pending(&app, "webapp"), Value::Null);
    assert!(taskboardd::pending_cut::check(&app).unwrap().is_empty());
    releases_on(&app, "webapp");
    assert_eq!(pending(&app, "webapp")["name"], "v1.5");
}

#[test]
fn a_long_release_list_comes_a_page_at_a_time() {
    let (_d, app) = new_board();
    releases_on(&app, "webapp");
    for k in 0..340 {
        app.db.insert("releases", vec![("project", json!("webapp")), ("name", json!(format!("v{k}"))), ("cut_at", json!("2026-10-15T10:00:00Z"))]).unwrap();
    }
    let whole = get(&app, "/releases", "webapp");
    assert_eq!(whole["releases"].as_array().unwrap().len(), 340, "no limit: every release, as tb release list reads it");
    assert_eq!((whole["total"].as_i64(), whole["more"].as_bool()), (Some(340), Some(false)));

    let (mut seen, mut before) = (Vec::new(), None::<String>);
    loop {
        let mut q = vec![("project", "webapp"), ("limit", "100")];
        if let Some(b) = &before {
            q.push(("before", b.as_str()));
        }
        let page = try_call(&app, "GET", "/releases", &q, json!({})).unwrap();
        assert_eq!(page["total"], 340);
        let rels = page["releases"].as_array().unwrap();
        assert!(rels.len() <= 100);
        seen.extend(rels.iter().map(|r| r["id"].as_i64().unwrap()));
        before = rels.last().map(|r| r["ref"].as_str().unwrap().to_string());
        if page["more"] != true {
            break;
        }
    }
    assert_eq!(seen.len(), 340, "paging back walks every release");
    assert!(seen.windows(2).all(|p| p[0] > p[1]), "newest first, each once");
}

fn git(dir: &std::path::Path, args: &[&str]) {
    assert!(std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap().status.success(), "git {args:?}");
}

#[test]
fn the_board_looks_for_a_release_run_only_in_a_github_checkout() {
    let (d, app) = new_board();
    releases_on(&app, "webapp");
    let (gh, other) = (d.path().join("webapp"), d.path().join("api"));
    for (repo, origin) in [(&gh, "https://github.com/o/webapp.git"), (&other, "https://bitbucket.org/o/api.git")] {
        git(repo, &["init", "-q"]);
        git(repo, &["remote", "add", "origin", origin]);
    }
    done(&app, task(&app, "webapp", "Login form", None), None);
    let r = call(&app, "POST", "/releases", json!({"name": "v1", "project": "webapp", "cwd": gh.to_string_lossy(), "script_secs": 30}));
    assert_eq!(r["run_looking"], true, "tb release cut waits for the run's link");
    assert_eq!(r["run_url"], Value::Null);

    releases_on(&app, "api");
    done(&app, task(&app, "api", "Login endpoint", None), None);
    let r = call(&app, "POST", "/releases", json!({"name": "v1", "project": "api", "cwd": other.to_string_lossy()}));
    assert_eq!(r["run_looking"], false, "off GitHub there's no run to wait for");
}
