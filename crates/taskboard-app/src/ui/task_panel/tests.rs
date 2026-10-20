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
    }
}

/// The panel tree for one golden case's input.
fn tree(i: &Value) -> view::Node {
    let ui = ui_from(&i["ui"]);
    let folds: HashMap<String, String> = i["ui"]["folds"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
    let trail: Vec<String> = i["from"].as_str().map(|f| f.split(',').filter(|x| !x.is_empty()).map(str::to_string).collect()).unwrap_or_default();
    let r = i["task"]["ref"].as_str().or(i["ref"].as_str()).unwrap_or("").to_string();
    let goal_page = (i["page"] == "goal").then(|| i["page_id"].as_str()).flatten();
    let c = Ctx {
        state: &i["state"],
        task: i["task"].is_object().then_some(&i["task"]),
        card: i["card"].is_object().then_some(&i["card"]),
        r: &r,
        tab: i["tab"].as_str().unwrap_or("overview"),
        trail: &trail,
        goal_page,
        err: i["err"].as_str(),
        ui: &ui,
        folds: &folds,
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
fn a_needs_box_without_a_terminal_still_offers_an_action() {
    let acts = |task: Value| json!(view::acts(&tree(&json!({"state": {}, "task": task, "tab": "overview"})))).to_string();
    let asked = json!({"ref": "T5", "id": 5, "status": "needs", "needs_reason": "question", "question": "Add the generated patterns?"});
    let a = acts(asked.clone());
    assert!(a.contains("\"answer\"") && !a.contains("\"focus\""), "{a}");
    let fields = json!(view::values(&tree(&json!({"state": {}, "task": asked, "tab": "overview", "ui": {"drafts": {"answer:T5": "yes"}}}))));
    assert_eq!(fields, json!(["yes"]), "the answer box shows its draft");
    let a = acts(json!({"ref": "T5", "id": 5, "status": "needs", "needs_reason": "attention"}));
    assert!(a.contains("\"resume:fresh\""), "{a}");
    // With its terminal open, the terminal is the action.
    let a = acts(json!({"ref": "T5", "id": 5, "status": "needs", "needs_reason": "question", "question": "Q?", "session": {"status": "idle"}}));
    assert!(a.contains("\"focus\"") && !a.contains("\"resume") && !a.contains("\"answer\""), "{a}");
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
    assert!(after.contains("Overview") && !after.contains("Loading…"), "{after}");
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
fn done_task_requeues(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    open(cx, &w, "T1");
    press(cx, &w, "requeue", None);
    assert_eq!(rec.last("tasks/T1/requeue"), Some(json!({})));
    assert_eq!(note("act:T1"), Some(("Back in the queue".into(), false)));
}

#[gpui_kit::test]
fn manage_box_actions(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    open(cx, &w, "T2");
    press(cx, &w, "focus", None);
    assert_eq!(rec.last("tasks/T2/focus"), Some(json!({})));
    press(cx, &w, "close-term", Some("force"));
    assert_eq!(rec.last("tasks/T2/close-terminal"), Some(json!({"force": true})));
    press(cx, &w, "term-focus", None);
    assert_eq!(rec.last("sessions/fake-s1/focus"), Some(json!({})));
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
fn attachments_edit_and_remove(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    let added = rec.board().post("tasks/T2/attachments", json!({"kind": "design", "title": "Mock", "url": "https://figma.com/x"})).unwrap();
    let id = added["attachments"].as_array().and_then(|a| a.last()).map(|a| a["id"].to_string()).or_else(|| added["id"].as_i64().map(|x| x.to_string())).expect("attachment id");
    open(cx, &w, "T2");
    press(cx, &w, "att-menu", None);
    assert!(control(cx, &w, "att-edit", None).id == id);
    press(cx, &w, "att-edit", None);
    press(cx, &w, "att-ekind", Some("doc"));
    typed(cx, &w, &format!("attedit:{id}:title"), "Login mock");
    press(cx, &w, "att-esave", None);
    // Only the fields you changed are sent (the web's drafts).
    assert_eq!(rec.last(&format!("attachments/{id}")), Some(json!({"kind": "doc", "title": "Login mock"})));
    assert_eq!(ui(|u| u.att_edit.clone()), None, "the edit form closes");
    press(cx, &w, "att-menu", None);
    press(cx, &w, "att-remove", None);
    assert_eq!(rec.last(&format!("attachments/{id}/remove")), Some(json!({})));
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
    open(cx, &w, "T1");
    // "What to do" starts closed (foldOpen('what', true)); Details starts open.
    let text = w.update(cx, |m, _, _| view::text(&build(m).unwrap())).unwrap();
    assert!(text.ends_with("What to do"), "{text}");
    crate::prefs::set("tb.fold.what", json!("open"));
    crate::prefs::set("tb.linked", json!("closed"));
    let text = w.update(cx, |m, _, _| view::text(&build(m).unwrap())).unwrap();
    assert!(!text.contains("Pull request"), "Details closed: {text}");
    assert!(!text.ends_with("What to do"), "What to do open: {text}");
}

#[gpui_kit::test]
fn renders_every_tab(cx: &mut gpui_kit::TestAppContext) {
    let (w, _rec) = parity::window(cx);
    for r in ["T1", "T2", "T3", "T4", "T5"] {
        open(cx, &w, r);
        for tab in ["overview", "context", "log"] {
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
    let (w, _rec) = parity::window(cx);
    open(cx, &w, "T2");
    // The terminal name (`termHref`): the Sessions page on that terminal (its "Back to T2" is tested in sessions.rs).
    w.update(cx, |m, _, cx| super::go(m, &view::Go::Session("fake-s1".into()), cx)).unwrap();
    settle(cx);
    w.update(cx, |m, _, _| {
        assert!(matches!(&m.panel, Some(Panel::Task { r, .. }) if r == "T2"));
        assert_eq!(m.page, Page::Sessions);
        assert_eq!(m.sessions.selected.as_deref(), Some("fake-s1"));
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
    // The project chip: the Tasks page filtered to that project.
    let project = control(cx, &w, "open-project", None).arg;
    assert!(!project.is_empty());
    press(cx, &w, "open-project", Some(&project));
    w.update(cx, |m, _, _| {
        assert_eq!(m.page, Page::Tasks);
        assert_eq!(m.tasks.project.as_deref(), Some(project.as_str()));
    })
    .unwrap();
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
    // A goal, goal-backlog or terminal link keeps the task open: one ⌘← comes back to the page it was opened on, the next closes it.
    for link in [view::Go::Goal("G2".into()), view::Go::GoalBacklog("G2".into()), view::Go::Session("fake-s1".into())] {
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
                     "wd": {"bar": "WD", "headline": "2 open findings", "open": 2, "passed": false}, "reviewer_rows": []});
    let task = with_bar(bar, json!({"phase": "waits", "label": "Waits on base"}), "pending");
    let tree = tree(&json!({"task": task, "state": {}, "ui": {}}));
    let text = view::text(&tree);
    for want in ["WD 2 open findings", "Checks Passed", "You Waiting on you", "Review 1 of 2", "Merge Waits on base", "Stacks on T3 PR #12", "Waiting on your review Mark reviewed ›"] {
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
    assert!(tail.contains("Stacks on T3 PR #12 Tail merge") && tail.contains("Merge Waits on base"), "{tail}");
    let head = panel_text(with_bar(json!({"stacks_on": so("head")}), json!({"phase": "merge", "label": "Merging"}), "approved"), json!({}));
    assert!(head.contains("Stacks on T3 PR #12 Head merge") && !head.contains("Waits on base"), "{head}");
    let why = "T5's PR #14 head-merges into it and has to merge first";
    let parent = panel_text(with_bar(json!({"waits_on_stack": why}), json!({"phase": "stack", "label": "Waits on stack"}), "approved"), json!({}));
    assert!(parent.contains("Merge Waits on stack") && parent.contains(why), "{parent}");
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
fn the_lens_step_shows_after_checks_only_when_lens_ran() {
    let steps = |lens: Value| {
        let bar = json!({"approvals": 0, "reviewers": 0, "new_comments": 0, "reviewer_rows": [], "lens": lens});
        let p = with_bar(bar.clone(), json!({"phase": "lens", "label": "Lens review"}), "pending")["pr"].clone();
        view::pr_steps_full(&p, &bar).into_iter().map(|s| (s.name, s.sub.unwrap_or_default(), s.st)).collect::<Vec<_>>()
    };
    let names = |v: &[(String, String, view::StepSt)]| v.iter().map(|s| s.0.clone()).collect::<Vec<_>>();
    assert_eq!(names(&steps(Value::Null)), ["Checks", "You", "Review", "Merge"], "no lens run, no step");
    let running = steps(json!({"state": "running"}));
    assert_eq!(names(&running), ["Checks", "Lens", "You", "Review", "Merge"]);
    assert_eq!(running[1], ("Lens".into(), "Reviewing".into(), view::StepSt::Now));
    assert_eq!(steps(json!({"state": "done"}))[1], ("Lens".into(), "Posted".into(), view::StepSt::Done));
    assert_eq!(steps(json!({"state": "failed", "error": "boom"}))[1], ("Lens".into(), "Failed".into(), view::StepSt::Fail));
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
fn new_comments_link_to_the_first_unread_thread() {
    let thread = "https://github.com/acme/webapp/pull/13#discussion_r9";
    let tree = tree(&json!({"task": with_bar(json!({"new_comments": 1, "comments_url": thread}), json!({"phase": "comments"}), "none"), "state": {}, "ui": {}}));
    assert!(view::text(&tree).contains("Review 1 new comment"), "{}", view::text(&tree));
    assert!(format!("{tree:?}").contains(thread), "the link goes to the thread");
}

#[::core::prelude::v1::test]
fn the_review_step_says_before_its_first_round_and_opens_the_findings() {
    let pending = json!({"name": "Author review", "bar": "WD", "pending": true});
    let text = panel_text(with_bar(json!({"wd": pending}), Value::Null, "none"), json!({}));
    assert!(text.contains("WD Not reviewed by WD yet"), "{text}");
    let tree = tree(&json!({"task": with_bar(json!({"wd": {"bar": "WD", "headline": "2 open findings", "open": 2}}), Value::Null, "none"), "state": {}, "ui": {}}));
    assert!(format!("{tree:?}").contains("Findings"), "the step links to its findings: {tree:?}");
    // A task that doesn't end in a PR has none.
    let task = json!({"ref": "T8", "id": 8, "title": "x", "status": "working", "project": "webapp", "ships_pr": false, "wd": pending});
    assert!(!panel_text(task, json!({})).contains("WD"));
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
                      "wd": {"bar": "WD", "headline": "2 open findings", "open": 2, "passed": false}});
    let text = panel_text(task, json!({}));
    assert!(text.contains("Ends in a PR No PR yet.") && text.contains("WD 2 open findings"), "{text}");
    // From the detail's step results too.
    let task = json!({"ref": "T8", "id": 8, "title": "x", "status": "working", "project": "webapp",
                      "step_results": [{"name": "Author review", "bar": "WD", "headline": "No findings", "open": 0, "passed": true, "findings": []}]});
    assert!(panel_text(task, json!({})).contains("WD No findings"));
}

#[::core::prelude::v1::test]
fn a_canceled_pr_shows_why_with_its_refs_linked() {
    let task = json!({"ref": "T5", "id": 5, "title": "Fix it", "status": "done", "project": "webapp",
                      "no_pr": "T3 fixed it in ABC-7.", "ships_pr": true, "no_evidence": "no UI change"});
    let tree = tree(&json!({"task": task, "state": {"jira": {"enabled": true, "site": "acme.atlassian.net"}}, "ui": {}}));
    let text = view::text(&tree);
    assert!(text.contains("PR canceled: T3 fixed it in ABC-7 .") && text.contains("No evidence: no UI change"), "{text}");
    assert!(view::acts(&tree).iter().any(|a| a == "open-task"), "T3 opens its task");
    let links = format!("{tree:?}");
    assert!(links.contains("https://acme.atlassian.net/browse/ABC-7"), "the ticket links to Jira");
}

#[::core::prelude::v1::test]
fn the_pr_plan_shows_on_a_task_without_a_pr() {
    let t = |ships: bool| json!({"ref": "T6", "id": 6, "title": "x", "status": "queued", "project": "webapp", "ships_pr": ships});
    assert!(panel_text(t(true), json!({})).contains("Ends in a PR No PR yet."));
    assert!(panel_text(t(false), json!({})).contains("Pull request No PR"));
}

#[::core::prelude::v1::test]
fn a_steps_findings_fold_under_its_headline() {
    let task = json!({"ref": "T7", "id": 7, "title": "x", "status": "working", "project": "webapp",
                      "step_results": [{"name": "Author review", "headline": "1 open finding", "open": 1, "stale": true,
                                        "findings": [{"id": "F2", "title": "SQL injection", "severity": "high", "file": "api.rs", "line": 12},
                                                     {"id": "F1", "title": "Typo", "state": "fixed"}]}]});
    let shut = panel_text(task.clone(), json!({}));
    assert!(shut.contains("1 open finding · Findings ›") && !shut.contains("SQL injection"), "{shut}");
    let text = view::text(&tree(&json!({"task": task, "state": {}, "ui": {"folds": {"tb.fold.findings": "open"}}})));
    assert!(text.contains("Author review 1 open finding Moved since"), "{text}");
    assert!(text.contains("F2 open high SQL injection api.rs:12") && text.contains("F1 fixed Typo"), "{text}");
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
fn the_note_for_testers_shows_and_says_when_the_ticket_waits_for_it() {
    let t = |note: Value| json!({"ref": "T9", "id": 9, "title": "x", "status": "done", "project": "webapp", "merged_note": note});
    let text = panel_text(t(json!({"text": "What changed: the login page", "waiting": false})), json!({}));
    assert!(text.contains("Note for testers") && text.contains("What changed: the login page") && !text.contains("Waiting"), "{text}");
    let text = panel_text(t(json!({"text": null, "waiting": true})), json!({}));
    assert!(text.contains("Note for testers Waiting Merged without one; the ticket waits"), "{text}");
    assert!(!panel_text(t(Value::Null), json!({})).contains("Note for testers"));
}

#[::core::prelude::v1::test]
fn a_long_note_for_testers_shows_bold_and_folds_after_its_first_block() {
    let checks = (1..=4).map(|k| format!("- Reconnect to the Core {k} times; the library, queue and zones stay right.")).collect::<Vec<_>>().join("\n");
    let note = format!("**What changed:** the app no longer grows in memory on reconnect.\n\n**Please check:**\n\n{checks}");
    let task = json!({"ref": "T9", "id": 9, "title": "x", "status": "done", "project": "webapp", "merged_note": {"text": note, "waiting": false}});
    let text = panel_text(task.clone(), json!({}));
    assert!(text.contains("What changed: the app no longer") && !text.contains("**"), "{text}");
    assert!(text.contains("Show more") && !text.contains("Please check"), "{text}");
    let open = view::text(&tree(&json!({"task": task, "state": {}, "ui": {"folds": {"tb.fold.note": "open"}}})));
    assert!(open.contains("Please check: Reconnect to the Core 1 times") && open.contains("Show less") && !open.contains("Show more"), "{open}");
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
