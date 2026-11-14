//! The Ideas page (`GET /ideas?project=all&state=all&first=questions`, a page at a time): the ideas
//! as rows with one pill each, under two tabs: Open, where the ones waiting on the owner (open
//! questions, or ready to plan) come first, and Closed (shelved and dropped). A row opens the
//! idea's page (`ui::idea`), where it's closed and reopened. New idea opens a dialog that adds one
//! (`POST /ideas`), its draft kept until it's added; notes go through `tb idea`.
use crate::app::MainWindow;
use crate::fmt::{self, arr, s};
use crate::theme::Theme;
use crate::ui::issue_panel as ip;
use crate::ui::releases::cut_at;
use crate::ui::kit::KeyOutcome;
use crate::ui::modals::{self, Modal};
use crate::ui::paged::Paged;
use crate::ui::text_input::FieldChanged;
use crate::ui::{idea, kit};
use chrono::{DateTime, NaiveDate, Utc};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};

const PAGE_X: f32 = 28.;
const PROJECT_FILTER: &str = "ideas-project";
const FORM_PROJECT: &str = "new-idea-project";
/// Ideas per page.
pub const PAGE: usize = 200;
/// Lines left below the scroll top before the next older page is asked for.
const AHEAD: usize = 60;

#[derive(Default)]
pub struct State {
    /// "all" or one project.
    pub project: String,
    /// The New idea dialog's fields while it's open.
    pub form: Option<Form>,
    /// The ideas fetched so far.
    pub held: Paged,
    /// (project, its open ideas) for the filter, from the board's count of every idea.
    pub projects: Vec<(String, usize)>,
    /// (project, its ideas under Open, under Closed) for the tabs.
    pub split: Vec<(String, usize, usize)>,
    /// The Closed tab is showing.
    pub closed: bool,
    /// The page as last drawn, rebuilt when the ideas or the filter change.
    view: Option<View>,
    list: Option<ListState>,
}

/// What the page's groups are built from.
#[derive(Clone, PartialEq)]
struct Key {
    version: u64,
    picked: String,
    closed: bool,
    projects: Vec<(String, usize)>,
    split: Vec<(String, usize, usize)>,
    day: NaiveDate,
}

struct View {
    key: Key,
    groups: Vec<(String, Vec<Row>)>,
    more: bool,
    lines: Vec<Line>,
}

/// One item of the page's list.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    /// The title and New idea, the tabs and the filter.
    Top,
    /// "No ideas" (or "No closed ideas"), or "Loading…" while older pages are still to come.
    Empty,
    /// A section's name and count: "Waiting on you 3".
    Group(usize),
    /// A section's row.
    Row(usize, usize),
    End,
}

/// What every page of `GET /ideas` asks for.
pub fn query() -> Vec<(&'static str, String)> {
    vec![("project", "all".into()), ("state", "all".into()), ("first", "questions".into())]
}

/// The New idea dialog's draft: kept when the dialog is put away, until the idea is added.
pub struct Form {
    pub title: kit::Input,
    pub detail: kit::Input,
    /// Empty until one is chosen.
    pub project: String,
    /// "Say what the idea is.", under the title until it changes.
    pub title_err: Option<String>,
    /// "Choose the project it’s for.", under the project until one is picked.
    pub project_err: Option<String>,
    /// The board's refusal, over the buttons until anything changes.
    pub error: Option<String>,
    pub busy: bool,
    /// Give the title field focus on the next draw (after a failed add).
    refocus: bool,
    /// The dialog has been drawn since it last opened, so its leaving can be noticed.
    shown: bool,
}

impl State {
    pub fn project(&self) -> &str {
        if self.project.is_empty() { "all" } else { &self.project }
    }
}

// ================================================================== view models

/// A report's verdict as a pill: (label, tone).
pub fn verdict_pill(v: &Value) -> Option<(&'static str, &'static str)> {
    match v.as_str()? {
        "fits" => Some(("Fits", "up")),
        "partly" => Some(("Partly fits", "muted")),
        "doesnt_fit" => Some(("Doesn’t fit", "muted")),
        _ => None,
    }
}

/// An idea's state as a pill: (label, tone). A plan running shows over the state.
pub fn state_pill(i: &Value) -> (&'static str, &'static str) {
    if fmt::b(i, "planning") {
        return ("Planning", "accent");
    }
    match s(i, "state") {
        "digging" => ("Digging", "accent"),
        "reported" => ("Reported", "results"),
        "planned" => ("Planned", "goal"),
        "shelved" => ("Shelved", "muted"),
        "dropped" => ("Won’t do", "muted"),
        _ => ("New", "muted"),
    }
}

/// Open ideas first, then planned, shelved and dropped.
fn rank(state: &str) -> u8 {
    match state {
        "planned" => 1,
        "shelved" => 2,
        "dropped" => 3,
        _ => 0,
    }
}

/// Not yet planned, shelved or dropped: still changing.
pub fn is_open(i: &Value) -> bool {
    rank(s(i, "state")) == 0
}

/// Shelved or dropped: shown only under Closed.
pub fn is_closed(i: &Value) -> bool {
    rank(s(i, "state")) >= 2
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub r: String,
    pub title: String,
    pub project: String,
    /// Its one pill: its state, or its newest report's verdict once reported.
    pub pill: (&'static str, &'static str),
    /// What it waits on the owner for: "2 questions", or "Ready to plan".
    pub note: Option<String>,
    /// When it last changed.
    pub when: String,
}

/// The list's one pill: a plan or dig running, else planned or closed, else the newest report's
/// verdict, else its state.
pub fn row_pill(i: &Value) -> (&'static str, &'static str) {
    let state = state_pill(i);
    if fmt::b(i, "planning") || s(i, "state") != "reported" {
        return state;
    }
    verdict_pill(&i["verdict"]).unwrap_or(state)
}

/// Its open questions, from the board's count.
pub fn questions(i: &Value) -> u64 {
    i["questions"].as_u64().unwrap_or(0)
}

/// What an open idea waits on the owner for: its open questions, else a report that says it fits
/// and asks nothing. Nothing while a dig or plan runs.
pub fn waiting_note(i: &Value) -> Option<String> {
    if !is_open(i) || fmt::b(i, "planning") || s(i, "state") == "digging" {
        return None;
    }
    match questions(i) {
        0 if s(i, "state") == "reported" && s(i, "verdict") == "fits" => Some("Ready to plan".into()),
        0 => None,
        n => Some(fmt::plural(n as i64, "question", "questions")),
    }
}

/// The rows' sections: under Open, "Waiting on you" (open questions first, then ready to plan) over
/// "Other ideas" (open first, then planned), each newest change first; under Closed, one section
/// with no name. A section with no rows is left out. Only the picked project's rows when it has any.
pub fn groups(list: &Value, picked: &str, closed: bool, now: DateTime<Utc>) -> Vec<(String, Vec<Row>)> {
    groups_of(&arr(list, "ideas").iter().filter(|i| is_closed(i) == closed).collect::<Vec<_>>(), picked, closed, now)
}

fn groups_of(ideas: &[&Value], picked: &str, closed: bool, now: DateTime<Utc>) -> Vec<(String, Vec<Row>)> {
    let any_picked = ideas.iter().any(|i| s(i, "project") == picked);
    let mut mine: Vec<&Value> = ideas.iter().copied().filter(|i| !any_picked || s(i, "project") == picked).collect();
    mine.sort_by(|a, b| {
        let key = |i: &Value| (waiting_note(i).is_none(), questions(i) == 0, rank(s(i, "state")));
        key(a).cmp(&key(b)).then_with(|| s(b, "updated_at").cmp(s(a, "updated_at")))
    });
    let row = |i: &Value| Row {
        r: fmt::ref_of(i, "I"),
        title: s(i, "title").to_string(),
        project: s(i, "project").to_string(),
        pill: row_pill(i),
        note: waiting_note(i),
        when: fmt::parse(s(i, "updated_at")).map(|t| cut_at(t, now)).unwrap_or_default(),
    };
    if closed {
        let rows: Vec<Row> = mine.into_iter().map(row).collect();
        return if rows.is_empty() { vec![] } else { vec![(String::new(), rows)] };
    }
    let (waiting, other): (Vec<Row>, Vec<Row>) = mine.into_iter().map(row).partition(|r| r.note.is_some());
    [("Waiting on you", waiting), ("Other ideas", other)].into_iter().filter(|(_, rows)| !rows.is_empty()).map(|(n, rows)| (n.to_string(), rows)).collect()
}

/// (project, its open ideas) for the filter: the board's `projects` when it sends them, else
/// counted from the ideas.
pub fn tabs(list: &Value) -> Vec<(String, usize)> {
    if let Some(ps) = list["projects"].as_array() {
        return ps.iter().map(|p| (s(p, "project").to_string(), p["open"].as_u64().unwrap_or(0) as usize)).collect();
    }
    let mut v: Vec<(String, usize)> = vec![];
    for i in arr(list, "ideas") {
        let p = s(i, "project");
        let n = usize::from(is_open(i));
        match v.iter_mut().find(|(q, _)| q == p) {
            Some((_, c)) => *c += n,
            None => v.push((p.to_string(), n)),
        }
    }
    v.sort();
    v
}

/// (project, its ideas under Open, under Closed): the board's `projects` when it sends them, else
/// counted from the ideas.
pub fn split(list: &Value) -> Vec<(String, usize, usize)> {
    let n = |p: &Value, k: &str| p[k].as_u64().unwrap_or(0) as usize;
    if let Some(ps) = list["projects"].as_array().filter(|ps| ps.iter().all(|p| p["total"].is_u64())) {
        return ps.iter().map(|p| (s(p, "project").to_string(), n(p, "total").saturating_sub(n(p, "closed")), n(p, "closed"))).collect();
    }
    let mut v: Vec<(String, usize, usize)> = vec![];
    for i in arr(list, "ideas") {
        let p = s(i, "project");
        let (open, closed) = if is_closed(i) { (0, 1) } else { (1, 0) };
        match v.iter_mut().find(|(q, ..)| q == p) {
            Some((_, o, c)) => (*o, *c) = (*o + open, *c + closed),
            None => v.push((p.to_string(), open, closed)),
        }
    }
    v.sort();
    v
}

/// The ideas under (Open, Closed) for the picked project, or every one's.
fn tab_counts(split: &[(String, usize, usize)], picked: &str) -> (usize, usize) {
    split.iter().filter(|(p, ..)| picked == "all" || p == picked).fold((0, 0), |(o, c), (_, po, pc)| (o + po, c + pc))
}

// ================================================================== new idea

/// Open the New idea dialog: the draft left earlier when there is one, else empty on the project
/// the page shows (none when it shows them all).
pub fn open_form(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    new_form(m, cx);
    m.set_modal(Some(Modal::NewIdea), window, cx);
    if let Some(f) = &m.ideas.form {
        let to = f.title.focus.clone();
        window.focus(&to, cx);
    }
}

/// The draft typed before an update restart, put back unopened: ⌘N shows it.
pub fn restore_draft(m: &mut MainWindow, title: &str, detail: &str, project: &str, cx: &mut Context<MainWindow>) {
    new_form(m, cx);
    if let Some(f) = &mut m.ideas.form {
        f.title.set_text(title, cx);
        f.detail.set_text(detail, cx);
        if !project.is_empty() {
            f.project = project.to_string();
        }
    }
}

/// The idea draft's words and project, while it holds anything typed.
pub fn draft(m: &MainWindow, cx: &App) -> Option<(String, String, String)> {
    let f = m.ideas.form.as_ref().filter(|f| has_text(f, cx))?;
    Some((f.title.text(cx), f.detail.text(cx), f.project.clone()))
}

fn new_form(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    if m.ideas.form.is_none() {
        let project = match m.ideas.project() {
            "all" => String::new(),
            p => p.to_string(),
        };
        let title = kit::Input::new(cx, "", false);
        let detail = kit::Input::new(cx, "", true);
        cx.subscribe(&title.field, |m: &mut MainWindow, _, _: &FieldChanged, cx| {
            if let Some(f) = m.ideas.form.as_mut().filter(|f| f.title_err.is_some() || f.error.is_some()) {
                (f.title_err, f.error) = (None, None);
                cx.notify();
            }
        })
        .detach();
        cx.subscribe(&detail.field, |m: &mut MainWindow, _, _: &FieldChanged, cx| {
            if let Some(f) = m.ideas.form.as_mut().filter(|f| f.error.is_some()) {
                f.error = None;
                cx.notify();
            }
        })
        .detach();
        m.ideas.form = Some(Form { title, detail, project, title_err: None, project_err: None, error: None, busy: false, refocus: false, shown: false });
    }
}

/// Whether the draft holds anything typed or picked.
fn has_text(f: &Form, cx: &App) -> bool {
    !f.title.text(cx).trim().is_empty() || !f.detail.text(cx).trim().is_empty()
}

/// Put the dialog away, keeping what's typed.
fn close_form(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    m.set_modal(None, window, cx);
    draft_left(m, cx);
}

/// Once the dialog is gone (Cancel, esc, or another dialog taking its place), say a typed draft is
/// kept for next time; an empty one is dropped. Called from every page's draw.
pub fn draft_left(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    if matches!(m.modal, Some(Modal::NewIdea)) {
        return;
    }
    let Some(f) = m.ideas.form.as_mut() else { return };
    if !std::mem::take(&mut f.shown) || f.busy {
        return;
    }
    if has_text(f, cx) {
        m.toast("Idea draft kept · Reopen ⌘N", false, cx);
    } else {
        m.ideas.form = None;
    }
}

/// Add the idea, then open its page. Each missing field says so under itself; a refusal shows
/// over the buttons.
fn submit(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    let Some(f) = m.ideas.form.as_mut() else { return };
    if f.busy {
        return;
    }
    let title = f.title.text(cx).trim().to_string();
    f.title_err = title.is_empty().then(|| "Say what the idea is.".to_string());
    f.project_err = f.project.is_empty().then(|| "Choose the project it’s for.".to_string());
    if f.title_err.is_some() || f.project_err.is_some() {
        if f.title_err.is_some() {
            f.refocus = true;
        }
        cx.notify();
        return;
    }
    let body = json!({"title": title, "detail": f.detail.text(cx).trim(), "project": f.project});
    f.busy = true;
    f.error = None;
    window.focus(&m.focus, cx);
    m.post_or(
        "ideas",
        body,
        cx,
        |m, v, cx| {
            m.ideas.form = None;
            m.modal = None;
            m.toast(format!("Kept {}", s(&v, "ref")), false, cx);
            idea::open(m, s(&v, "ref"), cx);
        },
        |m, e, cx| {
            if let Some(f) = m.ideas.form.as_mut() {
                f.busy = false;
                f.error = Some(e.message);
                f.refocus = true;
            }
            cx.notify();
        },
    );
    cx.notify();
}

/// ↩ in the title (⌘↩ in the detail) adds it; esc puts the dialog away with the draft kept. ⇥
/// walks the fields and buttons in order.
fn on_key(m: &mut MainWindow, ev: &KeyDownEvent, in_detail: bool, window: &mut Window, cx: &mut Context<MainWindow>) {
    let Some(f) = m.ideas.form.as_ref() else { return };
    let here = if in_detail { &f.detail } else { &f.title };
    match here.on_key(ev, cx) {
        KeyOutcome::Submit => {
            cx.stop_propagation();
            submit(m, window, cx);
        }
        KeyOutcome::Cancel => {
            cx.stop_propagation();
            close_form(m, window, cx);
        }
        KeyOutcome::Ignored => {}
    }
}

/// A field's name over it.
fn field_label(t: &Theme, text: &'static str) -> Div {
    div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(t.muted).child(text)
}

/// What's wrong with a field, under it.
fn field_error(t: &Theme, e: &Option<String>) -> Option<Div> {
    e.clone().map(|e| div().text_size(px(12.5)).text_color(t.down).child(e))
}

/// A shortcut's keys, quiet beside a button's label.
pub fn keys(t: &Theme, k: &'static str) -> Div {
    div().px(px(5.)).rounded(px(4.)).bg(t.seg).text_color(t.muted).font_family(t.mono_font.clone()).text_size(px(10.5)).child(k)
}

/// The New idea dialog: the title, the project it's for and the detail, in that order.
pub fn form(m: &mut MainWindow, t: &Theme, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let mut projects = crate::app::project_names(m.state(), m.data.goals.iter());
    for (p, _) in &m.ideas.projects {
        if !projects.contains(p) {
            projects.push(p.clone());
        }
    }
    projects.sort_by(|a, b| modals::locale_cmp(a, b));
    let Some(f) = m.ideas.form.as_mut() else { return div().into_any_element() };
    f.shown = true;
    if std::mem::take(&mut f.refocus) {
        window.focus(&f.title.focus, cx);
    }
    let title = f.title.render(t, "new-idea-title", window).on_key_down(cx.listener(|m, ev: &KeyDownEvent, window, cx| on_key(m, ev, false, window, cx)));
    // Only the detail scrolls once it outgrows its box; the dialog keeps its size.
    let detail = f
        .detail
        .render(t, "new-idea-detail", window)
        .items_start()
        .flex_none()
        .min_h(px(120.))
        .max_h(px(220.))
        .overflow_y_scroll()
        .on_key_down(cx.listener(|m, ev: &KeyDownEvent, window, cx| on_key(m, ev, true, window, cx)));
    let cur = f.project.clone();
    let label = if cur.is_empty() { "Pick a project".to_string() } else { cur.clone() };
    let project = ip::picker_button(t, "new-idea-project-btn", label, cur.is_empty()).child(kit::anchor(FORM_PROJECT)).on_click(cx.listener(move |m, e: &ClickEvent, window, cx| {
        let p = e.position();
        ip::open_project_choice(m, FORM_PROJECT, &cur, &projects, point(p.x - px(10.), p.y + px(14.)), window, cx, |m, p, cx| {
            if let Some(f) = m.ideas.form.as_mut() {
                f.project = p;
                f.project_err = None;
                f.error = None;
            }
            cx.notify();
        });
    }));
    let add = if f.busy { kit::btn_primary(t, "new-idea-add", "Adding…").opacity(0.6) } else {
        kit::btn_primary(t, "new-idea-add", "Add idea").child(keys(t, "⌘↩")).on_click(cx.listener(|m, _, window, cx| submit(m, window, cx)))
    };
    let cancel = kit::btn(t, "new-idea-cancel", "Cancel").on_click(cx.listener(|m, _, window, cx| close_form(m, window, cx)));
    let field = |label: Div, input: AnyElement, err: Option<Div>| div().flex().flex_none().flex_col().gap(px(6.)).child(label).child(input).children(err);
    let body = div()
        .flex()
        .flex_none()
        .flex_col()
        .gap(px(14.))
        .px(px(20.))
        .pb(px(20.))
        .child(field(field_label(t, "Title"), title.into_any_element(), field_error(t, &f.title_err)))
        .child(field(field_label(t, "Project"), div().flex().child(project).into_any_element(), field_error(t, &f.project_err)))
        .child(field(field_label(t, "Detail (optional)"), detail.into_any_element(), None))
        .children(field_error(t, &f.error))
        .child(div().flex().flex_none().items_center().justify_end().gap(px(8.)).pt(px(4.)).child(cancel).child(add));
    // Capped by the window: the box's own cap is a share of the dialog's height, which is its own.
    kit::modal_box(t, 560.)
        .max_h(window.viewport_size().height * 0.9)
        .child(div().px(px(20.)).pt(px(18.)).pb(px(14.)).text_size(px(16.)).font_weight(FontWeight::BOLD).child("New idea"))
        .child(body)
        .into_any_element()
}

// ================================================================== render

/// The list's fixed columns: ref, (title), project, pill, note, time.
const COL_REF: f32 = 56.;
const COL_PROJECT: f32 = 120.;
const COL_PILL: f32 = 112.;
const COL_NOTE: f32 = 120.;
const COL_WHEN: f32 = 150.;

fn row(t: &Theme, row: &Row, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let r = row.r.clone();
    let hover = t.panel_2;
    let cell = |w: f32| div().flex_none().w(px(w)).min_w_0().overflow_hidden();
    div()
        .id(SharedString::from(format!("ideas-row-{}", row.r)))
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(12.))
        .py(px(11.))
        .border_b_1()
        .border_color(t.divider)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(cell(COL_REF).text_size(px(12.)).font_family(t.mono_font.clone()).text_color(t.muted).child(row.r.clone()))
        .child(div().min_w_0().flex_1().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).truncate().child(row.title.clone()))
        .child(cell(COL_PROJECT).text_size(px(12.5)).text_color(t.muted).truncate().child(row.project.clone()))
        .child(cell(COL_PILL).child(kit::tone_pill(t, row.pill.1, row.pill.0)))
        .child(cell(COL_NOTE).text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(t.warn_fg).truncate().children(row.note.clone()))
        .child(cell(COL_WHEN).whitespace_nowrap().text_size(px(12.)).text_color(t.muted).text_right().child(row.when.clone()))
        .on_click(cx.listener(move |m, _, _, cx| idea::open(m, &r, cx)))
}

/// Bring the groups and the list's lines up to date, rebuilding only what changed.
fn sync(m: &mut MainWindow) {
    let s = &mut m.ideas;
    let picked = if s.projects.iter().any(|(p, _)| p == s.project()) { s.project().to_string() } else { "all".to_string() };
    let now = fmt::now();
    let key = Key {
        version: s.held.version,
        picked,
        closed: s.closed,
        projects: s.projects.clone(),
        split: s.split.clone(),
        day: fmt::local(now).date_naive(),
    };
    let more = s.held.next.is_some();
    let old = s.view.take();
    let filter_same = old.as_ref().is_some_and(|v| v.key.picked == key.picked && v.key.closed == key.closed);
    let mut v = match old {
        Some(v) if v.key == key => v,
        _ => {
            let mine: Vec<&Value> = s
                .held
                .rows
                .iter()
                .filter(|i| (key.picked == "all" || fmt::s(i, "project") == key.picked) && is_closed(i) == key.closed)
                .collect();
            let groups = groups_of(&mine, &key.picked, key.closed, now);
            View { key, groups, more, lines: vec![] }
        }
    };
    let mut lines = vec![Line::Top];
    if v.groups.is_empty() {
        lines.push(Line::Empty);
    }
    for (gi, (name, rows)) in v.groups.iter().enumerate() {
        if !name.is_empty() {
            lines.push(Line::Group(gi));
        }
        lines.extend((0..rows.len()).map(|ri| Line::Row(gi, ri)));
    }
    lines.push(Line::End);
    let changed = lines != v.lines || v.more != more;
    let count = lines.len();
    match &s.list {
        None => s.list = Some(ListState::new(count, ListAlignment::Top, px(600.))),
        Some(l) if changed || !filter_same => {
            let top = l.logical_scroll_top();
            l.reset(count);
            // New ideas keep the place scrolled to; another project or tab starts at the top.
            if filter_same {
                l.scroll_to(ListOffset { item_ix: top.item_ix.min(count - 1), offset_in_item: top.offset_in_item });
            }
        }
        // The filter's and tabs' counts can change with the lines staying put.
        Some(l) => l.remeasure_items(0..1),
    }
    v.lines = lines;
    v.more = more;
    s.view = Some(v);
}

/// Fetch the next older page once the list nears its end (a short list is always near it).
fn load_older(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let s = &mut m.ideas;
    let (Some(before), Some(l), Some(v)) = (s.held.next.clone(), &s.list, &s.view) else { return };
    if s.held.loading || v.lines.len() > l.logical_scroll_top().item_ix + AHEAD {
        return;
    }
    s.held.loading = true;
    let backend = m.backend.clone();
    let mut query = query();
    query.extend([("limit", PAGE.to_string()), ("before", before.clone())]);
    cx.spawn(async move |this, cx| {
        let got = cx.background_executor().spawn(async move { backend.get("ideas", &query).map_err(|e| e.message) }).await;
        let _ = this.update(cx, |m, cx| {
            m.ideas.held.land_older("ideas", &before, got);
            cx.notify();
        });
    })
    .detach();
}

/// The list's first line: the title and New idea, then the Open / Closed tabs and the project
/// filter.
fn top(m: &MainWindow, t: &Theme, picked: &str, cx: &mut Context<MainWindow>) -> Div {
    let tabs = &m.ideas.projects;
    let (open, closed) = tab_counts(&m.ideas.split, picked);
    let labels = [format!("Open {open}"), format!("Closed {closed}")];
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    let which = kit::seg(t, "ideas-tab", &labels, m.ideas.closed as usize, |i, item| {
        item.on_click(cx.listener(move |m, _, _, cx| {
            m.ideas.closed = i == 1;
            cx.notify();
        }))
    });
    let new = kit::btn_primary(t, "ideas-new", "New idea").child(keys(t, "⌘N")).on_click(cx.listener(|m, _, window, cx| open_form(m, window, cx)));
    div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(div().flex().items_center().gap(px(16.)).child(div().flex_1().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Ideas")).child(new))
        .child(
            div().flex().items_center().gap(px(16.)).child(which).child(div().flex_1()).when(tabs.len() > 1, |d| {
                d.child(ip::project_filter(t, PROJECT_FILTER, picked, tabs, cx, |m, p, cx| {
                    m.ideas.project = p;
                    cx.notify();
                }))
            }),
        )
}

/// One line of the page's list.
fn line_item(m: &mut MainWindow, ix: usize, _: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let Some(v) = m.ideas.view.take() else { return div().into_any_element() };
    let line = div().px(px(PAGE_X)).line_height(relative(1.4));
    let el = match v.lines.get(ix).copied() {
        Some(Line::Top) => line.pt(px(22.)).pb(px(12.)).child(top(m, &t, &v.key.picked, cx)),
        Some(Line::Empty) => {
            let none = if v.key.closed { "No closed ideas" } else { "No ideas" };
            line.child(kit::empty(&t, if v.more { "Loading…" } else { none }))
        }
        Some(Line::Group(gi)) => {
            let (name, rows) = &v.groups[gi];
            line.pt(px(18.)).child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(8.))
                    .px(px(12.))
                    .pb(px(8.))
                    .border_b_1()
                    .border_color(t.divider)
                    .child(div().text_size(px(13.)).font_weight(FontWeight::BOLD).child(name.clone()))
                    .child(div().text_size(px(12.5)).text_color(t.muted).child(rows.len().to_string())),
            )
        }
        Some(Line::Row(gi, ri)) => line.child(row(&t, &v.groups[gi].1[ri], cx)),
        Some(Line::End) | None => line.h(px(32.)),
    };
    m.ideas.view = Some(v);
    el.into_any_element()
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    draft_left(m, cx);
    let t = cx.global::<Theme>().clone();
    let filter = ip::render_project_filter(m, &t, PROJECT_FILTER, window, cx);
    // The dialog's project list: a popover, drawn over the dialog.
    let form_project = ip::render_project_filter(m, &t, FORM_PROJECT, window, cx);
    let shell = div().id("ideas-page").relative().flex().flex_col().flex_1().min_w_0().h_full().children(filter).children(form_project);

    if !m.ideas.held.loaded {
        let page = div().flex().flex_col().gap(px(20.)).px(px(PAGE_X)).pt(px(22.)).pb(px(32.)).line_height(relative(1.4)).child(top(m, &t, "all", cx));
        let state = match m.data.errs.ideas.clone() {
            Some(e) => div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(10.))
                .py(px(40.))
                .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child("Ideas didn’t load"))
                .child(div().text_size(px(12.5)).text_color(t.muted).child(e))
                .child(kit::btn(&t, "ideas-retry", "Try again").on_click(cx.listener(|m, _, _, cx| m.refresh(cx)))),
            None => kit::empty(&t, "Loading…"),
        };
        return shell.child(page.child(state)).into_any_element();
    }
    sync(m);
    load_older(m, cx);
    let Some(state) = m.ideas.list.clone() else { return shell.into_any_element() };
    shell.child(list(state, cx.processor(line_item)).flex_1().min_h_0()).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // Beats the glob's `gpui_kit::test`.
    use ::core::prelude::v1::test;
    use crate::app::Page;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
        fmt::local(Utc::now()).offset().with_ymd_and_hms(y, mo, d, h, 5, 0).unwrap().with_timezone(&Utc)
    }

    fn idea(id: i64, project: &str, state: &str, verdict: Value, updated: DateTime<Utc>) -> Value {
        json!({"id": id, "ref": format!("I{id}"), "project": project, "title": format!("Idea {id}"), "state": state,
               "verdict": verdict, "planning": false, "updated_at": updated.to_rfc3339()})
    }

    #[test]
    fn ideas_waiting_on_the_owner_come_first_then_the_rest_and_closed_ones_apart() {
        let now = at(2026, 10, 15, 16);
        let asks = |mut i: Value, n: u64| {
            i["questions"] = json!(n);
            i
        };
        let list = json!({"ideas": [
            idea(1, "web", "dropped", json!(null), at(2026, 10, 15, 15)),
            idea(2, "web", "planned", json!("fits"), at(2026, 10, 14, 9)),
            idea(3, "web", "new", json!(null), at(2026, 10, 7, 9)),
            asks(idea(4, "web", "reported", json!("partly"), at(2026, 10, 15, 11)), 2),
            idea(5, "web", "shelved", json!("doesnt_fit"), at(2025, 12, 1, 9)),
            idea(6, "web", "reported", json!("fits"), at(2026, 10, 15, 12)),
            idea(7, "web", "digging", json!(null), at(2026, 10, 15, 13)),
        ]});
        let rows = |g: &(String, Vec<Row>)| g.1.iter().map(|r| (r.r.clone(), r.pill.0, r.note.clone(), r.when.clone())).collect::<Vec<_>>();
        let open = groups(&list, "all", false, now);
        assert_eq!(open.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>(), ["Waiting on you", "Other ideas"]);
        assert_eq!(
            rows(&open[0]),
            [
                ("I4".into(), "Partly fits", Some("2 questions".into()), "Today, 11:05 AM".into()),
                ("I6".into(), "Fits", Some("Ready to plan".into()), "Today, 12:05 PM".into()),
            ]
        );
        assert_eq!(
            rows(&open[1]),
            [
                ("I7".into(), "Digging", None, "Today, 1:05 PM".into()),
                ("I3".into(), "New", None, "Oct 7, 9:05 AM".into()),
                ("I2".into(), "Planned", None, "Oct 14, 9:05 AM".into()),
            ]
        );
        let closed = groups(&list, "all", true, now);
        assert_eq!(closed.len(), 1);
        assert_eq!(closed[0].0, "", "closed ideas sit in one unnamed section");
        assert_eq!(
            rows(&closed[0]),
            [("I5".into(), "Shelved", None, "Dec 1, 2025, 9:05 AM".into()), ("I1".into(), "Won’t do", None, "Today, 3:05 PM".into())]
        );
    }

    #[test]
    fn a_row_has_one_pill_and_only_open_ideas_wait_on_the_owner() {
        let i = |state: &str, verdict: Value, planning: bool, questions: u64| json!({"state": state, "verdict": verdict, "planning": planning, "questions": questions});
        assert_eq!(row_pill(&i("reported", json!("fits"), false, 0)), ("Fits", "up"));
        assert_eq!(row_pill(&i("reported", json!("doesnt_fit"), false, 0)), ("Doesn’t fit", "muted"), "grey: nothing failed");
        assert_eq!(row_pill(&i("reported", json!("fits"), true, 0)), ("Planning", "accent"));
        assert_eq!(row_pill(&i("planned", json!("fits"), false, 0)), ("Planned", "goal"));
        assert_eq!(row_pill(&i("new", json!(null), false, 0)), ("New", "muted"));
        assert_eq!(waiting_note(&i("reported", json!("doesnt_fit"), false, 1)).as_deref(), Some("1 question"));
        assert_eq!(waiting_note(&i("reported", json!("partly"), false, 0)), None);
        assert_eq!(waiting_note(&i("reported", json!("fits"), true, 2)), None, "not while it's planning");
        assert_eq!(waiting_note(&i("digging", json!("fits"), false, 2)), None, "not while it's digging");
        assert_eq!(waiting_note(&i("shelved", json!("fits"), false, 0)), None);
    }

    #[test]
    fn the_tabs_count_open_and_closed_ideas_from_the_board_or_the_ideas() {
        let now = at(2026, 10, 15, 16);
        let ideas = json!([
            idea(1, "web", "new", json!(null), now),
            idea(2, "web", "planned", json!(null), now),
            idea(3, "web", "shelved", json!(null), now),
            idea(4, "api", "dropped", json!(null), now),
        ]);
        let counted = split(&json!({"ideas": ideas}));
        assert_eq!(counted, [("api".to_string(), 0, 1), ("web".to_string(), 2, 1)]);
        // The board counts every idea, not only the page held.
        let board = json!({"ideas": ideas, "projects": [
            {"project": "api", "open": 3, "closed": 4, "total": 9},
            {"project": "web", "open": 1, "closed": 1, "total": 3},
        ]});
        assert_eq!(split(&board), [("api".to_string(), 5, 4), ("web".to_string(), 2, 1)]);
        assert_eq!(tab_counts(&split(&board), "all"), (7, 5));
        assert_eq!(tab_counts(&split(&board), "web"), (2, 1));
        // A board without totals falls back to counting.
        let old = json!({"ideas": ideas, "projects": [{"project": "web", "open": 1}]});
        assert_eq!(split(&old), counted);
    }

    #[gpui_kit::test]
    fn shelved_and_dropped_ideas_show_only_under_closed(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = crate::parity::window(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| m.open_link("ideas", cx)).unwrap();
        crate::parity::settle(cx);
        redraw(w, cx);
        let shown = |cx: &mut gpui_kit::TestAppContext| {
            w.update(cx, |m, _, _| {
                let v = m.ideas.view.as_ref().unwrap();
                let rows: Vec<&'static str> = v.lines.iter().filter_map(|l| if let Line::Row(g, r) = l { Some(v.groups[*g].1[*r].pill.0) } else { None }).collect();
                (rows, tab_counts(&m.ideas.split, "all"))
            })
            .unwrap()
        };
        let (open, (n_open, n_closed)) = shown(cx);
        assert!(!open.is_empty() && open.iter().all(|s| !["Shelved", "Won’t do"].contains(s)), "{open:?}");
        assert_eq!(open.len(), n_open);
        w.update(cx, |m, _, cx| {
            m.ideas.closed = true;
            cx.notify();
        })
        .unwrap();
        redraw(w, cx);
        let (closed, _) = shown(cx);
        assert_eq!(closed, ["Shelved"], "the seeded shelved idea");
        assert_eq!(closed.len(), n_closed);
    }

    #[test]
    fn a_picked_project_shows_only_its_ideas_and_the_filter_counts_open_ones() {
        let now = at(2026, 10, 15, 16);
        let list = json!({"ideas": [
            idea(1, "web", "new", json!(null), now),
            idea(2, "api", "shelved", json!(null), now),
            idea(3, "web", "planned", json!(null), now),
            idea(4, "api", "digging", json!(null), now),
        ]});
        let projects = |picked: &str| {
            let mut v: Vec<String> = groups(&list, picked, false, now).into_iter().flat_map(|(_, rows)| rows.into_iter().map(|r| r.project)).collect();
            v.sort();
            v.dedup();
            v
        };
        assert_eq!(projects("all"), ["api", "web"]);
        assert_eq!(projects("web"), ["web"]);
        // A project without ideas falls back to every one.
        assert_eq!(projects("kass"), ["api", "web"]);
        assert_eq!(tabs(&list), [("api".to_string(), 1), ("web".to_string(), 1)]);
    }

    fn redraw(w: gpui_kit::WindowHandle<MainWindow>, cx: &mut gpui_kit::TestAppContext) {
        cx.update_window(w.into(), |_, window, cx| {
            window.refresh();
            window.draw(cx).clear(cx);
        })
        .unwrap();
        crate::parity::settle(cx);
    }

    #[gpui_kit::test]
    fn new_idea_needs_a_project_then_adds_it_and_opens_its_page(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = crate::parity::window(cx);
        w.update(cx, |m: &mut MainWindow, window, cx| {
            m.open_link("ideas", cx);
            open_form(m, window, cx);
            m.ideas.form.as_ref().unwrap().title.set_text("  Dark mode for the phone  ", cx);
            m.ideas.form.as_ref().unwrap().detail.set_text("Follow the system setting.", cx);
        })
        .unwrap();
        redraw(w, cx);
        cx.simulate_keystrokes(w.into(), "enter");
        crate::parity::settle(cx);
        w.update(cx, |m: &mut MainWindow, _, _| {
            let f = m.ideas.form.as_mut().unwrap();
            assert_eq!(f.project_err.as_deref(), Some("Choose the project it’s for."));
            assert_eq!(f.title_err, None, "the title is there");
            assert!(rec.last("ideas").is_none(), "nothing is sent without a project");
            f.project = "webapp".into();
        })
        .unwrap();
        redraw(w, cx);
        cx.simulate_keystrokes(w.into(), "enter");
        crate::parity::settle(cx);
        assert_eq!(rec.last("ideas").unwrap(), json!({"title": "Dark mode for the phone", "detail": "Follow the system setting.", "project": "webapp"}));
        w.update(cx, |m: &mut MainWindow, _, _| {
            assert!(m.modal.is_none() && m.ideas.form.is_none(), "the dialog closes");
            let Page::Idea(r) = &m.page else { panic!("on {:?}", m.page) };
            assert_eq!(m.data.idea.as_ref().unwrap()["title"], "Dark mode for the phone", "{r} opens");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn shift_enter_starts_a_new_line_in_the_detail_only(cx: &mut gpui_kit::TestAppContext) {
        use crate::ui::text_input::Newline;
        // The test platform types ↩ as "\n" on its own, so check the binding a real ⇧↩ goes through.
        let (w, rec) = crate::parity::window(cx);
        let newline_key = |field: fn(&Form) -> &kit::Input, cx: &mut gpui_kit::TestAppContext| {
            w.update(cx, |m: &mut MainWindow, window, _| {
                let focus = &field(m.ideas.form.as_ref().unwrap()).focus;
                window.bindings_for_action_in(&Newline, focus).last().map(|b| b.keystrokes().iter().map(|k| k.unparse()).collect::<Vec<_>>().join(" "))
            })
            .unwrap()
        };
        w.update(cx, |m: &mut MainWindow, window, cx| {
            m.open_link("ideas", cx);
            open_form(m, window, cx);
            m.ideas.form.as_ref().unwrap().detail.set_text("Follow the system setting.", cx);
        })
        .unwrap();
        redraw(w, cx);
        assert_eq!(newline_key(|f| &f.title, cx), None, "the title stays one line");
        assert_eq!(newline_key(|f| &f.detail, cx).as_deref(), Some("shift-enter"));
        w.update(cx, |m: &mut MainWindow, window, cx| {
            window.focus(&m.ideas.form.as_ref().unwrap().detail.focus, cx);
            window.dispatch_action(Box::new(Newline), cx);
        })
        .unwrap();
        crate::parity::settle(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            let f = m.ideas.form.as_ref().expect("the dialog stays open");
            assert_eq!(f.detail.text(cx), "Follow the system setting.\n");
            assert!(rec.last("ideas").is_none(), "nothing is sent");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn a_long_idea_list_fetches_older_pages_as_it_scrolls(cx: &mut gpui_kit::TestAppContext) {
        use crate::backend::Backend;
        use std::collections::HashSet;
        let (w, rec) = crate::parity::window(cx);
        for k in 0..320 {
            let project = if k % 2 == 0 { "webapp" } else { "api" };
            rec.board().post("ideas", json!({"title": format!("Paged {k}"), "project": project})).unwrap();
        }
        w.update(cx, |m: &mut MainWindow, _, cx| m.open_link("ideas", cx)).unwrap();
        crate::parity::settle(cx);
        redraw(w, cx);
        let held = |cx: &mut gpui_kit::TestAppContext| w.update(cx, |m, _, _| (m.ideas.held.rows.len(), m.ideas.held.next.is_some(), m.ideas.held.total)).unwrap();
        let (n, more, total) = held(cx);
        assert_eq!((n, more), (PAGE, true), "only the first page until the list nears its end");
        assert!(total > 320, "the board counts every idea: {total}");

        for _ in 0..5 {
            w.update(cx, |m, _, _| {
                let n = m.ideas.view.as_ref().unwrap().lines.len();
                m.ideas.list.as_ref().unwrap().scroll_to(ListOffset { item_ix: n, offset_in_item: px(0.) });
            })
            .unwrap();
            redraw(w, cx);
        }
        assert_eq!(held(cx), (total, false, total), "scrolling to the end pages back through the rest");
        let ids: HashSet<i64> = w.update(cx, |m, _, _| m.ideas.held.rows.iter().map(|i| i["id"].as_i64().unwrap()).collect()).unwrap();
        assert_eq!(ids.len(), total, "each idea once");
        let (rows, open) = w
            .update(cx, |m, _, _| {
                let rows = m.ideas.view.as_ref().unwrap().lines.iter().filter(|l| matches!(l, Line::Row(..))).count();
                (rows, m.ideas.held.rows.iter().filter(|i| !is_closed(i)).count())
            })
            .unwrap();
        assert_eq!(rows, open, "every open idea has a line");
        let webapp = w.update(cx, |m, _, _| m.ideas.projects.iter().find(|(p, _)| p == "webapp").map(|(_, n)| *n).unwrap()).unwrap();
        assert!(webapp >= 160, "the filter counts the board's open ideas, not the held ones: {webapp}");
    }

    #[gpui_kit::test]
    fn a_put_away_draft_is_kept_and_comes_back_and_an_empty_one_is_dropped(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = crate::parity::window(cx);
        w.update(cx, |m: &mut MainWindow, window, cx| {
            m.open_link("ideas", cx);
            open_form(m, window, cx);
            m.ideas.form.as_ref().unwrap().title.set_text("Dark mode", cx);
            m.ideas.form.as_mut().unwrap().project = "webapp".into();
        })
        .unwrap();
        redraw(w, cx);
        cx.simulate_keystrokes(w.into(), "escape");
        redraw(w, cx);
        w.update(cx, |m: &mut MainWindow, window, cx| {
            assert!(m.modal.is_none(), "esc puts the dialog away");
            assert!(m.toasts.iter().any(|t| t.text == "Idea draft kept · Reopen ⌘N"), "{:?}", m.toasts.iter().map(|t| &t.text).collect::<Vec<_>>());
            open_form(m, window, cx);
            let f = m.ideas.form.as_ref().unwrap();
            assert_eq!((f.title.text(cx), f.project.as_str()), ("Dark mode".to_string(), "webapp"), "the draft comes back");
        })
        .unwrap();
        redraw(w, cx);
        w.update(cx, |m: &mut MainWindow, window, cx| {
            m.ideas.form.as_ref().unwrap().title.set_text("", cx);
            close_form(m, window, cx);
        })
        .unwrap();
        redraw(w, cx);
        w.update(cx, |m: &mut MainWindow, _, _| assert!(m.ideas.form.is_none(), "nothing typed, nothing kept")).unwrap();
        assert!(rec.last("ideas").is_none(), "putting it away sends nothing");
    }

    #[gpui_kit::test]
    fn each_missing_field_says_so_under_itself_until_it_changes(cx: &mut gpui_kit::TestAppContext) {
        let (w, _rec) = crate::parity::window(cx);
        w.update(cx, |m: &mut MainWindow, window, cx| {
            m.open_link("ideas", cx);
            open_form(m, window, cx);
        })
        .unwrap();
        redraw(w, cx);
        cx.simulate_keystrokes(w.into(), "enter");
        crate::parity::settle(cx);
        w.update(cx, |m: &mut MainWindow, _, cx| {
            let f = m.ideas.form.as_ref().unwrap();
            assert_eq!(f.title_err.as_deref(), Some("Say what the idea is."));
            assert_eq!(f.project_err.as_deref(), Some("Choose the project it’s for."));
            f.title.set_text("D", cx);
        })
        .unwrap();
        crate::parity::settle(cx);
        w.update(cx, |m: &mut MainWindow, _, _| {
            let f = m.ideas.form.as_ref().unwrap();
            assert_eq!(f.title_err, None, "typing clears the title's error");
            assert!(f.project_err.is_some(), "the project's stays until one is picked");
        })
        .unwrap();
    }

    #[test]
    fn a_running_plan_shows_over_the_state() {
        assert_eq!(state_pill(&json!({"state": "reported", "planning": true})), ("Planning", "accent"));
        assert_eq!(state_pill(&json!({"state": "digging", "planning": false})), ("Digging", "accent"));
        assert_eq!(verdict_pill(&json!("maybe")), None);
    }
}
