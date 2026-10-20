//! The Ideas page (`GET /ideas?project=all&state=all`): each project's ideas as rows, the open
//! ones first, each with its newest report's verdict and its state. A row opens the idea's page
//! (`ui::idea`). Read-only: ideas are added, noted, shelved and dropped with `tb idea`.
use crate::app::MainWindow;
use crate::fmt::{self, arr, s};
use crate::theme::Theme;
use crate::ui::issue_panel as ip;
use crate::ui::releases::cut_at;
use crate::ui::{idea, kit};
use chrono::{DateTime, Utc};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

const PAGE_X: f32 = 28.;
const PROJECT_FILTER: &str = "ideas-project";

#[derive(Default)]
pub struct State {
    /// "all" or one project.
    pub project: String,
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
        });
    let mut page = div().flex().flex_col().gap(px(20.)).px(px(PAGE_X)).pt(px(22.)).pb(px(32.)).line_height(relative(1.4)).child(head);
    let filter = ip::render_project_filter(m, &t, PROJECT_FILTER, window, cx);
    let shell = div().id("ideas-page").flex_1().min_w_0().h_full().overflow_y_scroll().restrict_scroll_to_axis().children(filter);

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
    use chrono::TimeZone;
    use serde_json::json;

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

    #[test]
    fn a_running_plan_shows_over_the_state() {
        assert_eq!(state_pill(&json!({"state": "reported", "planning": true})), ("Planning", "accent"));
        assert_eq!(state_pill(&json!({"state": "digging", "planning": false})), ("Digging", "accent"));
        assert_eq!(verdict_pill(&json!("maybe")), None);
    }
}
