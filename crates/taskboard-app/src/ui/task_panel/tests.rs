//! The task panel against the web board: every panel state from `parity/golden/task.json` (what
//! the web showed, its controls, tooltips and fields), and every action's request.
use super::view::{self, Ctx, Ui};
use super::{Act, act, build, handoff_stamp, on_input, save_meta, ui};
use crate::app::{MainWindow, Panel, TaskTab};
use crate::parity::{self, golden, settle};
use crate::backend::Backend;
use gpui_kit::{AppContext as _, WindowHandle};
use serde_json::{Value, json};
use std::collections::HashMap;

fn ui_from(v: &Value) -> Ui {
    let strs = |k: &str| -> HashMap<String, String> {
        v[k].as_object().map(|o| o.iter().map(|(k, x)| (k.clone(), x.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default()
    };
    Ui {
        drafts: strs("drafts"),
        handoffs: strs("handoffs"),
        busy: v["busy"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
        notes: v["notes"]
            .as_object()
            .map(|o| o.iter().map(|(k, n)| (k.clone(), (n["text"].as_str().unwrap_or("").to_string(), n["err"].as_bool().unwrap_or(false)))).collect())
            .unwrap_or_default(),
        meta_edit: v["meta_edit"]
            .as_object()
            .map(|o| {
                o.iter()
                    .map(|(r, rows)| {
                        let rows = rows.as_array().cloned().unwrap_or_default();
                        (r.clone(), rows.iter().map(|p| (p[0].as_str().unwrap_or("").to_string(), p[1].as_str().unwrap_or("").to_string())).collect())
                    })
                    .collect()
            })
            .unwrap_or_default(),
        log_filter: v["log_filter"].as_str().unwrap_or("").to_string(),
        att_menu: v["att_menu"].as_str().map(str::to_string),
        att_edit: v["att_edit"].as_str().map(str::to_string),
        att_drop: v["att_drop"].as_bool().unwrap_or(false),
        term_menu: v["term_menu"].as_bool().unwrap_or(false),
        tl_open: v["tl_open"].as_array().map(|a| a.iter().filter_map(tl_step).collect()).unwrap_or_default(),
        ..Default::default()
    }
}

/// The panel tree for one golden case's input.
fn tree(i: &Value) -> view::Node {
    let ui = ui_from(&i["ui"]);
    let folds: HashMap<String, String> = i["ui"]["folds"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
    let trail: Vec<String> = i["from"].as_str().map(|f| f.split(',').filter(|x| !x.is_empty()).map(str::to_string).collect()).unwrap_or_default();
    let r = i["task"]["ref"].as_str().or(i["ref"].as_str()).unwrap_or("").to_string();
    let log = view::LogRows::of(&i["task"], ui.log_filter());
    let c = Ctx {
        state: &i["state"],
        task: i["task"].is_object().then_some(&i["task"]),
        card: i["card"].is_object().then_some(&i["card"]),
        r: &r,
        tab: i["tab"].as_str().unwrap_or("overview"),
        trail: &trail,
        err: i["err"].as_str(),
        ui: &ui,
        folds: &folds,
        log: &log,
    };
    view::panel(&c)
}

fn check_part(part: &str, f: impl Fn(&view::Node) -> Value) {
    let g = golden("task").only("panel");
    let mut bad = Vec::new();
    for c in &g.cases {
        let got = f(&tree(&c["input"]));
        if got != c["expect"][part] {
            bad.push(format!("  {}:\n    web {}\n    app {}", c["name"].as_str().unwrap_or("?"), c["expect"][part], got));
        }
    }
    assert!(bad.is_empty(), "{} of {} panel cases differ from the web board in {part}:\n{}", bad.len(), g.cases.len(), bad.join("\n"));
}

#[::core::prelude::v1::test]
fn panel_text_matches_web() {
    check_part("text", |n| json!(view::text(n)));
}

#[::core::prelude::v1::test]
fn panel_controls_match_web() {
    check_part("acts", |n| json!(view::acts(n)));
}

#[::core::prelude::v1::test]
fn panel_tooltips_match_web() {
    check_part("titles", |n| json!(view::titles(n)));
}

#[::core::prelude::v1::test]
fn panel_fields_match_web() {
    check_part("values", |n| json!(view::values(n)));
}

#[::core::prelude::v1::test]
fn helpers_match_web() {
    let g = golden("task");
    g.only("hoursOpenAt").check(|i| json!(view::hours_open_at(&i["state"])));
    g.only("sentNote").check(|i| json!(view::sent_note(&i["state"])));
}

#[::core::prelude::v1::test]
fn lost_task_shows_what_shipped_since() {
    let found = json!([{"what": "Tag v0.2.0", "at": "2026-10-09T21:21:00Z"},
                       {"what": "Release v0.2.0 published", "at": "2026-10-09T21:26:00Z", "url": "https://github.com/o/r/releases/tag/v0.2.0"}]);
    let task = json!({"ref": "T3", "id": 3, "title": "Release v0.2.0", "status": "needs", "lost": true, "needs_reason": "lost", "looks_finished": found});
    let text = json!(view::text(&tree(&json!({"state": {}, "task": task, "tab": "overview"})))).to_string();
    assert!(text.contains("Looks finished") && !text.contains("Terminal lost"), "{text}");
    assert!(text.contains(&format!("Release v0.2.0 published · {}", crate::fmt::hhmm("2026-10-09T21:26:00Z"))), "{text}");
    assert!(text.contains("Resume in a new terminal"), "{text}");
    let plain = json!(view::text(&tree(&json!({"state": {}, "task": {"ref": "T3", "id": 3, "status": "needs", "lost": true}, "tab": "overview"})))).to_string();
    assert!(plain.contains("Terminal lost") && !plain.contains("Looks finished"), "{plain}");
}

#[::core::prelude::v1::test]
fn a_design_task_wears_the_design_pill() {
    let task = json!({"ref": "T5", "id": 5, "title": "Concepts", "status": "working", "kind": "design"});
    let text = json!(view::text(&tree(&json!({"state": {}, "task": task, "tab": "overview"})))).to_string();
    assert!(text.contains(" Design "), "{text}");
    let task = json!({"ref": "T6", "id": 6, "title": "Build it", "status": "working", "kind": "agent"});
    let text = json!(view::text(&tree(&json!({"state": {}, "task": task, "tab": "overview"})))).to_string();
    assert!(!text.contains(" Design "), "{text}");
}

#[::core::prelude::v1::test]
fn a_card_shows_until_the_task_answers() {
    let card = json!({"ref": "T7", "id": 7, "title": "Passkey sign-in", "status": "working", "project": "web",
                      "pr": {"num": 12, "title": "Passkey sign-in", "url": "https://github.com/o/r/pull/12", "repo": "o/r"}});
    let text = json!(view::text(&tree(&json!({"state": {}, "ref": "T7", "card": card, "tab": "overview"})))).to_string();
    assert!(text.contains("Passkey sign-in") && text.contains("#12") && text.contains("Loading…"), "{text}");
    assert!(!text.contains("Overview"), "no tabs before the task answers: {text}");
    // Its own answer replaces the card.
    let task = json!({"ref": "T7", "id": 7, "title": "Passkey sign-in (full)", "status": "working"});
    let text = json!(view::text(&tree(&json!({"state": {}, "card": card, "task": task, "tab": "overview"})))).to_string();
    assert!(text.contains("(full)") && !text.contains("Loading…"), "{text}");
    // A failed fetch says why instead.
    let text = json!(view::text(&tree(&json!({"state": {}, "ref": "T7", "card": card, "err": "No such task.", "tab": "overview"})))).to_string();
    assert!(text.contains("No such task.") && !text.contains("Loading…"), "{text}");
}

#[::core::prelude::v1::test]
fn a_stuck_working_task_shows_the_stuck_banner() {
    let text = |task: Value| json!(view::text(&tree(&json!({"state": {}, "task": task, "tab": "overview"})))).to_string();
    let stuck = json!({"why": "Waits on a y/n prompt", "tail": "Continue? (y/n)"});
    let t = text(json!({"ref": "T5", "id": 5, "status": "working", "stuck": stuck}));
    assert!(t.contains("may be stuck") && t.contains("Waits on a y/n prompt"), "{t}");
    // A question outranks it: one banner per task.
    let t = text(json!({"ref": "T5", "id": 5, "status": "needs", "needs_reason": "question", "question": "Q?", "stuck": stuck}));
    assert!(!t.contains("may be stuck") && t.contains("Q?"), "{t}");
    let t = text(json!({"ref": "T5", "id": 5, "status": "working"}));
    assert!(!t.contains("may be stuck"), "{t}");
}

#[::core::prelude::v1::test]
fn write_an_answer_opens_its_terminal_or_a_new_one() {
    let acts = |task: Value| json!(view::acts(&tree(&json!({"state": {}, "task": task, "tab": "overview"})))).to_string();
    // No terminal but a question: focus reopens one that asks it again, keeping the question.
    let a = acts(json!({"ref": "T5", "id": 5, "status": "needs", "needs_reason": "question", "question": "Add the generated patterns?"}));
    assert!(a.contains("\"focus\"") && !a.contains("\"resume") && !a.contains("\"answer\""), "{a}");
    // No terminal and nothing asked: a new one picks up from its handoff.
    let a = acts(json!({"ref": "T5", "id": 5, "status": "needs", "needs_reason": "attention"}));
    assert!(a.contains("\"resume:fresh\"") && !a.contains("\"focus\""), "{a}");
    // With its terminal open, the terminal is the action.
    let a = acts(json!({"ref": "T5", "id": 5, "status": "needs", "needs_reason": "question", "question": "Q?", "session": {"status": "idle"}}));
    assert!(a.contains("\"focus\"") && !a.contains("\"resume") && !a.contains("\"answer\""), "{a}");
    // A human task has no terminal to answer in: its box holds Done and Talk it over instead.
    let a = acts(json!({"ref": "H5", "id": 5, "kind": "human", "status": "needs", "needs_reason": "human", "question": "Your task: Sign it. Follow its steps, then press Done.", "detail": "1. Sign it."}));
    assert!(a.contains("\"did\"") && a.contains("\"talk\"") && !a.contains("\"focus\"") && !a.contains("\"resume"), "{a}");
}

#[::core::prelude::v1::test]
fn a_question_shows_once_in_the_banner() {
    // Asked just now, so its time reads as a clock time rather than a date.
    let asked = chrono::Utc::now() - chrono::Duration::minutes(1);
    let iso = |t: chrono::DateTime<chrono::Utc>| t.format("%Y-%m-%dT%H:%M:%SZ").to_string();
    let (started_at, updated_at) = (iso(asked - chrono::Duration::minutes(140)), iso(asked));
    let task = json!({"ref": "T5", "id": 5, "status": "needs", "needs_reason": "question", "question": "Add the generated patterns?",
                      "started_at": started_at, "updated_at": updated_at,
                      "ask": {"questions": [{"question": "Add the generated patterns?", "options": [{"label": "Yes, add them"}, {"label": "No"}]}]}});
    let panel = tree(&json!({"state": {}, "task": task, "tab": "overview"}));
    let text = json!(view::text(&panel)).to_string();
    assert_eq!(text.matches("Add the generated patterns?").count(), 1, "{text}");
    assert!(!text.contains("Needs you"), "no Needs-you pill beside the banner: {text}");
    let at = crate::fmt::hhmm(&updated_at);
    assert!(text.contains("Waiting on you") && text.contains("after 2h 20m of work") && text.contains(&at), "{text}");
    assert!(at.ends_with("AM") || at.ends_with("PM"), "{at}");
    let a = json!(view::acts(&panel)).to_string();
    assert!(a.contains("\"answer-opt:Yes, add them\"") && a.contains("\"answer-opt:No\""), "{a}");
    // A lost terminal waits too, but nothing was asked.
    let lost = json!({"ref": "T5", "id": 5, "status": "needs", "lost": true, "updated_at": "2026-10-09T11:34:00Z"});
    let text = json!(view::text(&tree(&json!({"state": {}, "task": lost, "tab": "overview"})))).to_string();
    assert!(text.contains("Waiting on you") && !text.contains("Asked"), "{text}");
}

// ------------------------------------------------------------------ actions

/// Open task `r` in the panel and let its detail load.
fn open(cx: &mut gpui_kit::TestAppContext, w: &WindowHandle<MainWindow>, r: &str) {
    w.update(cx, |m, _, cx| m.open_task(r, cx)).unwrap();
    settle(cx);
}

/// The control with this act (and arg) in the panel as it is now; panics if the panel doesn't offer it.
fn control(cx: &mut gpui_kit::TestAppContext, w: &WindowHandle<MainWindow>, a: &str, arg: Option<&str>) -> Act {
    w.update(cx, |m, _, _| {
        let tree = build(m).expect("panel open");
        view::find_act(&tree, a, arg).cloned().unwrap_or_else(|| panic!("the panel doesn't offer {a} {arg:?}; it offers {:?}", view::acts(&tree)))
    })
    .unwrap()
}

/// Press a control the panel offers, then let the request land.
fn press(cx: &mut gpui_kit::TestAppContext, w: &WindowHandle<MainWindow>, a: &str, arg: Option<&str>) {
    let a = control(cx, w, a, arg);
    w.update(cx, |m, _, cx| act(m, &a, cx)).unwrap();
    settle(cx);
}

fn typed(cx: &mut gpui_kit::TestAppContext, w: &WindowHandle<MainWindow>, key: &str, text: &str) {
    w.update(cx, |m, _, _| on_input(m, key, text.to_string())).unwrap();
}

fn note(grp: &str) -> Option<(String, bool)> {
    ui(|u| u.notes.get(grp).cloned())
}

fn sent_note(cx: &mut gpui_kit::TestAppContext, w: &WindowHandle<MainWindow>) -> String {
    w.update(cx, |m, _, _| view::sent_note(m.state()).to_string()).unwrap()
}

#[gpui_kit::test]
fn cmd_c_copies_a_selection_made_after_typing_in_a_field(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, VisualTestContext, point, px};
    let (w, rec) = parity::window(cx);
    rec.board().post("tasks/T2/meta", json!({"meta": [["Owner", "Sam"]]})).unwrap();
    open(cx, &w, "T2");
    press(cx, &w, "tab", Some("context"));
    let cx = &mut VisualTestContext::from_window(w.into(), cx);
    let width = cx.update(|window, cx| {
        drop(window.draw(cx));
        window.viewport_size().width
    });
    // The reader typed in one of the panel's fields: it still has focus.
    w.update(cx, |m, window, cx| {
        let field = m.task_panel.fields["meta:T2:0:1"].focus.clone();
        window.focus(&field, cx);
    })
    .unwrap();
    // Drag across the title: 24px in from the panel's left edge, under the pills.
    let left = width - px(super::WIDTH) + px(24.);
    cx.simulate_mouse_down(point(left + px(2.), px(68.)), MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(point(left + px(80.), px(68.)), Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(point(left + px(80.), px(68.)), MouseButton::Left, Modifiers::default());
    let picked = cx.update(|window, cx| gpui_kit::base::TextSelection::selected_text(window, cx));
    assert!(!picked.is_empty(), "the drag selected some of the title");
    cx.simulate_keystrokes("cmd-c");
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()), Some(picked));
}

#[gpui_kit::test]
fn a_task_opens_from_its_row_and_loads_on_its_own(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    let title = rec.board().get("tasks/T3", &[]).unwrap()["title"].as_str().unwrap().to_string();
    // The board's column already has T3: its panel shows the row's title before anything answers.
    w.update(cx, |m, _, cx| m.open_task("T3", cx)).unwrap();
    let first = w.update(cx, |m, _, _| view::text(&build(m).expect("panel open"))).unwrap();
    assert!(first.contains(&title) && first.contains("Loading…"), "{first}");
    // Its own fetch lands even when the rest of the refresh can't.
    rec.fail("state", 500, "down");
    settle(cx);
    let after = w.update(cx, |m, _, _| view::text(&build(m).expect("panel open"))).unwrap();
    assert!(after.contains("Full log") && !after.contains("Loading…"), "{after}");
}

#[gpui_kit::test]
fn an_option_answers_with_its_label(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    // T3's terminal asks again, this time with options.
    let ask = json!({"event": "tb.question", "session": "fake-s2", "claude_session": "c-s2", "cwd": "",
                     "text": "Should removing the last passkey require a password re-check?", "options": ["Yes: Ask for the password first", "No"]});
    rec.board().post("report", ask).unwrap();
    w.update(cx, |m, _, cx| m.refresh(cx)).unwrap();
    open(cx, &w, "T3");
    press(cx, &w, "answer-opt", Some("Yes"));
    assert_eq!(rec.last("tasks/T3/answer"), Some(json!({"text": "Yes", "when": "now"})));
    assert_eq!(note("ask:T3").map(|n| n.0), Some(sent_note(cx, &w)));
}

/// T2's terminal shows Claude's picker with `questions`.
fn ask_picker(cx: &mut gpui_kit::TestAppContext, w: &WindowHandle<MainWindow>, rec: &parity::Recording, questions: Value) {
    let ask = json!({"event": "hook.ask", "session": "fake-s1", "claude_session": "c-s1", "cwd": "", "tool_input": {"questions": questions}});
    rec.board().post("report", ask).unwrap();
    w.update(cx, |m, _, cx| m.refresh(cx)).unwrap();
    open(cx, w, "T2");
}

#[gpui_kit::test]
fn a_one_question_picker_answers_in_a_click(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    ask_picker(cx, &w, &rec, json!([{"question": "Pick a fruit", "header": "Fruit", "multiSelect": false, "options": [{"label": "Apple", "description": ""}, {"label": "Pear", "description": ""}]}]));
    press(cx, &w, "pick", Some("1"));
    assert_eq!(rec.last("tasks/T2/pick"), Some(json!({"answers": [[1]]})));
    // No Midna in the test board: the refusal says why, under the options.
    let (msg, err) = note("ask:T2").expect("a note");
    assert!(err && msg.contains("Midna"), "{msg}");
}

#[gpui_kit::test]
fn a_picker_s_choices_toggle_and_send_once_each_question_has_one(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    ask_picker(cx, &w, &rec, json!([
        {"question": "Pick a fruit", "header": "Fruit", "multiSelect": false, "options": [{"label": "Apple", "description": ""}, {"label": "Pear", "description": ""}]},
        {"question": "Pick sizes", "header": "Size", "multiSelect": true, "options": [{"label": "S", "description": ""}, {"label": "M", "description": ""}, {"label": "L", "description": ""}]}]));
    let acts = |cx: &mut gpui_kit::TestAppContext| w.update(cx, |m, _, _| view::acts(&build(m).unwrap())).unwrap();
    assert!(acts(cx).contains(&"pick!".to_string()), "Send waits for a choice per question: {:?}", acts(cx));
    // A single-select question keeps its last choice; a multi-select one keeps them all.
    for k in ["0:0", "0:1", "1:2", "1:0", "1:1", "1:1"] {
        press(cx, &w, "pick-opt", Some(k));
    }
    assert_eq!(ui(|u| u.drafts.get("pick:T2").cloned()), Some("0:1,1:2,1:0".into()));
    assert!(acts(cx).contains(&"pick".to_string()), "{:?}", acts(cx));
    press(cx, &w, "pick", Some(""));
    assert_eq!(rec.last("tasks/T2/pick"), Some(json!({"answers": [[1], [0, 2]]})));
    assert!(ui(|u| u.drafts.contains_key("pick:T2")), "a refused pick keeps the choices");
}

#[gpui_kit::test]
fn a_busy_button_sends_once(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    open(cx, &w, "T5");
    let a = control(cx, &w, "start", Some("new"));
    w.update(cx, |m, _, cx| {
        act(m, &a, cx);
        // Still sending: the web's busy button reads "Sending…" and ignores a second press.
        let tree = build(m).unwrap();
        assert!(view::acts(&tree).contains(&"start:new!".to_string()), "busy start is disabled: {:?}", view::acts(&tree));
        act(m, &a, cx);
    })
    .unwrap();
    settle(cx);
    assert_eq!(rec.posts().iter().filter(|(p, _)| p == "tasks/T5/start").count(), 1);
    assert_eq!(rec.last("tasks/T5/start"), Some(json!({"mode": "new"})));
}

#[gpui_kit::test]
fn start_buttons(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    open(cx, &w, "T5");
    press(cx, &w, "start", Some("queue"));
    assert_eq!(rec.last("tasks/T5/start"), Some(json!({"mode": "queue"})));
    assert_eq!(note("act:T5").map(|n| n.0), Some(sent_note(cx, &w)));
}

#[gpui_kit::test]
fn planned_task_queues(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    open(cx, &w, "T4");
    press(cx, &w, "queue-planned", None);
    assert_eq!(rec.last("tasks/T4"), Some(json!({"status": "queued"})));
    assert_eq!(note("act:T4"), Some(("Queued".into(), false)));
}

#[gpui_kit::test]
fn done_task_has_no_queue_again(cx: &mut gpui_kit::TestAppContext) {
    let (w, _rec) = parity::window(cx);
    open(cx, &w, "T1");
    assert!(w.update(cx, |m, _, _| view::find_act(&build(m).unwrap(), "requeue", None).is_none()).unwrap());
}

#[gpui_kit::test]
fn done_card_closes_its_terminal_behind_the_menu(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    // T2 holds a live terminal; finishing it leaves that terminal open behind a Done card.
    rec.board().app().db.x("UPDATE tasks SET status = 'done', finished_at = ? WHERE id = 2", taskboardd::p![taskboardd::util::now_iso()]).unwrap();
    open(cx, &w, "T2");
    assert!(w.update(cx, |m, _, _| view::find_act(&build(m).unwrap(), "close-term", None).is_none()).unwrap(), "hidden while the menu is shut");
    press(cx, &w, "term-menu", None);
    press(cx, &w, "close-term", None);
    assert_eq!(rec.last("tasks/T2/close-terminal"), Some(json!({"force": false})));
    assert!(!ui(|u| u.term_menu), "picking the item closes the menu");
    assert!(note("act:T2").is_some(), "the result lands on the Done card");
}

#[gpui_kit::test]
fn manage_box_actions(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    open(cx, &w, "T2");
    press(cx, &w, "focus", None);
    assert_eq!(rec.last("tasks/T2/focus"), Some(json!({})));
    // Detach and Close terminal wait behind the ⋯ menu, which closes once one is picked.
    assert!(w.update(cx, |m, _, _| view::find_act(&build(m).unwrap(), "detach", None).is_none()).unwrap(), "hidden while the menu is shut");
    press(cx, &w, "term-menu", None);
    press(cx, &w, "close-term", Some("force"));
    assert_eq!(rec.last("tasks/T2/close-terminal"), Some(json!({"force": true})));
    assert!(!ui(|u| u.term_menu), "picking an item closes the menu");
    press(cx, &w, "term-menu", None);
    press(cx, &w, "detach", None);
    assert_eq!(rec.last("tasks/T2/detach"), Some(json!({})));
    assert_eq!(note("manage:T2"), Some(("Detached. It’s back in the queue.".into(), false)));
}

#[gpui_kit::test]
fn lost_task_resumes(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    w.update(cx, |m, _, cx| {
        act(m, &Act::new("resume", "fresh", "T3", "lost:T3"), cx);
        act(m, &Act::new("resume", "reopen", "T3", "lost:T3"), cx);
        act(m, &Act::new("resume", "finish", "T3", "lost:T3"), cx);
    })
    .unwrap();
    settle(cx);
    let resumes: Vec<Value> = rec.posts().into_iter().filter(|(p, _)| p == "tasks/T3/resume").map(|(_, b)| b).collect();
    assert_eq!(resumes, vec![json!({"mode": "fresh"}), json!({"mode": "reopen"}), json!({"mode": "finish"})]);
}

#[gpui_kit::test]
fn details_fields_edit_in_place(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    rec.board().post("tasks/T2/meta", json!({"meta": [["Figma", "https://figma.com/a"], ["Owner", "Sam"]]})).unwrap();
    open(cx, &w, "T2");
    press(cx, &w, "tab", Some("context"));
    typed(cx, &w, "meta:T2:1:1", " Alex ");
    w.update(cx, |m, _, cx| save_meta(m, "T2", "Saved", cx)).unwrap();
    settle(cx);
    assert_eq!(rec.last("tasks/T2/meta"), Some(json!({"meta": [["Figma", "https://figma.com/a"], ["Owner", "Alex"]]})));
    assert_eq!(note("meta:T2"), Some(("Saved".into(), false)));
    press(cx, &w, "meta-del", Some("0"));
    assert_eq!(rec.last("tasks/T2/meta"), Some(json!({"meta": [["Owner", "Alex"]]})));
    assert_eq!(note("meta:T2"), Some(("Removed".into(), false)));
}

#[gpui_kit::test]
fn trail_and_back(cx: &mut gpui_kit::TestAppContext) {
    let (w, _rec) = parity::window(cx);
    open(cx, &w, "T2");
    // Opening another task from inside the panel remembers this one (`?from=`).
    w.update(cx, |m, _, cx| act(m, &Act::new("open-task", "", "T4", ""), cx)).unwrap();
    settle(cx);
    w.update(cx, |m, _, _| assert!(matches!(&m.panel, Some(Panel::Task { r, .. }) if r == "T4"))).unwrap();
    press(cx, &w, "task-back", None);
    w.update(cx, |m, _, _| assert!(matches!(&m.panel, Some(Panel::Task { r, .. }) if r == "T2"))).unwrap();
    // Back at the start: no back button.
    let has_back = w.update(cx, |m, _, _| view::find_act(&build(m).unwrap(), "task-back", None).is_some()).unwrap();
    assert!(!has_back);
    // Opening a task from elsewhere starts a new trail.
    w.update(cx, |m, _, cx| act(m, &Act::new("open-task", "", "T4", ""), cx)).unwrap();
    open(cx, &w, "T1");
    let has_back = w.update(cx, |m, _, _| view::find_act(&build(m).unwrap(), "task-back", None).is_some()).unwrap();
    assert!(!has_back);
    press(cx, &w, "close-panel", None);
    w.update(cx, |m, _, _| assert!(m.panel.is_none())).unwrap();
}

#[gpui_kit::test]
fn tabs_and_log_filter(cx: &mut gpui_kit::TestAppContext) {
    let (w, _rec) = parity::window(cx);
    open(cx, &w, "T2");
    press(cx, &w, "tab", Some("log"));
    press(cx, &w, "log-filter", Some("status"));
    w.update(cx, |m, _, _| assert!(matches!(&m.panel, Some(Panel::Task { tab: TaskTab::Log, .. })))).unwrap();
    // The filter stays when another task opens (the web's S.logFilter); the tab goes back to Overview.
    open(cx, &w, "T1");
    w.update(cx, |m, _, _| assert!(matches!(&m.panel, Some(Panel::Task { tab: TaskTab::Overview, .. })))).unwrap();
    press(cx, &w, "tab", Some("log"));
    let on = w.update(cx, |m, _, _| view::acts(&build(m).unwrap())).unwrap();
    assert!(on.contains(&"log-filter:status".to_string()));
    assert_eq!(ui(|u| u.log_filter.clone()), "status");
}

#[gpui_kit::test]
fn folds_are_saved(cx: &mut gpui_kit::TestAppContext) {
    let (w, _rec) = parity::window(cx);
    let task = json!({"ref": "T9", "id": 9, "title": "x", "status": "working", "project": "webapp", "stuck": {"why": "Waits on a y/n prompt", "tail": "Continue? (y/n)"}});
    w.update(cx, |m, _, _| {
        m.panel = Some(Panel::Task { r: "T9".into(), tab: TaskTab::Overview });
        m.data.task = Some(task);
    })
    .unwrap();
    let text = |cx: &mut gpui_kit::TestAppContext| w.update(cx, |m, _, _| view::text(&build(m).unwrap())).unwrap();
    crate::prefs::set("tb.fold.tail", json!("closed"));
    assert!(!text(cx).contains("Continue? (y/n)"));
    crate::prefs::set("tb.fold.tail", json!("open"));
    assert!(text(cx).contains("Continue? (y/n)"), "the fold reads its saved state");
}

#[gpui_kit::test]
fn renders_every_tab(cx: &mut gpui_kit::TestAppContext) {
    let (w, _rec) = parity::window(cx);
    for r in ["T1", "T2", "T3", "T4", "T5"] {
        open(cx, &w, r);
        for tab in ["what", "context", "log", "overview"] {
            press(cx, &w, "tab", Some(tab));
            let _ = cx.update_window(w.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            });
        }
    }
}

#[gpui_kit::test]
fn page_links_keep_the_panel_beside_them(cx: &mut gpui_kit::TestAppContext) {
    use crate::app::Page;
    let (w, rec) = parity::window(cx);
    open(cx, &w, "T2");
    // The PR bar's "Terminal ›": the terminal itself, in Midna; the board stays put.
    let before = w.update(cx, |m, _, _| m.page.clone()).unwrap();
    w.update(cx, |m, _, cx| super::go(m, &view::Go::Session("fake-s1".into()), cx)).unwrap();
    settle(cx);
    assert_eq!(rec.last("sessions/fake-s1/focus"), Some(json!({})));
    w.update(cx, |m, _, _| {
        assert!(matches!(&m.panel, Some(Panel::Task { r, .. }) if r == "T2"));
        assert_eq!(m.page, before);
    })
    .unwrap();
    // A found issue (`#/?issue=B1`): the board with that issue's panel.
    open(cx, &w, "T1");
    w.update(cx, |m, _, cx| super::go(m, &view::Go::Issue("B1".into()), cx)).unwrap();
    settle(cx);
    w.update(cx, |m, _, _| {
        assert_eq!(m.page, Page::Board);
        assert!(matches!(&m.panel, Some(Panel::Issue { r }) if r == "B1"));
    })
    .unwrap();
    // The goal's name: its page, the panel still beside it.
    open(cx, &w, "T2");
    w.update(cx, |m, _, cx| super::go(m, &view::Go::Goal("G1".into()), cx)).unwrap();
    w.update(cx, |m, _, _| {
        assert_eq!(m.page, Page::Goal("G1".into()));
        assert!(matches!(&m.panel, Some(Panel::Task { r, .. }) if r == "T2"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn header_chips_drop_the_attachments_and_open_the_project(cx: &mut gpui_kit::TestAppContext) {
    use crate::app::Page;
    let (w, rec) = parity::window(cx);
    rec.board().post("tasks/T2/attachments", json!({"kind": "design", "title": "Mock", "url": "https://figma.com/x"})).unwrap();
    open(cx, &w, "T2");
    // "Attached N" opens its list and a second tap closes it.
    press(cx, &w, "att-drop", None);
    assert!(ui(|u| u.att_drop));
    press(cx, &w, "att-drop", None);
    assert!(!ui(|u| u.att_drop));
    // Picking a row closes the list.
    press(cx, &w, "att-drop", None);
    w.update(cx, |m, _, cx| super::go(m, &view::Go::Url(String::new()), cx)).unwrap();
    assert!(!ui(|u| u.att_drop));
    // The project chip: the Projects page on that project.
    let project = control(cx, &w, "open-project", None).arg;
    assert!(!project.is_empty());
    press(cx, &w, "open-project", Some(&project));
    w.update(cx, |m, _, _| {
        assert_eq!(m.page, Page::Projects);
        assert_eq!(m.projects.selected.as_deref(), Some(project.as_str()));
    })
    .unwrap();
}

#[test]
fn attachment_icons_follow_the_file_type() {
    let t = |u: &str| view::att_file_type(u).map(|(t, ext)| format!("{t} {ext}"));
    assert_eq!(t("/Users/me/shots/after_home_top.MOV").as_deref(), Some("video mov"));
    assert_eq!(t("~/shots/before.png").as_deref(), Some("image png"));
    assert_eq!(t("https://example.com/r/report.pdf?dl=1").as_deref(), Some("pdf pdf"));
    assert_eq!(t("/tmp/out.tar.gz").as_deref(), Some("archive gz"));
    // A page, a host that looks like a file, a dotfile and an unknown extension: the kind's icon.
    assert_eq!(t("https://claude.ai/code/artifact/0b1c"), None);
    assert_eq!(t("https://install.sh"), None);
    assert_eq!(t("/Users/me/.env"), None);
    assert_eq!(t("/Users/me/thing.xyz"), None);
}

#[test]
fn an_attachment_summary_rides_last_in_its_dropdown_item() {
    use super::view::Node;
    let last = |a: Value| match view::drop_item(&a) {
        Node::El { kids, .. } if kids.len() == 4 => match &kids[3] {
            Node::Text { s, .. } => Some(s.clone()),
            _ => None,
        },
        _ => None,
    };
    let a = json!({"id": 1, "kind": "doc", "title": "Spec", "url": "https://example.com/spec", "summary": "The login flow"});
    assert_eq!(last(a).as_deref(), Some("The login flow"));
    assert_eq!(last(json!({"id": 1, "kind": "doc", "title": "Spec", "url": "https://example.com/spec", "summary": null})), None);
}

#[gpui_kit::test]
fn back_from_a_link_walks_the_history(cx: &mut gpui_kit::TestAppContext) {
    use crate::app::Page;
    let (w, _rec) = parity::window(cx);
    let at = |m: &MainWindow| (m.page.clone(), match &m.panel {
        Some(Panel::Task { r, .. }) => Some(r.clone()),
        _ => None,
    });
    let g1 = || Page::Goal("G1".into());
    w.update(cx, |m, _, cx| m.go(g1(), cx)).unwrap();
    // A goal link keeps the task open: one ⌘← comes back to the page it was opened on, the next closes it.
    for link in [view::Go::Goal("G2".into())] {
        open(cx, &w, "T1");
        w.update(cx, |m, _, cx| super::go(m, &link, cx)).unwrap();
        settle(cx);
        w.update(cx, |m, _, cx| {
            assert_eq!(at(m).1, Some("T1".into()), "{link:?} keeps the task");
            m.go_back(cx);
            assert_eq!(at(m), (g1(), Some("T1".into())), "{link:?}: back to the page under the task");
            m.go_back(cx);
            assert_eq!(at(m), (g1(), None), "{link:?}: then the goal");
        })
        .unwrap();
        settle(cx);
    }
    // A linked task's "Back to T1" steps back, not forward: ⌘← then reaches the goal, ⌘→ the linked task.
    open(cx, &w, "T1");
    w.update(cx, |m, _, cx| act(m, &Act::new("open-task", "", "T2", ""), cx)).unwrap();
    w.update(cx, |m, _, cx| {
        act(m, &Act::new("task-back", "", "T2", ""), cx);
        assert_eq!(at(m), (g1(), Some("T1".into())));
        m.go_back(cx);
        assert_eq!(at(m), (g1(), None));
        m.go_forward(cx);
        m.go_forward(cx);
        assert_eq!(at(m), (g1(), Some("T2".into())));
    })
    .unwrap();
}

/// A done task whose green PR waits for its owner.
fn awaiting_review(awaiting: bool) -> Value {
    json!({"ref": "T1", "id": 1, "title": "Add passkeys", "status": "done", "project": "webapp",
           "pr": {"num": 42, "repo": "acme/webapp", "url": "https://github.com/acme/webapp/pull/42", "state": "OPEN",
                  "stage": {"phase": "review", "label": "Ready for review", "awaiting_you": awaiting}}})
}

#[::core::prelude::v1::test]
fn reviewed_button_shows_only_while_the_pr_waits_for_you() {
    for (awaiting, shown) in [(true, true), (false, false)] {
        let task = awaiting_review(awaiting);
        let tree = tree(&json!({"task": task, "state": {}, "ui": {}}));
        assert_eq!(view::acts(&tree).iter().any(|a| a.starts_with("pr-reviewed")), shown, "awaiting_you {awaiting}: {:?}", view::acts(&tree));
    }
}

#[gpui_kit::test]
fn reviewed_button_tells_the_board(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    open(cx, &w, "T1");
    w.update(cx, |m, _, cx| act(m, &Act::new("pr-reviewed", "", "T1", "pr:T1"), cx)).unwrap();
    settle(cx);
    assert_eq!(rec.last("tasks/T1/pr/reviewed"), Some(json!({})));
    assert_eq!(note("pr:T1"), Some(("Marked reviewed".into(), false)));
}

#[::core::prelude::v1::test]
fn a_failure_cleared_as_not_ours_shows_its_reason_and_proof() {
    let mut task = awaiting_review(false);
    task["pr"]["stage"]["not_ours"] = json!({"title": "Flaky login e2e", "reason": "The base branch fails it on its last five builds.",
                                             "checks": ["e2e"], "proof": ["https://ci.example.com/b/1"]});
    let text = view::text(&tree(&json!({"task": task, "state": {}, "ui": {}})));
    for want in ["Failed, but not because of this PR", "Flaky login e2e", "The base branch fails it on its last five builds.", "e2e", "https://ci.example.com/b/1"] {
        assert!(text.contains(want), "{want} in {text}");
    }
    let at = (chrono::Utc::now() - chrono::Duration::minutes(12)).to_rfc3339();
    task["pr"]["stage"]["not_ours"]["links"] = json!([{"url": "https://ci.example.com/b/1", "label": "ci.example.com"}]);
    task["pr"]["stage"]["not_ours"]["at"] = json!(at);
    let text = view::text(&tree(&json!({"task": task, "state": {}, "ui": {}})));
    assert!(text.contains("ci.example.com") && !text.contains("https://ci.example.com/b/1"), "the link goes by its label: {text}");
    assert!(text.contains("Checked 12m ago"), "{text}");
    task["pr"]["stage"]["not_ours"] = Value::Null;
    assert!(!view::text(&tree(&json!({"task": task, "state": {}, "ui": {}}))).contains("not because of this PR"));
}

// ------------------------------------------------------------------ the PR bar, the PR plan, canceled PRs

/// A done task whose PR the board describes with `pr.bar` (the five-step bar).
fn with_bar(bar: Value, stage: Value, review: &str) -> Value {
    json!({"ref": "T4", "id": 4, "title": "Use the endpoint", "status": "done", "project": "webapp",
           "pr": {"num": 13, "repo": "webapp", "url": "https://github.com/acme/webapp/pull/13", "state": "OPEN",
                  "checks": "pass", "review": review, "stage": stage, "bar": bar}})
}

fn panel_text(task: Value, state: Value) -> String {
    view::text(&tree(&json!({"task": task, "state": state, "ui": {}})))
}

#[::core::prelude::v1::test]
fn the_pr_bar_has_every_step_once_the_board_sends_it() {
    let bar = json!({"build_url": "https://ci.example/9", "you": "waiting", "approvals": 1, "reviewers": 2, "new_comments": 0,
                     "stacks_on": {"ref": "T3", "num": 12, "url": "https://github.com/acme/webapp/pull/12", "merged": false},
                     "author_review": {"bar": "Critic", "headline": "2 open findings", "open": 2, "passed": false}, "reviewer_rows": []});
    let task = with_bar(bar, json!({"phase": "waits", "label": "Waits on base"}), "pending");
    let tree = tree(&json!({"task": task, "state": {}, "ui": {}}));
    let text = view::text(&tree);
    for want in ["Critic 2 open findings", "Checks Passed", "You Waiting on you", "Review 1 of 2", "Pending merge Waits on base", "Stacks on T3 PR #12", "Waiting on your review Mark reviewed ›"] {
        assert!(text.contains(want), "{want:?} in {text}");
    }
    let acts = view::acts(&tree);
    assert!(acts.iter().any(|a| a == "open-task") && acts.iter().any(|a| a == "pr-reviewed"), "{acts:?}");
    assert!(!text.contains("I reviewed it"), "{text}");
}

#[::core::prelude::v1::test]
fn the_stack_card_says_how_it_merges_and_the_parent_waits_on_its_stack() {
    let so = |merge: &str| json!({"ref": "T3", "num": 12, "url": "https://github.com/acme/webapp/pull/12", "merged": false, "merge": merge});
    let tail = panel_text(with_bar(json!({"stacks_on": so("tail")}), json!({"phase": "waits", "label": "Waits on base"}), "approved"), json!({}));
    assert!(tail.contains("Stacks on T3 PR #12 Tail merge") && tail.contains("Pending merge Waits on base"), "{tail}");
    let head = panel_text(with_bar(json!({"stacks_on": so("head")}), json!({"phase": "merge", "label": "Merging"}), "approved"), json!({}));
    assert!(head.contains("Stacks on T3 PR #12 Head merge") && !head.contains("Waits on base"), "{head}");
    // What the base is at, and the Merge step to its running build; no builds yet isn't "Not needed".
    let wb = json!({"ref": "T3", "line": "T3's checks running, 2 of 5 passed", "url": "https://ci.example/7"});
    let bar = json!({"stacks_on": so("tail"), "waits_on_base": wb, "checks": "none_yet"});
    let task = with_bar(bar.clone(), json!({"phase": "waits", "label": "Waits on base"}), "approved");
    let text = panel_text(task.clone(), json!({}));
    assert!(text.contains("Pending merge Waits on T3") && text.contains("T3's checks running, 2 of 5 passed") && text.contains("Checks None yet"), "{text}");
    let merge = view::pr_steps_full(&task["pr"], &bar).pop().unwrap();
    assert!(format!("{:?}", merge.link).contains("https://ci.example/7"), "{:?}", merge.link);
    let why = "T5's PR #14 head-merges into it and has to merge first";
    let parent = panel_text(with_bar(json!({"waits_on_stack": why}), json!({"phase": "stack", "label": "Waits on stack"}), "approved"), json!({}));
    assert!(parent.contains("Pending merge Waits on stack") && parent.contains(why), "{parent}");
}

#[::core::prelude::v1::test]
fn the_review_step_counts_approvals_against_the_ones_needed() {
    let review = |approvals: i64, reviewers: i64, need: Value| {
        let bar = json!({"approvals": approvals, "reviewers": reviewers, "need": need, "new_comments": 0, "reviewer_rows": []});
        let p = with_bar(bar.clone(), json!({"phase": "review", "label": "Awaiting reviews"}), "pending")["pr"].clone();
        view::pr_steps_full(&p, &bar).into_iter().find(|s| s.name == "Review").map(|s| (s.sub.unwrap_or_default(), s.st)).unwrap()
    };
    assert_eq!(review(2, 3, json!(2)), ("2 of 2".into(), view::StepSt::Done), "enough approvals, though one reviewer hasn't");
    assert_eq!(review(1, 3, json!(2)), ("1 of 2".into(), view::StepSt::Wait));
    assert_eq!(review(1, 2, Value::Null), ("1 of 2".into(), view::StepSt::Wait), "no count: out of the reviewers");
    assert_eq!(review(2, 2, Value::Null).1, view::StepSt::Done);
    assert_eq!(review(3, 3, json!(2)), ("2 of 2".into(), view::StepSt::Done), "more than it needs never reads 3 of 2");
}

#[::core::prelude::v1::test]
fn merge_steps_sit_before_the_merge_and_a_skip_shows_why() {
    // #477: steps that hold only the merge, judged on the PR's head.
    let merge_steps = json!([
        {"name": "Staging", "state": "skipped", "why": "docs only"},
        {"name": "Notes", "state": "passed"},
        {"name": "Smoke", "state": "todo"},
    ]);
    let bar = json!({"approvals": 1, "reviewers": 1, "new_comments": 0, "reviewer_rows": [], "merge_steps": merge_steps});
    let p = with_bar(bar.clone(), json!({"phase": "steps", "label": "Waits on steps"}), "approved")["pr"].clone();
    let got: Vec<_> = view::pr_steps_full(&p, &bar).into_iter().map(|s| (s.name, s.sub.unwrap_or_default(), s.st)).collect();
    let tail = &got[got.len() - 4..];
    assert_eq!(tail[0], ("Staging".into(), "Skipped: docs only".into(), view::StepSt::Done));
    assert_eq!(tail[1], ("Notes".into(), "Passed".into(), view::StepSt::Done));
    assert_eq!(tail[2], ("Smoke".into(), "To do".into(), view::StepSt::Ask));
    assert_eq!(tail[3], ("Merge".into(), "Waits on steps".into(), view::StepSt::Wait));
}

#[::core::prelude::v1::test]
fn with_no_count_the_host_s_approval_finishes_the_review_step() {
    // #150: no approvals count, the host decided APPROVED with 1 of 3 reviewers approving.
    let review = |rv: &str, need: Value| {
        let bar = json!({"approvals": 1, "reviewers": 3, "need": need, "new_comments": 0, "reviewer_rows": []});
        let p = with_bar(bar.clone(), json!({"phase": "merge", "label": "Ready to merge"}), rv)["pr"].clone();
        view::pr_steps_full(&p, &bar).into_iter().find(|s| s.name == "Review").map(|s| (s.sub.unwrap_or_default(), s.st)).unwrap()
    };
    assert_eq!(review("approved", Value::Null), ("Approved".into(), view::StepSt::Done));
    assert_eq!(review("pending", Value::Null), ("1 of 3".into(), view::StepSt::Wait), "not approved yet: out of the reviewers");
    assert_eq!(review("approved", json!(2)), ("1 of 2".into(), view::StepSt::Wait), "a count set goes by the count");
}

#[::core::prelude::v1::test]
fn without_a_bar_the_review_step_waits_short_of_the_approvals_needed() {
    let review = |review: &str, have: i64, need: Value| {
        let p = json!({"num": 13, "state": "OPEN", "checks": "pass", "review": review, "approvals": {"have": have, "need": need},
                       "stage": {"phase": "review", "label": "Awaiting reviews"}});
        let [_, (_, st, sub), _] = view::pr_steps(&p);
        (sub.unwrap_or_default(), st)
    };
    assert_eq!(review("pending", 1, json!(2)), ("1 of 2".into(), view::StepSt::Wait));
    assert_eq!(review("approved", 2, json!(2)), ("Approved".into(), view::StepSt::Done));
    assert_eq!(review("approved", 3, json!(2)), ("Approved".into(), view::StepSt::Done));
    assert_eq!(review("pending", 0, json!(2)), ("Waiting".into(), view::StepSt::Wait));
    let mut task = with_bar(Value::Null, json!({"phase": "review", "label": "Awaiting reviews"}), "pending");
    task["pr"]["approvals"] = json!({"have": 1, "need": 2});
    let text = panel_text(task, json!({}));
    assert!(text.contains("Review 1 of 2") && !text.contains("Approved"), "{text}");
}

#[::core::prelude::v1::test]
fn checks_and_review_say_when_the_open_pr_was_last_read() {
    let at = (chrono::Utc::now() - chrono::Duration::minutes(4)).to_rfc3339();
    let mut task = with_bar(json!({}), Value::Null, "pending");
    task["pr"]["checked_at"] = json!(at);
    assert!(panel_text(task.clone(), json!({})).contains("Checks and review Updated 4m ago"));
    task["pr"]["state"] = json!("MERGED");
    assert!(!panel_text(task, json!({})).contains("Updated"), "a merged PR isn't read any more");
}

#[::core::prelude::v1::test]
fn checks_say_when_they_arent_needed_or_this_prs() {
    let text = panel_text(with_bar(json!({"checks": "not_needed"}), Value::Null, "none"), json!({}));
    assert!(text.contains("Checks Not needed") && text.contains("You Review Not asked"), "{text}");
    let text = panel_text(with_bar(json!({"checks": "not_ours", "you": "reviewed"}), Value::Null, "approved"), json!({}));
    assert!(text.contains("Checks Not this PR's") && text.contains("You Reviewed") && text.contains("Review Approved"), "{text}");
    let text = panel_text(with_bar(json!({"checks": "skipped"}), Value::Null, "none"), json!({}));
    assert!(text.contains("Checks Skipped") && !text.contains("Not this PR's"), "{text}");
    let text = panel_text(with_bar(json!({"new_comments": 2}), json!({"phase": "review"}), "none"), json!({}));
    assert!(text.contains("Review 2 new comments"), "{text}");
}

#[::core::prelude::v1::test]
fn review_says_when_it_needs_setup_or_is_off_and_not_asked_links_nowhere() {
    let tree = tree(&json!({"task": with_bar(json!({}), Value::Null, "none"), "state": {}, "ui": {}}));
    assert!(view::text(&tree).contains("Review Not asked"), "{}", view::text(&tree));
    let dbg = format!("{tree:?}");
    assert!(dbg.contains("\"Not asked\"") && !dbg.contains("Link { s: \"Not asked\""), "Not asked has no link: {dbg}");
    let text = panel_text(with_bar(json!({"review": "setup"}), Value::Null, "none"), json!({}));
    assert!(text.contains("Review Needs setup"), "{text}");
    let text = panel_text(with_bar(json!({"review": "off"}), Value::Null, "none"), json!({}));
    assert!(text.contains("Review Off"), "{text}");
}

#[::core::prelude::v1::test]
fn before_checks_pass_the_you_step_says_when_and_can_still_be_marked() {
    let tree = tree(&json!({"task": with_bar(json!({"you": "early"}), Value::Null, "none"), "state": {}, "ui": {}}));
    let text = view::text(&tree);
    assert!(text.contains("You After checks") && text.contains("Not reviewed yet Mark reviewed ›"), "{text}");
    assert!(!text.contains("Waiting on you"), "{text}");
    assert!(view::acts(&tree).iter().any(|a| a == "pr-reviewed"));
}

#[::core::prelude::v1::test]
fn you_reads_off_when_owner_review_is_off() {
    let text = panel_text(with_bar(json!({"you": "off"}), Value::Null, "none"), json!({}));
    assert!(text.contains("You Off") && !text.contains("Waiting on you"), "{text}");
}

#[::core::prelude::v1::test]
fn new_comments_link_to_the_first_unread_thread() {
    let thread = "https://github.com/acme/webapp/pull/13#discussion_r9";
    let tree = tree(&json!({"task": with_bar(json!({"new_comments": 1, "comments_url": thread}), json!({"phase": "comments"}), "none"), "state": {}, "ui": {}}));
    assert!(view::text(&tree).contains("Review 1 new comment"), "{}", view::text(&tree));
    assert!(format!("{tree:?}").contains(thread), "the link goes to the thread");
}

#[::core::prelude::v1::test]
fn the_review_step_says_before_its_first_round_and_opens_the_findings() {
    let pending = json!({"name": "Author review", "bar": "Critic", "pending": true});
    let text = panel_text(with_bar(json!({"author_review": pending}), Value::Null, "none"), json!({}));
    assert!(text.contains("Critic Not reviewed by Critic yet"), "{text}");
    let tree = tree(&json!({"task": with_bar(json!({"author_review": {"bar": "Critic", "headline": "2 open findings", "open": 2}}), Value::Null, "none"), "state": {}, "ui": {}}));
    assert!(format!("{tree:?}").contains("Findings"), "the step links to its findings: {tree:?}");
    // A task that doesn't end in a PR has none.
    let task = json!({"ref": "T8", "id": 8, "title": "x", "status": "working", "project": "webapp", "ships_pr": false, "author_review": pending});
    assert!(!panel_text(task, json!({})).contains("Critic"));
}

#[::core::prelude::v1::test]
fn new_comments_show_while_changes_are_asked() {
    let bar = json!({"new_comments": 2, "comments_url": "https://github.com/acme/webapp/pull/13#discussion_r1"});
    let text = panel_text(with_bar(bar.clone(), json!({"phase": "fix"}), "changes"), json!({}));
    assert!(text.contains("2 new comments ›"), "{text}");
    let text = panel_text(with_bar(bar, json!({"phase": "review"}), "none"), json!({}));
    assert!(text.contains("Review 2 new comments") && !text.contains("2 new comments ›"), "once, in the step: {text}");
}

#[::core::prelude::v1::test]
fn the_review_step_shows_before_the_pr_opens() {
    let task = json!({"ref": "T8", "id": 8, "title": "x", "status": "working", "project": "webapp", "ships_pr": true,
                      "author_review": {"bar": "Critic", "headline": "2 open findings", "open": 2, "passed": false}});
    let text = panel_text(task, json!({}));
    assert!(text.contains("Pending merge Checks and review Critic 2 open findings Checks You Review Pull request Working"), "newest on top: {text}");
    // From the detail's step results too.
    let task = json!({"ref": "T8", "id": 8, "title": "x", "status": "working", "project": "webapp",
                      "step_results": [{"name": "Author review", "bar": "Critic", "headline": "No findings", "open": 0, "passed": true, "findings": []}]});
    assert!(panel_text(task, json!({})).contains("Author review No findings"));
}

#[::core::prelude::v1::test]
fn no_pr_shows_why_with_its_refs_linked() {
    let task = json!({"ref": "T5", "id": 5, "title": "Fix it", "status": "done", "project": "webapp",
                      "no_pr": "T3 fixed it in ABC-7.", "ships_pr": true, "no_evidence": "no UI change"});
    let tree = tree(&json!({"task": task, "state": {"jira": {"enabled": true, "site": "acme.atlassian.net"}}, "ui": {}}));
    let text = view::text(&tree);
    assert!(text.contains("No PR: T3 fixed it in ABC-7 .") && text.contains("No evidence: no UI change"), "{text}");
    assert!(view::acts(&tree).iter().any(|a| a == "open-task"), "T3 opens its task");
    let links = format!("{tree:?}");
    assert!(links.contains("https://acme.atlassian.net/browse/ABC-7"), "the ticket links to Jira");
}

#[::core::prelude::v1::test]
fn the_pr_plan_shows_on_a_task_without_a_pr() {
    let t = |ships: bool| json!({"ref": "T6", "id": 6, "title": "x", "status": "queued", "project": "webapp", "ships_pr": ships});
    // The whole way ahead, its steps in place as once the PR is open, the last stop on top.
    assert!(panel_text(t(true), json!({})).contains("Pending merge Checks and review Checks You Review Pull request"));
    assert!(!panel_text(t(false), json!({})).contains("Pull request"), "a task that doesn't end in a PR stops at Done");
}

#[::core::prelude::v1::test]
fn a_step_opens_to_its_findings_with_their_state() {
    let task = json!({"ref": "T7", "id": 7, "title": "x", "status": "working", "project": "webapp",
                      "step_results": [{"name": "Author review", "headline": "1 open finding", "open": 1, "stale": true, "at": "2026-01-05T12:00:00Z",
                                        "findings": [{"id": "F2", "title": "SQL injection", "severity": "high", "file": "api.rs", "line": 12},
                                                     {"id": "F1", "title": "Typo", "state": "fixed"}]}]});
    let shut = panel_text(task.clone(), json!({}));
    assert!(shut.contains("Author review 1 open finding") && !shut.contains("SQL injection"), "{shut}");
    let text = view::text(&tree(&json!({"task": task, "state": {}, "ui": {"tl_open": ["result:0"]}})));
    assert!(text.contains("Author review 1 open finding Jan 5 F2") && text.contains("Typo Moved since"), "{text}");
    assert!(text.contains("F2 open high SQL injection api.rs:12") && text.contains("F1 fixed Typo"), "{text}");
}

#[::core::prelude::v1::test]
fn a_code_review_round_opens_to_its_findings_and_how_each_was_triaged() {
    let round = json!({"round": 1, "try": 1, "ok": true, "reviewer": "acme-review", "verdict": "fail", "headline": "4 findings", "cost": 0.4,
                       "head": "abc1234def", "at": "2026-01-05T12:00:00Z", "capped": false,
                       "findings": [{"id": "F1", "title": "Off by one", "severity": "high", "file": "src/a.rs", "line": 3, "outcome": "fix", "reason": "Breaks paging", "fixed": "Counts from zero"},
                                    {"id": "F2", "title": "Slow loop", "outcome": "backlog", "reason": "Older code", "issue": "B7"},
                                    {"id": "F3", "title": "Naming", "outcome": "dismiss", "reason": "Matches the file"},
                                    {"id": "F4", "title": "Null check", "outcome": "fix"}]});
    let task = json!({"ref": "T7", "id": 7, "title": "x", "status": "working", "project": "webapp", "code_review_rounds": [round]});
    let shut = panel_text(task.clone(), json!({}));
    assert!(shut.contains("Code review Round 1 · 4 findings") && !shut.contains("Off by one"), "{shut}");
    let text = view::text(&tree(&json!({"task": task, "state": {}, "ui": {"tl_open": ["round:0"]}})));
    assert!(text.contains("F1 Fixed high Off by one src/a.rs:3 Breaks paging Counts from zero"), "{text}");
    assert!(text.contains("F2 Backlog Slow loop B7 Older code") && text.contains("F3 Dismissed Naming Matches the file"), "{text}");
    assert!(text.contains("F4 To fix Null check") && text.contains("Reviewer acme-review · $0.40") && text.contains("Ran on commit abc1234"), "{text}");
}

#[::core::prelude::v1::test]
fn a_code_review_shows_where_its_round_stands_and_the_cap_alert() {
    let round = |n: u64, capped: bool| json!({"round": n, "ok": true, "verdict": "fail", "headline": "1 finding", "at": "2026-01-05T12:00:00Z", "capped": capped,
                                              "findings": [{"id": "F1", "title": "Off by one", "outcome": "fix"}]});
    let task = |rounds: Value, cr: Value| json!({"ref": "T7", "id": 7, "title": "x", "status": "working", "project": "webapp", "code_review_rounds": rounds, "code_review": cr});
    let triaging = panel_text(task(json!([round(1, false)]), json!({"state": "triaging", "round": 1, "triage": {"findings": 3, "triaged": 1}})), json!({}));
    assert!(triaging.contains("Round 1 · 1 finding") && triaging.contains("Triaging 1 of 3"), "{triaging}");
    let fixing = panel_text(task(json!([round(1, false)]), json!({"state": "fixing", "round": 1, "fix": {"findings": 2, "fixed": 0}})), json!({}));
    assert!(fixing.contains("Fixing 0 of 2"), "{fixing}");
    // A retried round shows once, as the one being reviewed.
    let failed = json!({"round": 2, "ok": false, "error": "exit 1", "at": "2026-01-05T12:00:00Z", "findings": []});
    let reviewing = panel_text(task(json!([round(1, false), failed]), json!({"state": "running", "round": 2, "since": "2026-01-05T12:05:00Z"})), json!({}));
    assert!(reviewing.contains("Code review Round 2 · Reviewing") && !reviewing.contains("didn’t run"), "{reviewing}");
    let capped = panel_text(task(json!([round(5, true)]), Value::Null), json!({}));
    assert!(capped.contains("Reached its 5-round cap It went on to its PR with F1 still to fix."), "{capped}");
    assert!(!panel_text(task(json!([round(1, false)]), Value::Null), json!({})).contains("cap"));
}

#[::core::prelude::v1::test]
fn a_findings_link_opens_the_step_results_holding_findings() {
    let task = json!({"step_results": [{"name": "QA", "findings": []},
                                       {"name": "Author review", "findings": [{"id": "F1", "title": "Typo"}]}]});
    assert_eq!(view::findings_steps(&task), vec![view::TlStep::Result(1)]);
}

#[::core::prelude::v1::test]
fn reviewer_pills_show_each_reviewers_state() {
    let rows = json!([{"name": "Ana", "user": "ana", "state": "approved", "swaps": 0},
                      {"name": "Bo", "user": "bo", "state": "changes", "swaps": 0},
                      {"name": "Cy", "user": "cy", "state": "rereview", "swaps": 0},
                      {"name": "Di", "user": "di", "state": "waiting", "swaps": 2}]);
    let bar = json!({"approvals": 1, "reviewers": 4, "new_comments": 0, "reviewer_rows": rows});
    let text = panel_text(with_bar(bar, json!({"phase": "rereview"}), "changes"), json!({}));
    for want in ["Ana Approved", "Bo Changes asked", "Cy Re-review waiting", "Di Waiting Swapped 2 times"] {
        assert!(text.contains(want), "{want:?} in {text}");
    }
}

#[::core::prelude::v1::test]
fn reviewer_pills_say_when_each_was_asked_and_the_ask_stage_shows_its_bar() {
    let at = crate::fmt::now().to_rfc3339();
    let rows = json!([{"name": "Ana", "user": "ana", "state": "waiting", "swaps": 0, "asked_at": at}]);
    let bar = json!({"approvals": 0, "reviewers": 1, "new_comments": 0, "reviewer_rows": rows});
    let text = panel_text(with_bar(bar, json!({"phase": "ask", "label": "Asking for reviews"}), "pending"), json!({}));
    let when = crate::fmt::hhmm(&at);
    assert!(when.ends_with("AM") || when.ends_with("PM"), "{when}");
    assert!(text.contains(&format!("Ana Waiting Asked {when}")), "{text}");
    assert!(text.contains("Asking for reviews"), "{text}");
}

#[::core::prelude::v1::test]
fn a_reviewer_asked_to_look_again_says_when_or_that_the_ask_is_queued() {
    let at = (crate::fmt::now() - chrono::Duration::days(2)).to_rfc3339();
    let rows = json!([{"name": "Ana", "user": "ana", "state": "rereview", "changes": true, "swaps": 0,
                       "asked_at": at, "rereview_at": at, "rereview_queued": false},
                      {"name": "Bo", "user": "bo", "state": "rereview", "changes": true, "swaps": 0,
                       "asked_at": at, "rereview_at": null, "rereview_queued": true},
                      {"name": "Cy", "user": "cy", "state": "changes", "swaps": 0, "asked_at": at}]);
    let bar = json!({"approvals": 0, "reviewers": 3, "new_comments": 0, "reviewer_rows": rows});
    let text = panel_text(with_bar(bar, json!({"phase": "rereview"}), "changes"), json!({}));
    let when = crate::fmt::day_time(&at);
    assert!(when.ends_with("am") || when.ends_with("pm"), "12-hour: {when}");
    assert!(text.contains(&format!("Ana Changes asked Re-review asked {when}")), "{text}");
    assert!(text.contains("Bo Changes asked Re-review queued"), "{text}");
    assert!(text.contains("Cy Changes asked Asked") && !text.contains("Cy Changes asked Re-review"), "{text}");
}

#[::core::prelude::v1::test]
fn a_waiting_reviewer_says_how_long_the_pr_has_waited_on_them() {
    let at = (crate::fmt::now() - chrono::Duration::minutes(125)).to_rfc3339();
    let rows = json!([{"name": "Ana", "user": "ana", "state": "waiting", "swaps": 0, "asked_at": at},
                      {"name": "Bo", "user": "bo", "state": "approved", "swaps": 0, "asked_at": at},
                      {"name": "Cy", "user": "cy", "state": "waiting", "swaps": 0}]);
    let bar = json!({"approvals": 1, "reviewers": 3, "new_comments": 0, "reviewer_rows": rows});
    let text = panel_text(with_bar(bar, json!({"phase": "review", "label": "Awaiting reviews"}), "pending"), json!({}));
    assert!(text.contains("Ana Waiting 2h 5m"), "{text}");
    assert!(text.contains("Bo Approved") && !text.contains("Approved 2h"), "an answered reviewer isn't waited on: {text}");
    assert!(text.contains("Cy Waiting") && !text.contains("Cy Waiting 2h"), "{text}");
}

/// The panel for T4 with `user`'s reviewer popover open, holding `asks` as the daemon sent them
/// (`None` while they load).
fn with_pop(user: &str, asks: Option<Result<Value, String>>) -> view::Node {
    let rows = json!([{"name": "Ana Lee", "user": user, "state": "waiting", "swaps": 0}]);
    let bar = json!({"approvals": 0, "reviewers": 1, "new_comments": 0, "reviewer_rows": rows});
    let task = with_bar(bar, json!({"phase": "review", "label": "Awaiting reviews"}), "pending");
    let mut ui = Ui { rev_pop: Some(user.to_string()), ..Default::default() };
    if let Some(a) = asks {
        ui.rev_asks.insert(user.to_string(), a);
    }
    let (state, folds, trail) = (json!({}), HashMap::new(), Vec::new());
    let log = view::LogRows::of(&task, ui.log_filter());
    let c = Ctx { state: &state, task: Some(&task), card: None, r: "T4", tab: "overview", trail: &trail, err: None, ui: &ui, folds: &folds, log: &log };
    view::panel(&c)
}

/// The popover in the tree, if it's open.
fn find_pop(n: &view::Node) -> Option<&view::Node> {
    match n {
        view::Node::El { k: view::K::RevPop, .. } => Some(n),
        view::Node::El { kids, .. } => kids.iter().find_map(find_pop),
        _ => None,
    }
}

fn ask(task: &str, num: i64, wait: f64, swap_in: Value, rereview: bool, at: &str) -> Value {
    json!({"id": num, "project": "webapp", "why": "picked", "asked_at": at, "task": task, "goal": "G7",
           "pr": {"host": "github", "repo": "webapp", "num": num, "url": format!("https://github.com/acme/webapp/pull/{num}"), "title": format!("PR {num}")},
           "rereview": rereview, "wait_work_mins": wait, "swap_in_mins": swap_in})
}

#[::core::prelude::v1::test]
fn a_reviewers_name_opens_their_popover() {
    let popped = with_pop("ana", None);
    assert!(view::acts(&popped).contains(&"rev-pop:ana".to_string()), "{:?}", view::acts(&popped));
    let pop = find_pop(&popped).expect("open");
    assert_eq!(view::text(pop), "Ana Lee ana Loading…");
    let rows = json!([{"name": "Ana Lee", "user": "ana", "state": "waiting", "swaps": 0}]);
    let bar = json!({"approvals": 0, "reviewers": 1, "new_comments": 0, "reviewer_rows": rows});
    let shut = tree(&json!({"task": with_bar(bar, json!({"phase": "review"}), "pending"), "state": {}, "ui": {}}));
    assert!(find_pop(&shut).is_none(), "shut until the name is clicked");
}

#[::core::prelude::v1::test]
fn the_reviewer_popover_lists_each_open_ask_with_its_wait_and_notes() {
    let now = crate::fmt::now();
    let at = (now - chrono::Duration::minutes(40)).to_rfc3339();
    let answered = (now - chrono::Duration::minutes(10)).to_rfc3339();
    let asks = json!({"id": 3, "name": "Ana Lee", "user": "ana", "median_work_mins": 24, "last_review_at": answered,
        "open": [ask("T9", 21, 64., json!(20), false, &at), ask("T4", 13, 32., json!(150), true, &at), ask("T11", 30, 5., Value::Null, false, &at)],
        "answered_this_week": [{"id": 8, "project": "webapp", "why": "picked", "asked_at": at, "task": "T2", "goal": null,
            "pr": {"host": "github", "repo": "webapp", "num": 9, "url": "https://github.com/acme/webapp/pull/9", "title": "PR 9"},
            "answer": "approved", "work_mins": 18, "answered_at": answered}]});
    let tree = with_pop("ana", Some(Ok(asks)));
    let pop = find_pop(&tree).expect("open");
    let text = view::text(pop);
    let (when, done) = (view::ask_when(&at), view::ask_when(&answered));
    assert!(when.ends_with(" AM") || when.ends_with(" PM"), "12-hour: {when}");
    assert!(text.starts_with(&format!("Ana Lee ana 3 open · reviews in about 25m · last review {done}")), "{text}");
    // Close to swapping: the wait and its note in orange.
    assert!(text.contains(&format!("webapp#21 PR 21 1h 04m T9 G7 · Asked {when} Swaps in 20m")), "{text}");
    // A re-review, for the task in the panel.
    assert!(text.contains(&format!("webapp#13 PR 13 32m T4 G7 · Changes addressed {when} Re-review This task")), "{text}");
    assert!(text.contains(&format!("webapp#30 PR 30 5m T11 G7 · Asked {when}")) && !text.contains("Swaps in 2h"), "{text}");
    assert!(text.ends_with("Reviewed this week · 1"), "folded until opened: {text}");
    let rows: Vec<&view::Node> = match pop {
        view::Node::El { kids, .. } => kids.iter().filter(|k| matches!(k, view::Node::El { k: view::K::RevAsk { .. }, .. })).collect(),
        _ => vec![],
    };
    let here: Vec<bool> = rows.iter().map(|r| matches!(r, view::Node::El { k: view::K::RevAsk { here: true }, .. })).collect();
    assert_eq!(here, [false, true, false], "only this task's row is highlighted");
    let late = |n: &view::Node| format!("{n:?}").contains("Wait { late: true }");
    assert_eq!(rows.iter().map(|r| late(r)).collect::<Vec<_>>(), [true, false, false], "only the one about to swap is orange");
    // Nothing to change: only links to the PRs, tasks and goals.
    assert_eq!(view::acts(pop), ["open-task", "open-task", "open-task"]);
}

#[::core::prelude::v1::test]
fn the_reviewed_this_week_fold_lists_each_answer_took_and_when() {
    let at = crate::fmt::now().to_rfc3339();
    let asks = json!({"id": null, "name": "ana", "user": "ana", "median_work_mins": null, "last_review_at": at, "open": [],
        "answered_this_week": [{"id": 8, "project": "webapp", "asked_at": at, "task": "T2", "goal": null,
            "pr": {"host": "github", "repo": "webapp", "num": 9, "url": "https://github.com/acme/webapp/pull/9", "title": "PR 9"},
            "answer": "changes", "work_mins": 75, "answered_at": at}]});
    let rows = json!([{"name": "ana", "user": "ana", "state": "waiting", "swaps": 0}]);
    let bar = json!({"approvals": 0, "reviewers": 1, "new_comments": 0, "reviewer_rows": rows});
    let task = with_bar(bar, json!({"phase": "review"}), "pending");
    let ui = Ui { rev_pop: Some("ana".into()), rev_asks: HashMap::from([("ana".to_string(), Ok(asks))]), ..Default::default() };
    let (state, trail) = (json!({}), Vec::new());
    let folds = HashMap::from([("tb.fold.reviewed".to_string(), "open".to_string())]);
    let log = view::LogRows::of(&task, ui.log_filter());
    let c = Ctx { state: &state, task: Some(&task), card: None, r: "T4", tab: "overview", trail: &trail, err: None, ui: &ui, folds: &folds, log: &log };
    let tree = view::panel(&c);
    let text = view::text(find_pop(&tree).expect("open"));
    let when = view::ask_when(&at);
    // Off the roster: the login is the name, shown once; no median yet.
    assert_eq!(text, format!("ana 0 open · last review {when} No open reviews Reviewed this week · 1 webapp#9 Asked for changes · took 1h 15m · {when}"));
}

#[gpui_kit::test]
fn a_reviewers_popover_loads_by_login_and_esc_or_their_name_closes_it(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    open(cx, &w, "T3");
    let name = Act::new("rev-pop", "someone-new", "T3", "");
    w.update(cx, |m, _, cx| act(m, &name, cx)).unwrap();
    settle(cx);
    // Not on the roster: the board still answers, by their login.
    let (open, got) = ui(|u| (u.rev_pop.clone(), u.rev_asks.get("someone-new").cloned()));
    assert_eq!(open.as_deref(), Some("someone-new"));
    let got = got.expect("fetched").expect("loads");
    assert_eq!((got["id"].clone(), got["user"].clone(), got["open"].clone()), (Value::Null, json!("someone-new"), json!([])));
    assert!(rec.posts().is_empty(), "opening it changes nothing");
    // Esc closes the popover, then the panel.
    w.update(cx, |m, window, cx| m.escape(window, cx)).unwrap();
    assert!(ui(|u| u.rev_pop.is_none()));
    assert!(w.update(cx, |m, _, _| m.panel.is_some()).unwrap(), "the panel stays");
    // Their name toggles it.
    w.update(cx, |m, _, cx| act(m, &name, cx)).unwrap();
    w.update(cx, |m, _, cx| act(m, &name, cx)).unwrap();
    assert!(ui(|u| u.rev_pop.is_none()));
    w.update(cx, |m, window, cx| m.escape(window, cx)).unwrap();
    assert!(w.update(cx, |m, _, _| m.panel.is_none()).unwrap());
}

#[::core::prelude::v1::test]
fn the_reviewer_popover_says_when_their_reviews_didnt_load() {
    let tree = with_pop("ana", Some(Err("daemon down".into())));
    assert_eq!(view::text(find_pop(&tree).expect("open")), "Ana Lee ana Couldn’t load their reviews: daemon down");
}

#[::core::prelude::v1::test]
fn ask_times_are_12_hour_with_the_day_when_not_today() {
    let now = crate::fmt::now();
    let today = view::ask_when(&now.to_rfc3339());
    assert!(today.ends_with(" AM") || today.ends_with(" PM"), "{today}");
    let week = view::ask_when(&(now - chrono::Duration::days(3)).to_rfc3339());
    assert!(["Mon ", "Tue ", "Wed ", "Thu ", "Fri ", "Sat ", "Sun "].iter().any(|d| week.starts_with(d)) && (week.ends_with(" AM") || week.ends_with(" PM")), "{week}");
    let old = view::ask_when(&(now - chrono::Duration::days(30)).to_rfc3339());
    assert!(old.contains(", ") && (old.ends_with(" AM") || old.ends_with(" PM")), "{old}");
    assert_eq!((view::wait_span(32.), view::wait_span(64.), view::wait_span(1570.)), ("32m".into(), "1h 04m".into(), "26h 10m".into()));
}

#[::core::prelude::v1::test]
fn the_note_for_testers_shows_and_says_when_the_ticket_waits_for_it() {
    let t = |note: Value| json!({"ref": "T9", "id": 9, "title": "x", "status": "done", "project": "webapp", "pr": {"num": 13, "repo": "webapp", "state": "MERGED", "checks": "pass", "review": "approved"}, "merged_note": note});
    let text = panel_text(t(json!({"text": "What changed: the login page", "waiting": false})), json!({}));
    assert!(text.contains("Note for testers") && text.contains("What changed: the login page") && !text.contains("Waiting"), "{text}");
    let text = panel_text(t(json!({"text": null, "waiting": true})), json!({}));
    assert!(text.contains("Note for testers Waiting Merged without one; the ticket waits"), "{text}");
    assert!(!panel_text(t(Value::Null), json!({})).contains("Note for testers"));
}

#[::core::prelude::v1::test]
fn a_long_note_for_testers_shows_its_first_three_lines_then_show_all() {
    let checks = (1..=4).map(|k| format!("- Reconnect to the Core {k} times; the library, queue and zones stay right.")).collect::<Vec<_>>().join("\n");
    let note = format!("**What changed:** the app no longer grows in memory on reconnect.\n\n**Please check:**\n\n{checks}");
    let task = json!({"ref": "T9", "id": 9, "title": "x", "status": "done", "project": "webapp", "pr": {"num": 13, "repo": "webapp", "state": "MERGED", "checks": "pass", "review": "approved"}, "merged_note": {"text": note, "waiting": false}});
    let text = panel_text(task.clone(), json!({}));
    assert!(text.contains("Note for testers What changed: the app no longer") && !text.contains("**"), "{text}");
    assert!(text.contains("Please check: Reconnect to the Core 1 times") && !text.contains("Core 2 times"), "{text}");
    assert!(text.contains("Show all"), "{text}");
    let tr = tree(&json!({"task": task, "state": {}}));
    assert!(view::acts(&tr).iter().any(|a| a == "note-all"), "{:?}", view::acts(&tr));
    let short = panel_text(json!({"ref": "T9", "id": 9, "title": "x", "status": "done", "project": "webapp", "pr": {"num": 13, "repo": "webapp", "state": "MERGED", "checks": "pass", "review": "approved"}, "merged_note": {"text": "One\n\nTwo\nThree", "waiting": false}}), json!({}));
    assert!(short.contains("One Two Three") && !short.contains("Show all"), "{short}");
}

#[::core::prelude::v1::test]
fn a_note_label_on_its_own_line_takes_the_value_under_it() {
    let one = view::text(&view::note_body("**What to test:**\n Sign in twice.\n**Branch:** fix/x"));
    let two = view::text(&view::note_body("**What to test:** Sign in twice.\n**Branch:** fix/x"));
    assert_eq!(one, two);
    // An indented value is still the label's, not a code block.
    assert_eq!(view::text(&view::note_body("**Affected areas:**\n  Sign-in")), view::text(&view::note_body("**Affected areas:** Sign-in")));
    let view::Node::El { kids, .. } = view::note_body("**Please check:**\n- One\n- Two") else { unreachable!() };
    assert_eq!(kids.len(), 2, "a list under a label stays a list");
}

#[gpui_kit::test]
fn show_all_opens_the_whole_note_in_a_dialog(cx: &mut gpui_kit::TestAppContext) {
    let (w, _rec) = parity::window(cx);
    let note = (1..=6).map(|k| format!("Line {k}")).collect::<Vec<_>>().join("\n");
    let task = json!({"ref": "T9", "id": 9, "title": "x", "status": "done", "project": "webapp", "pr": {"num": 13, "repo": "webapp", "state": "MERGED", "checks": "pass", "review": "approved"}, "merged_note": {"text": note.clone(), "waiting": false}});
    w.update(cx, |m, _, cx| {
        m.panel = Some(Panel::Task { r: "T9".into(), tab: TaskTab::Overview });
        m.data.task = Some(task);
        act(m, &Act::new("note-all", "", "T9", ""), cx);
        assert!(matches!(&m.modal, Some(crate::ui::modals::Modal::Note(t)) if *t == note));
    })
    .unwrap();
    let _ = cx.update_window(w.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
}

#[test]
fn a_fetched_handoff_is_out_of_date_once_the_task_moves_on() {
    let t = json!({"ctx_version": 3, "updated_at": "2026-10-16T10:00:00", "status": "working", "log": [{"at": "2026-10-16T10:00:00"}]});
    let same = handoff_stamp(&t);
    assert_eq!(handoff_stamp(&t.clone()), same, "the same task keeps its handoff");
    for (k, v) in [("ctx_version", json!(4)), ("updated_at", json!("2026-10-16T10:05:00")), ("status", json!("review")), ("log", json!([{"at": "2026-10-16T10:07:00"}]))] {
        let mut moved = t.clone();
        moved[k] = v;
        assert_ne!(handoff_stamp(&moved), same, "a new {k} fetches the handoff again");
    }
}

#[::core::prelude::v1::test]
fn the_checks_step_says_being_fixed_only_while_a_terminal_is_on_it() {
    // #489: a fix nobody is on reads Failed, one only the owner can fix reads Needs you.
    let checks = |stage: Value| {
        let p = json!({"num": 13, "state": "OPEN", "checks": "fail", "review": "pending", "stage": stage});
        let [(_, st, sub), _, _] = view::pr_steps(&p);
        (sub.unwrap_or_default(), st)
    };
    let fix = |extra: Value| {
        let mut s = json!({"phase": "fix", "label": "Fixing checks"});
        s.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        checks(s).0
    };
    assert_eq!(fix(json!({"session": "s1"})), "Being fixed");
    assert_eq!(fix(json!({})), "Fix starting");
    assert_eq!(fix(json!({"idle": true})), "Failed");
    assert_eq!(fix(json!({"owner": {"reason": "Add the NPM_TOKEN secret"}})), "Needs you");
    assert_eq!(fix(json!({"owner": {"reason": "Add the NPM_TOKEN secret"}, "session": "s1"})), "Needs you");
}

fn tl_step(v: &Value) -> Option<view::TlStep> {
    match v.as_str()? {
        "started" => Some(view::TlStep::Started),
        s => match s.strip_prefix("round:") {
            Some(i) => i.parse().ok().map(view::TlStep::Round),
            None => s.strip_prefix("result:")?.parse().ok().map(view::TlStep::Result),
        },
    }
}

fn every(n: &view::Node, f: &mut impl FnMut(&view::Node)) {
    f(n);
    if let view::Node::El { kids, .. } = n {
        kids.iter().for_each(|k| every(k, f));
    }
}

fn two_terminals() -> Value {
    json!({"ref": "T7", "id": 7, "title": "x", "status": "working", "project": "webapp", "started_at": "2026-01-05T14:00:00Z",
           "session": {"id": "s2", "status": "working"},
           "terminals": [{"id": "s2", "name": "T7 Second go", "status": "working", "background": null, "at": "2026-01-05T15:14:00Z"},
                         {"id": "s1", "name": "T7 First go", "status": "gone", "background": null, "at": "2026-01-05T14:02:00Z",
                          "gone_at": "2026-01-05T14:40:00Z"}],
           "context": {"where": {"branch": "feat/t7-progress", "uncommitted": 3}},
           "log": [{"kind": "commit", "at": "2026-01-05T15:52:00Z", "text": "wip"}]})
}

#[::core::prelude::v1::test]
fn the_started_step_shows_its_terminals_only_when_opened() {
    let task = two_terminals();
    let shut = panel_text(task.clone(), json!({}));
    assert!(!shut.contains("First go") && !shut.contains("feat/t7-progress"), "{shut}");
    let text = view::text(&tree(&json!({"task": task, "state": {}, "ui": {"tl_open": ["started"]}})));
    let closed = crate::fmt::hhmm("2026-01-05T14:40:00Z");
    assert!(text.contains("T7 Second go Working"), "{text}");
    assert!(text.contains(&format!("T7 First go Closed {closed}")), "{text}");
    let last = crate::fmt::hhmm("2026-01-05T15:52:00Z");
    assert!(text.contains(&format!("Branch feat/t7-progress Changes 3 files not committed · last commit {last}")), "{text}");
}

#[::core::prelude::v1::test]
fn a_terminal_row_has_one_status_marker() {
    let tree = tree(&json!({"task": two_terminals(), "state": {}, "ui": {"tl_open": ["started"]}}));
    fn pills(n: &view::Node, out: &mut usize, in_term: bool) {
        match n {
            view::Node::Pill { .. } if in_term => *out += 1,
            view::Node::El { k, kids, .. } => kids.iter().for_each(|k2| pills(k2, out, in_term || matches!(k, view::K::DetTerm { .. }))),
            _ => {}
        }
    }
    let mut n = 0;
    pills(&tree, &mut n, false);
    assert_eq!(n, 0, "the row's dot is its marker; the state is words beside the name");
}

#[::core::prelude::v1::test]
fn the_live_terminal_has_a_focus_button() {
    let tree = tree(&json!({"task": two_terminals(), "state": {}, "ui": {"tl_open": ["started"]}}));
    let mut tips = Vec::new();
    every(&tree, &mut |n| {
        if let view::Node::Btn { act, tip: Some(tip), .. } = n {
            tips.push(format!("{} {tip}", act.act));
        }
    });
    assert_eq!(tips.iter().filter(|t| *t == "focus Focus in Midna").count(), 1, "{tips:?}");
}

#[::core::prelude::v1::test]
fn a_step_result_opens_to_its_full_note_and_what_it_ran_on() {
    let task = json!({"ref": "T7", "id": 7, "title": "x", "status": "working", "project": "webapp",
                      "devices": {"lent_labels": ["Android emulator"]},
                      "step_results": [{"name": "Verify numbers", "headline": "No findings", "note": "Checked the totals after teardown.", "open": 0,
                                        "head": "4e1a9c2f00", "at": "2026-01-05T12:00:00Z", "findings": []}]});
    let shut = panel_text(task.clone(), json!({}));
    assert!(shut.contains("Verify numbers No findings") && !shut.contains("Checked the totals"), "{shut}");
    let text = view::text(&tree(&json!({"task": task, "state": {}, "ui": {"tl_open": ["result:0"]}})));
    assert!(text.contains("Checked the totals after teardown. Ran on Android emulator · commit 4e1a9c2"), "{text}");
}

#[::core::prelude::v1::test]
fn a_step_with_nothing_more_to_show_does_not_open() {
    let task = json!({"ref": "T7", "id": 7, "title": "x", "status": "working", "project": "webapp",
                      "step_results": [{"name": "Lint", "headline": "Passed", "open": 0, "at": "2026-01-05T12:00:00Z", "findings": []}]});
    let tree = tree(&json!({"task": task, "state": {}, "ui": {}}));
    let mut steps = 0;
    every(&tree, &mut |n| {
        if let view::Node::El { k: view::K::TlStep { step: view::TlStep::Result(_), .. }, .. } = n {
            steps += 1;
        }
    });
    assert_eq!(steps, 0);
}

/// Draw the window as it is now, then let what that started (fetches) land.
fn draw(cx: &mut gpui_kit::TestAppContext, w: &WindowHandle<MainWindow>) {
    let _ = cx.update_window((*w).into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
    settle(cx);
}

#[gpui_kit::test]
fn a_long_log_fetches_older_pages_as_it_scrolls(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    for k in 0..450 {
        taskboardd::board::log_event(rec.board().app(), 2, "", if k % 3 == 0 { "turn" } else { "status" }, &format!("Entry {k}")).unwrap();
    }
    open(cx, &w, "T2");
    press(cx, &w, "tab", Some("log"));
    draw(cx, &w);
    let total = w.update(cx, |m, _, _| m.data.task.as_ref().unwrap()["log_total"].as_u64().unwrap() as usize).unwrap();
    let held = |cx: &mut gpui_kit::TestAppContext| w.update(cx, |m, _, _| m.task_panel.log_list.as_ref().map(|l| (l.rows.len(), l.more))).unwrap();
    assert_eq!(held(cx), Some((200, true)), "only the newest page until the list nears its end");
    assert!(w.update(cx, |m, _, _| view::text(&build(m).unwrap())).unwrap().contains(&format!("Full log {total}")), "the tab counts the whole log");

    for _ in 0..5 {
        w.update(cx, |m, _, _| {
            let l = m.task_panel.log_list.as_ref().unwrap();
            l.state.scroll_to(gpui_kit::ListOffset { item_ix: l.rows.len(), offset_in_item: gpui_kit::px(0.) });
        })
        .unwrap();
        draw(cx, &w);
    }
    let (n, more) = held(cx).unwrap();
    assert_eq!((n, more), (total, false), "scrolling to the end pages back through the rest");
    let order: Vec<(String, i64)> = w
        .update(cx, |m, _, _| m.task_panel.log_list.as_ref().unwrap().rows.iter().map(|e| (e["at"].as_str().unwrap().to_string(), e["id"].as_i64().unwrap())).collect())
        .unwrap();
    assert!(order.windows(2).all(|p| p[0] > p[1]), "newest first, each once");

    // A filter starts over with that kind's entries, fetched from the board.
    press(cx, &w, "log-filter", Some("turn"));
    for _ in 0..3 {
        draw(cx, &w);
    }
    let rows = w.update(cx, |m, _, _| m.task_panel.log_list.as_ref().unwrap().rows.clone()).unwrap();
    assert_eq!(rows.iter().filter(|e| e["text"].as_str().unwrap_or("").starts_with("Entry ")).count(), 150);
    assert!(rows.iter().all(|e| view::log_group(e["kind"].as_str().unwrap()) == "turn"));
}

/// A human task's box offers Talk it over beside Done, which asks the board for a terminal.
#[gpui_kit::test]
fn a_human_task_talks_it_over(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    let t = rec.board().post("tasks", json!({"title": "Sign the form", "detail": "1. Open https://example.com and sign.", "project": "webapp", "kind": "human", "status": "queued"})).unwrap();
    let r = format!("T{}", t["id"].as_i64().or_else(|| t["task"]["id"].as_i64()).expect("new task id"));
    open(cx, &w, &r);
    let a = control(cx, &w, "talk", None);
    w.update(cx, |m, _, cx| act(m, &a, cx)).unwrap();
    settle(cx);
    assert_eq!(rec.posts().iter().filter(|(p, _)| p.ends_with("/talk")).count(), 1, "{:?}", rec.posts());
}

/// A human task's Done reads "Sending…" until the box turns to "Done by you", with no note beside it.
#[gpui_kit::test]
fn a_human_tasks_done_turns_the_box_without_a_note(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    let t = rec.board().post("tasks", json!({"title": "Sign the form", "detail": "1. Open https://example.com and sign.", "project": "webapp", "kind": "human", "status": "queued"})).unwrap();
    let r = format!("T{}", t["id"].as_i64().or_else(|| t["task"]["id"].as_i64()).expect("new task id"));
    open(cx, &w, &r);
    let a = control(cx, &w, "did", None);
    w.update(cx, |m, _, cx| act(m, &a, cx)).unwrap();
    w.update(cx, |m, _, _| assert!(view::acts(&build(m).unwrap()).contains(&"did!".to_string()), "busy while sending")).unwrap();
    settle(cx);
    assert_eq!(rec.posts().iter().filter(|(p, _)| p.ends_with("/did")).count(), 1, "{:?}", rec.posts());
    assert_eq!(note(&format!("did:{r}")), None);
    w.update(cx, |m, _, _| {
        let tree = build(m).unwrap();
        assert!(!view::acts(&tree).iter().any(|x| x.starts_with("did")), "Done is gone once it's done: {:?}", view::acts(&tree));
        assert!(ui(|u| u.busy.is_empty()));
    })
    .unwrap();
}

/// A note's lines drawn as the panel draws them.
struct NoteLines;

impl gpui_kit::Render for NoteLines {
    fn render(&mut self, window: &mut gpui_kit::Window, cx: &mut gpui_kit::Context<Self>) -> impl gpui_kit::IntoElement {
        use gpui_kit::{ParentElement as _, Styled as _, div, px};
        let t = cx.global::<crate::theme::Theme>().clone();
        let view::Node::El { kids, .. } = view::note_body("Start `quill touch ios` then **swipe** it.\n- Run `idb` now.") else { unreachable!() };
        let mut d = super::Draw { t: &t, window, path: Vec::new(), within: vec![view::K::MdP] };
        let mut col = div().flex().flex_col().w(px(400.));
        for (i, block) in kids.iter().enumerate() {
            d.path = vec![i];
            let line = match block {
                view::Node::El { k: view::K::MdP, kids, .. } => &kids[0],
                view::Node::El { k: view::K::MdUl, kids, .. } => &kids[0],
                _ => unreachable!(),
            };
            col = col.child(div().h(px(20.)).child(d.md_line(line).expect("one run")));
        }
        div().size_full().child(gpui_kit::base::TextSelectionLayer).child(col)
    }
}

#[gpui_kit::test]
fn a_note_line_copies_as_written(cx: &mut gpui_kit::TestAppContext) {
    use gpui_kit::{Modifiers, MouseButton, point, px};
    cx.update(|cx| cx.set_global(crate::theme::Theme::new(crate::theme::ThemeMode::Dark, "Helvetica".into(), "Menlo".into())));
    let (_, cx) = cx.add_window_view(|_, _| NoteLines);
    cx.update(|window, cx| drop(window.draw(cx)));
    cx.simulate_mouse_down(point(px(1.), px(10.)), MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(point(px(399.), px(30.)), Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(point(px(399.), px(30.)), MouseButton::Left, Modifiers::default());
    let picked = cx.update(|window, cx| {
        drop(window.draw(cx));
        gpui_kit::base::TextSelection::selected_text(window, cx)
    });
    // One run per line: a code span or bold word doesn't break the line in the copy.
    assert_eq!(picked, "Start quill touch ios then swipe it.\nRun idb now.");
}

#[test]
fn a_fenced_block_is_code_kept_as_written() {
    let view::Node::El { kids, .. } = view::note_body("Run:\n```\nquill touch ios\n\n  --port 9\n```\nThen swipe.") else { unreachable!() };
    assert_eq!(kids.len(), 3, "{kids:?}");
    assert!(matches!(&kids[1], view::Node::Text { s, st: view::St::Pre, .. } if s == "quill touch ios\n\n  --port 9"), "{:?}", kids[1]);
}
