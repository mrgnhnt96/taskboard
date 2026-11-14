//! The components that open a page: the opening line, the hero numbers, the key-number tiles, and
//! the callouts (a surprising fact, quiet days, milestones).
use super::parts::*;
use crate::app::MainWindow;
use crate::fmt::{arr, s};
use crate::ui::motion;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

/// A sentence split after its commas, colons, semicolons and full stops, each part keeping its mark.
pub fn clauses(text: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut chars = text.trim().chars().peekable();
    while let Some(ch) = chars.next() {
        cur.push(ch);
        if matches!(ch, ',' | ':' | ';' | '.') && chars.peek() == Some(&' ') {
            out.push(cur.trim().to_string());
            cur.clear();
        }
    }
    if !cur.trim().is_empty() {
        out.push(cur.trim().to_string());
    }
    out
}

/// `.lede`: a 24px line whose clauses fade in one after another.
pub fn lede(c: &Ctx, text: &str) -> Option<AnyElement> {
    let parts = clauses(text);
    if parts.is_empty() {
        return None;
    }
    let el = div()
        .flex()
        .flex_wrap()
        .gap_x(px(6.))
        .max_w(px(816.))
        .text_size(px(24.))
        .font_weight(FontWeight::SEMIBOLD)
        .line_height(px(31.))
        .children(parts.into_iter().enumerate().map(|(i, p)| motion::fade(div().child(p), c.id(format!("lede{i}")), i as u64 * 240)));
    Some(el.into_any_element())
}

/// The page's highlights, after its opening line.
pub fn highlights(c: &Ctx) -> Option<AnyElement> {
    if c.highlights.is_empty() {
        return None;
    }
    let after = clauses(&c.summary).len() as u64 * 240;
    let t = &c.t;
    let rows = c.highlights.iter().enumerate().map(|(i, h)| {
        let el = div().flex().items_start().gap(px(8.)).child(div().flex_none().mt(px(7.)).size(px(6.)).rounded_full().bg(t.accent)).child(div().flex_1().min_w_0().text_color(t.text_2).child(h.clone()));
        motion::fade(el, c.id(format!("hl{i}")), motion::st(i, 40, after))
    });
    Some(div().flex().flex_col().gap(px(4.)).children(rows).into_any_element())
}

pub fn open(c: &Ctx, _comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let mut body: Vec<AnyElement> = vec![];
    body.extend(lede(c, &c.summary));
    body.extend(highlights(c));
    let tot = c.fact("totals");
    if num(&tot["tasks"]) > 0. {
        let facts = vec![
            ("Tasks", int(num(&tot["tasks"]))),
            ("PRs merged", int(num(&tot["merged_pr"]))),
            ("Releases", int(num(&tot["releases"]))),
            ("Goals finished", int(num(&tot["goals_finished"]))),
            ("Projects", int(active(&c.facts["projects"]).len() as f64)),
            ("Agent time", dur(num(&tot["agent_min"]))),
        ];
        let t = c.t.clone();
        body.extend(xp(c, "facts", "Facts", None, 200., cx, |_| {
            div().flex().flex_col().children(facts.into_iter().enumerate().map(|(i, (a, b))| motion::fade(kv(&t, a, b), c.id(format!("facts{i}")), motion::st(i, 24, 0))))
        }));
    }
    if body.is_empty() {
        return None;
    }
    Some(card(c, "open", None, None, body))
}

/// "tasks landed · ↑ 3 vs last week".
fn under(c: &Ctx, one: &str, many: &str, cur: f64, key: &str) -> AnyElement {
    let word = if cur.round() == 1. { one } else { many };
    let line = match vs(c, cur, key, int) {
        Some(d) => format!("{word} · {d}"),
        None => word.to_string(),
    };
    small(&c.t, line).text_size(px(14.)).into_any_element()
}

/// Shares by project as a wiped-in stack with its legend.
fn by_project(c: &Ctx, key: &str, rows: Vec<(String, f64)>, one: &str, many: &str) -> Vec<AnyElement> {
    let rows = fold(rows, 4);
    let color = |n: &str| if n == "Other" { grey(&c.t) } else { c.pc(n) };
    let parts = rows.iter().map(|(n, v)| (*v, color(n), format!("{n}: {}", pl(*v, one, many)))).collect();
    let items = rows.iter().map(|(n, v)| (color(n), format!("{n} {}", int(*v)))).collect();
    vec![stack(c, key, 80, 400, 16., parts), legend(&c.t, items).into_any_element()]
}

pub fn hero(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let n = num(c.prop(comp, "lead", "totals.tasks"));
    let tot = c.fact("totals").clone();
    let projects: Vec<(String, f64)> = active(c.prop(comp, "projects", "projects"))
        .iter()
        .map(|p| (s(p, "name").to_string(), num(&p["tasks"])))
        .collect();
    let mut body = vec![
        div().flex().flex_col().child(super::parts::hero(c, "hero", n, int)).child(under(c, "task landed", "tasks landed", n, "tasks")).into_any_element(),
    ];
    let t = c.t.clone();
    body.extend(xp(c, "hero-x", "By outcome and project", None, 180., cx, |_| {
        let stats = stat3(&t, vec![("Merged".into(), int(num(&tot["merged"]))), ("No PR".into(), int(num(&tot["no_pr"]))), ("Closed".into(), int(num(&tot["closed_pr"])))]);
        div().flex().flex_col().gap(px(12.)).child(stats).when(!projects.is_empty(), |d| d.children(by_project(c, "hero-s", projects, "task", "tasks")))
    }));
    Some(card(c, "hero", None, None, body))
}

pub fn hero_r(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let rel = c.prop(comp, "releases", "releases").as_array().cloned().unwrap_or_default();
    let n = rel.len() as f64;
    let mut by: Vec<(String, f64)> = vec![];
    for r in &rel {
        match by.iter_mut().find(|x| x.0 == s(r, "project")) {
            Some(x) => x.1 += 1.,
            None => by.push((s(r, "project").to_string(), 1.)),
        }
    }
    by.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut days: Vec<String> = rel.iter().map(|r| when_day(s(r, "cut_at"))).collect();
    days.sort();
    days.dedup();
    let mut body = vec![
        div().flex().flex_col().child(super::parts::hero(c, "heroR", n, int)).child(under(c, "release", "releases", n, "releases")).into_any_element(),
    ];
    let t = c.t.clone();
    let projects = by.len() as f64;
    body.extend(xp(c, "heroR-x", "By project", None, 180., cx, |_| {
        let stats = stat3(&t, vec![("Projects".into(), int(projects)), ("Days with a release".into(), int(days.len() as f64))]);
        div().flex().flex_col().gap(px(12.)).child(stats).children(by_project(c, "heroR-s", by, "release", "releases"))
    }));
    Some(card(c, "heroR", None, None, body))
}

/// A key number of the tiles: its name, value, the value before, and its value by day.
struct Kpi {
    name: &'static str,
    cur: f64,
    prev: Option<f64>,
    f: fn(f64) -> String,
    by_day: Vec<f64>,
}

pub fn kpis(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let tot = c.prop(comp, "totals", "totals");
    let before = c.prop(comp, "before", "before");
    let days = c.prop(comp, "days", "days").as_array().cloned().unwrap_or_default();
    let dates: Vec<String> = days.iter().map(|d| s(d, "date").to_string()).collect();
    let on_day = |list: &[Value], key: &str| -> Vec<f64> {
        dates.iter().map(|d| list.iter().filter(|x| fmt_date(s(x, key)) == *d).count() as f64).collect()
    };
    let prev = |k: &str| before.is_object().then(|| num(&before[k]));
    let m = [
        Kpi { name: "Tasks", cur: num(&tot["tasks"]), prev: prev("tasks"), f: int, by_day: days.iter().map(|d| num(&d["tasks"])).collect() },
        Kpi { name: "PRs", cur: num(&tot["prs"]), prev: prev("prs"), f: int, by_day: days.iter().map(|d| num(&d["merged"])).collect() },
        Kpi { name: "Releases", cur: num(&tot["releases"]), prev: prev("releases"), f: int, by_day: on_day(arr(&c.facts, "releases"), "cut_at") },
        Kpi { name: "Goals", cur: num(&tot["goals_finished"]), prev: prev("goals_finished"), f: int, by_day: on_day(arr(&c.facts, "goals"), "finished_at") },
        Kpi { name: "Agent time", cur: num(&tot["agent_min"]), prev: prev("agent_min"), f: dur, by_day: days.iter().map(|d| num(&d["agent_min"])).collect() },
    ];
    let t = &c.t;
    let on = (0..m.len()).find(|i| c.is_open(&format!("kpi|{i}")));
    let tiles = div().flex().gap(px(8.)).children(m.iter().enumerate().map(|(i, k)| {
        let key = format!("kpi|{i}");
        let open = on == Some(i);
        let (hover, press, accent, soft) = (t.panel_2, t.seg, t.accent, t.accent_soft);
        let label = div().flex().items_center().text_size(px(12.)).text_color(t.muted).child(k.name).child(div().ml(px(4.)).child(chev(t, open)));
        let tile = div()
            .id(c.id(format!("kpi{i}")))
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .p(px(8.))
            .rounded(px(8.))
            .cursor_pointer()
            .when(open, |d| d.bg(soft).border_b(px(2.)).border_color(accent))
            .when(!open, |d| d.hover(move |s| s.bg(hover)))
            .active(move |s| s.bg(press))
            .child(label)
            .child(big_count(c, &format!("kpi{i}"), i as u64 * 60, k.cur, k.f))
            .children(k.prev.filter(|p| k.cur != 0. || *p != 0.).map(|p| small(t, arrow(k.cur, p, k.f))))
            .on_click(cx.listener(move |m, _, _, cx| super::toggle(m, &key, cx)));
        motion::fade(tile, c.id(format!("kpi{i}-in")), i as u64 * 60)
    }));
    let mut body = vec![tiles.into_any_element()];
    if let Some(i) = on {
        let k = &m[i];
        let list: Vec<Col> = k.by_day.iter().zip(&dates).map(|(v, d)| Col { value: *v, tip: format!("{} {}: {}", weekday(d), day(d), (k.f)(*v)), dim: false }).collect();
        let labels = dates.iter().map(|d| if c.month { day(d) } else { weekday(d) }).collect();
        let key = format!("kpi|{i}");
        body.push(opened(c, &key, 100., div().flex().flex_col().gap(px(4.)).child(cols(c, &format!("kpi{i}-cols"), 64., 20, 0., &list, None, vec![])).child(axis(t, labels))));
    }
    Some(card(c, "kpis", None, None, body))
}

/// A fact key read as words: "before.agent_min" is "Agent time, last week".
fn fact_label(c: &Ctx, key: &str) -> String {
    let last = key.rsplit('.').next().unwrap_or(key);
    let name = match last {
        "tasks" => "Tasks",
        "merged" | "merged_pr" => "PRs merged",
        "prs" => "PRs",
        "no_pr" => "Tasks without a PR",
        "releases" => "Releases",
        "commits" => "Commits",
        "added" => "Lines added",
        "removed" => "Lines removed",
        "agent_min" => "Agent time",
        "busy_min" => "Busy time",
        "command_min" => "Command time",
        "est_human_min" => "Estimated human time",
        "est_agent_min" => "Agent time on estimated tasks",
        "goals_finished" => "Goals finished",
        "human_done" => "Your tasks done",
        "minutes" if key.starts_with("waiting") => "Waiting on you",
        "found" => "Issues found",
        other => return crate::fmt::cap(&other.replace('_', " ")),
    };
    let when = if key.starts_with("before.") {
        format!(", {}", c.prev())
    } else if key.starts_with("totals.") {
        format!(", this {}", if c.month { "month" } else { "week" })
    } else {
        String::new()
    };
    format!("{name}{when}")
}

fn fact_value(key: &str, v: f64) -> String {
    if key.ends_with("_min") || key.ends_with("minutes") { dur(v) } else { int(v) }
}

/// "+38%" or "−12%".
fn spct(v: f64) -> String {
    format!("{}{}%", if v < 0. { "−" } else { "+" }, int(v.abs()))
}

pub fn fact(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let text = s(comp, "text").to_string();
    let pick = |k: &str| (s(&comp["props"][k], "fact").to_string(), num(&comp["props"][k]["value"]));
    let ((ka, a), (kb, b)) = (pick("a"), pick("b"));
    let lead = if a != 0. { spct((b - a) / a * 100.) } else { fact_value(&kb, b) };
    let mut body = vec![
        div().flex().flex_col().gap(px(4.)).child(motion::count(div().text_size(px(56.)).font_weight(FontWeight::BOLD).line_height(px(59.)), c.id("fact-hero"), 80, 1., move |_| lead.clone())).into_any_element(),
    ];
    body.push(motion::fade(div().max_w(px(816.)).text_size(px(24.)).font_weight(FontWeight::SEMIBOLD).line_height(px(31.)).child(text), c.id("fact-lede"), 320));
    let t = c.t.clone();
    let rows = vec![(fact_label(c, &ka), fact_value(&ka, a)), (fact_label(c, &kb), fact_value(&kb, b))];
    body.extend(xp(c, "fact-x", "Numbers", None, 80., cx, |_| {
        div().flex().flex_col().children(rows.into_iter().enumerate().map(|(i, (l, v))| motion::fade(kv(&t, l, v), c.id(format!("fact-r{i}")), motion::st(i, 24, 0))))
    }));
    Some(card(c, "fact", None, None, body))
}

pub fn quiet(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let days = c.prop(comp, "days", "days").as_array().cloned().unwrap_or_default();
    let mx = days.iter().map(|d| num(&d["tasks"])).fold(1., f64::max);
    let mut body = vec![motion::fade(div().text_color(c.t.text_2).child(s(comp, "text").to_string()), c.id("quiet-s"), 80)];
    body.extend(xp(c, "quiet-x", "Every day", None, days.len() as f32 * 34., cx, |_| {
        div().flex().flex_col().children(days.iter().enumerate().map(|(i, d)| {
            let date = s(d, "date");
            brow(c, &format!("quiet{i}"), i, Brow::new(format!("{} {}", weekday(date), day(date)), int(num(&d["tasks"])), num(&d["tasks"]) / mx), None)
        }))
    }));
    Some(card(c, "quiet", Some("Quiet days"), None, body))
}

pub fn first(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let rel = c.prop(comp, "releases", "releases").as_array().cloned().unwrap_or_default();
    let firsts: Vec<&Value> = rel.iter().filter(|r| r["first"].as_bool() == Some(true)).collect();
    let t = &c.t;
    let mut body = vec![motion::fade(div().text_color(t.text_2).child(s(comp, "text").to_string()), c.id("first-s"), 80)];
    let row = |i: usize, r: &Value, cx: &mut Context<MainWindow>| {
        let click = go_release(s(r, "project"), s(r, "release"), cx);
        let el = lrow(t, c.id(format!("first{i}")), named(c, s(r, "project"), s(r, "name")), when(s(r, "cut_at")), Some(click));
        motion::fade(el, c.id(format!("first{i}-in")), motion::row(i))
    };
    let shown: Vec<AnyElement> = firsts.iter().take(3).enumerate().map(|(i, r)| row(i, r, cx)).collect();
    body.push(div().flex().flex_col().children(shown).into_any_element());
    if firsts.len() > 3 {
        let rest: Vec<Value> = firsts[3..].iter().map(|r| (*r).clone()).collect();
        body.extend(xp(c, "first-x", &format!("{} more", rest.len()), None, rest.len() as f32 * 34., cx, |cx| {
            div().flex().flex_col().children(rest.iter().enumerate().map(|(i, r)| row(i + 3, r, cx)).collect::<Vec<_>>())
        }));
    }
    let right = (!firsts.is_empty()).then(|| super::parts::right(t, int(firsts.len() as f64)));
    Some(card(c, "first", Some("Milestones"), right, body))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's `test` attribute.
    use super::{clauses, spct};

    #[test]
    fn a_summary_splits_into_clauses() {
        assert_eq!(clauses("A release every day, a new Mac app, and 40 tasks landed."), vec!["A release every day,", "a new Mac app,", "and 40 tasks landed."]);
        assert_eq!(clauses("Up 3.5 times: fast"), vec!["Up 3.5 times:", "fast"]);
        assert!(clauses("  ").is_empty());
    }

    #[test]
    fn a_change_reads_signed() {
        assert_eq!(spct(38.4), "+38%");
        assert_eq!(spct(-12.), "−12%");
    }
}
