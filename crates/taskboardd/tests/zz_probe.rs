//! The start-word row suite from #157 (1,048 rows): every row must stay green.
//! Rows live in `tests/startword_rows/`. Point `PROBE_CASES` / `PROBE_GOALS` at another
//! file to probe it; `<file>.out` gets `ok`/`BAD` per row.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::{board, midna, reports, startword};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[]).unwrap();
    Board { app, _dir: dir }
}

impl Board {
    fn report(&self, event: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": ""});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap()
    }
    fn typed(&self, prompt: &str) {
        self.report("hook.prompt", startword::hook_prompt(prompt));
    }
    fn replied(&self, message: &str) {
        let start: String = message.chars().take(2000).collect();
        let end: String = message.chars().rev().take(startword::REPLY_END_KEEP).collect::<Vec<_>>().into_iter().rev().collect();
        self.report("hook.stop", json!({"last_message": start, "last_message_end": end}));
    }
    fn new_task(&self) -> i64 {
        let v = self.report("tb.new_task", json!({"title": "Add login"}));
        v["created"][0].as_str().unwrap().trim_start_matches('T').parse().unwrap()
    }
    fn elsewhere(&self) -> i64 {
        api::dispatch(&self.app, "POST", "/tasks", &Query::new(), &json!({"title": "Chips", "project": "webapp"})).unwrap()["id"].as_i64().unwrap()
    }
    fn tb_start(&self, id: i64) -> Result<Value, (u16, String)> {
        api::dispatch(&self.app, "POST", &format!("/tasks/T{id}/start"), &Query::new(), &json!({"mode": "queue", "via_session": "s1"}))
            .map_err(|e| (e.status, e.message))
    }
    fn goal(&self) -> i64 {
        api::dispatch(&self.app, "POST", "/goals", &Query::new(), &json!({"name": "Settings", "project": "webapp"})).unwrap()["id"].as_i64().unwrap()
    }
    fn planned(&self, g: i64) -> i64 {
        let id = self.elsewhere();
        board::update_task(&self.app, id, vec![("status", json!("planned")), ("goal_id", json!(g))]).unwrap();
        id
    }
    fn goal_new(&self) -> i64 {
        let v = self.report("tb.goal", json!({"name": "Dark mode", "project": "webapp", "tasks": ["Colors::pick them", "Toggle::add it"]}));
        v["goal"].as_str().unwrap().trim_start_matches('G').parse().unwrap()
    }
    fn run(&self, g: i64, via: &str, app: bool) -> Result<Value, (u16, String)> {
        let q: Query = if app { [(api::FROM.to_string(), "app".to_string())].into_iter().collect() } else { Query::new() };
        let body = if via.is_empty() { json!({}) } else { json!({"via_session": via}) };
        api::dispatch(&self.app, "POST", &format!("/goals/G{g}/run"), &q, &body).map_err(|e| (e.status, e.message))
    }
}

fn log_paste(n: usize) -> String {
    let line = "2026-10-09 12:01:02 INFO worker 3 finished the batch in 41ms\n";
    let s = line.repeat(n / line.len() + 1);
    s.trim_end_matches('\n').to_string()
}

fn expand(s: &str) -> String {
    let typed = {
        let head = "start T8. ";
        let body = "word ".repeat(2000);
        let s: String = format!("{head}{body}").chars().take(7994).collect();
        format!("{s}…")
    };
    s.replace("\\n", "\n")
        .replace("{LOG500}", &log_paste(500))
        .replace("{LOG9K}", &log_paste(9000))
        .replace("{LOG25K}", &log_paste(25000))
        .replace("{TYPED7995}", &typed)
}

/// The row file: `$var` when set, else the suite checked in under `tests/startword_rows/`.
fn rows(var: &str, file: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| format!("{}/tests/startword_rows/{file}", env!("CARGO_MANIFEST_DIR")))
}

/// Writes `<file>.out` for a probe run, and fails naming every BAD row.
fn finish(path: &str, out: &str) {
    if !path.starts_with(env!("CARGO_MANIFEST_DIR")) {
        std::fs::write(format!("{path}.out"), out).unwrap();
    }
    let bad: Vec<&str> = out.lines().filter(|l| l.starts_with("BAD")).collect();
    println!("ok={} bad={}", out.lines().count() - bad.len(), bad.len());
    assert!(bad.is_empty(), "{} rows fail:\n{}", bad.len(), bad.join("\n"));
}

/// Each row of `text` through `row`, a few at once (every row has a board of its own), in order.
fn each_row(text: &str, row: fn(&str) -> String) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).collect();
    let n = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);
    let chunk = lines.len().div_ceil(n).max(1);
    std::thread::scope(|sc| {
        let parts: Vec<_> = lines.chunks(chunk).map(|c| sc.spawn(move || c.iter().map(|l| row(l)).collect::<String>())).collect();
        parts.into_iter().map(|h| h.join().unwrap()).collect()
    })
}

#[test]
fn probe_tasks() {
    let path = rows("PROBE_CASES", "start_rows.txt");
    let text = std::fs::read_to_string(&path).unwrap();
    finish(&path, &each_row(&text, task_row));
}

fn task_row(line: &str) -> String {
    let (exp, steps) = line.split_once(" ## ").unwrap();
    let b = board();
    for _ in 0..7 {
        b.elsewhere();
    }
    assert_eq!(b.elsewhere(), 8);
    assert_eq!(b.elsewhere(), 9);
    let mut made = vec![];
    for st in steps.split(" || ") {
        if st == "+" {
            made.push(b.new_task());
        } else if let Some(r) = st.strip_prefix("agent:") {
            b.replied(&expand(r));
        } else {
            b.typed(&expand(st));
        }
    }
    let mut got = vec![];
    let mut all_ok = true;
    for e in exp.split_whitespace() {
        let (k, want) = e.split_once('=').unwrap();
        let id = match k {
            "T8" => 8,
            "T9" => 9,
            m if m.starts_with('M') => made[m[1..].parse::<usize>().unwrap() - 1],
            _ => panic!("{k}"),
        };
        let r = b.tb_start(id);
        let g = if r.is_ok() { "S" } else { "R" };
        if want != "?" && g != want {
            all_ok = false;
        }
        got.push(format!("{k}={g}"));
    }
    let shown: String = steps.chars().take(110).collect();
    format!("{}\t{}\t{}\t{}\n", if all_ok { "ok " } else { "BAD" }, exp, got.join(" "), shown)
}

fn goal_refs(t: &str, g: i64) -> String {
    regex::Regex::new(r"\bG\b").unwrap().replace_all(t, format!("G{g}").as_str()).to_string()
}

#[test]
fn probe_goals() {
    let path = rows("PROBE_GOALS", "goal_rows.txt");
    let text = std::fs::read_to_string(&path).unwrap();
    finish(&path, &each_row(&text, goal_row));
}

fn goal_row(line: &str) -> String {
    let (exp, steps) = line.split_once(" ## ").unwrap();
    let b = board();
    let mut g = b.goal();
    b.planned(g);
    let mut gs = vec![g];
    let mut via = "s1".to_string();
    let mut from_app = false;
    for st in steps.split(" || ") {
        let t = expand(&goal_refs(st, g));
        match st {
            "goal new" => {
                g = b.goal_new();
                gs.push(g);
            }
            s if s.starts_with("use:") => g = gs[s[4..].parse::<usize>().unwrap() - 1],
            "propose" => {
                b.report("tb.propose", json!({"goal": format!("G{g}"), "tasks": ["Colors::pick them"]}));
            }
            "task new" => {
                b.report("tb.new_task", json!({"title": "Toggle", "goal": format!("G{g}")}));
            }
            "clear" => {
                b.report("hook.session_start", json!({"source": "clear"}));
            }
            "plan" | "plan-nosession" => {
                api::dispatch(&b.app, "POST", &format!("/goals/G{g}/plan"), &Query::new(), &json!({"mode": "edit"})).unwrap();
                if st == "plan" {
                    b.app.db.x("UPDATE jobs SET target = json_set(target, '$.session', 's1') WHERE purpose = 'plan'", taskboardd::p![]).unwrap();
                }
            }
            "nosession" => via = String::new(),
            "app" => from_app = true,
            s if s.starts_with("say:") => b.typed(&t[4..]),
            s if s.starts_with("agent:") => b.replied(&t[6..]),
            s => panic!("{s}"),
        }
    }
    let r = b.run(g, &via, from_app);
    let gr = if r.is_ok() { "S" } else { "R" };
    let why = r.err().map(|e| format!("{} {}", e.0, e.1.chars().take(90).collect::<String>())).unwrap_or_default();
    format!("{}\t{}\t{}\t{}\t{}\n", if exp == gr || exp == "?" { "ok " } else { "BAD" }, exp, gr, steps, why)
}
