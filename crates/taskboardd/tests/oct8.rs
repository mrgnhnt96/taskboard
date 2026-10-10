//! The Python board's changes of 2026-10-08 (`docs/parity/oct8-changes.md`): shared tasks, task origin,
//! `tb task new --here` and its Stop backstop, review alerts that stay, and the re-review and merge-retry stages.

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, dispatch, midna, p, prflow, reports};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn new_board() -> Board {
    board_with(|_| {})
}

fn board_with(f: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    f(&mut cfg);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db
        .set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string()))
        .unwrap();
    Board { app, dir }
}

impl Board {
    fn get(&self, path: &str) -> Value {
        let (p, q) = path.split_once('?').unwrap_or((path, ""));
        let query: Query = q.split('&').filter(|x| !x.is_empty()).filter_map(|kv| kv.split_once('=')).map(|(k, v)| (k.to_string(), v.to_string())).collect();
        api::dispatch(&self.app, "GET", p, &query, &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn post_err(&self, path: &str, body: Value) -> (u16, String) {
        let e = api::dispatch(&self.app, "POST", path, &Query::new(), &body).expect_err("should fail");
        (e.status, e.message)
    }
    fn report(&self, event: &str, session: &str, extra: Value) -> Value {
        self.try_report(event, session, extra).unwrap_or_else(|e| panic!("{event}: {e}"))
    }
    fn try_report(&self, event: &str, session: &str, extra: Value) -> Result<Value, String> {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).map_err(|e| e.message)
    }
    fn repo(&self) -> String {
        self.dir.path().join("webapp").to_string_lossy().to_string()
    }
    fn add_session(&self, sid: &str) {
        midna::sync(
            &self.app,
            &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": self.repo(), "status": {"state": "working"}})],
            &[],
        )
        .unwrap();
    }
    fn goal(&self, name: &str) -> i64 {
        self.post("/goals", json!({"name": name, "project": "webapp"}))["id"].as_i64().unwrap()
    }
    fn task(&self, title: &str, extra: Value) -> i64 {
        let mut body = json!({"title": title, "detail": "Do it.", "project": "webapp"});
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        self.post("/tasks", body)["id"].as_i64().unwrap()
    }
}

fn refs(v: &Value) -> Vec<String> {
    v.as_array().unwrap().iter().map(|x| x["ref"].as_str().unwrap().to_string()).collect()
}

#[test]
fn a_shared_task_counts_toward_every_goal_it_finishes() {
    let b = new_board();
    let (home, other) = (b.goal("Login"), b.goal("Signup"));
    let t = b.task("Add the auth client", json!({"goal_id": home, "also": [format!("G{other}")]}));

    let card = b.get(&format!("tasks/T{t}"));
    assert_eq!(refs(&card["also"]), vec![format!("G{other}")]);
    assert_eq!(card["goal"]["id"], home);

    let g = b.get(&format!("goals/G{other}"));
    assert_eq!(g["tasks"].as_array().unwrap().len(), 0, "its home goal lists it, not this one");
    assert_eq!(refs(&g["shared"]), vec![format!("T{t}")]);
    assert_eq!(g["total"], 1, "it counts toward this goal");

    let filtered = b.get(&format!("tasks?goal=G{other}"));
    assert_eq!(refs(&filtered["tasks"]), vec![format!("T{t}")], "a goal filter finds it through --also");

    let handoff = board::get_task(&b.app, t).map(|_| taskboardd::handoff::build(&b.app, t).unwrap()).unwrap();
    assert!(handoff.contains("also finishes another goal"), "{handoff}");
    assert!(handoff.contains(&format!("G{other} “Signup”")), "{handoff}");

    b.post(&format!("tasks/T{t}"), json!({"not_also": [format!("G{other}")]}));
    assert_eq!(b.get(&format!("goals/G{other}"))["total"], 0);
}

#[test]
fn also_is_checked() {
    let b = new_board();
    let home = b.goal("Login");
    let elsewhere = b.post("/goals", json!({"name": "API", "project": "api"}))["id"].as_i64().unwrap();
    let (code, msg) = b.post_err("/tasks", json!({"title": "X", "project": "webapp", "goal_id": home, "also": [format!("G{elsewhere}")]}));
    assert_eq!(code, 409);
    assert!(msg.contains("can't finish it"), "{msg}");

    let loose = b.task("Loose", json!({}));
    let (code, msg) = b.post_err(&format!("tasks/T{loose}"), json!({"also": [format!("G{home}")]}));
    assert_eq!(code, 409);
    assert!(msg.contains("no goal of its own"), "{msg}");

    b.add_session("s1");
    let out = b.try_report("tb.new_task", "s1", json!({"title": "No home", "also": [format!("G{home}")]}));
    assert!(out.unwrap_err().contains("Only a task in a goal"));
}

#[test]
fn deleting_the_home_goal_moves_a_shared_task_to_its_newest_other_goal() {
    let b = new_board();
    let (home, g2, g3) = (b.goal("Home"), b.goal("Two"), b.goal("Three"));
    let t = b.task("Shared", json!({"goal_id": home, "also": [format!("G{g2}")]}));
    b.post(&format!("tasks/T{t}"), json!({"also": [format!("G{g3}")]}));
    let gone = b.task("Only home", json!({"goal_id": home}));

    let r = b.post(&format!("goals/G{home}/delete"), json!({"tasks": true}));
    assert_eq!(r["moved"], json!([format!("T{t}")]));
    assert_eq!(r["tasks"], 1);
    let task = board::get_task(&b.app, t).unwrap();
    assert_eq!(task.i("goal_id"), Some(g3), "the newest goal that references it");
    assert!(board::find_task(&b.app, Some(gone)).unwrap().is_none());
    let g3d = b.get(&format!("goals/G{g3}"));
    assert_eq!(refs(&g3d["tasks"]), vec![format!("T{t}")]);
    assert_eq!(refs(&g3d["shared"]), Vec::<String>::new(), "it runs here now, so it isn't shared into here");
    assert_eq!(refs(&b.get(&format!("goals/G{g2}"))["shared"]), vec![format!("T{t}")]);
}

#[test]
fn every_task_says_where_it_came_from() {
    let b = new_board();
    let g = b.goal("Login");
    let t = b.task("By hand", json!({}));
    assert_eq!(b.get(&format!("tasks/T{t}"))["origin"], json!({"from": "Added on the board", "by": "You"}));

    b.add_session("s1");
    let r = b.report("tb.propose", "s1", json!({"goal": format!("G{g}"), "tasks": ["Plan one::do it"]}));
    let planned = r["created"][0].as_str().unwrap().to_string();
    assert_eq!(b.get(&format!("tasks/{planned}"))["origin"], json!({"from": format!("Planned in G{g}"), "by": "Term s1"}));

    let i = b.post("/backlog", json!({"title": "Flaky test", "project": "webapp", "kind": "bug"}))["id"].as_i64().unwrap();
    let made = b.post(&format!("backlog/B{i}/promote"), json!({}));
    let tid = made["task"]["id"].as_i64().unwrap();
    assert_eq!(b.get(&format!("tasks/T{tid}"))["origin"], json!({"from": format!("Backlog B{i}: Flaky test"), "by": "You"}));

    let linked = b.task("Linked", json!({"origin": {"from": "A report", "by": "Sam", "url": "https://example.com/r/1"}}));
    assert_eq!(b.get(&format!("tasks/T{linked}"))["origin"]["url"], "https://example.com/r/1");
    let bad = b.task("Not a link", json!({"origin": {"from": "A report", "url": "file:///etc/passwd"}}));
    assert!(b.get(&format!("tasks/T{bad}"))["origin"].get("url").is_none());
}

#[test]
fn here_makes_a_standalone_task_this_terminal_is_on() {
    let b = new_board();
    b.add_session("s1");
    let r = b.report("tb.new_task", "s1", json!({"title": "Fix the header", "detail": "The owner asked for it.", "here": true}));
    let tr = r["created"][0].as_str().unwrap().to_string();
    assert_eq!(r["status"], "working");
    assert!(r["context"].as_str().unwrap().contains(&format!("you're on {tr} now")));
    let t = b.get(&format!("tasks/{tr}"));
    assert_eq!(t["status"], "working");
    assert_eq!(t["session_id"], "s1");
    assert!(t["goal"].is_null());
    assert_eq!(t["origin"]["from"], format!("Code changed in Term s1 while {} worked there", b.app.cfg.owner));

    let again = b.try_report("tb.new_task", "s1", json!({"title": "Another", "here": true}));
    assert!(again.unwrap_err().contains("already on"), "a terminal on a task tracks its work there");
    b.add_session("s2");
    let g = b.goal("Login");
    let with_goal = b.try_report("tb.new_task", "s2", json!({"title": "X", "here": true, "goal": format!("G{g}")}));
    assert!(with_goal.unwrap_err().contains("leave out --goal"));
}

#[test]
fn a_turn_that_changed_code_with_no_task_is_asked_to_track_it() {
    let b = new_board();
    b.add_session("s1");
    b.report("hook.prompt", "s1", json!({"prompt": "Make the header blue"}));
    let s = board::get_session(&b.app, Some("s1")).unwrap().unwrap();
    let file = taskboardd::transcript::transcript_file(&b.app.cfg.claude_projects, &s.st("project_path"), &s.st("claude_session_id"));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let inside = format!("{}/src/header.rs", b.repo());
    let lines = [
        json!({"type": "user", "message": {"content": "Make the header blue"}, "timestamp": "2026-10-08T10:00:00Z"}),
        json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Edit", "input": {"file_path": inside}}]}}),
        json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Write", "input": {"file_path": "/tmp/scratch.txt"}}]}}),
    ];
    std::fs::write(&file, lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")).unwrap();

    let out = b.report("hook.stop", "s1", json!({"last_message": "Done."}));
    let block = out["block"].as_str().expect("the stop is blocked");
    assert!(block.contains("header.rs") && !block.contains("scratch"), "{block}");
    assert!(block.contains("--here"), "{block}");

    let again = b.report("hook.stop", "s1", json!({"last_message": "Done.", "stop_hook_active": true}));
    assert!(again.get("block").is_none(), "it asks once per turn");
}

fn tree(b: &Board, head: &str, files: &[(&str, &str)]) -> Value {
    tree_with(b, head, files, &[("aaa", "commit (initial): start")])
}

/// A tree stamp whose reflog is `reflog`, newest first, as (sha, how).
fn tree_with(b: &Board, head: &str, files: &[(&str, &str)], reflog: &[(&str, &str)]) -> Value {
    let files: serde_json::Map<String, Value> = files.iter().map(|(f, s)| (format!("{}/{f}", b.repo()), json!(s))).collect();
    let n = reflog.len() as i64;
    let reflog: Vec<Value> = reflog.iter().enumerate().map(|(i, (sha, how))| json!({"sha": sha, "at": 1_800_000_000 + n - i as i64, "how": how})).collect();
    json!({"root": b.repo(), "head": head, "files": files, "reflog": reflog})
}

fn bash(command: &str) -> Value {
    json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Bash", "input": {"command": command}}]}})
}

/// Writes the session's transcript as one turn: `prompt`, then `calls`.
fn turn(b: &Board, sid: &str, prompt: &str, calls: &[Value]) {
    let s = board::get_session(&b.app, Some(sid)).unwrap().unwrap();
    let file = taskboardd::transcript::transcript_file(&b.app.cfg.claude_projects, &s.st("project_path"), &s.st("claude_session_id"));
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mut lines = vec![json!({"type": "user", "message": {"content": prompt}, "timestamp": "2026-10-08T10:00:00Z"})];
    lines.extend(calls.iter().cloned());
    std::fs::write(&file, lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")).unwrap();
}

/// A prompt stamped `before`, a turn that ran `calls`, and the Stop stamped `after`: the Stop's block.
fn stop_after(b: &Board, calls: &[Value], before: Value, after: Value) -> Option<String> {
    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tree": before}));
    turn(b, "s1", "Go", calls);
    let out = b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": b.repo(), "tree": after}));
    out["block"].as_str().map(|s| s.to_string())
}

#[test]
fn a_turn_that_changed_code_through_bash_is_asked_to_track_it() {
    let b = new_board();
    b.add_session("s1");
    b.report("hook.prompt", "s1", json!({"prompt": "This is clipping", "tree": tree(&b, "aaa", &[("src/old.rs", "h1:10")])}));
    turn(&b, "s1", "This is clipping", &[bash("python3 fix.py")]);
    let after = tree(&b, "aaa", &[("src/old.rs", "h1:10"), ("src/charts.rs", "h2:40")]);
    let out = b.report("hook.stop", "s1", json!({"last_message": "Fixed.", "tree": after}));
    let block = out["block"].as_str().expect("an edit made by a script still blocks the stop");
    assert!(block.contains("charts.rs") && !block.contains("old.rs"), "{block}");

    b.report("hook.prompt", "s1", json!({"prompt": "What does this do?", "tree": tree(&b, "aaa", &[("src/charts.rs", "h2:40")])}));
    turn(&b, "s1", "What does this do?", &[bash("cat src/charts.rs")]);
    let read_only = b.report("hook.stop", "s1", json!({"last_message": "It draws bars.", "tree": tree(&b, "aaa", &[("src/charts.rs", "h2:40")])}));
    assert!(read_only.get("block").is_none(), "a turn that changed nothing isn't asked: {read_only}");
}

#[test]
fn a_turn_that_only_committed_is_asked_to_track_it() {
    let b = new_board();
    b.add_session("s1");
    let after = tree_with(&b, "bbb", &[], &[("bbb", "commit: fix(app): rows scroll"), ("aaa", "commit (initial): start")]);
    let block = stop_after(&b, &[bash("git commit -am 'fix(app): rows scroll'")], tree(&b, "aaa", &[]), after).expect("a commit with no task blocks the stop");
    assert!(block.contains("a commit: fix(app): rows scroll"), "{block}");
}

#[test]
fn a_turn_that_ran_no_shell_or_subagent_isnt_blamed_for_the_tree() {
    let b = new_board();
    b.add_session("s1");
    let read = json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Read", "input": {"file_path": format!("{}/a.rs", b.repo())}}]}});
    let owner_saved = tree(&b, "aaa", &[("a.rs", "h9:5")]);
    assert_eq!(stop_after(&b, &[read], tree(&b, "aaa", &[]), owner_saved.clone()), None, "the owner's save in their editor isn't this turn's");

    let agent = json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Agent", "input": {"prompt": "fix it"}}]}});
    let block = stop_after(&b, &[agent], tree(&b, "aaa", &[]), owner_saved).expect("a subagent's edits aren't in the transcript");
    assert!(block.contains("a.rs"), "{block}");
}

#[test]
fn junk_files_and_unchanged_content_dont_block_the_stop() {
    let b = new_board();
    b.add_session("s1");
    let before = tree(&b, "aaa", &[("a.rs", "h1:5")]);
    let after = tree(&b, "aaa", &[("a.rs", "h1:5"), (".DS_Store", "h7:6148"), ("src/._a.rs", "h8:4"), ("src/a.rs.swp", "h9:12")]);
    assert_eq!(stop_after(&b, &[bash("ls")], before, after), None, "Finder's and editors' files, and a touched file with the same bytes");
}

#[test]
fn a_head_moved_by_pull_or_checkout_isnt_a_commit() {
    let b = new_board();
    b.add_session("s1");
    let start = ("aaa", "commit (initial): start");
    for how in ["pull: Fast-forward", "checkout: moving from main to other", "reset: moving to HEAD~1", "rebase (finish): returning to refs/heads/main", "merge feature: Fast-forward"] {
        let after = tree_with(&b, "ccc", &[], &[("ccc", how), start]);
        assert_eq!(stop_after(&b, &[bash("git pull")], tree(&b, "aaa", &[]), after), None, "{how}");
    }
    let after = tree_with(&b, "ccc", &[], &[("ccc", "pull: Fast-forward"), ("bbb", "commit (amend): mine"), start]);
    let block = stop_after(&b, &[bash("git commit --amend")], tree(&b, "aaa", &[]), after).expect("a commit made before the pull still counts");
    assert!(block.contains("a commit: mine"), "{block}");
    let old_hook = json!({"root": b.repo(), "head": "ddd", "files": {}});
    assert_eq!(stop_after(&b, &[bash("git pull")], json!({"root": b.repo(), "head": "aaa", "files": {}}), old_hook), None, "no reflog, no commit");
}

#[test]
fn files_outside_where_the_turn_worked_dont_block_the_stop() {
    let b = new_board();
    b.add_session("s1");
    let app = format!("{}/app", b.repo());
    std::fs::create_dir_all(&app).unwrap();
    let before = tree(&b, "aaa", &[]);
    let after = tree(&b, "aaa", &[("lib/other.rs", "h1:5")]);
    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tree": before.clone()}));
    turn(&b, "s1", "Go", &[bash("cargo fmt")]);
    let out = b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": app, "tree": after.clone()}));
    assert!(out.get("block").is_none(), "another terminal's file outside this one's folder: {out}");

    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tree": before}));
    turn(&b, "s1", "Go", &[bash("cd ../lib && sed -i '' s/a/b/ other.rs")]);
    let out = b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": app, "tree": after}));
    assert!(out["block"].as_str().is_some_and(|x| x.contains("other.rs")), "a folder the shell moved into is the turn's: {out}");
}

#[test]
fn a_second_stop_with_no_prompt_doesnt_reuse_the_stamp() {
    let b = new_board();
    b.add_session("s1");
    assert_eq!(stop_after(&b, &[bash("ls")], tree(&b, "aaa", &[]), tree(&b, "aaa", &[])), None);
    let out = b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": b.repo(), "tree": tree(&b, "aaa", &[("a.rs", "h1:5")])}));
    assert!(out.get("block").is_none(), "the owner's edit after the turn ended: {out}");
}

#[test]
fn the_jira_desk_isnt_asked_to_track_its_turns() {
    let b = new_board();
    b.add_session("s1");
    b.app.db.x("INSERT INTO jobs(kind, state, purpose, target) VALUES ('agent', 'done', ?, ?)", p![taskboardd::jira_desk::PURPOSE, json!({"session": "s1"}).to_string()]).unwrap();
    assert_eq!(stop_after(&b, &[bash("python3 fix.py")], tree(&b, "aaa", &[]), tree(&b, "aaa", &[("a.rs", "h1:5")])), None);
}

/// One Bash or subagent call as the hook reports it: its stamp as it starts, and as it ends.
fn call(b: &Board, id: &str, start: Value, end: Option<Value>) {
    b.report("hook.tool_start", "s1", json!({"tool": "Bash", "tool_use_id": id, "tree": start}));
    if let Some(end) = end {
        b.report("hook.tool_end", "s1", json!({"tool": "Bash", "tool_use_id": id, "tree": end}));
    }
}

#[test]
fn only_changes_made_while_the_agents_calls_ran_block_the_stop() {
    let b = new_board();
    b.add_session("s1");
    let app = format!("{}/app", b.repo());
    std::fs::create_dir_all(&app).unwrap();
    let clean = tree(&b, "aaa", &[]);
    let saved = tree(&b, "aaa", &[("src/a.rs", "h1:5")]);
    let stop = |cwd: &str, after: Value| b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": cwd, "tree": after}))["block"].as_str().map(|s| s.to_string());

    // `ls`, then the owner saves in their editor (or another terminal edits) before the Stop.
    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tool_windows": true, "tree": clean.clone()}));
    turn(&b, "s1", "Go", &[bash("ls")]);
    call(&b, "t1", clean.clone(), Some(clean.clone()));
    assert_eq!(stop(&b.repo(), saved.clone()), None, "a save between the agent's calls isn't the agent's");

    // The same save, and a generated file, while a later `git status` ran: still between calls when it
    // landed before the call started.
    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tool_windows": true, "tree": clean.clone()}));
    turn(&b, "s1", "Go", &[bash("ls"), bash("git status")]);
    call(&b, "t1", clean.clone(), Some(clean.clone()));
    let generated = tree(&b, "aaa", &[("src/a.rs", "h1:5"), ("coverage/lcov.info", "h2:9")]);
    call(&b, "t2", generated.clone(), Some(generated.clone()));
    assert_eq!(stop(&b.repo(), generated), None);

    // From `app/`, a script edits `../lib/x.rs` (sed, an absolute path, python): no folder guess misses it.
    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tool_windows": true, "tree": clean.clone()}));
    turn(&b, "s1", "Go", &[bash("cd app"), bash("sed -i '' s/a/b/ ../lib/x.rs")]);
    call(&b, "t1", clean.clone(), Some(clean.clone()));
    let edited = tree(&b, "aaa", &[("lib/x.rs", "h3:7")]);
    call(&b, "t2", clean.clone(), Some(edited.clone()));
    let block = stop(&app, tree(&b, "aaa", &[("lib/x.rs", "h3:7"), ("src/a.rs", "h1:5")])).expect("the agent's own edit outside its folder");
    assert!(block.contains("x.rs") && !block.contains("a.rs"), "{block}");
}

#[test]
fn a_commit_counts_only_when_one_of_the_agents_calls_made_it() {
    let b = new_board();
    b.add_session("s1");
    let start = ("aaa", "commit (initial): start");
    let clean = tree(&b, "aaa", &[]);
    let theirs = tree_with(&b, "bbb", &[], &[("bbb", "commit: theirs"), start]);
    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tool_windows": true, "tree": clean.clone()}));
    turn(&b, "s1", "Go", &[bash("ls")]);
    call(&b, "t1", clean.clone(), Some(clean));
    let out = b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": b.repo(), "tree": theirs.clone()}));
    assert!(out.get("block").is_none(), "another terminal's commit in the same checkout: {out}");

    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tool_windows": true, "tree": theirs.clone()}));
    turn(&b, "s1", "Go", &[bash("git commit -am mine")]);
    let mine = tree_with(&b, "ccc", &[], &[("ccc", "commit: mine"), ("bbb", "commit: theirs"), start]);
    call(&b, "t1", theirs, Some(mine.clone()));
    let out = b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": b.repo(), "tree": mine}));
    assert!(out["block"].as_str().is_some_and(|x| x.contains("a commit: mine")), "{out}");
}

/// A tool call the transcript shows, by its id.
fn tool_use(id: &str, tool: &str, input: Value) -> Value {
    json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": id, "name": tool, "input": input}]}})
}

/// The result the transcript shows for call `id`: an error is the owner's refusal.
fn tool_result(id: &str, error: bool) -> Value {
    let content = if error { "The user doesn't want to proceed with this tool use. The tool use was rejected." } else { "x" };
    json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": id, "is_error": error, "content": content}]}})
}

/// An error result for call `id` with this text.
fn tool_error(id: &str, content: &str) -> Value {
    json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": id, "is_error": true, "content": content}]}})
}

/// A prompt from hooks that stamp each call, which have already closed one call's window (so the board
/// knows they send `ToolEnd`).
fn prompt_with_windows(b: &Board, before: &Value) {
    b.report("hook.tool_end", "s1", json!({"tool": "Bash", "tool_use_id": "t0", "tree": before}));
    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tool_windows": true, "tree": before}));
}

#[test]
fn a_call_whose_end_was_lost_or_runs_in_the_background_counts_to_the_stop() {
    let b = new_board();
    b.add_session("s1");
    let clean = tree(&b, "aaa", &[]);
    let changed = tree(&b, "aaa", &[("a.rs", "h1:5")]);
    let stop = || b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": b.repo(), "tree": changed.clone()}))["block"].as_str().map(|s| s.to_string());

    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "python3 fix.py"})), tool_result("t1", false)]);
    call(&b, "t1", clean.clone(), None);
    assert!(stop().is_some_and(|x| x.contains("a.rs")), "a call that ran, whose end the hook didn't send");

    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "./watch.sh", "run_in_background": true})), tool_result("t1", false)]);
    b.report("hook.tool_start", "s1", json!({"tool": "Bash", "tool_use_id": "t1", "background": true, "tree": clean.clone()}));
    b.report("hook.tool_end", "s1", json!({"tool": "Bash", "tool_use_id": "t1", "tree": clean.clone()}));
    assert!(stop().is_some_and(|x| x.contains("a.rs")), "a background command goes on past its end");

    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[bash("ls")]);
    b.report("hook.tool_end", "s1", json!({"tool": "Bash", "tool_use_id": "t9", "tree": changed.clone()}));
    assert_eq!(stop(), None, "an end with no start this turn opens nothing");
}

#[test]
fn a_call_refused_or_never_answered_doesnt_count_the_owners_edits() {
    let b = new_board();
    b.add_session("s1");
    let clean = tree(&b, "aaa", &[]);
    let saved = tree(&b, "aaa", &[("a.rs", "h1:5")]);
    let stop = || b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": b.repo(), "tree": saved.clone()}))["block"].as_str().map(|s| s.to_string());

    // Denied at the permission prompt, or by another PreToolUse hook: an error result, and no ToolEnd.
    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "rm -rf build"})), tool_result("t1", true)]);
    call(&b, "t1", clean.clone(), None);
    assert_eq!(stop(), None, "a refused call never ran");

    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("t1", "Agent", json!({"prompt": "fix it"}))]);
    b.report("hook.tool_start", "s1", json!({"tool": "Agent", "tool_use_id": "t1", "tree": clean.clone()}));
    assert_eq!(stop(), None, "a call with no result in the transcript never ran");

    // Blocked by another PreToolUse hook.
    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "rm -rf build"})), tool_error("t1", "PreToolUse:Bash hook error: midna: denied by the human")]);
    call(&b, "t1", clean.clone(), None);
    assert_eq!(stop(), None, "a call a hook blocked never ran");
}

#[test]
fn a_call_that_failed_after_it_started_still_counts_to_the_stop() {
    let b = new_board();
    b.add_session("s1");
    let clean = tree(&b, "aaa", &[]);
    let wrote = tree(&b, "aaa", &[("a.rs", "h1:5")]);
    let stop = || b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": b.repo(), "tree": wrote.clone()}))["block"].as_str().map(|s| s.to_string());

    for result in ["Exit code 1\nerror: could not compile", "[Tool call interrupted: the session ended before this call's result was recorded]"] {
        prompt_with_windows(&b, &clean);
        turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "python3 fix.py && cargo build"})), tool_error("t1", result)]);
        call(&b, "t1", clean.clone(), None);
        assert!(stop().is_some_and(|x| x.contains("a.rs")), "it wrote the file, then failed: {result}");
    }
}

#[test]
fn a_call_refused_in_any_wording_doesnt_count_the_owners_edits() {
    let b = new_board();
    b.add_session("s1");
    let clean = tree(&b, "aaa", &[]);
    let saved = tree(&b, "aaa", &[("a.rs", "h1:5")]);
    let stop = || b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": b.repo(), "tree": saved.clone()}))["block"].as_str().map(|s| s.to_string());

    // A warm session (it has sent ToolEnd): the Bash call never ran, and the owner saves `a.rs`.
    for refusal in [
        "Hook PreToolUse:Bash denied this tool",
        "Error: Hook PreToolUse:Bash denied this tool",
        "Permission for this action was denied by the Claude Code auto mode classifier. Reason: it deletes the build folder",
        "Permission for this action has been denied. Reason: not now",
        "Permission for this tool use was denied. The tool use was rejected",
        "sprout refuses this write: build/ is outside this node's declared work",
    ] {
        prompt_with_windows(&b, &clean);
        turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "rm -rf build"})), tool_error("t1", refusal)]);
        call(&b, "t1", clean.clone(), None);
        assert_eq!(stop(), None, "{refusal}");
    }

    // The tool's own output beside an error says it ran.
    prompt_with_windows(&b, &clean);
    let ran = json!({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "is_error": true, "content": "odd failure"}]}, "toolUseResult": {"stdout": "", "stderr": "odd failure"}});
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "python3 fix.py"})), ran]);
    call(&b, "t1", clean.clone(), None);
    assert!(stop().is_some_and(|x| x.contains("a.rs")), "it ran, then failed");
}

#[test]
fn two_identical_calls_with_no_tool_use_id_pair_with_their_own_results() {
    let b = new_board();
    b.add_session("s1");
    let clean = tree(&b, "aaa", &[]);
    let saved = tree(&b, "aaa", &[("a.rs", "h1:5")]);
    let stop = || b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": b.repo(), "tree": saved.clone()}))["block"].as_str().map(|s| s.to_string());
    let input = json!({"command": "cargo build", "description": "Build"});
    let key = taskboardd::transcript::call_key("Bash", &input);
    let denied = "Permission for this action has been denied. Reason: not now";
    let twins = |first: Value, second: Value| [tool_use("toolu_1", "Bash", input.clone()), first, tool_use("toolu_2", "Bash", input.clone()), second];

    // The first ran and closed; the same call again was refused; then the owner saves `a.rs`.
    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &twins(tool_result("toolu_1", false), tool_error("toolu_2", denied)));
    call(&b, &key, clean.clone(), Some(clean.clone()));
    call(&b, &key, clean.clone(), None);
    assert_eq!(stop(), None, "the refused twin never ran");

    // Refused first (no end), then the same call ran and its end was lost.
    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &twins(tool_error("toolu_1", denied), tool_result("toolu_2", false)));
    call(&b, &key, clean.clone(), None);
    call(&b, &key, clean.clone(), None);
    assert!(stop().is_some_and(|x| x.contains("a.rs")), "the twin that ran");

    // Refused first, then the same call ran and closed before the save.
    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &twins(tool_error("toolu_1", denied), tool_result("toolu_2", false)));
    call(&b, &key, clean.clone(), None);
    call(&b, &key, clean.clone(), Some(clean.clone()));
    assert_eq!(stop(), None, "the one that ran closed");

    // Both ran, the second's end lost.
    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &twins(tool_result("toolu_1", false), tool_result("toolu_2", false)));
    call(&b, &key, clean.clone(), Some(clean.clone()));
    call(&b, &key, clean.clone(), None);
    assert!(stop().is_some_and(|x| x.contains("a.rs")), "the second ran to the Stop");
}

#[test]
fn a_file_a_command_runs_or_rewrites_from_widens_the_turn_to_its_folder() {
    let b = new_board();
    b.add_session("s1");
    let app = format!("{}/app", b.repo());
    std::fs::create_dir_all(format!("{}/tools", b.repo())).unwrap();
    std::fs::create_dir_all(format!("{}/lib", b.repo())).unwrap();
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(format!("{}/Cargo.toml", b.repo()), "").unwrap();
    std::fs::write(format!("{}/tools/gen.py", b.repo()), "").unwrap();
    let clean = tree(&b, "aaa", &[]);
    let stop = |after: &Value| b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": app, "tree": after}))["block"].as_str().map(|s| s.to_string());

    for (command, wrote) in [("cargo fmt --manifest-path ../Cargo.toml", "lib/x.rs"), ("python3 ../tools/gen.py", "tools/out.rs")] {
        let after = tree(&b, "aaa", &[(wrote, "h1:5")]);
        prompt_with_windows(&b, &clean);
        turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": command})), tool_result("t1", false)]);
        call(&b, "t1", clean.clone(), Some(after.clone()));
        assert!(stop(&after).is_some_and(|x| x.contains(wrote.rsplit('/').next().unwrap())), "{command}");
    }

    // Reading the same files keeps the turn where it was.
    for command in ["cat ../tools/gen.py", "head ../Cargo.toml"] {
        let after = tree(&b, "aaa", &[("lib/x.rs", "h1:5")]);
        prompt_with_windows(&b, &clean);
        turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": command})), tool_result("t1", false)]);
        call(&b, "t1", clean.clone(), Some(after.clone()));
        assert_eq!(stop(&after), None, "{command}");
    }
}

#[test]
fn a_call_with_a_missed_stamp_or_a_lost_start_still_counts() {
    let b = new_board();
    b.add_session("s1");
    let clean = tree(&b, "aaa", &[]);
    let wrote = tree(&b, "aaa", &[("a.rs", "h1:5")]);
    let stop = || b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": b.repo(), "tree": wrote.clone()}))["block"].as_str().map(|s| s.to_string());

    // Git ran past the hook's limit at the call's start (as under load): its window opens at the
    // prompt's stamp.
    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "python3 fix.py"})), tool_result("t1", false)]);
    call(&b, "t1", Value::Null, Some(wrote.clone()));
    assert!(stop().is_some_and(|x| x.contains("a.rs")), "no stamp at the start");

    // And at its end: the window stays open to the Stop.
    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "python3 fix.py"})), tool_result("t1", false)]);
    call(&b, "t1", clean.clone(), Some(Value::Null));
    assert!(stop().is_some_and(|x| x.contains("a.rs")), "no stamp at the end");

    // The call's start never reached the board.
    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "python3 fix.py"})), tool_result("t1", false)]);
    assert!(stop().is_some_and(|x| x.contains("a.rs")), "a call that ran with no window");

    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "rm -rf build"})), tool_result("t1", true)]);
    assert_eq!(stop(), None, "a refused call with no window");
}

/// A call's start from hooks whose hooks.json sends ToolEnd.
fn start_with_ends(b: &Board, tool: &str, id: &str, start: &Value) {
    b.report("hook.tool_start", "s1", json!({"tool": tool, "tool_use_id": id, "ends": true, "tree": start}));
}

#[test]
fn a_cold_sessions_first_call_goes_by_the_windows_when_its_hooks_send_tool_end() {
    let b = new_board();
    b.add_session("s1");
    let clean = tree(&b, "aaa", &[]);
    let saved = tree(&b, "aaa", &[("a.rs", "h1:5")]);
    let stop = || b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": b.repo(), "tree": saved.clone()}))["block"].as_str().map(|s| s.to_string());

    // A fresh terminal: no ToolEnd yet. Its first Bash call is denied; the owner saves `a.rs`.
    b.report("hook.session_start", "s1", json!({"source": "startup"}));
    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tool_windows": true, "tree": clean.clone()}));
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "rm -rf build"})), tool_result("t1", true)]);
    start_with_ends(&b, "Bash", "t1", &clean);
    assert_eq!(stop(), None, "a denied first call");

    b.report("hook.session_start", "s1", json!({"source": "startup"}));
    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tool_windows": true, "tree": clean.clone()}));
    turn(&b, "s1", "Go", &[tool_use("t1", "Agent", json!({"prompt": "fix it"})), tool_result("t1", false)]);
    start_with_ends(&b, "Agent", "t1", &clean);
    assert!(stop().is_some_and(|x| x.contains("a.rs")), "a first call that ran, its end lost");

    // A start from hooks that don't send ToolEnd still keeps to the prompt's stamp.
    b.report("hook.session_start", "s1", json!({"source": "startup"}));
    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tool_windows": true, "tree": clean.clone()}));
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "python3 fix.py"})), tool_result("t1", false)]);
    b.report("hook.tool_start", "s1", json!({"tool": "Bash", "tool_use_id": "t1", "tree": saved.clone()}));
    assert!(stop().is_some_and(|x| x.contains("a.rs")), "the prompt's stamp");
}

#[test]
fn a_call_with_no_tool_use_id_is_found_in_the_transcript_by_its_tool_and_input() {
    let b = new_board();
    b.add_session("s1");
    let clean = tree(&b, "aaa", &[]);
    let saved = tree(&b, "aaa", &[("a.rs", "h1:5")]);
    let stop = || b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": b.repo(), "tree": saved.clone()}))["block"].as_str().map(|s| s.to_string());
    let input = json!({"command": "python3 fix.py", "description": "Fix it"});
    // The hook's own id for it, from the payload's copy of the input (keys in another order).
    let made_up = taskboardd::transcript::call_key("Bash", &json!({"description": "Fix it", "command": "python3 fix.py"}));

    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("toolu_1", "Bash", input.clone()), tool_result("toolu_1", true)]);
    call(&b, &made_up, clean.clone(), None);
    assert_eq!(stop(), None, "the denied call, matched by its tool and input");

    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("toolu_1", "Bash", input.clone()), tool_result("toolu_1", false)]);
    call(&b, &made_up, clean.clone(), None);
    assert!(stop().is_some_and(|x| x.contains("a.rs")), "the same call, run");

    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("toolu_1", "Bash", input), tool_result("toolu_1", true), tool_use("toolu_2", "Bash", json!({"command": "ls"})), tool_result("toolu_2", false)]);
    call(&b, "call-0000000000000000", clean.clone(), None);
    assert!(stop().is_some_and(|x| x.contains("a.rs")), "a call the transcript doesn't show can't be told apart: it ran");
}

#[test]
fn a_folder_or_file_a_command_names_doesnt_widen_the_turn_to_its_parent() {
    let b = new_board();
    b.add_session("s1");
    let app = format!("{}/app", b.repo());
    std::fs::create_dir_all(format!("{}/docs", b.repo())).unwrap();
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(format!("{}/Cargo.toml", b.repo()), "").unwrap();
    let clean = tree(&b, "aaa", &[]);
    let saved = tree(&b, "aaa", &[("lib/x.rs", "h1:5")]);
    let stop = || b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": app, "tree": saved.clone()}))["block"].as_str().map(|s| s.to_string());

    // From `app/`, a long call names `../docs/` and `../Cargo.toml`; the owner saves `lib/x.rs` meanwhile.
    for command in ["ls ../docs/ && cargo test", "cargo test --manifest-path ../Cargo.toml"] {
        prompt_with_windows(&b, &clean);
        turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": command})), tool_result("t1", false)]);
        call(&b, "t1", clean.clone(), Some(saved.clone()));
        assert_eq!(stop(), None, "{command}");
    }

    // A file the command makes: its folder is the turn's.
    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "python3 gen.py > ../lib/new.rs"})), tool_result("t1", false)]);
    call(&b, "t1", clean.clone(), Some(saved.clone()));
    assert!(stop().is_some_and(|x| x.contains("x.rs")), "the folder a new file goes in");
}

#[test]
fn a_long_command_doesnt_count_saves_outside_the_folders_the_turn_worked_in() {
    let b = new_board();
    b.add_session("s1");
    let app = format!("{}/app", b.repo());
    std::fs::create_dir_all(&app).unwrap();
    let clean = tree(&b, "aaa", &[]);
    let stop = |after: Value| b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": app, "tree": after}))["block"].as_str().map(|s| s.to_string());

    // From `app/`, `cargo test` runs a while; the owner saves `lib/x.rs` meanwhile, and the build writes `app/gen.rs`.
    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "cargo test"})), tool_result("t1", false)]);
    call(&b, "t1", clean.clone(), Some(tree(&b, "aaa", &[("lib/x.rs", "h1:5")])));
    assert_eq!(stop(tree(&b, "aaa", &[("lib/x.rs", "h1:5")])), None, "a save in a sibling folder");

    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "cargo test"})), tool_result("t1", false)]);
    let both = tree(&b, "aaa", &[("lib/x.rs", "h1:5"), ("app/gen.rs", "h2:5")]);
    call(&b, "t1", clean.clone(), None);
    let block = stop(both.clone()).expect("a change in the terminal's folder");
    assert!(block.contains("gen.rs") && !block.contains("x.rs"), "{block}");

    // A path the command names, or a subagent, counts outside it.
    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "cargo fmt --manifest-path=../lib/Cargo.toml"})), tool_result("t1", false)]);
    call(&b, "t1", clean.clone(), Some(tree(&b, "aaa", &[("lib/x.rs", "h1:5")])));
    assert!(stop(tree(&b, "aaa", &[("lib/x.rs", "h1:5")])).is_some_and(|x| x.contains("x.rs")), "the folder of a path the command names");

    prompt_with_windows(&b, &clean);
    turn(&b, "s1", "Go", &[tool_use("t1", "Agent", json!({"prompt": "fix lib"})), tool_result("t1", false)]);
    b.report("hook.tool_start", "s1", json!({"tool": "Agent", "tool_use_id": "t1", "tree": clean.clone()}));
    b.report("hook.tool_end", "s1", json!({"tool": "Agent", "tool_use_id": "t1", "tree": tree(&b, "aaa", &[("lib/x.rs", "h1:5")])}));
    assert!(stop(tree(&b, "aaa", &[("lib/x.rs", "h1:5")])).is_some_and(|x| x.contains("x.rs")), "a subagent works anywhere in the checkout");
}

#[test]
fn hooks_loaded_before_the_call_stamps_keep_to_the_prompts_stamp() {
    let b = new_board();
    b.add_session("s1");
    let clean = tree(&b, "aaa", &[]);
    let edited = tree(&b, "aaa", &[("a.rs", "h1:5")]);
    let stop = |after: Value| b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": b.repo(), "tree": after}))["block"].as_str().map(|s| s.to_string());

    // The new `tb` says it stamps each call, but the terminal's hooks.json has no Agent matcher and no
    // ToolEnd: a subagent's edit, a background one's, and its commit still block.
    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tool_windows": true, "tree": clean.clone()}));
    turn(&b, "s1", "Go", &[tool_use("t1", "Agent", json!({"prompt": "fix it"})), tool_result("t1", false)]);
    assert!(stop(edited.clone()).is_some_and(|x| x.contains("a.rs")), "a subagent's edit");

    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tool_windows": true, "tree": clean.clone()}));
    turn(&b, "s1", "Go", &[tool_use("t1", "Agent", json!({"prompt": "fix it", "run_in_background": true})), tool_result("t1", false)]);
    let committed = tree_with(&b, "bbb", &[], &[("bbb", "commit: theirs too"), ("aaa", "commit (initial): start")]);
    assert!(stop(committed).is_some_and(|x| x.contains("a commit: theirs too")), "a background subagent's commit");

    // Once these hooks send a call's end, the windows decide.
    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tool_windows": true, "tree": clean.clone()}));
    turn(&b, "s1", "Go", &[tool_use("t1", "Bash", json!({"command": "ls"})), tool_result("t1", false)]);
    call(&b, "t1", clean.clone(), Some(clean.clone()));
    assert_eq!(stop(edited.clone()), None, "a save between the agent's calls");

    // A new Claude process may have loaded other hooks.
    b.report("hook.session_start", "s1", json!({"source": "startup"}));
    b.report("hook.prompt", "s1", json!({"prompt": "Go", "tool_windows": true, "tree": clean.clone()}));
    turn(&b, "s1", "Go", &[tool_use("t1", "Agent", json!({"prompt": "fix it"})), tool_result("t1", false)]);
    assert!(stop(edited).is_some_and(|x| x.contains("a.rs")), "back to the prompt's stamp after a restart");
}

#[test]
fn the_hooks_transcript_path_stands_in_when_the_session_id_finds_none() {
    let b = new_board();
    b.add_session("s1");
    let file = b.app.cfg.claude_projects.join("-elsewhere").join("other-id.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let edit = json!({"type": "assistant", "message": {"content": [{"type": "tool_use", "name": "Edit", "input": {"file_path": format!("{}/src/a.rs", b.repo())}}]}});
    let lines = [json!({"type": "user", "message": {"content": "Go"}, "timestamp": "2026-10-08T10:00:00Z"}), edit];
    std::fs::write(&file, lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")).unwrap();
    let out = b.report("hook.stop", "s1", json!({"last_message": "Done.", "cwd": b.repo(), "transcript_path": file.to_string_lossy()}));
    assert!(out["block"].as_str().is_some_and(|x| x.contains("a.rs")), "{out}");
}


#[test]
fn a_review_alert_stays_until_the_pr_is_reviewed() {
    let b = new_board();
    let t = b.task("Ship it", json!({}));
    b.app.db.tx(|| dispatch::add_alert(&b.app, "PR #1 for T1 is green and ready for review.", Some(t), None, None, Some("review"))).unwrap();
    let id = dispatch::alerts(&b.app)[0]["id"].as_str().unwrap().to_string();
    let ids = || dispatch::alerts(&b.app).iter().map(|a| a["id"].as_str().unwrap().to_string()).collect::<Vec<_>>();

    let (code, _) = b.post_err(&format!("alerts/{id}/dismiss"), json!({}));
    assert_eq!(code, 409, "it can't be dismissed");
    b.post(&format!("alerts/{id}/snooze"), json!({"mins": 15}));
    assert_eq!(ids(), vec![id.clone()], "but it snoozes");

    b.app.db.tx(|| {
        dispatch::add_alert(&b.app, "T1 didn't start.", Some(t), None, None, None)?;
        dispatch::clear_alerts(&b.app, Some(t), None)
    }).unwrap();
    assert_eq!(ids(), vec![id.clone()], "other alerts for the task don't replace it");

    b.app.db.tx(|| {
        for n in 0..25 {
            dispatch::add_alert(&b.app, &format!("Alert {n}"), None, None, Some(&format!("s{n}")), None)?;
        }
        Ok(())
    }).unwrap();
    assert!(ids().contains(&id), "a flood of other alerts doesn't push it out");
    assert_eq!(ids().len(), 21);
}

fn pr_task(b: &Board) -> i64 {
    let id = b.task("Ship it", json!({}));
    b.add_session("s1");
    b.report("tb.take", "s1", json!({"task": format!("T{id}")}));
    b.report("tb.done", "s1", json!({"summary": "Done", "pr": "https://github.com/acme/webapp/pull/9"}));
    id
}

fn rec(decision: &str, comments: i64, changes_at: &str) -> Value {
    json!({"state": "OPEN", "head": "h1", "checks": [{"name": "ci", "state": "passed"}], "failed": [], "running": 0,
           "comments": comments, "approvals": if decision == "APPROVED" { 1 } else { 0 }, "review_decision": decision, "changes_at": changes_at})
}

#[test]
fn addressed_changes_wait_in_rereview() {
    let b = board_with(|c| {
        c.pr.watch = true;
        c.pr.wake = true;
    });
    let id = pr_task(&b);
    let t = || board::get_task(&b.app, id).unwrap();
    b.app.db.tx(|| prflow::step(&b.app, &t(), &rec("CHANGES_REQUESTED", 1, "c1"))).unwrap();
    assert_eq!(t().s("pr_phase"), Some("comments"));
    b.app.db.x("UPDATE jobs SET state = 'done' WHERE purpose = 'pr'", p![]).unwrap();
    b.post(&format!("tasks/T{id}/pr/wait"), json!({}));
    b.app.db.tx(|| prflow::step(&b.app, &t(), &rec("CHANGES_REQUESTED", 1, "c1"))).unwrap();
    assert_eq!(t().s("pr_phase"), Some("rereview"));
    assert_eq!(prflow::label("rereview"), "Awaiting re-review");
    assert!(!prflow::awaiting_owner(&t()), "it waits on the reviewer, not the owner");
    b.app.db.tx(|| prflow::step(&b.app, &t(), &rec("CHANGES_REQUESTED", 1, "c2"))).unwrap();
    assert_eq!(t().s("pr_phase"), Some("comments"), "new changes asked: back to work");
}
