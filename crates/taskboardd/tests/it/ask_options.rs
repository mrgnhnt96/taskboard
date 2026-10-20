//! What a task waiting on the owner asks: `tb question --option` answers in a click, Claude's picker
//! (`AskUserQuestion`) shows its questions and options until it closes, and an idle prompt says what
//! the agent's last reply ended on rather than Claude's "waiting for your input". The app's buttons
//! answer a picker through Midna (`POST /tasks/:id/pick`).

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::RowExt;
use taskboardd::{board, midna, reports};

struct Board {
    app: Arc<App>,
    _dir: tempfile::TempDir,
}

fn new_board() -> Board {
    new_board_with(|_| {})
}

fn new_board_with(tweak: impl FnOnce(&mut Config)) -> Board {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = Config::for_tests(dir.path());
    tweak(&mut cfg);
    let repo = dir.path().join("webapp");
    std::fs::create_dir_all(&repo).unwrap();
    let app = App::for_tests(cfg);
    app.db.set_setting("midna_projects", Some(&json!([{"name": "webapp", "path": repo.to_string_lossy()}]).to_string())).unwrap();
    midna::sync(&app, &[json!({"id": "s1", "name": "Term s1", "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})], &[])
        .unwrap();
    let b = Board { app, _dir: dir };
    b.post("/tasks", json!({"title": "Cap the retries", "detail": "Do the thing.", "project": "webapp"}));
    b.report("tb.take", json!({"task": "T1"}));
    b
}

impl Board {
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn report(&self, event: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": "s1", "claude_session": "c-s1", "cwd": "", "git": {"branch": "feature/x", "sha": "abc123"}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message))
    }
    fn post_err(&self, path: &str, body: Value) -> (u16, String) {
        let e = api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_err();
        (e.status, e.message)
    }
    fn card(&self) -> Value {
        board::task_card(&self.app, &board::get_task(&self.app, 1).unwrap()).unwrap()
    }
}

#[test]
fn question_options_go_on_the_card_and_drop_with_the_question() {
    let b = new_board();
    b.report("tb.question", json!({"text": "10 or 30 retries?", "options": ["10: The old limit", "30"]}));
    let c = b.card();
    assert_eq!(c["status"], "needs");
    assert_eq!(c["ask"]["picker"], false);
    assert_eq!(c["ask"]["questions"][0]["options"], json!([{"label": "10", "description": "The old limit"}, {"label": "30", "description": ""}]));

    b.post("/tasks/T1/answer", json!({"text": "10", "when": "now"}));
    b.report("hook.prompt", json!({"prompt": "[task-board:T1] answer: 10"}));
    let c = b.card();
    assert_eq!(c["status"], "working");
    assert_eq!(c["ask"], Value::Null);

    b.report("tb.question", json!({"text": "Ship it?"}));
    assert_eq!(b.card()["ask"], Value::Null, "a question without options has none");
}

#[test]
fn a_picker_waits_on_the_owner_until_it_closes() {
    let b = new_board();
    let input = json!({"questions": [{"question": "Pick a fruit", "header": "Fruit", "multiSelect": false,
                                       "options": [{"label": "Apple", "description": "Red"}, {"label": "Pear", "description": "Green"}]}]});
    b.report("hook.ask", json!({"tool_input": input}));
    let c = b.card();
    assert_eq!((c["status"].as_str(), c["needs_reason"].as_str(), c["question"].as_str()), (Some("needs"), Some("attention"), Some("Pick a fruit")));
    assert_eq!(c["ask"]["picker"], true);
    assert_eq!(c["ask"]["questions"][0]["options"][1], json!({"label": "Pear", "description": "Green"}));

    // Claude's own notification about the picker says less than the picker.
    b.report("hook.attention", json!({"notification_type": "permission_prompt", "message": "Claude needs your input"}));
    assert_eq!(b.card()["question"], "Pick a fruit");

    b.report("hook.ask_done", json!({}));
    let c = b.card();
    assert_eq!(c["status"], "working");
    assert_eq!((c["question"].clone(), c["ask"].clone()), (Value::Null, Value::Null));
    assert!(board::get_task(&b.app, 1).unwrap().s("answered_at").is_some());
}

#[test]
fn an_idle_prompt_says_what_the_last_reply_ended_on() {
    let b = new_board();
    let dir = b.app.cfg.claude_projects.join("webapp");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("c-s1.jsonl");
    let lines = [
        json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "Removed the subscription.\n\nShould I also drop the old topic?"}]}}),
        json!({"type": "assistant", "isSidechain": true, "message": {"content": [{"type": "text", "text": "a subagent's"}]}}),
    ];
    std::fs::write(&path, lines.iter().map(|l| l.to_string()).collect::<Vec<_>>().join("\n")).unwrap();
    b.report("hook.attention", json!({"notification_type": "idle_prompt", "message": "Claude is waiting for your input", "transcript_path": path.to_string_lossy()}));
    let c = b.card();
    assert_eq!((c["status"].as_str(), c["question"].as_str()), (Some("needs"), Some("Should I also drop the old topic?")));
}

#[test]
fn a_question_answered_another_way_is_withdrawn() {
    let b = new_board();
    let none = reports::handle(&b.app, json!({"event": "tb.withdraw", "session": "s1", "why": "Found it"}), false).unwrap_err();
    assert_eq!(none.status, 409, "nothing to withdraw before a question");

    b.report("tb.question", json!({"text": "10 or 30 retries?", "options": ["10", "30"]}));
    assert_eq!(b.card()["status"], "needs");
    let empty = reports::handle(&b.app, json!({"event": "tb.withdraw", "session": "s1", "why": " "}), false).unwrap_err();
    assert_eq!(empty.status, 400, "a withdrawal says how it got answered");

    let v = b.report("tb.withdraw", json!({"why": "The config already says 10"}));
    assert_eq!(v["task"], "T1");
    let c = b.card();
    assert_eq!((c["status"].as_str(), c["question"].as_str(), c["ask"].as_object()), (Some("working"), None, None));
    let t = board::get_task(&b.app, 1).unwrap();
    assert_eq!(t.s("needs_reason"), None);
    let logged = b.app.db.q1("SELECT text FROM events WHERE task_id = 1 ORDER BY id DESC LIMIT 1", taskboardd::p![]).unwrap().unwrap().st("text");
    assert_eq!(logged, "Withdrew the question: The config already says 10");
}

/// Midna's socket, answering each connection with the next reply (`result` or `error`) after a
/// notification line, and keeping the requests it got.
struct FakeMidna {
    sock: std::path::PathBuf,
    got: Arc<std::sync::Mutex<Vec<Value>>>,
    _dir: tempfile::TempDir,
}

fn fake_midna(replies: Vec<Value>) -> FakeMidna {
    use std::io::{BufRead, BufReader, Write};
    // Short: a Unix socket's path has a length limit.
    let dir = tempfile::Builder::new().prefix("m").tempdir_in("/tmp").unwrap();
    let sock = dir.path().join("s.sock");
    let listener = std::os::unix::net::UnixListener::bind(&sock).unwrap();
    let got = Arc::new(std::sync::Mutex::new(Vec::new()));
    let seen = got.clone();
    std::thread::spawn(move || {
        for (conn, reply) in listener.incoming().zip(replies) {
            let mut conn = conn.unwrap();
            let mut line = String::new();
            BufReader::new(&conn).read_line(&mut line).unwrap();
            let req: Value = serde_json::from_str(&line).unwrap();
            seen.lock().unwrap().push(req.clone());
            let mut out = json!({"jsonrpc": "2.0", "id": req["id"]});
            let key = if reply.get("code").is_some() { "error" } else { "result" };
            out[key] = reply;
            writeln!(conn, "{}", json!({"jsonrpc": "2.0", "method": "event", "params": {}})).unwrap();
            writeln!(conn, "{out}").unwrap();
        }
    });
    FakeMidna { sock, got, _dir: dir }
}

fn picker_board(m: &FakeMidna) -> Board {
    let sock = m.sock.clone();
    let b = new_board_with(|c| c.midna_socket = sock);
    let input = json!({"questions": [
        {"question": "Pick a fruit", "header": "Fruit", "multiSelect": false, "options": [{"label": "Apple", "description": ""}, {"label": "Pear", "description": ""}]},
        {"question": "Pick sizes", "header": "Size", "multiSelect": true, "options": [{"label": "S", "description": ""}, {"label": "M", "description": ""}, {"label": "L", "description": ""}]}]});
    b.report("hook.ask", json!({"tool_input": input}));
    b
}

#[test]
fn a_pick_answers_the_picker_through_midna() {
    let m = fake_midna(vec![json!({"ok": true, "answers": []})]);
    let b = picker_board(&m);
    b.post("/tasks/T1/pick", json!({"answers": [[1], [2, 0]]}));
    let req = m.got.lock().unwrap()[0].clone();
    assert_eq!(req["method"], "session.answer_question");
    assert_eq!(
        req["params"],
        json!({"id": "s1", "answers": [{"options": [1], "question": "Pick a fruit"}, {"options": [0, 2], "question": "Pick sizes"}]})
    );
    let logged = b.app.db.q1("SELECT text FROM events WHERE task_id = 1 ORDER BY id DESC LIMIT 1", taskboardd::p![]).unwrap().unwrap().st("text");
    assert_eq!(logged, "Answered: Pear; S, L");
    let t = board::get_task(&b.app, 1).unwrap();
    assert!(t.s("answered_at").is_some());
    assert!(board::task_context(&t)["answers"][0].as_str().unwrap().contains("“Pear; S, L”"));
    assert_eq!(b.card()["status"], "needs", "it waits until the picker closes");

    b.report("hook.ask_done", json!({}));
    assert_eq!((b.card()["status"].clone(), b.card()["ask"].clone()), (json!("working"), Value::Null));
    let (code, msg) = b.post_err("/tasks/T1/pick", json!({"answers": [[0], [0]]}));
    assert_eq!((code, msg.as_str()), (409, "The picker closed: T1 isn't waiting on it any more."));
}

#[test]
fn a_pick_names_one_choice_per_question() {
    let m = fake_midna(vec![]);
    let b = picker_board(&m);
    for (answers, want) in [
        (json!([[0]]), "Pick an answer for each of the picker's 2 questions."),
        (json!([[0, 1], [0]]), "Pick one option for “Pick a fruit”."),
        (json!([[0], []]), "Pick at least one option for “Pick sizes”."),
        (json!([[5], [0]]), "“Pick a fruit” has no such option."),
    ] {
        assert_eq!(b.post_err("/tasks/T1/pick", json!({"answers": answers})), (400, want.to_string()));
    }
    assert!(m.got.lock().unwrap().is_empty(), "Midna isn't asked");
}

#[test]
fn a_pick_says_why_midna_turned_it_down() {
    let m = fake_midna(vec![
        json!({"code": 2, "message": "only the human answers Claude's questions"}),
        json!({"code": 4, "message": "session s1: the picker shows another question; nothing was pressed that answers the picker"}),
        json!({"code": -32601, "message": "unknown method"}),
    ]);
    let b = picker_board(&m);
    let pick = || b.post_err("/tasks/T1/pick", json!({"answers": [[0], [1]]}));
    let (code, msg) = pick();
    assert_eq!(code, 403);
    assert!(msg.contains("Add com.mrgnhnt.taskboard.daemon to Midna's agents.trusted_clients"), "{msg}");
    let (code, msg) = pick();
    assert_eq!(code, 409);
    assert!(msg.contains("the picker shows another question"), "{msg}");
    assert_eq!(pick().0, 501);
    assert_eq!(b.card()["status"], "needs", "still waiting on the owner");

    // Midna isn't running.
    let down = new_board();
    down.report("hook.ask", json!({"tool_input": {"questions": [{"question": "Go?", "options": [{"label": "Yes"}]}]}}));
    assert_eq!(down.post_err("/tasks/T1/pick", json!({"answers": [[0]]})).0, 503);
}

#[test]
fn a_pick_needs_a_picker() {
    let b = new_board();
    b.report("tb.question", json!({"text": "10 or 30 retries?", "options": ["10", "30"]}));
    assert_eq!(b.post_err("/tasks/T1/pick", json!({"answers": [[0]]})).0, 409, "a tb question is answered with its text");
}
