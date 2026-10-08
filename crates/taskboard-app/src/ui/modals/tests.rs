//! Parity tests for the forms: every case in `parity/golden/forms.json` was answered by the web
//! board's own code (`parity/gen/forms.mjs`); the same inputs go through the functions the
//! native forms use. Then headless-window tests drive the real forms and check the requests.
use super::*;
// An explicit import beats the glob above, which would bring in GPUI's own `test` attribute
// (and `#[gpui_kit::test]` expands to a plain `#[test]`).
#[allow(unused_imports)]
use ::core::prelude::v1::test;
use crate::parity::{self, golden};
use serde_json::{Value, json};

/// The board a golden case was run against.
struct Case {
    state: Value,
    goals: Vec<Value>,
    filter: Value,
    page: String,
    bl: Value,
    store: Value,
}

impl Case {
    fn of(input: &Value) -> Case {
        let b = &input["board"];
        Case {
            state: b["state"].clone(),
            goals: b["goals"].as_array().cloned().unwrap_or_default(),
            filter: b["filter"].clone(),
            page: b["page"].as_str().unwrap_or("board").into(),
            bl: b["blFilter"].clone(),
            store: b["store"].clone(),
        }
    }
    fn board(&self) -> Board<'_> {
        Board {
            state: &self.state,
            goals: &self.goals,
            filter_project: self.filter["project"].as_str().unwrap_or("all"),
            filter_goal: self.filter["goal"].as_str().unwrap_or("all"),
        }
    }
    /// What `loadDraft` would find.
    fn draft(&self, key: &str) -> Option<Value> {
        self.store.get(key).cloned().filter(has_words)
    }
}

fn st(v: &Value, k: &str) -> String {
    v[k].as_str().unwrap_or("").to_string()
}

/// `S.modal` (web field names) → the native values.
fn task_vals(m: &Value) -> TaskVals {
    TaskVals {
        title: st(m, "title"),
        detail: st(m, "detail"),
        project: st(m, "project"),
        goal: st(m, "goal"),
        planned: m["planned"].as_bool().unwrap_or(false),
        priority: st(m, "priority"),
        pickup: st(m, "pickup"),
        session: st(m, "session"),
        jira_mode: st(m, "jiraMode"),
        jira_key: st(m, "jiraKey"),
        auto_close: m["auto_close"].as_bool().unwrap_or(false),
        restored: m["restored"].as_bool().unwrap_or(false),
    }
}

fn goal_vals(m: &Value) -> GoalVals {
    GoalVals {
        id: m["id"].as_str().map(str::to_string),
        name: st(m, "name"),
        tldr: st(m, "tldr"),
        outcome: st(m, "outcome"),
        project: st(m, "project"),
        epic_mode: st(m, "epicMode"),
        epic_key: st(m, "epicKey"),
        run_in_order: m["run_in_order"].as_bool().unwrap_or(false),
        max_terminals: number_or_2(&m["max_terminals"]),
        auto_close: m["auto_close"].as_bool().unwrap_or(false),
        restored: m["restored"].as_bool().unwrap_or(false),
    }
}

fn issue_vals(m: &Value) -> IssueVals {
    IssueVals { title: st(m, "title"), kind: st(m, "kindV"), goal: st(m, "goal"), project: st(m, "project"), said: st(m, "said"), detail: st(m, "detail") }
}

fn restored(r: bool) -> Value {
    if r { json!(true) } else { Value::Null }
}

fn request(r: Result<(String, Value), String>) -> Value {
    match r {
        Ok((path, body)) => json!({"err": "", "path": path, "body": body}),
        Err(e) => json!({"err": e, "path": null, "body": null}),
    }
}

fn choice_pressed(field: &str, c: &[Choice]) -> Vec<String> {
    c.iter().filter(|c| c.pressed).map(|c| format!("{field}={}", c.id)).collect()
}

/// A native view in the shape `view()` reads off the web form's HTML.
fn flat(
    title: &str,
    labels: Vec<&str>,
    legends: Vec<&str>,
    checks: Vec<(&str, bool, &str)>,
    helps: Vec<&str>,
    pickers: Vec<String>,
    pressed: Vec<String>,
    disabled: Vec<String>,
    options: Vec<(String, String)>,
    inputs: Vec<String>,
    draft: Option<String>,
    err: Option<String>,
    submit: &str,
) -> Value {
    json!({
        "title": title, "labels": labels, "legends": legends,
        "checks": checks.iter().map(|(f, on, l)| json!({"field": f, "on": on, "label": l})).collect::<Vec<_>>(),
        "helps": helps, "pickers": pickers, "pressed": pressed, "disabled": disabled,
        "options": options.iter().map(|(a, b)| json!([a, b])).collect::<Vec<_>>(),
        "inputs": inputs, "draft": draft, "err": err, "submit": submit,
    })
}

fn flat_task(v: &TaskView) -> Value {
    let mut legends = vec!["Priority"];
    let mut pressed = choice_pressed("priority", &v.priority);
    let mut disabled = vec![];
    if let Some(start) = &v.start {
        legends.push("Start");
        pressed.extend(choice_pressed("pickup", start));
        disabled.extend(start.iter().filter(|c| c.disabled).map(|c| format!("pickup={}", c.id)));
    }
    let mut inputs = vec![];
    if let Some((modes, key)) = &v.jira {
        legends.push("Jira ticket");
        pressed.extend(choice_pressed("jiraMode", modes));
        if *key {
            inputs.push("nt-jira:PROJ-123".to_string());
        }
    }
    let mut checks = vec![];
    if let Some(on) = v.planned {
        checks.push(("planned", on, CHECK_PLANNED));
    }
    checks.push(("auto_close", v.auto_close, CHECK_TASK_CLOSE));
    flat(
        "New task",
        vec!["Title", "What to do", "Project", "Goal"],
        legends,
        checks,
        vec![HELP_DETAIL, v.help],
        vec![v.project_label.clone(), v.goal_label.clone()],
        pressed,
        disabled,
        v.terminals.clone(),
        inputs,
        v.draft.clone(),
        v.err.clone(),
        v.submit,
    )
}

fn flat_goal(v: &GoalView) -> Value {
    let mut labels = vec!["Name", "TLDR", "Done when", "Project"];
    let mut legends = vec![];
    let mut helps = vec![HELP_TLDR, HELP_OUTCOME];
    let mut pressed = vec![];
    let mut inputs = vec![];
    if let Some((hint, help)) = v.epic_field {
        labels.push("Jira epic");
        helps.push(help);
        inputs.push(format!("ng-epic:{hint}"));
    }
    if let Some((modes, key, help)) = &v.epic_choices {
        legends.push("Jira epic");
        pressed.extend(choice_pressed("epicMode", modes));
        if *key {
            inputs.push("ng-epic:PROJ-123".to_string());
        }
        helps.push(help);
    }
    flat(
        v.title,
        labels,
        legends,
        vec![("run_in_order", v.run_in_order, CHECK_ORDER), ("auto_close", v.auto_close, CHECK_GOAL_CLOSE)],
        helps,
        vec![v.project_label.clone()],
        pressed,
        vec![],
        vec![],
        inputs,
        v.draft.clone(),
        v.err.clone(),
        v.submit,
    )
}

fn flat_issue(v: &IssueView) -> Value {
    flat(
        "Add an issue",
        vec!["Title", "Goal", "Project", "What you saw", "More detail (optional)"],
        vec!["Type"],
        vec![],
        vec![HELP_ISSUE],
        vec![v.goal_label.clone(), v.project_label.clone()],
        choice_pressed("kindV", &v.kinds),
        vec![],
        vec![],
        vec![],
        None,
        v.err.clone(),
        v.submit,
    )
}

fn specials(v: &Value) -> Vec<(String, String)> {
    v.as_array().map(|a| a.iter().map(|p| (p[0].as_str().unwrap_or("").to_string(), p[1].as_str().unwrap_or("").to_string())).collect()).unwrap_or_default()
}

/// The select in the web goal form keeps `max_terminals` as text ("4"); the app keeps a number.
fn number_max(v: &mut Value) {
    if let Some(Value::String(s)) = v.get("max_terminals") {
        let n = number_or_2(&json!(s));
        v["max_terminals"] = json!(n);
    }
}

#[::core::prelude::v1::test]
fn forms_match_web() {
    let mut g = golden("forms");
    for c in &mut g.cases {
        let f = c["input"]["fn"].as_str().unwrap_or("").to_string();
        match f.as_str() {
            "openGoal" => number_max(&mut c["expect"]),
            "saveDraft" => {
                for k in ["task", "goal"] {
                    if c["expect"][k].is_object() {
                        number_max(&mut c["expect"][k]);
                    }
                }
            }
            _ => {}
        }
    }
    g.check(|i| {
        let case = Case::of(i);
        let board = case.board();
        match i["fn"].as_str().unwrap() {
            "openTask" => {
                let o = &i["o"];
                let opts = TaskFormOpts { goal_id: o["goal"].as_str().and_then(id_of), planned: o["planned"].as_bool().unwrap_or(false), ..Default::default() };
                let v = open_task_vals(&board, &opts, case.draft(DRAFT_TASK).as_ref());
                json!({"title": v.title, "detail": v.detail, "project": v.project, "goal": v.goal, "planned": v.planned, "priority": v.priority,
                       "pickup": v.pickup, "session": v.session, "jiraMode": v.jira_mode, "jiraKey": v.jira_key, "auto_close": v.auto_close,
                       "restored": restored(v.restored)})
            }
            "openGoal" => {
                let goal = i["goal"].as_str().and_then(|r| board.goal_by_ref(r));
                let draft = if goal.is_none() { case.draft(DRAFT_GOAL) } else { None };
                let v = open_goal_vals(&board, goal, draft.as_ref());
                json!({"id": v.id, "name": v.name, "tldr": v.tldr, "outcome": v.outcome, "project": v.project, "epicMode": v.epic_mode,
                       "epicKey": v.epic_key, "run_in_order": v.run_in_order, "max_terminals": v.max_terminals, "auto_close": v.auto_close,
                       "restored": restored(v.restored)})
            }
            "openIssue" => {
                let backlog = (case.page == "backlog").then(|| (st(&case.bl, "project"), st(&case.bl, "goal")));
                let v = open_issue_vals(&board, i["goal"].as_str(), backlog.as_ref().map(|(p, g)| (p.as_str(), g.as_str())));
                json!({"title": v.title, "kindV": v.kind, "goal": v.goal, "project": v.project, "said": v.said, "detail": v.detail})
            }
            "submitTask" => request(task_request(&task_vals(&i["modal"]), &board)),
            "submitGoal" => request(goal_request(&goal_vals(&i["modal"]), &board)),
            "submitIssue" => request(issue_request(&issue_vals(&i["modal"]))),
            "picker" => {
                let p = &i["picker"];
                let kind = if p["kind"] == "goal" { PickKind::Goal } else { PickKind::Project };
                let sp = specials(&p["specials"]);
                let sp: Vec<(&str, &str)> = sp.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
                let value = st(p, "value");
                let q = st(p, "q");
                let items = picker_items(&board, kind, &value, &sp, &st(p, "proj"), &q);
                json!({
                    "items": items.iter().map(|it| json!({"v": it.v, "name": it.name, "sub": it.sub, "special": it.special, "current": it.current})).collect::<Vec<_>>(),
                    "active": picker_start(&items, &value),
                    "empty": if items.is_empty() { json!(picker_empty(kind, &q)) } else { Value::Null },
                    "search": picker_search(kind),
                })
            }
            "pickerButton" => {
                let kind = if i["kind"] == "goal" { PickKind::Goal } else { PickKind::Project };
                let sp: &[(&str, &str)] = if kind == PickKind::Goal { &NO_GOAL } else { &[] };
                json!(picker_label(&board, kind, &st(i, "value"), sp))
            }
            "saveDraft" => {
                let m = &i["modal"];
                let mut store = case.store.clone();
                let form = match m["kind"].as_str() {
                    Some("task") => FormVals::Task(task_vals(m)),
                    Some("goal") => FormVals::Goal(goal_vals(m)),
                    _ => FormVals::Issue,
                };
                if let Some((key, d)) = draft_write(&form, m["busy"].as_bool().unwrap_or(false)) {
                    match d {
                        Some(d) => store[key] = d,
                        None => {
                            store.as_object_mut().map(|o| o.remove(key));
                        }
                    }
                }
                json!({"task": store.get(DRAFT_TASK).cloned().unwrap_or(Value::Null), "goal": store.get(DRAFT_GOAL).cloned().unwrap_or(Value::Null)})
            }
            "taskView" => {
                let m = &i["modal"];
                flat_task(&task_view(&task_vals(m), &board, m["err"].as_str(), m["busy"].as_bool().unwrap_or(false)))
            }
            "goalView" => {
                let m = &i["modal"];
                flat_goal(&goal_view(&goal_vals(m), &board, m["err"].as_str(), m["busy"].as_bool().unwrap_or(false)))
            }
            "issueView" => {
                let m = &i["modal"];
                flat_issue(&issue_view(&issue_vals(m), &board, m["err"].as_str(), m["busy"].as_bool().unwrap_or(false)))
            }
            "localeSort" => {
                let mut v: Vec<String> = i["list"].as_array().unwrap().iter().map(|x| x.as_str().unwrap().to_string()).collect();
                v.sort_by(|a, b| locale_cmp(a, b));
                json!(v)
            }
            f => panic!("no app function for {f}"),
        }
    });
}

/// Native difference: a terminal picked before the project changed isn't sent (the web's select
/// showed "Pick a terminal" for it but still sent the old id).
#[::core::prelude::v1::test]
fn a_terminal_from_another_project_is_not_sent() {
    let state = json!({"projects": [{"name": "webapp"}, {"name": "api"}], "sessions": [
        {"id": "s2", "name": "webapp shell", "project": "webapp", "can_take": true},
        {"id": "s3", "name": "api shell", "project": "api", "can_take": true}]});
    let board = Board { state: &state, goals: &[], filter_project: "all", filter_goal: "all" };
    let v = TaskVals {
        title: "x".into(),
        detail: String::new(),
        project: "api".into(),
        goal: String::new(),
        planned: false,
        priority: "normal".into(),
        pickup: "attach".into(),
        session: "s2".into(),
        jira_mode: "none".into(),
        jira_key: String::new(),
        auto_close: true,
        restored: false,
    };
    assert_eq!(task_request(&v, &board), Err("Pick the terminal to hand it to.".into()));
}

// ------------------------------------------------------------------ the forms in a window

use gpui_kit::{TestAppContext, WindowHandle};

type W = WindowHandle<MainWindow>;

fn task_form_mut(m: &mut MainWindow) -> &mut TaskForm {
    match m.modal.as_mut() {
        Some(Modal::Task(f)) => f,
        _ => panic!("no task form open"),
    }
}

/// Draw the window once (forms save their draft while they render).
fn redraw(w: W, cx: &mut TestAppContext) {
    cx.update_window(w.into(), |_, window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    })
    .unwrap();
    parity::settle(cx);
}

#[gpui_kit::test]
fn new_task_posts_the_web_body_and_opens_the_task(cx: &mut TestAppContext) {
    let (w, rec) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        open_task_form(m, TaskFormOpts::default(), window, cx);
        let f = task_form_mut(m);
        f.title.set_text("  Add a health check  ", cx);
        f.detail.set_text("GET /health returns 200", cx);
        f.v.priority = "high".into();
        f.v.pickup = "manual".into();
        submit_task(m, window, cx);
    })
    .unwrap();
    parity::settle(cx);
    let body = rec.last("tasks").expect("POST /tasks");
    assert_eq!(
        body,
        json!({"title": "Add a health check", "detail": "GET /health returns 200", "project": "api", "priority": "high",
               "goal_id": null, "auto_close": true, "pickup": {"mode": "manual"}})
    );
    w.update(cx, |m, _, _| {
        assert!(m.modal.is_none(), "the form closes");
        assert!(matches!(&m.panel, Some(crate::app::Panel::Task { r, .. }) if r.starts_with('T')), "the new task opens");
        assert!(m.toasts.iter().any(|t| t.text.starts_with("Added T")));
    })
    .unwrap();
}

#[gpui_kit::test]
fn add_task_to_a_goal_plans_it(cx: &mut TestAppContext) {
    let (w, rec) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        open_task_form(m, TaskFormOpts { goal_id: Some(1), planned: true, project: Some("webapp".into()), ..Default::default() }, window, cx);
        let f = task_form_mut(m);
        assert_eq!((f.v.goal.as_str(), f.v.project.as_str(), f.v.planned), ("G1", "webapp", true));
        f.title.set_text("Docs", cx);
        submit_task(m, window, cx);
    })
    .unwrap();
    parity::settle(cx);
    let body = rec.last("tasks").unwrap();
    assert_eq!((body["goal_id"].clone(), body["status"].clone(), body["pickup"].clone()), (json!(1), json!("planned"), json!({"mode": "queue"})));
}

#[gpui_kit::test]
fn a_task_without_a_title_says_so_and_sends_nothing(cx: &mut TestAppContext) {
    let (w, rec) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        open_task_form(m, TaskFormOpts::default(), window, cx);
        submit_task(m, window, cx);
        assert_eq!(task_form_mut(m).err.as_deref(), Some("Give the task a title."));
    })
    .unwrap();
    parity::settle(cx);
    assert!(rec.posts().is_empty());
}

#[gpui_kit::test]
fn a_segment_click_clears_the_error(cx: &mut TestAppContext) {
    let (w, _) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        open_task_form(m, TaskFormOpts::default(), window, cx);
        submit_task(m, window, cx);
        // What a priority click does (`m-set`).
        let x = m.modal.as_mut().unwrap();
        if let Modal::Task(f) = x {
            f.v.priority = "high".into();
        }
        x.set_err(None);
        assert!(task_form_mut(m).err.is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn picking_a_goal_changes_only_the_goal(cx: &mut TestAppContext) {
    let (w, _) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        open_task_form(m, TaskFormOpts::default(), window, cx);
        assert_eq!(task_form_mut(m).v.project, "api");
        let auto = task_form_mut(m).v.auto_close;
        open_picker(m, PickKind::Goal, window, cx);
        // "Not in a goal" is current, so ↓ then ↩ picks the first goal.
        assert!(picker_key(m, PickKind::Goal, "down", window, cx));
        assert!(picker_key(m, PickKind::Goal, "enter", window, cx));
        let f = task_form_mut(m);
        assert_eq!(f.v.goal, "G1");
        assert_eq!(f.v.project, "api", "the web keeps the project");
        assert_eq!(f.v.auto_close, auto);
        assert!(f.picker.open.is_none());
    })
    .unwrap();
}

#[gpui_kit::test]
fn picker_keys_wrap_and_escape_closes_only_the_picker(cx: &mut TestAppContext) {
    let (w, _) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        open_task_form(m, TaskFormOpts::default(), window, cx);
        open_picker(m, PickKind::Project, window, cx);
        // api is current: the first of [api, webapp].
        assert_eq!(task_form_mut(m).picker.active, 0);
        picker_key(m, PickKind::Project, "up", window, cx);
        assert_eq!(task_form_mut(m).picker.active, 1, "↑ from the top wraps to the bottom");
        picker_key(m, PickKind::Project, "down", window, cx);
        assert_eq!(task_form_mut(m).picker.active, 0);
        on_escape(m, window, cx);
        assert!(task_form_mut(m).picker.open.is_none());
        on_escape(m, window, cx);
        assert!(m.modal.is_some(), "esc never closes a form");
    })
    .unwrap();
}

#[gpui_kit::test]
fn escape_keys_reach_the_form_not_the_window(cx: &mut TestAppContext) {
    let (w, _) = parity::window(cx);
    w.update(cx, |m, window, cx| open_task_form(m, TaskFormOpts::default(), window, cx)).unwrap();
    redraw(w, cx);
    cx.simulate_keystrokes(w.into(), "escape");
    parity::settle(cx);
    w.update(cx, |m, _, _| assert!(m.modal.is_some(), "esc keeps the form open")).unwrap();
    w.update(cx, |m, window, cx| {
        m.set_modal(None, window, cx);
        // An alert, so the dialog doesn't close by itself.
        m.data.state.as_mut().unwrap()["alerts"] = json!([{"id": "a1", "at": "2026-10-07T14:00:00Z", "text": "T3 couldn't start.", "task": "T3", "goal": null}]);
        m.set_modal(Some(Modal::Alerts), window, cx);
    })
    .unwrap();
    redraw(w, cx);
    w.update(cx, |m, _, _| assert!(matches!(m.modal, Some(Modal::Alerts)), "the dialog stays while there are alerts")).unwrap();
    cx.simulate_keystrokes(w.into(), "escape");
    parity::settle(cx);
    w.update(cx, |m, _, _| assert!(m.modal.is_none(), "esc closes the alerts dialog")).unwrap();
}

#[gpui_kit::test]
fn the_alerts_dialog_closes_itself_when_none_are_left(cx: &mut TestAppContext) {
    let (w, _) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        m.data.state.as_mut().unwrap()["alerts"] = json!([{"id": "a1", "at": "2026-10-07T14:00:00Z", "text": "T3 couldn't start.", "task": "T3", "goal": null}]);
        m.set_modal(Some(Modal::Alerts), window, cx);
    })
    .unwrap();
    redraw(w, cx);
    w.update(cx, |m, _, _| {
        assert!(matches!(m.modal, Some(Modal::Alerts)));
        m.data.state.as_mut().unwrap()["alerts"] = json!([]);
    })
    .unwrap();
    redraw(w, cx);
    w.update(cx, |m, _, _| assert!(m.modal.is_none())).unwrap();
}

#[gpui_kit::test]
fn a_task_draft_comes_back_and_can_be_discarded(cx: &mut TestAppContext) {
    let (w, _) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        open_task_form(m, TaskFormOpts::default(), window, cx);
        task_form_mut(m).title.set_text("Half typed", cx);
    })
    .unwrap();
    redraw(w, cx);
    assert_eq!(crate::prefs::get(DRAFT_TASK).map(|d| d["title"].clone()), Some(json!("Half typed")));
    w.update(cx, |m, window, cx| {
        close(m, window, cx);
        open_task_form(m, TaskFormOpts::default(), window, cx);
        let v = task_form_mut(m).vals(cx);
        assert!(v.restored);
        assert_eq!(v.title, "Half typed");
        discard_draft(m, window, cx);
        let v = task_form_mut(m).vals(cx);
        assert!(!v.restored && v.title.is_empty());
    })
    .unwrap();
    assert!(crate::prefs::get(DRAFT_TASK).is_none());
}

#[gpui_kit::test]
fn adding_a_task_clears_its_draft(cx: &mut TestAppContext) {
    let (w, _) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        open_task_form(m, TaskFormOpts::default(), window, cx);
        task_form_mut(m).title.set_text("Ship it", cx);
    })
    .unwrap();
    redraw(w, cx);
    assert!(crate::prefs::get(DRAFT_TASK).is_some());
    w.update(cx, |m, window, cx| submit_task(m, window, cx)).unwrap();
    parity::settle(cx);
    redraw(w, cx);
    assert!(crate::prefs::get(DRAFT_TASK).is_none());
}

#[gpui_kit::test]
fn new_goal_posts_and_goes_to_its_page(cx: &mut TestAppContext) {
    let (w, rec) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        open_goal_form(m, None, window, cx);
        let Some(Modal::Goal(f)) = m.modal.as_mut() else { panic!() };
        f.name.set_text("Faster CI", cx);
        f.v.max_terminals = 3;
        submit_goal(m, window, cx);
    })
    .unwrap();
    parity::settle(cx);
    assert_eq!(
        rec.last("goals").unwrap(),
        json!({"name": "Faster CI", "tldr": "", "outcome": "", "project": "api", "run_in_order": true, "max_terminals": 3,
               "auto_close": true, "epic": {"mode": "none"}})
    );
    w.update(cx, |m, _, _| assert!(matches!(&m.page, Page::Goal(r) if r.starts_with('G') && r != "G1"))).unwrap();
}

#[gpui_kit::test]
fn edit_goal_posts_to_the_goal(cx: &mut TestAppContext) {
    let (w, rec) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        let g = m.data.goals[0].clone();
        open_goal_form(m, Some(g), window, cx);
        let Some(Modal::Goal(f)) = m.modal.as_mut() else { panic!() };
        assert_eq!(f.v.id.as_deref(), Some("G1"));
        f.name.set_text("Passkeys everywhere", cx);
        submit_goal(m, window, cx);
    })
    .unwrap();
    parity::settle(cx);
    let body = rec.last("goals/G1").expect("POST /goals/G1");
    assert_eq!(body["name"], json!("Passkeys everywhere"));
    assert!(body.get("epic").is_none() && body.get("epic_key").is_none(), "no Jira on the sample board");
    w.update(cx, |m, _, _| {
        assert!(m.modal.is_none());
        assert!(m.toasts.iter().any(|t| t.text == "Goal saved"));
    })
    .unwrap();
}

#[gpui_kit::test]
fn an_issue_added_on_the_board_stays_on_the_board(cx: &mut TestAppContext) {
    let (w, rec) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        open_issue_form(m, None, window, cx);
        let Some(Modal::Issue(f)) = m.modal.as_mut() else { panic!() };
        f.title.set_text("Flaky login test", cx);
        f.said.set_text("  It fails one run in ten  ", cx);
        f.v.kind = "gap".into();
        submit_issue(m, window, cx);
    })
    .unwrap();
    parity::settle(cx);
    assert_eq!(
        rec.last("backlog").unwrap(),
        json!({"title": "Flaky login test", "kind": "gap", "goal_id": null, "project": "api", "said": "It fails one run in ten"})
    );
    w.update(cx, |m, _, _| {
        assert!(m.modal.is_none());
        assert!(m.panel.is_none(), "on the board the web doesn't open it");
    })
    .unwrap();
}

#[gpui_kit::test]
fn an_issue_added_from_a_goal_opens(cx: &mut TestAppContext) {
    let (w, rec) = parity::window(cx);
    w.update(cx, |m, window, cx| {
        m.go(Page::Goal("G1".into()), cx);
        open_issue_form(m, Some("G1".into()), window, cx);
        let Some(Modal::Issue(f)) = m.modal.as_mut() else { panic!() };
        assert_eq!((f.v.goal.as_str(), f.v.project.as_str()), ("G1", "webapp"));
        f.title.set_text("Missing test", cx);
        submit_issue(m, window, cx);
    })
    .unwrap();
    parity::settle(cx);
    assert_eq!(rec.last("backlog").unwrap()["goal_id"], json!(1));
    w.update(cx, |m, _, _| assert!(matches!(&m.panel, Some(crate::app::Panel::Issue { r }) if r.starts_with('B')))).unwrap();
}
