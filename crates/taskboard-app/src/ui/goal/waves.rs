//! The goal page's tasks: one card per wave (its state, when it ran or what it waits for, and the
//! review stop after it), the tasks with no wave, then tasks of other goals that also finish this
//! one. A wave the goal stopped at, held or failed offers its one signal: let the goal go on.
use super::*;
use crate::ui::status::{self, St};

/// A wave card's header.
#[derive(Clone, Debug, PartialEq)]
pub struct Head {
    pub title: String,
    /// "Done Tuesday 10:45 AM", "Now", "After wave 2 and your review".
    pub meta: String,
    /// The dot: done, now, failed or held, later.
    pub dot: St,
    /// "Review stop after this wave".
    pub stop: bool,
}

/// What a stopped, held or failed wave says, and its button.
#[derive(Clone, Debug, PartialEq)]
pub struct Gate {
    pub bad: bool,
    pub text: String,
    pub button: String,
    pub wave: i64,
}

/// One row: the status row plus its ref, title and PR.
#[derive(Clone, Debug, PartialEq)]
pub struct WRow {
    pub r: String,
    pub title: String,
    pub row: status::Row,
    pub pr: String,
    pub pr_url: Option<String>,
}

/// A card: its header (none when the goal has no waves), the gate under it, its rows.
pub struct Card {
    pub key: String,
    pub head: Option<Head>,
    pub gate: Option<Gate>,
    pub rows: Vec<WRow>,
}

fn stopping(w: &Value) -> bool {
    b(w, "stop_after") && fmt::opt_s(w, "released_at").is_none()
}

/// "Done Tuesday 10:45 AM"; the day left out today.
fn done_at(iso: &str) -> String {
    let Some(t) = fmt::parse(iso) else { return "Done".into() };
    let l = fmt::local(t);
    let today = fmt::local(fmt::now()).date_naive();
    let day = match (today - l.date_naive()).num_days() {
        0 => String::new(),
        1 => "yesterday ".into(),
        2..=6 => format!("{} ", l.format("%A")),
        _ => format!("{} ", l.format("%b %-d")),
    };
    format!("Done {day}{}", l.format("%-I:%M %p"))
}

pub fn head(w: &Value, prev: Option<&Value>, tasks: &[&Value]) -> Head {
    let n = i(w, "wave");
    let title = match fmt::opt_s(w, "name") {
        Some(name) => format!("Wave {n} · {name}"),
        None => format!("Wave {n}"),
    };
    let (meta, dot) = match s(w, "state") {
        "done" => {
            let last = tasks.iter().filter_map(|t| fmt::opt_s(t, "finished_at").or(fmt::opt_s(t, "when"))).filter(|x| fmt::parse(x).is_some()).max_by_key(|x| fmt::parse(x));
            (last.map(done_at).unwrap_or_else(|| "Done".into()), St::Done)
        }
        "stopped" => ("Done · waiting for your review".into(), St::Needs),
        "failed" => ("A task failed".into(), St::Failed),
        "held" => ("Held".into(), St::Needs),
        "running" => ("Now".into(), St::Working),
        _ => match prev {
            Some(p) if stopping(p) || s(p, "state") == "stopped" => (format!("After wave {} and your review", i(p, "wave")), St::Queued),
            Some(p) => (format!("After wave {}", i(p, "wave")), St::Queued),
            None => ("Next".into(), St::Queued),
        },
    };
    Head { title, meta, dot, stop: stopping(w) && !matches!(s(w, "state"), "stopped" | "done") }
}

/// The gate under a wave the goal stopped at, that's held, or that a task failed in. A failed wave
/// the goal already went on past (`passed`) stays `failed` but has nothing left to ask.
pub fn gate(w: &Value, next: Option<&Value>) -> Option<Gate> {
    let wave = i(w, "wave");
    let on = next.map(|n| format!("wave {}", i(n, "wave"))).unwrap_or_else(|| "the rest of the goal".into());
    let failed: Vec<&str> = arr(w, "failed").iter().filter_map(|x| x.as_str()).collect();
    if s(w, "state") == "held" || (b(w, "held") && s(w, "state") == "running") {
        return Some(Gate { bad: false, text: format!("Wave {wave} is held."), button: format!("Let wave {wave} start"), wave });
    }
    if s(w, "state") == "stopped" {
        return Some(Gate { bad: false, text: format!("Wave {wave} is done. Look it over, then let {on} start."), button: format!("Continue to {on}"), wave });
    }
    if (s(w, "state") == "failed" || !failed.is_empty()) && !b(w, "passed") {
        return Some(Gate { bad: true, text: format!("{} failed. {} waits.", failed.join(", "), fmt::cap(&on)), button: "Continue anyway".into(), wave });
    }
    None
}

/// The PR column: "PR #398 merged", "PR #401 · In review", the branch, "Ships one PR", "No PR".
pub fn pr_cell(t: &Value) -> String {
    if has_pr(t) {
        let n = num_text(&t["pr"]["num"]);
        if s(&t["pr"], "state") == "MERGED" {
            return format!("PR #{n} merged");
        }
        return match fmt::opt_s(&t["pr"]["stage"], "label") {
            Some(l) => format!("PR #{n} · {l}"),
            None => format!("PR #{n}"),
        };
    }
    if let Some(why) = fmt::opt_s(t, "no_pr") {
        return format!("PR canceled: {why}");
    }
    if let Some(br) = fmt::opt_s(t, "branch") {
        return br.to_string();
    }
    match t["ships_pr"].as_bool() {
        Some(true) if s(t, "status") != "done" => "Ships one PR".into(),
        Some(false) => "No PR".into(),
        _ => String::new(),
    }
}

pub fn wrow(t: &Value) -> WRow {
    WRow {
        r: fmt::ref_of(t, "T"),
        title: s(t, "title").to_string(),
        row: status::row(t),
        pr: pr_cell(t),
        pr_url: t["pr"]["url"].as_str().filter(|u| !u.is_empty()).map(str::to_string),
    }
}

/// The goal's cards, in wave order.
pub fn cards(g: &Value) -> Vec<Card> {
    let tasks = arr(g, "tasks");
    let ws = arr(g, "waves");
    let mut out = vec![];
    for (k, w) in ws.iter().enumerate() {
        let mine: Vec<&Value> = tasks.iter().filter(|t| t["wave"] == w["wave"]).collect();
        out.push(Card {
            key: format!("w{}", i(w, "wave")),
            head: Some(head(w, k.checked_sub(1).map(|p| &ws[p]), &mine)),
            gate: gate(w, ws.get(k + 1)),
            rows: mine.into_iter().map(wrow).collect(),
        });
    }
    let loose: Vec<WRow> = tasks.iter().filter(|t| ws.is_empty() || t["wave"].is_null()).map(wrow).collect();
    if !loose.is_empty() {
        let head = (!ws.is_empty()).then(|| Head { title: "No wave".into(), meta: String::new(), dot: St::Queued, stop: false });
        out.push(Card { key: "loose".into(), head, gate: None, rows: loose });
    }
    let shared = arr(g, "shared");
    if !shared.is_empty() {
        out.push(Card {
            key: "shared".into(),
            head: Some(Head { title: "From other goals".into(), meta: String::new(), dot: St::Queued, stop: false }),
            gate: None,
            rows: shared.iter().map(wrow).collect(),
        });
    }
    out
}

// ------------------------------------------------------------------ render

fn head_el(t: &Theme, h: Head) -> Div {
    let (_, _, dot) = h.dot.colors(t);
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(18.))
        .py(px(10.))
        .bg(t.panel_2)
        .border_b_1()
        .border_color(t.divider)
        .text_size(px(13.))
        .font_weight(FontWeight::BOLD)
        .child(div().flex_none().size(px(8.)).rounded(px(4.)).bg(if h.dot == St::Queued { t.border_2 } else { dot }))
        .child(h.title)
        .when(!h.meta.is_empty(), |d| d.child(div().font_weight(FontWeight::NORMAL).text_color(t.muted).child(h.meta)))
        .when(h.stop, |d| {
            d.child(div().ml_auto().px(px(8.)).py(px(2.)).rounded(px(9.)).bg(t.goal_soft).text_color(t.goal).text_size(px(11.)).child("Review stop after this wave"))
        })
}

fn gate_el(m: &MainWindow, t: &Theme, gr: &str, g: Gate, cx: &mut Context<MainWindow>) -> Div {
    let busy = m.goal_page.busy.contains(&format!("wave-continue:{gr}:{}", g.wave));
    let id = SharedString::from(format!("wave-continue-{}", g.wave));
    let (gr2, n) = (gr.to_string(), g.wave);
    let btn = if busy { kit::disabled(kit::btn_primary(t, id, "Continuing…")) } else { kit::btn_primary(t, id, g.button).on_click(cx.listener(move |m, _, _, cx| continue_wave(m, &gr2, n, cx))) };
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(18.))
        .py(px(10.))
        .bg(if g.bad { t.down_soft } else { t.warn_soft })
        .border_b_1()
        .border_color(t.divider)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .text_size(px(13.))
                .text_color(if g.bad { t.text } else { t.warn_text })
                .child(div().flex_1().min_w_0().child(g.text))
                .child(div().flex_none().child(btn)),
        )
        .children(inline_note(m, t, &format!("wgate:{gr}")))
}

fn row_el(t: &Theme, x: WRow, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let (loud, _, _) = x.row.st.colors(t);
    let (o1, o2) = (x.r.clone(), x.r.clone());
    let (hover, link) = (t.panel_2, t.accent);
    let cell = |d: Div, grow: f32| {
        let mut d = d.min_w_0();
        d.style().flex_grow = Some(grow);
        d.style().flex_shrink = Some(1.);
        d.style().flex_basis = Some(relative(0.).into());
        d
    };
    let pr = div().flex_none().w(px(120.)).min_w_0().truncate().text_size(px(12.)).text_color(t.muted).child(x.pr.clone());
    let pr = match x.pr_url.clone() {
        Some(url) => div()
            .id(SharedString::from(format!("goal-pr-{}", x.r)))
            .flex_none()
            .child(pr.cursor_pointer().hover(move |s| s.text_color(link).underline()))
            .on_click(move |_, _, cx| cx.open_url(&url))
            .into_any_element(),
        None => pr.into_any_element(),
    };
    div()
        .id(SharedString::from(format!("goal-task-{}", x.r)))
        .flex()
        .items_center()
        .gap(px(12.))
        .h(px(36.))
        .px(px(18.))
        .border_b_1()
        .border_color(t.divider)
        .text_size(px(13.))
        .hover(move |s| s.bg(hover))
        .cursor_pointer()
        .on_click(cx.listener(move |m, _, _, cx| m.open_task(o1.clone(), cx)))
        .child(div().flex_none().w(px(86.)).child(status::chip(t, x.row.st)))
        .child(div().flex_none().w(px(36.)).text_size(px(11.)).font_family(t.mono_font.clone()).text_color(t.muted).child(x.r.clone()))
        .child(cell(
            div().flex().child(
                div()
                    .id(SharedString::from(format!("goal-task-title-{o2}")))
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .hover(move |s| s.text_color(link).underline())
                    .child(x.title),
            ),
            1.,
        ))
        .child(cell(div().truncate().text_size(px(12.)).text_color(if x.row.loud { loud } else { t.muted }).child(x.row.line), 1.1))
        .child(pr)
        .child(div().flex_none().w(px(44.)).whitespace_nowrap().text_size(px(12.)).text_color(t.muted).text_right().child(x.row.age))
}

/// How many list items card `c` takes: its header (with its gate) when it has one, then each row.
pub fn pieces(c: &Card) -> usize {
    (c.head.is_some() || c.gate.is_some()) as usize + c.rows.len()
}

/// Piece `k` of card `c` (`pieces`) as a list item: the card's border down its sides, its top edge
/// and corners on the first piece, its bottom ones on the last, and 12px above every card but the first.
pub fn piece(m: &MainWindow, t: &Theme, gr: &str, cards: &[Card], c: usize, k: usize, cx: &mut Context<MainWindow>) -> AnyElement {
    let card = &cards[c];
    let head = (card.head.is_some() || card.gate.is_some()) as usize;
    let (top, bottom) = (k == 0, k + 1 == pieces(card));
    let body = match k.checked_sub(head) {
        Some(r) => row_el(t, card.rows[r].clone(), cx).into_any_element(),
        None => div().flex().flex_col().children(card.head.clone().map(|h| head_el(t, h))).children(card.gate.clone().map(|g| gate_el(m, t, gr, g, cx))).into_any_element(),
    };
    let edge = div()
        .flex()
        .flex_col()
        .border_l_1()
        .border_r_1()
        .border_color(t.border)
        .bg(t.card)
        .overflow_hidden()
        .when(top, |d| d.border_t_1().rounded_t(px(12.)))
        .when(bottom, |d| d.border_b_1().rounded_b(px(12.)))
        .child(body);
    let edge = if top { edge.id(SharedString::from(format!("goal-wave-{}", card.key))).into_any_element() } else { edge.into_any_element() };
    div().when(top && c > 0, |d| d.pt(px(12.))).child(edge).into_any_element()
}

/// "Continue to wave 2": `POST goals/:g/waves/:n/continue`; "The goal goes on" under the gate.
pub fn continue_wave(m: &mut MainWindow, gr: &str, n: i64, cx: &mut Context<MainWindow>) {
    run(m, format!("wave-continue:{gr}:{n}"), Some(format!("wgate:{gr}")), false, format!("goals/{gr}/waves/{n}/continue"), json!({}), cx, move |_, v, _| {
        if v["let_start"] == true { format!("Wave {n} can start") } else { "The goal goes on".into() }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;

    #[test]
    fn waves_say_when_they_ran_and_what_they_wait_for() {
        let w1 = json!({"wave": 1, "name": "Foundation", "state": "done"});
        let w2 = json!({"wave": 2, "state": "running", "stop_after": true});
        let w3 = json!({"wave": 3, "state": "waiting"});
        let h = head(&w2, Some(&w1), &[]);
        assert_eq!((h.title.as_str(), h.meta.as_str(), h.dot, h.stop), ("Wave 2", "Now", St::Working, true));
        assert_eq!(head(&w3, Some(&w2), &[]).meta, "After wave 2 and your review");
        assert_eq!(head(&w1, None, &[]).title, "Wave 1 · Foundation");
        let stopped = json!({"wave": 1, "state": "stopped"});
        assert_eq!(gate(&stopped, Some(&w2)).unwrap().button, "Continue to wave 2");
        assert_eq!(gate(&w1, Some(&w2)), None);
    }

    #[test]
    fn a_failed_wave_stops_asking_once_the_goal_went_on() {
        let w6 = json!({"wave": 6, "state": "running"});
        let holding = json!({"wave": 5, "state": "failed", "failed": ["T36"], "passed": false});
        assert_eq!(gate(&holding, Some(&w6)).unwrap().text, "T36 failed. Wave 6 waits.");
        let released = json!({"wave": 5, "state": "failed", "failed": ["T36"], "passed": true});
        assert_eq!(gate(&released, Some(&w6)), None);
        assert_eq!(gate(&released, None), None);
    }

    #[test]
    fn the_pr_column_says_where_the_work_lands() {
        assert_eq!(pr_cell(&json!({"pr": {"num": 398, "state": "MERGED"}})), "PR #398 merged");
        assert_eq!(pr_cell(&json!({"pr": {"num": 4, "state": "OPEN", "stage": {"label": "In review"}}})), "PR #4 · In review");
        assert_eq!(pr_cell(&json!({"status": "queued", "ships_pr": true})), "Ships one PR");
        assert_eq!(pr_cell(&json!({"status": "done", "ships_pr": true, "no_pr": "not needed"})), "PR canceled: not needed");
    }

    #[test]
    fn a_goal_without_waves_is_one_card_without_a_header() {
        let g = json!({"ref": "G1", "tasks": [{"ref": "T1", "status": "queued"}, {"ref": "T2", "status": "done"}]});
        let cs = cards(&g);
        assert_eq!(cs.len(), 1);
        assert!(cs[0].head.is_none());
        assert_eq!(cs[0].rows.len(), 2);
    }
}
