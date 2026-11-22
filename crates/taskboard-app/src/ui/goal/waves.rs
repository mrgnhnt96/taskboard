//! The goal page's tasks: one card per wave (its state, when it ran or what it waits for, and the
//! review stop after it), the tasks with no wave, then tasks of other goals that also finish this
//! one, a card for each goal they came from. A wave the goal stopped at, held or failed offers its
//! one signal: let the goal go on.
use super::*;
use crate::ui::status::{self, St};
use crate::ui::stop::Kind;

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
    /// Held, failed or ready for review, said as the goal's stop says it.
    pub kind: Kind,
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
    pub design: bool,
    pub research: bool,
    pub row: status::Row,
    pub pr: String,
    /// Its start failed, or waits on a retry timer: the row offers to try it again now.
    pub retry: bool,
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
        "stopped" => (format!("{} Done · ready for review", Kind::Review.glyph()), St::Needs),
        "failed" => (Kind::Failed.label(), St::Failed),
        "held" => (Kind::Held.label(), St::Needs),
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
        return Some(Gate { kind: Kind::Held, bad: false, text: format!("Wave {wave} is held."), button: format!("Let wave {wave} start"), wave });
    }
    if s(w, "state") == "stopped" {
        return Some(Gate { kind: Kind::Review, bad: false, text: format!("Wave {wave} is done. Look it over, then let {on} start."), button: format!("Continue to {on}"), wave });
    }
    if (s(w, "state") == "failed" || !failed.is_empty()) && !b(w, "passed") {
        return Some(Gate { kind: Kind::Failed, bad: true, text: format!("{} failed. {} waits.", failed.join(", "), fmt::cap(&on)), button: "Continue anyway".into(), wave });
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
        return format!("No PR: {why}");
    }
    if let Some(br) = fmt::opt_s(t, "branch") {
        return br.to_string();
    }
    if let Some(n) = t["same_pr"].as_i64() {
        return format!("Shares T{n}'s PR");
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
        design: kit::is_design(t),
        research: kit::is_research(t),
        row: status::row(t),
        pr: pr_cell(t),
        retry: start_failed(t),
    }
}

/// A task whose start gave up, or one queued to try its start again later.
fn start_failed(t: &Value) -> bool {
    (s(t, "status") == "needs" && s(t, "needs_reason") == "start_failed") || crate::ui::task_panel::start_retry_at(t).is_some()
}

/// Run a failed start again now.
pub fn try_again(m: &mut MainWindow, r: &str, cx: &mut Context<MainWindow>) {
    run(m, format!("task-start:{r}"), None, true, format!("tasks/{r}/start"), json!({"mode": "new"}), cx, |_, _, _| "Starting".into());
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
    // Shared tasks, a card for each goal they came from: "From G6 Import from CSV".
    for (from, rows) in crate::ui::projects::shared_by_goal(g) {
        let title = format!("From {} {}", fmt::ref_of(from, "G"), s(from, "name")).trim_end().to_string();
        out.push(Card {
            key: format!("shared:{}", fmt::ref_of(from, "G")),
            head: Some(Head { title, meta: String::new(), dot: St::Queued, stop: false }),
            gate: None,
            rows: rows.into_iter().map(wrow).collect(),
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
    let (fg, soft, _) = g.kind.colors(t);
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .px(px(18.))
        .py(px(10.))
        .bg(soft)
        .border_b_1()
        .border_color(t.divider)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .text_size(px(13.))
                .text_color(t.text)
                .child(div().flex_none().text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(fg).child(g.kind.label().to_uppercase()))
                .child(div().flex_1().min_w_0().child(g.text))
                .child(div().flex_none().child(btn)),
        )
        .children(inline_note(m, t, &format!("wgate:{gr}")))
}

/// A waves column narrower than this stacks each task row onto two lines (status and ref beside
/// the title, then its latest, PR and time under the title), so titles and status words keep
/// their room.
pub const ROWS_STACK_BELOW: f32 = 760.;
/// The chip's width; it borrows 6px of the gap after it.
const CHIP_W: f32 = 92.;
const REF_W: f32 = 44.;
const ROW_GAP: f32 = 12.;
/// Where a stacked row's second line starts: under the title, past the chip and ref.
const STACK_INDENT: f32 = CHIP_W - 6. + ROW_GAP + REF_W + ROW_GAP;

fn row_el(m: &MainWindow, t: &Theme, x: WRow, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let stacked = m.goal_page.lists.stack_rows;
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
    // Just words: a click here opens the task like the rest of the row; its panel links the PR.
    let pr = div().flex_none().w(px(120.)).min_w_0().truncate().text_size(px(12.)).text_color(t.muted).child(kit::clipped(SharedString::from(format!("goal-task-pr-{}", x.r)), x.pr.clone()));
    // A tighter chip than the board's, borrowing 6px of the gap so the ref and title stay put.
    let chip = div().flex_none().w(px(CHIP_W)).mr(px(-6.)).child(status::chip(t, x.row.st).w(px(CHIP_W)).px(px(5.)).text_size(px(11.)));
    let r = div().flex_none().w(px(REF_W)).whitespace_nowrap().text_size(px(11.)).font_family(t.mono_font.clone()).text_color(t.muted).child(x.r.clone());
    let words = div()
        .id(SharedString::from(format!("goal-task-title-{o2}")))
        .min_w_0()
        .font_weight(FontWeight::MEDIUM)
        .hover(move |s| s.text_color(link).underline())
        .child(kit::clipped(SharedString::from(format!("goal-task-title-text-{}", x.r)), x.title));
    let title = cell(
        div()
            .flex()
            .child(if stacked { words.line_clamp(2) } else { words.truncate() })
            .items_center()
            .gap(px(8.))
            .when(x.design, |d| d.child(kit::design_pill(t)))
            .when(x.research, |d| d.child(kit::research_pill(t))),
        1.,
    );
    let line = |grow| cell(div().truncate().text_size(px(12.)).text_color(if x.row.loud { loud } else { t.muted }).child(kit::clipped(SharedString::from(format!("goal-task-line-{}", x.r)), x.row.line.clone())), grow);
    let age = div().flex_none().w(px(44.)).whitespace_nowrap().text_size(px(12.)).text_color(t.muted).text_right().child(x.row.age.clone());
    let retry = x.retry.then(|| retry_btn(m, t, &x.r, cx));
    // The task open in the panel: a soft accent fill and a 2px accent edge on its left.
    let on = crate::app::open_ref(m) == Some(x.r.as_str());
    let base = focus::stop(t, div().id(SharedString::from(format!("goal-task-{}", x.r))))
        .relative()
        .when(on, |d| d.bg(t.accent_soft).child(div().absolute().left_0().top_0().bottom_0().w(px(2.)).bg(t.accent)))
        .px(px(18.))
        .border_b_1()
        .border_color(t.divider)
        .text_size(px(13.))
        .when(!on, |d| d.hover(move |s| s.bg(hover)))
        .cursor_pointer()
        .on_click(cx.listener(move |m, _, _, cx| m.open_task(o1.clone(), cx)));
    if stacked {
        return base
            .flex()
            .flex_col()
            .gap(px(4.))
            .py(px(8.))
            .child(div().flex().items_center().gap(px(ROW_GAP)).child(chip).child(r).child(title))
            .child(div().flex().items_center().gap(px(ROW_GAP)).pl(px(STACK_INDENT)).child(line(1.)).children(retry).child(pr).child(age));
    }
    base.flex().items_center().gap(px(ROW_GAP)).h(px(36.)).child(chip).child(r).child(title).child(line(1.1)).children(retry).child(pr).child(age)
}

fn retry_btn(m: &MainWindow, t: &Theme, r: &str, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let id = SharedString::from(format!("goal-task-retry-{r}"));
    if m.goal_page.busy.contains(&format!("task-start:{r}")) {
        return kit::disabled(kit::btn_small(t, id, "Starting…")).flex_none();
    }
    let r = r.to_string();
    kit::btn_small(t, id, "Try again").flex_none().on_click(cx.listener(move |m, _, _, cx| {
        cx.stop_propagation();
        try_again(m, &r, cx);
    }))
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
        Some(r) => row_el(m, t, card.rows[r].clone(), cx).into_any_element(),
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
        assert_eq!(gate(&stopped, Some(&w2)).unwrap().kind, Kind::Review);
        assert_eq!(head(&stopped, None, &[]).meta, "◐ Done · ready for review");
        assert_eq!(head(&json!({"wave": 2, "state": "held"}), Some(&w1), &[]).meta, "‖ Held");
        assert_eq!(gate(&w1, Some(&w2)), None);
    }

    #[test]
    fn a_design_task_row_carries_the_design_pill() {
        assert!(wrow(&json!({"ref": "T1", "title": "Concepts", "kind": "design"})).design);
        assert!(!wrow(&json!({"ref": "T2", "title": "Build it", "kind": "agent"})).design);
        assert!(kit::is_design(&json!({"kind": "design"})) && !kit::is_design(&json!({"kind": "human"})));
        assert!(wrow(&json!({"ref": "T3", "title": "Compare queues", "kind": "research"})).research);
    }

    #[test]
    fn a_task_whose_start_failed_offers_to_try_again() {
        assert!(wrow(&json!({"ref": "T1", "status": "needs", "needs_reason": "start_failed"})).retry);
        assert!(wrow(&json!({"ref": "T2", "status": "queued", "start_retry_at": "2026-11-02T15:05:00Z"})).retry);
        assert!(!wrow(&json!({"ref": "T3", "status": "queued", "start_retry_at": "2026-11-02T15:05:00Z", "starting": true})).retry);
        assert!(!wrow(&json!({"ref": "T4", "status": "queued"})).retry);
        assert!(!wrow(&json!({"ref": "T5", "status": "needs", "needs_reason": "question"})).retry);
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
        assert_eq!(pr_cell(&json!({"status": "queued", "ships_pr": true, "same_pr": 1666})), "Shares T1666's PR");
        assert_eq!(pr_cell(&json!({"status": "queued", "ships_pr": true, "same_pr": null})), "Ships one PR");
        assert_eq!(pr_cell(&json!({"status": "done", "ships_pr": true, "no_pr": "not needed"})), "No PR: not needed");
    }

    #[test]
    fn shared_tasks_sit_under_the_goal_they_came_from() {
        let g6 = json!({"id": 6, "ref": "G6", "name": "Import from CSV"});
        let g2 = json!({"id": 2, "ref": "G2", "name": "Search filters"});
        let g = json!({"ref": "G1", "tasks": [{"ref": "T1", "status": "queued"}], "shared": [
            {"id": 9, "ref": "T9", "status": "queued", "goal": g6},
            {"id": 4, "ref": "T4", "status": "done", "goal": g2},
            {"id": 7, "ref": "T7", "status": "queued", "goal": g6},
        ]});
        let cs = cards(&g);
        let shared: Vec<(&str, String, Vec<&str>)> = cs[1..].iter().map(|c| (c.key.as_str(), c.head.as_ref().unwrap().title.clone(), c.rows.iter().map(|r| r.r.as_str()).collect())).collect();
        assert_eq!(
            shared,
            vec![("shared:G2", "From G2 Search filters".to_string(), vec!["T4"]), ("shared:G6", "From G6 Import from CSV".to_string(), vec!["T7", "T9"])],
            "a card per source goal, lowest ref first, each in id order"
        );
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
