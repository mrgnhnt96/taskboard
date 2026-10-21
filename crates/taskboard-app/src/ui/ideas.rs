//! The Ideas page (`GET /ideas?project=all&state=all`, a page at a time): each project's ideas as
//! rows, the open ones first, each with its newest report's verdict and its state. A row opens the
//! idea's page (`ui::idea`). New idea opens a dialog that adds one (`POST /ideas`); notes, shelving
//! and dropping go through `tb idea`.
use crate::app::MainWindow;
use crate::fmt::{self, arr, s};
use crate::theme::Theme;
use crate::ui::issue_panel as ip;
use crate::ui::releases::cut_at;
use crate::ui::kit::KeyOutcome;
use crate::ui::modals::{self, Modal};
use crate::ui::paged::Paged;
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
    /// The page as last drawn, rebuilt when the ideas or the filter change.
    view: Option<View>,
    list: Option<ListState>,
}

/// What the page's groups are built from.
#[derive(Clone, PartialEq)]
struct Key {
    version: u64,
    picked: String,
    projects: Vec<(String, usize)>,
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
    /// The title, the filter and New idea.
    Top,
    /// "No ideas", or "Loading…" while older pages are still to come.
    Empty,
    /// A project's name, over its card when there are several.
    Group(usize),
    /// A project's row: the first opens its card, the last closes it.
    Row(usize, usize),
    End,
}

/// What every page of `GET /ideas` asks for.
pub fn query() -> Vec<(&'static str, String)> {
    vec![("project", "all".into()), ("state", "all".into())]
}

pub struct Form {
    pub title: kit::Input,
    pub detail: kit::Input,
    /// Empty until one is chosen.
    pub project: String,
    pub error: Option<String>,
    pub busy: bool,
    /// Give the title field focus on the next draw (after a failed add).
    refocus: bool,
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
        "partly" => Some(("Partly fits", "warn")),
        "doesnt_fit" => Some(("Doesn’t fit", "down")),
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
        "dropped" => ("Dropped", "muted"),
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

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub r: String,
    pub title: String,
    pub project: String,
    pub verdict: Option<(&'static str, &'static str)>,
    pub state: (&'static str, &'static str),
    /// When it last changed.
    pub when: String,
    /// Shelved or dropped: shown faded.
    pub put_away: bool,
}

/// The projects shown and each one's rows: the picked project while it has ideas, else every one
/// with ideas, by name. Rows: open first, each group newest change first.
pub fn groups(list: &Value, picked: &str, now: DateTime<Utc>) -> Vec<(String, Vec<Row>)> {
    groups_of(&arr(list, "ideas").iter().collect::<Vec<_>>(), picked, now)
}

fn groups_of(ideas: &[&Value], picked: &str, now: DateTime<Utc>) -> Vec<(String, Vec<Row>)> {
    let mut projects: Vec<String> = ideas.iter().map(|i| s(i, "project").to_string()).collect();
    projects.sort();
    projects.dedup();
    if projects.iter().any(|p| p == picked) {
        projects.retain(|p| p == picked);
    }
    projects
        .into_iter()
        .map(|p| {
            let mut mine: Vec<&Value> = ideas.iter().copied().filter(|i| s(i, "project") == p).collect();
            mine.sort_by(|a, b| rank(s(a, "state")).cmp(&rank(s(b, "state"))).then_with(|| s(b, "updated_at").cmp(s(a, "updated_at"))));
            let rows = mine
                .into_iter()
                .map(|i| Row {
                    r: fmt::ref_of(i, "I"),
                    title: s(i, "title").to_string(),
                    project: p.clone(),
                    verdict: verdict_pill(&i["verdict"]),
                    state: state_pill(i),
                    when: fmt::parse(s(i, "updated_at")).map(|t| cut_at(t, now)).unwrap_or_default(),
                    put_away: rank(s(i, "state")) >= 2,
                })
                .collect();
            (p, rows)
        })
        .collect()
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

// ================================================================== new idea

/// Open the New idea dialog, on the project the page shows (none when it shows them all).
pub fn open_form(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    let project = match m.ideas.project() {
        "all" => String::new(),
        p => p.to_string(),
    };
    let title = kit::Input::new(cx, "Title", false);
    let detail = kit::Input::new(cx, "Detail", true);
    m.set_modal(Some(Modal::NewIdea), window, cx);
    window.focus(&title.focus, cx);
    m.ideas.form = Some(Form { title, detail, project, error: None, busy: false, refocus: false });
}

/// Add the idea, then open its page. A refusal shows in the dialog.
fn submit(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    let Some(f) = m.ideas.form.as_mut() else { return };
    if f.busy {
        return;
    }
    let title = f.title.text(cx).trim().to_string();
    let error = if title.is_empty() {
        Some("Say what the idea is.")
    } else if f.project.is_empty() {
        Some("Choose the project it’s for.")
    } else {
        None
    };
    if let Some(e) = error {
        f.error = Some(e.into());
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

/// ↩ in the title (⌘↩ in the detail) adds it, esc closes the dialog, ⇥ moves between the two.
fn on_key(m: &mut MainWindow, ev: &KeyDownEvent, in_detail: bool, window: &mut Window, cx: &mut Context<MainWindow>) {
    let Some(f) = m.ideas.form.as_ref() else { return };
    let (here, other) = if in_detail { (&f.detail, &f.title) } else { (&f.title, &f.detail) };
    match here.on_key(ev, cx) {
        KeyOutcome::Submit => {
            cx.stop_propagation();
            submit(m, window, cx);
        }
        KeyOutcome::Cancel => {
            cx.stop_propagation();
            m.set_modal(None, window, cx);
        }
        KeyOutcome::Ignored if ev.keystroke.key == "tab" => {
            cx.stop_propagation();
            let to = other.focus.clone();
            window.focus(&to, cx);
        }
        KeyOutcome::Ignored => {}
    }
}

/// The New idea dialog: title, detail, and the project it's for.
pub fn form(m: &mut MainWindow, t: &Theme, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let mut projects = crate::app::project_names(m.state(), &m.data.goals);
    for (p, _) in &m.ideas.projects {
        if !projects.contains(p) {
            projects.push(p.clone());
        }
    }
    projects.sort_by(|a, b| modals::locale_cmp(a, b));
    let Some(f) = m.ideas.form.as_mut() else { return div().into_any_element() };
    if std::mem::take(&mut f.refocus) {
        window.focus(&f.title.focus, cx);
    }
    let title = f.title.render(t, "new-idea-title", window).on_key_down(cx.listener(|m, ev: &KeyDownEvent, window, cx| on_key(m, ev, false, window, cx)));
    let detail = f
        .detail
        .render(t, "new-idea-detail", window)
        .items_start()
        .flex_none()
        .min_h(px(120.))
        .on_key_down(cx.listener(|m, ev: &KeyDownEvent, window, cx| on_key(m, ev, true, window, cx)));
    let cur = f.project.clone();
    let label = if cur.is_empty() { "Choose a project".to_string() } else { cur.clone() };
    let project = ip::picker_button(t, "new-idea-project-btn", label, cur.is_empty()).on_click(cx.listener(move |m, e: &ClickEvent, window, cx| {
        let p = e.position();
        ip::open_project_choice(m, FORM_PROJECT, &cur, &projects, point(p.x - px(10.), p.y + px(14.)), window, cx, |m, p, cx| {
            if let Some(f) = m.ideas.form.as_mut() {
                f.project = p;
                f.error = None;
            }
            cx.notify();
        });
    }));
    let add = if f.busy { kit::btn_primary(t, "new-idea-add", "Adding…").opacity(0.6) } else {
        kit::btn_primary(t, "new-idea-add", "Add idea").on_click(cx.listener(|m, _, window, cx| submit(m, window, cx)))
    };
    let error = f.error.clone().map(|e| div().text_size(px(12.5)).text_color(t.down).child(e));
    // Capped by the window: the box's own cap is a share of the dialog's height, which is its own.
    kit::modal_box(t, 560.)
        .max_h(window.viewport_size().height * 0.9)
        .child(modals::head(t, "New idea", cx))
        .child(
            div()
                .flex()
                .flex_none()
                .flex_col()
                .gap(px(10.))
                .px(px(20.))
                .pb(px(20.))
                .child(title)
                .child(detail)
                .children(error)
                .child(div().flex().flex_none().items_center().justify_between().gap(px(10.)).pt(px(4.)).child(project).child(add)),
        )
        .into_any_element()
}

// ================================================================== render

fn row(t: &Theme, row: &Row, first: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let r = row.r.clone();
    let hover = t.panel_2;
    div()
        .id(SharedString::from(format!("ideas-row-{}", row.r)))
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(20.))
        .py(px(12.))
        .when(!first, |d| d.border_t_1().border_color(t.divider))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .when(row.put_away, |d| d.opacity(0.6))
        .child(div().flex_none().w(px(36.)).text_size(px(12.)).font_family(t.mono_font.clone()).text_color(t.muted).child(row.r.clone()))
        .child(div().min_w_0().flex_1().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).truncate().child(row.title.clone()))
        .child(kit::chip(t, row.project.clone()))
        .children(row.verdict.map(|(label, tone)| kit::tone_pill(t, tone, label)))
        .child(kit::tone_pill(t, row.state.1, row.state.0))
        .child(div().flex_none().w(px(140.)).whitespace_nowrap().text_size(px(12.)).text_color(t.muted).text_right().child(row.when.clone()))
        .on_click(cx.listener(move |m, _, _, cx| idea::open(m, &r, cx)))
}

/// Bring the groups and the list's lines up to date, rebuilding only what changed.
fn sync(m: &mut MainWindow) {
    let s = &mut m.ideas;
    let picked = if s.projects.iter().any(|(p, _)| p == s.project()) { s.project().to_string() } else { "all".to_string() };
    let now = fmt::now();
    let key = Key { version: s.held.version, picked, projects: s.projects.clone(), day: fmt::local(now).date_naive() };
    let more = s.held.next.is_some();
    let old = s.view.take();
    let filter_same = old.as_ref().is_some_and(|v| v.key.picked == key.picked);
    let mut v = match old {
        Some(v) if v.key == key => v,
        _ => {
            let mine: Vec<&Value> = s.held.rows.iter().filter(|i| key.picked == "all" || fmt::s(i, "project") == key.picked).collect();
            let groups = groups_of(&mine, &key.picked, now);
            View { key, groups, more, lines: vec![] }
        }
    };
    let mut lines = vec![Line::Top];
    if v.groups.is_empty() {
        lines.push(Line::Empty);
    }
    let many = v.groups.len() > 1;
    for (gi, (_, rows)) in v.groups.iter().enumerate() {
        if many {
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
            // New ideas keep the place scrolled to; another project starts at the top.
            if filter_same {
                l.scroll_to(ListOffset { item_ix: top.item_ix.min(count - 1), offset_in_item: top.offset_in_item });
            }
        }
        // The filter's counts can change with the lines staying put.
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

/// The title, the project filter and New idea: the list's first line.
fn top(m: &MainWindow, t: &Theme, picked: &str, cx: &mut Context<MainWindow>) -> Div {
    let tabs = &m.ideas.projects;
    div()
        .flex()
        .items_center()
        .gap(px(16.))
        .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Ideas"))
        .when(tabs.len() > 1, |d| {
            d.child(ip::project_filter(t, PROJECT_FILTER, picked, tabs, cx, |m, p, cx| {
                m.ideas.project = p;
                cx.notify();
            }))
        })
        .child(div().flex_1())
        .child(kit::btn_primary(t, "ideas-new", "New idea").on_click(cx.listener(|m, _, window, cx| open_form(m, window, cx))))
}

/// One line of the page's list. A project's card is drawn in pieces: its first row carries the
/// top edge, each row the sides, the last row the bottom.
fn line_item(m: &mut MainWindow, ix: usize, _: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let Some(v) = m.ideas.view.take() else { return div().into_any_element() };
    let line = div().px(px(PAGE_X)).line_height(relative(1.4));
    let el = match v.lines.get(ix).copied() {
        Some(Line::Top) => line.pt(px(22.)).pb(px(20.)).child(top(m, &t, &v.key.picked, cx)),
        Some(Line::Empty) => line.child(kit::empty(&t, if v.more { "Loading…" } else { "No ideas" })),
        Some(Line::Group(gi)) => line
            .when(gi > 0, |d| d.pt(px(20.)))
            .pb(px(10.))
            .child(div().text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(t.muted).child(v.groups[gi].0.to_uppercase())),
        Some(Line::Row(gi, ri)) => {
            let rows = &v.groups[gi].1;
            let (first, last) = (ri == 0, ri + 1 == rows.len());
            let card = div()
                .bg(t.card)
                .border_color(t.border)
                .border_x_1()
                .overflow_hidden()
                .when(first, |d| d.border_t_1().rounded_t(px(12.)))
                .when(last, |d| d.border_b_1().rounded_b(px(12.)));
            line.child(card.child(row(&t, &rows[ri], first, cx)))
        }
        Some(Line::End) | None => line.h(px(32.)),
    };
    m.ideas.view = Some(v);
    el.into_any_element()
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let filter = ip::render_project_filter(m, &t, PROJECT_FILTER, window, cx);
    // The dialog's project list: a popover, drawn over the dialog.
    let form_project = ip::render_project_filter(m, &t, FORM_PROJECT, window, cx);
    let shell = div().id("ideas-page").relative().flex().flex_col().flex_1().min_w_0().h_full().children(filter).children(form_project);

    if !m.ideas.held.loaded {
        let msg = m.data.errs.ideas.clone().unwrap_or_else(|| "Loading…".into());
        let page = div().flex().flex_col().gap(px(20.)).px(px(PAGE_X)).pt(px(22.)).pb(px(32.)).line_height(relative(1.4));
        return shell.child(page.child(top(m, &t, "all", cx)).child(kit::empty(&t, msg))).into_any_element();
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
    fn open_ideas_come_first_then_planned_shelved_and_dropped() {
        let now = at(2026, 10, 15, 16);
        let list = json!({"ideas": [
            idea(1, "web", "dropped", json!(null), at(2026, 10, 15, 15)),
            idea(2, "web", "planned", json!("fits"), at(2026, 10, 14, 9)),
            idea(3, "web", "new", json!(null), at(2026, 10, 7, 9)),
            idea(4, "web", "reported", json!("partly"), at(2026, 10, 15, 11)),
            idea(5, "web", "shelved", json!("doesnt_fit"), at(2025, 12, 1, 9)),
        ]});
        let got = groups(&list, "all", now);
        assert_eq!(got.len(), 1);
        let rows: Vec<(&str, Option<&str>, &str, &str, bool)> =
            got[0].1.iter().map(|r| (r.r.as_str(), r.verdict.map(|v| v.0), r.state.0, r.when.as_str(), r.put_away)).collect();
        assert_eq!(
            rows,
            [
                ("I4", Some("Partly fits"), "Reported", "Today, 11:05 AM", false),
                ("I3", None, "New", "Oct 7, 9:05 AM", false),
                ("I2", Some("Fits"), "Planned", "Oct 14, 9:05 AM", false),
                ("I5", Some("Doesn’t fit"), "Shelved", "Dec 1, 2025, 9:05 AM", true),
                ("I1", None, "Dropped", "Today, 3:05 PM", true),
            ]
        );
    }

    #[test]
    fn projects_group_by_name_and_the_filter_counts_open_ideas() {
        let now = at(2026, 10, 15, 16);
        let list = json!({"ideas": [
            idea(1, "web", "new", json!(null), now),
            idea(2, "api", "shelved", json!(null), now),
            idea(3, "web", "planned", json!(null), now),
            idea(4, "api", "digging", json!(null), now),
        ]});
        let names = |picked: &str| groups(&list, picked, now).into_iter().map(|(p, _)| p).collect::<Vec<_>>();
        assert_eq!(names("all"), ["api", "web"]);
        assert_eq!(names("web"), ["web"]);
        // A project without ideas falls back to every one.
        assert_eq!(names("kass"), ["api", "web"]);
        assert!(groups(&list, "all", now).iter().all(|(p, rows)| rows.iter().all(|r| &r.project == p)), "each row carries its project");
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
            assert_eq!(f.error.as_deref(), Some("Choose the project it’s for."));
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
        let rows = w.update(cx, |m, _, _| m.ideas.view.as_ref().unwrap().lines.iter().filter(|l| matches!(l, Line::Row(..))).count()).unwrap();
        assert_eq!(rows, total, "every idea has a line");
        let webapp = w.update(cx, |m, _, _| m.ideas.projects.iter().find(|(p, _)| p == "webapp").map(|(_, n)| *n).unwrap()).unwrap();
        assert!(webapp >= 160, "the filter counts the board's open ideas, not the held ones: {webapp}");
    }

    #[test]
    fn a_running_plan_shows_over_the_state() {
        assert_eq!(state_pill(&json!({"state": "reported", "planning": true})), ("Planning", "accent"));
        assert_eq!(state_pill(&json!({"state": "digging", "planning": false})), ("Digging", "accent"));
        assert_eq!(verdict_pill(&json!("maybe")), None);
    }
}
