//! The goal page's Attached card: the goal's own attachments, then every task's, grouped by wave
//! and task. Waves and tasks fold (view state, kept per goal); chips narrow it to one kind of
//! file; hovering an image or a video shows a larger preview.
use super::*;
use crate::ui::task_panel::{ATT_KINDS, att_file_type, att_icon, chevron};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Filter {
    #[default]
    All,
    Evidence,
    Design,
    Docs,
}

const FILTERS: [(Filter, &str); 4] = [(Filter::All, "All"), (Filter::Evidence, "Evidence"), (Filter::Design, "Design"), (Filter::Docs, "Docs")];

/// The chip an attachment's kind counts under; `other` only shows under All.
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

#[derive(Clone, Debug, PartialEq)]
pub struct WaveGroup {
    /// The fold key: `w2`, or `loose` for the tasks with no wave.
    pub key: String,
    /// None when the goal has no waves: its tasks stand on their own.
    pub label: Option<String>,
    pub tasks: Vec<TaskGroup>,
}

impl WaveGroup {
    pub fn count(&self) -> usize {
        self.tasks.iter().map(|t| t.files.len()).sum()
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rollup {
    pub goal: Vec<File>,
    pub waves: Vec<WaveGroup>,
    /// Every attachment, whatever the filter.
    pub total: usize,
    /// How many each chip holds, in `FILTERS` order.
    pub counts: [usize; 4],
    /// The filter in force: the one asked for, or All when it holds nothing.
    pub filter: Filter,
}

impl Rollup {
    pub fn shown(&self) -> usize {
        self.goal.len() + self.waves.iter().map(WaveGroup::count).sum::<usize>()
    }

    /// Chips only help when some kind holds part of the list, not all of it.
    pub fn chips(&self) -> bool {
        self.counts[1..].iter().any(|&n| n > 0 && n < self.total)
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

/// The card's lists under `filter`: the goal's own files, then each wave's tasks that hold any.
pub fn rollup(g: &Value, filter: Filter) -> Rollup {
    let own = arr(g, "attachments");
    let on_tasks = arr(g, "task_attachments");
    let kinds = own.iter().chain(on_tasks.iter()).map(|a| s(a, "kind"));
    let mut counts = [0; 4];
    for k in kinds {
        counts[0] += 1;
        if let Some(b) = bucket(k) {
            counts[FILTERS.iter().position(|(f, _)| *f == b).unwrap()] += 1;
        }
    }
    let filter = if counts[FILTERS.iter().position(|(f, _)| *f == filter).unwrap()] == 0 { Filter::All } else { filter };
    let pick = |list: &[Value], r: Option<&str>| -> Vec<File> {
        list.iter().filter(|a| fmt::opt_s(a, "task") == r && passes(filter, s(a, "kind"))).map(file).collect()
    };
    let goal = pick(own, None);
    let tasks = arr(g, "tasks");
    let group = |t: &Value| -> Option<TaskGroup> {
        let r = s(t, "ref");
        let files = pick(on_tasks, Some(r));
        (!files.is_empty()).then(|| TaskGroup { r: r.to_string(), title: s(t, "title").to_string(), files })
    };
    let ws = arr(g, "waves");
    let mut waves = vec![];
    for w in ws {
        let n = i(w, "wave");
        let label = match fmt::opt_s(w, "name") {
            Some(name) => format!("Wave {n} · {name}"),
            None => format!("Wave {n}"),
        };
        waves.push(WaveGroup { key: format!("w{n}"), label: Some(label), tasks: tasks.iter().filter(|t| t["wave"] == w["wave"]).filter_map(group).collect() });
    }
    let loose = tasks.iter().filter(|t| ws.is_empty() || t["wave"].is_null()).filter_map(group).collect();
    waves.push(WaveGroup { key: "loose".into(), label: (!ws.is_empty()).then(|| "No wave".into()), tasks: loose });
    waves.retain(|w| !w.tasks.is_empty());
    Rollup { goal, waves, total: counts[0], counts, filter }
}

// ------------------------------------------------------------------ view

fn chip(t: &Theme, id: String, label: String, on: bool) -> Stateful<Div> {
    let hover = t.tint;
    div()
        .id(SharedString::from(id))
        .flex_none()
        .px(px(9.))
        .py(px(3.))
        .rounded_full()
        .border_1()
        .text_size(px(12.))
        .cursor_pointer()
        .when(on, |d| d.bg(t.accent_soft).border_color(t.accent_line).text_color(t.accent_fg).font_weight(FontWeight::MEDIUM))
        .when(!on, |d| d.bg(t.card).border_color(t.border).text_color(t.text_2).hover(move |s| s.bg(hover)))
        .child(label)
}

/// A fold's header row: chevron, what it is, its count on the right; the whole row toggles.
fn toggle(t: &Theme, gr: &str, key: &str, open: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let tint = t.tint;
    let (gr, k) = (gr.to_string(), key.to_string());
    div()
        .id(SharedString::from(format!("goal-att-fold-{key}")))
        .flex()
        .items_center()
        .gap(px(6.))
        .mx(px(-6.))
        .px(px(6.))
        .py(px(4.))
        .rounded(px(6.))
        .cursor_pointer()
        .hover(move |s| s.bg(tint))
        .child(chevron(open, t.muted))
        .on_click(cx.listener(move |m, _, _, cx| {
            let shut = m.goal_page.att_closed.entry(gr.clone()).or_default();
            if !shut.remove(&k) {
                shut.insert(k.clone());
            }
            cx.notify();
        }))
}

fn count(t: &Theme, n: usize) -> Div {
    div().ml_auto().flex_none().pl(px(8.)).text_size(px(12.)).text_color(t.muted).child(n.to_string())
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
        .py(px(3.))
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
    div().flex().flex_col().gap(px(2.)).children(list.iter().enumerate().map(|(ix, f)| file_row(t, f, format!("goal-att-{key}-{ix}"))))
}

/// How many list items the Attached section takes: its head (the count, chips and the goal's own
/// files) and one per wave group; none when nothing's attached.
pub(super) fn parts(r: &Rollup) -> usize {
    if r.total == 0 { 0 } else { 1 + r.waves.len() }
}

/// Piece `k` of the Attached section (`parts`) of goal `g`, rolled up as `r`: 0 is its head, then
/// each wave group.
pub(super) fn attached_piece(m: &MainWindow, t: &Theme, g: &Value, r: &Rollup, k: usize, cx: &mut Context<MainWindow>) -> Div {
    let last = k == r.waves.len();
    let gr = s(g, "ref").to_string();
    let shut = m.goal_page.att_closed.get(&gr).cloned().unwrap_or_default();
    let mut out = div().flex().flex_col().gap(px(10.));
    let Some(w) = k.checked_sub(1).and_then(|k| r.waves.get(k)) else {
        if r.chips() {
            let mut row = div().flex().flex_wrap().gap(px(6.));
            for (ix, (f, label)) in FILTERS.iter().enumerate() {
                let n = r.counts[ix];
                if n == 0 {
                    continue;
                }
                let f = *f;
                row = row.child(chip(t, format!("goal-att-chip-{ix}"), format!("{label} {n}"), r.filter == f).on_click(cx.listener(move |m, _, _, cx| {
                    m.goal_page.att_filter = f;
                    cx.notify();
                })));
            }
            out = out.child(row);
        }
        if !r.goal.is_empty() {
            let label = div().pb(px(2.)).text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(t.muted).child("GOAL");
            out = out.child(div().flex().flex_col().gap(px(4.)).child(label).child(files(t, &r.goal, "goal")));
        }
        let n = if r.filter == Filter::All { r.total.to_string() } else { format!("{} of {}", r.shown(), r.total) };
        return side_piece(t, true, last, 0., side_head(t, "Attached", Some(n)).pb(px(8.))).child(out);
    };
    let open = !shut.contains(&w.key);
    let mut tasks = div().flex().flex_col().gap(px(6.));
    for tg in &w.tasks {
        let topen = !shut.contains(&tg.r);
        let r2 = tg.r.clone();
        let head = toggle(t, &gr, &tg.r, topen, cx)
            .child(
                div()
                    .id(SharedString::from(format!("goal-att-ref-{}", tg.r)))
                    .flex_none()
                    .text_size(px(11.))
                    .font_family(t.mono_font.clone())
                    .text_color(t.muted)
                    .cursor_pointer()
                    .hover(|s| s.underline())
                    .child(tg.r.clone())
                    .on_click(cx.listener(move |m, _, _, cx| {
                        cx.stop_propagation();
                        m.open_task(r2.clone(), cx);
                    })),
            )
            .child(div().min_w_0().truncate().text_size(px(12.)).font_weight(FontWeight::MEDIUM).text_color(t.text_2).child(tg.title.clone()))
            .child(count(t, tg.files.len()));
        tasks = tasks.child(div().flex().flex_col().gap(px(2.)).child(head).when(topen, |d| d.child(files(t, &tg.files, &tg.r).pl(px(16.)))));
    }
    let body = match &w.label {
        Some(label) => {
            let head = toggle(t, &gr, &w.key, open, cx).child(div().text_size(px(13.)).font_weight(FontWeight::BOLD).text_color(t.text).child(label.clone())).child(count(t, w.count()));
            div().flex().flex_col().gap(px(6.)).pt(px(10.)).border_t_1().border_color(t.divider).child(head).when(open, |d| d.child(tasks.pl(px(18.))))
        }
        None => tasks,
    };
    // The head's column kept a 10px gap before its first group, none when it had nothing under it.
    let gap = if k == 1 && !r.chips() && r.goal.is_empty() { 0. } else { 10. };
    side_piece(t, false, last, gap, body)
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
        let waves = json!([{"wave": 1, "name": "Grouping"}, {"wave": 2}]);
        json!({"ref": "G1", "attachments": own, "task_attachments": on_tasks, "waves": waves, "tasks": tasks})
    }

    fn shape(r: &Rollup) -> Vec<(Option<String>, usize, Vec<(String, usize)>)> {
        r.waves.iter().map(|w| (w.label.clone(), w.count(), w.tasks.iter().map(|t| (t.r.clone(), t.files.len())).collect())).collect()
    }

    #[test]
    fn groups_by_wave_then_task_and_skips_empty_ones() {
        let r = rollup(&goal(), Filter::All);
        assert_eq!(r.goal.iter().map(|f| f.title.as_str()).collect::<Vec<_>>(), ["Concepts", "Rules"]);
        assert_eq!(
            shape(&r),
            vec![
                (Some("Wave 1 · Grouping".into()), 2, vec![("T1".into(), 2)]),
                (Some("Wave 2".into()), 1, vec![("T3".into(), 1)]),
                (Some("No wave".into()), 1, vec![("T4".into(), 1)]),
            ],
            "T2 has nothing attached, so it has no group"
        );
        assert_eq!((r.total, r.shown(), r.counts), (6, 6, [6, 3, 1, 1]));
        assert!(r.chips());
    }

    #[test]
    fn a_filter_narrows_every_group_and_drops_the_ones_it_empties() {
        let r = rollup(&goal(), Filter::Evidence);
        assert!(r.goal.is_empty());
        assert_eq!(shape(&r), vec![(Some("Wave 1 · Grouping".into()), 2, vec![("T1".into(), 2)]), (Some("Wave 2".into()), 1, vec![("T3".into(), 1)])]);
        assert_eq!((r.total, r.shown()), (6, 3));
        let d = rollup(&goal(), Filter::Docs);
        assert_eq!((d.goal.len(), d.waves.len()), (1, 0));
    }

    #[test]
    fn a_filter_that_holds_nothing_falls_back_to_all() {
        let mut g = goal();
        g["attachments"] = json!([]);
        let r = rollup(&g, Filter::Design);
        assert_eq!((r.filter, r.shown()), (Filter::All, 4));
    }

    #[test]
    fn without_waves_tasks_stand_on_their_own_and_one_kind_needs_no_chips() {
        let g = json!({
            "attachments": [],
            "task_attachments": [att("evidence", "Shot", "~/a.png", Some("T1"))],
            "waves": [],
            "tasks": [{"ref": "T1", "title": "One", "wave": null}],
        });
        let r = rollup(&g, Filter::All);
        assert_eq!(shape(&r), vec![(None, 1, vec![("T1".into(), 1)])]);
        assert!(!r.chips(), "every file is evidence");
        assert_eq!(rollup(&json!({"attachments": [], "task_attachments": [], "tasks": [], "waves": []}), Filter::All).total, 0);
    }

    #[test]
    fn images_and_videos_get_a_preview_with_the_file_name() {
        let r = rollup(&goal(), Filter::All);
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
