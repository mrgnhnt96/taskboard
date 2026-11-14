//! How the period went: day by day, against the one before, by week, by project, and what was
//! learned and found along the way.
use super::parts::*;
use crate::app::MainWindow;
use crate::fmt::{arr, cap, s};
use crate::ui::{kit, motion};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

/// A toggle for the page's open set.
fn flip(key: String, cx: &mut Context<MainWindow>) -> Click {
    Box::new(cx.listener(move |m, _, _, cx| super::toggle(m, &key, cx)))
}

/// The releases cut on a day ("2026-11-02").
fn cut_on<'a>(c: &'a Ctx, date: &str) -> Vec<&'a Value> {
    arr(&c.facts, "releases").iter().filter(|r| fmt_date(s(r, "cut_at")) == date).collect()
}

pub fn days(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let t = &c.t;
    let list: Vec<Value> = c.prop(comp, "days", "days").as_array().cloned().unwrap_or_default();
    if list.iter().all(|d| num(&d["tasks"]) <= 0.) {
        return Some(card(c, "days", Some("Day by day"), None, vec![none(t, "0 tasks")]));
    }
    let on = (0..list.len()).find(|i| c.is_open(&format!("day|{i}")));
    let rel: Vec<usize> = list.iter().map(|d| cut_on(c, s(d, "date")).len()).collect();
    let cs: Vec<Col> = list
        .iter()
        .zip(&rel)
        .map(|(d, r)| {
            let mut tip = format!("{} {}: {}", weekday(s(d, "date")), day(s(d, "date")), pl(num(&d["tasks"]), "task", "tasks"));
            if *r > 0 {
                tip.push_str(&format!(", {}", pl(*r as f64, "release", "releases")));
            }
            Col { value: num(&d["tasks"]), tip, dim: false }
        })
        .collect();
    let clicks = (0..list.len()).map(|i| flip(format!("day|{i}"), cx)).collect();
    let rc = category(t, "git").1;
    let ticks = div().flex().gap(px(2.)).h(px(8.)).children(rel.iter().enumerate().map(|(i, r)| {
        let cell = div().flex_1().min_w_0().flex().justify_center();
        if *r > 0 { cell.child(motion::op(div().size(px(6.)).rounded_full().bg(rc), c.id(format!("days-tick{i}")), i.min(8) as u64 * 20 + 240)) } else { cell }
    }));
    let labels = list.iter().map(|d| if c.month { day(s(d, "date")) } else { weekday(s(d, "date")) }).collect();
    let mut leg = vec![(t.accent, "Tasks".to_string())];
    if rel.iter().any(|r| *r > 0) {
        leg.push((rc, "Release".into()));
    }
    let mut body = vec![
        div().flex().flex_col().gap(px(4.)).child(cols(c, "days", 120., 20, 0., &cs, on, clicks)).child(ticks).child(axis(t, labels)).into_any_element(),
        legend(t, leg).into_any_element(),
    ];
    if let Some(i) = on {
        let d = &list[i];
        let rs = cut_on(c, s(d, "date"));
        let rows: Vec<AnyElement> = rs
            .iter()
            .enumerate()
            .map(|(j, r)| {
                let el = lrow(t, c.id(format!("days-r{i}-{j}")), named(c, s(r, "project"), s(r, "name")), when(s(r, "cut_at")), Some(go_release(s(r, "project"), s(r, "release"), cx)));
                motion::fade(el, c.id(format!("days-rin{i}-{j}")), motion::st(j, 24, 0))
            })
            .collect();
        let stats = stat3(t, vec![("Tasks".into(), int(num(&d["tasks"]))), ("Merged".into(), int(num(&d["merged"]))), ("Commits".into(), int(num(&d["commits"]))), ("Agent time".into(), dur(num(&d["agent_min"])))]);
        let inner = div().flex().flex_col().gap(px(8.)).child(lbl(t, format!("{}, {}", weekday(s(d, "date")), day(s(d, "date"))))).child(stats).child(div().flex().flex_col().children(rows));
        body.push(opened(c, &format!("day|{i}"), 120. + 32. * rs.len() as f32, inner));
    }
    Some(card(c, "days", Some("Day by day"), None, body))
}

/// The numbers before and after compare.
fn measures(c: &Ctx) -> Vec<(&'static str, f64, f64, fn(f64) -> String)> {
    let cur = c.fact("totals");
    let prev = c.before();
    let p = |k: &str| prev.map_or(0., |b| num(&b[k]));
    vec![
        ("Tasks", num(&cur["tasks"]), p("tasks"), int as fn(f64) -> String),
        ("PRs", num(&cur["prs"]), p("prs"), int),
        ("Releases", num(&cur["releases"]), p("releases"), int),
        ("Goals", num(&cur["goals_finished"]), p("goals_finished"), int),
        ("Agent time", num(&cur["agent_min"]), p("agent_min"), dur),
    ]
}

pub fn ba(c: &Ctx, _comp: &Value, _cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let t = &c.t;
    let m = measures(c);
    if m.iter().all(|x| x.1 == 0. && x.2 == 0.) {
        return Some(card(c, "ba", Some("Before and after"), None, vec![none(t, "0 in both periods")]));
    }
    let prev = c.prev();
    let rows: Vec<AnyElement> = m
        .iter()
        .enumerate()
        .map(|(i, (name, cur, was, f))| {
            let top = cur.max(*was).max(1e-4);
            let bar = |k: &str, delay: u64, v: f64, color: Hsla| div().flex().w_full().h(px(6.)).child(motion::grow_w(c.id(format!("ba{i}-{k}")), delay, motion::GROW_W, (v / top).max(0.004) as f32, color, 6.));
            let pair = div()
                .id(c.id(format!("ba{i}-pair")))
                .flex()
                .flex_col()
                .justify_center()
                .gap(px(2.))
                .flex_1()
                .min_w_0()
                .child(bar("p", motion::st(i, 40, 120), *was, grey(t)))
                .child(bar("c", motion::st(i, 40, 200), *cur, t.accent))
                .tooltip(kit::tip(format!("{name}: {} {prev}, {} now", f(*was), f(*cur))));
            let vals = div()
                .flex()
                .flex_none()
                .w(px(120.))
                .justify_end()
                .items_baseline()
                .gap(px(8.))
                .whitespace_nowrap()
                .child(div().text_size(px(12.)).text_color(t.muted).child(f(*was)))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(f(*cur)));
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .min_h(px(28.))
                .child(div().flex_none().w(px(80.)).text_color(t.text_2).child(name.to_string()))
                .child(pair)
                .child(vals)
                .into_any_element()
        })
        .collect();
    let body = vec![div().flex().flex_col().children(rows).into_any_element(), legend(t, vec![(grey(t), cap(prev)), (t.accent, "This period".into())]).into_any_element()];
    Some(card(c, "ba", Some("Before and after"), None, body))
}

pub fn trend(c: &Ctx, comp: &Value, _cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let t = &c.t;
    let weeks: Vec<Value> = c.prop(comp, "weeks", "weeks").as_array().cloned().unwrap_or_default();
    if weeks.is_empty() {
        return None;
    }
    let i0 = weeks.iter().position(|w| num(&w["prs"]) > 0. || num(&w["tasks"]) > 0.);
    let Some(i0) = i0 else {
        return Some(card(c, "trend", Some("PRs by week"), None, vec![none(t, "0 PRs")]));
    };
    let ws = &weeks[i0..];
    let cs: Vec<Col> = ws
        .iter()
        .map(|w| Col {
            value: num(&w["prs"]),
            tip: format!("Week of {}: {}, {}, {}", day(s(w, "first_day")), pl(num(&w["prs"]), "PR", "PRs"), pl(num(&w["tasks"]), "task", "tasks"), pl(num(&w["releases"]), "release", "releases")),
            dim: false,
        })
        .collect();
    let row = |v: Vec<String>, color: Hsla| div().flex().gap(px(2.)).text_size(px(12.)).text_color(color).children(v.into_iter().map(|x| div().flex_1().min_w_0().flex().justify_center().whitespace_nowrap().child(x)));
    let body = vec![
        div()
            .flex()
            .flex_col()
            .gap(px(4.))
            .child(cols(c, "trend", 120., 60, 0., &cs, None, vec![]))
            .child(row(ws.iter().map(|w| int(num(&w["prs"]))).collect(), t.text_2))
            .child(row(ws.iter().map(|w| day(s(w, "first_day"))).collect(), t.muted))
            .into_any_element(),
    ];
    Some(card(c, "trend", Some("PRs by week"), None, body))
}

pub fn lines(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let t = &c.t;
    let added = num(c.prop(comp, "added", "totals.added"));
    let removed = num(c.prop(comp, "removed", "totals.removed"));
    if added <= 0. && removed <= 0. {
        return Some(card(c, "lines", Some("Lines changed"), None, vec![none(t, "0 lines changed")]));
    }
    let gone = grey(t);
    let mut body = vec![
        stack(c, "lines", 80, 400, 12., vec![(added, t.accent, format!("Added: {}", int(added))), (removed, gone, format!("Removed: {}", int(removed)))]),
        legend(t, vec![(t.accent, "Added".into()), (gone, "Removed".into())]).into_any_element(),
    ];
    let mut gs: Vec<Value> = arr(&c.facts, "goals").iter().filter(|g| num(&g["added"]) + num(&g["removed"]) > 0.).cloned().collect();
    gs.sort_by(|a, b| (num(&b["added"]) + num(&b["removed"])).total_cmp(&(num(&a["added"]) + num(&a["removed"]))));
    gs.truncate(6);
    if !gs.is_empty() {
        let h = 32. * gs.len() as f32;
        body.extend(xp(c, "lines-goals", "Largest goals", None, h, cx, |cx| {
            div().flex().flex_col().children(
                gs.iter()
                    .enumerate()
                    .map(|(i, g)| {
                        let label = div().flex().items_center().gap(px(6.)).min_w_0().child(div().flex_none().font_family(t.mono_font.clone()).text_size(px(12.)).text_color(t.muted).child(s(g, "goal").to_string())).child(div().min_w_0().truncate().child(s(g, "name").to_string()));
                        let el = lrow(t, c.id(format!("lines-g{i}")), label, format!("+{} −{}", int(num(&g["added"])), int(num(&g["removed"]))), Some(go_goal(s(g, "goal"), cx)));
                        motion::fade(el, c.id(format!("lines-gin{i}")), motion::st(i, 24, 0))
                    })
                    .collect::<Vec<_>>(),
            )
        }));
    }
    Some(card(c, "lines", Some("Lines changed"), Some(right(t, format!("+{} −{}", int(added), int(removed)))), body))
}

pub fn board(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let t = &c.t;
    let ps = active(c.prop(comp, "projects", "projects"));
    if ps.is_empty() {
        return Some(card(c, "board", Some("Projects"), None, vec![none(t, "0 projects")]));
    }
    let max = num(&ps[0]["tasks"]).max(1.);
    let mut body = vec![];
    for (i, p) in ps.iter().take(8).enumerate() {
        let name = s(p, "name");
        let key = format!("board|{i}");
        let open = c.is_open(&key);
        let n = num(&p["tasks"]);
        body.push(brow(c, &format!("board{i}"), i, Brow::new(name, pl(n, "task", "tasks"), n / max).color(c.pc(name)).toggle(open), Some(flip(key.clone(), cx))));
        if open {
            let gs: Vec<&Value> = arr(&c.facts, "goals").iter().filter(|g| s(g, "project") == name).take(6).collect();
            let rows: Vec<AnyElement> = gs
                .iter()
                .enumerate()
                .map(|(j, g)| {
                    let est = num(&g["est_human_min"]);
                    let el = lrow(t, c.id(format!("board{i}-g{j}")), div().min_w_0().truncate().child(s(g, "name").to_string()), if est > 0. { dur(est) } else { String::new() }, Some(go_goal(s(g, "goal"), cx)));
                    motion::fade(el, c.id(format!("board{i}-gin{j}")), motion::st(j, 24, 0))
                })
                .collect();
            let stats = stat3(t, vec![("PRs".into(), int(num(&p["prs"]))), ("Releases".into(), int(num(&p["releases"]))), ("Agent time".into(), dur(num(&p["agent_min"])))]);
            body.push(opened(c, &key, 80. + 32. * gs.len() as f32, div().flex().flex_col().gap(px(8.)).pl(px(16.)).child(stats).child(div().flex().flex_col().children(rows))));
        }
    }
    let rest = &ps[ps.len().min(8)..];
    if !rest.is_empty() {
        let n: f64 = rest.iter().map(|p| num(&p["tasks"])).sum();
        let tip = rest.iter().map(|p| format!("{} {}", s(p, "name"), int(num(&p["tasks"])))).collect::<Vec<_>>().join(", ");
        body.push(brow(c, "board-other", 8, Brow::new("Other", pl(n, "task", "tasks"), n / max).color(grey(t)).tip(tip), None));
    }
    Some(card(c, "board", Some("Projects"), None, body))
}

/// A plan-quality share as a percent, or a dash when there were no tasks.
fn share(v: &Value) -> String {
    v.as_f64().map_or("–".into(), |x| format!("{}%", int(x * 100.)))
}

fn per(v: &Value) -> String {
    v.as_f64().map_or("–".into(), |x| format!("{x:.2}"))
}

fn ratio(v: &Value) -> String {
    v.as_f64().map_or("–".into(), super::time::mult)
}

pub fn plan(c: &Ctx, comp: &Value, _cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let t = &c.t;
    let q = c.prop(comp, "quality", "quality");
    if !q.is_object() {
        return None;
    }
    let this = &q["this"];
    if num(&this["goals"]) <= 0. {
        return Some(card(c, "plan", Some("Plan quality"), None, vec![none(t, "0 goals planned")]));
    }
    let before = Some(&q["before"]).filter(|b| b.is_object());
    let line = |label: &str, f: &dyn Fn(&Value) -> String, k: &str| {
        let mut r = vec![label.to_string()];
        if let Some(b) = before {
            r.push(f(&b[k]));
        }
        r.push(f(&this[k]));
        r
    };
    let whole = |v: &Value| v.as_f64().map_or("–".into(), int);
    let rows = vec![
        line("Goals", &whole, "goals"),
        line("Tasks", &whole, "tasks"),
        line("Added late", &share, "late_share"),
        line("Failed or retried", &share, "failed_share"),
        line("Fix rounds per task", &per, "rounds_per_task"),
        line("Questions per task", &per, "questions_per_task"),
        line("Time vs estimate", &ratio, "actual_vs_estimate"),
    ];
    let prev = cap(c.prev());
    let head: Vec<&str> = if before.is_some() { vec!["", &prev, "This period"] } else { vec!["", "This period"] };
    let tasks = num(&this["tasks"]);
    let (late, failed) = (num(&this["added_late"]), num(&this["failed"]));
    let mut body = vec![];
    let (n, what) = if late >= failed { (late, "added late") } else { (failed, "failed or retried") };
    if n > 0. && tasks > 0. {
        body.push(motion::fade(div().child(format!("{}% of tasks were {what}, {} of {}.", int(n / tasks * 100.), int(n), pl(tasks, "task", "tasks"))), c.id("plan-lede"), 40));
    }
    body.push(motion::fade(table(t, &head, rows), c.id("plan-tbl"), 80));
    Some(card(c, "plan", Some("Plan quality"), None, body))
}

pub fn lessons(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let t = &c.t;
    let ls = c.prop(comp, "lessons", "lessons").as_array()?;
    if ls.is_empty() {
        return Some(card(c, "lessons", Some("Lessons kept"), None, vec![none(t, "0 lessons kept")]));
    }
    let mut body = vec![];
    for (i, l) in ls.iter().enumerate() {
        let key = format!("lesson-{}", s(l, "lesson"));
        let open = c.is_open(&key);
        let n = num(&l["confirmations"]);
        let conf = match n as i64 {
            0 => String::new(),
            1 => "Confirmed once".into(),
            k => format!("Confirmed {k} times"),
        };
        let label = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .child(chev(t, open))
            .child(div().flex_none().font_family(t.mono_font.clone()).text_size(px(12.)).text_color(t.muted).child(s(l, "lesson").to_string()))
            .child(div().min_w_0().truncate().child(s(l, "text").to_string()));
        let row = lrow(t, c.id(format!("lesson{i}")), label, conf, Some(flip(key.clone(), cx)));
        body.push(motion::fade(row, c.id(format!("lesson-in{i}")), motion::row(i)));
        if open {
            let when = l["kept_at"].as_str().map(when_day).unwrap_or_default();
            let meta = [cap(s(l, "kind")), when].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
            body.push(opened(c, &key, 80., div().flex().flex_col().gap(px(4.)).pl(px(16.)).child(div().child(s(l, "text").to_string())).child(muted(t, meta))));
        }
    }
    Some(card(c, "lessons", Some("Lessons kept"), None, body))
}

pub fn issues(c: &Ctx, comp: &Value, _cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let t = &c.t;
    let iss = c.prop(comp, "issues", "issues");
    let kinds: Vec<(&str, f64)> = [("bug", "Bugs"), ("gap", "Gaps"), ("follow", "Follow-ups"), ("clean", "Clean-ups")]
        .into_iter()
        .map(|(k, n)| (n, arr(iss, "kinds").iter().filter(|x| s(x, "kind") == k).map(|x| num(&x["found"])).sum::<f64>()))
        .filter(|x| x.1 > 0.)
        .collect();
    let all: f64 = kinds.iter().map(|x| x.1).sum();
    if all <= 0. {
        return Some(card(c, "issues", Some("Found along the way"), None, vec![none(t, "0 issues found")]));
    }
    let max = kinds.iter().map(|x| x.1).fold(0., f64::max);
    let body = kinds.iter().enumerate().map(|(i, (n, v))| brow(c, &format!("issues{i}"), i, Brow::new(*n, int(*v), v / max), None)).collect();
    Some(card(c, "issues", Some("Found along the way"), Some(right(t, int(all))), body))
}
