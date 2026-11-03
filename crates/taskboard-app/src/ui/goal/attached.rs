//! The goal page's Attached card: the goal's own attachments, then its tasks' files, one wave at a
//! time. A wave menu picks the wave (it opens on the wave in progress, or on All waves), a kind menu
//! narrows to one kind of file, and neither resets the other: each lists what the other leaves and
//! disables the choices that hold nothing. Each task's ref and title head its files. Hovering an
//! image or a video shows a larger preview.
use super::*;
use crate::ui::task_panel::{ATT_KINDS, att_file_type, att_icon, chevron};

pub(super) const WAVE_MENU: &str = "goal-att-wave";
pub(super) const KIND_MENU: &str = "goal-att-kind";

/// The wave choice that shows every wave.
pub(super) const ALL_WAVES: &str = "all";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Evidence,
    Design,
    Docs,
}

const FILTERS: [(Filter, &str); 4] = [(Filter::All, "All kinds"), (Filter::Evidence, "Evidence"), (Filter::Design, "Design"), (Filter::Docs, "Docs")];

/// The kind an attachment counts under; `other` only shows under All.
fn bucket(kind: &str) -> Option<Filter> {
    match kind {
        "evidence" | "results" => Some(Filter::Evidence),
        "design" => Some(Filter::Design),
        "doc" | "proposal" => Some(Filter::Docs),
        _ => None,
    }
}

fn passes(filter: Filter, kind: &str) -> bool {
    filter == Filter::All || bucket(kind) == Some(filter)
}

/// How many of `kinds` each filter holds, in `FILTERS` order.
fn tally<'a>(kinds: impl Iterator<Item = &'a str>) -> [usize; 4] {
    let mut counts = [0; 4];
    for k in kinds {
        counts[0] += 1;
        if let Some(b) = bucket(k) {
            counts[FILTERS.iter().position(|(f, _)| *f == b).unwrap()] += 1;
        }
    }
    counts
}

fn slot(filter: Filter) -> usize {
    FILTERS.iter().position(|(f, _)| *f == filter).unwrap()
}

#[derive(Clone, Debug, PartialEq)]
pub enum Preview {
    /// A local image file (its full path) or an image link.
    Image(String),
    Video,
}

#[derive(Clone, Debug, PartialEq)]
pub struct File {
    pub title: String,
    pub url: String,
    /// A link or a file path the board can open.
    pub href: bool,
    pub kind: &'static str,
    /// The file's type when we draw one, else its kind.
    pub icon: &'static str,
    pub preview: Option<Preview>,
    /// "after.png · Oct 17, 3:42 PM".
    pub detail: String,
    pub summary: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskGroup {
    pub r: String,
    pub title: String,
    pub files: Vec<File>,
}

/// Where a wave stands, as the wave menu says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Done,
    Now,
    Held,
    Failed,
    Review,
    Ahead,
}

impl Phase {
    fn of(state: &str) -> Phase {
        match state {
            "done" => Phase::Done,
            "running" => Phase::Now,
            "held" => Phase::Held,
            "failed" => Phase::Failed,
            "stopped" => Phase::Review,
            _ => Phase::Ahead,
        }
    }

    pub fn label(self) -> Option<&'static str> {
        match self {
            Phase::Done => Some("Done"),
            Phase::Now => Some("Now"),
            Phase::Held => Some("Held"),
            Phase::Failed => Some("Failed"),
            Phase::Review => Some("Your review"),
            Phase::Ahead => None,
        }
    }

    /// The wave the goal is on: the card opens on it.
    fn current(self) -> bool {
        matches!(self, Phase::Now | Phase::Held | Phase::Failed | Phase::Review)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WaveGroup {
    /// The menu's key: `w2`, or `loose` for the tasks with no wave.
    pub key: String,
    /// None when the goal has no waves: its tasks stand on their own.
    pub label: Option<String>,
    pub phase: Phase,
    /// Its tasks with files of the kind in force; none when the kind leaves it empty.
    pub tasks: Vec<TaskGroup>,
}

impl WaveGroup {
    pub fn count(&self) -> usize {
        self.tasks.iter().map(|t| t.files.len()).sum()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rollup {
    /// The goal's own files of the kind in force.
    pub goal: Vec<File>,
    /// Every wave that holds a file of any kind, narrowed to the kind in force.
    pub waves: Vec<WaveGroup>,
    /// The wave shown (`w2`), or None for every wave.
    pub wave: Option<String>,
    /// Every attachment, whatever the choices.
    pub total: usize,
    /// How many each kind holds in the goal's own files and the wave shown, in `FILTERS` order.
    pub counts: [usize; 4],
    /// The kind in force: the one asked for, or All when the wave shown holds none of it.
    pub filter: Filter,
    /// The kind menu helps: some kind holds part of the goal's files, not all of them.
    pub kinds: bool,
}

impl Rollup {
    /// The waves listed: the one picked, or every wave that holds a file of the kind in force.
    pub fn shown_waves(&self) -> Vec<&WaveGroup> {
        match &self.wave {
            Some(k) => self.waves.iter().filter(|w| &w.key == k).collect(),
            None => self.waves.iter().filter(|w| w.count() > 0).collect(),
        }
    }

    pub fn shown(&self) -> usize {
        self.goal.len() + self.shown_waves().iter().map(|w| w.count()).sum::<usize>()
    }

    /// The wave menu shows when there's more than one wave to choose.
    pub fn picker(&self) -> bool {
        self.waves.len() > 1
    }

    /// "15", or "4 of 15" while either choice narrows the list.
    pub fn head_count(&self) -> String {
        let n = self.shown();
        if n == self.total { n.to_string() } else { format!("{n} of {}", self.total) }
    }

    /// The wave picker's label.
    pub fn wave_label(&self) -> String {
        self.wave.as_ref().and_then(|k| self.waves.iter().find(|w| &w.key == k)).and_then(|w| w.label.clone()).unwrap_or_else(|| "All waves".into())
    }
}

fn file(a: &Value) -> File {
    let kind = ATT_KINDS.iter().find(|k| **k == s(a, "kind")).copied().unwrap_or("other");
    let url = s(a, "url").to_string();
    let ftype = att_file_type(&url).map(|(t, _)| t);
    let preview = match ftype {
        Some("image") if att_web(a) => Some(Preview::Image(url.clone())),
        Some("image") => Some(Preview::Image(taskboardd::util::expand_home(&url).to_string_lossy().into_owned())),
        Some("video") => Some(Preview::Video),
        _ => None,
    };
    let name = url.trim_end_matches('/').rsplit('/').next().unwrap_or(&url).to_string();
    let when = fmt::opt_s(a, "at").and_then(fmt::parse).map(|t| fmt::local(t).format("%b %-d, %-I:%M %p").to_string());
    let detail = [Some(name).filter(|n| !n.is_empty()), when].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    File {
        title: s(a, "title").to_string(),
        href: att_has_href(a),
        url,
        kind,
        icon: ftype.unwrap_or(kind),
        preview,
        detail,
        summary: fmt::opt_s(a, "summary").map(str::to_string),
    }
}

/// The card for goal `g` with the kind `filter` and the `wave` asked for (`w2`, `all`, or None
/// for the default: the wave the goal is on, else every wave).
pub fn rollup(g: &Value, filter: Filter, wave: Option<&str>) -> Rollup {
    let own = arr(g, "attachments");
    let on_tasks = arr(g, "task_attachments");
    let total = own.len() + on_tasks.len();
    let whole = tally(own.iter().chain(on_tasks.iter()).map(|a| s(a, "kind")));
    let kinds = whole[1..].iter().any(|&n| n > 0 && n < total);
    let own: Vec<&Value> = own.iter().filter(|a| fmt::opt_s(a, "task").is_none()).collect();
    let tasks = arr(g, "tasks");
    // Each wave's tasks and their files, every kind.
    let group = |t: &Value| -> Option<(TaskGroup, Vec<&Value>)> {
        let r = s(t, "ref");
        let atts: Vec<&Value> = on_tasks.iter().filter(|a| fmt::opt_s(a, "task") == Some(r)).collect();
        (!atts.is_empty()).then(|| (TaskGroup { r: r.to_string(), title: s(t, "title").to_string(), files: vec![] }, atts))
    };
    let ws = arr(g, "waves");
    let mut all: Vec<(WaveGroup, Vec<(TaskGroup, Vec<&Value>)>)> = vec![];
    for w in ws {
        let n = i(w, "wave");
        let label = match fmt::opt_s(w, "name") {
            Some(name) => format!("Wave {n} · {name}"),
            None => format!("Wave {n}"),
        };
        let groups = tasks.iter().filter(|t| t["wave"] == w["wave"]).filter_map(group).collect();
        all.push((WaveGroup { key: format!("w{n}"), label: Some(label), phase: Phase::of(s(w, "state")), tasks: vec![] }, groups));
    }
    let loose = tasks.iter().filter(|t| ws.is_empty() || t["wave"].is_null()).filter_map(group).collect();
    all.push((WaveGroup { key: "loose".into(), label: (!ws.is_empty()).then(|| "No wave".into()), phase: Phase::Ahead, tasks: vec![] }, loose));
    all.retain(|(_, groups)| !groups.is_empty());

    let wave = match wave {
        _ if all.len() < 2 => None,
        Some(ALL_WAVES) => None,
        Some(k) if all.iter().any(|(w, _)| w.key == k) => Some(k.to_string()),
        _ => all.iter().find(|(w, _)| w.phase.current()).map(|(w, _)| w.key.clone()),
    };
    let in_view = |w: &WaveGroup| wave.as_ref().is_none_or(|k| &w.key == k);
    let counts = tally(own.iter().map(|a| s(a, "kind")).chain(all.iter().filter(|(w, _)| in_view(w)).flat_map(|(_, gs)| gs.iter().flat_map(|(_, atts)| atts.iter().map(|a| s(a, "kind"))))));
    let filter = if counts[slot(filter)] == 0 { Filter::All } else { filter };
    let keep = |atts: &[&Value]| -> Vec<File> { atts.iter().filter(|a| passes(filter, s(a, "kind"))).map(|a| file(a)).collect() };
    let waves = all
        .into_iter()
        .map(|(mut w, groups)| {
            w.tasks = groups.into_iter().map(|(mut tg, atts)| { tg.files = keep(&atts); tg }).filter(|tg| !tg.files.is_empty()).collect();
            w
        })
        .collect();
    Rollup { goal: keep(&own), waves, wave, total, counts, filter, kinds }
}

// ------------------------------------------------------------------ view

/// A text button that opens one of the card's menus where it was clicked.
fn menu_btn(t: &Theme, id: &'static str, menu: &'static str, label: String, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let tint = t.tint;
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .gap(px(4.))
        .px(px(6.))
        .h(px(24.))
        .rounded(px(6.))
        .text_size(px(12.))
        .text_color(t.text_2)
        .cursor_pointer()
        .hover(move |s| s.bg(tint))
        .child(label)
        .child(chevron(true, t.muted))
        .on_click(cx.listener(move |m, e: &ClickEvent, _, cx| {
            let p = e.position();
            m.toggle_menu(menu, point(p.x - px(150.), p.y + px(14.)), cx);
        }))
}

/// The wave picker: the wave shown, full width, opening the wave menu.
fn wave_btn(t: &Theme, label: String, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let tint = t.tint;
    div()
        .id("goal-att-wave-btn")
        .flex()
        .items_center()
        .gap(px(6.))
        .min_w_0()
        .mx(px(-6.))
        .px(px(6.))
        .h(px(30.))
        .rounded(px(6.))
        .border_1()
        .border_color(t.border)
        .cursor_pointer()
        .hover(move |s| s.bg(tint))
        .child(div().flex_1().min_w_0().truncate().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(label.clone()))
        .child(chevron(true, t.muted))
        .tooltip(kit::tip(label))
        .on_click(cx.listener(|m, e: &ClickEvent, _, cx| {
            let p = e.position();
            m.toggle_menu(WAVE_MENU, point(p.x - px(150.), p.y + px(16.)), cx);
        }))
}

fn file_row(t: &Theme, f: &File, id: String) -> Stateful<Div> {
    let tint = t.tint;
    let color = if f.kind == "results" { t.results } else { t.muted };
    let title = div().flex_1().min_w_0().truncate().child(f.title.clone());
    let row = div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .gap(px(8.))
        .min_w_0()
        .mx(px(-6.))
        .px(px(6.))
        .h(px(28.))
        .rounded(px(6.))
        .text_size(px(13.))
        .hover(move |s| s.bg(tint))
        .child(att_icon(f.icon, color))
        .child(if f.href { title.text_color(t.accent) } else { title.text_color(t.text) });
    let row = match &f.preview {
        Some(p) => row.tooltip(preview_tip(f.title.clone(), f.detail.clone(), f.summary.clone(), p.clone())),
        None => {
            let place = if f.url.starts_with("http://") || f.url.starts_with("https://") { "Link" } else { "File" };
            row.tooltip(kit::card_tip(f.title.clone(), f.summary.clone(), vec![(place.into(), fmt::short_path(&f.url))]))
        }
    };
    if f.href {
        let url = f.url.clone();
        row.cursor_pointer().on_click(move |_, _, cx| open_attachment(&url, cx))
    } else {
        row
    }
}

fn files(t: &Theme, list: &[File], key: &str) -> Div {
    div().flex().flex_col().children(list.iter().enumerate().map(|(ix, f)| file_row(t, f, format!("goal-att-{key}-{ix}"))))
}

/// A task's ref and title over its files; the label opens the task.
fn task_group(t: &Theme, tg: &TaskGroup, cx: &mut Context<MainWindow>) -> Div {
    let tint = t.tint;
    let r = tg.r.clone();
    let label = div()
        .id(SharedString::from(format!("goal-att-task-{}", tg.r)))
        .flex()
        .items_center()
        .gap(px(6.))
        .min_w_0()
        .mx(px(-6.))
        .px(px(6.))
        .h(px(24.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(move |s| s.bg(tint))
        .child(div().flex_none().text_size(px(11.)).font_family(t.mono_font.clone()).text_color(t.muted).child(tg.r.clone()))
        .child(div().min_w_0().truncate().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(t.text_2).child(tg.title.clone()))
        .tooltip(kit::tip(format!("{} {}", tg.r, tg.title)))
        .on_click(cx.listener(move |m, _, _, cx| m.open_task(r.clone(), cx)));
    div().flex().flex_col().min_w_0().child(label).child(files(t, &tg.files, &tg.r).pl(px(12.)))
}

/// How many list items the Attached section takes: its head (the count, the menus and the goal's
/// own files) and one per wave listed; none when nothing's attached.
pub(super) fn parts(r: &Rollup) -> usize {
    if r.total == 0 { 0 } else { 1 + r.shown_waves().len() }
}

/// Piece `k` of the Attached section (`parts`) of goal `g`, rolled up as `r`: 0 is its head, then
/// each wave listed.
pub(super) fn attached_piece(t: &Theme, r: &Rollup, k: usize, cx: &mut Context<MainWindow>) -> Div {
    let shown = r.shown_waves();
    let last = k == shown.len();
    let Some(w) = k.checked_sub(1).and_then(|k| shown.get(k)) else {
        let head = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(side_head(t, "Attached", Some(r.head_count())).flex_1())
            .when(r.kinds, |d| d.child(menu_btn(t, "goal-att-kind-btn", KIND_MENU, FILTERS[slot(r.filter)].1.to_string(), cx)));
        let mut out = div().flex().flex_col().gap(px(10.));
        if r.picker() {
            out = out.child(wave_btn(t, r.wave_label(), cx));
        }
        if !r.goal.is_empty() {
            let label = div().pb(px(2.)).text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(t.muted).child("GOAL");
            out = out.child(div().flex().flex_col().gap(px(4.)).child(label).child(files(t, &r.goal, "goal")));
        }
        return side_piece(t, true, last, 0., head.pb(px(8.))).child(out);
    };
    let tasks = div().flex().flex_col().gap(px(12.)).children(w.tasks.iter().map(|tg| task_group(t, tg, cx)));
    // Under All waves each wave says which it is; one wave picked is named by the picker.
    let body = match (&r.wave, &w.label) {
        (None, Some(label)) => {
            let head = div()
                .flex()
                .items_baseline()
                .gap(px(6.))
                .child(div().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(label.clone()))
                .child(div().text_size(px(12.)).text_color(t.muted).child(w.count().to_string()));
            div().flex().flex_col().gap(px(8.)).pt(px(10.)).border_t_1().border_color(t.divider).child(head).child(tasks)
        }
        _ => tasks,
    };
    side_piece(t, false, last, 10., body)
}

/// One choice in the card's menus: its label, a note (a wave's phase), its count; dimmed and
/// inert when it holds nothing.
fn pick_row(t: &Theme, id: String, label: String, note: Option<&'static str>, n: usize, on: bool) -> Stateful<Div> {
    let hover = t.panel_2;
    let off = n == 0 && !on;
    div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .gap(px(8.))
        .h(px(30.))
        .px(px(10.))
        .rounded(px(6.))
        .text_size(px(13.))
        .text_color(if on { t.accent_fg } else { t.text })
        .when(on, |d| d.bg(t.accent_soft))
        .when(off, |d| d.opacity(0.45))
        .when(!off, |d| d.cursor_pointer().hover(move |s| s.bg(hover)))
        .child(div().flex_1().min_w_0().truncate().child(label))
        .children(note.map(|x| div().flex_none().text_size(px(12.)).text_color(t.muted).child(x)))
        .child(div().flex_none().text_size(px(12.)).text_color(t.muted).child(n.to_string()))
}

/// The card's open menu, if one is: the waves or the kinds.
pub(super) fn menu(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let gr = s(g, "ref").to_string();
    let r = || rollup(g, m.goal_page.att_filter, m.goal_page.att_wave.get(&gr).map(String::as_str));
    if let Some(at) = m.menu_open(WAVE_MENU) {
        let r = r();
        let every = r.waves.iter().map(WaveGroup::count).sum::<usize>();
        let pick = |key: String| {
            let gr = gr.clone();
            cx.listener(move |m: &mut MainWindow, _: &ClickEvent, _: &mut Window, cx: &mut Context<MainWindow>| {
                m.goal_page.att_wave.insert(gr.clone(), key.clone());
                m.menu = None;
                cx.notify();
            })
        };
        let mut rows = vec![pick_row(t, "goal-att-wave-all".into(), "All waves".into(), None, every, r.wave.is_none()).on_click(pick(ALL_WAVES.into())).into_any_element()];
        for w in &r.waves {
            let row = pick_row(t, format!("goal-att-wave-{}", w.key), w.label.clone().unwrap_or_default(), w.phase.label(), w.count(), r.wave.as_ref() == Some(&w.key));
            rows.push(if w.count() > 0 { row.on_click(pick(w.key.clone())).into_any_element() } else { row.into_any_element() });
        }
        let list = div().id("goal-att-wave-list").flex().flex_col().max_h(px(360.)).overflow_y_scroll().children(rows);
        let menu = kit::menu_box(t, 300.).id("goal-att-wave-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).child(list);
        return Some(kit::popover(at, menu));
    }
    let at = m.menu_open(KIND_MENU)?;
    let r = r();
    let mut menu = kit::menu_box(t, 200.).id("goal-att-kind-menu").on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for (ix, (f, label)) in FILTERS.iter().enumerate() {
        let (f, n) = (*f, r.counts[ix]);
        let row = pick_row(t, format!("goal-att-kind-{ix}"), (*label).into(), None, n, r.filter == f);
        menu = menu.child(if n > 0 {
            row.on_click(cx.listener(move |m, _, _, cx| {
                m.goal_page.att_filter = f;
                m.menu = None;
                cx.notify();
            }))
        } else {
            row
        });
    }
    Some(kit::popover(at, menu))
}

// ------------------------------------------------------------------ hover card

struct PreviewTip {
    title: SharedString,
    detail: String,
    summary: Option<String>,
    preview: Preview,
}

impl Render for PreviewTip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<Theme>();
        let pane = div().h(px(200.)).w_full().flex().items_center().justify_center().border_b_1().border_color(t.border).overflow_hidden();
        let pane = match &self.preview {
            Preview::Image(src) => {
                let source: ImageSource = if src.starts_with('/') { std::path::Path::new(src).into() } else { SharedString::from(src.clone()).into() };
                pane.bg(t.panel_2).child(img(source).size_full().object_fit(ObjectFit::Contain))
            }
            Preview::Video => pane.bg(rgb(0x1c2129)).child(play(34.)),
        };
        div()
            .flex()
            .flex_col()
            .w(px(320.))
            .rounded(px(10.))
            .border_1()
            .border_color(t.border_2)
            .bg(t.card)
            .shadow_lg()
            .overflow_hidden()
            .font_family(t.ui_font.clone())
            .child(pane)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .px(px(12.))
                    .py(px(10.))
                    .child(div().text_size(px(13.)).font_weight(FontWeight::MEDIUM).text_color(t.text).child(self.title.clone()))
                    .when(!self.detail.is_empty(), |d| d.child(div().text_size(px(12.)).text_color(t.muted).child(self.detail.clone())))
                    .children(self.summary.clone().map(|x| div().text_size(px(12.)).text_color(t.text_2).child(x))),
            )
    }
}

/// A white play triangle, `size` px square.
fn play(size: f32) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let u = |x: f32, y: f32| point(b.origin.x + px(x * size / 24.), b.origin.y + px(y * size / 24.));
            let mut p = PathBuilder::fill();
            p.move_to(u(8., 5.));
            p.line_to(u(19., 12.));
            p.line_to(u(8., 19.));
            p.close();
            if let Ok(p) = p.build() {
                window.paint_path(p, white());
            }
        },
    )
    .size(px(size))
    .flex_none()
}

fn preview_tip(title: String, detail: String, summary: Option<String>, preview: Preview) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let title = SharedString::from(title);
    move |_, cx| cx.new(|_| PreviewTip { title: title.clone(), detail: detail.clone(), summary: summary.clone(), preview: preview.clone() }).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ::core::prelude::v1::test;

    fn att(kind: &str, title: &str, url: &str, task: Option<&str>) -> Value {
        json!({"kind": kind, "title": title, "url": url, "task": task, "at": null, "summary": null})
    }

    fn goal() -> Value {
        let own = vec![att("design", "Concepts", "https://example.com/d", None), att("doc", "Rules", "~/rules.pdf", None)];
        let on_tasks = vec![
            att("evidence", "Before", "~/before.png", Some("T1")),
            att("evidence", "After", "~/after.mov", Some("T1")),
            att("results", "Counts", "~/counts.csv", Some("T3")),
            att("other", "Misc", "https://example.com/x", Some("T4")),
        ];
        let tasks = vec![
            json!({"ref": "T1", "title": "Group by project", "wave": 1}),
            json!({"ref": "T2", "title": "Nothing attached", "wave": 1}),
            json!({"ref": "T3", "title": "Counts", "wave": 2}),
            json!({"ref": "T4", "title": "Loose", "wave": null}),
        ];
        let waves = json!([{"wave": 1, "name": "Grouping", "state": "done"}, {"wave": 2, "state": "running"}]);
        json!({"ref": "G1", "attachments": own, "task_attachments": on_tasks, "waves": waves, "tasks": tasks})
    }

    fn shape(r: &Rollup) -> Vec<(Option<String>, usize, Vec<(String, usize)>)> {
        r.shown_waves().iter().map(|w| (w.label.clone(), w.count(), w.tasks.iter().map(|t| (t.r.clone(), t.files.len())).collect())).collect()
    }

    #[test]
    fn every_wave_with_a_file_is_a_choice_and_empty_tasks_have_no_group() {
        let r = rollup(&goal(), Filter::All, Some(ALL_WAVES));
        assert_eq!(r.goal.iter().map(|f| f.title.as_str()).collect::<Vec<_>>(), ["Concepts", "Rules"]);
        assert_eq!(r.waves.iter().map(|w| w.key.as_str()).collect::<Vec<_>>(), ["w1", "w2", "loose"]);
        assert_eq!(
            shape(&r),
            vec![
                (Some("Wave 1 · Grouping".into()), 2, vec![("T1".into(), 2)]),
                (Some("Wave 2".into()), 1, vec![("T3".into(), 1)]),
                (Some("No wave".into()), 1, vec![("T4".into(), 1)]),
            ],
            "T2 has nothing attached, so it has no group"
        );
        assert_eq!((r.total, r.shown(), r.counts, r.head_count()), (6, 6, [6, 3, 1, 1], "6".into()));
        assert!(r.kinds && r.picker());
        assert_eq!(r.wave_label(), "All waves");
    }

    #[test]
    fn it_opens_on_the_wave_the_goal_is_on() {
        let r = rollup(&goal(), Filter::All, None);
        assert_eq!(r.wave.as_deref(), Some("w2"), "wave 2 is running");
        assert_eq!(shape(&r), vec![(Some("Wave 2".into()), 1, vec![("T3".into(), 1)])]);
        assert_eq!((r.wave_label().as_str(), r.head_count().as_str()), ("Wave 2", "3 of 6"), "the goal's own two files and wave 2's one");
        assert_eq!(r.waves[0].phase.label(), Some("Done"));
        let mut idle = goal();
        idle["waves"] = json!([{"wave": 1, "name": "Grouping", "state": "done"}, {"wave": 2, "state": "waiting"}]);
        assert_eq!(rollup(&idle, Filter::All, None).wave, None, "no wave running: every wave");
    }

    #[test]
    fn a_wave_picked_stays_picked_and_the_kinds_count_within_it() {
        let r = rollup(&goal(), Filter::All, Some("w1"));
        assert_eq!(r.wave.as_deref(), Some("w1"));
        assert_eq!(r.counts, [4, 2, 1, 1], "the goal's design and doc, and wave 1's two evidence files");
        let r = rollup(&goal(), Filter::Evidence, Some("w2"));
        assert_eq!((r.wave.as_deref(), r.filter), (Some("w2"), Filter::Evidence), "neither choice resets the other");
        assert_eq!(shape(&r), vec![(Some("Wave 2".into()), 1, vec![("T3".into(), 1)])]);
        assert_eq!(r.head_count(), "1 of 6");
        assert_eq!(rollup(&goal(), Filter::All, Some("w9")).wave.as_deref(), Some("w2"), "a wave that's gone falls back to the default");
    }

    #[test]
    fn a_kind_narrows_every_wave_and_the_menu_dims_the_waves_it_empties() {
        let r = rollup(&goal(), Filter::Docs, Some(ALL_WAVES));
        assert_eq!((r.goal.len(), r.shown_waves().len()), (1, 0));
        assert_eq!(r.waves.iter().map(WaveGroup::count).collect::<Vec<_>>(), [0, 0, 0], "listed, with nothing to show");
        assert_eq!(r.head_count(), "1 of 6");
        let r = rollup(&goal(), Filter::Evidence, Some(ALL_WAVES));
        assert!(r.goal.is_empty());
        assert_eq!(shape(&r), vec![(Some("Wave 1 · Grouping".into()), 2, vec![("T1".into(), 2)]), (Some("Wave 2".into()), 1, vec![("T3".into(), 1)])]);
    }

    #[test]
    fn a_kind_the_shown_files_dont_hold_falls_back_to_all() {
        let mut g = goal();
        g["attachments"] = json!([]);
        let r = rollup(&g, Filter::Design, Some(ALL_WAVES));
        assert_eq!((r.filter, r.shown()), (Filter::All, 4));
    }

    #[test]
    fn without_waves_tasks_stand_on_their_own_with_no_picker_and_one_kind_needs_no_menu() {
        let g = json!({
            "attachments": [],
            "task_attachments": [att("evidence", "Shot", "~/a.png", Some("T1"))],
            "waves": [],
            "tasks": [{"ref": "T1", "title": "One", "wave": null}],
        });
        let r = rollup(&g, Filter::All, None);
        assert_eq!(shape(&r), vec![(None, 1, vec![("T1".into(), 1)])]);
        assert!(!r.kinds, "every file is evidence");
        assert!(!r.picker());
        assert_eq!(rollup(&json!({"attachments": [], "task_attachments": [], "tasks": [], "waves": []}), Filter::All, None).total, 0);
    }

    #[test]
    fn images_and_videos_get_a_preview_with_the_file_name() {
        let r = rollup(&goal(), Filter::All, Some(ALL_WAVES));
        let t1 = &r.waves[0].tasks[0].files;
        assert!(matches!(&t1[0].preview, Some(Preview::Image(p)) if p.ends_with("/before.png") && !p.starts_with('~')));
        assert_eq!((t1[0].icon, t1[0].detail.as_str()), ("image", "before.png"));
        assert_eq!((t1[1].preview.clone(), t1[1].icon), (Some(Preview::Video), "video"));
        assert_eq!(r.goal[1].preview, None);
    }

    #[test]
    fn the_added_time_is_twelve_hour() {
        let a = json!({"kind": "evidence", "title": "Shot", "url": "~/a.png", "at": "2026-10-17T22:42:00Z"});
        let f = file(&a);
        assert!(f.detail.ends_with(" AM") || f.detail.ends_with(" PM"), "{}", f.detail);
    }
}
