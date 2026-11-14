//! Needs you on screen: a virtual list of the page's lines, so only what's in view is built.
use super::groups::{self, Do, Entry, GoalRow, Group, Kind, LaterLine, Note, Row};
use super::*;

const LATER_MENU: &str = "home-later:";
const MORE_MENU: &str = "home-more:";
const AFTER_MENU: &str = "home-after:";

/// A human task's note field and what it's for.
struct Composer {
    input: kit::Input,
    kind: Note,
    /// Waiting on someone's date.
    until: Option<String>,
}

impl Composer {
    fn new(cx: &mut App) -> Composer {
        Composer { input: kit::Input::new(cx, "", true), kind: Note::Done, until: None }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Line {
    Head,
    Stats,
    Empty,
    Group(usize),
    /// Group, row, open.
    Row(usize, usize, bool),
    /// Group, row, kid, open.
    Kid(usize, usize, usize, bool),
    /// Open.
    Later(bool),
    LaterItem(usize),
    End,
}

struct View {
    asks: Rc<Vec<Ask>>,
    project: String,
    stats: Rc<Vec<Stat>>,
    names: Rc<Vec<String>>,
    counts: Vec<(String, usize)>,
    groups: Vec<Group>,
    later: Option<LaterLine>,
    sub: String,
}

impl View {
    fn of(built: &Built, project: &str, home: &Value) -> View {
        let shown: Vec<&Ask> = built.asks.iter().filter(|a| project == "all" || a.project == project).collect();
        let groups = groups::groups_vm(&shown);
        let later = groups::later_vm(home, project);
        let agents = groups.iter().find(|g| g.kind == Kind::Waiting).map_or(0, |g| g.count);
        let sub = groups::waiting_line(agents + later.as_ref().map_or(0, |l| l.waiting));
        let counts = built.names.iter().map(|p| (p.clone(), built.asks.iter().filter(|a| &a.project == p).count())).collect();
        View { asks: built.asks.clone(), project: project.to_string(), stats: built.stats.clone(), names: built.names.clone(), counts, groups, later, sub }
    }
}

#[derive(Default)]
pub struct State {
    list: Option<ListState>,
    view: Option<Rc<View>>,
    lines: Vec<Line>,
    /// A line's height changed with the lines the same: a note field took another shape.
    remeasure: bool,
    later_open: bool,
    notes: HashMap<String, Composer>,
}

fn has_body(a: &Ask) -> bool {
    a.human || a.answer || !a.help.is_empty() || !a.actions.is_empty() || !a.line.is_empty() || (!a.text.is_empty() && a.text != a.title)
}

fn goal_open(g: &GoalRow) -> bool {
    g.kids.iter().any(starts_open) != flipped(&g.key)
}

fn lines_of(v: &View, later_open: bool) -> Vec<Line> {
    let mut out = vec![Line::Head];
    if !v.stats.is_empty() {
        out.push(Line::Stats);
    }
    if v.groups.is_empty() {
        out.push(Line::Empty);
    }
    for (gi, g) in v.groups.iter().enumerate() {
        out.push(Line::Group(gi));
        for (ri, r) in g.rows.iter().enumerate() {
            match r {
                Row::One(a) => out.push(Line::Row(gi, ri, has_body(a) && is_open(a))),
                Row::Goal(x) => {
                    let open = goal_open(x);
                    out.push(Line::Row(gi, ri, open));
                    if open {
                        out.extend(x.kids.iter().enumerate().map(|(ki, a)| Line::Kid(gi, ri, ki, has_body(a) && is_open(a))));
                    }
                }
            }
        }
    }
    if let Some(l) = &v.later {
        out.push(Line::Later(later_open));
        if later_open {
            out.extend((0..l.items.len()).map(Line::LaterItem));
        }
    }
    out.push(Line::End);
    out
}

/// Brings the view and the list up to what `m` holds, keeping the place scrolled to.
fn prepare(m: &mut MainWindow) {
    let built = Built::of(m);
    let project = m.filters.project.clone();
    let home = m.data.home.clone().unwrap_or_default();
    let st = &mut m.home;
    let held = st.view.as_ref().filter(|v| Rc::ptr_eq(&v.asks, &built.asks) && v.project == project).cloned();
    let same_project = st.view.as_ref().is_some_and(|v| v.project == project);
    let fresh = held.is_none();
    let v = held.unwrap_or_else(|| Rc::new(View::of(&built, &project, &home)));
    let lines = lines_of(&v, st.later_open);
    let count = lines.len();
    match &st.list {
        None => st.list = Some(ListState::new(count, ListAlignment::Top, px(600.))),
        Some(l) if fresh || st.remeasure || lines != st.lines => {
            let top = l.logical_scroll_top();
            l.reset(count);
            if same_project {
                l.scroll_to(ListOffset { item_ix: top.item_ix.min(count - 1), offset_in_item: top.offset_in_item });
            }
        }
        _ => {}
    }
    st.remeasure = false;
    st.lines = lines;
    st.view = Some(v);
}

pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    prepare(m);
    let filter = ip::render_project_filter(m, &t, PROJECT_FILTER, window, cx);
    let menu = m.home.view.clone().and_then(|v| row_menu(m, &t, &v, cx));
    let page = div().id("home-page").flex().flex_1().min_w_0().h_full().bg(t.bg);
    let Some(list) = m.home.list.clone() else { return page.into_any_element() };
    page.child(gpui_kit::list(list, cx.processor(line_item)).flex_1().min_w_0().h_full()).children(filter).children(menu).into_any_element()
}

fn line_item(m: &mut MainWindow, ix: usize, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let (Some(v), Some(line)) = (m.home.view.clone(), m.home.lines.get(ix).copied()) else { return div().into_any_element() };
    let inner = match line {
        Line::Head => head_line(&t, &v, cx),
        Line::Stats => stats_line(&t, &v),
        Line::Empty if m.data.home.is_none() => kit::empty(&t, if m.down.is_some() { "Can’t load the board." } else { "Loading…" }),
        Line::Empty => kit::empty(&t, "Nothing needs you.").pt(px(32.)),
        Line::Group(gi) => group_label(&t, &v.groups[gi]),
        Line::Row(gi, ri, open) => row_line(m, &t, &v.groups[gi], ri, open, window, cx),
        Line::Kid(gi, ri, ki, open) => kid_line(m, &t, &v.groups[gi], ri, ki, open, window, cx),
        Line::Later(open) => later_line(&t, &v, open, cx),
        Line::LaterItem(i) => later_item(&t, &v, i, cx),
        Line::End => div().h(px(32.)),
    };
    div().px(px(PAGE_X)).line_height(relative(1.4)).child(inner).into_any_element()
}

// ------------------------------------------------------------------ the page's top

fn head_line(t: &Theme, v: &View, cx: &mut Context<MainWindow>) -> Div {
    let tabs = ip::project_filter(t, PROJECT_FILTER, &v.project, &v.counts, cx, |m, p, cx| {
        let mut f = m.filters.clone();
        f.project = p;
        m.set_filters(f, cx);
    });
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .pt(px(22.))
        .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).text_color(t.text).child("Needs you"))
        .child(div().text_size(px(13.)).text_color(t.muted).child(v.sub.clone()))
        .child(div().flex_1())
        .when(v.names.len() > 1, |d| d.child(tabs))
}

fn stats_line(t: &Theme, v: &View) -> Div {
    let mut row = div().flex().gap(px(10.)).pt(px(16.));
    for x in v.stats.iter() {
        row = row.child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .px(px(16.))
                .py(px(12.))
                .rounded(px(10.))
                .border_1()
                .border_color(t.border)
                .bg(t.card)
                .child(div().text_size(px(20.)).font_weight(FontWeight::BOLD).text_color(if x.warn { t.warn_fg } else { t.text }).child(x.value.clone()))
                .child(div().text_size(px(12.)).text_color(t.muted).truncate().child(x.label)),
        );
    }
    row
}

fn group_label(t: &Theme, g: &Group) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .pt(px(18.))
        .pb(px(8.))
        .text_size(px(12.))
        .text_color(t.muted)
        .child(div().font_weight(FontWeight::BOLD).child(g.kind.label().to_uppercase()))
        .child(g.count.to_string())
}

// ------------------------------------------------------------------ rows

/// What a row's head line says and offers.
struct Head {
    key: String,
    above: String,
    title: String,
    link: Option<Act>,
    strong: bool,
    quiet: bool,
    tag: String,
    tag_color: Hsla,
    holds: String,
    finish: Vec<(String, Do)>,
    later: bool,
    time: String,
    /// Open or shut, on a goal's row.
    chevron: Option<bool>,
    opens: bool,
}

/// The tag's color: what's gone wrong in the failed color, an agent's in the Needs you color.
fn tag_color(t: &Theme, a: &Ask) -> Hsla {
    let st = match (a.kind.as_str(), a.sev) {
        ("Review the PR", _) => St::Review,
        ("Ready to merge", _) => St::Done,
        (_, Sev::Crit) => St::Failed,
        (_, Sev::Warn) => St::Needs,
        (_, Sev::Ok) => St::Done,
        (_, Sev::Quiet) => return t.muted,
    };
    st.colors(t).0
}

fn link_of(a: &Ask) -> Option<Act> {
    let task = a.task.as_ref().map(|(r, _)| Act::OpenTask(r.clone()));
    let issue = a.issue.as_ref().map(|b| Act::OpenIssue(b.clone()));
    let goal = a.goal.as_ref().map(|(g, _)| Act::OpenGoal(g.clone()));
    issue.or(task).or(goal)
}

/// The goal named above a row of its own, unless the title already says it.
fn above_of(a: &Ask) -> String {
    a.goal.as_ref().map(|(_, n)| n.clone()).filter(|n| !n.is_empty() && !a.title.contains(n.as_str())).unwrap_or_default()
}

fn ask_head(t: &Theme, a: &Ask, kind: Kind, nested: bool) -> Head {
    let tasks_only = Holds { tasks: a.holds.tasks.clone(), goal: false };
    let quiet = a.sev == Sev::Quiet;
    Head {
        key: a.key(),
        above: if nested { String::new() } else { above_of(a) },
        title: a.title.clone(),
        link: link_of(a),
        strong: !nested && matches!(kind, Kind::Waiting | Kind::Decide),
        quiet,
        tag: groups::tag_of(a, nested).unwrap_or_default(),
        tag_color: tag_color(t, a),
        holds: if nested { tasks_only.label() } else { a.holds.label() },
        finish: groups::finish_vm(a),
        later: a.snooze,
        time: if quiet { String::new() } else { a.time.clone() },
        chevron: None,
        opens: has_body(a),
    }
}

fn goal_head(t: &Theme, g: &GoalRow, kind: Kind, open: bool) -> Head {
    Head {
        key: g.key.clone(),
        above: String::new(),
        title: g.title.clone(),
        link: Some(Act::OpenGoal(g.goal.0.clone())),
        strong: matches!(kind, Kind::Waiting | Kind::Decide),
        quiet: g.quiet,
        tag: g.tag.clone(),
        tag_color: if g.kids.iter().all(|a| a.issue.is_some()) { g.kids.first().map_or(t.muted, |a| tag_color(t, a)) } else { t.muted },
        holds: g.holds.label(),
        finish: g.finish.iter().map(|act| (act.label().to_string(), Do::Act(act.clone()))).collect(),
        later: g.kids.iter().any(|a| a.snooze),
        time: if g.quiet { String::new() } else { g.time.clone() },
        chevron: Some(open),
        opens: true,
    }
}

fn do_btn(t: &Theme, key: &str, n: usize, label: String, act: Do, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let id = SharedString::from(format!("home-do-{n}-{key}"));
    let b = match act {
        Do::Done(_) => kit::btn_primary(t, id, label).h(px(24.)).px(px(8.)).rounded(px(6.)).text_size(px(12.)),
        _ => kit::btn_small(t, id, label),
    };
    let key = key.to_string();
    b.on_click(cx.listener(move |m, _, window, cx| {
        cx.stop_propagation();
        run_do(m, &key, &act, window, cx);
    }))
}

fn menu_btn(t: &Theme, key: &str, menu: &'static str, label: &'static str, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let id = SharedString::from(format!("{menu}{key}"));
    let open = format!("{menu}{key}");
    kit::btn_small(t, id, label).child(kit::anchor(open.clone())).on_click(cx.listener(move |m, e: &ClickEvent, _, cx| {
        cx.stop_propagation();
        let p = e.position();
        m.toggle_menu(&open, point(p.x - px(150.), p.y + px(14.)), cx);
    }))
}

fn head_el(t: &Theme, h: Head, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let key = h.key.clone();
    let color = match (h.quiet, h.strong) {
        (true, _) => t.muted,
        (_, true) => t.text,
        _ => t.text_2,
    };
    let title = div()
        .id(SharedString::from(format!("home-title-{key}")))
        .min_w_0()
        .truncate()
        .text_size(px(14.))
        .font_weight(if h.strong { FontWeight::SEMIBOLD } else { FontWeight::NORMAL })
        .text_color(color)
        .child(h.title.clone())
        .when_some(h.link.clone(), |d, act| {
            d.cursor_pointer().hover(|s| s.underline()).on_click(cx.listener(move |m, _, _, cx| {
                cx.stop_propagation();
                run_act(m, &act, cx);
            }))
        });
    let words = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(120.))
        .overflow_hidden()
        .when(!h.above.is_empty(), |d| d.child(div().text_size(px(11.5)).text_color(t.goal).truncate().child(h.above.clone())))
        .child(div().flex().min_w_0().child(title));
    let soft = |b: Stateful<Div>| if h.quiet { b.bg(gpui_kit::transparent_black()).text_color(t.text_2) } else { b };
    let buttons: Vec<_> = h.finish.iter().enumerate().map(|(n, (label, act))| soft(do_btn(t, &key, n, label.clone(), act.clone(), cx))).collect();
    let later = h.later.then(|| soft(menu_btn(t, &key, LATER_MENU, "Later ▾", cx)));
    let more = soft(menu_btn(t, &key, MORE_MENU, "More ▾", cx));
    let flip_key = key.clone();
    div()
        .id(SharedString::from(format!("home-head-{key}")))
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(14.))
        .py(px(9.))
        .when(h.opens, |d| {
            d.cursor_pointer().on_click(cx.listener(move |m, _, _, cx| {
                flip(flip_key.clone());
                m.home.remeasure = true;
                cx.notify();
            }))
        })
        .when_some(h.chevron, |d, open| d.child(chevron(open, t.muted)))
        .child(words)
        .when(!h.tag.is_empty(), |d| {
            let weight = if h.tag_color == t.muted { FontWeight::NORMAL } else { FontWeight::SEMIBOLD };
            d.child(div().flex_none().text_size(px(12.)).font_weight(weight).text_color(h.tag_color).child(h.tag.clone()))
        })
        .when(!h.holds.is_empty(), |d| d.child(div().flex_none().text_size(px(12.)).text_color(t.muted).child(h.holds.clone())))
        .child(div().flex_none().flex().items_center().gap(px(6.)).children(buttons).children(later).child(more))
        .when(!h.time.is_empty(), |d| d.child(div().flex_none().w(px(36.)).flex().justify_end().text_size(px(12.)).text_color(t.muted).child(h.time.clone())))
}

fn frame(t: &Theme, warn: bool, quiet: bool) -> Div {
    div().flex().flex_col().border_color(if warn { t.warn_line } else { t.border }).when(!quiet, |d| d.bg(t.card)).overflow_hidden()
}

fn row_line(m: &mut MainWindow, t: &Theme, g: &Group, ri: usize, open: bool, window: &mut Window, cx: &mut Context<MainWindow>) -> Div {
    let warn = g.kind == Kind::Waiting;
    match &g.rows[ri] {
        Row::One(a) => {
            let head = head_el(t, ask_head(t, a, g.kind, false), cx);
            let card = frame(t, warn, a.sev == Sev::Quiet).rounded(px(10.)).border_1().child(head);
            let card = if open { card.child(body(m, t, a, 14., window, cx)) } else { card };
            div().pb(px(6.)).child(card)
        }
        Row::Goal(x) => {
            let head = head_el(t, goal_head(t, x, g.kind, open), cx);
            let card = frame(t, warn, x.quiet).child(head);
            if open {
                card.rounded_t(px(10.)).border_t_1().border_l_1().border_r_1()
            } else {
                div().pb(px(6.)).child(card.rounded(px(10.)).border_1())
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn kid_line(m: &mut MainWindow, t: &Theme, g: &Group, ri: usize, ki: usize, open: bool, window: &mut Window, cx: &mut Context<MainWindow>) -> Div {
    let Row::Goal(x) = &g.rows[ri] else { return div() };
    let a = &x.kids[ki];
    let last = ki + 1 == x.kids.len();
    let head = head_el(t, ask_head(t, a, g.kind, true), cx).pl(px(30.));
    let card = frame(t, g.kind == Kind::Waiting, x.quiet).border_l_1().border_r_1().child(head);
    let card = if open { card.child(body(m, t, a, 30., window, cx)) } else { card };
    if last { div().pb(px(6.)).child(card.border_b_1().rounded_b(px(10.))) } else { card }
}

/// A row opened: a question's answers, a to-do's note, or the rest of what the alert said.
fn body(m: &mut MainWindow, t: &Theme, a: &Ask, left: f32, window: &mut Window, cx: &mut Context<MainWindow>) -> Div {
    let frame = div().flex().flex_col().gap(px(10.)).pl(px(left)).pr(px(14.)).pb(px(12.));
    if a.human {
        return todo_body(m, t, a, frame, window, cx);
    }
    if a.answer {
        return question_body(t, a, frame, cx);
    }
    alert_body(t, a, frame, cx)
}

fn ask_id(a: &Ask, part: &str) -> SharedString {
    SharedString::from(format!("home-{part}-{}", a.key()))
}

fn alert_body(t: &Theme, a: &Ask, body: Div, cx: &mut Context<MainWindow>) -> Div {
    let fixes: Vec<_> = a
        .actions
        .iter()
        .enumerate()
        .map(|(n, (label, command))| {
            let (aid, label, command) = (a.id.clone(), label.clone(), command.clone());
            kit::btn(t, ask_id(a, &format!("fix-{n}")), label.clone()).on_click(cx.listener(move |m, _, _, cx| crate::app::run_alert_action(m, &aid, n, &label, &command, cx)))
        })
        .collect();
    let mut body = body;
    if !a.line.is_empty() && a.line != a.text {
        body = body.child(div().text_size(px(13.)).text_color(t.text_2).child(ref_text(t, ask_id(a, "line"), &a.line, cx)));
    }
    if !a.text.is_empty() && a.text != a.title && a.issue.is_none() {
        body = body.child(div().text_size(px(13.)).text_color(t.text_2).child(ref_text(t, ask_id(a, "text"), &a.text, cx)));
    }
    for (n, line) in a.help.iter().enumerate() {
        body = body.child(div().text_size(px(12.)).text_color(t.muted).child(ref_text(t, ask_id(a, &format!("help-{n}")), line, cx)));
    }
    body.when(!fixes.is_empty(), |d| d.child(div().flex().flex_wrap().items_center().gap(px(8.)).children(fixes)))
}

fn question_body(t: &Theme, a: &Ask, body: Div, cx: &mut Context<MainWindow>) -> Div {
    let id = |part: &str| ask_id(a, part);
    let r = a.task.as_ref().map(|(r, _)| r.clone()).unwrap_or_default();
    let focus = body_focus(a).map(|act| kit::btn(t, id("focus"), act.label()).on_click(cx.listener(move |m, _, _, cx| run_act(m, &act, cx))));
    // An option answers in one click, and so does a picker's when it asks one single-select
    // question; otherwise its options toggle and Send presses them. The alert's suggested
    // answers answer like options.
    let chosen = picked(&r);
    let one = one_click(&a.asks);
    let option = |label: &str, desc: &str, qo: Option<(usize, usize)>| {
        let n = qo.map_or_else(|| label.to_string(), |(q, o)| format!("{q}-{o}"));
        let (r, text) = (r.clone(), label.to_string());
        let head = match qo {
            Some((_, o)) if a.picker && one => kit::btn(t, id(&format!("pick-{n}")), text).on_click(cx.listener(move |m, _, _, cx| pick(m, &r, vec![vec![o]], cx))),
            Some((q, o)) if a.picker => {
                let asks = a.asks.clone();
                let b = kit::btn(t, id(&format!("pick-{n}")), text).on_click(cx.listener(move |_, _, _, cx| {
                    let next = toggle_pick(&asks, &picked(&r), q, o);
                    PICKED.with(|p| p.borrow_mut().insert(r.clone(), next));
                    cx.notify();
                }));
                if chosen.contains(&(q, o)) { b.border_color(t.warn).bg(t.warn_soft).font_weight(FontWeight::SEMIBOLD) } else { b }
            }
            _ => kit::btn(t, id(&format!("opt-{n}")), text.clone()).on_click(cx.listener(move |m, _, _, cx| answer(m, &r, &text, cx))),
        };
        div()
            .flex()
            .items_center()
            .gap(px(10.))
            .child(div().flex_none().child(head))
            .when(!desc.is_empty(), |d| d.child(div().min_w_0().text_size(px(13.)).text_color(t.text_2).child(desc.to_string())))
    };
    let mut rows = Vec::new();
    for (q, x) in a.asks.iter().enumerate() {
        if !x.question.is_empty() {
            rows.push(div().pt(px(4.)).text_size(px(13.)).font_weight(FontWeight::BOLD).text_color(t.text).child(x.question.clone()));
        }
        rows.extend(x.options.iter().enumerate().filter(|(_, (l, _))| !l.is_empty()).map(|(o, (l, d))| option(l, d, Some((q, o)))));
    }
    if a.asks.is_empty() {
        rows.extend(a.actions.iter().map(|(l, _)| option(l, "", None)));
    }
    let send = (a.picker && !one).then(|| {
        let answers = pick_answers(&a.asks, &chosen);
        let b = kit::btn_primary(t, id("pick-send"), "Send");
        match answers {
            Some(answers) => {
                let r = r.clone();
                b.on_click(cx.listener(move |m, _, _, cx| pick(m, &r, answers.clone(), cx)))
            }
            None => b.opacity(0.5).cursor_default(),
        }
    });
    body.child(div().text_size(px(14.)).text_color(t.warn_text).child(a.text.clone()))
        .when(!rows.is_empty(), |d| d.child(div().flex().flex_col().gap(px(8.)).children(rows)))
        .when(send.is_some() || focus.is_some(), |d| d.child(div().flex().items_center().gap(px(8.)).children(send).children(focus)))
}

fn todo_body(m: &mut MainWindow, t: &Theme, a: &Ask, body: Div, window: &mut Window, cx: &mut Context<MainWindow>) -> Div {
    let key = a.key();
    let r = a.task.as_ref().map(|(r, _)| r.clone()).unwrap_or_default();
    let c = m.home.notes.entry(key.clone()).or_insert_with(|| Composer::new(cx));
    let (kind, until) = (c.kind, c.until.clone());
    let (k, tr) = (key.clone(), r.clone());
    let field = c.input.render(t, ask_id(a, "note"), window).on_key_down(cx.listener(move |m, ev: &KeyDownEvent, _, cx| {
        let Some(c) = m.home.notes.get(&k) else { return };
        match c.input.on_key(ev, cx) {
            kit::KeyOutcome::Submit => send_note(m, &k, &tr, cx),
            kit::KeyOutcome::Cancel => close_note(m, &k, cx),
            kit::KeyOutcome::Ignored => {}
        }
    }));
    let mut out = body;
    if !a.text.is_empty() && a.text != a.title {
        out = out.child(div().text_size(px(13.)).text_color(t.text_2).child(ref_text(t, ask_id(a, "text"), &a.text, cx)));
    }
    if kind != Note::Done {
        out = out.child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(kind.label()));
    }
    out = out.child(field);
    if kind == Note::WaitingOn {
        let chips: Vec<_> = groups::wait_dates(chrono::Local::now().date_naive())
            .into_iter()
            .map(|(label, day)| {
                let on = until.as_deref() == Some(day.as_str());
                let k = key.clone();
                let b = kit::btn_small(t, ask_id(a, &format!("date-{label}")), label).on_click(cx.listener(move |m, _, _, cx| {
                    if let Some(c) = m.home.notes.get_mut(&k) {
                        c.until = Some(day.clone());
                    }
                    cx.notify();
                }));
                if on { b.border_color(t.accent).font_weight(FontWeight::SEMIBOLD) } else { b }
            })
            .collect();
        out = out.child(div().flex().flex_wrap().items_center().gap(px(6.)).children(chips));
    }
    if kind != Note::Done {
        let (k, k2) = (key.clone(), key.clone());
        let send = kit::btn_primary(t, ask_id(a, "note-send"), "Send").h(px(24.)).px(px(8.)).rounded(px(6.)).text_size(px(12.));
        let send = if kind == Note::WaitingOn && until.is_none() { kit::disabled(send) } else { send.on_click(cx.listener(move |m, _, _, cx| send_note(m, &k, &r, cx))) };
        let cancel = kit::btn_small(t, ask_id(a, "note-cancel"), "Cancel").on_click(cx.listener(move |m, _, _, cx| close_note(m, &k2, cx)));
        out = out.child(div().flex().items_center().gap(px(6.)).child(send).child(cancel));
    }
    out
}

/// A body's text with its refs as links that open them.
fn ref_text(t: &Theme, id: SharedString, text: &str, cx: &mut Context<MainWindow>) -> AnyElement {
    let spans = ref_spans(text);
    if spans.is_empty() {
        return text.to_string().into_any_element();
    }
    let under = UnderlineStyle { thickness: px(1.), color: Some(t.accent), wavy: false };
    let style = HighlightStyle { color: Some(t.accent), underline: Some(under), ..Default::default() };
    let styled = StyledText::new(text.to_string()).with_highlights(spans.iter().map(|(r, _)| (r.clone(), style)));
    let ranges: Vec<_> = spans.iter().map(|(r, _)| r.clone()).collect();
    let acts: Vec<Act> = spans.into_iter().map(|(_, a)| a).collect();
    let this = cx.entity().downgrade();
    InteractiveText::new(id, styled)
        .on_click(ranges, move |ix, _, cx| {
            if let Some(act) = acts.get(ix) {
                let _ = this.update(cx, |m, cx| run_act(m, act, cx));
            }
        })
        .into_any_element()
}

/// A 16px chevron: right shut, down open.
fn chevron(open: bool, color: Hsla) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let k = b.size.width.as_f32() / 24.;
            let u = |x: f32, y: f32| point(b.origin.x + px(x * k), b.origin.y + px(y * k));
            let pts = if open { [(6., 9.), (12., 15.), (18., 9.)] } else { [(9., 6.), (15., 12.), (9., 18.)] };
            let mut p = PathBuilder::stroke(px(1.8));
            p.move_to(u(pts[0].0, pts[0].1));
            for (x, y) in &pts[1..] {
                p.line_to(u(*x, *y));
            }
            if let Ok(p) = p.build() {
                window.paint_path(p, color);
            }
        },
    )
    .size(px(16.))
    .flex_none()
}

// ------------------------------------------------------------------ the Later line

fn later_line(t: &Theme, v: &View, open: bool, cx: &mut Context<MainWindow>) -> Div {
    let Some(l) = &v.later else { return div() };
    div().pt(px(12.)).pb(px(4.)).child(
        div()
            .id("home-later-line")
            .flex()
            .items_center()
            .gap(px(4.))
            .text_size(px(12.5))
            .text_color(t.muted)
            .cursor_pointer()
            .on_click(cx.listener(|m, _, _, cx| {
                m.home.later_open = !m.home.later_open;
                cx.notify();
            }))
            .child(chevron(open, t.muted))
            .child(l.text.clone()),
    )
}

fn later_item(t: &Theme, v: &View, i: usize, cx: &mut Context<MainWindow>) -> Div {
    let Some(x) = v.later.as_ref().and_then(|l| l.items.get(i)) else { return div() };
    let open = x.open.clone();
    let row = div()
        .id(SharedString::from(format!("home-later-{}", x.key)))
        .flex()
        .items_center()
        .gap(px(10.))
        .pl(px(20.))
        .py(px(4.))
        .text_size(px(12.5))
        .text_color(t.muted)
        .child(div().flex_1().min_w_0().truncate().child(x.title.clone()))
        .child(div().flex_none().child(x.when.clone()))
        .when_some(open, |d, act| {
            d.cursor_pointer().hover(|s| s.underline()).on_click(cx.listener(move |m, _, _, cx| run_act(m, &act, cx)))
        });
    div().child(row)
}

// ------------------------------------------------------------------ menus

/// What a row's menus act on.
struct Target {
    later: Vec<String>,
    goal: Option<String>,
    more: Vec<Entry>,
}

fn target(v: &View, key: &str, jira: bool) -> Option<Target> {
    for row in v.groups.iter().flat_map(|g| &g.rows) {
        if row.key() == key {
            return Some(Target { later: row.later_ids(), goal: row.goal_ref().map(str::to_string), more: row.more(jira) });
        }
        if let Row::Goal(x) = row {
            if let Some(a) = x.kids.iter().find(|a| a.key() == key) {
                return Some(Target { later: vec![a.id.clone()], goal: a.goal.as_ref().map(|(g, _)| g.clone()), more: groups::more_vm(a, jira) });
            }
        }
    }
    None
}

fn menu_head(t: &Theme, text: &str) -> Div {
    div().px(px(10.)).pt(px(6.)).pb(px(4.)).text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(t.muted).child(text.to_uppercase())
}

fn menu_line(t: &Theme) -> Div {
    div().my(px(4.)).h(px(1.)).bg(t.divider)
}

fn later_menu(t: &Theme, key: &str, x: Target, others: bool, at: Point<Pixels>, cx: &mut Context<MainWindow>) -> Div {
    let mut menu = kit::menu_box(t, 170.).child(menu_head(t, "Later"));
    let item = |menu: Div, n: usize, label: &'static str, body: Value, ids: Vec<String>, cx: &mut Context<MainWindow>| {
        menu.child(kit::menu_item(t, ("home-later-choice", n), label, false).on_click(cx.listener(move |m, _, _, cx| later(m, ids.clone(), body.clone(), cx))))
    };
    for (n, (label, body)) in groups::later_choices().into_iter().enumerate() {
        menu = item(menu, n, label, body, x.later.clone(), cx);
    }
    menu = menu.child(menu_line(t));
    menu = item(menu, 10, "Next week", json!({"until": "next_week"}), x.later.clone(), cx);
    if others {
        let after = format!("{AFTER_MENU}{key}");
        menu = menu.child(kit::menu_item(t, "home-later-after", "After a goal ▸", false).on_click(cx.listener(move |m, _, _, cx| {
            m.menu = Some((after.clone(), at));
            cx.notify();
        })));
    }
    menu
}

fn after_menu(t: &Theme, x: Target, goals: Vec<(String, String)>, cx: &mut Context<MainWindow>) -> Div {
    let mut menu = kit::menu_box(t, 240.).child(menu_head(t, "After a goal"));
    for (n, (g, name)) in goals.into_iter().enumerate() {
        let ids = x.later.clone();
        menu = menu.child(kit::menu_item(t, ("home-after-choice", n), name, false).on_click(cx.listener(move |m, _, _, cx| later(m, ids.clone(), json!({"after_goal": g}), cx))));
    }
    menu
}

fn more_menu(t: &Theme, key: &str, x: Target, cx: &mut Context<MainWindow>) -> Div {
    let mut menu = kit::menu_box(t, 210.);
    for (n, entry) in x.more.into_iter().enumerate() {
        menu = match entry {
            Entry::Line => menu.child(menu_line(t)),
            Entry::Item(label, act) => {
                let key = key.to_string();
                menu.child(kit::menu_item(t, ("home-more-choice", n), label, false).on_click(cx.listener(move |m, _, window, cx| run_do(m, &key, &act, window, cx))))
            }
        };
    }
    menu
}

/// The open Later or More menu, by the row it's for.
fn row_menu(m: &MainWindow, t: &Theme, v: &View, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let (open, at) = m.menu.as_ref()?;
    let at = *at;
    let (prefix, key) = [LATER_MENU, MORE_MENU, AFTER_MENU].iter().find_map(|p| open.strip_prefix(p).map(|k| (*p, k)))?;
    let x = target(v, key, m.jira_on())?;
    let home = m.data.home.clone().unwrap_or_default();
    let goals = groups::after_goals(&home, x.goal.as_deref());
    let menu = match prefix {
        LATER_MENU => later_menu(t, key, x, !goals.is_empty(), at, cx),
        AFTER_MENU => after_menu(t, x, goals, cx),
        _ => more_menu(t, key, x, cx),
    };
    // After a goal replaces the Later menu, so both open under the Later button.
    let button = format!("{}{key}", if prefix == AFTER_MENU { LATER_MENU } else { prefix });
    Some(kit::menu_under(&button, kit::Align::Right, at, menu.id("home-row-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())))
}

// ------------------------------------------------------------------ signals

/// Off the list at once, ahead of the board's next answer.
fn take_off(m: &mut MainWindow, ids: &[String]) {
    if let Some(needs) = m.data.home.as_mut().and_then(|h| Rc::make_mut(h).get_mut("needs")).and_then(Value::as_array_mut) {
        needs.retain(|a| !ids.iter().any(|id| id == s(a, "id")));
    }
}

/// Later on a row: `POST /alerts/:id/snooze` for each thing in it, one toast for the lot.
fn later(m: &mut MainWindow, ids: Vec<String>, body: Value, cx: &mut Context<MainWindow>) {
    m.menu = None;
    take_off(m, &ids);
    for (n, id) in ids.into_iter().enumerate() {
        m.post(format!("alerts/{id}/snooze"), body.clone(), cx, move |m, v, cx| {
            if n == 0 {
                sent(m, groups::later_note(&v["alert"]), cx);
            }
        });
    }
    cx.notify();
}

fn open_note(m: &mut MainWindow, key: &str, kind: Note, window: &mut Window, cx: &mut Context<MainWindow>) {
    FLIPPED.with(|f| f.borrow_mut().insert(key.to_string()));
    let c = m.home.notes.entry(key.to_string()).or_insert_with(|| Composer::new(cx));
    c.kind = kind;
    c.until = None;
    let focus = c.input.focus.clone();
    window.focus(&focus, cx);
    m.home.remeasure = true;
    cx.notify();
}

/// Back from an answer to Done's plain note; a row with only that shuts.
fn close_note(m: &mut MainWindow, key: &str, cx: &mut Context<MainWindow>) {
    match m.home.notes.get_mut(key) {
        Some(c) if c.kind != Note::Done => {
            c.kind = Note::Done;
            c.until = None;
        }
        _ => {
            FLIPPED.with(|f| f.borrow_mut().remove(key));
        }
    }
    m.home.remeasure = true;
    cx.notify();
}

/// Sends the note on `key`'s row; it stays until the board has it, so a refusal loses nothing.
fn send_note(m: &mut MainWindow, key: &str, r: &str, cx: &mut Context<MainWindow>) {
    let Some(c) = m.home.notes.get(key) else { return };
    let kind = c.kind;
    let Some((path, body)) = groups::note_post(r, kind, &c.input.text(cx), c.until.as_deref()) else { return };
    let key = key.to_string();
    m.post(path, body, cx, move |m, _, cx| {
        m.home.notes.remove(&key);
        FLIPPED.with(|f| f.borrow_mut().remove(&key));
        take_off(m, std::slice::from_ref(&key));
        sent(m, kind.said().to_string(), cx);
    });
}

fn post_said(m: &mut MainWindow, path: String, said: &'static str, cx: &mut Context<MainWindow>) {
    m.post(path, json!({}), cx, move |m, _, cx| sent(m, said.to_string(), cx));
}

/// A backlog item settled from its row: the Backlog page refetches.
fn settle_issue(m: &mut MainWindow, path: String, body: Value, said: &'static str, cx: &mut Context<MainWindow>) {
    m.post(path, body, cx, move |m, _, cx| {
        crate::ui::backlog::stale(m);
        sent(m, said.to_string(), cx);
    });
}

fn talk(m: &mut MainWindow, path: String, cx: &mut Context<MainWindow>) {
    m.post(path, json!({}), cx, |m, _, cx| sent(m, "Opening a terminal in Midna to talk it over".to_string(), cx));
}

fn run_do(m: &mut MainWindow, key: &str, act: &Do, window: &mut Window, cx: &mut Context<MainWindow>) {
    use crate::ui::modals::{Ask as Confirm, Modal};
    m.menu = None;
    match act.clone() {
        Do::Act(a) => run_act(m, &a, cx),
        Do::Done(r) => {
            m.home.notes.entry(key.to_string()).or_insert_with(|| Composer::new(cx)).kind = Note::Done;
            send_note(m, key, &r, cx);
        }
        Do::Ask(kind) => open_note(m, key, kind, window, cx),
        Do::DontWait(r) => m.post(format!("tasks/{r}/owner-answer"), json!({"answer": "dont_wait"}), cx, |m, _, cx| sent(m, "The work goes on without you".to_string(), cx)),
        Do::TalkTask(r) => talk(m, format!("tasks/{r}/talk"), cx),
        Do::TalkAlert(id) => talk(m, format!("alerts/{id}/talk"), cx),
        Do::MakeTask(b) => settle_issue(m, format!("backlog/{b}/promote"), json!({"where": "board"}), "Made into a task", cx),
        Do::WontDo(b) => settle_issue(m, format!("backlog/{b}/drop"), json!({}), "Closed as won’t do", cx),
        Do::Ticket(b) => settle_issue(m, format!("backlog/{b}/ticket"), json!({}), "Asked Jira for a ticket", cx),
        Do::FinishWithout(b) => settle_issue(m, format!("backlog/{b}/without-goal"), json!({}), "The goal finishes without it", cx),
        Do::Dismiss(id) => dismiss(m, &id, cx),
        Do::PauseGoal(g) => crate::ui::goal::set_paused(m, &g, true, cx),
        Do::SetAside(g) => post_said(m, format!("goals/{g}/set-aside"), "Set aside", cx),
        Do::ConfirmDone(r) => m.modal = Some(Modal::finish(r.to_uppercase(), Confirm::Done)),
        Do::ConfirmCancel(r) => m.modal = Some(Modal::finish(r.to_uppercase(), Confirm::Cancel)),
    }
    cx.notify();
}
