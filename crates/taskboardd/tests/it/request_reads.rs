//! A read request loads each settings row and the steps file once for every task it shows, and still
//! sees what it writes itself; the next request reads them again.

use std::sync::Arc;

use serde_json::json;
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{dispatch, projects, reqcache, steps};

const STEPS: &str = r#"
[[steps]]
name = "Author review"
prompt = "Review it."
"#;

fn board() -> (tempfile::TempDir, Arc<App>) {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    std::fs::write(&cfg.config_path, STEPS).unwrap();
    (dir, App::for_tests(cfg))
}

#[test]
fn a_request_sees_the_settings_it_writes() {
    let (_dir, app) = board();
    reqcache::scope(|| {
        assert_eq!(projects::pr_flow(&app, "webapp").unwrap(), "auto");
        projects::set_pr_flow(&app, "webapp", "off").unwrap();
        assert_eq!(projects::pr_flow(&app, "webapp").unwrap(), "off");

        assert_eq!(projects::pr_rules(&app, Some("webapp")).approvals, None);
        projects::set_pr_rules(&app, "webapp", &json!({"approvals": 2})).unwrap();
        assert_eq!(projects::pr_rules(&app, Some("webapp")).approvals, Some(2));

        assert!(dispatch::alerts(&app).is_empty());
        dispatch::add_alert(&app, "Look at this", None, None, None, None).unwrap();
        assert_eq!(dispatch::alerts(&app).len(), 1);
    });
}

#[test]
fn the_steps_file_is_read_once_per_request() {
    let (_dir, app) = board();
    reqcache::scope(|| {
        assert_eq!(steps::load(&app).len(), 1);
        std::fs::write(&app.cfg.config_path, "").unwrap();
        assert_eq!(steps::load(&app).len(), 1, "kept for the rest of the request");
    });
    assert!(reqcache::scope(|| steps::load(&app)).is_empty(), "the next request reads it again");
    std::fs::write(&app.cfg.config_path, STEPS).unwrap();
    assert_eq!(steps::load(&app).len(), 1, "outside a request every call reads it");
}

#[test]
fn a_setting_changed_between_requests_is_read_fresh() {
    let (_dir, app) = board();
    assert_eq!(reqcache::scope(|| projects::pr_flow(&app, "webapp")).unwrap(), "auto");
    app.db.set_setting("project_pr_flow", Some(r#"{"webapp": "on"}"#)).unwrap();
    assert_eq!(reqcache::scope(|| projects::pr_flow(&app, "webapp")).unwrap(), "on");
}
