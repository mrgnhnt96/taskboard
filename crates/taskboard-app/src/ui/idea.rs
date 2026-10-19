//! An idea's page (`GET /ideas/:id`): its ref, project, state and verdict, its detail and notes,
//! and its report rounds newest first.
//!
//! The only actions are Dig (`POST /ideas/:id/dig`: Claude reads the project's code and reports)
//! and Plan (`POST /ideas/:id/plan`: Claude makes a planned goal from it). Each is a start signal:
//! the page shows the dig's progress and links to the goal once planned. Nothing here edits the
//! idea: `tb idea` does.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, b, s};
use crate::theme::Theme;
use crate::ui::ideas::{state_pill, verdict_pill};
use crate::ui::releases::cut_at;
use crate::ui::select::sel;
use crate::ui::{goal, kit, md};
use chrono::{DateTime, Utc};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::HashSet;

/// How much of the detail the page renders.
const DETAIL_MAX: usize = 20_000;

#[derive(Default)]
pub struct State {
    /// The requests running, by `dig::I3` / `plan::I3`.
    busy: HashSet<String>,
}

/// Open an idea's page.
pub fn open(m: &mut MainWindow, r: &str, cx: &mut Context<MainWindow>) {
    m.go(Page::Idea(r.to_string()), cx);
    cx.notify();
}

// ================================================================== view models

/// A header button: its label, whether it can be pressed, and why not.
#[derive(Clone, Debug, PartialEq)]
pub struct Btn {
    pub label: String,
    pub enabled: bool,
    pub tip: String,
}

/// The Plan slot: a button, or the goal the idea became.
#[derive(Clone, Debug, PartialEq)]
pub enum PlanSlot {
    Button(Btn),
    Goal(String),
}

/// The round a dig is still writing: the newest, unfinished and without an error.
fn open_round(i: &Value) -> Option<&Value> {
    arr(i, "rounds").first().filter(|r| r["finished_at"].is_null() && r["error"].is_null())
}

/// "Digging…", then "Digging… 4m" once it has run a minute.
fn digging_label(started: &str, now: DateTime<Utc>) -> String {
    match fmt::parse(started).map(|t| (now - t).num_minutes()) {
        Some(m) if m >= 1 => format!("Digging… {}", fmt::span(m)),
        _ => "Digging…".into(),
    }
}

/// Why neither button works on an idea in this state, if one doesn't.
fn closed_reason(i: &Value) -> Option<String> {
    if b(i, "planning") {
        return Some("A plan is being made".into());
    }
    match s(i, "state") {
        "planned" => Some(format!("It became {}", fmt::opt_s(i, "goal").unwrap_or("a goal"))),
        "shelved" => Some("It’s shelved; reopen it with tb idea reopen".into()),
        "dropped" => Some("It’s dropped; reopen it with tb idea reopen".into()),
        _ => None,
    }
}

pub fn dig_button(i: &Value, now: DateTime<Utc>) -> Btn {
    if s(i, "state") == "digging" {
        let started = open_round(i).map(|r| s(r, "started_at")).unwrap_or("");
        return Btn { label: digging_label(started, now), enabled: false, tip: "Claude is reading the code".into() };
    }
    let label = if arr(i, "rounds").iter().any(|r| r["report"].is_object()) { "Dig again" } else { "Dig" }.to_string();
    match closed_reason(i) {
        Some(why) => Btn { label, enabled: false, tip: why },
        None => Btn { label, enabled: true, tip: "Claude reads the project’s code and reports how the idea fits".into() },
    }
}

pub fn plan_slot(i: &Value) -> PlanSlot {
    if let Some(g) = fmt::opt_s(i, "goal").filter(|g| !g.is_empty() && s(i, "state") == "planned") {
        return PlanSlot::Goal(g.to_string());
    }
    if b(i, "planning") {
        return PlanSlot::Button(Btn { label: "Planning…".into(), enabled: false, tip: "Claude is making a goal from it".into() });
    }
    let label = "Plan".to_string();
    if s(i, "state") == "digging" {
        return PlanSlot::Button(Btn { label, enabled: false, tip: "Wait for the dig to finish".into() });
    }
    match closed_reason(i) {
        Some(why) => PlanSlot::Button(Btn { label, enabled: false, tip: why }),
        None => PlanSlot::Button(Btn { label, enabled: true, tip: "Claude makes a planned goal from it and its reports, not started".into() }),
    }
}

/// One report round as the page shows it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RoundView {
    /// "Round 2": counted from the first.
    pub title: String,
    pub focus: String,
    pub verdict: Option<(&'static str, &'static str)>,
    pub why: String,
    /// (what, its files).
    pub builds_on: Vec<(String, Vec<String>)>,
    pub extend: Vec<String>,
    /// (kind, risk).
    pub risks: Vec<(String, String)>,
    pub pitfalls: Vec<String>,
    pub questions: Vec<String>,
    pub related: Vec<String>,
    /// The commit it read, short.
    pub sha: String,
    /// "3 commits behind", "Stale", or empty while current.
    pub drift: String,
    pub when: String,
    /// "Digging… 4m" while it runs.
    pub running: Option<String>,
    pub error: Option<String>,
    /// It read the web too.
    pub web: bool,
}

fn strings(v: &Value) -> Vec<String> {
    v.as_array().map(|a| a.iter().filter_map(|x| x.as_str()).map(str::to_string).collect()).unwrap_or_default()
}

fn drift(r: &Value) -> String {
    if b(r, "stale") {
        return "Stale".into();
    }
    match r["behind"].as_i64() {
        Some(n) if n > 0 => format!("{} behind", fmt::plural(n, "commit", "commits")),
        _ => String::new(),
    }
}

pub fn rounds(i: &Value, now: DateTime<Utc>) -> Vec<RoundView> {
    let all = arr(i, "rounds");
    let open = open_round(i).map(|r| r["id"].clone());
    all.iter()
        .enumerate()
        .map(|(n, r)| {
            let rep = &r["report"];
            let at = fmt::opt_s(r, "finished_at").or(fmt::opt_s(r, "started_at")).unwrap_or("");
            RoundView {
                title: format!("Round {}", all.len() - n),
                focus: s(r, "focus").to_string(),
                verdict: verdict_pill(&rep["verdict"]),
                why: s(rep, "why").to_string(),
                builds_on: arr(rep, "builds_on").iter().map(|x| (s(x, "what").to_string(), strings(&x["files"]))).collect(),
                extend: strings(&rep["ways_to_extend"]),
                risks: arr(rep, "risks").iter().map(|x| (s(x, "kind").to_string(), s(x, "risk").to_string())).collect(),
                pitfalls: strings(&rep["pitfalls"]),
                questions: strings(&rep["open_questions"]),
                related: strings(&rep["related"]),
                sha: s(r, "sha").chars().take(7).collect(),
                drift: drift(r),
                when: fmt::parse(at).map(|t| cut_at(t, now)).unwrap_or_default(),
                running: (open.as_ref() == Some(&r["id"])).then(|| digging_label(s(r, "started_at"), now)),
                error: fmt::opt_s(r, "error").filter(|e| !e.is_empty()).map(str::to_string),
                web: b(r, "web"),
            }
        })
        .collect()
}

/// A note's byline: who left it and when.
pub fn note_byline(n: &Value, now: DateTime<Utc>) -> String {
    let when = fmt::parse(s(n, "at")).map(|t| cut_at(t, now)).unwrap_or_default();
    [s(n, "source"), when.as_str()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ")
}

// ================================================================== requests

/// Dig or Plan: a start signal, once at a time; the board's answer shows on the next refresh.
pub(crate) fn start(m: &mut MainWindow, act: &'static str, r: &str, cx: &mut Context<MainWindow>) {
    let key = format!("{act}::{r}");
    if !m.idea.busy.insert(key.clone()) {
        return;
    }
    let k2 = key.clone();
    m.post_or(
        format!("ideas/{r}/{act}"),
        json!({}),
        cx,
        move |m, _, _| {
            m.idea.busy.remove(&key);
        },
        move |m, e, cx| {
            m.idea.busy.remove(&k2);
            m.toast(e.message, true, cx);
        },
    );
    cx.notify();
}

// ================================================================== render

/// Ideas › webapp › I3: the project crumb opens the Ideas page on that project.
fn crumbs(t: &Theme, i: &Value, cx: &mut Context<MainWindow>) -> Div {
    let hover = t.text;
    let link = |id: &'static str, text: String| div().id(id).cursor_pointer().hover(move |s| s.text_color(hover)).child(text);
    let sep = || div().text_color(t.faint).child("›");
    let project = s(i, "project").to_string();
    let open = project.clone();
    div()
        .flex()
        .items_center()
        .gap(px(6.))
        .text_size(px(12.))
        .text_color(t.muted)
        .child(link("idea-crumb", "Ideas".into()).on_click(cx.listener(|m, _, _, cx| open_ideas(m, "all", cx))))
        .child(sep())
        .child(link("idea-crumb-project", project).on_click(cx.listener(move |m, _, _, cx| open_ideas(m, &open, cx))))
        .child(sep())
        .child(div().font_family(t.mono_font.clone()).child(fmt::ref_of(i, "I")))
}

fn open_ideas(m: &mut MainWindow, project: &str, cx: &mut Context<MainWindow>) {
    m.ideas.project = project.to_string();
    m.go(Page::Ideas, cx);
}

fn button(t: &Theme, id: &'static str, bt: Btn, primary: bool, busy: bool) -> Stateful<Div> {
    let make = if primary { kit::btn_primary } else { kit::btn };
    let label = if busy { "Sending…".to_string() } else { bt.label };
    let el = make(t, id, label).tooltip(kit::tip(bt.tip));
    if bt.enabled && !busy { el } else { kit::disabled(el) }
}

fn buttons(m: &MainWindow, t: &Theme, i: &Value, cx: &mut Context<MainWindow>) -> Div {
    let r = fmt::ref_of(i, "I");
    let dig = dig_button(i, fmt::now());
    let dig_busy = m.idea.busy.contains(&format!("dig::{r}"));
    let can_dig = dig.enabled && !dig_busy;
    let r1 = r.clone();
    let dig_el = button(t, "idea-dig", dig, true, dig_busy).when(can_dig, |d| d.on_click(cx.listener(move |m, _, _, cx| start(m, "dig", &r1, cx))));
    let plan_el = match plan_slot(i) {
        PlanSlot::Goal(g) => {
            let g2 = g.clone();
            kit::btn(t, "idea-goal", format!("{g} →")).tooltip(kit::tip("Open the goal it became")).on_click(cx.listener(move |m, _, _, cx| goal::open(m, &g2, false, cx)))
        }
        PlanSlot::Button(bt) => {
            let busy = m.idea.busy.contains(&format!("plan::{r}"));
            let can = bt.enabled && !busy;
            let r2 = r.clone();
            button(t, "idea-plan", bt, false, busy).when(can, |d| d.on_click(cx.listener(move |m, _, _, cx| start(m, "plan", &r2, cx))))
        }
    };
    div().flex().flex_none().items_center().gap(px(8.)).child(dig_el).child(plan_el)
}

fn header(m: &MainWindow, t: &Theme, i: &Value, cx: &mut Context<MainWindow>) -> Div {
    let (state, tone) = state_pill(i);
    let project = div().px(px(7.)).py(px(1.)).rounded(px(9.)).bg(t.panel_2).text_size(px(11.)).text_color(t.muted).child(s(i, "project").to_string());
    div()
        .flex()
        .items_start()
        .gap(px(16.))
        .child(
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .gap(px(6.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .child(div().text_size(px(12.)).font_family(t.mono_font.clone()).text_color(t.muted).child(sel(t, "idea-ref", fmt::ref_of(i, "I"))))
                        .child(project)
                        .child(kit::tone_pill(t, tone, state))
                        .children(verdict_pill(&i["verdict"]).map(|(label, tone)| kit::tone_pill(t, tone, label))),
                )
                .child(div().text_size(px(26.)).font_weight(FontWeight::BOLD).line_height(relative(1.2)).child(sel(t, "idea-title", s(i, "title").to_string()))),
        )
        .child(buttons(m, t, i, cx))
}

fn section(t: &Theme, title: &str) -> Div {
    div().flex().flex_col().gap(px(8.)).child(kit::h3(t, title.to_string()))
}

fn notes(t: &Theme, i: &Value) -> Option<Div> {
    let list = arr(i, "notes");
    if list.is_empty() {
        return None;
    }
    let now = fmt::now();
    let mut card = kit::card(t).flex().flex_col();
    for (n, note) in list.iter().enumerate() {
        card = card.child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(px(16.))
                .py(px(10.))
                .when(n > 0, |d| d.border_t_1().border_color(t.divider))
                .child(div().text_size(px(13.5)).text_color(t.text).child(sel(t, SharedString::from(format!("idea-note-{n}")), s(note, "text").to_string())))
                .child(div().text_size(px(11.5)).text_color(t.muted).child(note_byline(note, now))),
        );
    }
    Some(section(t, "Notes").child(card))
}

fn list_block(t: &Theme, title: &str, items: &[String]) -> Option<Div> {
    if items.is_empty() {
        return None;
    }
    let mut col = div().flex().flex_col().gap(px(4.)).child(div().text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(t.text_2).child(title.to_string()));
    for x in items {
        col = col.child(div().flex().gap(px(8.)).text_size(px(13.)).child(div().flex_none().text_color(t.faint).child("•")).child(div().min_w_0().flex_1().child(x.clone())));
    }
    Some(col)
}

fn related_link(t: &Theme, n: usize, r: &str, cx: &mut Context<MainWindow>) -> AnyElement {
    let id = SharedString::from(format!("idea-related-{n}-{r}"));
    let el = kit::link(t, id, r.to_string()).font_family(t.mono_font.clone()).text_size(px(12.));
    let r = r.to_string();
    match r.chars().next() {
        Some('G') => el.text_color(t.goal).on_click(cx.listener(move |m, _, _, cx| goal::open(m, &r, false, cx))).into_any_element(),
        Some('T') => el.on_click(cx.listener(move |m, _, _, cx| m.open_task(r.clone(), cx))).into_any_element(),
        Some('B') => el.on_click(cx.listener(move |m, _, _, cx| m.open_issue(r.clone(), cx))).into_any_element(),
        Some('I') => el.on_click(cx.listener(move |m, _, _, cx| open(m, &r, cx))).into_any_element(),
        _ => kit::mono(t, r).text_color(t.muted).into_any_element(),
    }
}

fn round_card(t: &Theme, n: usize, rv: &RoundView, cx: &mut Context<MainWindow>) -> Div {
    let meta = [rv.sha.clone(), rv.drift.clone(), rv.when.clone()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
    let head = div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(div().flex_none().text_size(px(14.)).font_weight(FontWeight::BOLD).child(rv.title.clone()))
        .children(rv.verdict.map(|(label, tone)| kit::tone_pill(t, tone, label)))
        .children(rv.running.clone().map(|l| kit::tone_pill(t, "accent", l)))
        .when(rv.web, |d| d.child(kit::chip(t, "Web")))
        .child(div().flex_1())
        .child(div().flex_none().whitespace_nowrap().text_size(px(12.)).font_family(t.mono_font.clone()).text_color(if rv.drift == "Stale" { t.warn_fg } else { t.muted }).child(meta));
    let mut body = div().flex().flex_col().gap(px(12.)).px(px(18.)).py(px(14.)).child(head);
    if !rv.focus.is_empty() {
        body = body.child(div().text_size(px(13.)).text_color(t.text_2).child(format!("Focus: {}", rv.focus)));
    }
    if let Some(e) = &rv.error {
        body = body.child(div().text_size(px(12.5)).text_color(t.down).font_weight(FontWeight::MEDIUM).child(e.clone()));
    }
    if !rv.why.is_empty() {
        body = body.child(div().text_size(px(14.)).line_height(relative(1.5)).child(sel(t, SharedString::from(format!("idea-why-{n}")), rv.why.clone())));
    }
    if !rv.builds_on.is_empty() {
        let mut col = div().flex().flex_col().gap(px(6.)).child(div().text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(t.text_2).child("Builds on"));
        for (what, files) in &rv.builds_on {
            col = col.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().flex().gap(px(8.)).text_size(px(13.)).child(div().flex_none().text_color(t.faint).child("•")).child(div().min_w_0().flex_1().child(what.clone())))
                    .when(!files.is_empty(), |d| d.child(div().pl(px(16.)).flex().flex_wrap().gap(px(6.)).children(files.iter().map(|f| kit::mono(t, f.clone()).text_size(px(11.5)).text_color(t.muted))))),
            );
        }
        body = body.child(col);
    }
    body = body.children(list_block(t, "Ways to extend", &rv.extend));
    if !rv.risks.is_empty() {
        let mut col = div().flex().flex_col().gap(px(4.)).child(div().text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(t.text_2).child("Risks"));
        for (kind, risk) in &rv.risks {
            let tone = if kind == "security" || kind == "privacy" { "down" } else { "warn" };
            col = col.child(div().flex().items_start().gap(px(8.)).text_size(px(13.)).child(kit::tone_pill(t, tone, kind.clone())).child(div().min_w_0().flex_1().child(risk.clone())));
        }
        body = body.child(col);
    }
    body = body.children(list_block(t, "Pitfalls", &rv.pitfalls)).children(list_block(t, "Open questions", &rv.questions));
    if !rv.related.is_empty() {
        body = body.child(
            div()
                .flex()
                .flex_wrap()
                .items_center()
                .gap(px(10.))
                .child(div().text_size(px(12.)).font_weight(FontWeight::BOLD).text_color(t.text_2).child("Related"))
                .children(rv.related.iter().map(|r| related_link(t, n, r, cx))),
        );
    }
    kit::card(t).child(body)
}

pub fn render(m: &mut MainWindow, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let want = match &m.page {
        Page::Idea(r) => r.clone(),
        _ => String::new(),
    };
    let Some(i) = m.data.idea.clone().filter(|i| fmt::ref_of(i, "I") == want) else {
        let err = m.data.errs.idea.clone();
        return div()
            .flex_1()
            .pt(px(20.))
            .px(px(36.))
            .child(match err {
                Some(e) => div().text_size(px(12.5)).text_color(t.down).font_weight(FontWeight::MEDIUM).child(e),
                None => kit::help(&t, "Loading…"),
            })
            .into_any_element();
    };
    let detail = s(&i, "detail").trim().to_string();
    let views = rounds(&i, fmt::now());
    let mut reports = section(&t, "Reports");
    for (n, rv) in views.iter().enumerate() {
        reports = reports.child(round_card(&t, n, rv, cx));
    }
    div()
        .id("idea-page")
        .flex_1()
        .min_w_0()
        .overflow_y_scroll()
        .line_height(relative(1.5))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(20.))
                .max_w(px(920.))
                .pt(px(20.))
                .px(px(36.))
                .pb(px(48.))
                .child(crumbs(&t, &i, cx))
                .child(header(m, &t, &i, cx))
                .when(!detail.is_empty(), |d| d.child(div().text_size(px(14.)).text_color(t.text_2).child(md::render(&t, &detail, DETAIL_MAX, "idea-detail"))))
                .children(notes(&t, &i))
                .when(!views.is_empty(), |d| d.child(reports)),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    // Beats the glob's `gpui_kit::test`.
    use ::core::prelude::v1::test;
    use chrono::{Duration, TimeZone};

    fn at(y: i32, mo: u32, d: u32, h: u32) -> DateTime<Utc> {
        fmt::local(Utc::now()).offset().with_ymd_and_hms(y, mo, d, h, 5, 0).unwrap().with_timezone(&Utc)
    }

    fn idea(state: &str, rounds: Value) -> Value {
        json!({"ref": "I3", "project": "web", "state": state, "planning": false, "goal": null, "rounds": rounds, "notes": []})
    }

    #[test]
    fn dig_shows_progress_while_digging_and_is_closed_once_planned() {
        let now = at(2026, 10, 15, 16);
        let open = json!([{"id": 2, "started_at": (now - Duration::minutes(4)).to_rfc3339(), "finished_at": null, "error": null, "report": null}]);
        assert_eq!(dig_button(&idea("digging", open), now), Btn { label: "Digging… 4m".into(), enabled: false, tip: "Claude is reading the code".into() });
        let fresh = json!([{"id": 2, "started_at": now.to_rfc3339(), "finished_at": null, "error": null}]);
        assert_eq!(dig_button(&idea("digging", fresh), now).label, "Digging…");

        let new = dig_button(&idea("new", json!([])), now);
        assert_eq!((new.label.as_str(), new.enabled), ("Dig", true));
        let done = json!([{"id": 1, "report": {"verdict": "fits"}, "finished_at": now.to_rfc3339()}]);
        let again = dig_button(&idea("reported", done), now);
        assert_eq!((again.label.as_str(), again.enabled), ("Dig again", true));

        let mut planned = idea("planned", json!([]));
        planned["goal"] = json!("G5");
        assert_eq!(dig_button(&planned, now), Btn { label: "Dig".into(), enabled: false, tip: "It became G5".into() });
        assert!(!dig_button(&idea("shelved", json!([])), now).enabled);
    }

    #[test]
    fn plan_links_to_the_goal_once_planned() {
        let mut planned = idea("planned", json!([]));
        planned["goal"] = json!("G5");
        assert_eq!(plan_slot(&planned), PlanSlot::Goal("G5".into()));

        let mut planning = idea("reported", json!([]));
        planning["planning"] = json!(true);
        assert_eq!(plan_slot(&planning), PlanSlot::Button(Btn { label: "Planning…".into(), enabled: false, tip: "Claude is making a goal from it".into() }));
        assert!(!dig_button(&planning, Utc::now()).enabled, "no dig while a plan runs");

        let enabled = |state: &str| matches!(plan_slot(&idea(state, json!([]))), PlanSlot::Button(Btn { enabled: true, .. }));
        assert!(enabled("new") && enabled("reported"));
        assert!(!enabled("digging") && !enabled("dropped") && !enabled("shelved"));
    }

    #[test]
    fn rounds_read_newest_first_with_their_report_commit_and_time() {
        let now = at(2026, 10, 15, 16);
        let rounds = json!([
            {"id": 3, "focus": "", "report": null, "sha": "", "web": false, "started_at": (now - Duration::minutes(2)).to_rfc3339(), "finished_at": null, "error": null, "behind": null, "stale": false},
            {"id": 2, "focus": "offline sync", "sha": "abc1234def", "web": true, "started_at": at(2026, 10, 15, 9).to_rfc3339(),
             "finished_at": at(2026, 10, 15, 10).to_rfc3339(), "error": null, "behind": 3, "stale": false,
             "report": {"verdict": "partly", "why": "Needs a queue.", "builds_on": [{"what": "The lens thread", "files": ["src/lens.rs:40"]}],
                        "ways_to_extend": ["Sync settings"], "risks": [{"kind": "security", "risk": "Tokens on disk"}],
                        "pitfalls": ["Clock skew"], "open_questions": ["Which store?"], "related": ["G2", "T9"]}},
            {"id": 1, "focus": "", "report": null, "sha": "0011223344", "web": false, "started_at": at(2026, 10, 7, 9).to_rfc3339(),
             "finished_at": at(2026, 10, 7, 9).to_rfc3339(), "error": "claude timed out", "behind": null, "stale": true},
        ]);
        let v = rounds_of(&idea("digging", rounds), now);
        assert_eq!(v.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["Round 3", "Round 2", "Round 1"]);
        assert_eq!(v[0].running.as_deref(), Some("Digging… 2m"));
        assert_eq!(
            v[1],
            RoundView {
                title: "Round 2".into(),
                focus: "offline sync".into(),
                verdict: Some(("Partly fits", "warn")),
                why: "Needs a queue.".into(),
                builds_on: vec![("The lens thread".into(), vec!["src/lens.rs:40".into()])],
                extend: vec!["Sync settings".into()],
                risks: vec![("security".into(), "Tokens on disk".into())],
                pitfalls: vec!["Clock skew".into()],
                questions: vec!["Which store?".into()],
                related: vec!["G2".into(), "T9".into()],
                sha: "abc1234".into(),
                drift: "3 commits behind".into(),
                when: "Today, 10:05 AM".into(),
                running: None,
                error: None,
                web: true,
            }
        );
        assert_eq!((v[2].error.as_deref(), v[2].drift.as_str(), v[2].running.clone()), (Some("claude timed out"), "Stale", None));
    }

    fn rounds_of(i: &Value, now: DateTime<Utc>) -> Vec<RoundView> {
        rounds(i, now)
    }

    #[test]
    fn a_notes_byline_is_who_and_when() {
        let now = at(2026, 10, 15, 16);
        assert_eq!(note_byline(&json!({"source": "T12", "at": at(2026, 10, 15, 14).to_rfc3339()}), now), "T12 · Today, 2:05 PM");
        assert_eq!(note_byline(&json!({"source": null, "at": at(2026, 10, 2, 8).to_rfc3339()}), now), "Oct 2, 8:05 AM");
    }

    #[gpui_kit::test]
    fn dig_sends_once_and_a_refusal_frees_the_button(cx: &mut gpui_kit::TestAppContext) {
        use crate::parity::{settle, window};
        let (w, rec) = window(cx);
        w.update(cx, |m, _, cx| {
            start(m, "dig", "I5", cx);
            assert!(m.idea.busy.contains("dig::I5"), "sending");
            start(m, "dig", "I5", cx);
        })
        .unwrap();
        settle(cx);
        assert_eq!(rec.posts().iter().filter(|(p, _)| p == "ideas/I5/dig").count(), 1);
        assert_eq!(rec.last("ideas/I5/dig"), Some(json!({})));
        w.update(cx, |m, _, _| {
            assert!(m.idea.busy.is_empty(), "a refused dig doesn't stay on Sending…");
            assert_eq!(m.toasts.iter().map(|t| (t.text.as_str(), t.err)).collect::<Vec<_>>(), [("I5 became a goal already; work on the goal instead.", true)]);
        })
        .unwrap();
    }
}
