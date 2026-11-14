//! What the recap components are made of: the card, its rows and bars, disclosures, stat tiles,
//! legends, columns, and the number formats the design uses.
use crate::app::MainWindow;
use crate::fmt;
use crate::theme::{Theme, ThemeMode};
use crate::ui::{kit, motion};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::Value;
use std::collections::HashSet;

/// A click handler built with `cx.listener`.
pub type Click = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

/// Lists longer than this draw only the rows in view, in a box that scrolls.
pub const LONG: usize = 24;
/// Rows a long list shows before it scrolls.
const LONG_SHOWN: usize = 12;
/// A row's height in a long list.
pub const ROW_H: f32 = 32.;

/// What every component draws from: the recap's facts and the page's open disclosures.
pub struct Ctx {
    pub t: Theme,
    /// The recap shown; in every animation id, so another recap plays its entrances again.
    pub rid: i64,
    pub month: bool,
    pub facts: Value,
    pub open: HashSet<String>,
    /// Project names by tasks, for their colours.
    pub projects: Vec<String>,
    /// The page's summary and highlights, in the opening line.
    pub summary: String,
    pub highlights: Vec<String>,
}

impl Ctx {
    pub fn new(t: Theme, rid: i64, month: bool, facts: Value, open: HashSet<String>) -> Ctx {
        let projects = active(&facts["projects"]).iter().map(|p| fmt::s(p, "name").to_string()).collect();
        Ctx { t, rid, month, facts, open, projects, summary: String::new(), highlights: vec![] }
    }

    /// A fact by its dotted key ("totals.tasks").
    pub fn fact(&self, key: &str) -> &Value {
        key.split('.').fold(&self.facts, |v, k| &v[k])
    }

    /// A component's prop as the page filled it, or the fact it defaults to.
    pub fn prop<'a>(&'a self, comp: &'a Value, name: &str, default: &str) -> &'a Value {
        match &comp["props"][name]["value"] {
            Value::Null => self.fact(default),
            v => v,
        }
    }

    /// A goal from the facts by its ref ("G12").
    pub fn goal(&self, r: &str) -> Option<&Value> {
        fmt::arr(&self.facts, "goals").iter().find(|g| fmt::s(g, "goal") == r)
    }

    pub fn id(&self, k: impl std::fmt::Display) -> SharedString {
        format!("rc{}-{k}", self.rid).into()
    }

    /// A project's colour: the four busiest get their own, the rest grey.
    pub fn pc(&self, name: &str) -> Hsla {
        palette(self.t.mode, self.projects.iter().position(|p| p == name).unwrap_or(usize::MAX))
    }

    /// The period before, as the deltas name it.
    pub fn prev(&self) -> &'static str {
        if self.month { "last month" } else { "last week" }
    }

    pub fn before(&self) -> Option<&Value> {
        Some(&self.facts["before"]).filter(|v| v.is_object())
    }

    pub fn is_open(&self, k: &str) -> bool {
        self.open.contains(k)
    }
}

/// Projects (a `projects` fact) that finished tasks, busiest first.
pub fn active(list: &Value) -> Vec<Value> {
    let mut ps: Vec<Value> = list.as_array().map_or(&[][..], Vec::as_slice).iter().filter(|p| num(&p["tasks"]) > 0.).cloned().collect();
    ps.sort_by(|a, b| num(&b["tasks"]).total_cmp(&num(&a["tasks"])));
    ps
}

/// The project colours of the design, then grey.
pub fn palette(mode: ThemeMode, i: usize) -> Hsla {
    const LIGHT: [u32; 5] = [0x178a77, 0xb87a00, 0x4a3aa7, 0xd4558a, 0x8a95a3];
    const DARK: [u32; 5] = [0x2bb39c, 0xc98500, 0x9085e9, 0xd55181, 0x6f7a88];
    let pal = if mode == ThemeMode::Dark { DARK } else { LIGHT };
    rgb(pal[i.min(4)]).into()
}

pub fn grey(t: &Theme) -> Hsla {
    palette(t.mode, 4)
}

/// A command category's name and colour; the waits for CI, polls and loads go together.
pub fn category(t: &Theme, name: &str) -> (&'static str, Hsla, usize) {
    let dark = t.mode == ThemeMode::Dark;
    let c = |l: u32, d: u32| -> Hsla { rgb(if dark { d } else { l }).into() };
    match name {
        "test" => ("Tests", c(0x2447b8, 0xa9bcff), 0),
        "build" => ("Builds", t.accent, 1),
        "lint" => ("Lint", c(0x6f8df0, 0x5a78d8), 2),
        "git" => ("Git", c(0x5f6b7a, 0xa1abb8), 3),
        "ci wait" | "background poll" | "load wait" | "wait" => ("CI and polling", c(0x3d4652, 0xc3cad4), 4),
        _ => ("Other", grey(t), 5),
    }
}

// ------------------------------------------------------------------ numbers

pub fn num(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.)
}

/// A whole number with thousands separators.
pub fn int(x: f64) -> String {
    let n = x.round() as i64;
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 { format!("−{out}") } else { out }
}

/// Minutes as "45m", "3h 20m", or "140h" past 100 hours.
pub fn dur(min: f64) -> String {
    let m = min.max(0.).round() as i64;
    if m < 60 {
        return format!("{m}m");
    }
    if min >= 6000. {
        return format!("{}h", int(min / 60.));
    }
    match (m / 60, m % 60) {
        (h, 0) => format!("{h}h"),
        (h, r) => format!("{h}h {r}m"),
    }
}

pub fn pl(n: f64, one: &str, many: &str) -> String {
    format!("{} {}", int(n), if n.round() == 1. { one } else { many })
}

/// "↑ 3", "↓ 2h", or "No change".
pub fn arrow(cur: f64, prev: f64, f: fn(f64) -> String) -> String {
    let d = cur - prev;
    if d.abs() < 0.5 {
        return "No change".into();
    }
    format!("{} {}", if d > 0. { "↑" } else { "↓" }, f(d.abs()))
}

/// "↑ 3 vs last week", when there's a period before and either side has something.
pub fn vs(c: &Ctx, cur: f64, key: &str, f: fn(f64) -> String) -> Option<String> {
    let prev = num(&c.before()?[key]);
    (cur != 0. || prev != 0.).then(|| format!("{} vs {}", arrow(cur, prev, f), c.prev()))
}

pub fn date(d: &str) -> Option<chrono::NaiveDate> {
    chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()
}

/// "Nov 2".
pub fn day(d: &str) -> String {
    date(d).map(|d| d.format("%b %-d").to_string()).unwrap_or_default()
}

/// "Mon".
pub fn weekday(d: &str) -> String {
    date(d).map(|d| d.format("%a").to_string()).unwrap_or_default()
}

/// "Fri, Nov 6 · 3:12 PM" in local time.
pub fn when(iso: &str) -> String {
    fmt::parse(iso).map(|t| fmt::local(t).format("%a, %b %-d · %-I:%M %p").to_string()).unwrap_or_default()
}

/// An ISO time's local date, "2026-11-02".
pub fn fmt_date(iso: &str) -> String {
    fmt::parse(iso).map(|t| fmt::local(t).format("%Y-%m-%d").to_string()).unwrap_or_default()
}

/// "Fri, Nov 6".
pub fn when_day(iso: &str) -> String {
    fmt::parse(iso).map(|t| fmt::local(t).format("%a, %b %-d").to_string()).unwrap_or_default()
}

/// Keeps the first `n` and folds the rest into one "Other" by `key`.
pub fn fold(rows: Vec<(String, f64)>, n: usize) -> Vec<(String, f64)> {
    if rows.len() <= n {
        return rows;
    }
    let rest: f64 = rows[n..].iter().map(|r| r.1).sum();
    let mut out = rows[..n].to_vec();
    out.push(("Other".into(), rest));
    out
}

// ------------------------------------------------------------------ text

pub fn small(t: &Theme, text: impl Into<SharedString>) -> Div {
    div().text_size(px(12.)).text_color(t.text_2).child(text.into())
}

pub fn muted(t: &Theme, text: impl Into<SharedString>) -> Div {
    div().text_size(px(12.)).text_color(t.muted).child(text.into())
}

/// The 24px figure.
pub fn big(text: impl Into<SharedString>) -> Div {
    div().text_size(px(24.)).font_weight(FontWeight::BOLD).line_height(px(29.)).whitespace_nowrap().child(text.into())
}

/// A 24px figure counting up.
pub fn big_count(c: &Ctx, key: &str, delay: u64, to: f64, f: fn(f64) -> String) -> AnyElement {
    let base = div().text_size(px(24.)).font_weight(FontWeight::BOLD).line_height(px(29.)).whitespace_nowrap();
    motion::count(base, c.id(format!("{key}-n")), delay, to, f)
}

/// The 56px figure counting up.
pub fn hero(c: &Ctx, key: &str, to: f64, f: fn(f64) -> String) -> AnyElement {
    let base = div().text_size(px(56.)).font_weight(FontWeight::BOLD).line_height(px(59.)).whitespace_nowrap();
    motion::count(base, c.id(format!("{key}-hero")), 80, to, f)
}

/// The value at a card's top right.
pub fn right(t: &Theme, text: impl Into<SharedString>) -> AnyElement {
    div().text_size(px(14.)).text_color(t.text_2).whitespace_nowrap().child(text.into()).into_any_element()
}

/// The 8px square before a coloured label.
pub fn sw(color: Hsla) -> Div {
    div().flex_none().size(px(8.)).rounded(px(2.)).bg(color).mr(px(4.))
}

pub fn legend(t: &Theme, items: Vec<(Hsla, String)>) -> Div {
    div()
        .flex()
        .flex_wrap()
        .gap_x(px(16.))
        .gap_y(px(4.))
        .text_size(px(12.))
        .text_color(t.text_2)
        .children(items.into_iter().map(|(c, l)| div().flex().items_center().whitespace_nowrap().child(sw(c)).child(l)))
}

// ------------------------------------------------------------------ the card

/// A component's card: an optional title with a value at its right, then the body. It fades in.
pub fn card(c: &Ctx, key: &str, title: Option<&str>, right: Option<AnyElement>, body: Vec<AnyElement>) -> AnyElement {
    let t = &c.t;
    let head = title.map(|title| {
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .min_h(px(24.))
            .child(div().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
            .children(right)
    });
    let el = div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .p(px(16.))
        .h_full()
        .min_w_0()
        .bg(t.card)
        .border_1()
        .border_color(t.border)
        .rounded(px(12.))
        .text_size(px(14.))
        .text_color(t.text)
        .children(head)
        .children(body);
    motion::fade(el, c.id(format!("{key}-card")), 0)
}

/// What a card says when it has nothing to show.
pub fn none(t: &Theme, text: impl Into<SharedString>) -> AnyElement {
    div().text_color(t.muted).child(text.into()).into_any_element()
}

// ------------------------------------------------------------------ rows

/// A row that highlights under the pointer.
pub fn hit(t: &Theme, id: SharedString) -> Stateful<Div> {
    let (hover, press) = (t.panel_2, t.seg);
    div().id(id).rounded(px(8.)).cursor_pointer().hover(move |s| s.bg(hover)).active(move |s| s.bg(press))
}

/// A bar row: label and value, with a bar under them for the share. `key` names it.
pub struct Brow {
    pub label: String,
    pub value: String,
    pub share: f64,
    pub color: Option<Hsla>,
    pub tip: Option<String>,
    /// A chevron before the label, pointing down while open.
    pub toggle: Option<bool>,
    /// A ref in mono before the label.
    pub pre: Option<String>,
}

impl Brow {
    pub fn new(label: impl Into<String>, value: impl Into<String>, share: f64) -> Brow {
        Brow { label: label.into(), value: value.into(), share, color: None, tip: None, toggle: None, pre: None }
    }
    pub fn color(mut self, c: Hsla) -> Brow {
        self.color = Some(c);
        self
    }
    pub fn tip(mut self, t: impl Into<String>) -> Brow {
        self.tip = Some(t.into());
        self
    }
    pub fn pre(mut self, r: impl Into<String>) -> Brow {
        self.pre = Some(r.into());
        self
    }
    pub fn toggle(mut self, open: bool) -> Brow {
        self.toggle = Some(open);
        self
    }
}

pub fn brow(c: &Ctx, key: &str, i: usize, b: Brow, click: Option<Click>) -> AnyElement {
    let t = &c.t;
    let color = b.color.unwrap_or(t.accent);
    let label = div()
        .flex()
        .items_center()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .children(b.toggle.map(|open| chev(t, open)))
        .children(b.color.map(sw))
        .children(b.pre.map(|r| div().flex_none().mr(px(6.)).font_family(t.mono_font.clone()).text_size(px(12.)).text_color(t.muted).child(r)))
        .child(div().min_w_0().truncate().child(b.label));
    let top = div().flex().items_center().gap(px(16.)).child(label).child(div().flex_none().text_color(t.text_2).whitespace_nowrap().child(b.value));
    let track = div().flex().w_full().h(px(8.)).child(motion::grow_w(c.id(format!("{key}-bar")), motion::st(i, 40, 200), motion::GROW_W, b.share.max(0.004) as f32, color, 8.));
    let open = b.toggle == Some(true);
    let row = div()
        .id(c.id(format!("{key}-row")))
        .flex()
        .flex_col()
        .gap(px(2.))
        .px(px(8.))
        .py(px(4.))
        .mx(px(-8.))
        .min_h(px(32.))
        .rounded(px(8.))
        .when(open, |d| d.bg(t.accent_soft))
        .child(top)
        .child(track);
    let row = match (click, b.tip) {
        (Some(f), tip) => {
            let (hover, press) = (t.panel_2, t.seg);
            row.cursor_pointer().when(!open, |d| d.hover(move |s| s.bg(hover))).active(move |s| s.bg(press)).on_click(f).when_some(tip, |d, s| d.tooltip(kit::tip(s)))
        }
        (None, Some(tip)) => {
            let hover = t.panel_2;
            row.hover(move |s| s.bg(hover)).tooltip(kit::tip(tip))
        }
        (None, None) => row,
    };
    motion::fade(row, c.id(format!("{key}-in")), motion::row(i))
}

/// A task row: its ref and title, a value at the right; it opens the task in the side panel.
pub fn trow(c: &Ctx, key: &str, i: usize, task: &str, title: &str, value: String, cx: &mut Context<MainWindow>) -> AnyElement {
    let el = task_row(&c.t, c.id(format!("{key}-t")), task, title, value, cx);
    motion::fade(el, c.id(format!("{key}-tin")), motion::st(i, 24, 0))
}

/// [`trow`] without its entrance, for long lists.
pub fn task_row(t: &Theme, id: SharedString, task: &str, title: &str, value: String, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let r = task.to_string();
    hit(t, id)
        .flex()
        .items_center()
        .gap(px(16.))
        .h(px(ROW_H))
        .px(px(8.))
        .mx(px(-8.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .flex_1()
                .min_w_0()
                .child(div().flex_none().font_family(t.mono_font.clone()).text_size(px(12.)).text_color(t.muted).child(task.to_string()))
                .child(div().min_w_0().truncate().child(title.to_string())),
        )
        .when(!value.is_empty(), |d| d.child(div().flex_none().text_size(px(12.)).text_color(t.text_2).whitespace_nowrap().child(value)))
        .on_click(cx.listener(move |m, _, _, cx| m.open_task(r.clone(), cx)))
}

/// Opens a goal's page.
pub fn go_goal(r: &str, cx: &mut Context<MainWindow>) -> Click {
    let r = r.to_string();
    Box::new(cx.listener(move |m, _, _, cx| crate::ui::goal::open(m, &r, false, cx)))
}

/// Opens a release on its project's page.
pub fn go_release(project: &str, r: &str, cx: &mut Context<MainWindow>) -> Click {
    let (p, r) = (project.to_string(), r.to_string());
    Box::new(cx.listener(move |m, _, _, cx| crate::ui::releases::open_in(m, p.clone(), &r, cx)))
}

/// A row of a label and a small value; it highlights and opens something when it has a click.
pub fn lrow(t: &Theme, id: SharedString, label: impl IntoElement, value: impl Into<SharedString>, click: Option<Click>) -> Stateful<Div> {
    let row = div()
        .id(id)
        .flex()
        .items_center()
        .gap(px(16.))
        .min_h(px(ROW_H))
        .px(px(8.))
        .mx(px(-8.))
        .rounded(px(8.))
        .child(div().flex().items_center().flex_1().min_w_0().overflow_hidden().child(label))
        .child(div().flex_none().text_size(px(12.)).text_color(t.text_2).whitespace_nowrap().child(value.into()));
    match click {
        Some(f) => {
            let (hover, press) = (t.panel_2, t.seg);
            row.cursor_pointer().hover(move |s| s.bg(hover)).active(move |s| s.bg(press)).on_click(f)
        }
        None => row,
    }
}

/// A project's swatch and name, then `rest` in mono.
pub fn named(c: &Ctx, project: &str, rest: &str) -> Div {
    div()
        .flex()
        .items_center()
        .min_w_0()
        .child(sw(c.pc(project)))
        .child(div().flex_none().child(project.to_string()))
        .when(!rest.is_empty(), |d| d.child(div().ml(px(6.)).min_w_0().truncate().font_family(c.t.mono_font.clone()).child(rest.to_string())))
}

/// A row of plain label and value, no bar.
pub fn kv(t: &Theme, label: impl Into<SharedString>, value: impl Into<SharedString>) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(16.))
        .min_h(px(28.))
        .child(div().flex_1().min_w_0().truncate().child(label.into()))
        .child(div().flex_none().text_color(t.text_2).whitespace_nowrap().child(value.into()))
}

/// Rows drawn all at once, or past [`LONG`] only those in view in a box that scrolls.
pub fn many<T: 'static>(c: &Ctx, key: &str, items: Vec<T>, cx: &mut Context<MainWindow>, row: impl Fn(&Theme, usize, &T, &mut Context<MainWindow>) -> AnyElement + 'static) -> AnyElement {
    let n = items.len();
    if n <= LONG {
        let rows: Vec<AnyElement> = items.iter().enumerate().map(|(i, x)| row(&c.t, i, x, cx)).collect();
        return div().flex().flex_col().children(rows).into_any_element();
    }
    let t = c.t.clone();
    uniform_list(c.id(format!("{key}-list")), n, cx.processor(move |_, range: std::ops::Range<usize>, _, cx| range.map(|i| row(&t, i, &items[i], cx)).collect::<Vec<_>>()))
        .h(px(LONG_SHOWN as f32 * ROW_H))
        .into_any_element()
}

/// A goal's or release's tasks as rows that open the task panel; past [`LONG`] only those in view.
pub fn tasks(c: &Ctx, key: &str, items: Vec<Value>, cx: &mut Context<MainWindow>) -> AnyElement {
    let long = items.len() > LONG;
    let pre = format!("rc{}-{key}", c.rid);
    many(c, key, items, cx, move |t, i, x, cx| {
        let id = SharedString::from(format!("{pre}-t{i}"));
        let el = task_row(t, id, fmt::s(x, "task"), fmt::s(x, "title"), when_day(fmt::s(x, "finished_at")), cx);
        if long { el.into_any_element() } else { motion::fade(el, SharedString::from(format!("{pre}-tin{i}")), motion::st(i, 24, 0)) }
    })
}

/// `.lbl`: a small heading over rows.
pub fn lbl(t: &Theme, text: impl Into<SharedString>) -> Div {
    div().pt(px(4.)).text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(t.muted).child(text.into())
}

/// "■ project · 12h estimated".
pub fn goal_line(c: &Ctx, g: &Value) -> Div {
    let est = num(&g["est_human_min"]);
    let text = if est > 0. { format!("{} · {} estimated", fmt::s(g, "project"), dur(est)) } else { fmt::s(g, "project").to_string() };
    div().flex().items_center().text_size(px(12.)).text_color(c.t.text_2).child(sw(c.pc(fmt::s(g, "project")))).child(div().min_w_0().truncate().child(text))
}

/// A goal's finished tasks, from the facts' task lists.
pub fn goal_tasks(c: &Ctx, r: &str) -> Vec<Value> {
    fmt::arr(&c.facts["tasks"], "by_goal").iter().find(|g| fmt::s(g, "goal") == r).map(|g| fmt::arr(g, "tasks").to_vec()).unwrap_or_default()
}

// ------------------------------------------------------------------ disclosures

pub fn chev(t: &Theme, open: bool) -> Div {
    div().flex_none().w(px(12.)).mr(px(4.)).text_size(px(10.)).text_color(t.muted).child(if open { "▾" } else { "▸" })
}

/// A disclosure's button; `key` goes in the page's open set. A key with a `|` belongs to the group
/// before it, which keeps one open at a time.
pub fn disc(c: &Ctx, key: &str, label: impl Into<SharedString>, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let t = &c.t;
    let open = c.is_open(key);
    let (hover, press, fg) = (t.panel_2, t.seg, t.text);
    let k = key.to_string();
    div()
        .id(c.id(format!("{key}-disc")))
        .flex()
        .flex_none()
        .items_center()
        .min_h(px(24.))
        .px(px(4.))
        .ml(px(-4.))
        .rounded(px(4.))
        .text_size(px(12.))
        .text_color(t.text_2)
        .cursor_pointer()
        .hover(move |s| s.bg(hover).text_color(fg))
        .active(move |s| s.bg(press))
        .child(chev(t, open))
        .child(label.into())
        .on_click(cx.listener(move |m, _, _, cx| super::toggle(m, &k, cx)))
}

/// A disclosure and, while open, what it holds; `less` relabels it while open. `h` is about how
/// tall the body is, for its opening.
pub fn xp(c: &Ctx, key: &str, label: &str, less: Option<&str>, h: f32, cx: &mut Context<MainWindow>, body: impl FnOnce(&mut Context<MainWindow>) -> Div) -> Vec<AnyElement> {
    let open = c.is_open(key);
    let btn = disc(c, key, if open { less.unwrap_or(label).to_string() } else { label.to_string() }, cx).into_any_element();
    if !open {
        return vec![btn];
    }
    vec![btn, opened(c, key, h, body(cx))]
}

/// An open disclosure's body, opening over 200 ms.
pub fn opened(c: &Ctx, key: &str, h: f32, body: Div) -> AnyElement {
    motion::expand(div().flex().flex_col().gap(px(8.)).pt(px(4.)).pb(px(8.)).child(body), c.id(format!("{key}-open")), h)
}

// ------------------------------------------------------------------ figures

/// `.stat3`: label over a 24px value, wrapping.
pub fn stat3(t: &Theme, items: Vec<(String, String)>) -> Div {
    div().flex().flex_wrap().gap_x(px(24.)).gap_y(px(8.)).children(items.into_iter().map(|(l, v)| {
        div().flex().flex_col().child(div().text_size(px(12.)).text_color(t.muted).child(l)).child(big(v))
    }))
}

/// Stat tiles with a coloured left edge: three to a row, a fourth and fifth two to a row.
pub fn tiles(t: &Theme, color: Hsla, items: Vec<(String, String)>) -> Div {
    let n = items.len();
    let per = match n {
        4 => vec![2, 2],
        5 => vec![3, 2],
        _ => vec![3; n.div_ceil(3)],
    };
    let mut it = items.into_iter();
    let mut out = div().flex().flex_col().gap(px(8.));
    for k in per {
        let row = div().flex().gap(px(8.)).children(it.by_ref().take(k).map(|(l, v)| {
            div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .px(px(12.))
                .py(px(8.))
                .rounded(px(8.))
                .bg(t.panel_2)
                .border_l(px(3.))
                .border_color(color)
                .child(div().text_size(px(12.)).text_color(t.muted).child(l))
                .child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).line_height(px(29.)).truncate().child(v))
        }));
        out = out.child(row);
    }
    out
}

/// A horizontal stack of shares, wiped in from the left.
pub fn stack(c: &Ctx, key: &str, delay: u64, dur: u64, h: f32, parts: Vec<(f64, Hsla, String)>) -> AnyElement {
    let total: f64 = parts.iter().map(|p| p.0).sum::<f64>().max(1e-9);
    let last = parts.len().saturating_sub(1);
    let bar = div().relative().flex().gap(px(2.)).w_full().h(px(h)).children(parts.into_iter().enumerate().filter(|(_, p)| p.0 > 0.).map(|(i, (w, color, tip))| {
        let fg = c.t.text;
        div()
            .id(c.id(format!("{key}-s{i}")))
            .h_full()
            .min_w(px(2.))
            .w(relative((w / total) as f32))
            .flex_shrink(1.)
            .bg(color)
            .when(i == last, |d| d.rounded_r(px(4.)))
            .hover(move |s| s.border_1().border_color(fg))
            .tooltip(kit::tip(tip))
    }));
    bar.child(motion::wipe(c.id(format!("{key}-wipe")), delay, dur, c.t.card)).into_any_element()
}

/// Columns growing up from a baseline, one per value; `on` marks the open one.
pub struct Col {
    pub value: f64,
    pub tip: String,
    pub dim: bool,
}

/// `top` is the value of a full column; 0 scales to the largest. `click` has one handler per
/// column, or none.
pub fn cols(c: &Ctx, key: &str, h: f32, step: u64, top: f64, list: &[Col], on: Option<usize>, click: Vec<Click>) -> Div {
    let t = &c.t;
    let max = if top > 0. { top } else { list.iter().map(|x| x.value).fold(0., f64::max).max(1.) };
    let (hover, press) = (t.panel_2, t.seg);
    let mut click = click.into_iter();
    div().flex().items_end().gap(px(2.)).h(px(h)).children(list.iter().enumerate().map(|(i, x)| {
        let bar = div().w_full().max_w(px(24.)).rounded_t(px(4.)).bg(if x.dim { grey(t) } else { t.accent });
        let bar = motion::grow_h(bar, c.id(format!("{key}-c{i}")), i.min(8) as u64 * step, motion::GROW_H, (x.value / max) as f32);
        let col = div()
            .id(c.id(format!("{key}-cw{i}")))
            .flex()
            .flex_col()
            .justify_end()
            .items_center()
            .flex_1()
            .min_w_0()
            .h_full()
            .rounded(px(4.))
            .when(on == Some(i), |d| d.bg(t.accent_soft))
            .child(bar)
            .tooltip(kit::tip(x.tip.clone()));
        match click.next() {
            Some(f) => col.cursor_pointer().hover(move |s| s.bg(hover)).active(move |s| s.bg(press)).on_click(f),
            None => col.hover(move |s| s.bg(hover)),
        }
    }))
}

/// Labels under columns, one each, or the first, middle and last past a week.
pub fn axis(t: &Theme, labels: Vec<String>) -> Div {
    let n = labels.len();
    let row = div().flex().gap(px(2.)).text_size(px(12.)).text_color(t.muted);
    if n <= 7 {
        return row.children(labels.into_iter().map(|l| div().flex_1().min_w_0().flex().justify_center().overflow_hidden().whitespace_nowrap().child(l)));
    }
    let keep = [0, n / 2, n - 1];
    row.justify_between().children(labels.into_iter().enumerate().filter(|(i, _)| keep.contains(i)).map(|(_, l)| div().flex_none().whitespace_nowrap().child(l)))
}

/// `.tbl`: a header row and rows, the first column left and the rest right.
pub fn table(t: &Theme, head: &[&str], rows: Vec<Vec<String>>) -> Div {
    let line = |cells: Vec<String>, header: bool| {
        div().flex().items_center().gap(px(16.)).py(px(4.)).border_b_1().border_color(t.divider).text_size(px(12.)).children(cells.into_iter().enumerate().map(|(i, s)| {
            let cell = if i == 0 { div().flex_1().min_w_0() } else { div().flex_none().w(px(84.)).flex().justify_end().whitespace_nowrap() };
            let color = match (header, i) {
                (true, _) => t.muted,
                (false, 0) => t.text,
                _ => t.text_2,
            };
            cell.text_color(color).child(s)
        }))
    };
    div().flex().flex_col().child(line(head.iter().map(|s| s.to_string()).collect(), true)).children(rows.into_iter().map(|r| line(r, false)))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's `test` attribute.
    use super::{arrow, day, dur, fold, int, pl, weekday};

    #[test]
    fn numbers_read_as_the_design_writes_them() {
        assert_eq!(int(1234567.), "1,234,567");
        assert_eq!(int(12.4), "12");
        assert_eq!(dur(45.), "45m");
        assert_eq!(dur(200.), "3h 20m");
        assert_eq!(dur(180.), "3h");
        assert_eq!(dur(7200.), "120h");
        assert_eq!(arrow(5., 5., int), "No change");
        assert_eq!(arrow(8., 5., int), "↑ 3");
        assert_eq!(arrow(60., 180., dur), "↓ 2h");
        assert_eq!(pl(1., "task", "tasks"), "1 task");
        assert_eq!(pl(3., "task", "tasks"), "3 tasks");
    }

    #[test]
    fn fold_keeps_the_top_and_sums_the_rest() {
        let rows = vec![("a".into(), 5.), ("b".into(), 3.), ("c".into(), 2.), ("d".into(), 1.)];
        assert_eq!(fold(rows.clone(), 4), rows);
        assert_eq!(fold(rows, 2), vec![("a".into(), 5.), ("b".into(), 3.), ("Other".into(), 3.)]);
    }

    #[test]
    fn days_read_short() {
        assert_eq!(day("2026-11-02"), "Nov 2");
        assert_eq!(weekday("2026-11-02"), "Mon");
    }
}
