//! The Review page: one weekly time review (`GET /reviews/:id`) with the same sections as
//! `tb review`, a picker for earlier weeks, and its suggested fixes as backlog issues. Read-only:
//! opening a review marks it seen (its Needs you item goes), and Plan a goal hands the open fixes
//! to the Backlog page, checked.
use crate::app::{MainWindow, Page};
use crate::fmt::{self, arr, s};
use crate::theme::Theme;
use crate::ui::kit;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};
use std::collections::HashSet;

const MENU_WEEK: &str = "review-week";

#[derive(Default)]
pub struct State {
    /// The review shown; `None` for the newest.
    pub id: Option<i64>,
    /// Reviews already marked seen from here, so each is sent once.
    marked: HashSet<i64>,
}

// ------------------------------------------------------------------ view model

/// "3.2 h", "12 min", "40 s" (`tb review`'s `took`).
pub fn took(secs: f64) -> String {
    let secs = secs.abs();
    if secs >= 3600.0 {
        format!("{:.1} h", secs / 3600.0)
    } else if secs >= 60.0 {
        format!("{} min", (secs / 60.0).round() as i64)
    } else {
        format!("{} s", secs.round() as i64)
    }
}

fn signed(secs: f64) -> String {
    format!("{}{}", if secs < 0.0 { "-" } else { "+" }, took(secs))
}

fn secs(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}

/// "Oct 5 – Oct 12": a review's week, in the reader's zone.
pub fn period(r: &Value) -> String {
    let day = |k: &str| fmt::parse(s(r, k)).map(|t| fmt::local(t).format("%b %-d").to_string()).unwrap_or_default();
    format!("{} – {}", day("period_start"), day("period_end"))
}

/// One row of a section: what it is, how long, and a line under it.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    /// A task ref to open, when the row is a task.
    pub task: Option<String>,
    pub name: String,
    pub took: String,
    pub sub: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fix {
    pub r: String,
    pub title: String,
    pub kind: String,
    /// "In G4", "Done", "Dropped": where it went once triaged; "" while it waits on the backlog.
    pub went: String,
    /// Open and in no goal: Plan a goal checks it.
    pub untriaged: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub id: i64,
    pub period: String,
    pub total: String,
    pub change: Option<String>,
    /// (name, time, share 0..1)
    pub categories: Vec<(String, String, f32)>,
    pub projects: Vec<Line>,
    pub slowest: Vec<Line>,
    pub repeated: Vec<Line>,
    pub timeouts: Vec<Line>,
    pub reruns: Vec<Line>,
    /// "2.1 h" and its reasons ("question 1.2 h, review 50 min").
    pub wait: Option<(String, String)>,
    pub fixes: Vec<Fix>,
    /// While no fix is filed: the running/failed line, else the plain suggestions.
    pub fixes_note: Option<String>,
    pub suggestions: Vec<(String, String)>,
}

impl View {
    /// The fixes Plan a goal checks on the Backlog page.
    pub fn plan(&self) -> Vec<String> {
        self.fixes.iter().filter(|f| f.untriaged).map(|f| f.r.clone()).collect()
    }
}

pub fn view(r: &Value) -> View {
    let d = &r["data"];
    let total = secs(&d["seconds"]);
    let change = d["change"].is_object().then(|| {
        let c = &d["change"];
        format!(
            "Since review {}: {} agent time, {} waiting on you, {:+} timeouts, {:+} reruns",
            c["review"],
            signed(secs(&c["seconds"])),
            signed(secs(&c["owner_wait"])),
            c["timeouts"].as_i64().unwrap_or(0),
            c["reruns"].as_i64().unwrap_or(0)
        )
    });
    let categories = arr(d, "categories")
        .iter()
        .map(|x| (s(x, "name").to_string(), took(secs(&x["seconds"])), if total > 0.0 { (secs(&x["seconds"]) / total) as f32 } else { 0.0 }))
        .collect();
    let line = |task: Option<&str>, name: &str, t: f64, sub: String| Line { task: task.filter(|t| !t.is_empty()).map(str::to_string), name: name.to_string(), took: took(t), sub };
    let projects = arr(d, "projects").iter().map(|x| line(None, s(x, "name"), secs(&x["seconds"]), format!("mostly {}", s(&x["categories"][0], "name")))).collect();
    let slowest = arr(d, "slowest_tasks")
        .iter()
        .map(|x| line(Some(s(x, "task")), &format!("{} {}", s(x, "task"), s(x, "title")), secs(&x["seconds"]), format!("mostly {}", s(x, "top_category"))))
        .collect();
    let repeated = arr(d, "repeated")
        .iter()
        .map(|x| line(None, s(x, "command"), secs(&x["median"]), format!("median · p90 {} · {} runs", took(secs(&x["p90"])), fmt::i(x, "runs"))))
        .collect();
    let timeouts = arr(&d["timeouts"], "commands")
        .iter()
        .map(|x| line(None, s(x, "command"), secs(&x["seconds"]), fmt::plural(fmt::i(x, "count"), "timeout", "timeouts")))
        .collect();
    let reruns = arr(d, "reruns")
        .iter()
        .map(|x| {
            let end = if x["passed"] == true { "then passed" } else { "never passed" };
            line(Some(s(x, "task")), s(x, "command"), secs(&x["seconds"]), format!("{} · {} runs, {end}", s(x, "task"), fmt::i(x, "runs")))
        })
        .collect();
    let w = &d["owner_wait"];
    let wait = (secs(&w["seconds"]) > 0.0).then(|| {
        let why: Vec<String> = arr(w, "reasons").iter().map(|x| format!("{} {}", s(x, "name"), took(secs(&x["seconds"])))).collect();
        (took(secs(&w["seconds"])), why.join(", "))
    });
    let fixes: Vec<Fix> = arr(r, "fix_issues")
        .iter()
        .map(|b| {
            let state = s(b, "state");
            let goal = s(b, "goal");
            let went = match state {
                "open" if goal.is_empty() => String::new(),
                "open" => format!("In {goal}"),
                st => fmt::cap(st),
            };
            Fix { r: fmt::ref_of(b, "B"), title: s(b, "title").to_string(), kind: s(b, "kind_label").to_string(), untriaged: went.is_empty(), went }
        })
        .collect();
    let f = &r["fixes"];
    let fixes_note = match s(f, "state") {
        _ if !fixes.is_empty() => None,
        "running" => Some("Filing these as backlog issues…".to_string()),
        "failed" => Some(format!("Couldn’t file these as backlog issues: {}", s(f, "error"))),
        _ => None,
    };
    let suggestions = if fixes.is_empty() { arr(d, "suggestions").iter().map(|x| (s(x, "title").to_string(), s(x, "detail").to_string())).collect() } else { vec![] };
    View {
        id: r["id"].as_i64().unwrap_or(0),
        period: period(r),
        total: took(total),
        change,
        categories,
        projects,
        slowest,
        repeated,
        timeouts,
        reruns,
        wait,
        fixes,
        fixes_note,
        suggestions,
    }
}

/// The review to show: the picked one, else the newest in the list.
pub fn current(m: &MainWindow) -> Option<i64> {
    m.review.id.or_else(|| m.data.reviews.as_ref().and_then(|l| arr(l, "reviews").first()).and_then(|r| r["id"].as_i64()))
}

// ------------------------------------------------------------------ actions

/// Open review `id` (`None`: the newest), from a link, Home or the Days page.
pub fn open(m: &mut MainWindow, id: Option<i64>, cx: &mut Context<MainWindow>) {
    if m.review.id != id || id.is_none() {
        m.data.review = None;
        m.data.errs.review = None;
    }
    m.review.id = id;
    m.go(Page::Review, cx);
    m.refresh(cx);
}

fn pick(m: &mut MainWindow, id: i64, cx: &mut Context<MainWindow>) {
    m.menu = None;
    if current(m) != Some(id) {
        m.data.review = None;
        m.data.errs.review = None;
    }
    m.review.id = Some(id);
    m.refresh(cx);
}

/// Opening an unseen review clears its Needs you item and withdraws its notification.
fn mark_seen(m: &mut MainWindow, r: &Value, cx: &mut Context<MainWindow>) {
    let Some(id) = r["id"].as_i64() else { return };
    if !r["seen_at"].is_null() || !m.review.marked.insert(id) {
        return;
    }
    m.post(format!("reviews/{id}/seen"), json!({}), cx, |m, v, cx| {
        if m.data.review.as_ref().and_then(|r| r["id"].as_i64()) == v["id"].as_i64() {
            m.data.review = Some(v);
        }
        cx.notify();
    });
}

/// Plan a goal: the Backlog page with the review's open fixes checked; planning, Create and
/// Start go on from there as for any picked issues.
fn plan(m: &mut MainWindow, refs: Vec<String>, cx: &mut Context<MainWindow>) {
    m.backlog.picked = refs;
    m.backlog.project = "all".into();
    m.backlog.kinds = [true; 4];
    m.go(Page::Backlog, cx);
}

// ------------------------------------------------------------------ render

fn section(t: &Theme, title: &str) -> Div {
    kit::card(t).flex().flex_col().gap(px(8.)).child(kit::h3(t, title.to_string()))
}

fn line_el(t: &Theme, id: String, l: &Line, mono: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let name = if mono { kit::mono(t, l.name.clone()) } else { div().text_size(px(13.)).child(l.name.clone()) };
    let row = div()
        .id(SharedString::from(id))
        .flex()
        .items_start()
        .gap(px(12.))
        .py(px(3.))
        .child(div().w(px(64.)).flex_none().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(l.took.clone()))
        .child(div().flex().flex_col().flex_1().min_w_0().gap(px(1.)).child(name.overflow_hidden().text_ellipsis()).child(div().text_size(px(12.)).text_color(t.muted).child(l.sub.clone())));
    match l.task.clone() {
        Some(r) => row.cursor_pointer().hover(|d| d.bg(t.panel_2)).rounded(px(6.)).on_click(cx.listener(move |m, _, _, cx| m.open_task(r.clone(), cx))),
        None => row,
    }
}

fn lines(t: &Theme, key: &str, title: &str, list: &[Line], mono: bool, cx: &mut Context<MainWindow>) -> Option<Div> {
    (!list.is_empty()).then(|| section(t, title).children(list.iter().enumerate().map(|(n, l)| line_el(t, format!("review-{key}-{n}"), l, mono, cx))))
}

pub fn render(m: &mut MainWindow, _window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let shell = div().id("review-page").flex().flex_col().flex_1().min_w_0().size_full().overflow_y_scroll();
    let list: Vec<Value> = m.data.reviews.as_ref().map(|l| arr(l, "reviews").to_vec()).unwrap_or_default();
    let want = current(m);
    let shown = m.data.review.clone().filter(|r| r["id"].as_i64().is_some() && r["id"].as_i64() == want);
    let Some(r) = shown else {
        let msg = match (&m.data.errs.review, &m.data.reviews) {
            (Some(e), _) => e.clone(),
            (None, Some(_)) if list.is_empty() => "No reviews yet".into(),
            _ => "Loading…".into(),
        };
        return shell.child(div().p(px(28.)).child(kit::empty(&t, msg))).into_any_element();
    };
    mark_seen(m, &r, cx);
    let v = view(&r);

    // ---- header: the week, earlier and later weeks, and a picker for any of them
    let at = list.iter().position(|x| x["id"].as_i64() == Some(v.id));
    let older = at.and_then(|i| list.get(i + 1)).and_then(|x| x["id"].as_i64());
    let newer = at.and_then(|i| i.checked_sub(1)).and_then(|i| list.get(i)).and_then(|x| x["id"].as_i64());
    let nav = div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(kit::btn(&t, "review-older", "‹").w(px(36.)).when(older.is_none(), kit::disabled).tooltip(kit::tip("Earlier week")).on_click(cx.listener(move |m, _, _, cx| {
            if let Some(id) = older {
                pick(m, id, cx)
            }
        })))
        .child(kit::btn(&t, "review-newer", "›").w(px(36.)).when(newer.is_none(), kit::disabled).tooltip(kit::tip("Later week")).on_click(cx.listener(move |m, _, _, cx| {
            if let Some(id) = newer {
                pick(m, id, cx)
            }
        })))
        .child(kit::btn(&t, "review-week-btn", "Weeks  ▾").on_click(cx.listener(|m, e: &ClickEvent, _, cx| {
            let p = e.position();
            m.toggle_menu(MENU_WEEK, point(p.x - px(160.), p.y + px(14.)), cx);
        })));
    let header = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(14.))
        .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child(v.period.clone()))
        .child(div().text_size(px(14.)).text_color(t.text_2).child(format!("{} of agent time", v.total)))
        .child(div().flex_1())
        .child(nav);

    // ---- where the time went
    let cats = (!v.categories.is_empty()).then(|| {
        section(&t, "By category").children(v.categories.iter().map(|(name, took, share)| {
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(div().w(px(130.)).flex_none().text_size(px(13.)).child(name.clone()))
                .child(div().flex_1().h(px(8.)).rounded_full().bg(t.seg).child(div().h_full().rounded_full().w(relative(share.clamp(0.0, 1.0))).bg(t.accent)))
                .child(div().w(px(64.)).flex_none().text_size(px(13.)).text_align(TextAlign::Right).child(took.clone()))
                .child(div().w(px(40.)).flex_none().text_size(px(12.)).text_color(t.muted).text_align(TextAlign::Right).child(format!("{:.0}%", share * 100.0)))
        }))
    });
    let wait = v.wait.clone().map(|(time, why)| {
        section(&t, "Waiting on you").child(div().text_size(px(13.)).child(div().font_weight(FontWeight::SEMIBOLD).child(time)).child(div().text_size(px(12.)).text_color(t.muted).child(why)))
    });

    // ---- suggested fixes
    let refs = v.plan();
    let mut fixes = section(&t, "Suggested fixes");
    if !refs.is_empty() {
        let n = refs.len();
        fixes = fixes.child(
            div().flex().child(kit::btn_primary(&t, "review-plan", "Plan a goal").tooltip(kit::tip(format!("Check {} on the Backlog page", fmt::plural(n as i64, "fix", "fixes")))).on_click(
                cx.listener(move |m, _, _, cx| plan(m, refs.clone(), cx)),
            )),
        );
    }
    fixes = fixes.children(v.fixes.iter().map(|f| {
        let r = f.r.clone();
        div()
            .id(SharedString::from(format!("review-fix-{}", f.r)))
            .flex()
            .items_center()
            .gap(px(10.))
            .py(px(4.))
            .px(px(6.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|d| d.bg(t.panel_2))
            .child(kit::mono(&t, f.r.clone()).flex_none())
            .child(div().flex_1().min_w_0().text_size(px(13.)).overflow_hidden().text_ellipsis().child(f.title.clone()))
            .when(!f.kind.is_empty(), |d| d.child(kit::chip(&t, f.kind.clone())))
            .when(!f.went.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(t.muted).child(f.went.clone())))
            .on_click(cx.listener(move |m, _, _, cx| m.open_issue(r.clone(), cx)))
    }));
    fixes = fixes.children(v.fixes_note.clone().map(|n| div().text_size(px(13.)).text_color(t.muted).child(n)));
    fixes = fixes.children(
        v.suggestions.iter().map(|(title, detail)| div().flex().flex_col().gap(px(1.)).child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(title.clone())).child(div().text_size(px(12.)).text_color(t.muted).child(detail.clone()))),
    );
    let has_fixes = !v.fixes.is_empty() || v.fixes_note.is_some() || !v.suggestions.is_empty();

    let left = div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .flex_1()
        .min_w(px(420.))
        .children(cats)
        .children(lines(&t, "slow", "Slowest tasks", &v.slowest, false, cx))
        .children(lines(&t, "rep", "Slowest repeated commands", &v.repeated, true, cx))
        .children(lines(&t, "to", "Timeouts", &v.timeouts, true, cx))
        .children(lines(&t, "rerun", "Failed, then rerun", &v.reruns, true, cx));
    let rail = div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .w(px(340.))
        .flex_none()
        .when(has_fixes, |d| d.child(fixes))
        .children(wait)
        .children(lines(&t, "proj", "By project", &v.projects, false, cx));

    let mut menus: Vec<AnyElement> = vec![];
    if let Some(at) = m.menu_open(MENU_WEEK) {
        let mut menu = kit::menu_box(&t, 220.).id("review-week-menu");
        for x in &list {
            let Some(id) = x["id"].as_i64() else { continue };
            let label = format!("{} · {}", period(x), took(secs(&x["seconds"])));
            menu = menu.child(kit::menu_item(&t, SharedString::from(format!("review-week-{id}")), label, id == v.id).on_click(cx.listener(move |m, _, _, cx| pick(m, id, cx))));
        }
        menus.push(kit::popover(at, menu.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())));
    }

    shell
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(16.))
                .w_full()
                .px(px(28.))
                .py(px(22.))
                .child(header)
                .children(v.change.clone().map(|c| div().text_size(px(13.)).text_color(t.text_2).child(c)))
                .child(div().flex().flex_wrap().items_start().gap(px(16.)).child(left).child(rail)),
        )
        .children(menus)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn review() -> Value {
        json!({
            "id": 4, "period_start": "2026-10-05T00:00:00Z", "period_end": "2026-10-12T00:00:00Z", "seen_at": null,
            "fixes": {"state": "done", "filed": ["B12", "B13"], "linked": ["B9"]},
            "data": {
                "seconds": 36000.0,
                "change": {"review": 3, "seconds": -1800.0, "owner_wait": 600.0, "timeouts": -2, "reruns": 1},
                "categories": [{"name": "build", "seconds": 18000.0}, {"name": "test", "seconds": 9000.0}],
                "projects": [{"name": "app", "seconds": 30000.0, "categories": [{"name": "build", "seconds": 20000.0}]}],
                "slowest_tasks": [{"task": "T12", "title": "Make it fast", "seconds": 7200.0, "top_category": "test"}],
                "repeated": [{"command": "cargo test", "runs": 9, "median": 300.0, "p90": 540.0}],
                "timeouts": {"count": 2, "commands": [{"command": "cargo build", "count": 2, "seconds": 1200.0}]},
                "reruns": [{"command": "cargo test -p x", "task": "T12", "runs": 3, "seconds": 900.0, "passed": true}],
                "owner_wait": {"seconds": 5400.0, "reasons": [{"name": "question", "seconds": 3600.0}, {"name": "review", "seconds": 1800.0}]},
                "suggestions": [{"title": "Cap builds", "detail": "Too many at once"}]
            },
            "fix_issues": [
                {"id": 12, "ref": "B12", "title": "Cap concurrent builds", "kind_label": "Gap", "state": "open", "goal": null, "linked": false},
                {"id": 13, "ref": "B13", "title": "Warm caches", "kind_label": "Follow-up", "state": "open", "goal": "G40", "linked": false},
                {"id": 9, "ref": "B9", "title": "Long test timeouts", "kind_label": "Bug", "state": "done", "goal": null, "linked": true}
            ]
        })
    }

    #[::core::prelude::v1::test]
    fn a_review_reads_as_tb_review_does() {
        let v = view(&review());
        assert_eq!(v.period, "Oct 5 – Oct 12");
        assert_eq!(v.total, "10.0 h");
        assert_eq!(v.change.as_deref(), Some("Since review 3: -30 min agent time, +10 min waiting on you, -2 timeouts, +1 reruns"));
        assert_eq!(v.categories[0], ("build".into(), "5.0 h".into(), 0.5));
        assert_eq!(v.projects[0].sub, "mostly build");
        assert_eq!(v.slowest[0], Line { task: Some("T12".into()), name: "T12 Make it fast".into(), took: "2.0 h".into(), sub: "mostly test".into() });
        assert_eq!(v.repeated[0].took, "5 min");
        assert_eq!(v.repeated[0].sub, "median · p90 9 min · 9 runs");
        assert_eq!(v.timeouts[0].sub, "2 timeouts");
        assert_eq!(v.reruns[0].sub, "T12 · 3 runs, then passed");
        assert_eq!(v.wait, Some(("1.5 h".into(), "question 1.0 h, review 30 min".into())));
    }

    #[::core::prelude::v1::test]
    fn plan_a_goal_checks_only_the_fixes_still_on_the_backlog() {
        let v = view(&review());
        let went: Vec<&str> = v.fixes.iter().map(|f| f.went.as_str()).collect();
        assert_eq!(went, ["", "In G40", "Done"]);
        assert_eq!(v.plan(), ["B12"]);
        // Filed fixes replace the plain suggestions.
        assert!(v.suggestions.is_empty() && v.fixes_note.is_none());
    }

    #[::core::prelude::v1::test]
    fn with_nothing_filed_the_suggestions_show_and_say_why() {
        let mut r = review();
        r["fix_issues"] = json!([]);
        r["fixes"] = json!({"state": "failed", "error": "Claude timed out"});
        let v = view(&r);
        assert_eq!(v.fixes_note.as_deref(), Some("Couldn’t file these as backlog issues: Claude timed out"));
        assert_eq!(v.suggestions, [("Cap builds".to_string(), "Too many at once".to_string())]);
        assert!(v.plan().is_empty());
        r["fixes"] = json!({"state": "none"});
        assert_eq!(view(&r).fixes_note, None);
    }
}
