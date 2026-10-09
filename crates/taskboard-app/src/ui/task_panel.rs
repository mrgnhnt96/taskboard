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
use crate::theme::{Theme, ThemeMode};
use crate::ui::kit;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
pub use view::{Act, Go, Node};
use view::{BoxTone, Dot, Fold, Icon, K, LinkLook, Look, St, StepSt, Tone};

/// `.panel { width: min(480px, 100vw) }`.
pub const WIDTH: f32 = 480.;

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
    [Fold::Linked, Fold::Summary, Fold::What, Fold::Terms, Fold::Findings]
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
        "pr-reviewed" => run(m, a, tp("/pr/reviewed"), json!({}), |_, _| "Marked reviewed".into(), cx),
        "detach" => run(m, a, tp("/detach"), json!({}), |_, _| "Detached. It’s back in the queue.".into(), cx),
        "close-term" => run(m, a, tp("/close-terminal"), json!({"force": a.arg == "force"}), |m, _| sent(m), cx),
        "focus" => run(m, a, tp("/focus"), json!({}), |m, _| sent(m), cx),
        "step" => run(m, a, tp("/step"), json!({"skip": a.arg == "skip"}), |m, _| sent(m), cx),
        "open-url" => open_target(&a.arg, cx),
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
    let mut d = Draw { t: &t, window, path: Vec::new(), within: Vec::new() };
    let body = d.panel(m, &tree, cx);
    // `.panel`: a 480px sheet on the right with a soft shadow; the board stays undimmed behind it
    // (a click outside still closes it).
    let shadow = if t.mode == ThemeMode::Dark { hsla(0., 0., 0., 0.45) } else { rgba(0x10182819).into() };
    deferred(
        kit::scrim(&t, "task-scrim")
            .bg(transparent_black())
            .on_mouse_down(MouseButton::Left, cx.listener(|m, _, _, cx| act(m, &Act::new("close-panel", "", "", ""), cx)))
            .child(
                div()
                    .id("task-drawer")
                    .absolute()
                    .top_0()
                    .right_0()
                    .bottom_0()
                    .w(px(WIDTH))
                    .max_w(relative(1.))
                    .flex()
                    .flex_col()
                    .bg(t.card)
                    .border_l_1()
                    .border_color(t.border)
                    .shadow(vec![BoxShadow { color: shadow, offset: point(px(-12.), px(0.)), blur_radius: px(32.), spread_radius: px(0.), inset: false }])
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .child(body),
            ),
    )
    .with_priority(1)
    .into_any_element()
}

/// `--accent-tint` (not in the shared theme).
fn accent_tint(t: &Theme) -> Hsla {
    if t.mode == ThemeMode::Dark { rgb(0x172036).into() } else { rgb(0xf5f8ff).into() }
}

/// `--warn-input`: the answer box's border.
fn warn_input(t: &Theme) -> Hsla {
    if t.mode == ThemeMode::Dark { rgb(0x6b3a1c).into() } else { rgb(0xe0c3b2).into() }
}

struct Draw<'a> {
    t: &'a Theme,
    window: &'a mut Window,
    path: Vec<usize>,
    /// The containers the node being drawn sits in (innermost last): some leaves look different
    /// inside some containers, as the web's descendant selectors made them.
    within: Vec<K>,
}

impl Draw<'_> {
    fn id(&self, tag: &str) -> SharedString {
        let p: Vec<String> = self.path.iter().map(|x| x.to_string()).collect();
        SharedString::from(format!("tp-{tag}-{}", p.join("-")))
    }

    fn parent(&self) -> Option<K> {
        self.within.last().copied()
    }

    /// The whole sheet scrolls as one (`.panel { overflow-y: auto; gap: 18px; padding: 24px 24px 32px }`),
    /// the title and its subline stacked 4px apart.
    fn panel(&mut self, m: &MainWindow, tree: &Node, cx: &mut Context<MainWindow>) -> AnyElement {
        let Node::El { kids, .. } = tree else { return div().into_any_element() };
        let mut col = div().flex().flex_col().gap(px(18.)).px(px(24.)).pt(px(24.)).pb(px(32.)).line_height(relative(1.5));
        let mut title: Option<Div> = None;
        self.within.push(K::Col);
        for (i, k) in kids.iter().enumerate() {
            self.path.push(i);
            let e = self.node(m, k, None, cx);
            self.path.pop();
            match k {
                Node::Text { st: St::Title, .. } => title = Some(div().flex().flex_col().gap(px(4.)).min_w_0().child(e)),
                Node::El { k: K::Sub, .. } if title.is_some() => {
                    if let Some(x) = title.take() {
                        col = col.child(x.child(e));
                    }
                }
                _ => {
                    if let Some(x) = title.take() {
                        col = col.child(x);
                    }
                    col = col.child(e);
                }
            }
        }
        self.within.pop();
        if let Some(x) = title.take() {
            col = col.child(x);
        }
        let r = m.task_panel.r.clone();
        div().id(SharedString::from(format!("task-body-{r}"))).size_full().overflow_y_scroll().text_size(px(14.)).child(col).into_any_element()
    }

    fn kids(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> Vec<AnyElement> {
        let Node::El { kids, k, .. } = n else { return Vec::new() };
        self.within.push(*k);
        let mut out = Vec::new();
        for (i, kid) in kids.iter().enumerate() {
            self.path.push(i);
            out.push(self.node(m, kid, boxed, cx));
            self.path.pop();
        }
        self.within.pop();
        out
    }

    /// Draw one kid of `n` (index `i`) as if inside `n`.
    fn kid(&mut self, m: &MainWindow, n: &Node, i: usize, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
        let Node::El { kids, k, .. } = n else { return None };
        let kid = kids.get(i)?;
        self.within.push(*k);
        self.path.push(i);
        let e = self.node(m, kid, boxed, cx);
        self.path.pop();
        self.within.pop();
        Some(e)
    }

    // Every element kind is built in its own small function: the tree is drawn recursively, and
    // one big function would put every kind's locals on each level's stack frame.
    #[inline(never)]
    fn node(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        match n {
            Node::Text { s, st, tip } => self.leaf_text(s, *st, tip, boxed),
            Node::Pill { s, tone, tip } => self.leaf_pill(s, *tone, tip),
            Node::Btn { label, act: a, look, disabled, tip } => self.leaf_btn(label, a, *look, *disabled, tip, boxed, cx),
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
        // `.pill.sm` for the repo, a Jira status and a terminal's state.
        let small = matches!(tone, Tone::Repo | Tone::Jira) || self.parent() == Some(K::TlSub);
        let p = pill(self.t, s, tone, small);
        match tip {
            Some(tip) => div().id(self.id("pill")).flex_none().child(p).tooltip(kit::tip(tip.clone())).into_any_element(),
            None => p.into_any_element(),
        }
    }

    #[inline(never)]
    #[allow(clippy::too_many_arguments)]
    fn leaf_btn(&self, label: &str, a: &Act, look: Look, disabled: bool, tip: &Option<String>, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let chip = self.parent() == Some(K::Chips);
        let tab = self.parent() == Some(K::Tabs);
        let b = match look {
            Look::Tab { on } if tab => tab_btn(self.t, self.id("btn"), label, on),
            Look::Tab { on } if chip => fchip(self.t, self.id("btn"), label, on),
            // `.box .btn { padding: 0 12px }`
            _ if boxed.is_some() && !matches!(look, Look::Icon(_) | Look::Menu | Look::Src | Look::Ref) => button(self.t, self.id("btn"), label, look).px(px(12.)),
            _ => button(self.t, self.id("btn"), label, look),
        };
        let b = if disabled {
            kit::disabled(b)
        } else {
            let a = a.clone();
            b.on_click(cx.listener(move |m, _, _, cx| act(m, &a, cx)))
        };
        let b = if look == Look::Icon(Icon::More) { b.opacity(0.).group_hover("tp-att", |s| s.opacity(1.)) } else { b };
        match tip {
            Some(tip) => b.tooltip(kit::tip(tip.clone())).into_any_element(),
            None => b.into_any_element(),
        }
    }

    #[inline(never)]
    fn leaf_link(&self, s: &str, g: &Go, tip: &Option<String>, look: LinkLook, cx: &mut Context<MainWindow>) -> AnyElement {
        let g = g.clone();
        let t = self.t;
        let l = if self.parent() == Some(K::PrHead) {
            // `.pr-link`: the PR's number and title in the text colour, underlined on hover.
            div().id(self.id("link")).flex_none().cursor_pointer().text_size(px(13.)).text_color(t.text).hover(|d| d.underline()).child(pr_label(s))
        } else if self.parent() == Some(K::PrBar { warn: false }) || self.parent() == Some(K::PrBar { warn: true }) {
            // `.pr-bar .go`.
            let fg = if self.parent() == Some(K::PrBar { warn: true }) { t.warn_fg } else { t.accent_fg };
            div().id(self.id("link")).flex_none().cursor_pointer().whitespace_nowrap().font_weight(FontWeight::SEMIBOLD).text_color(fg).child(s.to_string())
        } else {
            link(t, self.id("link"), s, look)
        };
        let l = l.on_click(cx.listener(move |m, _, _, cx| go(m, &g, cx)));
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
        let focused = input.focus.is_focused(self.window);
        let meta = self.parent() == Some(K::MetaRow);
        let name = meta && key.ends_with(":0");
        input
            .render(t, self.id("field"), self.window)
            // `.input`: 14px, 10px 12px, 9px corners.
            .text_size(px(14.))
            .px(px(12.))
            .py(px(10.))
            .rounded(px(9.))
            .when(multi, |d| d.min_h(px(64.)).items_start())
            .when(mono, |d| d.font_family(t.mono_font.clone()))
            .when(boxed == Some(BoxTone::Ask) && !focused, |d| d.border_color(warn_input(t)))
            // `.meta-row .input`: 36px, 13px, 7px corners, the light border; the name in semibold.
            .when(meta, |d| d.min_h(px(36.)).py(px(0.)).px(px(10.)).text_size(px(13.)).rounded(px(7.)).when(!focused, |d| d.border_color(t.border)))
            .when(name, |d| d.w(px(140.)).flex_none().font_weight(FontWeight::SEMIBOLD))
            .when(meta && !name, |d| d.flex_1().min_w_0())
            .on_key_down(cx.listener(move |m, ev: &KeyDownEvent, _, cx| on_key(m, &k, ev, cx)))
            .into_any_element()
    }

    #[inline(never)]
    fn leaf_note(&self, s: &str, err: bool) -> AnyElement {
        let t = self.t;
        div()
            .text_size(px(12.5))
            .text_color(if err { t.down } else { t.up_fg })
            .when(err, |d| d.font_weight(FontWeight::MEDIUM))
            .child(s.to_string())
            .into_any_element()
    }

    #[inline(never)]
    fn leaf_dot(&self, d: Dot) -> AnyElement {
        // `.tl-dot` is 9px (an empty ring for a closed terminal); `.dot` 8px.
        let size = if self.parent() == Some(K::TlItem) { 9. } else { 8. };
        match dot_color(self.t, d) {
            Some(c) => kit::dot(c, size).into_any_element(),
            None => div().flex_none().size(px(size)).rounded_full().border_1().border_color(self.t.faint).bg(self.t.card).into_any_element(),
        }
    }

    #[inline(never)]
    fn text(&self, s: &str, st: St, boxed: Option<BoxTone>) -> Div {
        let t = self.t;
        let d = div().child(s.to_string());
        match st {
            // Inherits the container's size and colour.
            St::Plain => d.min_w_0(),
            St::Title => d.text_size(px(20.)).font_weight(FontWeight::BOLD).line_height(px(26.)),
            St::Sub => d.text_size(px(13.)).text_color(t.muted),
            St::Small if boxed.is_some() && self.parent() != Some(K::Lrow) => d.text_size(px(13.)).text_color(t.text_2),
            St::Small | St::Help => d.text_size(px(12.5)).text_color(t.muted),
            St::Muted => d.text_color(t.muted).when(self.parent() == Some(K::Body), |d| d.text_size(px(13.))),
            St::WarnHelp => d.text_size(px(12.5)).text_color(t.warn_fg),
            St::BoxLabel => div().child(s.to_uppercase()).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(match boxed {
                Some(BoxTone::Ask) => t.warn_fg,
                Some(BoxTone::Lost | BoxTone::Failed) => t.down,
                Some(BoxTone::Done) => t.up_fg,
                _ => t.accent_fg,
            }),
            St::H3 => div()
                .child(s.to_uppercase())
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if boxed == Some(BoxTone::Info) { t.accent_fg } else { t.muted })
                .whitespace_nowrap(),
            St::Strong => d.font_weight(FontWeight::SEMIBOLD),
            St::Mono => d.font_family(t.mono_font.clone()),
            St::Empty => d.py(px(20.)).px(px(8.)).flex().justify_center().text_size(px(13.)).text_color(t.muted),
            St::Time => d.text_size(px(12.)).text_color(t.muted).whitespace_nowrap(),
            St::Kind => d.flex_none().px(px(5.)).rounded(px(4.)).bg(t.panel_2).text_size(px(11.5)).text_color(t.muted),
            // `.handoff`: 13px pre-wrap, at most 360px tall (it scrolls past that).
            St::Handoff => div().child(
                div().id(self.id("handoff")).max_h(px(360.)).overflow_y_scroll().text_size(px(13.)).line_height(relative(1.5)).text_color(t.text).child(s.to_string()),
            ),
            St::Code => d.px(px(5.)).py(px(1.)).rounded(px(5.)).bg(t.bg).border_1().border_color(t.border).font_family(t.mono_font.clone()).text_size(px(12.3)),
            St::Pre => d.px(px(12.)).py(px(10.)).rounded(px(8.)).bg(t.bg).border_1().border_color(t.border).font_family(t.mono_font.clone()).text_size(px(12.5)),
            St::StepName => d.text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).whitespace_nowrap(),
            St::StepSub => d.text_size(px(11.5)).text_center(),
            St::LrowKey => d.text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(t.muted),
            St::Count => d.flex_none().px(px(7.)).rounded_full().bg(t.col).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(t.muted),
            St::Jkey => d.font_family(t.mono_font.clone()).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.accent_fg),
            St::Glyph(dot) => d.flex_none().w(px(14.)).flex().justify_center().text_color(dot_color(t, dot).unwrap_or(t.muted)),
        }
    }

    #[inline(never)]
    fn container(&mut self, m: &MainWindow, n: &Node, k: K, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        match k {
            K::Fold(f, open) => self.c_fold(m, n, f, open, boxed, cx),
            K::Box(tone) => self.c_box(m, n, tone, cx),
            K::Top => self.c_top(m, n, boxed, cx),
            K::Tabs => self.c_tabs(m, n, boxed, cx),
            K::Linked => self.c_linked(m, n, boxed, cx),
            K::Lrow => self.c_lrow(m, n, boxed, cx),
            K::Lsec => self.c_lsec(m, n, boxed, cx),
            K::Tl => self.c_tl(m, n, boxed, cx),
            K::TlItem => self.c_tl_item(m, n, boxed, cx),
            K::Steps => self.c_steps(m, n, boxed, cx),
            K::Step(st) => self.c_step(m, n, st, boxed, cx),
            K::Att => self.c_att(m, n, boxed, cx),
            K::Kv => self.c_kv(m, n, boxed, cx),
            K::Files => self.c_files(m, n, boxed, cx),
            K::LogItem => self.c_log_item(m, n, boxed, cx),
            K::MdLi => self.c_md_li(m, n, boxed, cx),
            K::Li => self.c_li(m, n, boxed, cx),
            _ => self.c_simple(m, n, k, boxed, cx),
        }
    }

    /// Plain flex boxes.
    #[inline(never)]
    fn c_simple(&mut self, m: &MainWindow, n: &Node, k: K, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let in_lrow = self.parent() == Some(K::Lrow);
        let kids = self.kids(m, n, boxed, cx);
        let small_btns = matches!(n, Node::El { kids, .. } if kids.iter().any(|x| matches!(x, Node::Btn { look: Look::Small | Look::DangerSmall, .. })));
        // `.stack` is 6px apart; the manage box, Files touched and Details 8px.
        let wide = small_btns
            || matches!(n, Node::El { kids, .. } if kids.iter().any(|x| matches!(x, Node::El { k: K::Row, kids, .. } if kids.iter().any(|b| matches!(b, Node::Btn { look: Look::Small | Look::DangerSmall, .. })))
                || matches!(kids.first(), Some(Node::Text { s, st: St::H3, .. }) if s == "Files touched" || s == "Details")));
        let d = match k {
            // `.subline`
            K::Sub => div().flex().flex_wrap().items_center().gap_x(px(12.)).gap_y(px(6.)).text_size(px(13.)).text_color(t.muted),
            // `.fchips`
            K::Chips => div().flex().flex_wrap().gap(px(6.)),
            K::Seg => div().self_start().flex().flex_wrap().gap(px(2.)).p(px(3.)).rounded(px(9.)).bg(t.seg),
            // `.tabbody`
            K::Body => div().flex().flex_col().gap(px(18.)),
            // `.manage .row` is 6px apart; `.row` 8px.
            K::Row => div().flex().flex_wrap().items_center().gap(px(if small_btns { 6. } else { 8. })),
            // `.lrow .line`
            K::Line if in_lrow => div().flex().flex_wrap().items_center().gap(px(8.)).text_size(px(13.)),
            K::Line => div().flex().flex_wrap().items_center(),
            // `.pr-head`
            K::PrHead => div().flex().flex_col().gap(px(2.)),
            // `.tl-name` / `.tl-sub` are styled by the item (live or not).
            K::TlName => div().flex().flex_wrap().items_center().gap(px(6.)).min_h(px(24.)),
            K::TlSub => div().flex().flex_wrap().items_center().gap(px(6.)),
            K::AttMeta => div().flex().items_center().gap(px(4.)).text_size(px(12.)).text_color(t.muted).whitespace_nowrap().overflow_hidden(),
            // `.stack` (the manage box and files 8px apart; lists 6px).
            K::Stack => div().flex().flex_col().gap(px(if wide { 8. } else { 6. })).min_w_0(),
            K::Md => div().flex().flex_col().gap(px(10.)).min_w_0().text_size(px(14.)).line_height(relative(1.55)).text_color(t.text),
            K::List => div().flex().flex_col().gap(px(4.)).min_w_0(),
            K::Atts => div().flex().flex_col().min_w_0(),
            K::MdUl => div().flex().flex_col().gap(px(4.)).pl(px(4.)).min_w_0(),
            K::Col => div().flex().flex_col().gap(px(18.)).min_w_0(),
            // `.pr-bar`
            K::PrBar { warn } => {
                let (fg, bg, line) = if warn { (t.warn_text, t.warn_soft, t.warn_line) } else { (t.text_2, t.panel_2, transparent_black()) };
                div().flex().items_center().gap(px(8.)).w_full().px(px(12.)).py(px(7.)).rounded(px(8.)).bg(bg).border_1().border_color(line).text_color(fg).text_size(px(13.))
            }
            K::AttMenu => kit::menu_box(t, 160.),
            K::AttEdit => div().flex().flex_col().gap(px(8.)).w_full().py(px(6.)),
            K::KvRow => div().flex().gap(px(12.)),
            // `.meta-row`: 140px name, value, 32px remove.
            K::MetaRow => div().flex().items_center().gap(px(8.)),
            // `.log`
            K::Log => div().flex().flex_col(),
            // `.note-body p`: lines joined by line breaks.
            K::MdP => div().flex().flex_col().min_w_0(),
            // Linked, Log, MdP
            _ => div().flex().flex_col().min_w_0(),
        };
        let d = match k {
            K::PrBar { .. } => d.children(kids.into_iter().enumerate().map(|(i, e)| if i == 1 { div().flex_1().min_w_0().child(e).into_any_element() } else { e })),
            _ => d.children(kids),
        };
        d.into_any_element()
    }

    /// `<details>`: the board's folds (`.linked-d`, `.box-fold`, `.tl-more`).
    #[inline(never)]
    fn c_fold(&mut self, m: &MainWindow, n: &Node, f: Fold, open: bool, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let Node::El { kids, .. } = n else { return div().into_any_element() };
        let hover = t.text;
        let toggle = cx.listener(move |_, _, _, cx| {
            crate::prefs::set(f.key(), json!(if open { "closed" } else { "open" }));
            cx.notify();
        });
        let label = match kids.first() {
            Some(Node::Text { s, .. }) => s.clone(),
            _ => String::new(),
        };
        let head = match f {
            // `.linked-d > summary`: 12.5px semibold uppercase muted, a chevron before.
            Fold::Linked | Fold::What => div()
                .id(self.id("fold"))
                .flex()
                .items_center()
                .gap(px(6.))
                .pt(px(2.))
                .pb(px(8.))
                .cursor_pointer()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.muted)
                .hover(move |d| d.text_color(hover))
                .child(div().mr(px(2.)).child(chevron(open, t.muted)))
                .child(label.to_uppercase()),
            // `.box-fold > summary`: the box label with a chevron, 8px apart.
            Fold::Summary => {
                let summary = self.kid(m, n, 0, boxed, cx);
                let c = match boxed {
                    Some(BoxTone::Failed | BoxTone::Lost) => t.down,
                    Some(BoxTone::Ask) => t.warn_fg,
                    _ => t.up_fg,
                };
                div().id(self.id("fold")).flex().items_center().gap(px(8.)).cursor_pointer().child(chevron(open, c)).children(summary)
            }
            // `.tl-more > summary`: 12.5px muted, the browser's disclosure triangle.
            Fold::Terms | Fold::Findings => div()
                .id(self.id("fold"))
                .flex()
                .items_center()
                .gap(px(4.))
                .self_start()
                .cursor_pointer()
                .text_size(px(12.5))
                .text_color(t.muted)
                .hover(move |d| d.text_color(hover))
                .child(div().text_size(px(9.)).child(if open { "▼" } else { "▶" }))
                .child(label),
        };
        let head = head.on_click(toggle);
        let mut col = div().flex().flex_col().min_w_0().child(head);
        if open {
            let gap = match f {
                Fold::Linked => 0.,
                Fold::What => 2.,
                Fold::Summary => 8.,
                Fold::Terms => 10.,
                Fold::Findings => 6.,
            };
            for i in 1..kids.len() {
                if let Some(e) = self.kid(m, n, i, boxed, cx) {
                    col = col.child(div().mt(px(gap)).min_w_0().when(f == Fold::What, |d| d.text_size(px(14.))).child(e));
                }
            }
        }
        col.into_any_element()
    }

    /// `.box`: 14px 16px, 12px corners, tinted by kind.
    #[inline(never)]
    fn c_box(&mut self, m: &MainWindow, n: &Node, tone: BoxTone, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let (bg, line, gap) = match tone {
            BoxTone::Ask => (t.warn_soft, None, 8.),
            BoxTone::Lost => (t.down_soft, None, 10.),
            BoxTone::Failed => (t.down_soft, None, 8.),
            BoxTone::Done => (t.up_soft, None, 8.),
            BoxTone::Info => (accent_tint(t), Some(t.accent_line), 10.),
        };
        let kids = self.kids(m, n, Some(tone), cx);
        div()
            .flex()
            .flex_col()
            .gap(px(gap))
            .px(px(16.))
            .py(px(14.))
            .rounded(px(12.))
            .bg(bg)
            .when_some(line, |d, c| d.border_1().border_color(c))
            .text_size(px(14.))
            .children(kids)
            .into_any_element()
    }

    /// `.panel-top`: the pills, then the close button at the far end.
    #[inline(never)]
    fn c_top(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let kids = self.kids(m, n, boxed, cx);
        let n_kids = kids.len();
        let mut row = div().flex().flex_none().flex_wrap().items_center().gap(px(8.));
        for (i, e) in kids.into_iter().enumerate() {
            if i + 1 == n_kids {
                row = row.child(div().flex_1());
            }
            row = row.child(e);
        }
        row.into_any_element()
    }

    /// `.tabs`: underlined tabs on a rule that runs the sheet's full width.
    #[inline(never)]
    fn c_tabs(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let kids = self.kids(m, n, boxed, cx);
        div().flex().flex_none().gap(px(4.)).mx(px(-24.)).px(px(24.)).border_b_1().border_color(t.border).children(kids).into_any_element()
    }

    /// `.linked`: a bordered card of rows split by hairlines.
    #[inline(never)]
    fn c_linked(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let kids = self.kids(m, n, boxed, cx);
        let mut col = div().flex().flex_col().min_w_0().rounded(px(12.)).border_1().border_color(t.border);
        for (i, e) in kids.into_iter().enumerate() {
            col = col.child(div().min_w_0().when(i > 0, |d| d.border_t_1().border_color(t.divider)).child(e));
        }
        col.into_any_element()
    }

    /// `.lrow`: a 92px key, then the value column (8px apart), 14px 16px.
    #[inline(never)]
    fn c_lrow(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let blocked = matches!(n, Node::El { kids, .. } if matches!(kids.first(), Some(Node::Text { s, .. }) if s == "Blocked by"));
        let mut kids = self.kids(m, n, boxed, cx).into_iter();
        let key = kids.next();
        div()
            .flex()
            .gap(px(12.))
            .px(px(16.))
            .py(px(14.))
            .when(blocked, |d| d.border_1().border_color(t.warn).rounded_t(px(11.)))
            .child(div().flex_none().w(px(92.)).pt(px(2.)).children(key))
            .child(div().flex().flex_col().gap(px(8.)).flex_1().min_w_0().children(kids))
            .into_any_element()
    }

    /// `.lsec`: a titled section in the linked card (Attached).
    #[inline(never)]
    fn c_lsec(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let mut kids = self.kids(m, n, boxed, cx).into_iter();
        let head = kids.next();
        div()
            .flex()
            .flex_col()
            .gap(px(8.))
            .px(px(16.))
            .py(px(14.))
            .min_w_0()
            // `.lsec-h`
            .child(div().text_size(px(12.5)).text_color(t.muted).children(head))
            .children(kids)
            .into_any_element()
    }

    /// `.tl`: terminals 12px apart, joined by a thin line.
    #[inline(never)]
    fn c_tl(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let kids = self.kids(m, n, boxed, cx);
        let last = kids.len().saturating_sub(1);
        let mut col = div().flex().flex_col().gap(px(12.)).min_w_0();
        for (i, e) in kids.into_iter().enumerate() {
            col = col.child(
                div()
                    .relative()
                    .min_w_0()
                    .when(i < last, |d| d.child(div().absolute().left(px(4.)).top(px(20.)).bottom(px(-10.)).w(px(1.)).bg(t.border_2)))
                    .child(e),
            );
        }
        col.into_any_element()
    }

    /// `.tl-item`: dot, name and state, time.
    #[inline(never)]
    fn c_tl_item(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let live = !matches!(n, Node::El { kids, .. } if matches!(kids.first(), Some(Node::Dot(Dot::None))));
        let mut kids = self.kids(m, n, boxed, cx).into_iter();
        let (dot, name, sub, at) = (kids.next(), kids.next(), kids.next(), kids.next());
        div()
            .flex()
            .items_start()
            .gap(px(10.))
            .child(div().flex_none().w(px(9.)).pt(px(8.)).children(dot))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(if live { FontWeight::BOLD } else { FontWeight::SEMIBOLD })
                            .when(!live, |d| d.text_color(t.muted))
                            .children(name),
                    )
                    .child(div().text_size(px(12.5)).text_color(if live { t.muted } else { t.faint }).children(sub)),
            )
            .children(at.map(|a| div().flex_none().pt(px(4.)).text_color(if live { t.muted } else { t.faint }).child(a)))
            .into_any_element()
    }

    /// `.pr-steps`: three steps joined by lines.
    #[inline(never)]
    fn c_steps(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let kids = self.kids(m, n, boxed, cx);
        let mut row = div().flex().items_start().pt(px(4.)).pb(px(2.));
        for (i, e) in kids.into_iter().enumerate() {
            if i > 0 {
                row = row.child(div().flex_1().min_w(px(14.)).h(px(2.)).mt(px(10.)).rounded(px(1.)).bg(t.border_2));
            }
            row = row.child(e);
        }
        row.into_any_element()
    }

    /// `.pr-step`: a 22px dot, the name, the state below.
    #[inline(never)]
    fn c_step(&mut self, m: &MainWindow, n: &Node, st: StepSt, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let (name_c, sub_c) = match st {
            StepSt::Done => (t.text_2, t.up),
            StepSt::Fail => (t.down, t.down),
            StepSt::Ask => (t.warn, t.warn),
            StepSt::Wait | StepSt::Now => (t.text_2, t.accent_fg),
            StepSt::Todo => (t.muted, t.faint),
        };
        let bold = matches!(st, StepSt::Fail | StepSt::Ask);
        let dot = step_dot(t, st);
        let mut kids = self.kids(m, n, boxed, cx).into_iter();
        let (name, sub) = (kids.next(), kids.next());
        div()
            .flex()
            .flex_col()
            .flex_none()
            .items_center()
            .gap(px(4.))
            .child(dot)
            .child(div().text_color(name_c).when(bold, |d| d.font_weight(FontWeight::BOLD)).children(name))
            .children(sub.map(|s| div().text_color(sub_c).child(s)))
            .into_any_element()
    }

    /// `.att`: the kind's icon, the name over its kind / source / age, the menu button.
    #[inline(never)]
    fn c_att(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let kind = match n {
            Node::El { kids, .. } => match kids.get(1) {
                Some(Node::El { kids: meta, .. }) => match meta.first() {
                    Some(Node::Text { s, .. }) => view::ATT_KINDS.iter().find(|k| view::att_kind_label(k) == s).copied().unwrap_or("other"),
                    _ => "other",
                },
                _ => "other",
            },
            _ => "other",
        };
        let (fg, bg) = match kind {
            "design" => (t.warn, t.warn_soft),
            "proposal" => (t.goal, t.goal_soft),
            "doc" => (t.accent_fg, t.accent_soft),
            "evidence" => (t.up, t.up_soft),
            "results" => (t.results, t.results_soft),
            _ => (t.text_2, t.col),
        };
        let mut it = self.kids(m, n, boxed, cx).into_iter();
        let (name, meta, more, menu) = (it.next(), it.next(), it.next(), it.next());
        let tint = t.tint;
        div()
            .id(self.id("att"))
            .group("tp-att")
            .flex()
            .items_center()
            .gap(px(10.))
            .p(px(6.))
            .mx(px(-6.))
            .rounded(px(8.))
            .text_size(px(13.))
            .hover(move |d| d.bg(tint))
            .child(div().flex_none().size(px(26.)).rounded(px(7.)).bg(bg).flex().items_center().justify_center().child(att_icon(kind, fg)))
            .child(div().flex().flex_col().gap(px(1.)).flex_1().min_w_0().children(name).children(meta))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .w(px(24.))
                    .children(more)
                    .children(menu.map(|x| div().absolute().right_0().top(px(28.)).child(deferred(x).with_priority(2)))),
            )
            .into_any_element()
    }

    /// `.kv`: a bordered card of 110px keys and values.
    #[inline(never)]
    fn c_kv(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let Node::El { kids: rows, .. } = n else { return div().into_any_element() };
        let mut col = div().flex().flex_col().rounded(px(12.)).border_1().border_color(t.border);
        self.within.push(K::Kv);
        for (i, row) in rows.iter().enumerate() {
            self.path.push(i);
            let mut kv = self.kids(m, row, boxed, cx).into_iter();
            self.path.pop();
            let (k, v) = (kv.next(), kv.next());
            col = col.child(
                div()
                    .flex()
                    .gap(px(12.))
                    .px(px(16.))
                    .py(px(9.))
                    .when(i > 0, |d| d.border_t_1().border_color(t.divider))
                    .child(div().flex_none().w(px(110.)).children(k))
                    .child(div().flex_1().min_w_0().text_size(px(13.)).children(v)),
            );
        }
        self.within.pop();
        col.into_any_element()
    }

    /// `.files`: file names in mono chips.
    #[inline(never)]
    fn c_files(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let kids = self.kids(m, n, boxed, cx);
        div()
            .flex()
            .flex_wrap()
            .gap(px(6.))
            .children(kids.into_iter().map(|e| {
                div().px(px(8.)).py(px(2.)).rounded(px(5.)).bg(t.panel_2).font_family(t.mono_font.clone()).text_size(px(12.)).text_color(t.text_2).whitespace_nowrap().child(e)
            }))
            .into_any_element()
    }

    /// `.log li`: 52px time, 14px dot, who / kind over the text.
    #[inline(never)]
    fn c_log_item(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        let mut it = self.kids(m, n, boxed, cx).into_iter();
        let (time, dot, who, kind, text) = (it.next(), it.next(), it.next(), it.next(), it.next());
        div()
            .flex()
            .gap(px(8.))
            .pb(px(12.))
            .child(div().flex_none().w(px(52.)).pt(px(1.)).children(time))
            .child(div().flex_none().w(px(14.)).pt(px(6.)).children(dot))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(div().flex().flex_wrap().items_center().gap(px(6.)).child(div().text_size(px(13.)).children(who)).children(kind))
                    .child(div().text_size(px(13.)).text_color(t.text_2).children(text)),
            )
            .into_any_element()
    }

    /// `.list li`: the glyph, then the text. A found issue's link and "(in the backlog)" flow as
    /// one run of text, as the web's inline `<a>` did.
    #[inline(never)]
    fn c_li(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let t = self.t;
        if let Node::El { kids, .. } = n {
            if let [g @ Node::Text { st: St::Glyph(_), .. }, Node::Link { s: name, go: target, .. }, Node::Text { s: rest, st: St::Muted, .. }] = kids.as_slice() {
                let glyph = self.node(m, g, boxed, cx);
                let text = format!("{name} {rest}");
                let under = UnderlineStyle { thickness: px(1.), color: Some(t.accent), wavy: false };
                let runs = vec![
                    (0..name.len(), HighlightStyle { color: Some(t.accent), underline: Some(under), ..Default::default() }),
                    (name.len() + 1..text.len(), HighlightStyle { color: Some(t.muted), ..Default::default() }),
                ];
                let this = cx.entity().downgrade();
                let target = target.clone();
                let body = InteractiveText::new(self.id("li"), StyledText::new(text).with_highlights(runs)).on_click(vec![0..name.len()], move |_, _, cx| {
                    let _ = this.update(cx, |m, cx| go(m, &target, cx));
                });
                return div().flex().items_start().gap(px(8.)).child(glyph).child(div().flex_1().min_w_0().child(body)).into_any_element();
            }
        }
        let kids = self.kids(m, n, boxed, cx);
        div().flex().items_start().gap(px(8.)).children(kids).into_any_element()
    }

    #[inline(never)]
    fn c_md_li(&mut self, m: &MainWindow, n: &Node, boxed: Option<BoxTone>, cx: &mut Context<MainWindow>) -> AnyElement {
        let kids = self.kids(m, n, boxed, cx);
        div().flex().gap(px(6.)).child(div().flex_none().w(px(8.)).child("•")).child(div().flex().flex_wrap().items_center().gap(px(4.)).min_w_0().children(kids)).into_any_element()
    }
}

/// "#101 Store sessions…" with the number in bold (`<b>#101</b> title`).
fn pr_label(s: &str) -> Div {
    let (num, rest) = s.split_once(' ').unwrap_or((s, ""));
    div().flex().flex_wrap().gap(px(4.)).child(div().font_weight(FontWeight::BOLD).child(num.to_string())).child(rest.to_string())
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

/// `.pill` (12px semibold, 2px 10px, round) / `.pill.sm` (1px 8px).
fn pill(t: &Theme, s: &str, tone: Tone, small: bool) -> Div {
    let (fg, bg, line) = match tone {
        Tone::Queued | Tone::Neutral => (t.text_2, t.col, None),
        Tone::Review => (t.warn_fg, t.col, Some(t.warn)),
        Tone::Needs | Tone::Blocked => (t.warn_fg, t.warn_soft, None),
        Tone::High => (t.warn, t.warn_soft, None),
        Tone::Done => (t.up_fg, t.up_soft, None),
        Tone::Failed => (t.down, t.down_soft, None),
        Tone::Planned => (t.muted, t.card, Some(t.border)),
        Tone::Working | Tone::Jira => (t.accent_fg, t.accent_soft, None),
        Tone::Idle | Tone::Repo => (t.text_2, t.panel_2, None),
        Tone::Ref | Tone::Gone => (t.muted, t.panel_2, None),
        Tone::Closed => (t.faint, transparent_black(), Some(t.border)),
    };
    let p = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .h(px(if small { 20. } else { 22. }))
        .px(px(if small { 8. } else { 10. }))
        .rounded_full()
        .bg(bg)
        .text_color(fg)
        .text_size(px(12.))
        .font_weight(FontWeight::SEMIBOLD)
        .whitespace_nowrap()
        .when_some(line, |d, c| d.border_1().border_color(c))
        .child(s.to_string());
    if matches!(tone, Tone::Ref | Tone::Repo) { p.font_family(t.mono_font.clone()).font_weight(FontWeight::MEDIUM) } else { p }
}

/// `.btn`: 36px, 13px, 14px sides, 8px corners, the light border; hover darkens the border.
fn base(t: &Theme, id: SharedString, label: String) -> Stateful<Div> {
    let h = t.border_2;
    raw(t, id, label).hover(move |d| d.border_color(h))
}

/// `.btn` without a hover style (GPUI keeps one per element).
fn raw(t: &Theme, id: SharedString, label: String) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap(px(6.))
        .h(px(36.))
        .px(px(14.))
        .rounded(px(8.))
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .text_color(t.text)
        .text_size(px(13.))
        .whitespace_nowrap()
        .cursor_pointer()
        .child(label)
}

/// `.btn.sm`: 32px, 12.5px, 10px sides, 7px corners.
fn sm(b: Stateful<Div>) -> Stateful<Div> {
    b.h(px(32.)).px(px(10.)).text_size(px(12.5)).rounded(px(7.))
}

/// `.btn.soft`: accent text on the accent tint, no border.
fn soft(t: &Theme, id: SharedString, label: String) -> Stateful<Div> {
    raw(t, id, label).bg(t.accent_soft).border_color(transparent_black()).text_color(t.accent).font_weight(FontWeight::SEMIBOLD)
}

fn danger(t: &Theme, id: SharedString, label: String) -> Stateful<Div> {
    raw(t, id, label).text_color(t.down).border_color(t.down_line)
}

/// `.tab`: 14px semibold, 42px, an accent underline when selected.
fn tab_btn(t: &Theme, id: SharedString, label: &str, on: bool) -> Stateful<Div> {
    let hover = t.text;
    div()
        .id(id)
        .flex()
        .items_center()
        .min_h(px(42.))
        .px(px(10.))
        .mb(px(-1.))
        .border_b_2()
        .border_color(if on { t.accent } else { transparent_black() })
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .whitespace_nowrap()
        .text_color(if on { t.text } else { t.muted })
        .hover(move |s| s.text_color(hover))
        .child(label.to_string())
}

/// `.fchip`: a round 32px filter chip; the pressed one inverted.
fn fchip(t: &Theme, id: SharedString, label: &str, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(32.))
        .px(px(12.))
        .rounded_full()
        .border_1()
        .border_color(if on { t.text } else { t.border })
        .bg(if on { t.text } else { t.card })
        .text_color(if on { t.bg } else { t.text_2 })
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .whitespace_nowrap()
        .child(label.to_string())
}

fn link(t: &Theme, id: SharedString, s: &str, look: LinkLook) -> Stateful<Div> {
    let text = s.to_string();
    let a = div().id(id).flex_none().cursor_pointer();
    match look {
        // `.term-link`: the text colour, underlined in the accent on hover.
        LinkLook::Term => {
            let h = t.accent_fg;
            a.hover(move |d| d.text_color(h).underline()).child(text)
        }
        // `<a>`: accent, underlined.
        LinkLook::Plain => a.flex_shrink(1.).text_color(t.accent).underline().child(text),
        LinkLook::Small => a.text_size(px(12.5)).text_color(t.accent).underline().child(text),
        LinkLook::GoalName => a.text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.goal).underline().child(text),
        LinkLook::Jkey => a.font_family(t.mono_font.clone()).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.accent_fg).underline().child(text),
        LinkLook::AttName => a.flex_shrink(1.).min_w_0().font_weight(FontWeight::SEMIBOLD).text_color(t.accent_fg).truncate().hover(|d| d.underline()).child(text),
        LinkLook::SrcGoal => a
            .px(px(5.))
            .rounded(px(5.))
            .font_family(t.mono_font.clone())
            .text_size(px(11.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.goal)
            .bg(t.goal_soft)
            .hover(|d| d.underline())
            .child(text),
    }
}

/// `.icon-btn` (36px), `.sm` (32px) and `.xs` (24px).
fn icon_btn(t: &Theme, id: SharedString, size: f32, radius: f32, glyph: impl IntoElement) -> Stateful<Div> {
    let (h, hb) = (t.text, t.border_2);
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(px(size))
        .rounded(px(radius))
        .border_1()
        .border_color(t.border)
        .bg(t.card)
        .text_color(t.muted)
        .cursor_pointer()
        .hover(move |d| d.text_color(h).border_color(hb))
        .child(glyph)
}

fn button(t: &Theme, id: SharedString, label: &str, look: Look) -> Stateful<Div> {
    let label = label.to_string();
    match look {
        Look::Plain => base(t, id, label),
        Look::Primary | Look::PrimaryWide => {
            let b = raw(t, id, label).bg(t.accent_btn).border_color(transparent_black()).text_color(t.on_accent).font_weight(FontWeight::SEMIBOLD).hover(|d| d.opacity(0.94));
            // `.lg.wide`: 42px, 14px, sharing the row.
            if look == Look::PrimaryWide { b.flex_1().h(px(42.)).px(px(16.)).text_size(px(14.)) } else { b }
        }
        Look::SoftWide => soft(t, id, label).flex_1().h(px(42.)).px(px(16.)).text_size(px(14.)),
        Look::Soft => soft(t, id, label),
        Look::SoftSmall => sm(soft(t, id, label)),
        Look::Small => sm(base(t, id, label)),
        Look::Danger => danger(t, id, label),
        Look::DangerSmall => sm(danger(t, id, label)),
        Look::Ghost => {
            let h = t.text;
            sm(raw(t, id, label).border_color(transparent_black()).bg(transparent_black()).text_color(t.muted).hover(move |d| d.text_color(h)))
        }
        // `.btn.link`: accent text, underlined.
        Look::Link => div().id(id).flex_none().self_start().cursor_pointer().min_h(px(28.)).flex().items_center().text_size(px(13.)).text_color(t.accent_fg).underline().child(label),
        // `.btn.link.inline`
        Look::Ref => div().id(id).flex_none().cursor_pointer().text_color(t.accent_fg).underline().child(label),
        Look::Src => div()
            .id(id)
            .px(px(5.))
            .rounded(px(5.))
            .cursor_pointer()
            .font_family(t.mono_font.clone())
            .text_size(px(11.5))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(t.accent_fg)
            .bg(t.accent_soft)
            .hover(|d| d.underline())
            .child(label),
        Look::Menu => kit::menu_item(t, id, label, false),
        // Segmented buttons (the attachment kind picker).
        Look::Tab { on } => {
            let hover = t.text;
            div()
                .id(id)
                .flex()
                .items_center()
                .h(px(32.))
                .px(px(11.))
                .rounded(px(7.))
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .whitespace_nowrap()
                .text_color(if on { t.text } else { t.muted })
                .when(on, |d| d.bg(t.card).shadow_sm())
                .hover(move |s| s.text_color(hover))
                .child(label)
        }
        Look::Icon(icon) => match icon {
            Icon::Close => icon_btn(t, id, 36., 8., paint(Ic::X, 16., 2.2, t.muted)),
            Icon::Back => icon_btn(t, id, 36., 8., paint(Ic::Back, 16., 2.2, t.muted)),
            Icon::Remove => icon_btn(t, id, 32., 7., paint(Ic::X, 16., 2.2, t.muted)),
            Icon::External => icon_btn(t, id, 24., 6., paint(Ic::Ext, 12., 2.4, t.muted)),
            Icon::More => icon_btn(t, id, 24., 6., paint(Ic::More, 16., 0., t.muted)),
        },
    }
}

// ------------------------------------------------------------------ icons (the web's inline SVGs)

#[derive(Clone, Copy)]
enum Ic {
    X,
    Back,
    Ext,
    More,
    Check,
    Clock,
    Comment,
    /// `.s-now`: a ring with its right quarter open.
    Spinner,
    Att(&'static str),
}

/// A stroked icon on the web's 24-unit (attachments: 16-unit) viewbox, `size` px square.
fn paint(ic: Ic, size: f32, stroke: f32, color: Hsla) -> impl IntoElement {
    use std::f32::consts::PI;
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let vb = if matches!(ic, Ic::Att(_)) { 16. } else { 24. };
            let k = b.size.width.as_f32() / vb;
            let u = |x: f32, y: f32| point(b.origin.x + px(x * k), b.origin.y + px(y * k));
            let line = |pts: &[(f32, f32)], window: &mut Window| {
                let mut p = PathBuilder::stroke(px(stroke * k));
                p.move_to(u(pts[0].0, pts[0].1));
                for (x, y) in &pts[1..] {
                    p.line_to(u(*x, *y));
                }
                if let Ok(p) = p.build() {
                    window.paint_path(p, color);
                }
            };
            // An arc on a circle, angles in degrees clockwise from 3 o'clock.
            let arc = |c: (f32, f32), r: f32, from: f32, to: f32, window: &mut Window| {
                let n = (((to - from).abs() / 360.) * 48.).ceil().max(2.) as usize;
                let pts: Vec<(f32, f32)> = (0..=n)
                    .map(|i| {
                        let a = (from + (to - from) * i as f32 / n as f32) * PI / 180.;
                        (c.0 + r * a.cos(), c.1 + r * a.sin())
                    })
                    .collect();
                line(&pts, window);
            };
            let fill_dot = |c: (f32, f32), r: f32, window: &mut Window| {
                let mut p = PathBuilder::fill();
                p.move_to(u(c.0 + r, c.1));
                for i in 1..=24 {
                    let a = 2. * PI * i as f32 / 24.;
                    p.line_to(u(c.0 + r * a.cos(), c.1 + r * a.sin()));
                }
                p.close();
                if let Ok(p) = p.build() {
                    window.paint_path(p, color);
                }
            };
            match ic {
                Ic::X => {
                    line(&[(6., 6.), (18., 18.)], window);
                    line(&[(18., 6.), (6., 18.)], window);
                }
                Ic::Back => line(&[(15., 6.), (9., 12.), (15., 18.)], window),
                Ic::Ext => {
                    line(&[(14., 4.), (20., 4.), (20., 10.)], window);
                    line(&[(20., 4.), (11., 13.)], window);
                    line(&[(18., 14.), (18., 20.), (4., 20.), (4., 6.), (10., 6.)], window);
                }
                Ic::More => {
                    for x in [5., 12., 19.] {
                        fill_dot((x, 12.), 1.8, window);
                    }
                }
                Ic::Check => line(&[(5., 12.5), (9.5, 17.), (19., 7.5)], window),
                Ic::Clock => {
                    arc((12., 12.), 8.5, 0., 360., window);
                    line(&[(12., 7.5), (12., 12.), (15., 14.)], window);
                }
                Ic::Comment => {
                    arc((12., 12.), 8., 0., 117., window);
                    line(&[(8.4, 19.1), (4., 20.), (5., 15.9)], window);
                    arc((12., 12.), 8., 151., 360., window);
                }
                Ic::Spinner => arc((12., 12.), 11., 45., 315., window),
                Ic::Att(kind) => match kind {
                    "design" => {
                        line(&[(2.5, 2.5), (13.5, 2.5), (13.5, 13.5), (2.5, 13.5), (2.5, 2.5)], window);
                        line(&[(2.5, 6.5), (13.5, 6.5)], window);
                        line(&[(6.5, 6.5), (6.5, 13.5)], window);
                    }
                    "proposal" => {
                        arc((8., 6.5), 4., 127., 413., window);
                        line(&[(5.6, 9.7), (6.5, 11.3), (6.5, 11.5), (9.5, 11.5), (9.5, 11.3), (10.4, 9.7)], window);
                        line(&[(6.5, 13.5), (9.5, 13.5)], window);
                    }
                    "doc" => {
                        line(&[(4., 2.5), (9.5, 2.5), (12., 5.), (12., 13.5), (4., 13.5), (4., 2.5)], window);
                        line(&[(6.5, 8.), (9.5, 8.)], window);
                        line(&[(6.5, 10.5), (9.5, 10.5)], window);
                    }
                    "evidence" => {
                        line(&[(2.5, 13.5), (13.5, 13.5)], window);
                        line(&[(4.5, 11.), (4.5, 8.)], window);
                        line(&[(8., 11.), (8., 4.5)], window);
                        line(&[(11.5, 11.), (11.5, 6.5)], window);
                    }
                    "results" => {
                        line(&[(2.5, 13.5), (13.5, 13.5)], window);
                        line(&[(3.5, 10.5), (6.5, 7.5), (9., 9.5), (13., 5.)], window);
                        line(&[(10., 5.), (13., 5.), (13., 8.)], window);
                    }
                    _ => {
                        arc((10.3, 5.7), 2.5, -145., 55., window);
                        arc((5.7, 10.3), 2.5, 35., 235., window);
                        line(&[(6.8, 9.2), (9.2, 6.8)], window);
                    }
                },
            }
        },
    )
    .size(px(size))
    .flex_none()
}

fn att_icon(kind: &'static str, color: Hsla) -> impl IntoElement {
    paint(Ic::Att(kind), 14., 1.6, color)
}

/// The `<details>` marker: a 6px chevron, right when shut, down when open.
fn chevron(open: bool, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let u = |x: f32, y: f32| point(b.origin.x + px(x), b.origin.y + px(y));
            let pts = if open { [(1.5, 3.), (5., 6.5), (8.5, 3.)] } else { [(3., 1.5), (6.5, 5.), (3., 8.5)] };
            let mut p = PathBuilder::stroke(px(1.6));
            p.move_to(u(pts[0].0, pts[0].1));
            for (x, y) in &pts[1..] {
                p.line_to(u(*x, *y));
            }
            if let Ok(p) = p.build() {
                window.paint_path(p, color);
            }
        },
    )
    .size(px(10.))
    .flex_none()
}

/// `.pr-step-dot` in each state.
fn step_dot(t: &Theme, st: StepSt) -> AnyElement {
    let d = div().flex_none().size(px(22.)).rounded_full().flex().items_center().justify_center();
    // `box-shadow: 0 0 0 2px`: a 2px ring outside the dot.
    let ring = |c: Hsla, inner: Div| div().flex_none().size(px(26.)).m(px(-2.)).p(px(2.)).rounded_full().bg(c).child(inner).into_any_element();
    match st {
        StepSt::Done => d.bg(t.up_soft).child(paint(Ic::Check, 12., 3., t.up)).into_any_element(),
        StepSt::Fail => ring(t.down, d.bg(t.down_soft).child(paint(Ic::X, 11., 3., t.down))),
        StepSt::Ask => ring(t.warn, d.bg(t.warn_soft).child(paint(Ic::Comment, 11., 2.6, t.warn))),
        StepSt::Wait => d.border_1().border_color(t.accent).child(paint(Ic::Clock, 12., 2.6, t.accent_fg)).into_any_element(),
        StepSt::Now => d.child(paint(Ic::Spinner, 22., 2., t.accent)).into_any_element(),
        StepSt::Todo => d.border_1().border_color(t.border_2).into_any_element(),
    }
}

#[cfg(test)]
mod tests;
