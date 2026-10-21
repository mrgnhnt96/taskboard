//! A task's model and effort (`tb task new --model --effort`, `[models]`): kept on the task, carried by
//! planned tasks, and passed to its terminal as `--model` and `--effort`.

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, reports, runner};

struct Board {
    app: std::sync::Arc<App>,
    _dir: tempfile::TempDir,
}

fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    cfg.models.enabled = true;
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    f(&mut cfg);
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, _dir: dir }
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).map_err(|e| (e.status, e.message))
    }
    fn new_task(&self, extra: Value) -> i64 {
        let mut body = json!({"title": "Fix it", "detail": "Do the thing.", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body).unwrap()["id"].as_i64().unwrap()
    }
    /// The Claude flags the task's new terminal starts with.
    fn start_flags(&self, id: i64, flags: Option<&str>) -> Option<String> {
        let t = board::get_task(&self.app, id).unwrap();
        let jid = runner::start_task(&self.app, &t, "new", None, Some("go".into()), flags.map(str::to_string), None).unwrap();
        let job = self.app.db.q("SELECT * FROM jobs WHERE id = ?", taskboardd::p![jid]).unwrap().remove(0);
        board::job_args(&job).s("flags").map(str::to_string)
    }
}

#[test]
fn a_task_s_terminal_starts_with_its_model() {
    let b = board_with(|_| {});
    let small = b.new_task(json!({"model": "small"}));
    assert_eq!(board::get_task(&b.app, small).unwrap().s("model"), Some("small"));
    assert_eq!(b.start_flags(small, None).as_deref(), Some("--model sonnet --effort medium"));

    let plain = b.new_task(json!({}));
    assert_eq!(b.start_flags(plain, None).as_deref(), Some("--model opus --effort high"), "no model: [models] default, large");
    assert_eq!(b.start_flags(plain, Some("--resume c1")).as_deref(), Some("--resume c1 --model opus --effort high"));
    assert_eq!(b.start_flags(plain, Some("--model haiku")).as_deref(), Some("--model haiku --effort high"), "flags that name one keep it");
    assert_eq!(b.start_flags(plain, Some("--effort low")).as_deref(), Some("--effort low --model opus"));

    let named = b.new_task(json!({"model": "claude-haiku-5-5"}));
    assert_eq!(b.start_flags(named, None).as_deref(), Some("--model claude-haiku-5-5"), "a model name: Claude's own effort");

    let own = b.new_task(json!({"model": "small", "effort": "max"}));
    assert_eq!(board::get_task(&b.app, own).unwrap().s("effort"), Some("max"));
    assert_eq!(b.start_flags(own, None).as_deref(), Some("--model sonnet --effort max"));
}

#[test]
fn tier_efforts_come_from_config() {
    let b = board_with(|c| {
        c.models.small_effort = "low".into();
        c.models.default_effort = "xhigh".into();
    });
    let small = b.new_task(json!({"model": "small"}));
    assert_eq!(b.start_flags(small, None).as_deref(), Some("--model sonnet --effort low"));
    let named = b.new_task(json!({"model": "claude-haiku-5-5"}));
    assert_eq!(b.start_flags(named, None).as_deref(), Some("--model claude-haiku-5-5 --effort xhigh"));
}

#[test]
fn with_models_off_no_model_is_passed() {
    let b = board_with(|c| c.models.enabled = false);
    let id = b.new_task(json!({"model": "small", "effort": "high"}));
    assert_eq!(b.start_flags(id, None), None);
    assert_eq!(b.start_flags(id, Some("--resume c1")).as_deref(), Some("--resume c1"));
}

#[test]
fn task_set_changes_and_clears_the_model() {
    let b = board_with(|_| {});
    let id = b.new_task(json!({}));
    b.post(&format!("/tasks/T{id}"), json!({"model": "small"})).unwrap();
    assert_eq!(board::get_task(&b.app, id).unwrap().s("model"), Some("small"));
    let card = board::task_card(&b.app, &board::get_task(&b.app, id).unwrap()).unwrap();
    assert_eq!((card["model"].as_str(), card["model_runs"].as_str()), (Some("small"), Some("sonnet")));
    b.post(&format!("/tasks/T{id}"), json!({"model": "default"})).unwrap();
    assert_eq!(board::get_task(&b.app, id).unwrap().s("model"), None);
    let (status, msg) = b.post(&format!("/tasks/T{id}"), json!({"model": "son net; rm"})).unwrap_err();
    assert_eq!(status, 400, "{msg}");
}

#[test]
fn task_set_changes_and_clears_the_effort() {
    let b = board_with(|_| {});
    let id = b.new_task(json!({"model": "small"}));
    let card = board::task_card(&b.app, &board::get_task(&b.app, id).unwrap()).unwrap();
    assert_eq!((&card["effort"], card["effort_runs"].as_str()), (&Value::Null, Some("medium")));
    b.post(&format!("/tasks/T{id}"), json!({"effort": "xhigh"})).unwrap();
    let card = board::task_card(&b.app, &board::get_task(&b.app, id).unwrap()).unwrap();
    assert_eq!((card["effort"].as_str(), card["effort_runs"].as_str()), (Some("xhigh"), Some("xhigh")));
    b.post(&format!("/tasks/T{id}"), json!({"effort": "default"})).unwrap();
    assert_eq!(board::get_task(&b.app, id).unwrap().s("effort"), None);
    let (status, msg) = b.post(&format!("/tasks/T{id}"), json!({"effort": "turbo"})).unwrap_err();
    assert_eq!(status, 400, "{msg}");
    let (status, msg) = b.post("/tasks", json!({"title": "Bad", "project": "webapp", "effort": "turbo"})).unwrap_err();
    assert_eq!(status, 400, "{msg}");
}

#[test]
fn planned_tasks_carry_their_model() {
    let b = board_with(|_| {});
    let g = b.post("/goals", json!({"name": "Cut spend", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
    let r = reports::handle(
        &b.app,
        json!({"event": "tb.propose", "session": "s1", "goal": format!("G{g}"),
               "tasks": ["One::do it::model=small::effort=low", "Two::do it too::1::::src/a.rs::model=large", "Three::no model",
                         {"title": "Four", "model": "small", "effort": "max"}]}),
        false,
    )
    .unwrap();
    let ids: Vec<i64> = r["created"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()[1..].parse().unwrap()).collect();
    let tasks: Vec<_> = ids.iter().map(|id| board::get_task(&b.app, *id).unwrap()).collect();
    let models: Vec<Option<&str>> = tasks.iter().map(|t| t.s("model")).collect();
    assert_eq!(models, vec![Some("small"), Some("large"), None, Some("small")]);
    let efforts: Vec<Option<&str>> = tasks.iter().map(|t| t.s("effort")).collect();
    assert_eq!(efforts, vec![Some("low"), None, None, Some("max")]);
    assert_eq!(tasks[0].st("detail"), "do it");
    assert_eq!((tasks[1].st("detail"), tasks[1].i("wave")), ("do it too".to_string(), Some(1)));
    assert!(serde_json::Value::Object(board::task_context(&tasks[1])).to_string().contains("src/a.rs"), "the files part still reads");
}
