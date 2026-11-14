//! What shipped: finished goals, releases, and every task.
use super::parts::*;
use crate::app::MainWindow;
use crate::fmt::{arr, s};
use crate::ui::{kit, motion};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

/// Goals by estimated human time, most first.
fn by_human(list: &Value) -> Vec<Value> {
    let mut gs = list.as_array().cloned().unwrap_or_default();
    gs.sort_by(|a, b| num(&b["est_human_min"]).total_cmp(&num(&a["est_human_min"])));
    gs
}

/// A goal's name, opening its page.
fn goal_name(c: &Ctx, key: &str, g: &Value, size: f32, weight: FontWeight, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    hit(&c.t, c.id(format!("{key}-name")))
        .px(px(4.))
        .mx(px(-4.))
        .text_size(px(size))
        .font_weight(weight)
        .child(div().min_w_0().truncate().child(s(g, "name").to_string()))
        .tooltip(kit::tip(format!("{} {}", s(g, "goal"), s(g, "name"))))
        .on_click(go_goal(s(g, "goal"), cx))
}

pub fn gcards(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let gs = by_human(c.prop(comp, "goals", "goals"));
    let t = &c.t;
    if gs.is_empty() {
        return Some(card(c, "gcards", Some("Goals finished"), None, vec![none(t, "0 goals finished")]));
    }
    let all = c.is_open("gcards-all");
    let shown = if all { gs.len() } else { gs.len().min(6) };
    let mut body: Vec<AnyElement> = vec![];
    let mut grid = div().flex().flex_col().gap(px(8.));
    for (r, chunk) in gs[..shown].chunks(3).enumerate() {
        let mut row = div().flex().gap(px(8.));
        let mut open: Option<&Value> = None;
        for (k, g) in chunk.iter().enumerate() {
            let i = r * 3 + k;
            let key = format!("gc|{}", s(g, "goal"));
            if c.is_open(&key) {
                open = Some(g);
            }
            let n = num(&g["tasks"]);
            let cell = div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .p(px(8.))
                .h_full()
                .rounded(px(8.))
                .border_1()
                .border_color(if c.is_open(&key) { t.muted } else { t.border })
                .child(goal_name(c, &format!("gc{i}"), g, 14., FontWeight::SEMIBOLD, cx))
                .child(goal_line(c, g))
                .child(muted(t, when_day(s(g, "finished_at"))))
                .child(disc(c, &key, pl(n, "task", "tasks"), cx));
            row = row.child(div().flex_1().min_w_0().child(motion::fade(cell, c.id(format!("gc{i}-in")), i.min(8) as u64 * 60)));
        }
        for _ in chunk.len()..3 {
            row = row.child(div().flex_1());
        }
        grid = grid.child(row);
        if let Some(g) = open {
            let key = format!("gc|{}", s(g, "goal"));
            let list = goal_tasks(c, s(g, "goal"));
            let h = list.len().min(LONG) as f32 * ROW_H + 40.;
            let inner = div().flex().flex_col().gap(px(4.)).p(px(8.)).rounded(px(8.)).bg(t.panel_2).child(lbl(t, s(g, "name").to_string())).child(tasks(c, &format!("gc-{}", s(g, "goal")), list, cx));
            grid = grid.child(opened(c, &key, h, inner));
        }
    }
    body.push(grid.into_any_element());
    if gs.len() > 6 {
        let label = if all { "Show fewer".to_string() } else { format!("Show all {}", gs.len()) };
        body.push(disc(c, "gcards-all", label, cx).into_any_element());
    }
    Some(card(c, "gcards", Some("Goals finished"), Some(right(t, int(gs.len() as f64))), body))
}

/// The goal a ref prop names, or the biggest finished one.
fn the_goal(c: &Ctx, comp: &Value) -> Option<Value> {
    let r = s(&comp["props"]["goal"], "ref");
    c.goal(r).cloned().or_else(|| by_human(&c.facts["goals"]).into_iter().next())
}

pub fn story(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let t = &c.t;
    let Some(g) = the_goal(c, comp) else {
        return Some(card(c, "story", Some("Goal story"), None, vec![none(t, "0 goals finished")]));
    };
    let list = goal_tasks(c, s(&g, "goal"));
    // The goal's tasks by the day they finished.
    let mut days: Vec<(String, Vec<Value>)> = vec![];
    for x in &list {
        let d = when_day(s(x, "finished_at"));
        match days.iter_mut().find(|e| e.0 == d) {
            Some(e) => e.1.push(x.clone()),
            None => days.push((d, vec![x.clone()])),
        }
    }
    let mx = days.iter().map(|d| d.1.len()).max().unwrap_or(1) as f64;
    let color = c.pc(s(&g, "project"));
    let line = goal_line(c, &g).child(div().flex_none().child(format!(" · {}", when_day(s(&g, "finished_at")))));
    let mut body = vec![
        div().flex().flex_col().gap(px(2.)).child(goal_name(c, "story", &g, 16., FontWeight::BOLD, cx)).child(line).into_any_element(),
        div()
            .flex()
            .flex_col()
            .children(days.iter().enumerate().map(|(i, (d, ts))| brow(c, &format!("story{i}"), i, Brow::new(d.clone(), pl(ts.len() as f64, "task", "tasks"), ts.len() as f64 / mx).color(color), None)))
            .into_any_element(),
    ];
    let h = list.len().min(LONG) as f32 * ROW_H + days.len() as f32 * 24.;
    body.extend(xp(c, "story-x", "Tasks by day", None, h, cx, |cx| {
        div().flex().flex_col().children(days.into_iter().enumerate().flat_map(|(i, (d, ts))| [lbl(&c.t, d).into_any_element(), tasks(c, &format!("story-d{i}"), ts, cx)]).collect::<Vec<_>>())
    }));
    Some(card(c, "story", None, None, body))
}

pub fn spot(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let t = &c.t;
    let Some(g) = the_goal(c, comp) else {
        return Some(card(c, "spot", Some("Biggest win"), None, vec![none(t, "0 goals finished")]));
    };
    let project = s(&g, "project");
    let color = c.pc(project);
    let mut stats = vec![("Tasks".to_string(), int(num(&g["tasks"]))), ("PRs".into(), int(num(&g["prs"]))), ("Lines".into(), format!("+{}", int(num(&g["added"]))))];
    if num(&g["est_human_min"]) > 0. {
        stats.push(("Estimated".into(), dur(num(&g["est_human_min"]))));
    }
    stats.push(("Agent time".into(), dur(num(&g["agent_min"]))));
    let chip = div().flex().items_center().self_start().px(px(8.)).py(px(2.)).rounded_full().bg(t.panel_2).text_size(px(12.)).text_color(t.text_2).child(sw(color)).child(project.to_string());
    let name = hit(t, c.id("spot-name"))
        .px(px(4.))
        .mx(px(-4.))
        .text_size(px(24.))
        .font_weight(FontWeight::SEMIBOLD)
        .line_height(px(31.))
        .child(s(&g, "name").to_string())
        .on_click(go_goal(s(&g, "goal"), cx));
    let main = div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .flex_1()
        .min_w(px(320.))
        .child(chip)
        .child(motion::fade(name, c.id("spot-name-in"), 80))
        .children(crate::fmt::opt_s(&g, "tldr").map(|x| motion::fade(div().text_color(t.text_2).child(x.to_string()), c.id("spot-tldr"), 200)))
        .child(tiles(t, color, stats));
    let side = if c.month {
        let others: Vec<Value> = by_human(c.prop(comp, "goals", "goals")).into_iter().filter(|x| s(x, "goal") != s(&g, "goal")).take(5).collect();
        let rows: Vec<AnyElement> = others
            .iter()
            .enumerate()
            .map(|(i, x)| {
                let label = div().flex().items_center().min_w_0().child(sw(c.pc(s(x, "project")))).child(div().min_w_0().truncate().child(s(x, "name").to_string()));
                let est = num(&x["est_human_min"]);
                let el = lrow(t, c.id(format!("spot-r{i}")), label, if est > 0. { format!("{} est.", dur(est)) } else { String::new() }, Some(go_goal(s(x, "goal"), cx)));
                motion::fade(el, c.id(format!("spot-r{i}-in")), motion::st(i, 24, 0))
            })
            .collect();
        (!rows.is_empty()).then(|| div().flex().flex_col().child(lbl(t, "Runners-up")).children(rows))
    } else {
        let list = goal_tasks(c, s(&g, "goal"));
        let n = list.len();
        let rows: Vec<AnyElement> = list.iter().take(5).enumerate().map(|(i, x)| trow(c, &format!("spot{i}"), i, s(x, "task"), s(x, "title"), String::new(), cx)).collect();
        let more = (n > 5).then(|| {
            let (hover, fg) = (t.panel_2, t.text);
            div()
                .id(c.id("spot-more"))
                .flex()
                .items_center()
                .gap(px(4.))
                .min_h(px(24.))
                .px(px(4.))
                .ml(px(-4.))
                .rounded(px(4.))
                .text_size(px(12.))
                .text_color(t.text_2)
                .cursor_pointer()
                .hover(move |s| s.bg(hover).text_color(fg))
                .child(pl((n - 5) as f64, "more task", "more tasks"))
                .child("›")
                .on_click(go_goal(s(&g, "goal"), cx))
        });
        (n > 0).then(|| div().flex().flex_col().child(lbl(t, pl(n as f64, "task", "tasks"))).children(rows).children(more))
    };
    let inner = div().flex().flex_wrap().gap(px(24.)).child(main).children(side.map(|d| d.flex_1().min_w(px(280.)).max_w(px(420.))));
    Some(card(c, "spot", None, None, vec![inner.into_any_element()]))
}

/// The period's first local midnight and its length in hours.
fn span(c: &Ctx) -> Option<(chrono::NaiveDateTime, f64)> {
    let first = date(s(&c.facts["period"], "first_day"))?;
    let last = date(s(&c.facts["period"], "last_day"))?;
    Some((first.and_hms_opt(0, 0, 0)?, ((last - first).num_days() + 1) as f64 * 24.))
}

/// Where in the period (0–1) an ISO time falls.
fn at(c: &Ctx, iso: &str) -> f64 {
    let (Some((from, hours)), Some(t)) = (span(c), crate::fmt::parse(iso)) else { return 0. };
    let h = (crate::fmt::local(t).naive_local() - from).num_minutes() as f64 / 60.;
    (h / hours).clamp(0., 1.)
}

pub fn timeline(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let rel = c.prop(comp, "releases", "releases").as_array().cloned().unwrap_or_default();
    let t = &c.t;
    if rel.is_empty() {
        return Some(card(c, "timeline", Some("Releases"), None, vec![none(t, "0 releases")]));
    }
    let mut by: Vec<(String, usize)> = vec![];
    for r in &rel {
        match by.iter_mut().find(|x| x.0 == s(r, "project")) {
            Some(x) => x.1 += 1,
            None => by.push((s(r, "project").to_string(), 1)),
        }
    }
    by.sort_by(|a, b| b.1.cmp(&a.1));
    let top: Vec<String> = by.iter().take(4).map(|x| x.0.clone()).collect();
    let mut lanes: Vec<String> = top.clone();
    if by.len() > 4 {
        lanes.push("Other".into());
    }
    let label_w = px(96.);
    let mut body = div().flex().flex_col().gap(px(4.));
    let mut k = 0;
    for lane in &lanes {
        let color = if lane == "Other" { grey(t) } else { c.pc(lane) };
        let rs: Vec<&Value> = rel.iter().filter(|r| if lane == "Other" { !top.iter().any(|p| p == s(r, "project")) } else { s(r, "project") == lane }).collect();
        let rail = div().relative().flex_1().h(px(24.)).child(div().absolute().left_0().right_0().top(px(11.5)).h(px(1.)).bg(t.divider)).children(rs.into_iter().map(|r| {
            let x = at(c, s(r, "cut_at"));
            let tip = format!("{} {}\n{} · {}", s(r, "project"), s(r, "name"), when(s(r, "cut_at")), pl(num(&r["count"]), "task", "tasks"));
            let dot = div().size(px(14.)).flex().items_center().justify_center().rounded_full().bg(t.card).child(motion::pop(c.id(format!("pin{k}")), 100 + (x * 900.) as u64, 10., color));
            k += 1;
            div()
                .id(c.id(format!("pin{k}-hit")))
                .absolute()
                .top_0()
                .left(relative(x as f32))
                .ml(px(-12.))
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .child(dot)
                .tooltip(kit::tip(tip))
                .on_click(go_release(s(r, "project"), s(r, "release"), cx))
        }));
        let name = div().flex().items_center().flex_none().w(label_w).min_w_0().text_size(px(12.)).text_color(t.text_2).child(sw(color)).child(div().min_w_0().truncate().child(lane.clone()));
        body = body.child(div().flex().items_center().gap(px(8.)).child(name).child(rail));
    }
    let first = date(s(&c.facts["period"], "first_day"));
    let n = span(c).map_or(7, |(_, h)| (h / 24.) as usize);
    let labels: Vec<String> = (0..n).filter_map(|i| first.map(|f| f + chrono::Duration::days(i as i64))).map(|d| if c.month { d.format("%b %-d").to_string() } else { d.format("%a").to_string() }).collect();
    body = body.child(div().flex().gap(px(8.)).child(div().flex_none().w(label_w)).child(axis(t, labels).flex_1()));
    Some(card(c, "timeline", Some("Releases"), Some(right(t, int(rel.len() as f64))), vec![body.into_any_element()]))
}

pub fn train(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let rel = c.prop(comp, "releases", "releases").as_array().cloned().unwrap_or_default();
    let t = &c.t;
    if rel.is_empty() {
        return Some(card(c, "train", Some("Versions shipped"), None, vec![none(t, "0 releases")]));
    }
    let mut by: Vec<(String, Vec<Value>)> = vec![];
    for r in &rel {
        match by.iter_mut().find(|x| x.0 == s(r, "project")) {
            Some(x) => x.1.push(r.clone()),
            None => by.push((s(r, "project").to_string(), vec![r.clone()])),
        }
    }
    for (_, v) in &mut by {
        v.sort_by(|a, b| s(a, "cut_at").cmp(s(b, "cut_at")));
    }
    by.sort_by(|a, b| b.1.len().cmp(&a.1.len()));
    let rows: Vec<AnyElement> = by
        .iter()
        .enumerate()
        .map(|(i, (p, v))| {
            let (first, last) = (&v[0], &v[v.len() - 1]);
            let value = if v.len() > 1 { format!("{} → {}", s(first, "name"), s(last, "name")) } else { s(first, "name").to_string() };
            let tip = format!("{}, last {}", pl(v.len() as f64, "release", "releases"), when(s(last, "cut_at")));
            let el = lrow(t, c.id(format!("train{i}")), named(c, p, ""), value, Some(go_release(p, s(last, "release"), cx))).tooltip(kit::tip(tip));
            motion::fade_x(el, c.id(format!("train{i}-in")), motion::row(i))
        })
        .collect();
    let mut body = vec![div().flex().flex_col().children(rows).into_any_element()];
    body.extend(xp(c, "train-x", &format!("All {}", rel.len()), None, rel.len() as f32 * ROW_H + by.len() as f32 * 24., cx, |cx| {
        let mut out = div().flex().flex_col();
        for (p, v) in &by {
            out = out.child(lbl(t, p.clone()));
            for (i, r) in v.iter().enumerate() {
                let name = div().min_w_0().truncate().font_family(t.mono_font.clone()).child(s(r, "name").to_string());
                let el = lrow(t, c.id(format!("train-{}", s(r, "release"))), name, when(s(r, "cut_at")), Some(go_release(p, s(r, "release"), cx)));
                out = out.child(motion::fade(el, c.id(format!("train-{}-in", s(r, "release"))), motion::st(i, 24, 0)));
            }
        }
        out
    }));
    Some(card(c, "train", Some("Versions shipped"), None, body))
}

/// A group of the Shipped list: its label, a disclosure of its tasks, and a bar for its share.
fn group(c: &Ctx, key: &str, i: usize, label: AnyElement, ts: Vec<Value>, count: usize, share: f64, color: Hsla, goal: Option<&str>, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = &c.t;
    let top = div().flex().items_center().gap(px(16.)).child(div().flex().items_center().flex_1().min_w_0().overflow_hidden().child(label)).child(disc(c, key, pl(count as f64, "task", "tasks"), cx));
    let track = div().flex().w_full().h(px(8.)).child(motion::grow_w(c.id(format!("{key}-bar")), motion::st(i, 40, 200), motion::GROW_W, share.max(0.004) as f32, color, 8.));
    let mut el = div().flex().flex_col().gap(px(2.)).py(px(4.)).child(top).child(track);
    if c.is_open(key) {
        const CAP: usize = 8;
        let rest = count.saturating_sub(CAP.min(ts.len()));
        let shown: Vec<Value> = ts.into_iter().take(CAP).collect();
        let h = shown.len() as f32 * ROW_H + 24.;
        let more = (rest > 0).then(|| match goal {
            Some(g) => kit::link(t, c.id(format!("{key}-more")), format!("{} ›", pl(rest as f64, "more task", "more tasks"))).on_click(go_goal(g, cx)).into_any_element(),
            None => muted(t, pl(rest as f64, "more task", "more tasks")).into_any_element(),
        });
        let inner = div().flex().flex_col().pl(px(16.)).child(tasks(c, key, shown, cx)).children(more);
        el = el.child(opened(c, key, h, inner));
    }
    motion::fade(el, c.id(format!("{key}-in")), motion::row(i))
}

pub fn list(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let all = c.prop(comp, "tasks", "tasks");
    let t = &c.t;
    let goals = arr(all, "by_goal").to_vec();
    let loose = arr(all, "loose").to_vec();
    let total: usize = goals.iter().map(|g| arr(g, "tasks").len()).sum::<usize>() + loose.iter().map(|l| num(&l["count"]) as usize).sum::<usize>();
    if total == 0 {
        return Some(card(c, "list", Some("Shipped"), None, vec![none(t, "0 tasks")]));
    }
    let small_list = total <= 30;
    let (g_n, p_n, more_n) = (if small_list { goals.len() } else { 5 }, if small_list { loose.len() } else { 4 }, 10);
    let mx = goals.iter().map(|g| arr(g, "tasks").len()).chain(loose.iter().map(|l| num(&l["count"]) as usize)).max().unwrap_or(1).max(1) as f64;
    let goal_row = |i: usize, g: &Value, cx: &mut Context<MainWindow>| {
        let r = s(g, "goal");
        let color = c.pc(s(g, "project"));
        let label = hit(t, c.id(format!("list-g{r}")))
            .flex()
            .items_center()
            .min_w_0()
            .px(px(4.))
            .mx(px(-4.))
            .child(sw(color))
            .child(div().flex_none().mr(px(6.)).font_family(t.mono_font.clone()).text_size(px(12.)).text_color(t.muted).child(r.to_string()))
            .child(div().min_w_0().truncate().child(s(g, "name").to_string()))
            .on_click(go_goal(r, cx));
        let ts = arr(g, "tasks").to_vec();
        let n = ts.len();
        group(c, &format!("list-{r}"), i, label.into_any_element(), ts, n, n as f64 / mx, color, Some(r), cx)
    };
    let mut body = div().flex().flex_col();
    if !goals.is_empty() {
        body = body.child(lbl(t, "Goals"));
        let rows: Vec<AnyElement> = goals.iter().take(g_n).enumerate().map(|(i, g)| goal_row(i, g, cx)).collect();
        body = body.child(div().flex().flex_col().pl(px(16.)).children(rows));
        let later: Vec<Value> = goals.iter().skip(g_n).take(more_n).cloned().collect();
        let left: Vec<&Value> = goals.iter().skip(g_n + more_n).collect();
        if !later.is_empty() {
            let left_tasks: usize = left.iter().map(|g| arr(g, "tasks").len()).sum();
            let left_n = left.len();
            let x = xp(c, "list-more", &format!("Show {} more", later.len()), Some("Show fewer"), later.len() as f32 * 44., cx, |cx| {
                let rows: Vec<AnyElement> = later.iter().enumerate().map(|(i, g)| goal_row(i, g, cx)).collect();
                div().flex().flex_col().pl(px(16.)).children(rows).when(left_n > 0, |d| d.child(muted(t, format!("{}, {}", pl(left_n as f64, "more goal", "more goals"), pl(left_tasks as f64, "task", "tasks")))))
            });
            body = body.child(div().pl(px(16.)).children(x));
        }
    }
    if !loose.is_empty() {
        body = body.child(lbl(t, "No goal"));
        let mut rows: Vec<AnyElement> = vec![];
        for (i, l) in loose.iter().take(p_n).enumerate() {
            let p = s(l, "project");
            let n = num(&l["count"]) as usize;
            rows.push(group(c, &format!("list-p{p}"), i, named(c, p, "").into_any_element(), arr(l, "tasks").to_vec(), n, n as f64 / mx, c.pc(p), None, cx));
        }
        let other: Vec<&Value> = loose.iter().skip(p_n).collect();
        if !other.is_empty() {
            let n: usize = other.iter().map(|l| num(&l["count"]) as usize).sum();
            let ts: Vec<Value> = other.iter().flat_map(|l| arr(l, "tasks").to_vec()).collect();
            let tip = other.iter().map(|l| format!("{} {}", s(l, "project"), int(num(&l["count"])))).collect::<Vec<_>>().join(", ");
            let label = div().id(c.id("list-other")).flex().items_center().child(sw(grey(t))).child("Other").tooltip(kit::tip(tip));
            rows.push(group(c, "list-other", p_n, label.into_any_element(), ts, n, n as f64 / mx, grey(t), None, cx));
        }
        body = body.child(div().flex().flex_col().pl(px(16.)).children(rows));
    }
    Some(card(c, "list", Some("Shipped"), Some(right(t, int(total as f64))), vec![body.into_any_element()]))
}
