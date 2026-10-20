//! The Ideas page (`GET /ideas?project=all&state=all`): each project's ideas as rows, the open
//! ones first, each with its newest report's verdict and its state. A row opens the idea's page
//! (`ui::idea`). New idea opens a dialog that adds one (`POST /ideas`); notes, shelving and
//! dropping go through `tb idea`.
use crate::app::MainWindow;
use crate::fmt::{self, arr, s};
use crate::theme::Theme;
use crate::ui::issue_panel as ip;
use crate::ui::releases::cut_at;
use crate::ui::kit::KeyOutcome;
use crate::ui::modals::{self, Modal};
use crate::ui::{idea, kit};
use chrono::{DateTime, Utc};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};

const PAGE_X: f32 = 28.;
const PROJECT_FILTER: &str = "ideas-project";
const FORM_PROJECT: &str = "new-idea-project";

#[derive(Default)]
pub struct State {
    /// "all" or one project.
    pub project: String,
    /// The New idea dialog's fields while it's open.
    pub form: Option<Form>,
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

fn is_open(i: &Value) -> bool {
    rank(s(i, "state")) == 0
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub r: String,
    pub title: String,
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
    let ideas = arr(list, "ideas");
    let mut projects: Vec<String> = ideas.iter().map(|i| s(i, "project").to_string()).collect();
    projects.sort();
    projects.dedup();
    if projects.iter().any(|p| p == picked) {
        projects.retain(|p| p == picked);
    }
    projects
        .into_iter()
        .map(|p| {
            let mut mine: Vec<&Value> = ideas.iter().filter(|i| s(i, "project") == p).collect();
            mine.sort_by(|a, b| rank(s(a, "state")).cmp(&rank(s(b, "state"))).then_with(|| s(b, "updated_at").cmp(s(a, "updated_at"))));
            let rows = mine
                .into_iter()
                .map(|i| Row {
                    r: fmt::ref_of(i, "I"),
                    title: s(i, "title").to_string(),
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

/// (project, its open ideas) for the filter.
pub fn tabs(list: &Value) -> Vec<(String, usize)> {
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
    for (p, _) in m.data.ideas.as_ref().map(tabs).unwrap_or_default() {
        if !projects.contains(&p) {
            projects.push(p);
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
        .children(row.verdict.map(|(label, tone)| kit::tone_pill(t, tone, label)))
        .child(kit::tone_pill(t, row.state.1, row.state.0))
        .child(div().flex_none().w(px(140.)).whitespace_nowrap().text_size(px(12.)).text_color(t.muted).text_right().child(row.when.clone()))
        .on_click(cx.listener(move |m, _, _, cx| idea::open(m, &r, cx)))
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let now = fmt::now();
    let list = m.data.ideas.clone();
    let tabs = list.as_ref().map(tabs).unwrap_or_default();
    let picked = if tabs.iter().any(|(p, _)| p == m.ideas.project()) { m.ideas.project().to_string() } else { "all".to_string() };
    let head = div()
        .flex()
        .items_center()
        .gap(px(16.))
        .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Ideas"))
        .when(tabs.len() > 1, |d| {
            d.child(ip::project_filter(&t, PROJECT_FILTER, &picked, &tabs, cx, |m, p, cx| {
                m.ideas.project = p;
                cx.notify();
            }))
        })
        .child(div().flex_1())
        .child(kit::btn_primary(&t, "ideas-new", "New idea").on_click(cx.listener(|m, _, window, cx| open_form(m, window, cx))));
    let mut page = div().flex().flex_col().gap(px(20.)).px(px(PAGE_X)).pt(px(22.)).pb(px(32.)).line_height(relative(1.4)).child(head);
    let filter = ip::render_project_filter(m, &t, PROJECT_FILTER, window, cx);
    // The dialog's project list: a popover, drawn over the dialog.
    let form_project = ip::render_project_filter(m, &t, FORM_PROJECT, window, cx);
    let shell = div().id("ideas-page").flex_1().min_w_0().h_full().overflow_y_scroll().restrict_scroll_to_axis().children(filter).children(form_project);

    let Some(list) = list else {
        let msg = m.data.errs.ideas.clone().unwrap_or_else(|| "Loading…".into());
        return shell.child(page.child(kit::empty(&t, msg))).into_any_element();
    };
    let groups = groups(&list, &picked, now);
    if groups.is_empty() {
        return shell.child(page.child(kit::empty(&t, "No ideas"))).into_any_element();
    }
    let many = groups.len() > 1;
    for (p, rows) in groups {
        let mut col = div().flex().flex_col().gap(px(8.));
        if many {
            col = col.child(div().mb(px(2.)).text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(t.muted).child(p.to_uppercase()));
        }
        let mut card = div().flex().flex_col().rounded(px(12.)).border_1().border_color(t.border).bg(t.card).overflow_hidden();
        for (n, r) in rows.iter().enumerate() {
            card = card.child(row(&t, r, n == 0, cx));
        }
        page = page.child(col.child(card));
    }
    shell.child(page).into_any_element()
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

    #[test]
    fn a_running_plan_shows_over_the_state() {
        assert_eq!(state_pill(&json!({"state": "reported", "planning": true})), ("Planning", "accent"));
        assert_eq!(state_pill(&json!({"state": "digging", "planning": false})), ("Digging", "accent"));
        assert_eq!(verdict_pill(&json!("maybe")), None);
    }
}
