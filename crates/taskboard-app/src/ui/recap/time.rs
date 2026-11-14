//! Where the time went: agent time by kind of command, against estimates, waiting on you, usage.
use super::parts::*;
use crate::app::MainWindow;
use crate::fmt::{arr, s};
use crate::ui::motion;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;

/// "4.2×", or "12×" from ten up.
pub fn mult(x: f64) -> String {
    if x >= 9.95 { format!("{}×", int(x)) } else { format!("{}×", trim1(x)) }
}

/// One decimal, without a trailing ".0".
fn trim1(x: f64) -> String {
    let s = format!("{x:.1}");
    s.strip_suffix(".0").map(str::to_string).unwrap_or(s)
}

fn pct(x: f64) -> String {
    format!("{}%", int(x))
}

/// Weeks of forty hours, as the design writes them.
fn weeks(w: f64) -> String {
    if w < 0.05 {
        return "under 0.1 weeks".into();
    }
    let n = if w >= 10. { int(w) } else { trim1(w) };
    format!("{n} {}", if n == "1" { "week" } else { "weeks" })
}

/// What the estimate is in working time.
pub fn say(hours: f64) -> String {
    if hours < 80. {
        return format!("{} estimated hours is about {:.1} eight-hour working days.", int(hours), hours / 8.);
    }
    let w = hours / 40.;
    let months = w / 4.33;
    let span = if months >= 11.5 { "a year".to_string() } else { pl(months.round(), "month", "months") };
    format!("{} estimated hours is about {} forty-hour weeks, or {span} of one person’s full-time work.", int(hours), int(w))
}

/// Command minutes by category, with the waits together, largest first.
fn categories(c: &Ctx, list: &Value) -> Vec<(&'static str, Hsla, f64)> {
    let mut by: Vec<(&'static str, Hsla, usize, f64)> = vec![];
    for x in list.as_array().map_or(&[][..], Vec::as_slice) {
        let (name, color, i) = category(&c.t, s(x, "name"));
        match by.iter_mut().find(|b| b.2 == i) {
            Some(b) => b.3 += num(&x["minutes"]),
            None => by.push((name, color, i, num(&x["minutes"]))),
        }
    }
    by.retain(|b| b.3 > 0.);
    by.sort_by_key(|b| b.2);
    by.into_iter().map(|b| (b.0, b.1, b.3)).collect()
}

pub fn time(c: &Ctx, comp: &Value, _cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let t = &c.t;
    let agent = num(c.prop(comp, "agent_min", "totals.agent_min"));
    let cats = categories(c, c.prop(comp, "categories", "agent.categories"));
    if agent <= 0. && cats.is_empty() {
        return Some(card(c, "time", Some("Agent time"), None, vec![none(t, "0m of agent time")]));
    }
    let tot: f64 = cats.iter().map(|x| x.2).sum::<f64>().max(1e-9);
    let mut body = vec![big_count(c, "time", 80, agent, dur)];
    if !cats.is_empty() {
        body.push(stack(c, "time-stack", 80, 400, 12., cats.iter().map(|x| (x.2, x.1, format!("{}: {}", x.0, dur(x.2)))).collect()));
        let rows: Vec<AnyElement> = cats
            .iter()
            .enumerate()
            .map(|(i, x)| {
                let tip = format!("{}: {}% of command time", x.0, int(x.2 / tot * 100.));
                let row = div()
                    .id(c.id(format!("time-r{i}")))
                    .flex()
                    .items_center()
                    .gap(px(16.))
                    .min_h(px(ROW_H))
                    .px(px(8.))
                    .mx(px(-8.))
                    .rounded(px(8.))
                    .hover({
                        let h = t.panel_2;
                        move |s| s.bg(h)
                    })
                    .tooltip(crate::ui::kit::tip(tip))
                    .child(div().flex().items_center().flex_1().min_w_0().child(sw(x.1)).child(x.0))
                    .child(div().flex_none().text_color(t.text_2).child(dur(x.2)));
                motion::fade(row, c.id(format!("time-in{i}")), motion::row(i))
            })
            .collect();
        body.push(div().flex().flex_col().children(rows).into_any_element());
    }
    Some(card(c, "time", Some("Agent time"), None, body))
}

pub fn vs(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let t = &c.t;
    let human = num(c.prop(comp, "est_human_min", "totals.est_human_min"));
    if human <= 0. {
        return Some(card(c, "vs", Some("Agent vs estimated human"), None, vec![none(t, "0 estimated hours")]));
    }
    let agent = num(c.prop(comp, "est_agent_min", "totals.est_agent_min"));
    let estimated = num(&c.fact("totals")["estimated"]);
    let merged = num(&c.fact("totals")["merged"]).max(estimated);
    let (hh, ah) = (human / 60., agent / 60.);
    let w = hh / 40.;
    let top = w.max(ah / 40.).max(1e-9);
    let ratio = hh / ah.max(0.01);
    let base = div().text_size(px(24.)).font_weight(FontWeight::BOLD).line_height(px(29.)).whitespace_nowrap();
    let mut body = vec![
        motion::count(base, c.id("vs-n"), 80, ratio, mult),
        small(t, format!("Estimated human time vs agent time, on {} of {} tasks", int(estimated), int(merged))).into_any_element(),
        div().text_size(px(14.)).child(say(hh)).into_any_element(),
        brow(c, "vs-a", 0, Brow::new("Agent, on estimated tasks", format!("{} · {}", weeks(ah / 40.), dur(agent)), ah / 40. / top), None),
        brow(c, "vs-h", 1, Brow::new("Estimated human, at 40 hours a week", format!("{} · {}", if w >= 10. { pl(w.round(), "week", "weeks") } else { weeks(w) }, dur(human)), w / top).color(palette(t.mode, 0)), None),
    ];
    let ps: Vec<Value> = active(c.prop(comp, "projects", "projects")).into_iter().filter(|p| num(&p["est_human_min"]) > 0.).collect();
    if !ps.is_empty() {
        let rows = ps
            .iter()
            .map(|p| {
                let (a, h) = (num(&p["est_agent_min"]), num(&p["est_human_min"]));
                vec![s(p, "name").to_string(), dur(a), dur(h), if a >= 5. / 60. { format!("{}×", int(h / a)) } else { "–".into() }]
            })
            .collect::<Vec<_>>();
        let h = 28. * (rows.len() + 1) as f32;
        body.extend(xp(c, "vs-proj", "By project", None, h, cx, |_| table(t, &["Project", "Agent", "Estimated", "Estimated ÷ agent"], rows)));
    }
    Some(card(c, "vs", Some("Agent vs estimated human"), None, body))
}

/// A reason a task waited on you: its name and the colour the design gives it.
fn reason(c: &Ctx, name: &str) -> Option<(&'static str, Hsla)> {
    let (label, cat) = match name {
        "human" => ("Human tasks", "test"),
        "question" => ("Questions", "build"),
        "attention" => ("Attention", "lint"),
        "api_error" => ("API errors", "git"),
        _ => return None,
    };
    Some((label, category(&c.t, cat).1))
}

pub fn wait(c: &Ctx, comp: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let t = &c.t;
    let w = c.prop(comp, "waiting", "waiting");
    let total = num(&w["minutes"]);
    if total <= 0. {
        return Some(card(c, "wait", Some("Waiting on you"), None, vec![none(t, "0m waiting")]));
    }
    let top: Vec<Value> = arr(w, "tasks").iter().filter(|x| num(&x["minutes"]) > 0.).cloned().collect();
    let rs: Vec<(&str, Hsla, f64)> = arr(w, "reasons").iter().filter_map(|r| reason(c, s(r, "name")).map(|(l, col)| (l, col, num(&r["minutes"])))).filter(|r| r.2 > 0.).collect();
    let mut line = format!("Added up across {}", pl(top.len() as f64, "task", "tasks"));
    if let Some(first) = top.first().filter(|x| num(&x["minutes"]) / total > 0.5) {
        line.push_str(&format!(", the top task is {} of this", dur(num(&first["minutes"]))));
    }
    let mut body = vec![big_count(c, "wait", 80, total, dur), small(t, line).into_any_element()];
    if !rs.is_empty() {
        body.push(stack(c, "wait-stack", 80, 400, 12., rs.iter().map(|r| (r.2, r.1, format!("{}: {}", r.0, dur(r.2)))).collect()));
        body.push(legend(t, rs.iter().map(|r| (r.1, format!("{} {}", r.0, dur(r.2)))).collect()).into_any_element());
    }
    let max = top.first().map_or(1., |x| num(&x["minutes"])).max(1e-9);
    let row = |c: &Ctx, i: usize, x: &Value, cx: &mut Context<MainWindow>| {
        let r = s(x, "task").to_string();
        let open: Click = Box::new(cx.listener(move |m, _, _, cx| m.open_task(r.clone(), cx)));
        brow(c, &format!("wait-{i}"), i, Brow::new(s(x, "title"), dur(num(&x["minutes"])), num(&x["minutes"]) / max).pre(s(x, "task")), Some(open))
    };
    let first: Vec<AnyElement> = top.iter().take(4).enumerate().map(|(i, x)| row(c, i, x, cx)).collect();
    body.push(div().flex().flex_col().children(first).into_any_element());
    if top.len() > 4 {
        let next = (top.len() - 4).min(10);
        body.extend(xp(c, "wait-next", &format!("Next {next}"), None, 44. * next as f32, cx, |cx| {
            div().flex().flex_col().children(top.iter().enumerate().skip(4).take(10).map(|(i, x)| row(c, i, x, cx)).collect::<Vec<_>>())
        }));
    }
    Some(card(c, "wait", Some("Waiting on you"), None, body))
}

pub fn usage(c: &Ctx, comp: &Value, _cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let t = &c.t;
    let u = &c.prop(comp, "usage", "usage")["seven_day"];
    let days: Vec<&Value> = arr(u, "by_day").iter().collect();
    if !u.is_object() || days.iter().all(|d| d["peak"].is_null()) {
        return Some(card(c, "usage", Some("Usage"), None, vec![none(t, "No usage samples")]));
    }
    let list: Vec<Col> = days
        .iter()
        .map(|d| {
            let p = d["peak"].as_f64();
            let tip = match p {
                Some(p) => format!("{}, {}: peak {}", weekday(s(d, "date")), day(s(d, "date")), pct(p)),
                None => format!("{}, {}: no samples", weekday(s(d, "date")), day(s(d, "date"))),
            };
            Col { value: p.unwrap_or(0.), tip, dim: false }
        })
        .collect();
    let labels = days.iter().map(|d| if c.month { day(s(d, "date")) } else { weekday(s(d, "date")) }).collect();
    let fig = || div().text_size(px(24.)).font_weight(FontWeight::BOLD).line_height(px(29.)).whitespace_nowrap();
    let stat = |label: &str, el: AnyElement| div().flex().flex_col().child(muted(t, label.to_string())).child(el);
    let body = vec![
        div()
            .flex()
            .gap_x(px(24.))
            .child(stat("7-day peak", motion::count(fig(), c.id("usage-peak"), 80, num(&u["peak"]), pct)))
            .child(stat("Average", motion::count(fig(), c.id("usage-avg"), 80, num(&u["avg"]), pct)))
            .into_any_element(),
        div().flex().flex_col().gap(px(4.)).child(cols(c, "usage", 144., 20, 100., &list, None, vec![])).child(axis(t, labels)).into_any_element(),
    ];
    Some(card(c, "usage", Some("Usage"), None, body))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's `test` attribute.
    use super::{mult, say, weeks};

    #[test]
    fn multipliers_and_working_time_read_as_the_design_writes_them() {
        assert_eq!(mult(4.24), "4.2×");
        assert_eq!(mult(3.0), "3×");
        assert_eq!(mult(12.4), "12×");
        assert_eq!(weeks(0.01), "under 0.1 weeks");
        assert_eq!(weeks(1.0), "1 week");
        assert_eq!(weeks(2.54), "2.5 weeks");
        assert_eq!(say(40.), "40 estimated hours is about 5.0 eight-hour working days.");
        assert!(say(400.).starts_with("400 estimated hours is about 10 forty-hour weeks, or 2 months"));
    }
}
