//! The task panel against the web board: every panel state from `parity/golden/task.json` (what
//! the web showed, its controls, tooltips and fields), and every action's request.
use super::view::{self, Ctx, Ui};
use super::{Act, act, build, on_input, save_meta, ui};
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
        reveal: v["reveal"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
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
fn answer_sends_text_and_clears_the_draft(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    open(cx, &w, "T3");
    // Empty: the web's "Write an answer first." and no request.
    press(cx, &w, "answer", Some(""));
    assert!(rec.last("tasks/T3/answer").is_none());
    assert_eq!(note("ask:T3"), Some(("Write an answer first.".into(), true)));
    typed(cx, &w, "answer:T3", "  Yes, ask for the password.  ");
    press(cx, &w, "answer", Some(""));
    assert_eq!(rec.last("tasks/T3/answer"), Some(json!({"text": "Yes, ask for the password.", "when": "now"})));
    assert_eq!(ui(|u| u.drafts.get("answer:T3").cloned()), None, "draft cleared");
    assert_eq!(note("ask:T3").map(|n| n.0), Some(sent_note(cx, &w)));
}

#[gpui_kit::test]
fn answer_later_waits_for_work_hours(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    // Close the work hours so "Send at …" is offered.
    rec.board().post("hours", json!({"on": true, "start": "00:00", "end": "00:30", "days": []})).unwrap();
    w.update(cx, |m, _, cx| m.refresh(cx)).unwrap();
    open(cx, &w, "T3");
    typed(cx, &w, "answer:T3", "Tomorrow is fine");
    press(cx, &w, "answer", Some("morning"));
    assert_eq!(rec.last("tasks/T3/answer"), Some(json!({"text": "Tomorrow is fine", "when": "morning"})));
    let msg = note("ask:T3").unwrap().0;
    assert!(msg.starts_with("Saved. It sends at "), "{msg}");
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
fn mark_done_and_failed(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    open(cx, &w, "T2");
    press(cx, &w, "reveal", Some("done:T2"));
    assert!(control(cx, &w, "hide", Some("done:T2")).act == "hide", "the form offers Cancel");
    typed(cx, &w, "summary:T2", " Shipped it ");
    press(cx, &w, "mark-done", None);
    assert_eq!(rec.last("tasks/T2/done"), Some(json!({"summary": "Shipped it"})));
    assert!(!ui(|u| u.reveal.contains("done:T2")), "the form closes");

    let (w, rec) = parity::window(cx);
    open(cx, &w, "T2");
    press(cx, &w, "reveal", Some("fail:T2"));
    press(cx, &w, "mark-fail", None);
    assert_eq!(rec.last("tasks/T2/fail"), Some(json!({})), "an empty reason sends {{}}");
}

#[gpui_kit::test]
fn lost_task_resumes(cx: &mut gpui_kit::TestAppContext) {
    let (w, rec) = parity::window(cx);
    w.update(cx, |m, _, cx| {
        act(m, &Act::new("resume", "fresh", "T3", "lost:T3"), cx);
        act(m, &Act::new("resume", "reopen", "T3", "lost:T3"), cx);
    })
    .unwrap();
    settle(cx);
    let resumes: Vec<Value> = rec.posts().into_iter().filter(|(p, _)| p == "tasks/T3/resume").map(|(_, b)| b).collect();
    assert_eq!(resumes, vec![json!({"mode": "fresh"}), json!({"mode": "reopen"})]);
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
fn links_leave_the_panel(cx: &mut gpui_kit::TestAppContext) {
    use crate::app::Page;
    let (w, _rec) = parity::window(cx);
    open(cx, &w, "T2");
    // The terminal name (`termHref`): the Sessions page on that terminal (its "Back to T2" is tested in sessions.rs).
    w.update(cx, |m, _, cx| super::go(m, &view::Go::Session("fake-s1".into()), cx)).unwrap();
    settle(cx);
    w.update(cx, |m, _, _| {
        assert!(m.panel.is_none());
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
    // The goal's name: its page, panel closed.
    open(cx, &w, "T2");
    w.update(cx, |m, _, cx| super::go(m, &view::Go::Goal("G1".into()), cx)).unwrap();
    w.update(cx, |m, _, _| {
        assert_eq!(m.page, Page::Goal("G1".into()));
        assert!(m.panel.is_none());
    })
    .unwrap();
}
