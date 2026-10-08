//! The Backlog page, as the web board had it (`renderBacklogPage`): "← Task board", the "N open"
//! pill, Goals and "Add an issue"; filters (project and goal pickers, type, state, sort);
//! "Showing N of total" and the list, each open issue with Make it a task / Create ticket / Won't
//! do; and beside it the selected issue (`issueAside`), the first one until you pick another.
//!
//! An issue you just changed stays where it was until the filters change (`keepBacklogRow`,
//! `withKept`), refreshed from `GET /backlog/:id` while it no longer matches (`refreshKept`).
//! With other than the default filters, "N open" still counts every open issue (a second
//! `GET /backlog` with the defaults, as the web board did).
//!
//! What is drawn comes from [`page_view`], checked against the web board in
//! `parity/golden/backlog.json`.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, s};
use crate::theme::Theme;
use crate::ui::issue_panel::{self as ip, Act, AsideView, Ctx};
#[cfg(test)]
use crate::ui::issue_panel::flat;
use crate::ui::{kit, modals};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const KINDS: [(&str, &str); 5] = [("all", "All"), ("bug", "Bug"), ("gap", "Test gap"), ("follow", "Follow-up"), ("clean", "Clean-up")];
const STATES: [(&str, &str); 5] = [("open", "Open"), ("task", "Planned task"), ("ticket", "Jira ticket"), ("drop", "Won’t do"), ("all", "All")];
const SORTS: [(&str, &str); 3] = [("new", "Newest"), ("old", "Oldest"), ("kind", "Type")];
const SYNC_EVERY: Duration = Duration::from_secs(3);

/// The page's filters (`P.blFilter`; "" = the default), the issue beside the list (`?issue=`),
/// the rows kept after a change (`P.kept`, `P.order`), and what this page fetches itself.
#[derive(Default)]
pub struct State {
    pub project: String,
    /// "all", "none" or a goal ref (`G3`).
    pub goal: String,
    pub kind: String,
    pub state: String,
    pub sort: String,
    /// The issue picked to show beside the list; None = the first row (`currentIssueRef`).
    pub selected: Option<String>,
    kept: HashMap<String, Value>,
    order: Vec<String>,
    /// The rows last shown (`backlogRows()`), for `keepBacklogRow`.
    rows: Vec<Value>,
    /// The issue beside the list: its ref and `GET /backlog/:id` (or the error).
    aside: Option<(String, Result<Value, String>)>,
    /// Open issues with the default filters (`P.openAll`).
    open_all: Option<i64>,
    synced: Option<Instant>,
    generation: u64,
}

impl State {
    /// `GET /backlog` query for the current filters (`loadBacklog`).
    pub fn query(&self) -> Vec<(&'static str, String)> {
        let goal = match self.f_goal() {
            g @ ("all" | "none") => g.to_string(),
            g => g.trim_start_matches(|c: char| c.is_ascii_alphabetic()).to_string(),
        };
        vec![
            ("project", self.f_project().to_string()),
            ("goal", goal),
            ("kind", self.f_kind().to_string()),
            ("state", self.f_state().to_string()),
            ("sort", self.f_sort().to_string()),
        ]
    }

    fn f_project(&self) -> &str {
        or(&self.project, "all")
    }
    fn f_goal(&self) -> &str {
        or(&self.goal, "all")
    }
    fn f_kind(&self) -> &str {
        or(&self.kind, "all")
    }
    fn f_state(&self) -> &str {
        or(&self.state, "open")
    }
    fn f_sort(&self) -> &str {
        or(&self.sort, "new")
    }

    /// `isDefaultBl`.
    pub fn is_default(&self) -> bool {
        self.f_project() == "all" && self.f_goal() == "all" && self.f_kind() == "all" && self.f_state() == "open"
    }
}

fn or<'a>(v: &'a str, dflt: &'a str) -> &'a str {
    if v.is_empty() { dflt } else { v }
}

/// Fetch what this page shows besides the list on the next render.
pub fn stale(m: &mut MainWindow) {
    m.backlog.synced = None;
}

/// `keepBacklogRow`: keep this row where it is until the filters change.
pub fn keep_row(m: &mut MainWindow, r: &str) {
    if let Some(b) = m.backlog.rows.iter().find(|b| fmt::ref_of(b, "B") == r) {
        m.backlog.kept.insert(r.to_string(), b.clone());
    }
}

/// Coming to the page (`resetPageState`): no kept rows; show `issue` (or the first row).
pub fn enter(m: &mut MainWindow, issue: Option<String>) {
    let st = &mut m.backlog;
    st.kept.clear();
    st.order.clear();
    st.selected = issue;
    st.synced = None;
}

/// `blFiltersChanged`: forget kept rows and the picked issue, then fetch.
fn filters_changed(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let st = &mut m.backlog;
    st.kept.clear();
    st.order.clear();
    st.selected = None;
    st.synced = None;
    m.refresh(cx);
}

/// `withKept`: the answer, plus kept rows that no longer match, back in their old places; kept
/// copies refreshed from the answer (`refreshKept`).
pub fn with_kept(issues: &[Value], kept: &mut HashMap<String, Value>, order: &[String]) -> Vec<Value> {
    for b in issues {
        let r = fmt::ref_of(b, "B");
        if let Some(k) = kept.get_mut(&r) {
            *k = b.clone();
        }
    }
    let mut rows = issues.to_vec();
    let mut have: Vec<String> = rows.iter().map(|b| fmt::ref_of(b, "B")).collect();
    for (i, r) in order.iter().enumerate() {
        if !have.contains(r) {
            if let Some(b) = kept.get(r) {
                rows.insert(i.min(rows.len()), b.clone());
                have.push(r.clone());
            }
        }
    }
    rows
}

/// `blStateText`: the chip on a row that isn't open.
pub fn bl_state_text(b: &Value) -> String {
    if s(b, "state") == "task" {
        return if ip::goal_id(b).is_some() { "Planned task in its goal" } else { "Queued task, no goal" }.into();
    }
    match ip::issue_state(b) {
        Some(_) if s(b, "state") == "drop" => "Won’t do".into(),
        Some((_, text)) => text,
        None => String::new(),
    }
}

// ------------------------------------------------------------------ view model

pub struct RowView {
    pub r: String,
    pub kind: String,
    pub title: String,
    /// The goal's name, or None ("Not in a goal").
    pub goal: Option<String>,
    pub project: String,
    pub from: String,
    /// Open: its actions; else the state chip.
    pub acts: Vec<Act>,
    pub state_chip: Option<String>,
    pub note: Option<(String, bool)>,
    pub selected: bool,
}

impl RowView {
    #[cfg(test)]
    pub fn text_and_acts(&self) -> (String, Vec<&'static str>) {
        let mut out = vec![self.kind.clone(), self.title.clone(), self.goal.clone().unwrap_or_else(|| "Not in a goal".into()), self.project.clone(), self.from.clone()];
        let mut acts = vec!["pick-issue"];
        for a in &self.acts {
            out.push(a.label.clone());
            acts.push(a.act);
        }
        out.extend(self.state_chip.clone());
        out.extend(self.note.as_ref().map(|n| n.0.clone()));
        (flat(&out), acts)
    }
}

pub enum ListView {
    /// `Loading…`, or why the list couldn't be fetched.
    Loading(Option<String>),
    Rows { showing: String, rows: Vec<RowView> },
}

pub enum AsideState {
    None,
    Loading(Option<String>),
    Issue(Box<AsideView>),
}

pub struct PageView {
    pub open: Option<i64>,
    pub project_label: String,
    pub goal_label: String,
    pub kind_on: usize,
    pub state_options: Vec<&'static str>,
    pub list: ListView,
    pub aside: AsideState,
}

/// What the page needs to know, gathered from the window (or a test).
pub struct PageInput<'a> {
    pub st: &'a State,
    pub ctx: Ctx<'a>,
    pub state: &'a Value,
    /// `P.backlog`: None while the first answer is on its way.
    pub backlog: Option<&'a Value>,
    /// `P.backlogErr`: why the list couldn't be fetched (shown instead of "Loading…").
    pub backlog_err: Option<&'a str>,
    pub rows: Vec<Value>,
    /// The issue beside the list (`S.issueRef`) and its answer (None = loading).
    pub aside: Option<(String, Option<Result<Value, String>>)>,
}

pub fn page_view(p: &PageInput) -> PageView {
    let st = p.st;
    let goals = p.ctx.goals;
    let open = st.open_all.or_else(|| p.state.get("counts").filter(|c| c.is_object()).and_then(|c| c.get("open_issues")).and_then(Value::as_i64));
    let list = match p.backlog {
        None => ListView::Loading(p.backlog_err.map(str::to_string)),
        Some(b) => {
            let total = fmt::i(b, "total");
            let sel = p.aside.as_ref().map(|a| a.0.clone());
            ListView::Rows {
                showing: format!("Showing {} of {total} issue{}", p.rows.len(), if total == 1 { "" } else { "s" }),
                rows: p
                    .rows
                    .iter()
                    .map(|b| {
                        let r = fmt::ref_of(b, "B");
                        let is_open = matches!(s(b, "state"), "" | "open");
                        let place = if ip::goal_id(b).is_some() { "goal" } else { "board" };
                        RowView {
                            kind: ip::kind_label(s(b, "kind")),
                            title: s(b, "title").to_string(),
                            goal: Some(ip::goal_name(goals, b)).filter(|g| !g.is_empty()),
                            project: s(b, "project").to_string(),
                            from: ip::issue_from(b, true),
                            acts: if is_open { ip::issue_actions(&p.ctx, &r, place) } else { vec![] },
                            state_chip: (!is_open && ip::issue_state(b).is_some()).then(|| bl_state_text(b)),
                            note: p.ctx.note(&format!("issue:{r}")),
                            selected: sel.as_deref() == Some(r.as_str()),
                            r,
                        }
                    })
                    .collect(),
            }
        }
    };
    let aside = match &p.aside {
        None => AsideState::None,
        Some((_, None)) => AsideState::Loading(None),
        Some((_, Some(Err(e)))) => AsideState::Loading(Some(e.clone())),
        Some((_, Some(Ok(b)))) => AsideState::Issue(Box::new(ip::aside_view(&p.ctx, b))),
    };
    let pspec = [("all".to_string(), "All projects".to_string())];
    let gspec = [("all".to_string(), "All goals".to_string()), ("none".to_string(), "Not in a goal".to_string())];
    PageView {
        open,
        project_label: ip::picker_label(false, st.f_project(), &pspec, goals),
        goal_label: ip::picker_label(true, st.f_goal(), &gspec, goals),
        kind_on: KINDS.iter().position(|(k, _)| *k == st.f_kind()).unwrap_or(0),
        state_options: STATES.iter().map(|(k, _)| *k).filter(|k| *k != "ticket" || p.ctx.jira || st.f_state() == "ticket").collect(),
        list,
        aside,
    }
}

impl PageView {
    #[cfg(test)]
    pub fn header_text(&self) -> String {
        let mut out = vec!["Task board".to_string(), "Backlog".into()];
        out.extend(self.open.map(|n| format!("{n} open")));
        out.push("Goals".into());
        out.push("Add an issue".into());
        flat(&out)
    }

    #[cfg(test)]
    pub fn filters_text(&self) -> String {
        let mut out = vec!["Project".to_string(), self.project_label.clone(), "Goal".into(), self.goal_label.clone()];
        out.extend(KINDS.iter().map(|(_, l)| l.to_string()));
        out.push("State".into());
        out.push("Sort".into());
        flat(&out)
    }

    pub fn list_head(&self) -> String {
        match &self.list {
            ListView::Loading(e) => e.clone().unwrap_or_else(|| "Loading…".into()),
            ListView::Rows { showing, rows } if rows.is_empty() => format!("{showing} No issues match these filters."),
            ListView::Rows { showing, .. } => showing.clone(),
        }
    }

    #[cfg(test)]
    pub fn aside_text_and_acts(&self) -> (String, Vec<&'static str>) {
        match &self.aside {
            AsideState::None => (String::new(), vec![]),
            AsideState::Loading(e) => (e.clone().unwrap_or_else(|| "Loading…".into()), vec![]),
            AsideState::Issue(v) => v.text_and_acts(None),
        }
    }
}

/// `#/goals` with no goal: the board's goal filter, else the first active goal by project, else
/// the first goal (`renderGoalPage`).
pub fn goals_landing(m: &MainWindow) -> Option<String> {
    let goals = ip::live_goals(&m.data.goals);
    let finished = |g: &Value| {
        let (n, done) = (fmt::i(g, "total"), fmt::i(g, "done"));
        n > 0 && done == n && arr(g, "prs_open").is_empty()
    };
    if let Some(g) = goals.iter().find(|g| fmt::ref_of(g, "G") == m.filters.goal) {
        return Some(fmt::ref_of(g, "G"));
    }
    let mut active: Vec<&&Value> = goals.iter().filter(|g| !finished(g) && !fmt::b(g, "deprioritized")).collect();
    active.sort_by(|a, b| crate::ui::modals::locale_cmp(s(a, "project"), s(b, "project")));
    active.first().map(|g| fmt::ref_of(g, "G")).or_else(|| goals.first().map(|g| fmt::ref_of(g, "G")))
}

// ------------------------------------------------------------------ fetching

/// Fetch (every few seconds, and right after a change) the issue beside the list, kept rows
/// that left the answer, and the default-filter open count.
fn sync(m: &mut MainWindow, rows: &[Value], cx: &mut Context<MainWindow>) {
    let st = &mut m.backlog;
    if st.synced.is_some_and(|t| t.elapsed() < SYNC_EVERY) {
        return;
    }
    st.synced = Some(Instant::now());
    st.generation += 1;
    let generation = st.generation;
    let aside = st.selected.clone().or_else(|| rows.first().map(|b| fmt::ref_of(b, "B")));
    let have: Vec<String> = m.data.backlog.as_ref().map(|b| arr(b, "issues").iter().map(|x| fmt::ref_of(x, "B")).collect()).unwrap_or_default();
    let missing: Vec<String> = st.kept.keys().filter(|k| !have.contains(k)).cloned().collect();
    let all_q = (!st.is_default()).then(|| vec![("project", "all".to_string()), ("goal", "all".into()), ("kind", "all".into()), ("state", "open".into()), ("sort", "new".into())]);
    let backend = m.backend.clone();
    cx.spawn(async move |this, cx| {
        let got = cx
            .background_executor()
            .spawn(async move {
                let one = |r: &str| backend.get(&format!("backlog/{r}"), &[]).map(|v| v.get("issue").cloned().filter(Value::is_object).unwrap_or(v)).map_err(|e| e.message);
                let aside = aside.map(|r| {
                    let v = one(&r);
                    (r, v)
                });
                let kept: Vec<(String, Value)> = missing.iter().filter_map(|r| one(r).ok().map(|v| (r.clone(), v))).collect();
                let all = all_q.and_then(|q| backend.get("backlog", &q).ok()).map(|v| v.get("total").and_then(Value::as_i64).unwrap_or(arr(&v, "issues").len() as i64));
                (aside, kept, all)
            })
            .await;
        let _ = this.update(cx, |m, cx| {
            let st = &mut m.backlog;
            if st.generation != generation {
                return;
            }
            let (aside, kept, all) = got;
            st.aside = aside;
            for (r, v) in kept {
                if st.kept.contains_key(&r) {
                    st.kept.insert(r, v);
                }
            }
            if all.is_some() {
                st.open_all = all;
            }
            cx.notify();
        });
    })
    .detach();
}

// ------------------------------------------------------------------ render

/// A filter `<select>`: "Label ▾", opening the menu `key`.
fn select_btn(t: &Theme, key: &'static str, label: String, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    kit::btn_small(t, SharedString::from(key), format!("{label}  ▾")).on_click(cx.listener(move |m, e: &ClickEvent, _, cx| {
        let p = e.position();
        m.toggle_menu(key, point(p.x - px(10.), p.y + px(14.)), cx);
    }))
}

fn option_menu(m: &MainWindow, t: &Theme, key: &str, opts: Vec<(&'static str, &'static str)>, cur: &str, cx: &mut Context<MainWindow>, set: fn(&mut State, String)) -> Option<AnyElement> {
    let at = m.menu_open(key)?;
    let mut list = kit::menu_box(t, 200.).id(SharedString::from(format!("{key}-menu")));
    for (n, (v, l)) in opts.into_iter().enumerate() {
        list = list.child(kit::menu_item(t, SharedString::from(format!("{key}-{n}")), l, v == cur).on_click(cx.listener(move |m, _, _, cx| {
            m.menu = None;
            set(&mut m.backlog, v.to_string());
            filters_changed(m, cx);
        })));
    }
    Some(kit::popover(at, list.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())))
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    // `P.openAll`: the answer's total while the filters are the defaults.
    if m.backlog.is_default() {
        if let Some(b) = m.data.backlog.as_ref() {
            m.backlog.open_all = Some(fmt::i(b, "total"));
        }
    }
    let rows = match m.data.backlog.as_ref() {
        Some(b) => {
            let issues = arr(b, "issues").to_vec();
            let st = &mut m.backlog;
            let rows = with_kept(&issues, &mut st.kept, &st.order.clone());
            st.order = rows.iter().map(|b| fmt::ref_of(b, "B")).collect();
            st.rows = rows.clone();
            rows
        }
        None => vec![],
    };
    sync(m, &rows, cx);
    let aside_ref = m.backlog.selected.clone().or_else(|| rows.first().map(|b| fmt::ref_of(b, "B")));
    let aside = aside_ref.map(|r| {
        let got = m.backlog.aside.as_ref().filter(|(ar, _)| *ar == r).map(|(_, v)| v.clone());
        (r, got)
    });
    let state = m.state().clone();
    let v = page_view(&PageInput { st: &m.backlog, ctx: Ctx::of(m), state: &state, backlog: m.data.backlog.as_ref(), backlog_err: m.data.errs.backlog.as_deref(), rows, aside: aside.clone() });

    // ---- header
    let header = div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(12.))
        .child(kit::link(&t, "bl-back", "‹ Task board").text_size(px(13.5)).on_click(cx.listener(|m, _, _, cx| m.go(Page::Board, cx))))
        .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child("Backlog"))
        .children(v.open.map(|n| kit::tone_pill(&t, "warn", format!("{n} open"))))
        .child(div().flex_1())
        .child(kit::link(&t, "bl-goals", "Goals").text_size(px(13.5)).on_click(cx.listener(|m, _, _, cx| {
            if let Some(g) = goals_landing(m) {
                m.go(Page::Goal(g), cx);
            }
        })))
        .child(kit::btn_primary(&t, "bl-add", "Add an issue").on_click(cx.listener(|m, _, window, cx| {
            let g = m.backlog.f_goal().to_string();
            let goal = (g != "all" && g != "none").then_some(g);
            modals::open_issue_form(m, goal, window, cx);
        })));

    // ---- filters
    let st = &m.backlog;
    let (fp, fg) = (st.f_project().to_string(), st.f_goal().to_string());
    let labels: Vec<&str> = KINDS.iter().map(|(_, l)| *l).collect();
    let kinds = kit::seg(&t, "bl-kind", &labels, v.kind_on, |i, item| {
        item.on_click(cx.listener(move |m, _, _, cx| {
            m.backlog.kind = KINDS[i].0.to_string();
            filters_changed(m, cx);
        }))
    });
    let lbl = |text: &str| div().text_size(px(12.5)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_2).child(text.to_string());
    let (fp1, fg1, fp2) = (fp.clone(), fg.clone(), fp.clone());
    let filters = div()
        .flex()
        .flex_none()
        .flex_wrap()
        .items_center()
        .gap(px(8.))
        .child(lbl("Project"))
        .child(ip::picker_button(&t, "bl-project", v.project_label.clone(), false).min_w(px(170.)).on_click(cx.listener(move |m, e: &ClickEvent, window, cx| {
            let p = e.position();
            ip::open_picker(m, "bl-project", false, fp1.clone(), vec![("all".into(), "All projects".into())], String::new(), point(p.x - px(10.), p.y + px(14.)), window, cx, |m, v, cx| {
                m.backlog.project = v;
                filters_changed(m, cx);
            });
        })))
        .child(lbl("Goal").ml(px(4.)))
        .child(ip::picker_button(&t, "bl-goal", v.goal_label.clone(), v.goal_label == "Choose a goal").min_w(px(220.)).on_click(cx.listener(move |m, e: &ClickEvent, window, cx| {
            let p = e.position();
            let specials = vec![("all".into(), "All goals".into()), ("none".into(), "Not in a goal".into())];
            ip::open_picker(m, "bl-goal", true, fg1.clone(), specials, fp2.clone(), point(p.x - px(10.), p.y + px(14.)), window, cx, |m, v, cx| {
                m.backlog.goal = v;
                filters_changed(m, cx);
            });
        })))
        .child(div().ml(px(4.)).child(kinds))
        .child(lbl("State").ml(px(4.)))
        .child(select_btn(&t, "bl-state", STATES.iter().find(|(k, _)| *k == st.f_state()).map(|(_, l)| l.to_string()).unwrap_or_default(), cx))
        .child(div().flex_1())
        .child(lbl("Sort"))
        .child(select_btn(&t, "bl-sort", SORTS.iter().find(|(k, _)| *k == st.f_sort()).map(|(_, l)| l.to_string()).unwrap_or_default(), cx));

    // ---- list
    let mut list = div().flex().flex_col().gap(px(8.)).flex_1().min_w_0();
    match &v.list {
        ListView::Loading(e) => list = list.child(div().text_size(px(13.)).text_color(if e.is_some() { t.down } else { t.muted }).child(v.list_head())),
        ListView::Rows { showing, rows } => {
            list = list.child(div().text_size(px(13.)).text_color(t.muted).child(showing.clone()));
            if rows.is_empty() {
                list = list.child(div().p(px(24.)).rounded(px(12.)).border_1().border_color(t.border).flex().justify_center().text_size(px(13.)).text_color(t.muted).child("No issues match these filters."));
            } else {
                let mut ul = kit::card(&t).overflow_hidden();
                for (n, row) in rows.iter().enumerate() {
                    if n > 0 {
                        ul = ul.child(kit::divider(&t));
                    }
                    let b = m.backlog.rows.iter().find(|b| fmt::ref_of(b, "B") == row.r).cloned().unwrap_or(Value::Null);
                    ul = ul.child(row_el(&t, row, &b, n, cx));
                }
                list = list.child(ul);
            }
        }
    }

    // ---- the issue beside it
    let aside_el = match &v.aside {
        AsideState::None => None,
        AsideState::Loading(e) => Some(ip::render_aside(m, &t, None, e.as_deref(), window, cx)),
        AsideState::Issue(view) => {
            let b = aside.as_ref().and_then(|(_, g)| g.as_ref()).and_then(|g| g.as_ref().ok()).cloned();
            Some(ip::render_aside(m, &t, b.as_ref().map(|b| (b, &**view)), None, window, cx))
        }
    };

    // ---- menus
    let st = &m.backlog;
    let mut menus: Vec<AnyElement> = Vec::new();
    let cur_state = st.f_state().to_string();
    let sopts: Vec<(&str, &str)> = STATES.iter().copied().filter(|(k, _)| v.state_options.contains(k)).collect();
    menus.extend(option_menu(m, &t, "bl-state", sopts, &cur_state, cx, |st, v| st.state = v));
    let cur_sort = st.f_sort().to_string();
    menus.extend(option_menu(m, &t, "bl-sort", SORTS.to_vec(), &cur_sort, cx, |st, v| st.sort = v));
    menus.extend(ip::render_picker(m, &t, window, cx));

    div()
        .id("backlog-page")
        .flex()
        .flex_col()
        .flex_1()
        .min_w_0()
        .size_full()
        .overflow_y_scroll()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(16.))
                .w_full()
                .max_w(px(1320.))
                .px(px(28.))
                .pt(px(22.))
                .pb(px(40.))
                .child(header)
                .child(filters)
                .child(div().flex().items_start().gap(px(20.)).child(list).children(aside_el.map(|a| div().flex_none().w(px(420.)).child(a)))),
        )
        .children(menus)
        .into_any_element()
}

/// One row: kind and title; goal, project, who found it; then actions or the state chip, and
/// its note. Clicking it shows the issue beside the list (`pick-issue`).
fn row_el(t: &Theme, v: &RowView, b: &Value, n: usize, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let hover = t.tint;
    let l1 = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .min_w_0()
        .child({
            let (fg, bg) = ip::kind_colors(t, s(b, "kind"));
            kit::pill(fg, bg, v.kind.clone())
        })
        .child(div().min_w_0().truncate().text_size(px(14.)).font_weight(FontWeight::BOLD).child(v.title.clone()));
    let l2 = div()
        .flex()
        .items_center()
        .gap(px(10.))
        .min_w_0()
        .text_size(px(12.5))
        .text_color(t.muted)
        .child(match &v.goal {
            Some(g) => div().flex_none().max_w(px(220.)).truncate().text_color(t.goal).child(g.clone()),
            None => div().flex_none().text_color(t.faint).child("Not in a goal"),
        })
        .child(kit::mono(t, v.project.clone()).flex_none().text_color(t.muted))
        .child(div().min_w_0().truncate().child(v.from.clone()));
    let mut acts = div().flex().flex_none().flex_col().items_end().gap(px(4.));
    if !v.acts.is_empty() {
        acts = acts.child(ip::action_row(t, &v.r, &v.acts, true, "bl", cx));
    }
    if let Some(text) = &v.state_chip {
        let (fg, bg) = ip::state_colors(t, b);
        acts = acts.child(kit::pill(fg, bg, text.clone()));
    }
    acts = acts.children(ip::note_el(t, &v.note));
    let r = v.r.clone();
    div()
        .id(("bl-row", n))
        .flex()
        .items_center()
        .gap(px(14.))
        .px(px(16.))
        .py(px(11.))
        .cursor_pointer()
        .when(v.selected, |d| d.bg(t.accent_soft))
        .when(!v.selected, |d| d.hover(move |s| s.bg(hover)))
        .child(div().flex().flex_col().gap(px(4.)).flex_1().min_w_0().child(l1).child(l2))
        .child(acts)
        .on_click(cx.listener(move |m, _, _, cx| {
            m.backlog.selected = Some(r.clone());
            m.backlog.synced = None;
            cx.notify();
        }))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's `test` attribute, which `#[gpui_kit::test]` then recurses on.
    use super::{Ctx, ListView, PageInput, State, bl_state_text, enter, filters_changed, ip, page_view, stale, with_kept};
    use crate::app::Page;
    use crate::backend::Backend;
    use crate::fmt::{self, arr, s};
    use crate::parity::{self, golden};
    use serde_json::{Value, json};
    use std::collections::{HashMap, HashSet};
    use std::time::Instant;

    #[::core::prelude::v1::test]
    fn state_text_matches_web() {
        golden("backlog").only("blStateText").check(|i| json!(bl_state_text(&i["issue"])));
    }

    #[::core::prelude::v1::test]
    fn page_matches_web() {
        golden("backlog").only("page").check(|i| {
            let f = &i["filter"];
            let mut st = State {
                project: s(f, "project").into(),
                goal: s(f, "goal").into(),
                kind: s(f, "kind").into(),
                state: s(f, "state").into(),
                sort: s(f, "sort").into(),
                ..Default::default()
            };
            st.open_all = i["openAll"].as_i64();
            let mut kept: HashMap<String, Value> = i["kept"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default();
            let order: Vec<String> = arr(i, "order").iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            let backlog = i.get("backlog").filter(|b| b.is_object());
            let rows = backlog.map(|b| with_kept(arr(b, "issues"), &mut kept, &order)).unwrap_or_default();
            let now = Instant::now();
            let notes: HashMap<String, (String, bool, Instant)> = i["notes"].as_object().map(|o| o.iter().map(|(k, n)| (k.clone(), (s(n, "text").to_string(), fmt::b(n, "err"), now))).collect()).unwrap_or_default();
            let busy: HashSet<String> = arr(i, "busy").iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            let reveal: HashSet<String> = arr(i, "reveal").iter().filter_map(|x| x.as_str().map(str::to_string)).collect();
            let drafts: HashMap<String, String> = i["drafts"].as_object().map(|o| o.iter().map(|(k, v)| (k.clone(), v.as_str().unwrap_or("").to_string())).collect()).unwrap_or_default();
            let goals = arr(i, "goals").to_vec();
            let state = json!({"projects": i["projects"], "counts": i["counts"]});
            let aside = fmt::opt_s(i, "selected").map(|r| (r.to_string(), Some(Ok(i["issue"].clone()))));
            let v = page_view(&PageInput {
                st: &st,
                ctx: Ctx { jira: fmt::b(i, "jira"), goals: &goals, notes: &notes, busy: &busy, reveal: &reveal, drafts: &drafts },
                state: &state,
                backlog,
                backlog_err: i["backlogErr"].as_str(),
                rows,
                aside,
            });
            let rows: Vec<Value> = match &v.list {
                ListView::Rows { rows, .. } => rows
                    .iter()
                    .map(|r| {
                        let (text, acts) = r.text_and_acts();
                        json!({"text": text, "acts": acts})
                    })
                    .collect(),
                ListView::Loading(_) => vec![],
            };
            let (aside, aside_acts) = v.aside_text_and_acts();
            json!({
                "header": v.header_text(), "header_acts": ["add-issue"],
                "filters": v.filters_text(), "filter_values": [v.project_label, v.goal_label],
                "state_options": v.state_options, "list_head": v.list_head(),
                "rows": rows, "aside": aside, "aside_acts": aside_acts,
            })
        });
    }

    #[::core::prelude::v1::test]
    fn query_sends_goal_ids() {
        let st = State { goal: "G12".into(), state: "all".into(), ..Default::default() };
        assert_eq!(st.query(), vec![("project", "all".into()), ("goal", "12".into()), ("kind", "all".into()), ("state", "all".into()), ("sort", "new".into())]);
        assert!(!st.is_default());
        let st = State { goal: "none".into(), ..Default::default() };
        assert_eq!(st.query()[1].1, "none");
        assert!(State::default().is_default());
    }

    #[::core::prelude::v1::test]
    fn kept_rows_go_back_in_place_and_refresh() {
        let b = |r: &str, st: &str| json!({"ref": r, "state": st});
        let mut kept: HashMap<String, Value> = [("B6".to_string(), b("B6", "open")), ("B1".to_string(), b("B1", "open"))].into_iter().collect();
        let order: Vec<String> = ["B6", "B1", "B2", "B8"].iter().map(|x| x.to_string()).collect();
        let rows = with_kept(&[b("B1", "task"), b("B8", "open")], &mut kept, &order);
        assert_eq!(rows.iter().map(|x| s(x, "ref").to_string()).collect::<Vec<_>>(), ["B6", "B1", "B8"]);
        // A kept row still in the answer takes the fresher copy.
        assert_eq!(s(&kept["B1"], "state"), "task");
    }

    #[gpui_kit::test]
    fn row_actions_and_filters_send_what_the_web_sent(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = parity::window(cx);
        let mk = |body: Value| {
            let v = rec.board().post("backlog", body).unwrap();
            fmt::ref_of(v.get("issue").unwrap_or(&v), "B")
        };
        let in_goal = mk(json!({"title": "Goal gap", "kind": "gap", "project": "webapp", "goal_id": 1}));
        let loose = mk(json!({"title": "Loose bug", "kind": "bug", "project": "api"}));
        w.update(cx, |m, _, cx| {
            m.go(Page::Backlog, cx);
            enter(m, None);
        })
        .unwrap();
        parity::settle(cx);
        w.update(cx, |m, _, cx| {
            // The first row shows beside the list until another is picked.
            assert!(m.data.backlog.is_some());
            ip::promote(m, &in_goal, "goal", cx);
        })
        .unwrap();
        parity::settle(cx);
        assert_eq!(rec.last(&format!("backlog/{in_goal}/promote")), Some(json!({"where": "goal"})));
        w.update(cx, |m, _, cx| {
            // Off the Board a promote leaves no note, and the row stays where it was.
            assert!(!m.issue_panel.notes.contains_key(&format!("issue:{in_goal}")));
            assert!(m.backlog.kept.contains_key(&in_goal));
            assert!(m.backlog.rows.iter().any(|b| fmt::ref_of(b, "B") == in_goal));
            ip::promote(m, &loose, "board", cx);
        })
        .unwrap();
        parity::settle(cx);
        assert_eq!(rec.last(&format!("backlog/{loose}/promote")), Some(json!({"where": "board"})));
        w.update(cx, |m, _, cx| {
            assert!(matches!(m.panel, None), "off the Board the new task doesn't open");
            m.backlog.goal = "G1".into();
            filters_changed(m, cx);
            assert!(m.backlog.kept.is_empty(), "new filters forget kept rows");
            assert_eq!(m.backlog.query()[1], ("goal", "1".to_string()));
        })
        .unwrap();
        parity::settle(cx);
        // Not the defaults: "N open" comes from a second, default-filter fetch.
        w.update(cx, |m, _, cx| {
            stale(m);
            cx.notify();
        })
        .unwrap();
        parity::settle(cx);
        w.update(cx, |m, _, _| assert!(m.backlog.open_all.is_some())).unwrap();
        assert!(rec.posts().iter().all(|(p, _)| !p.contains("bulk")), "the Backlog page has no bulk actions");
    }
}
