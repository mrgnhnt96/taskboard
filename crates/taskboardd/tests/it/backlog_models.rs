//! Grouping new backlog issues runs on `[backlog] group_model` (Haiku); wave planning stays on
//! `[backlog] model`.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::triage;

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.backlog.ai = true;
    let claude = dir.path().join("fake-claude");
    std::fs::write(&claude, "#!/bin/sh\nd=$(dirname \"$0\")\nprintf '%s\\n' \"$@\" >> \"$d/all-args.txt\"\necho '{\"structured_output\": {}}'\n").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&claude, std::fs::Permissions::from_mode(0o755)).unwrap();
    cfg.claude = claude.to_string_lossy().to_string();
    f(&mut cfg);
    Board { app: App::for_tests(cfg), dir }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn issue(&self, title: &str) -> i64 {
        self.post("/backlog", json!({"title": title, "project": "webapp", "kind": "bug"}))["id"].as_i64().unwrap()
    }
    /// The `--model` of each Claude call since the last look, cleared.
    fn models(&self) -> Vec<String> {
        let f = self.dir.path().join("all-args.txt");
        let s = std::fs::read_to_string(&f).unwrap_or_default();
        let _ = std::fs::remove_file(f);
        let args: Vec<&str> = s.lines().collect();
        args.windows(2).filter(|w| w[0] == "--model").map(|w| w[1].to_string()).collect()
    }
}

#[test]
fn grouping_runs_on_haiku_and_planning_on_the_backlog_model() {
    let b = board_with(|_| {});
    let a = b.issue("Crash on save");
    let c = b.issue("Typo in footer");
    b.models();
    triage::ensure_grouped(&b.app);
    assert_eq!(b.models(), ["haiku"], "grouping");
    b.post("/backlog/plan", json!({"ids": [a, c]}));
    assert_eq!(b.models(), ["sonnet"], "wave planning");
}

#[test]
fn an_empty_group_model_falls_back_to_the_backlog_model() {
    let b = board_with(|c| {
        c.backlog.model = "opus".into();
        c.backlog.group_model = "".into();
    });
    b.issue("Crash on save");
    b.models();
    triage::ensure_grouped(&b.app);
    assert_eq!(b.models(), ["opus"]);
}
