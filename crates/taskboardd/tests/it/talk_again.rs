//! Talk it over clicked again for the same thing brings back the terminal the last click opened,
//! while Midna still has it open, instead of opening a second one.

use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{midna, p};

/// A board whose Midna opens S9 and lists it as open until the file `closed` exists.
fn board() -> (Arc<App>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    let exe = dir.path().join("fake-midna");
    std::fs::write(
        &exe,
        "#!/bin/sh\nd=$(dirname \"$0\")\ncase \"$2\" in\n  session.open) echo '{\"id\": \"S9\"}';;\n  session.get) [ -e \"$d/closed\" ] && { echo 'no session S9' >&2; exit 1; }; echo '{\"id\": \"S9\", \"status\": {\"state\": \"idle\"}}';;\n  *) echo '{}';;\nesac\n",
    )
    .unwrap();
    std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.midna = exe;
    let app = App::for_tests(cfg);
    app.shared.lock().midna_seen_at = Some(taskboardd::util::now_iso());
    (app, dir)
}

fn talk(app: &App, body: Value) -> Value {
    api::dispatch(app, "POST", "home/talk", &Query::new(), &body).unwrap_or_else(|e| panic!("{}", e.message))
}

fn job(app: &App, v: &Value) -> taskboardd::util::Row {
    let id: i64 = v["job"].as_str().unwrap()[1..].parse().unwrap();
    app.db.q1("SELECT * FROM jobs WHERE id = ?", p![id]).unwrap().unwrap()
}

#[test]
fn talking_it_over_again_shows_the_terminal_still_open() {
    let (app, dir) = board();
    let first = talk(&app, json!({}));
    assert_eq!(talk(&app, json!({}))["job"], first["job"], "a second click while it opens doesn't open another");
    midna::run_job(&app, &job(&app, &first));

    let again = talk(&app, json!({}));
    assert_eq!(again["reused"], true, "{again}");
    let f = job(&app, &again);
    assert_eq!((f.st("kind"), taskboardd::board::job_args(&f)["session"].clone()), ("focus".to_string(), json!("S9")), "it's brought to the front");

    let other = talk(&app, json!({"project": "api"}));
    assert_eq!(job(&app, &other).st("kind"), "agent", "another project's list is another talk");

    std::fs::write(dir.path().join("closed"), "").unwrap();
    let fresh = talk(&app, json!({}));
    let j = job(&app, &fresh);
    assert_eq!(j.st("kind"), "agent", "once it's closed, a new one opens");
    assert_ne!(fresh["job"], first["job"]);
}
