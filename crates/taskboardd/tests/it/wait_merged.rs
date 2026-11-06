//! `tb wait-for T<n> --merged`: a task that waits for another task's PR to merge, not just for it to finish (#99).

use std::sync::Arc;

use serde_json::{json, Value};
use taskboardd::api::{self, Query};
use taskboardd::app::App;
use taskboardd::config::Config;
use taskboardd::util::{Row, RowExt};
use taskboardd::{board, declined, dispatch, midna, p, prflow, reports, runner, stack, waitsfor};

struct Board {
    app: Arc<App>,
    dir: tempfile::TempDir,
}

fn new_board() -> Board {
    let dir = tempfile::tempdir().unwrap();
    let cfg = Config::for_tests(dir.path());
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
        api::dispatch(&self.app, "GET", path, &Query::new(), &json!({})).unwrap_or_else(|e| panic!("GET {path}: {}", e.message))
    }
    fn post(&self, path: &str, body: Value) -> Value {
        api::dispatch(&self.app, "POST", path, &Query::new(), &body).unwrap_or_else(|e| panic!("POST {path}: {}", e.message))
    }
    fn report(&self, event: &str, session: &str, extra: Value) -> Value {
        let mut b = json!({"event": event, "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {"branch": "feature/x"}});
        for (k, v) in extra.as_object().unwrap() {
            b[k] = v.clone();
        }
        reports::handle(&self.app, b, false).unwrap_or_else(|e| panic!("{event}: {}", e.message))
    }
    fn session(&self, sid: &str) {
        let repo = self.dir.path().join("webapp");
        midna::sync(
            &self.app,
            &[json!({"id": sid, "name": format!("Term {sid}"), "agent": "claude", "cwd": repo.to_string_lossy(), "status": {"state": "working"}})],
            &[],
        )
        .unwrap();
    }
    fn task(&self, title: &str) -> i64 {
        self.post("/tasks", json!({"title": title, "detail": "Do it.", "project": "webapp", "pickup": "new"}))["id"].as_i64().unwrap()
    }
    fn on(&self, sid: &str, id: i64) {
        self.session(sid);
        self.report("tb.take", sid, json!({"task": format!("T{id}")}));
    }
    fn row(&self, id: i64) -> Row {
        board::get_task(&self.app, id).unwrap()
    }
    fn card(&self, id: i64) -> Value {
        self.get(&format!("tasks/T{id}"))
    }
    fn starts(&self, id: i64) -> Vec<Row> {
        self.app.db.q("SELECT * FROM jobs WHERE kind = 'agent' AND purpose = 'start' AND task_id = ? ORDER BY id", p![id]).unwrap()
    }
}

/// T1 is done with PR #9 open; T2 is on s2.
fn done_with_open_pr(b: &Board) -> (i64, i64) {
    let one = b.task("Add the endpoint");
    let two = b.task("Use the endpoint");
    b.on("s1", one);
    b.report("tb.done", "s1", json!({"summary": "Done.", "pr": "https://github.com/acme/webapp/pull/9"}));
    assert_eq!(b.row(one).i("pr_num"), Some(9));
    on_branch(&b, one, "feat/endpoint");
    b.on("s2", two);
    (one, two)
}

fn on_branch(b: &Board, id: i64, branch: &str) {
    prflow::merge_flow(&b.app, id, vec![("rec", json!({"state": "OPEN", "branch": branch, "base": "main"}))]).unwrap();
}

#[test]
fn it_parks_while_the_pr_is_open_and_resumes_once_it_merges() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    let dir = b.app.cfg.claude_projects.join("-webapp");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("c-s2.jsonl"), "{\"type\":\"user\"}\n").unwrap();

    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")], "merged": true, "why": "Needs it on main"}));
    assert_eq!(out["parked"], true, "{out}");
    assert!(out["context"].as_str().unwrap().contains(&format!("Blocked by T{one} until PR #9 merges")), "{out}");
    assert_eq!(waitsfor::merged_ids(&b.row(two)), vec![one]);
    let card = b.card(two);
    assert_eq!(card["status"], "queued");
    assert_eq!(card["blocked"], true);
    assert_eq!(card["waits_for_state"], json!([{"ref": format!("T{one}"), "done": false}]));
    assert!(card["waiting"].as_str().unwrap().contains("until PR #9 merges"), "{card}");
    assert!(runner::start_queued(&b.app).unwrap().is_empty(), "it waits while the PR is open");
    assert!(b.starts(two).is_empty());

    board::update_task(&b.app, one, vec![("pr_state", json!("MERGED")), ("pr_phase", json!("merged"))]).unwrap();
    assert_eq!(b.card(two)["waits_for_state"][0]["done"], true);
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![two]);
    let jobs = b.starts(two);
    assert_eq!(jobs.len(), 1);
    let args = board::job_args(&jobs[0]);
    assert_eq!(args.st("flags"), "--resume c-s2", "it carries on in the parked conversation");
    assert!(args.st("prompt").contains("is merged (PR #9)"), "{}", args.st("prompt"));
}

#[test]
fn a_declined_pr_says_so_and_keeps_it_waiting() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")], "merged": true}));
    board::update_task(&b.app, one, vec![("pr_state", json!("CLOSED")), ("pr_phase", json!("declined"))]).unwrap();
    let w = b.card(two)["waiting"].as_str().unwrap().to_string();
    assert!(w.contains(&format!("T{one}")) && w.contains("whose PR #9 was declined"), "{w}");
    assert!(runner::start_queued(&b.app).unwrap().is_empty());
}

#[test]
fn a_wait_on_done_work_with_an_open_pr_stacks_on_it() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")]}));
    assert_eq!(out["parked"], false, "{out}");
    let text = out["context"].as_str().unwrap();
    assert!(text.contains(&format!("This task stacks on T{one} (branch feat/endpoint, PR #9)")), "{text}");
    assert!(text.contains("open the PR into feat/endpoint"), "{text}");
    assert!(!text.contains("--merged instead"), "a stack waits for the merge itself: {text}");
    assert!(!text.contains("This task needs work from another task"), "{text}");
    let t = b.row(two);
    assert_eq!(t.i("pr_after"), Some(one));
    assert!(waitsfor::ids(&t).is_empty(), "the wait became the stack");
    assert_eq!(t.s("stack_merge"), None, "a tail merge");
    assert_eq!(stack::base_branch(&b.app, &t).unwrap().as_deref(), Some("feat/endpoint"));
    assert!(stack::holds(&b.app, &t).unwrap(), "its PR waits for #9");
    assert_eq!(t.s("status"), Some("working"));
}

#[test]
fn a_parked_wait_stacks_once_the_work_is_done_with_its_pr_open() {
    let b = new_board();
    let one = b.task("Add the endpoint");
    let two = b.task("Use the endpoint");
    b.on("s1", one);
    b.on("s2", two);
    let dir = b.app.cfg.claude_projects.join("-webapp");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("c-s2.jsonl"), "{\"type\":\"user\"}\n").unwrap();
    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")], "why": "Needs the endpoint"}));
    assert_eq!(out["parked"], true, "{out}");

    b.report("tb.done", "s1", json!({"summary": "Done.", "pr": "https://github.com/acme/webapp/pull/9"}));
    on_branch(&b, one, "feat/endpoint");
    runner::start_queued(&b.app).unwrap();
    let t = b.row(two);
    assert_eq!(t.i("pr_after"), Some(one), "it stacks before it starts again");
    assert!(waitsfor::ids(&t).is_empty());
    let jobs = b.starts(two);
    assert_eq!(jobs.len(), 1);
    let prompt = board::job_args(&jobs[0]).st("prompt").to_string();
    assert!(prompt.contains("open the PR into feat/endpoint"), "{prompt}");
    assert!(!prompt.contains("This task needs work from another task"), "the stack line replaces the rebase advice: {prompt}");
}

#[test]
fn a_wait_on_a_merged_pr_stays_a_plain_wait_on_main() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    board::update_task(&b.app, one, vec![("pr_state", json!("MERGED")), ("pr_phase", json!("merged"))]).unwrap();
    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")]}));
    assert_eq!(out["parked"], false, "{out}");
    let text = out["context"].as_str().unwrap();
    assert!(!text.contains("stacks on"), "{text}");
    let t = b.row(two);
    assert_eq!(t.i("pr_after"), None);
    assert_eq!(waitsfor::ids(&t), vec![one]);
    assert_eq!(stack::base_branch(&b.app, &t).unwrap(), None);
}

#[test]
fn a_merged_wait_on_an_open_pr_stays_a_wait() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")], "merged": true}));
    let t = b.row(two);
    assert_eq!(t.i("pr_after"), None, "--merged asks for main, not a stack");
    assert_eq!(waitsfor::ids(&t), vec![one]);
}

#[test]
fn work_with_no_pr_counts_as_merged() {
    let b = new_board();
    let one = b.task("Add the endpoint");
    let two = b.task("Use the endpoint");
    b.on("s1", one);
    b.report("tb.done", "s1", json!({"summary": "Done.", "no_pr": "Config only"}));
    b.on("s2", two);
    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")], "merged": true}));
    assert_eq!(out["parked"], false, "{out}");
    assert!(!out["context"].as_str().unwrap_or("").contains("--merged"), "{out}");
    assert_eq!(b.card(two)["waits_for_state"][0]["done"], true);
}

#[test]
fn a_task_that_ships_no_pr_counts_as_merged() {
    let b = new_board();
    let one = b.task("Add the endpoint");
    let two = b.task("Use the endpoint");
    b.post(&format!("/tasks/T{one}"), json!({"ships_pr": "no"}));
    b.on("s1", one);
    b.report("tb.done", "s1", json!({"summary": "Done."}));
    b.on("s2", two);
    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")], "merged": true}));
    assert_eq!(out["parked"], false, "{out}");
}

#[test]
fn waiting_for_none_forgets_the_merge_waits() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")], "merged": true}));
    assert_eq!(waitsfor::merged_ids(&b.row(two)), vec![one]);
    b.on("s2", two);
    b.report("tb.wait_for", "s2", json!({"tasks": "none"}));
    assert!(waitsfor::merged_ids(&b.row(two)).is_empty());
}

/// T3 stacks on T1 with the given merge mode, is done, and its PR #10 merged into T1's branch.
fn stacked_and_merged(b: &Board, parent: i64, mode: &str) -> i64 {
    let three = b.task("Polish the endpoint");
    let mode = if mode == "head" { json!("head") } else { Value::Null };
    board::update_task(
        &b.app,
        three,
        vec![("pr_after", json!(parent)), ("stack_merge", mode), ("status", json!("done")), ("pr_num", json!(10)),
             ("pr_url", json!("https://github.com/acme/webapp/pull/10")), ("pr_state", json!("MERGED")), ("pr_phase", json!("merged"))],
    )
    .unwrap();
    three
}

#[test]
fn a_head_merged_pr_counts_as_merged_once_its_stack_reaches_main() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    let three = stacked_and_merged(&b, one, "head");
    assert!(!waitsfor::merged(&b.app, Some(&b.row(three))).unwrap());

    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{three}")], "merged": true}));
    assert_eq!(out["parked"], true, "{out}");
    let w = b.card(two)["waiting"].as_str().unwrap().to_string();
    assert!(w.contains(&format!("merged into T{one}'s branch")) && w.contains(&format!("once T{one}'s PR #9 merges")), "{w}");
    assert!(runner::start_queued(&b.app).unwrap().is_empty(), "its work isn't on main yet");

    board::update_task(&b.app, one, vec![("pr_state", json!("MERGED")), ("pr_phase", json!("merged"))]).unwrap();
    assert!(waitsfor::merged(&b.app, Some(&b.row(three))).unwrap());
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![two]);
}

#[test]
fn a_tail_merged_pr_counts_as_merged_when_it_merges() {
    let b = new_board();
    let (one, _) = done_with_open_pr(&b);
    let three = stacked_and_merged(&b, one, "tail");
    assert!(waitsfor::merged(&b.app, Some(&b.row(three))).unwrap());
}

/// Runs git in the board's webapp checkout and gives its trimmed output.
fn git(b: &Board, args: &[&str]) -> String {
    let repo = b.dir.path().join("webapp");
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["-c", "user.name=Test", "-c", "user.email=test@example.com", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn commit(b: &Board, file: &str, text: &str) -> String {
    std::fs::write(b.dir.path().join("webapp").join(file), text).unwrap();
    git(b, &["add", file]);
    git(b, &["commit", "-q", "-m", file]);
    git(b, &["rev-parse", "HEAD"])
}

/// The checkout behind #836: T1's branch feat/endpoint (one commit on main), and T3's branch
/// feat/polish on top of it. Gives T1's commit and T3's.
fn stack_repo(b: &Board) -> (String, String) {
    git(b, &["init", "-q", "-b", "main"]);
    commit(b, "readme.txt", "base");
    git(b, &["switch", "-q", "-c", "feat/endpoint"]);
    let a = commit(b, "endpoint.txt", "endpoint");
    git(b, &["switch", "-q", "-c", "feat/polish"]);
    let c = commit(b, "polish.txt", "polish");
    git(b, &["switch", "-q", "feat/endpoint"]);
    (a, c)
}

fn set_head(b: &Board, id: i64, state: &str, branch: &str, head: &str) {
    prflow::merge_flow(&b.app, id, vec![("rec", json!({"state": state, "branch": branch, "base": "main", "head": head}))]).unwrap();
}

fn logged(b: &Board, id: i64, text: &str) -> bool {
    b.app.db.q("SELECT text FROM events WHERE task_id = ?", p![id]).unwrap().iter().any(|e| e.st("text").contains(text))
}

fn flagged(b: &Board, id: i64) -> bool {
    taskboardd::dispatch::alerts(&b.app).iter().any(|a| a["key"] == format!("stack-dropped:{id}"))
}

#[test]
fn a_head_merged_pr_whose_commits_a_force_push_dropped_isnt_merged() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    let three = stacked_and_merged(&b, one, "head");
    let (a, c) = stack_repo(&b);
    set_head(&b, three, "MERGED", "feat/polish", &c);
    // T3's PR merged into feat/endpoint, then feat/endpoint was rebased and force-pushed without it.
    git(&b, &["merge", "-q", "--no-ff", "-m", "Merge feat/polish", "feat/polish"]);
    git(&b, &["reset", "-q", "--hard", &a]);
    let pushed = commit(&b, "endpoint2.txt", "rebased");
    set_head(&b, one, "OPEN", "feat/endpoint", &pushed);

    let out = b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{three}")], "merged": true}));
    assert_eq!(out["parked"], true, "{out}");
    let w = b.card(two)["waiting"].as_str().unwrap().to_string();
    assert!(w.contains(&format!("merged into T{one}'s branch, but a push to that branch dropped its commits")), "{w}");
    assert!(flagged(&b, three), "the dropped work is flagged");
    assert!(logged(&b, three, "commits aren't on feat/endpoint any more"));

    // T1's PR merges without T3's commits: T3's work never reached the main branch.
    prflow::mark_merged(&b.app, &b.row(one), "test").unwrap();
    assert!(!logged(&b, three, "Its stack reached the main branch"), "it doesn't claim the stack landed");
    assert!(!waitsfor::merged(&b.app, Some(&b.row(three))).unwrap());
    assert!(!waitsfor::on_main(&b.app, &b.row(three)).unwrap());
    assert!(runner::start_queued(&b.app).unwrap().is_empty(), "T2 keeps waiting");
}

#[test]
fn dropped_work_brought_back_clears_the_flag() {
    let b = new_board();
    let (one, _) = done_with_open_pr(&b);
    let three = stacked_and_merged(&b, one, "head");
    let (a, c) = stack_repo(&b);
    set_head(&b, three, "MERGED", "feat/polish", &c);
    set_head(&b, one, "OPEN", "feat/endpoint", &a);
    taskboardd::stack_drop::sweep(&b.app).unwrap();
    assert!(flagged(&b, three));
    assert!(taskboardd::stack_drop::dropped(&b.row(three)));

    git(&b, &["merge", "-q", "--no-ff", "-m", "Merge feat/polish again", "feat/polish"]);
    set_head(&b, one, "OPEN", "feat/endpoint", &git(&b, &["rev-parse", "HEAD"]));
    taskboardd::stack_drop::sweep(&b.app).unwrap();
    assert!(!flagged(&b, three));
    assert!(!taskboardd::stack_drop::dropped(&b.row(three)));
    assert!(logged(&b, three, "commits are back on feat/endpoint"));

    prflow::mark_merged(&b.app, &b.row(one), "test").unwrap();
    assert!(logged(&b, three, &format!("Its stack reached the main branch with T{one}'s PR")));
    assert!(waitsfor::merged(&b.app, Some(&b.row(three))).unwrap());
}

#[test]
fn a_squashed_head_merge_still_counts_as_on_the_branch() {
    let b = new_board();
    let (one, _) = done_with_open_pr(&b);
    let three = stacked_and_merged(&b, one, "head");
    let (_, c) = stack_repo(&b);
    set_head(&b, three, "MERGED", "feat/polish", &c);
    git(&b, &["merge", "-q", "--squash", "feat/polish"]);
    git(&b, &["commit", "-q", "-m", "Polish (#10)"]);
    set_head(&b, one, "OPEN", "feat/endpoint", &git(&b, &["rev-parse", "HEAD"]));
    taskboardd::stack_drop::sweep(&b.app).unwrap();
    assert!(!flagged(&b, three), "a squash keeps the work though not the commits");

    prflow::mark_merged(&b.app, &b.row(one), "test").unwrap();
    assert!(waitsfor::merged(&b.app, Some(&b.row(three))).unwrap());
}

fn alert(b: &Board, id: i64) -> Option<String> {
    dispatch::alerts(&b.app).into_iter().find(|a| a["key"] == format!("declined-waits:{id}").as_str()).map(|a| a["text"].as_str().unwrap_or("").to_string())
}

fn patch(b: &Board, id: i64, body: Value) -> Result<Value, (u16, String)> {
    api::dispatch(&b.app, "POST", &format!("/tasks/T{id}"), &Query::new(), &body).map_err(|e| (e.status, e.message))
}

/// T1 is done with PR #9 declined, and T2 waits for it to merge.
fn declined_with_a_waiter(b: &Board) -> (i64, i64) {
    let (one, two) = done_with_open_pr(b);
    b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{one}")], "merged": true}));
    prflow::mark_declined(&b.app, &b.row(one), "owner", "").unwrap();
    (one, two)
}

#[test]
fn a_declined_pr_raises_a_needs_you_for_its_waiters() {
    let b = new_board();
    let (one, two) = declined_with_a_waiter(&b);
    let text = alert(&b, one).expect("a Needs-you for the held waiter");
    assert!(text.contains(&format!("T{one}'s PR #9 was declined, so T{two} waits")), "{text}");
    assert!(text.contains(&format!("tb task set T{one} --landed <commit>")), "{text}");
    assert!(text.contains(&format!("tb wait-for --drop T{one} --task T{two}")), "{text}");
    let w = b.card(two)["waiting"].as_str().unwrap().to_string();
    assert!(w.contains("--landed") && w.contains("--drop"), "the waiter's line says the ways out too: {w}");
    // The runner's sweep keeps it up and doesn't raise a second one.
    declined::sweep(&b.app).unwrap();
    assert_eq!(dispatch::alerts(&b.app).iter().filter(|a| a["key"].as_str().is_some_and(|k| k.starts_with("declined-waits:"))).count(), 1);
}

#[test]
fn a_declined_pr_with_no_waiters_raises_nothing() {
    let b = new_board();
    let (one, _) = done_with_open_pr(&b);
    prflow::mark_declined(&b.app, &b.row(one), "owner", "").unwrap();
    assert!(alert(&b, one).is_none());
    declined::sweep(&b.app).unwrap();
    assert!(alert(&b, one).is_none());
}

#[test]
fn work_that_landed_another_way_counts_as_merged() {
    let b = new_board();
    let (one, two) = declined_with_a_waiter(&b);
    assert!(runner::start_queued(&b.app).unwrap().is_empty());

    let out = patch(&b, one, json!({"landed": "ABCDEF1234567"})).unwrap();
    assert_eq!(out["landed"], "abcdef1234567", "{out}");
    assert_eq!(b.row(one).s("landed"), Some("abcdef1234567"));
    assert!(logged(&b, one, "landed on the main branch in abcdef123456, outside PR #9"));
    assert!(alert(&b, one).is_none(), "the Needs-you clears once it landed");
    assert!(waitsfor::merged(&b.app, Some(&b.row(one))).unwrap());
    assert_eq!(b.card(two)["waits_for_state"][0]["done"], true);
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![two]);
}

#[test]
fn landed_none_takes_it_back_and_the_needs_you_returns() {
    let b = new_board();
    let (one, two) = declined_with_a_waiter(&b);
    patch(&b, one, json!({"landed": "abcdef1"})).unwrap();
    assert!(alert(&b, one).is_none());
    patch(&b, one, json!({"landed": "none"})).unwrap();
    assert_eq!(b.row(one).s("landed"), None);
    assert!(logged(&b, one, "No longer marked as landed"));
    assert!(alert(&b, one).is_some_and(|t| t.contains(&format!("T{two} waits"))));
}

#[test]
fn landed_is_refused_where_it_cant_be_true() {
    let b = new_board();
    let one = b.task("Add the endpoint");
    let (code, msg) = patch(&b, one, json!({"landed": "abcdef1"})).unwrap_err();
    assert_eq!(code, 409, "{msg}");
    assert!(msg.contains("isn't done"), "{msg}");

    let (two, _) = done_with_open_pr(&b);
    let (code, msg) = patch(&b, two, json!({"landed": "not a commit"})).unwrap_err();
    assert_eq!(code, 400, "{msg}");

    board::update_task(&b.app, two, vec![("pr_state", json!("MERGED")), ("pr_phase", json!("merged"))]).unwrap();
    let (code, msg) = patch(&b, two, json!({"landed": "abcdef1"})).unwrap_err();
    assert_eq!(code, 409, "{msg}");
    assert!(msg.contains("already on the main branch"), "{msg}");
}

#[test]
fn landed_resolves_a_ref_in_the_tasks_repository() {
    let b = new_board();
    let (head, _) = stack_repo(&b);
    let (one, _) = declined_with_a_waiter(&b);
    patch(&b, one, json!({"landed": "HEAD"})).unwrap();
    assert_eq!(b.row(one).s("landed"), Some(head.as_str()));
}

#[test]
fn dropping_one_wait_keeps_the_others() {
    let b = new_board();
    let (one, two) = declined_with_a_waiter(&b);
    let three = b.task("Write the docs");
    b.on("s2", two);
    b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{three}")]}));
    let mut ids = waitsfor::ids(&b.row(two));
    ids.sort();
    assert_eq!(ids, vec![one, three]);
    assert!(alert(&b, one).is_some());

    b.on("s2", two);
    let out = b.report("tb.wait_for", "s2", json!({"tasks": [], "drop": [format!("T{one}")]}));
    assert_eq!(out["parked"], false, "{out}");
    let text = out["context"].as_str().unwrap_or("");
    assert!(text.contains(&format!("It no longer waits for T{one}")) && text.contains(&format!("T{three}")), "{out}");
    assert_eq!(waitsfor::ids(&b.row(two)), vec![three]);
    assert!(waitsfor::merged_ids(&b.row(two)).is_empty(), "the merge wait goes with it");
    assert!(logged(&b, two, &format!("No longer waits for T{one}")));
    assert!(alert(&b, one).is_none(), "nobody is held by the declined PR any more");
}

#[test]
fn dropping_the_last_wait_lets_it_go_on() {
    let b = new_board();
    let (one, two) = declined_with_a_waiter(&b);
    b.on("s2", two);
    let out = b.report("tb.wait_for", "s2", json!({"drop": [format!("T{one}")]}));
    assert!(out["context"].as_str().unwrap_or("").contains("has everything else it waits for"), "{out}");
    assert_eq!(b.row(two).s("waits_for"), None);
}

#[test]
fn dropping_a_wait_it_doesnt_have_is_refused() {
    let b = new_board();
    let (one, two) = declined_with_a_waiter(&b);
    let three = b.task("Write the docs");
    b.on("s2", two);
    let mut body = json!({"event": "tb.wait_for", "session": "s2", "claude_session": "c-s2", "cwd": "", "drop": [format!("T{three}")]});
    let e = reports::handle(&b.app, body.clone(), false).unwrap_err();
    assert_eq!(e.status, 400, "{}", e.message);
    assert!(e.message.contains(&format!("doesn't wait for T{three}")), "{}", e.message);
    assert_eq!(waitsfor::ids(&b.row(two)), vec![one], "nothing changed");

    // The task it stacks on is its stack, not a wait to drop.
    board::update_task(&b.app, two, vec![("pr_after", json!(one))]).unwrap();
    body["drop"] = json!([format!("T{one}")]);
    let e = reports::handle(&b.app, body, false).unwrap_err();
    assert_eq!(e.status, 409, "{}", e.message);
    assert!(e.message.contains("--stack-on none"), "{}", e.message);
}

const PR9: &str = "https://github.com/acme/webapp/pull/9";

fn submit(b: &Board, session: &str, extra: Value) -> Result<Value, (u16, String)> {
    let mut body = json!({"event": "tb.done", "session": session, "claude_session": format!("c-{session}"), "cwd": "", "git": {"branch": "feature/x"}});
    for (k, v) in extra.as_object().unwrap() {
        body[k] = v.clone();
    }
    reports::handle(&b.app, body, false).map_err(|e| (e.status, e.message))
}

fn merge(b: &Board, id: i64) {
    board::update_task(&b.app, id, vec![("pr_state", json!("MERGED")), ("pr_phase", json!("merged"))]).unwrap();
}

/// T1 owns PR #9 and T3's summary names it, so T3 finished with no PR linked. T2 waits for T3 to merge.
/// Gives T1, T2 and T3.
fn done_with_no_pr_and_a_waiter(b: &Board) -> (i64, i64, i64) {
    let (one, two) = done_with_open_pr(b);
    let three = b.task("Finish the endpoint");
    b.on("s3", three);
    submit(b, "s3", json!({"summary": format!("Went out in {PR9}.")})).unwrap();
    assert_eq!(b.row(three).s("status"), Some("done"));
    assert!(b.row(three).s("pr_url").is_none(), "a PR the summary only mentions isn't linked");
    b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{three}")], "merged": true}));
    (one, two, three)
}

#[test]
fn submitting_with_another_tasks_pr_joins_it() {
    let b = new_board();
    let (one, two) = done_with_open_pr(&b);
    let three = b.task("Finish the endpoint");
    b.on("s3", three);
    submit(&b, "s3", json!({"summary": "Done.", "pr": PR9})).unwrap();

    let t = b.row(three);
    assert_eq!(t.i("same_pr"), Some(one));
    assert_eq!(t.s("pr_url"), Some(PR9));
    assert_eq!(t.i("pr_num"), None, "the PR stays the other task's");
    assert!(logged(&b, three, &format!("Its work went through T{one}'s PR #9")));

    b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{three}")], "merged": true}));
    assert!(alert(&b, three).is_none());
    assert!(b.card(two)["waiting"].as_str().unwrap().contains("until its PR merges"));
    assert!(runner::start_queued(&b.app).unwrap().is_empty(), "the PR hasn't merged");

    merge(&b, one);
    assert!(waitsfor::merged(&b.app, Some(&b.row(three))).unwrap());
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![two]);
}

#[test]
fn finishing_with_no_pr_is_refused_while_a_task_waits_for_its_merge() {
    let b = new_board();
    let (_, two) = done_with_open_pr(&b);
    let three = b.task("Finish the endpoint");
    b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{three}")], "merged": true}));
    b.on("s3", three);

    let (status, text) = submit(&b, "s3", json!({"summary": format!("Went out in {PR9}.")})).unwrap_err();
    assert_eq!(status, 409, "{text}");
    assert!(text.contains(&format!("T{two} waits for T{three}'s PR to merge")), "{text}");
    assert_eq!(b.row(three).s("status"), Some("working"), "it isn't finished");

    submit(&b, "s3", json!({"summary": "Nothing to ship.", "no_pr": "Already on main"})).unwrap();
    assert_eq!(b.row(three).s("status"), Some("done"));
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![two], "no PR on purpose means no merge to wait for");
}

#[test]
fn a_done_task_with_no_pr_raises_a_needs_you_naming_the_ways_out() {
    let b = new_board();
    let (_, two, three) = done_with_no_pr_and_a_waiter(&b);
    declined::sweep(&b.app).unwrap();

    let text = alert(&b, three).expect("a Needs-you for the held waiter");
    assert!(text.contains(&format!("T{three} finished with no PR linked, so T{two} waits")), "{text}");
    for way in [format!("tb task set T{three} --pr-link <link>"), format!("tb task set T{three} --landed <commit>"), format!("tb wait-for --drop T{three} --task T{two}")] {
        assert!(text.contains(&way), "{text}");
    }
    let w = b.card(two)["waiting"].as_str().unwrap().to_string();
    assert!(w.contains("finished with no PR linked") && w.contains("--pr-link") && !w.contains("until its PR merges"), "{w}");
}

#[test]
fn linking_a_merged_pr_clears_the_needs_you_and_frees_the_waiter() {
    let b = new_board();
    let (one, two, three) = done_with_no_pr_and_a_waiter(&b);
    merge(&b, one);
    declined::sweep(&b.app).unwrap();
    assert!(alert(&b, three).is_some());
    assert!(runner::start_queued(&b.app).unwrap().is_empty());

    patch(&b, three, json!({"pr_link": PR9})).unwrap();
    assert_eq!(b.row(three).i("same_pr"), Some(one));
    assert!(alert(&b, three).is_none());
    assert!(waitsfor::merged(&b.app, Some(&b.row(three))).unwrap(), "the owner's merged state came with the link");
    assert_eq!(runner::start_queued(&b.app).unwrap(), vec![two]);
}

#[test]
fn a_pr_no_task_owns_links_as_the_tasks_own() {
    let b = new_board();
    let (_, _, three) = done_with_no_pr_and_a_waiter(&b);
    declined::sweep(&b.app).unwrap();
    patch(&b, three, json!({"pr_link": "https://github.com/acme/webapp/pull/12"})).unwrap();
    assert_eq!(b.row(three).i("pr_num"), Some(12));
    assert_eq!(b.row(three).i("same_pr"), None);
    assert!(alert(&b, three).is_none());
}

#[test]
fn a_pr_link_is_refused_where_it_cant_apply() {
    let b = new_board();
    let (one, two, three) = done_with_no_pr_and_a_waiter(&b);

    let (status, text) = patch(&b, three, json!({"pr_link": "not a link"})).unwrap_err();
    assert_eq!(status, 400, "{text}");
    let (status, text) = patch(&b, two, json!({"pr_link": PR9})).unwrap_err();
    assert_eq!(status, 409, "{text}");
    assert!(text.contains("isn't done"), "{text}");
    let (status, text) = patch(&b, one, json!({"pr_link": "https://github.com/acme/webapp/pull/12"})).unwrap_err();
    assert_eq!(status, 409, "{text}");
    assert!(text.contains("has a PR already"), "{text}");

    // A task that stacks can't also share a PR, but it can link one of its own.
    board::update_task(&b.app, three, vec![("pr_after", json!(two))]).unwrap();
    let (status, text) = patch(&b, three, json!({"pr_link": PR9})).unwrap_err();
    assert_eq!(status, 409, "{text}");
    assert!(text.contains("--stack-on none"), "{text}");
    patch(&b, three, json!({"pr_link": "https://github.com/acme/webapp/pull/12"})).unwrap();
    assert_eq!(b.row(three).i("pr_num"), Some(12));

    // Linking the same PR again changes nothing.
    patch(&b, three, json!({"pr_link": "https://github.com/acme/webapp/pull/12"})).unwrap();
}

#[test]
fn a_done_task_with_no_pr_raises_nothing_unless_a_merge_waits_on_it() {
    let b = new_board();
    let (_, two) = done_with_open_pr(&b);
    let three = b.task("Finish the endpoint");
    b.on("s3", three);
    submit(&b, "s3", json!({"summary": format!("Went out in {PR9}.")})).unwrap();
    declined::sweep(&b.app).unwrap();
    assert!(alert(&b, three).is_none(), "nothing waits on it");

    b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{three}")]}));
    declined::sweep(&b.app).unwrap();
    assert!(alert(&b, three).is_none(), "its waiter only needed it done");
    assert!(!declined::held(&b.app, &b.row(three)).unwrap().iter().any(|w| w.id() == two));
}

#[test]
fn a_task_finished_without_a_pr_on_purpose_holds_nobody() {
    let b = new_board();
    let (_, two) = done_with_open_pr(&b);
    let three = b.task("Finish the endpoint");
    b.on("s3", three);
    submit(&b, "s3", json!({"summary": "Nothing to ship.", "no_pr": "Already on main"})).unwrap();
    b.report("tb.wait_for", "s2", json!({"tasks": [format!("T{three}")], "merged": true}));
    declined::sweep(&b.app).unwrap();
    assert!(alert(&b, three).is_none());
    assert!(declined::held(&b.app, &b.row(three)).unwrap().iter().all(|w| w.id() != two));
}
