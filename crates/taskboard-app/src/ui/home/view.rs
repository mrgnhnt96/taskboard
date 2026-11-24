//! Needs you on screen: a virtual list of the page's lines, so only what's in view is built.
use super::groups::{self, Do, Entry, GoalRow, Group, IdeaRow, Kind, LaterLine, Note, Row};
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
    /// Done's note, kept aside while another form has the box, so each sends only its own text.
    done_text: String,
}

impl Composer {
    fn new(cx: &mut App) -> Composer {
        Composer { input: kit::Input::new(cx, "", true), kind: Note::Done, until: None, done_text: String::new() }
    }

    /// Back to Done's note, with its own text in the box.
    fn to_done(&mut self, cx: &mut App) {
        if self.kind == Note::Done {
            return;
        }
        self.kind = Note::Done;
        self.until = None;
        let done = std::mem::take(&mut self.done_text);
        self.input.set_text(&done, cx);
    }
}

/// The note forms by the name a restart saves them under.
const NOTE_KINDS: [(Note, &str); 4] = [(Note::Done, "done"), (Note::CantDo, "cant_do"), (Note::NotNeeded, "not_needed"), (Note::WaitingOn, "waiting_on")];

/// What's typed in the page's note boxes, for a restart to update to keep: by row key, the form
/// open, its text, Done's note kept aside and the waiting date. Boxes with no text are left out.
pub fn drafts(m: &MainWindow, cx: &App) -> HashMap<String, Value> {
    m.home
        .notes
        .iter()
        .filter_map(|(key, c)| {
            let text = c.input.text(cx);
            if text.trim().is_empty() && c.done_text.trim().is_empty() {
                return None;
            }
            let kind = NOTE_KINDS.iter().find(|(k, _)| *k == c.kind).map_or("done", |(_, n)| n);
            Some((key.clone(), json!({"kind": kind, "text": text, "done": c.done_text, "until": c.until})))
        })
        .collect()
}

/// Back after a restart: each saved note box with its text, its row open.
pub fn restore_drafts(m: &mut MainWindow, kept: &serde_json::Map<String, Value>, cx: &mut Context<MainWindow>) {
    for (key, v) in kept {
        let kind = NOTE_KINDS.iter().find(|(_, n)| v["kind"] == *n).map_or(Note::Done, |(k, _)| *k);
        let mut c = Composer::new(cx);
        c.kind = kind;
        c.until = v["until"].as_str().map(str::to_string);
        c.done_text = v["done"].as_str().unwrap_or("").to_string();
        c.input.set_text(v["text"].as_str().unwrap_or(""), cx);
        m.home.notes.insert(key.clone(), c);
        FLIPPED.with(|f| f.borrow_mut().insert(key.clone()));
    }
    m.home.remeasure = true;
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
    /// The ideas waiting on the owner: their label, then each.
    Ideas,
    Idea(usize),
    /// Open.
    Later(bool),
    LaterItem(usize),
    /// What the board took off the list itself: open, then each held.
    Handled(bool),
    HandledItem(usize),
    End,
}

struct View {
    asks: Rc<Vec<Ask>>,
    project: String,
    stats: Rc<Vec<Stat>>,
    names: Rc<Vec<String>>,
    counts: Vec<(String, usize)>,
    groups: Vec<Group>,
    ideas: Vec<IdeaRow>,
    later: Option<LaterLine>,
    sub: String,
}

impl View {
    fn of(built: &Built, project: &str, home: &Value) -> View {
        let shown: Vec<&Ask> = built.asks.iter().filter(|a| project == "all" || a.project == project).collect();
        let groups = groups::groups_vm(&shown);
        let ideas = groups::ideas_vm(home, project);
        let later = groups::later_vm(home, project);
        let agents = groups.iter().find(|g| g.kind == Kind::Waiting).map_or(0, |g| g.count);
        let sub = groups::waiting_line(agents + later.as_ref().map_or(0, |l| l.waiting));
        let counts = built.names.iter().map(|p| (p.clone(), built.asks.iter().filter(|a| &a.project == p).count())).collect();
        View { asks: built.asks.clone(), project: project.to_string(), stats: built.stats.clone(), names: built.names.clone(), counts, groups, ideas, later, sub }
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
    pub handled: super::handled::Handled,
    /// `handled.held.version` the list was last measured for.
    handled_seen: u64,
    notes: HashMap<String, Composer>,
    /// The row ↑ and ↓ are on, by its key.
    cursor: Option<String>,
    /// The page's own focus, so its keys reach it once it's clicked.
    focus: Option<FocusHandle>,
}

fn has_body(a: &Ask) -> bool {
    a.human || a.answer || !a.help.is_empty() || !a.actions.is_empty() || !a.line.is_empty() || (!a.text.is_empty() && a.text != a.title)
}

fn goal_open(g: &GoalRow) -> bool {
    g.kids.iter().any(starts_open) != flipped(&g.key)
}

/// The page's lines; with the board down, no counts from before it went.
fn lines_of(v: &View, later_open: bool, handled: (usize, bool), down: bool) -> Vec<Line> {
    let mut out = vec![Line::Head];
    if !v.stats.is_empty() && !down {
        out.push(Line::Stats);
    }
    if v.groups.is_empty() && v.ideas.is_empty() {
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
    if !v.ideas.is_empty() {
        out.push(Line::Ideas);
        out.extend((0..v.ideas.len()).map(Line::Idea));
    }
    if let Some(l) = &v.later {
        out.push(Line::Later(later_open));
        if later_open {
            out.extend((0..l.items.len()).map(Line::LaterItem));
        }
    }
    if handled.0 > 0 {
        out.push(Line::Handled(handled.1));
        if handled.1 {
            out.extend((0..handled.0).map(Line::HandledItem));
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
    let down = m.down.is_some();
    let st = &mut m.home;
    let held = st.view.as_ref().filter(|v| Rc::ptr_eq(&v.asks, &built.asks) && v.project == project).cloned();
    let same_project = st.view.as_ref().is_some_and(|v| v.project == project);
    let fresh = held.is_none();
    let v = held.unwrap_or_else(|| Rc::new(View::of(&built, &project, &home)));
    let lines = lines_of(&v, st.later_open, (st.handled.held.rows.len(), st.handled.open), down);
    if st.handled.held.version != st.handled_seen {
        st.handled_seen = st.handled.held.version;
        st.remeasure = true;
    }
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
    let focus = m.home.focus.get_or_insert_with(|| cx.focus_handle()).clone();
    let page = div()
        .id("home-page")
        .track_focus(&focus)
        // A click that gives nothing inside focus gives it to the page, for ↑ and ↓.
        .on_mouse_down(MouseButton::Left, cx.listener(|m, _, window, cx| {
            if let Some(f) = m.home.focus.clone().filter(|f| !f.contains_focused(window, cx)) {
                window.focus(&f, cx);
            }
        }))
        .on_key_down(cx.listener(on_key))
        .flex()
        .flex_1()
        .min_w_0()
        .h_full()
        .bg(t.bg);
    let Some(list) = m.home.list.clone() else { return page.into_any_element() };
    super::handled::load_older(m, m.home.lines.len(), list.logical_scroll_top().item_ix, cx);
    page.child(gpui_kit::list(list, cx.processor(line_item)).flex_1().min_w_0().h_full()).children(filter).children(menu).into_any_element()
}

fn line_item(m: &mut MainWindow, ix: usize, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let (Some(v), Some(line)) = (m.home.view.clone(), m.home.lines.get(ix).copied()) else { return div().into_any_element() };
    let inner = match line {
        Line::Head => head_line(&t, &v, m.data.home.is_some() && !paused(m), cx),
        Line::Stats => stats_line(&t, &v),
        Line::Empty if paused(m) => kit::empty(&t, "Can’t load the board."),
        Line::Empty if m.data.home.is_none() => kit::empty(&t, "Loading…"),
        Line::Empty => kit::empty(&t, "Nothing needs you.").pt(px(32.)),
        Line::Group(gi) => group_label(&t, &v.groups[gi]),
        Line::Row(gi, ri, open) => row_line(m, &t, &v.groups[gi], ri, open, window, cx),
        Line::Kid(gi, ri, ki, open) => kid_line(m, &t, &v.groups[gi], ri, ki, open, window, cx),
        Line::Ideas => label_line(&t, "Ideas", v.ideas.len()),
        Line::Idea(i) => idea_line(&t, &v, i, cx),
        Line::Later(open) => later_line(&t, &v, open, cx),
        Line::LaterItem(i) => later_item(&t, &v, i, cx),
        Line::Handled(open) => super::handled::head(&t, m.home.handled.held.total, open, chevron(open, t.muted), cx),
        Line::HandledItem(i) => super::handled::item(m, &t, i, cx),
        Line::End => div().h(px(32.)),
    };
    div().px(px(PAGE_X)).line_height(relative(1.4)).child(inner).into_any_element()
}

// ------------------------------------------------------------------ keys

/// Each row's head on the page in order, a goal's open items among them: (line, key).
fn heads(m: &MainWindow) -> Vec<(usize, String)> {
    let Some(v) = &m.home.view else { return vec![] };
    let key = |line: &Line| match *line {
        Line::Row(gi, ri, _) => Some(v.groups[gi].rows[ri].key()),
        Line::Kid(gi, ri, ki, _) => match &v.groups[gi].rows[ri] {
            Row::Goal(x) => x.kids.get(ki).map(Ask::key),
            Row::One(_) => None,
        },
        _ => None,
    };
    m.home.lines.iter().enumerate().filter_map(|(ix, l)| key(l).map(|k| (ix, k))).collect()
}

/// ↑ or ↓: the row before or after the one marked; the first when none is.
fn step(m: &mut MainWindow, down: bool, cx: &mut Context<MainWindow>) {
    let rows = heads(m);
    if rows.is_empty() {
        return;
    }
    let at = m.home.cursor.as_ref().and_then(|c| rows.iter().position(|(_, k)| k == c));
    let next = match at {
        None => 0,
        Some(i) if down => (i + 1).min(rows.len() - 1),
        Some(i) => i.saturating_sub(1),
    };
    let (ix, key) = rows[next].clone();
    m.home.cursor = Some(key);
    if let Some(l) = &m.home.list {
        l.scroll_to_reveal_item(ix);
    }
    cx.notify();
}

/// The marked row's head, as drawn.
fn marked_head(m: &MainWindow, t: &Theme) -> Option<Head> {
    let (v, key) = (m.home.view.as_ref()?, m.home.cursor.as_ref()?);
    m.home.lines.iter().find_map(|line| {
        let h = match *line {
            Line::Row(gi, ri, open) => match &v.groups[gi].rows[ri] {
                Row::One(a) => ask_head(t, a, v.groups[gi].kind, false),
                Row::Goal(x) => goal_head(t, x, v.groups[gi].kind, open),
            },
            Line::Kid(gi, ri, ki, _) => match &v.groups[gi].rows[ri] {
                Row::Goal(x) => ask_head(t, x.kids.get(ki)?, v.groups[gi].kind, true),
                Row::One(_) => return None,
            },
            _ => return None,
        };
        (h.key == *key).then_some(h)
    })
}

/// ↩ on the marked row: a row that opens opens or shuts, as a click does; any other follows its link.
fn press(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    let t = cx.global::<Theme>().clone();
    let Some(h) = marked_head(m, &t) else { return };
    if h.opens {
        open_head(m, h.key, h.note, window, cx);
    } else if let Some(act) = h.link {
        run_act(m, &act, cx);
    }
}

/// The page's keys, once it has focus (a click, or Tab onto a row): ↑ ↓ move between the rows,
/// a goal's open items among them, and ↩ opens the marked one.
fn on_key(m: &mut MainWindow, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<MainWindow>) {
    let ks = &ev.keystroke;
    if m.modal.is_some() || m.menu.is_some() || crate::ui::focus::typing(window) || ks.modifiers.platform || ks.modifiers.control || ks.modifiers.alt {
        return;
    }
    let on_page = m.home.focus.as_ref().is_some_and(|f| f.is_focused(window));
    match ks.key.as_str() {
        "down" => step(m, true, cx),
        "up" => step(m, false, cx),
        "enter" if on_page && m.home.cursor.is_some() => press(m, window, cx),
        _ => return,
    }
    cx.stop_propagation();
}

// ------------------------------------------------------------------ the page's top

/// The title, and how many agents wait only while that's known: no all-clear without the board.
fn head_line(t: &Theme, v: &View, known: bool, cx: &mut Context<MainWindow>) -> Div {
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
        .when(known, |d| d.child(div().text_size(px(13.)).text_color(t.muted).child(v.sub.clone())))
        .child(div().flex_1())
        .when(v.names.len() > 1, |d| d.child(tabs))
        .when(known, |d| d.child(talk_button(t, &v.project, cx)))
}

/// Talk it over: a Claude terminal in Midna with what the page shows in its context.
fn talk_button(t: &Theme, project: &str, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let project = project.to_string();
    kit::btn_small(t, "home-talk", "Talk it over").on_click(cx.listener(move |m, _: &ClickEvent, _, cx| {
        let body = if project == "all" { json!({}) } else { json!({"project": project}) };
        m.post("home/talk", body, cx, |m, v, cx| sent(m, talk_note(&v), cx));
    }))
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
    label_line(t, g.kind.label(), g.count)
}

fn label_line(t: &Theme, label: &str, count: usize) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(8.))
        .pt(px(18.))
        .pb(px(8.))
        .text_size(px(12.))
        .text_color(t.muted)
        .child(div().font_weight(FontWeight::BOLD).child(label.to_uppercase()))
        .child(count.to_string())
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
    /// Open or shut, on a row that opens.
    chevron: Option<bool>,
    opens: bool,
    /// A to-do: opening it puts the cursor in its note box.
    note: bool,
    /// The board is down: what would change it waits.
    paused: bool,
    /// Another form than Done's note is open on the row, so Done waits.
    form_open: bool,
    /// ↑ and ↓ are on it.
    marked: bool,
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
        chevron: has_body(a).then(|| is_open(a)),
        opens: has_body(a),
        note: a.human,
        paused: false,
        form_open: false,
        marked: false,
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
        finish: g.finish.iter().map(|act| (act.label().to_string(), Do::Act(act.clone()))).chain(make_all(g)).collect(),
        later: g.kids.iter().any(|a| a.snooze),
        time: if g.quiet { String::new() } else { g.time.clone() },
        chevron: Some(open),
        opens: true,
        note: false,
        paused: false,
        form_open: false,
        marked: false,
    }
}

/// "Make all 3 into tasks" on a goal whose open items are all misses.
fn make_all(g: &GoalRow) -> Option<(String, Do)> {
    let refs: Vec<String> = g.kids.iter().filter_map(|a| a.issue.clone()).collect();
    (refs.len() > 1 && refs.len() == g.kids.len()).then(|| (format!("Make all {} into tasks", refs.len()), Do::MakeAll(refs)))
}

/// A head with what the page knows now: whether the board answers and which form is open.
fn live_head(m: &MainWindow, mut h: Head, a: Option<&Ask>) -> Head {
    h.paused = paused(m);
    h.marked = m.home.cursor.as_deref() == Some(h.key.as_str());
    h.form_open = a.is_some_and(|a| is_open(a) && m.home.notes.get(&a.key()).is_some_and(|c| c.kind != Note::Done));
    h
}

/// Whether a row's button changes something on the board, rather than opening a page or a form.
fn posts(act: &Do) -> bool {
    match act {
        Do::Act(a) => a.posts(),
        Do::Ask(_) => false,
        _ => true,
    }
}

fn do_btn(t: &Theme, key: &str, n: usize, label: String, act: Do, off: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let id = SharedString::from(format!("home-do-{n}-{key}"));
    let b = match act {
        Do::Done(_) => kit::btn_primary(t, id, label).h(px(24.)).px(px(8.)).rounded(px(6.)).text_size(px(12.)),
        _ => kit::btn_small(t, id, label),
    };
    if off {
        return kit::disabled(b);
    }
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
            crate::ui::focus::stop(t, d).cursor_pointer().hover(|s| s.underline()).on_click(cx.listener(move |m, _, _, cx| {
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
    let off = |act: &Do| (h.paused && posts(act)) || (h.form_open && matches!(act, Do::Done(_)));
    let buttons: Vec<_> = h.finish.iter().enumerate().map(|(n, (label, act))| soft(do_btn(t, &key, n, label.clone(), act.clone(), off(act), cx))).collect();
    let later = h.later.then(|| soft(if h.paused { kit::disabled(kit::btn_small(t, SharedString::from(format!("{LATER_MENU}{key}")), "Later ▾")) } else { menu_btn(t, &key, LATER_MENU, "Later ▾", cx) }));
    let more = soft(menu_btn(t, &key, MORE_MENU, "More ▾", cx));
    let (flip_key, note) = (key.clone(), h.note);
    // A row that opens shows which way it is; the rest keep the chevron's space, so titles line up.
    let mark = match h.chevron {
        Some(open) => chevron(open, t.muted).into_any_element(),
        None => div().flex_none().w(px(16.)).into_any_element(),
    };
    let head = div().id(SharedString::from(format!("home-head-{key}")));
    let head = if h.opens { crate::ui::focus::stop(t, head) } else { head };
    head.flex()
        .items_center()
        .gap(px(10.))
        .px(px(14.))
        .py(px(9.))
        .when(h.marked, |d| d.bg(t.accent_soft))
        .when(h.opens, |d| {
            d.cursor_pointer().on_click(cx.listener(move |m, _, window, cx| {
                m.home.cursor = Some(flip_key.clone());
                open_head(m, flip_key.clone(), note, window, cx);
            }))
        })
        .child(mark)
        .child(words)
        .when(!h.tag.is_empty(), |d| {
            let weight = if h.tag_color == t.muted { FontWeight::NORMAL } else { FontWeight::SEMIBOLD };
            d.child(div().flex_none().text_size(px(12.)).font_weight(weight).text_color(h.tag_color).child(h.tag.clone()))
        })
        .when(!h.holds.is_empty(), |d| d.child(div().flex_none().text_size(px(12.)).text_color(t.muted).child(h.holds.clone())))
        .child(div().flex_none().flex().items_center().gap(px(6.)).children(buttons).children(later).child(more))
        .when(!h.time.is_empty(), |d| d.child(div().flex_none().w(px(36.)).flex().justify_end().text_size(px(12.)).text_color(t.muted).child(h.time.clone())))
}

/// A row that opens opens or shuts; a to-do's note box gets the cursor as it opens.
fn open_head(m: &mut MainWindow, key: String, note: bool, window: &mut Window, cx: &mut Context<MainWindow>) {
    flip(key.clone());
    if note && flipped(&key) {
        let c = m.home.notes.entry(key).or_insert_with(|| Composer::new(cx));
        let focus = c.input.focus.clone();
        window.focus(&focus, cx);
    }
    m.home.remeasure = true;
    cx.notify();
}

/// A row's card; an agent waiting's is tinted, the loudest on the page.
fn frame(t: &Theme, warn: bool, quiet: bool) -> Div {
    let fill = if warn { Some(t.warn_soft) } else { (!quiet).then_some(t.card) };
    div().flex().flex_col().border_color(if warn { t.warn_line } else { t.border }).when_some(fill, |d, c| d.bg(c)).overflow_hidden()
}

fn row_line(m: &mut MainWindow, t: &Theme, g: &Group, ri: usize, open: bool, window: &mut Window, cx: &mut Context<MainWindow>) -> Div {
    let warn = g.kind == Kind::Waiting;
    match &g.rows[ri] {
        Row::One(a) => {
            let head = head_el(t, live_head(m, ask_head(t, a, g.kind, false), Some(a)), cx);
            let card = frame(t, warn, a.sev == Sev::Quiet).rounded(px(10.)).border_1().child(head);
            let card = if open { card.child(body(m, t, a, 14., window, cx)) } else { card };
            div().pb(px(6.)).child(card)
        }
        Row::Goal(x) => {
            let head = head_el(t, live_head(m, goal_head(t, x, g.kind, open), None), cx);
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
    let head = head_el(t, live_head(m, ask_head(t, a, g.kind, true), Some(a)), cx).pl(px(30.));
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
        return question_body(t, a, frame, paused(m), cx);
    }
    alert_body(t, a, frame, paused(m), cx)
}

fn ask_id(a: &Ask, part: &str) -> SharedString {
    SharedString::from(format!("home-{part}-{}", a.key()))
}

fn alert_body(t: &Theme, a: &Ask, body: Div, paused: bool, cx: &mut Context<MainWindow>) -> Div {
    let fixes: Vec<_> = a
        .actions
        .iter()
        .enumerate()
        .map(|(n, (label, command))| {
            let (aid, label, command) = (a.id.clone(), label.clone(), command.clone());
            let b = kit::btn(t, ask_id(a, &format!("fix-{n}")), label.clone());
            if paused {
                return kit::disabled(b);
            }
            b.on_click(cx.listener(move |m, _, _, cx| crate::app::run_alert_action(m, &aid, n, &label, &command, cx)))
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

fn question_body(t: &Theme, a: &Ask, body: Div, paused: bool, cx: &mut Context<MainWindow>) -> Div {
    let id = |part: &str| ask_id(a, part);
    let r = a.task.as_ref().map(|(r, _)| r.clone()).unwrap_or_default();
    let focus = body_focus(a).map(|act| {
        let b = kit::btn(t, id("focus"), act.label());
        if paused { kit::disabled(b) } else { b.on_click(cx.listener(move |m, _, _, cx| run_act(m, &act, cx))) }
    });
    // An option answers in one click, and so does a picker's when it asks one single-select
    // question; otherwise its options toggle and Send presses them. The alert's suggested
    // answers answer like options. While an answer is on its way the pressed choice reads
    // Sending… and every choice waits, so a second press sends nothing.
    let chosen = picked(&r);
    let one = one_click(&a.asks);
    let busy = sending(&r);
    let wait = paused || busy.is_some();
    let choice = |part: String, label: String| {
        let label = if busy.as_deref() == Some(part.as_str()) { "Sending…".to_string() } else { label };
        let b = kit::btn(t, id(&part), label);
        (b, part)
    };
    let option = |label: &str, desc: &str, qo: Option<(usize, usize)>| {
        let n = qo.map_or_else(|| label.to_string(), |(q, o)| format!("{q}-{o}"));
        let (r, aid, text) = (r.clone(), a.id.clone(), label.to_string());
        let head = match qo {
            Some((_, o)) if a.picker && one => {
                let (b, part) = choice(format!("pick-{n}"), text);
                if wait { kit::disabled(b) } else { b.on_click(cx.listener(move |m, _, _, cx| pick(m, &aid, &r, &part, vec![vec![o]], cx))) }
            }
            Some((q, o)) if a.picker => {
                let asks = a.asks.clone();
                let (b, _) = choice(format!("pick-{n}"), text);
                let b = if chosen.contains(&(q, o)) { b.border_color(t.warn).bg(t.warn_soft).font_weight(FontWeight::SEMIBOLD) } else { b };
                if wait {
                    kit::disabled(b)
                } else {
                    b.on_click(cx.listener(move |_, _, _, cx| {
                        let next = toggle_pick(&asks, &picked(&r), q, o);
                        PICKED.with(|p| p.borrow_mut().insert(r.clone(), next));
                        cx.notify();
                    }))
                }
            }
            _ => {
                let (b, part) = choice(format!("opt-{n}"), text.clone());
                if wait { kit::disabled(b) } else { b.on_click(cx.listener(move |m, _, _, cx| answer(m, &aid, &r, &part, &text, cx))) }
            }
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
        let label = if busy.as_deref() == Some("pick-send") { "Sending…" } else { "Send" };
        let b = kit::btn_primary(t, id("pick-send"), label);
        match answers {
            Some(answers) if !wait => {
                let (r, aid) = (r.clone(), a.id.clone());
                b.on_click(cx.listener(move |m, _, _, cx| pick(m, &aid, &r, "pick-send", answers.clone(), cx)))
            }
            _ => kit::disabled(b),
        }
    });
    // A task that missed the time it parked itself until: its plan under the missed time.
    let under = (a.asks.is_empty() && !a.line.is_empty() && a.line != a.text).then(|| div().text_size(px(13.)).text_color(t.text_2).child(a.line.clone()));
    body.child(div().text_size(px(14.)).text_color(t.warn_text).child(a.text.clone()))
        .children(under)
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
    out = out.child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(kind.name()));
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
        let send = if paused(m) || (kind == Note::WaitingOn && until.is_none()) { kit::disabled(send) } else { send.on_click(cx.listener(move |m, _, _, cx| send_note(m, &k, &r, cx))) };
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
        crate::ui::focus::stop(t, div().id("home-later-line"))
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
            crate::ui::focus::stop(t, d).cursor_pointer().hover(|s| s.underline()).on_click(cx.listener(move |m, _, _, cx| run_act(m, &act, cx)))
        });
    div().child(row)
}

/// An idea waiting on the owner: its verdict, ref and title, and what it waits for. It opens the idea.
fn idea_line(t: &Theme, v: &View, i: usize, cx: &mut Context<MainWindow>) -> Div {
    let Some(x) = v.ideas.get(i) else { return div() };
    let r = x.r.clone();
    let head = crate::ui::focus::stop(t, div().id(SharedString::from(format!("home-idea-{}", x.r))))
        .flex()
        .items_center()
        .gap(px(10.))
        .px(px(14.))
        .py(px(9.))
        .cursor_pointer()
        .on_click(cx.listener(move |m, _, _, cx| crate::ui::idea::open(m, &r, cx)))
        .children(x.pill.map(|(label, tone)| kit::tone_pill(t, tone, label).flex_none()))
        .child(div().flex_none().text_size(px(12.)).font_family(t.mono_font.clone()).text_color(t.muted).child(x.r.clone()))
        .child(div().flex_1().min_w_0().truncate().text_size(px(14.)).text_color(t.text).child(x.title.clone()))
        .child(div().flex_none().text_size(px(12.)).text_color(t.muted).child(x.note.clone()));
    div().pb(px(6.)).child(frame(t, false, false).rounded(px(10.)).border_1().child(head))
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

fn more_menu(t: &Theme, key: &str, x: Target, paused: bool, cx: &mut Context<MainWindow>) -> Div {
    let mut menu = kit::menu_box(t, 210.);
    for (n, entry) in x.more.into_iter().enumerate() {
        menu = match entry {
            Entry::Line => menu.child(menu_line(t)),
            Entry::Item(label, act) if paused && posts(&act) => menu.child(kit::disabled(kit::menu_item(t, ("home-more-choice", n), label, false))),
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
        _ => more_menu(t, key, x, paused(m), cx),
    };
    // After a goal replaces the Later menu, so both open under the Later button.
    let button = format!("{}{key}", if prefix == AFTER_MENU { LATER_MENU } else { prefix });
    Some(kit::menu_under(&button, kit::Align::Right, at, menu.id("home-row-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())))
}

// ------------------------------------------------------------------ signals

/// Later on a row: `POST /alerts/:id/snooze` for each thing in it, one toast for the lot.
fn later(m: &mut MainWindow, ids: Vec<String>, body: Value, cx: &mut Context<MainWindow>) {
    m.menu = None;
    if paused(m) {
        return cx.notify();
    }
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
    if c.kind != kind {
        // Each form has its own text: Done's note waits aside, another form starts empty.
        if c.kind == Note::Done {
            c.done_text = c.input.text(cx);
        }
        c.input.clear(cx);
    }
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
        Some(c) if c.kind != Note::Done => c.to_done(cx),
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
    if paused(m) {
        return;
    }
    let kind = c.kind;
    let Some((path, body)) = groups::note_post(r, kind, &c.input.text(cx), c.until.as_deref()) else { return };
    let (key, said) = (key.to_string(), kind.sent(r));
    m.post(path, body, cx, move |m, _, cx| {
        m.home.notes.remove(&key);
        FLIPPED.with(|f| f.borrow_mut().remove(&key));
        take_off(m, std::slice::from_ref(&key));
        sent_undo(m, said, cx);
    });
}

/// A toast that may carry an Undo (Done, Don't wait for me).
fn sent_undo(m: &mut MainWindow, (text, undo): (String, Option<String>), cx: &mut Context<MainWindow>) {
    match undo {
        Some(u) => m.toast_undo(text, u, cx),
        None => sent(m, text, cx),
    }
}

fn post_said(m: &mut MainWindow, path: String, said: &'static str, cx: &mut Context<MainWindow>) {
    m.post(path, json!({}), cx, move |m, _, cx| sent(m, said.to_string(), cx));
}

/// A backlog item settled from its row: the Backlog page refetches.
fn settle_issue(m: &mut MainWindow, path: String, body: Value, said: &'static str, cx: &mut Context<MainWindow>) {
    m.post(path, body, cx, move |m, v, cx| {
        crate::ui::backlog::stale(m);
        // Won't do and Finish without it can be taken back from the toast.
        match v["undo"].as_str() {
            Some(undo) => m.toast_undo(said, undo, cx),
            None => sent(m, said.to_string(), cx),
        }
    });
}

/// A miss made a task from its row: the toast says whether it joined the goal's plan or was queued, with Open it.
fn make_task(m: &mut MainWindow, b: String, cx: &mut Context<MainWindow>) {
    m.post(format!("backlog/{b}/promote"), json!({"where": "board"}), cx, move |m, v, cx| {
        crate::ui::backlog::stale(m);
        match crate::ui::issue_panel::made_toast(&b, &v) {
            Some((text, task)) => crate::ui::backlog::toast_open(m, text, task, cx),
            None => sent(m, "Made into a task".into(), cx),
        }
    });
}

fn talk(m: &mut MainWindow, path: String, cx: &mut Context<MainWindow>) {
    m.post(path, json!({}), cx, |m, v, cx| sent(m, talk_note(&v), cx));
}

/// What a Talk it over click did: opened a terminal, or brought back the one still open from last time.
fn talk_note(v: &Value) -> String {
    if v["reused"] == true { "Showing the terminal you were talking it over in" } else { "Opening a terminal in Midna to talk it over" }.to_string()
}

fn run_do(m: &mut MainWindow, key: &str, act: &Do, window: &mut Window, cx: &mut Context<MainWindow>) {
    use crate::ui::modals::{Ask as Confirm, Modal};
    m.menu = None;
    if paused(m) && posts(act) {
        return cx.notify();
    }
    match act.clone() {
        Do::Act(a) => run_act(m, &a, cx),
        // Done sends Done's own note, and waits while another form is open in the box.
        Do::Done(r) => {
            let c = m.home.notes.entry(key.to_string()).or_insert_with(|| Composer::new(cx));
            if c.kind != Note::Done && flipped(key) {
                return cx.notify();
            }
            c.to_done(cx);
            send_note(m, key, &r, cx);
        }
        Do::Ask(kind) => open_note(m, key, kind, window, cx),
        Do::DontWait(r) => m.post(format!("tasks/{r}/owner-answer"), json!({"answer": "dont_wait"}), cx, move |m, _, cx| sent_undo(m, groups::dont_wait_sent(&r), cx)),
        Do::TalkTask(r) => talk(m, format!("tasks/{r}/talk"), cx),
        Do::TalkAlert(id) => talk(m, format!("alerts/{id}/talk"), cx),
        Do::MakeTask(b) => make_task(m, b, cx),
        Do::MakeAll(ids) => {
            let said = format!("Made {} into tasks", ids.len());
            m.post("backlog/bulk",json!({"ids": ids, "action": "task", "where": "board"}), cx, move |m, _, cx| {
                crate::ui::backlog::stale(m);
                sent(m, said, cx);
            })
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parity::{settle, window};
    // Beats the glob's `gpui_kit::test`.
    use ::core::prelude::v1::test;

    #[gpui_kit::test]
    fn done_waits_while_another_form_is_open_and_sends_only_its_own_note(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        let key = "task:T41".to_string();
        w.update(cx, |m, window, cx| {
            let c = m.home.notes.entry(key.clone()).or_insert_with(|| Composer::new(cx));
            c.input.set_text("Booked the room", cx);
            open_note(m, &key, Note::CantDo, window, cx);
            let c = &m.home.notes[&key];
            assert_eq!(c.input.text(cx), "", "another form starts empty");
            c.input.set_text("No access", cx);
            run_do(m, &key, &Do::Done("T41".into()), window, cx);
        })
        .unwrap();
        settle(cx);
        assert_eq!(rec.last("tasks/T41/did"), None, "Done waits while Can't do this is open");

        w.update(cx, |m, window, cx| {
            close_note(m, &key, cx);
            assert_eq!(m.home.notes[&key].input.text(cx), "Booked the room", "Done's note comes back");
            run_do(m, &key, &Do::Done("T41".into()), window, cx);
        })
        .unwrap();
        settle(cx);
        assert_eq!(rec.last("tasks/T41/did"), Some(json!({"note": "Booked the room"})));
    }

    #[gpui_kit::test]
    fn a_shut_row_with_a_form_left_open_sends_dones_note(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        let key = "task:T42".to_string();
        w.update(cx, |m, window, cx| {
            let c = m.home.notes.entry(key.clone()).or_insert_with(|| Composer::new(cx));
            c.input.set_text("Sent the invoice", cx);
            open_note(m, &key, Note::NotNeeded, window, cx);
            m.home.notes[&key].input.set_text("Dropped", cx);
            flip(key.clone());
            run_do(m, &key, &Do::Done("T42".into()), window, cx);
        })
        .unwrap();
        settle(cx);
        assert_eq!(rec.last("tasks/T42/did"), Some(json!({"note": "Sent the invoice"})));
    }

    #[gpui_kit::test]
    fn typed_notes_are_kept_for_a_restart_and_come_back_open(cx: &mut gpui_kit::TestAppContext) {
        let (w, _) = window(cx);
        let (key, empty) = ("task:T44".to_string(), "task:T45".to_string());
        let saved = w
            .update(cx, |m, window, cx| {
                m.home.notes.entry(key.clone()).or_insert_with(|| Composer::new(cx)).input.set_text("Sent the invoice", cx);
                open_note(m, &key, Note::WaitingOn, window, cx);
                let c = m.home.notes.get_mut(&key).unwrap();
                c.input.set_text("Dana", cx);
                c.until = Some("2026-12-01".into());
                m.home.notes.entry(empty.clone()).or_insert_with(|| Composer::new(cx));
                drafts(m, cx)
            })
            .unwrap();
        assert_eq!(saved.keys().collect::<Vec<_>>(), [&key], "an empty box isn't kept");
        let saved = serde_json::Map::from_iter(saved);
        w.update(cx, |m, _, cx| {
            m.home.notes.clear();
            FLIPPED.with(|f| f.borrow_mut().clear());
            restore_drafts(m, &saved, cx);
            let c = &m.home.notes[&key];
            assert_eq!((c.kind, c.input.text(cx), c.done_text.as_str(), c.until.as_deref()), (Note::WaitingOn, "Dana".to_string(), "Sent the invoice", Some("2026-12-01")));
            assert!(flipped(&key), "its row opens");
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn with_the_board_down_row_buttons_wait(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        rec.clear();
        w.update(cx, |m, window, cx| {
            m.down = Some("The board isn't answering.".into());
            run_do(m, "task:T43", &Do::DontWait("T43".into()), window, cx);
            run_do(m, "task:T43", &Do::Act(Act::Restart("T43".into())), window, cx);
            later(m, vec!["a1".into()], json!({"until": "next_week"}), cx);
        })
        .unwrap();
        settle(cx);
        assert_eq!(rec.posts(), vec![], "nothing is sent while the board is down");
    }

    #[gpui_kit::test]
    fn make_it_a_task_on_a_goal_that_isnt_running_says_it_joined_the_plan(cx: &mut gpui_kit::TestAppContext) {
        use crate::backend::Backend;
        let (w, rec) = window(cx);
        let g = rec.board().post("goals", json!({"name": "Ship it", "project": "webapp"})).unwrap()["id"].as_i64().unwrap();
        let v = rec.board().post("backlog", json!({"title": "Offline sign-in fails", "kind": "bug", "project": "webapp", "goal_id": g, "misses_goal": true})).unwrap();
        let b = crate::fmt::ref_of(v.get("issue").unwrap_or(&v), "B");
        w.update(cx, |m, window, cx| run_do(m, &format!("issue:{b}"), &Do::MakeTask(b.clone()), window, cx)).unwrap();
        settle(cx);
        assert_eq!(rec.last(&format!("backlog/{b}/promote")), Some(json!({"where": "board"})));
        w.update(cx, |m, _, _| {
            let toast = m.toasts.last().map(|t| t.text.clone()).unwrap_or_default();
            assert!(toast.starts_with(&format!("Made {b} into T")), "{toast}");
            assert!(toast.ends_with(&format!(" as planned work in G{g}. It starts when you start G{g}")), "{toast}");
            assert!(m.backlog.open_toast.as_ref().is_some_and(|(_, t)| t.starts_with('T')), "the toast offers Open it");
        })
        .unwrap();
    }

    #[test]
    fn only_what_changes_the_board_waits_for_it() {
        assert!(posts(&Do::Done("T1".into())) && posts(&Do::Act(Act::Restart("T1".into()))) && posts(&Do::Dismiss("a1".into())));
        assert!(!posts(&Do::Act(Act::OpenGoal("G1".into()))) && !posts(&Do::Ask(Note::CantDo)));
    }

    #[test]
    fn with_the_board_down_the_page_drops_its_counts() {
        let v = View { asks: Rc::new(vec![]), project: "all".into(), stats: Rc::new(stats_vm(&json!({}))), names: Rc::new(vec![]), counts: vec![], groups: vec![], ideas: vec![], later: None, sub: String::new() };
        assert!(lines_of(&v, false, (0, false), false).contains(&Line::Stats));
        assert!(!lines_of(&v, false, (0, false), true).contains(&Line::Stats));
    }

    #[test]
    fn ideas_waiting_on_the_owner_come_after_the_groups_and_before_later() {
        let home = json!({
            "ideas": [{"ref": "I3", "title": "Search across projects", "project": "web", "verdict": "fits", "questions": 1, "at": null}],
            "later": [{"id": "stage:G2:1", "stage": true, "goal": "G2", "goal_name": "Faster sign-in", "wave": 1, "later": {"back": true}}]});
        let v = View { asks: Rc::new(vec![]), project: "all".into(), stats: Rc::new(vec![]), names: Rc::new(vec![]), counts: vec![], groups: vec![],
                       ideas: groups::ideas_vm(&home, "all"), later: groups::later_vm(&home, "all"), sub: String::new() };
        assert_eq!(lines_of(&v, false, (0, false), false), [Line::Head, Line::Ideas, Line::Idea(0), Line::Later(false), Line::End], "an idea waiting isn't Nothing needs you");
        let none = View { ideas: vec![], ..v };
        assert_eq!(lines_of(&none, false, (0, false), false), [Line::Head, Line::Empty, Line::Later(false), Line::End]);
        assert_eq!(lines_of(&none, false, (2, false), false), [Line::Head, Line::Empty, Line::Later(false), Line::Handled(false), Line::End], "shut: its line alone");
        assert_eq!(
            lines_of(&none, false, (2, true), false),
            [Line::Head, Line::Empty, Line::Later(false), Line::Handled(true), Line::HandledItem(0), Line::HandledItem(1), Line::End]
        );
    }

    #[test]
    fn a_goal_of_misses_offers_one_button_for_all_of_them() {
        let miss = |n: i64| json!({"id": format!("a{n}"), "key": format!("miss:{n}"), "text": format!("Miss {n}"), "title": format!("Miss {n}"), "goal": "G1", "goal_name": "Picto", "project": "p",
            "at": format!("2026-11-0{n}T10:00:00Z"), "card": null, "waiting": false, "holds": {"tasks": [], "goal": "G1"}});
        let asks = crate::ui::home::asks_vm(&json!({"needs": [miss(4), miss(5), miss(6)], "goals": []}));
        let gs = groups::groups_vm(&asks.iter().collect::<Vec<_>>());
        let Row::Goal(g) = &gs[0].rows[0] else { panic!("three misses fold into one goal row") };
        assert_eq!(make_all(g), Some(("Make all 3 into tasks".to_string(), Do::MakeAll(vec!["B6".into(), "B5".into(), "B4".into()]))));
        let one = GoalRow { kids: vec![g.kids[0].clone()], ..g.clone() };
        assert_eq!(make_all(&one), None, "one miss has its own Make it a task");
    }

    #[gpui_kit::test]
    fn arrows_move_through_a_goals_misses_and_enter_opens_them(cx: &mut gpui_kit::TestAppContext) {
        use gpui_kit::AppContext as _;
        let (w, _rec) = window(cx);
        let draw = |cx: &mut gpui_kit::TestAppContext| {
            cx.update_window(w.into(), |_, window, cx| {
                window.refresh();
                window.draw(cx).clear(cx);
            })
            .unwrap();
        };
        let miss = |n: i64| json!({"id": format!("a{n}"), "key": format!("miss:{n}"), "text": format!("Miss {n}"), "title": format!("Miss {n}"), "goal": "G1", "goal_name": "Picto", "project": "p",
            "at": format!("2026-11-0{n}T10:00:00Z"), "card": null, "waiting": false, "holds": {"tasks": [], "goal": "G1"}});
        w.update(cx, |m, _, cx| {
            m.go(Page::Board, cx);
            m.filters.project = "all".into();
            m.data.home = Some(Rc::new(json!({"needs": [miss(4), miss(5)], "goals": []})));
        })
        .unwrap();
        draw(cx);
        w.update(cx, |m, window, cx| window.focus(m.home.focus.as_ref().unwrap(), cx)).unwrap();
        draw(cx);
        cx.simulate_keystrokes(w.into(), "down");
        let goal = w.update(cx, |m, _, _| {
            let rows = heads(m);
            assert_eq!(rows.len(), 1, "the misses fold into their goal's row, shut");
            assert_eq!(m.home.cursor.as_ref(), Some(&rows[0].1), "↓ marks the first row");
            rows[0].1.clone()
        })
        .unwrap();
        cx.simulate_keystrokes(w.into(), "enter");
        draw(cx);
        cx.simulate_keystrokes(w.into(), "down");
        w.update(cx, |m, _, _| {
            let keys: Vec<String> = heads(m).into_iter().map(|(_, k)| k).collect();
            assert_eq!(keys.len(), 3, "↩ opened the goal: its misses are rows now: {keys:?}");
            assert_eq!(m.home.cursor.as_ref(), Some(&keys[1]), "↓ goes into the misses");
        })
        .unwrap();
        cx.simulate_keystrokes(w.into(), "down down");
        w.update(cx, |m, _, _| assert_eq!(m.home.cursor.as_deref(), heads(m).last().map(|(_, k)| k.as_str()), "↓ stops at the last")).unwrap();
        cx.simulate_keystrokes(w.into(), "enter");
        w.update(cx, |m, _, _| {
            let last = m.home.cursor.clone().unwrap();
            assert!(flipped(&last), "↩ on a miss opens it, as a click does");
        })
        .unwrap();
        draw(cx);
        cx.simulate_keystrokes(w.into(), "up up up");
        w.update(cx, |m, _, _| assert_eq!(m.home.cursor.as_ref(), Some(&goal), "↑ goes back up to the goal")).unwrap();
    }

    #[gpui_kit::test]
    fn make_all_sends_one_batch_over_the_promote_action(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = window(cx);
        w.update(cx, |m, window, cx| run_do(m, "goal:G1", &Do::MakeAll(vec!["B4".into(), "B5".into()]), window, cx)).unwrap();
        settle(cx);
        assert_eq!(rec.last("backlog/bulk"), Some(json!({"ids": ["B4", "B5"], "action": "task", "where": "board"})));
    }
}
