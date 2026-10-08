//! The task side panel: Overview, Context and Log tabs, answers, PR steps and every task action.
//!
//! A port of the web board's `taskPanel()` and friends (app.js). `view.rs` builds the panel as a
//! tree of nodes (the same text, controls and tooltips the web showed, checked against the web's
//! own output by the parity tests); this file draws that tree and runs its controls through
//! [`act`], a port of the web's `ACTIONS` table, with the same requests, inline notes and busy
//! states.
mod view;

use crate::app::{MainWindow, Page, Panel, TaskTab};
use crate::fmt::{self, s};
use crate::theme::Theme;
use crate::ui::kit;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
pub use view::{Act, Go, Node};
use view::{BoxTone, Dot, Fold, Icon, K, LinkLook, Look, St, StepSt, Tone};

pub const WIDTH: f32 = 580.;

/// How long an inline note stays (`setNote`: 5 s, errors 15 s).
const NOTE_OK: Duration = Duration::from_secs(5);
const NOTE_ERR: Duration = Duration::from_secs(15);

thread_local! {
    /// The web's page-global `S` bits the panel uses (drafts, busy buttons, notes, open forms,
    /// the log filter…). Kept here rather than in `State` because they outlive switching tasks,
    /// as they did in the web board.
    static UI: RefCell<view::Ui> = RefCell::new(view::Ui::default());
    /// `?from=` (the tasks opened from the panel before this one) and the task it belongs to.
    static TRAIL: RefCell<(String, Vec<String>)> = RefCell::new((String::new(), Vec::new()));
    /// Handoffs being fetched (`loadHandoff`), and note generations (so an old timer doesn't
    /// clear a newer note).
    static LOADING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
    static NOTE_GEN: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
}

fn ui<R>(f: impl FnOnce(&mut view::Ui) -> R) -> R {
    UI.with(|u| f(&mut u.borrow_mut()))
}

/// Per-task GPUI state: the text fields drawn in the panel, by their web key (`data-k`).
#[derive(Default)]
pub struct State {
    r: String,
    fields: HashMap<String, kit::Input>,
    subs: Vec<Subscription>,
    /// Focus this field on the next draw (`reveal` focuses the new textarea).
    focus_next: Option<String>,
    /// The field that had focus at the last draw.
    focused: Option<FocusHandle>,
}

// ------------------------------------------------------------------ building the view

fn tab_name(tab: TaskTab) -> &'static str {
    match tab {
        TaskTab::Overview => "overview",
        TaskTab::Context => "context",
        TaskTab::Log => "log",
    }
}

fn folds() -> HashMap<String, String> {
    [Fold::Linked, Fold::Summary, Fold::What, Fold::Terms]
        .iter()
        .filter_map(|f| crate::prefs::get_str(f.key()).map(|v| (f.key().to_string(), v)))
        .collect()
}

/// The panel's tree for the current task and UI state.
pub fn build(m: &MainWindow) -> Option<Node> {
    let Some(Panel::Task { r, tab }) = &m.panel else { return None };
    let trail = TRAIL.with(|t| {
        let t = t.borrow();
        if t.0 == *r { t.1.clone() } else { Vec::new() }
    });
    let goal_page = match &m.page {
        Page::Goal(g) => Some(g.as_str()),
        _ => None,
    };
    let folds = folds();
    let u = UI.with(|u| u.borrow().clone());
    let null = Value::Null;
    let c = view::Ctx {
        state: m.data.state.as_ref().unwrap_or(&null),
        task: m.data.task.as_ref().filter(|t| fmt::ref_of(t, "T") == *r),
        r,
        tab: tab_name(*tab),
        trail: &trail,
        goal_page,
        err: m.data.errs.task.as_deref(),
        ui: &u,
        folds: &folds,
    };
    Some(view::panel(&c))
}

// ------------------------------------------------------------------ actions (the web's ACTIONS)

fn set_note(grp: &str, text: impl Into<String>, err: bool, cx: &mut Context<MainWindow>) {
    let grp = grp.to_string();
    ui(|u| u.notes.insert(grp.clone(), (text.into(), err)));
    let generation = NOTE_GEN.with(|g| {
        let mut g = g.borrow_mut();
        let n = g.entry(grp.clone()).or_default();
        *n += 1;
        *n
    });
    cx.spawn(async move |this, cx| {
        cx.background_executor().timer(if err { NOTE_ERR } else { NOTE_OK }).await;
        let _ = this.update(cx, |_, cx| {
            if NOTE_GEN.with(|g| g.borrow().get(&grp).copied()) == Some(generation) {
                ui(|u| u.notes.remove(&grp));
                cx.notify();
            }
        });
    })
    .detach();
    cx.notify();
}

/// The web's `run(el, fn, {ok})`: one request per button at a time (it reads "Sending…"
/// meanwhile), the group's note cleared first, then the `ok` sentence (or the board's error) in
/// the group's note, or a toast when the button has no group.
fn run(m: &mut MainWindow, a: &Act, path: String, body: Value, ok: impl FnOnce(&mut MainWindow, &Value) -> String + 'static, cx: &mut Context<MainWindow>) {
    let key = a.busy_key();
    if ui(|u| !u.busy.insert(key.clone())) {
        return;
    }
    let grp = a.grp.clone();
    if !grp.is_empty() {
        ui(|u| u.notes.remove(&grp));
    }
    cx.notify();
    let (k2, g2) = (key.clone(), grp.clone());
    m.post_or(
        path,
        body,
        cx,
        move |m, v, cx| {
            ui(|u| u.busy.remove(&key));
            let msg = ok(m, &v);
            if !grp.is_empty() && !msg.is_empty() {
                set_note(&grp, msg, false, cx);
            }
            cx.notify();
        },
        move |m, e, cx| {
            ui(|u| u.busy.remove(&k2));
            if g2.is_empty() {
                m.toast(e.message, true, cx);
            } else {
                set_note(&g2, e.message, true, cx);
            }
        },
    );
}

fn sent(m: &MainWindow) -> String {
    view::sent_note(m.state()).to_string()
}

/// `openTask(r, null, true)`: open another task from inside the panel, remembering this one.
fn open_from_panel(m: &mut MainWindow, next: &str, cx: &mut Context<MainWindow>) {
    let cur = match &m.panel {
        Some(Panel::Task { r, .. }) => r.clone(),
        _ => String::new(),
    };
    let next = fmt::ref_of(&json!(next), "T");
    TRAIL.with(|t| {
        let mut t = t.borrow_mut();
        let mut items = if t.0 == cur { t.1.clone() } else { Vec::new() };
        if !cur.is_empty() && cur != next {
            items.push(cur.clone());
        } else {
            items.clear();
        }
        *t = (next.clone(), items);
    });
    m.open_task(next, cx);
}

/// Forget an edited field so it's drawn again from its draft.
fn drop_field(m: &mut MainWindow, key: &str) {
    m.task_panel.fields.remove(key);
}

/// `saveMeta(el, r, msg)`.
fn save_meta(m: &mut MainWindow, r: &str, msg: &'static str, cx: &mut Context<MainWindow>) {
    let rows: Vec<Value> = ui(|u| u.meta_edit.get(r).cloned().unwrap_or_default())
        .into_iter()
        .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
        .filter(|(k, v)| !k.is_empty() || !v.is_empty())
        .map(|(k, v)| json!([k, v]))
        .collect();
    let a = Act::new("meta", "", r, format!("meta:{r}"));
    let r2 = r.to_string();
    run(
        m,
        &a,
        format!("tasks/{r}/meta"),
        json!({ "meta": rows }),
        move |m, _| {
            // Keep the edit while one of its fields still has focus (the reader is still typing).
            let prefix = format!("meta:{r2}:");
            let editing = m.task_panel.focused.as_ref().is_some_and(|f| m.task_panel.fields.iter().any(|(k, i)| k.starts_with(&prefix) && i.focus == *f));
            if !editing {
                ui(|u| u.meta_edit.remove(&r2));
                m.task_panel.fields.retain(|k, _| !k.starts_with(&format!("meta:{r2}:")));
            }
            msg.to_string()
        },
        cx,
    );
}

/// Run one control, exactly as the web board's `ACTIONS[act]` did.
pub fn act(m: &mut MainWindow, a: &Act, cx: &mut Context<MainWindow>) {
    let id = a.id.clone();
    let tp = |suffix: &str| format!("tasks/{id}{suffix}");
    match a.act {
        "close-panel" => {
            TRAIL.with(|t| *t.borrow_mut() = (String::new(), Vec::new()));
            m.close_panel(cx);
        }
        "task-back" => {
            let target = TRAIL.with(|t| {
                let mut t = t.borrow_mut();
                let target = t.1.pop();
                if let Some(x) = &target {
                    t.0 = x.clone();
                }
                target
            });
            if let Some(x) = target {
                m.open_task(x, cx);
            }
        }
        "open-task" => open_from_panel(m, &id, cx),
        "tab" => {
            if let Some(Panel::Task { tab, .. }) = m.panel.as_mut() {
                *tab = match a.arg.as_str() {
                    "context" => TaskTab::Context,
                    "log" => TaskTab::Log,
                    _ => TaskTab::Overview,
                };
            }
            cx.notify();
        }
        "log-filter" => {
            ui(|u| u.log_filter = a.arg.clone());
            cx.notify();
        }
        "reveal" => {
            ui(|u| u.reveal.insert(a.arg.clone()));
            let (kind, rest) = a.arg.split_once(':').unwrap_or((&a.arg, ""));
            let field = match kind {
                "done" => "summary",
                "fail" => "reason",
                other => other,
            };
            m.task_panel.focus_next = Some(format!("{field}:{rest}"));
            cx.notify();
        }
        "hide" => {
            ui(|u| u.reveal.remove(&a.arg));
            cx.notify();
        }
        "start" => {
            let body = json!({"mode": a.arg});
            run(m, a, tp("/start"), body, |m, _| sent(m), cx);
        }
        "queue-planned" => run(m, a, tp(""), json!({"status": "queued"}), |_, _| "Queued".into(), cx),
        "answer" => {
            let k = format!("answer:{id}");
            let text = ui(|u| u.drafts.get(&k).cloned().unwrap_or_default()).trim().to_string();
            if text.is_empty() {
                set_note(&a.grp, "Write an answer first.", true, cx);
                return;
            }
            let morning = a.arg == "morning";
            run(
                m,
                a,
                tp("/answer"),
                json!({"text": text, "when": if morning { "morning" } else { "now" }}),
                move |m, _| {
                    ui(|u| u.drafts.remove(&k));
                    drop_field(m, &k);
                    if morning {
                        let at = view::hours_open_at(m.state());
                        format!("Saved. It sends at {}.", if at.is_empty() { "the start of work hours".to_string() } else { at })
                    } else {
                        sent(m)
                    }
                },
                cx,
            );
        }
        "requeue" => run(m, a, tp("/requeue"), json!({}), |_, _| "Back in the queue".into(), cx),
        "detach" => run(m, a, tp("/detach"), json!({}), |_, _| "Detached. It’s back in the queue.".into(), cx),
        "mark-done" | "mark-fail" => {
            let done = a.act == "mark-done";
            let k = format!("{}:{id}", if done { "summary" } else { "reason" });
            let text = ui(|u| u.drafts.get(&k).cloned().unwrap_or_default()).trim().to_string();
            let body = if text.is_empty() { json!({}) } else if done { json!({"summary": text}) } else { json!({"reason": text}) };
            let reveal = format!("{}:{id}", if done { "done" } else { "fail" });
            run(
                m,
                a,
                tp(if done { "/done" } else { "/fail" }),
                body,
                move |m, _| {
                    ui(|u| {
                        u.drafts.remove(&k);
                        u.reveal.remove(&reveal);
                    });
                    drop_field(m, &k);
                    if done { "Marked done".into() } else { "Marked failed".into() }
                },
                cx,
            );
        }
        "close-term" => run(m, a, tp("/close-terminal"), json!({"force": a.arg == "force"}), |m, _| sent(m), cx),
        "focus" => run(m, a, tp("/focus"), json!({}), |m, _| sent(m), cx),
        "term-focus" => run(m, a, format!("sessions/{id}/focus"), json!({}), |m, _| sent(m), cx),
        "resume" => run(m, a, tp("/resume"), json!({"mode": a.arg}), |m, _| sent(m), cx),
        "meta-del" => {
            let Some(task) = m.data.task.clone() else { return };
            let i: usize = a.arg.parse().unwrap_or(usize::MAX);
            let original = fmt::arr(&task, "meta").len();
            ui(|u| {
                let mut rows = view::meta_rows(&task, u);
                if i < rows.len() {
                    rows.remove(i);
                }
                u.meta_edit.insert(id.clone(), rows);
            });
            m.task_panel.fields.retain(|k, _| !k.starts_with(&format!("meta:{id}:")));
            if i >= original {
                ui(|u| u.notes.remove(&format!("meta:{id}")));
                cx.notify();
                return;
            }
            save_meta(m, &id, "Removed", cx);
        }
        "att-menu" => {
            ui(|u| u.att_menu = if u.att_menu.as_deref() == Some(id.as_str()) { None } else { Some(id.clone()) });
            cx.notify();
        }
        "att-menu-close" => {
            if let Some(a) = attachment(m, &id) {
                if let Some(h) = view::att_href(&a) {
                    open_target(&h, cx);
                }
            }
            ui(|u| u.att_menu = None);
            cx.notify();
        }
        "att-copy" => {
            ui(|u| u.att_menu = None);
            let url = attachment(m, &id).map(|a| s(&a, "url").to_string()).unwrap_or_default();
            cx.write_to_clipboard(ClipboardItem::new_string(url));
            m.toast("Copied", false, cx);
        }
        "att-edit" => {
            ui(|u| {
                u.att_menu = None;
                u.att_edit = Some(id.clone());
                clear_att_edit(u, &id);
            });
            m.task_panel.fields.retain(|k, _| !k.starts_with(&format!("attedit:{id}:")));
            cx.notify();
        }
        "att-ekind" => {
            ui(|u| u.drafts.insert(format!("attedit:{id}:kind"), a.arg.clone()));
            cx.notify();
        }
        "att-ecancel" => {
            ui(|u| {
                u.att_edit = None;
                clear_att_edit(u, &id);
            });
            cx.notify();
        }
        "att-esave" => {
            let k = format!("attedit:{id}");
            let body = ui(|u| {
                let mut b = serde_json::Map::new();
                for f in ["title", "url", "kind"] {
                    if let Some(v) = u.drafts.get(&format!("{k}:{f}")) {
                        b.insert(f.into(), json!(v));
                    }
                }
                Value::Object(b)
            });
            let id2 = id.clone();
            run(
                m,
                a,
                format!("attachments/{id}"),
                body,
                move |_, _| {
                    ui(|u| {
                        u.att_edit = None;
                        clear_att_edit(u, &id2);
                    });
                    "Saved".into()
                },
                cx,
            );
        }
        "att-remove" => {
            ui(|u| u.att_menu = None);
            run(m, a, format!("attachments/{id}/remove"), json!({}), |_, _| "Removed".into(), cx);
        }
        _ => {}
    }
}

fn clear_att_edit(u: &mut view::Ui, id: &str) {
    for f in ["kind", "title", "url"] {
        u.drafts.remove(&format!("attedit:{id}:{f}"));
    }
}

fn attachment(m: &MainWindow, id: &str) -> Option<Value> {
    let t = m.data.task.as_ref()?;
    fmt::arr(t, "attachments").iter().chain(fmt::arr(t, "goal_attachments")).find(|a| fmt::ref_of(&a["id"], "") == id || a["id"].to_string() == id).cloned()
}

/// Follow a plain link (`<a href>`).
fn go(m: &mut MainWindow, g: &Go, cx: &mut Context<MainWindow>) {
    match g {
        Go::Url(u) => {
            if !u.is_empty() {
                open_target(u, cx);
            }
            return;
        }
        _ => {
            TRAIL.with(|t| *t.borrow_mut() = (String::new(), Vec::new()));
        }
    }
    match g {
        Go::Goal(r) => {
            m.close_panel(cx);
            m.go(Page::Goal(r.clone()), cx);
        }
        Go::GoalBacklog(r) => {
            m.close_panel(cx);
            crate::ui::goal::open(m, &r, true, cx);
        }
        Go::Session(id) => {
            // `termHref`: the Sessions page with this terminal, and "Back to T12".
            let tref = match &m.panel {
                Some(Panel::Task { r, .. }) => r.clone(),
                _ => String::new(),
            };
            m.close_panel(cx);
            crate::ui::sessions::open_from_task(m, id, &tref, cx);
        }
        Go::Issue(r) => {
            m.go(Page::Board, cx);
            m.open_issue(r.clone(), cx);
        }
        Go::Url(_) => {}
    }
}

/// Opens a web link, or a local file path (`~` or `/`) with `open`.
fn open_target(url: &str, cx: &mut App) {
    let l = url.to_ascii_lowercase();
    if l.starts_with("http://") || l.starts_with("https://") {
        cx.open_url(url);
    } else {
        let path = taskboardd::util::expand_home(url);
        let _ = std::process::Command::new("/usr/bin/open").arg(path).spawn();
    }
}

/// `loadHandoff(r)`: fetch the handoff once when the Context tab needs it and the task has none.
fn load_handoff(m: &MainWindow, cx: &mut Context<MainWindow>) {
    let Some(Panel::Task { r, tab: TaskTab::Context }) = &m.panel else { return };
    let Some(task) = m.data.task.as_ref() else { return };
    let r = r.clone();
    if ui(|u| view::handoff_text(task, u).is_some()) || LOADING.with(|l| !l.borrow_mut().insert(r.clone())) {
        return;
    }
    let backend = m.backend.clone();
    let path = format!("tasks/{r}/handoff");
    cx.spawn(async move |this, cx| {
        let got = cx.background_executor().spawn(async move { backend.get(&path, &[]) }).await;
        let text = match got {
            Ok(h) => h.get("text").and_then(Value::as_str).or(h.get("handoff").and_then(Value::as_str)).unwrap_or("").to_string(),
            Err(e) => format!("Couldn’t load the handoff: {}", e.message),
        };
        let _ = this.update(cx, |_, cx| {
            LOADING.with(|l| l.borrow_mut().remove(&r));
            ui(|u| u.handoffs.insert(r, text));
            cx.notify();
        });
    })
    .detach();
}

// ------------------------------------------------------------------ fields

/// Create the text fields the tree draws (once), keep unfocused ones in step with their value,
/// and route typing into the drafts / the Details edit like the web's `input` listener.
fn sync_fields(m: &mut MainWindow, n: &Node, window: &mut Window, cx: &mut Context<MainWindow>) {
    match n {
        Node::Field { key, value, placeholder, multi, .. } => {
            if let Some(i) = m.task_panel.fields.get(key) {
                if !i.focus.is_focused(window) && i.text(cx) != *value {
                    i.set_text(value, cx);
                }
                return;
            }
            let input = kit::Input::with_text(cx, placeholder.to_string(), *multi, value);
            let k = key.clone();
            let sub = cx.subscribe(&input.field, move |m, field, _: &crate::ui::text_input::FieldChanged, cx| {
                let text = field.read(cx).text().to_string();
                on_input(m, &k, text);
            });
            m.task_panel.subs.push(sub);
            if key.starts_with("meta:") {
                // `change` on leaving the field saves the Details (`saveMeta`).
                let k = key.clone();
                let sub = cx.on_blur(&input.focus, window, move |m, _, cx| {
                    if let Some(r) = k.split(':').nth(1) {
                        let r = r.to_string();
                        if ui(|u| u.meta_edit.contains_key(&r)) {
                            save_meta(m, &r, "Saved", cx);
                        }
                    }
                });
                m.task_panel.subs.push(sub);
            }
            m.task_panel.fields.insert(key.clone(), input);
        }
        Node::El { .. } => {
            for k in view::shown(n) {
                sync_fields(m, k, window, cx);
            }
        }
        _ => {}
    }
}

/// The web's `input` listener: `data-k` → drafts; `data-meta="r:i:j"` → the Details edit.
fn on_input(m: &mut MainWindow, key: &str, text: String) {
    if let Some(rest) = key.strip_prefix("meta:") {
        let mut it = rest.split(':');
        let (Some(r), Some(i), Some(j)) = (it.next(), it.next().and_then(|x| x.parse::<usize>().ok()), it.next()) else { return };
        let Some(task) = m.data.task.clone() else { return };
        ui(|u| {
            if !u.meta_edit.contains_key(r) {
                let rows = view::meta_rows(&task, u);
                u.meta_edit.insert(r.to_string(), rows);
            }
            if let Some(row) = u.meta_edit.get_mut(r).and_then(|rows| rows.get_mut(i)) {
                if j == "0" {
                    row.0 = text;
                } else {
                    row.1 = text;
                }
            }
        });
    } else {
        ui(|u| u.drafts.insert(key.to_string(), text));
    }
}

// ------------------------------------------------------------------ render

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let Some(Panel::Task { r, tab }) = m.panel.clone() else {
        return div().into_any_element();
    };
    if m.task_panel.r != r {
        m.task_panel = State { r: r.clone(), ..Default::default() };
        // Dev: `TASKBOARD_TAB=context|log` opens the panel on that tab (screenshots).
        if let (Ok(want), Some(Panel::Task { tab: cur, .. })) = (std::env::var("TASKBOARD_TAB"), m.panel.as_mut()) {
            if *cur == TaskTab::Overview {
                *cur = match want.as_str() {
                    "context" => TaskTab::Context,
                    "log" => TaskTab::Log,
                    _ => TaskTab::Overview,
                };
            }
        }
    }
    let _ = tab;
    load_handoff(m, cx);
    let Some(tree) = build(m) else { return div().into_any_element() };
    sync_fields(m, &tree, window, cx);
    m.task_panel.focused = m.task_panel.fields.values().find(|i| i.focus.is_focused(window)).map(|i| i.focus.clone());
    if let Some(k) = m.task_panel.focus_next.take() {
        if let Some(i) = m.task_panel.fields.get(&k) {
            window.focus(&i.focus, cx);
        }
    }
    let mut d = Draw { t: &t, window, path: Vec::new() };
    let body = d.panel(m, &tree, cx);
    deferred(
        kit::scrim(&t, "task-scrim")
            .on_mouse_down(MouseButton::Left, cx.listener(|m, _, _, cx| act(m, &Act::new("close-panel", "", "", ""), cx)))
            .child(
                div()
                    .id("task-drawer")
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(px(WIDTH))
                    .max_w(relative(0.94))
                    .flex()
                    .flex_col()
                    .bg(t.card)
                    .border_l_1()
                    .border_color(t.border)
                    .shadow_lg()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(body),
            ),
    )
    .with_priority(1)
    .into_any_element()
}

struct Draw<'a> {
    t: &'a Theme,
    window: &'a mut Window,
    path: Vec<usize>,
}

impl Draw<'_> {
    fn id(&self, tag: &str) -> SharedString {
        let p: Vec<String> = self.path.iter().map(|x| x.to_string()).collect();
        SharedString::from(format!("tp-{tag}-{}", p.join("-")))
    }

    /// The whole drawer: the head (top row, title, subline, alert, tabs) fixed, the body scrolling.
    fn panel(&mut self, m: &MainWindow, tree: &Node, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let Node::El { kids, .. } = tree else { return div().into_any_element() };
        let mut head = div().flex().flex_col().flex_none().gap(px(10.)).px(px(22.)).pt(px(18.)).pb(px(14.)).border_b_1().border_color(t.divider);
        let mut body = None;
        for (i, k) in kids.iter().enumerate() {
            self.path.push(i);
            if matches!(k, Node::El { k: K::Body, .. }) {
                body = Some(self.node(m, k, None, cx));
            } else {
                head = head.child(self.node(m, k, None, cx));
            }
            self.path.pop();
        }
        let r = m.task_panel.r.clone();
        let scroll = div()
            .id(SharedString::from(format!("task-body-{r}")))
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .child(div().flex().flex_col().px(px(22.)).py(px(18.)).children(body));
        div().flex().flex_col().size_full().child(head).child(scroll).into_any_element()
    }

    fn kids(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> Vec<AnyElement> {
        let Node::El { kids, .. } = n else { return Vec::new() };
        let mut out = Vec::new();
        for (i, k) in kids.iter().enumerate() {
            self.path.push(i);
            out.push(self.node(m, k, boxed, cx));
            self.path.pop();
        }
        out
    }

    // Every element kind is built in its own small function: the tree is drawn recursively, and
    // one big function would put every kind's locals on each level's stack frame.
    #[inline(never)]
    fn node(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        match n {
            Node::Text { s, st, tip } => self.leaf_text(s, *st, tip, boxed),
            Node::Pill { s, tone, tip } => self.leaf_pill(s, *tone, tip),
            Node::Btn { label, act: a, look, disabled, tip } => self.leaf_btn(label, a, *look, *disabled, tip, cx),
            Node::Link { s, go: g, tip, look } => self.leaf_link(s, g, tip, *look, cx),
            Node::Field { key, multi, mono, .. } => self.leaf_field(m, key, *multi, *mono, boxed, cx),
            Node::Note { s, err } => self.leaf_note(s, *err),
            Node::Dot(d) => self.leaf_dot(*d),
            Node::El { k, tip, .. } => {
                let el = self.container(m, n, *k, boxed, cx);
                match tip {
                    Some(tip) => div().id(self.id("tipbox")).child(el).tooltip(kit::tip(tip.clone())).into_any_element(),
                    None => el,
                }
            }
        }
    }

    #[inline(never)]
    fn leaf_text(&self, s: &str, st: St, tip: &Option<String>, boxed: Option<BoxTone>) -> AnyElement {
        let el = self.text(s, st, boxed);
        match tip {
            Some(tip) => div().id(self.id("tx")).child(el).tooltip(kit::tip(tip.clone())).into_any_element(),
            None => el.into_any_element(),
        }
    }

    #[inline(never)]
    fn leaf_pill(&self, s: &str, tone: Tone, tip: &Option<String>) -> AnyElement {
        let p = pill(self.t, s, tone);
        match tip {
            Some(tip) => div().id(self.id("pill")).child(p).tooltip(kit::tip(tip.clone())).into_any_element(),
            None => p.into_any_element(),
        }
    }

    #[inline(never)]
    fn leaf_btn(&self, label: &str, a: &Act, look: Look, disabled: bool, tip: &Option<String>, cx: &mut Context<MainWindow>) -> AnyElement {
        let b = button(self.t, self.id("btn"), label, look);
        let b = if disabled {
            kit::disabled(b)
        } else {
            let a = a.clone();
            b.on_click(cx.listener(move |m, _, _, cx| act(m, &a, cx)))
        };
        match tip {
            Some(tip) => b.tooltip(kit::tip(tip.clone())).into_any_element(),
            None => b.into_any_element(),
        }
    }

    #[inline(never)]
    fn leaf_link(&self, s: &str, g: &Go, tip: &Option<String>, look: LinkLook, cx: &mut Context<MainWindow>) -> AnyElement {
        let g = g.clone();
        let l = link(self.t, self.id("link"), s, look).on_click(cx.listener(move |m, _, _, cx| go(m, &g, cx)));
        match tip {
            Some(tip) => l.tooltip(kit::tip(tip.clone())).into_any_element(),
            None => l.into_any_element(),
        }
    }

    #[inline(never)]
    fn leaf_field(&mut self, m: &MainWindow, key: &str, multi: bool, mono: bool, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let Some(input) = m.task_panel.fields.get(key) else { return div().into_any_element() };
        let k = key.to_string();
        input
            .render(t, self.id("field"), self.window)
            .when(multi, |d| d.min_h(px(58.)).items_start())
            .when(mono, |d| d.font_family(t.mono_font.clone()))
            .when(boxed == Some(BoxTone::Ask), |d| d.border_color(t.warn_line))
            .on_key_down(cx.listener(move |m, ev: &KeyDownEvent, _, cx| on_key(m, &k, ev, cx)))
            .into_any_element()
    }

    #[inline(never)]
    fn leaf_note(&self, s: &str, err: bool) -> AnyElement {
        let t = self.t;
        div().text_size(px(12.5)).text_color(if err { t.down } else { t.up_fg }).child(s.to_string()).into_any_element()
    }

    #[inline(never)]
    fn leaf_dot(&self, d: Dot) -> AnyElement {
        match dot_color(self.t, d) {
            Some(c) => kit::dot(c, 8.).into_any_element(),
            None => div().flex_none().size(px(8.)).rounded_full().border_1().border_color(self.t.border_2).into_any_element(),
        }
    }

    #[inline(never)]
    fn text(&self, s: &str, st: St, boxed: Option<BoxTone>) -> Div {
        let t = self.t;
        let d = div().child(s.to_string());
        match st {
            St::Plain => d.text_size(px(13.5)),
            St::Title => d.text_size(px(19.)).font_weight(FontWeight::BOLD).line_height(px(25.)),
            St::Sub | St::Small | St::Help => d.text_size(px(12.5)).text_color(t.muted),
            St::Muted => d.text_size(px(13.)).text_color(t.muted),
            St::WarnHelp => d.text_size(px(12.5)).text_color(t.warn_fg),
            St::BoxLabel => d.text_size(px(12.5)).font_weight(FontWeight::BOLD).text_color(match boxed {
                Some(BoxTone::Ask) => t.warn_fg,
                Some(BoxTone::Lost | BoxTone::Failed) => t.down,
                Some(BoxTone::Done) => t.up_fg,
                _ => t.accent_fg,
            }),
            St::H3 => d.text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(if boxed == Some(BoxTone::Info) { t.accent_fg } else { t.muted }).whitespace_nowrap(),
            St::Strong => d.text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_2),
            St::Mono => d.font_family(t.mono_font.clone()).text_size(px(12.5)),
            St::Empty => d.py(px(20.)).flex().justify_center().text_size(px(13.)).text_color(t.muted),
            St::Time => d.flex_none().w(px(66.)).text_size(px(12.)).text_color(t.faint),
            St::Kind => d.text_size(px(12.5)).text_color(t.muted),
            St::Handoff => d.p(px(10.)).rounded(px(8.)).bg(t.card).border_1().border_color(t.border).font_family(t.mono_font.clone()).text_size(px(12.)).text_color(t.text_2),
            St::Code => d.px(px(4.)).rounded(px(4.)).bg(t.panel_2).font_family(t.mono_font.clone()).text_size(px(12.5)),
            St::Pre => d.p(px(10.)).rounded(px(8.)).bg(t.panel_2).font_family(t.mono_font.clone()).text_size(px(12.)),
            St::StepName => d.text_size(px(12.)).font_weight(FontWeight::SEMIBOLD),
            St::StepSub => d.text_size(px(11.5)).text_color(t.muted),
            St::LrowKey => d.flex_none().w(px(104.)).pt(px(1.)).text_size(px(12.5)).text_color(t.muted),
            St::Count => d.px(px(6.)).rounded_full().bg(t.seg).text_size(px(11.5)).text_color(t.muted),
            St::Jkey => d.font_family(t.mono_font.clone()).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.accent_fg),
            St::Glyph(dot) => d.flex_none().w(px(14.)).text_size(px(13.5)).text_color(dot_color(t, dot).unwrap_or(t.muted)),
        }
    }

    #[inline(never)]
    fn container(&mut self, m: &MainWindow, n: &Node, k: K, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        match k {
            K::Fold(f, open) => self.c_fold(m, n, f, open, boxed, cx),
            K::Box(tone) => self.c_box(m, n, tone, cx),
            K::Top => self.c_top(m, n, boxed, cx),
            K::Lrow => self.c_lrow(m, n, boxed, cx),
            K::TlItem => self.c_tl_item(m, n, boxed, cx),
            K::Steps => self.c_steps(m, n, boxed, cx),
            K::Step(st) => self.c_step(m, n, st, boxed, cx),
            K::Att => self.c_att(m, n, boxed, cx),
            K::Kv => self.c_kv(m, n, boxed, cx),
            K::Files => self.c_files(m, n, boxed, cx),
            K::LogItem => self.c_log_item(m, n, boxed, cx),
            K::MdLi => self.c_md_li(m, n, boxed, cx),
            _ => self.c_simple(m, n, k, boxed, cx),
        }
    }

    /// Plain flex boxes.
    #[inline(never)]
    fn c_simple(&mut self, m: &MainWindow, n: &Node, k: K, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let kids = self.kids(m, n, boxed, cx);
        let d = match k {
            K::Sub => div().flex().flex_wrap().items_center().gap(px(8.)),
            K::Tabs | K::Chips | K::Seg => div().self_start().flex().flex_wrap().gap(px(2.)).p(px(2.)).rounded(px(8.)).bg(t.seg),
            K::Body => div().flex().flex_col().gap(px(16.)),
            K::Row | K::Line | K::PrHead => div().flex().flex_wrap().items_center().gap(px(8.)),
            K::TlName => div().flex().flex_wrap().items_center().gap(px(6.)),
            K::TlSub | K::AttMeta => div().flex().flex_wrap().items_center().gap(px(4.)),
            K::Stack | K::Tl | K::Md => div().flex().flex_col().gap(px(8.)).min_w_0(),
            K::Atts | K::List | K::MdUl => div().flex().flex_col().gap(px(4.)).min_w_0(),
            K::Col => div().flex().flex_col().gap(px(12.)).min_w_0(),
            K::Lsec => div().flex().flex_col().gap(px(6.)).py(px(10.)).border_t_1().border_color(t.divider),
            K::PrBar { warn } => {
                let (fg, bg) = if warn { (t.warn_fg, t.warn_soft) } else { (t.accent_fg, t.accent_soft) };
                div().flex().items_center().gap(px(8.)).px(px(10.)).py(px(6.)).rounded(px(8.)).bg(bg).text_color(fg).text_size(px(12.5))
            }
            K::AttMenu => kit::menu_box(t, 200.),
            K::AttEdit => div().flex().flex_col().gap(px(8.)).p(px(8.)).rounded(px(8.)).border_1().border_color(t.border),
            K::KvRow | K::Li => div().flex().items_center().gap(px(8.)),
            K::MetaRow => div().flex().items_center().gap(px(6.)),
            // Linked, Log, MdP
            _ => div().flex().flex_col().min_w_0(),
        };
        d.children(kids).into_any_element()
    }

    #[inline(never)]
    fn c_fold(&mut self, m: &MainWindow, n: &Node, f: Fold, open: bool, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let Node::El { kids, .. } = n else { return div().into_any_element() };
        self.path.push(0);
        let summary = kids.first().map(|s| self.node(m, s, boxed, cx));
        self.path.pop();
        let hover = t.text;
        let head = div()
            .id(self.id("fold"))
            .flex()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .hover(move |d| d.text_color(hover))
            .child(div().w(px(12.)).text_size(px(12.)).text_color(t.faint).child(if open { "▾" } else { "▸" }))
            .children(summary)
            .on_click(cx.listener(move |_, _, _, cx| {
                crate::prefs::set(f.key(), json!(if open { "closed" } else { "open" }));
                cx.notify();
            }));
        let mut col = div().flex().flex_col().gap(px(8.)).child(head);
        if open {
            for (i, kid) in kids.iter().enumerate().skip(1) {
                self.path.push(i);
                let e = self.node(m, kid, boxed, cx);
                self.path.pop();
                col = col.child(div().pl(px(18.)).child(e));
            }
        }
        col.into_any_element()
    }

    #[inline(never)]
    fn c_box(&mut self, m: &MainWindow, n: &Node, tone: BoxTone, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let (bg, line) = match tone {
            BoxTone::Ask => (t.warn_soft, t.warn_line),
            BoxTone::Lost | BoxTone::Failed => (t.down_soft, t.down_line),
            BoxTone::Done => (t.up_soft, t.up_soft),
            BoxTone::Info => (t.accent_soft, t.accent_line),
        };
        let kids = self.kids(m, n, Some(tone), cx);
        div().flex().flex_col().gap(px(10.)).p(px(14.)).rounded(px(10.)).bg(bg).border_1().border_color(line).children(kids).into_any_element()
    }

    #[inline(never)]
    fn c_top(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let kids = self.kids(m, n, boxed, cx);
        let n_kids = kids.len();
        let mut row = div().flex().flex_none().items_center().gap(px(6.));
        for (i, e) in kids.into_iter().enumerate() {
            // The close button sits at the far end.
            if i + 1 == n_kids {
                row = row.child(div().flex_1());
            }
            row = row.child(e);
        }
        row.into_any_element()
    }

    #[inline(never)]
    fn c_lrow(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let mut kids = self.kids(m, n, boxed, cx).into_iter();
        let key = kids.next();
        div()
            .flex()
            .gap(px(12.))
            .py(px(10.))
            .border_t_1()
            .border_color(t.divider)
            .children(key)
            .child(div().flex().flex_col().gap(px(4.)).flex_1().min_w_0().text_size(px(13.5)).children(kids))
            .into_any_element()
    }

    #[inline(never)]
    fn c_tl_item(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let mut kids = self.kids(m, n, boxed, cx).into_iter();
        let (dot, name, sub, at) = (kids.next(), kids.next(), kids.next(), kids.next());
        div()
            .flex()
            .items_start()
            .gap(px(8.))
            .child(div().pt(px(6.)).children(dot))
            .child(div().flex().flex_col().gap(px(3.)).flex_1().min_w_0().children(name).children(sub))
            .children(at.map(|a| div().text_color(t.faint).child(a)))
            .into_any_element()
    }

    #[inline(never)]
    fn c_steps(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let kids = self.kids(m, n, boxed, cx);
        let mut row = div().flex().items_start().pt(px(4.));
        for (i, e) in kids.into_iter().enumerate() {
            if i > 0 {
                row = row.child(div().flex_1().h(px(2.)).mt(px(9.)).bg(t.border));
            }
            row = row.child(e);
        }
        row.into_any_element()
    }

    #[inline(never)]
    fn c_step(&mut self, m: &MainWindow, n: &Node, st: StepSt, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let (fg, bg, glyph) = match st {
            StepSt::Done => (t.on_accent, t.up, "✓"),
            StepSt::Fail => (t.on_accent, t.down, "✕"),
            StepSt::Ask => (t.on_accent, t.warn, "!"),
            StepSt::Wait => (t.muted, t.seg, "◷"),
            StepSt::Now => (t.on_accent, t.accent, "•"),
            StepSt::Todo => (t.faint, t.seg, ""),
        };
        let kids = self.kids(m, n, boxed, cx);
        div()
            .flex()
            .flex_col()
            .items_center()
            .gap(px(3.))
            .min_w(px(78.))
            .child(div().size(px(20.)).rounded_full().bg(bg).text_color(fg).flex().items_center().justify_center().text_size(px(11.)).font_weight(FontWeight::BOLD).child(glyph))
            .children(kids)
            .into_any_element()
    }

    #[inline(never)]
    fn c_att(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let mut it = self.kids(m, n, boxed, cx).into_iter();
        let (name, meta, more, menu) = (it.next(), it.next(), it.next(), it.next());
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .px(px(8.))
            .py(px(6.))
            .rounded(px(8.))
            .border_1()
            .border_color(t.border)
            .child(div().flex().items_center().gap(px(8.)).child(div().flex().flex_col().flex_1().min_w_0().children(name).children(meta)).children(more))
            .children(menu)
            .into_any_element()
    }

    #[inline(never)]
    fn c_kv(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let kids = self.kids(m, n, boxed, cx);
        let mut col = div().flex().flex_col().rounded(px(10.)).border_1().border_color(t.border).overflow_hidden();
        for (i, e) in kids.into_iter().enumerate() {
            col = col.child(div().px(px(12.)).py(px(8.)).when(i > 0, |d| d.border_t_1().border_color(t.divider)).child(e));
        }
        col.into_any_element()
    }

    #[inline(never)]
    fn c_files(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let kids = self.kids(m, n, boxed, cx);
        div().flex().flex_wrap().gap(px(6.)).children(kids.into_iter().map(|e| div().px(px(7.)).rounded(px(6.)).border_1().border_color(t.border).child(e))).into_any_element()
    }

    #[inline(never)]
    fn c_log_item(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let mut it = self.kids(m, n, boxed, cx).into_iter();
        let (time, dot, who, kind, text) = (it.next(), it.next(), it.next(), it.next(), it.next());
        div()
            .flex()
            .gap(px(10.))
            .py(px(8.))
            .border_t_1()
            .border_color(t.divider)
            .children(time)
            .child(div().pt(px(5.)).children(dot))
            .child(div().flex().flex_col().flex_1().min_w_0().gap(px(2.)).child(div().flex().items_center().gap(px(8.)).children(who).children(kind)).children(text))
            .into_any_element()
    }

    #[inline(never)]
    fn c_md_li(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let kids = self.kids(m, n, boxed, cx);
        div().flex().gap(px(6.)).child(div().text_color(t.muted).child("•")).child(div().flex().flex_wrap().items_center().gap(px(4.)).children(kids)).into_any_element()
    }
}

fn on_key(m: &mut MainWindow, key: &str, ev: &KeyDownEvent, cx: &mut Context<MainWindow>) {
    let ks = &ev.keystroke;
    // ⌘↩ in an answer sends it (the web's keydown handler); ↩ in a Details field commits it.
    if ks.key == "enter" && (ks.modifiers.platform || ks.modifiers.control) {
        if let Some(r) = key.strip_prefix("answer:") {
            let a = build(m).and_then(|tree| view::find_act(&tree, "answer", Some("")).cloned()).unwrap_or_else(|| Act::new("answer", "", r, format!("ask:{r}")));
            act(m, &a, cx);
            cx.stop_propagation();
        }
    } else if ks.key == "enter" && key.starts_with("meta:") {
        if let Some(r) = key.split(':').nth(1).map(str::to_string) {
            if ui(|u| u.meta_edit.contains_key(&r)) {
                save_meta(m, &r, "Saved", cx);
            }
        }
        cx.stop_propagation();
    }
}

fn dot_color(t: &Theme, d: Dot) -> Option<Hsla> {
    Some(match d {
        Dot::Idle | Dot::Faint => t.faint,
        Dot::Working | Dot::Accent => t.accent,
        Dot::Needs | Dot::Warn => t.warn,
        Dot::Gone => t.border_2,
        Dot::AccentFg => t.accent_fg,
        Dot::Up => t.up,
        Dot::Goal => t.goal,
        Dot::None => return None,
    })
}

fn pill(t: &Theme, s: &str, tone: Tone) -> Div {
    let (fg, bg, line) = match tone {
        Tone::Queued | Tone::Neutral => (t.text_2, t.col, None),
        Tone::Review => (t.warn_fg, t.col, Some(t.warn)),
        Tone::Needs | Tone::Blocked | Tone::High => (t.warn_fg, t.warn_soft, None),
        Tone::Done => (t.up_fg, t.up_soft, None),
        Tone::Failed => (t.down, t.down_soft, None),
        Tone::Planned => (t.muted, t.card, Some(t.border)),
        Tone::Working | Tone::Jira => (t.accent_fg, t.accent_soft, None),
        Tone::Idle | Tone::Ref | Tone::Repo => (t.text_2, t.panel_2, None),
        Tone::Gone => (t.muted, t.panel_2, None),
        Tone::Closed => (t.faint, t.card, Some(t.border)),
    };
    let p = kit::pill(fg, bg, s.to_string()).when_some(line, |d, c| d.border_1().border_color(c));
    if matches!(tone, Tone::Ref | Tone::Repo) { p.font_family(t.mono_font.clone()).font_weight(FontWeight::MEDIUM) } else { p }
}

/// A button without a hover style yet (the kit's buttons already carry one; GPUI allows one).
fn base(t: &Theme, id: SharedString, label: String) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(30.))
        .px(px(12.))
        .rounded(px(8.))
        .border_1()
        .border_color(t.border_2)
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .whitespace_nowrap()
        .cursor_pointer()
        .child(label)
}

/// `.btn.soft`: accent-tinted.
fn soft(t: &Theme, id: SharedString, label: String) -> Stateful<Div> {
    let h = t.accent_line;
    base(t, id, label).bg(t.accent_soft).border_color(t.accent_line).text_color(t.accent_fg).hover(move |d| d.bg(h))
}

fn link(t: &Theme, id: SharedString, s: &str, look: LinkLook) -> Stateful<Div> {
    if look == LinkLook::Term {
        let h = t.accent_fg;
        return div().id(id).flex_none().cursor_pointer().text_color(t.text).font_weight(FontWeight::MEDIUM).hover(move |d| d.text_color(h).underline()).child(s.to_string());
    }
    let l = kit::link(t, id, s.to_string());
    match look {
        LinkLook::Plain => l,
        LinkLook::Small => l.text_size(px(12.5)),
        LinkLook::Term => l,
        LinkLook::GoalName => l.text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.goal),
        LinkLook::Jkey => l.font_family(t.mono_font.clone()).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.accent_fg),
        LinkLook::AttName => l.font_weight(FontWeight::SEMIBOLD).text_color(t.accent_fg).truncate(),
        LinkLook::SrcGoal => l.px(px(5.)).rounded(px(5.)).font_family(t.mono_font.clone()).text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(t.goal).bg(t.goal_soft),
    }
}

fn button(t: &Theme, id: SharedString, label: &str, look: Look) -> Stateful<Div> {
    let label = label.to_string();
    match look {
        Look::Plain => kit::btn(t, id, label),
        Look::Primary => kit::btn_primary(t, id, label),
        Look::PrimaryWide => kit::btn_primary(t, id, label).flex_1().h(px(36.)),
        Look::SoftWide => soft(t, id, label).flex_1().h(px(36.)),
        Look::Soft => soft(t, id, label),
        Look::SoftSmall => soft(t, id, label).h(px(24.)).px(px(8.)).rounded(px(6.)).text_size(px(12.)),
        Look::Small => kit::btn_small(t, id, label),
        Look::Danger => kit::btn_danger(t, id, label),
        Look::DangerSmall => kit::btn_danger(t, id, label).h(px(24.)).px(px(8.)).rounded(px(6.)).text_size(px(12.)),
        Look::Ghost => {
            let h = t.panel_2;
            base(t, id, label).h(px(24.)).px(px(8.)).rounded(px(6.)).text_size(px(12.)).border_color(transparent_black()).bg(transparent_black()).text_color(t.text_2).hover(move |d| d.bg(h))
        }
        Look::Link => kit::link(t, id, label).text_size(px(12.5)),
        Look::Ref => kit::link(t, id, label).font_family(t.mono_font.clone()).font_weight(FontWeight::SEMIBOLD),
        Look::Src => div().id(id).px(px(5.)).rounded(px(5.)).cursor_pointer().font_family(t.mono_font.clone()).text_size(px(11.5)).font_weight(FontWeight::SEMIBOLD).text_color(t.accent_fg).bg(t.accent_soft).child(label),
        Look::Menu => kit::menu_item(t, id, label, false),
        Look::Tab { on } => {
            let hover = t.text;
            div()
                .id(id)
                .flex()
                .items_center()
                .h(px(24.))
                .px(px(10.))
                .rounded(px(6.))
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .whitespace_nowrap()
                .text_color(if on { t.text } else { t.muted })
                .when(on, |d| d.bg(t.card).shadow_sm())
                .hover(move |s| s.text_color(hover))
                .child(label)
        }
        Look::Icon(icon) => {
            let glyph = match icon {
                Icon::Close | Icon::Remove => "✕",
                Icon::Back => "‹",
                Icon::External => "↗",
                Icon::More => "⋯",
            };
            let h = t.panel_2;
            let size = if matches!(icon, Icon::External | Icon::More) { 20. } else { 28. };
            div()
                .id(id)
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(px(size))
                .rounded(px(6.))
                .cursor_pointer()
                .text_color(t.muted)
                .text_size(px(if size < 24. { 12. } else { 15. }))
                .hover(move |d| d.bg(h))
                .child(glyph)
        }
    }
}

#[cfg(test)]
mod tests;
