//! The Days page (`GET /days`): one day's timeline and its week, so you can see what every
//! project's agents did while you were on something else, and go back to any earlier day.
//!
//! The view model (`View::of`) is built from the board's answer alone and holds every number and
//! label; `render` only lays it out.
use crate::app::MainWindow;
use crate::fmt::{self, arr, s};
use crate::theme::{Theme, ThemeMode};
use crate::ui::kit;
use chrono::{Datelike, Duration, NaiveDate};
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};

const PREF_HIDDEN: &str = "taskboard.days.hidden";
const SLOTS: usize = 144;

/// The page's own state: the day shown (`None`: today) and the projects hidden from it.
#[derive(Clone, Debug)]
pub struct State {
    pub date: Option<NaiveDate>,
    pub hidden: Vec<String>,
}

impl Default for State {
    fn default() -> Self {
        let hidden = crate::prefs::get(PREF_HIDDEN)
            .and_then(|v| v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()))
            .unwrap_or_default();
        State { date: None, hidden }
    }
}

impl State {
    pub fn query(&self) -> Vec<(&'static str, String)> {
        let mut q = vec![("hide", self.hidden.join(","))];
        if let Some(d) = self.date {
            q.push(("date", d.format("%Y-%m-%d").to_string()));
        }
        q
    }
}

// ------------------------------------------------------------------ numbers

fn f(v: &Value, k: &str) -> f64 {
    v[k].as_f64().unwrap_or(0.0)
}

/// Minutes as "41h 17m", "45m", "3h".
pub fn dur(min: f64) -> String {
    let m = min.max(0.0).round() as i64;
    if m < 60 {
        format!("{m}m")
    } else if m % 60 == 0 {
        format!("{}h", m / 60)
    } else {
        format!("{}h {:02}m", m / 60, m % 60)
    }
}

/// Minutes as whole hours: "105h".
fn hours(min: f64) -> String {
    format!("{}h", (min / 60.0).round() as i64)
}

fn signed(x: f64, f: impl Fn(f64) -> String) -> String {
    if x >= 0.0 { format!("+{}", f(x)) } else { format!("−{}", f(-x)) }
}

fn count(x: f64) -> String {
    let r = (x * 10.0).round() / 10.0;
    if r.fract() == 0.0 { format!("{}", r as i64) } else { format!("{r}") }
}

/// "3:12 PM" in the reader's zone.
fn clock(iso: &str) -> String {
    fmt::parse(iso).map(|t| fmt::local(t).format("%-I:%M %p").to_string()).unwrap_or_default()
}

fn hour_label(h: i64) -> String {
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{h12} {}", if h % 24 < 12 { "AM" } else { "PM" })
}

fn date_of(v: &Value, k: &str) -> Option<NaiveDate> {
    NaiveDate::parse_from_str(s(v, k), "%Y-%m-%d").ok()
}

/// The project palette, in a fixed order checked for colour-blind separation in both modes. It
/// leaves out the greens, oranges and reds the page uses for done, waiting and lost. A project
/// keeps its slot (its place among all the board's projects) whatever is hidden; past four, grey.
pub fn project_color(mode: ThemeMode, index: usize) -> Hsla {
    const LIGHT: [u32; 4] = [0x2a78d6, 0xeda100, 0x4a3aa7, 0xe87ba4];
    const DARK: [u32; 4] = [0x3987e5, 0xc98500, 0x9085e9, 0xd55181];
    let pal = if mode == ThemeMode::Dark { DARK } else { LIGHT };
    rgb(pal.get(index).copied().unwrap_or(0x8a95a3)).into()
}

// ------------------------------------------------------------------ view model

#[derive(Clone, Debug, PartialEq)]
pub struct Kpi {
    pub label: &'static str,
    pub value: String,
    pub delta: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Col {
    pub date: NaiveDate,
    pub dow: String,
    pub num: u32,
    pub done: String,
    pub selected: bool,
    pub future: bool,
    pub tip: String,
    /// (project index, minutes), bottom first.
    pub segs: Vec<(usize, f64)>,
    pub total: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Bar {
    pub task: String,
    /// Bars that overlap in time go on separate rows of their lane.
    pub row: usize,
    pub label: String,
    pub kind: String,
    pub x0: f32,
    pub x1: f32,
    pub tip: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub kind: String,
    pub x: f32,
    pub tip: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    pub project: String,
    pub color: usize,
    pub rows: usize,
    pub bars: Vec<Bar>,
    pub marks: Vec<Mark>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TaskRow {
    pub task: String,
    pub title: String,
    pub color: usize,
    pub work: f64,
    pub wait: f64,
    pub total: String,
    pub state: String,
    pub tip: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub total: String,
    pub vs_last: String,
    pub breakdown: String,
    /// Running totals in minutes: this week up to its last day with data, and all of last week.
    pub this: Vec<f64>,
    pub last: Vec<f64>,
    pub selected: Option<usize>,
    pub any: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Wait {
    pub min: String,
    pub task: String,
    pub text: String,
    pub x: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub title: String,
    pub live: Option<String>,
    pub date: NaiveDate,
    pub today: NaiveDate,
    pub oldest: Option<NaiveDate>,
    /// (name, palette slot, shown).
    pub projects: Vec<(String, usize, bool)>,
    pub kpis: Vec<Kpi>,
    pub week_label: String,
    pub cols: Vec<Col>,
    pub y_max: f64,
    pub from_hour: i64,
    pub to_hour: i64,
    pub lanes: Vec<Lane>,
    pub now_x: Option<f32>,
    /// Terminals working in each ten-minute slot of the shown hours (`None`: still to come).
    pub running: Vec<Option<i64>>,
    pub peak: i64,
    pub tasks: Vec<TaskRow>,
    pub task_median: String,
    pub task_median_x: f32,
    pub task_max: f64,
    pub notes: Vec<(String, String)>,
    pub saved: Saved,
    pub wait_total: String,
    pub wait_sub: String,
    pub waits: Vec<Wait>,
    pub wait_usual_x: Option<f32>,
    pub done_week: String,
    pub done_sub: String,
    /// (day letter, this week, last week, selected).
    pub done_bars: Vec<(String, f64, f64, bool)>,
    /// (project, palette slot, minutes, share).
    pub split: Vec<(String, usize, f64, String)>,
}

impl View {
    pub fn of(v: &Value) -> Option<View> {
        let date = date_of(v, "date")?;
        let today = date_of(v, "today").unwrap_or(date);
        let day = &v["day"];
        let usual = &v["usual"];
        let all: Vec<String> = arr(v, "projects").iter().filter_map(|p| p.as_str().map(String::from)).collect();
        let hidden: Vec<String> = arr(v, "hidden").iter().filter_map(|p| p.as_str().map(String::from)).collect();
        let slot = |p: &str| all.iter().position(|x| x == p).unwrap_or(usize::MAX);
        let projects = all.iter().enumerate().map(|(i, p)| (p.clone(), i, !hidden.contains(p))).collect();
        let is_today = date == today;
        let start = fmt::parse(s(v, "day_start"));

        // Key numbers, against the median of the two weeks before.
        let have_usual = f(usual, "days") > 0.0;
        let vs = |cur: f64, k: &str, f: fn(f64) -> String| if have_usual { format!("{} vs usual", signed(cur - self::f(usual, k), f)) } else { String::new() };
        let (agent, human, est) = (f(day, "agent_min"), f(day, "human_min"), f(day, "est_agent_min"));
        let kpis = vec![
            Kpi { label: "Agent time", value: dur(agent), delta: vs(agent, "agent_min", dur) },
            Kpi {
                label: "Human estimate",
                value: if human > 0.0 { dur(human) } else { "—".into() },
                delta: if human > 0.0 && est > 0.0 { format!("{:.1}× agent time", human / est) } else { String::new() },
            },
            Kpi { label: "Tasks done", value: count(f(day, "done")), delta: vs(f(day, "done"), "done", count) },
            Kpi { label: "PRs opened", value: count(f(day, "prs")), delta: vs(f(day, "prs"), "prs", count) },
            Kpi { label: "Waiting on you", value: dur(f(day, "wait_min")), delta: vs(f(day, "wait_min"), "wait_min", dur) },
        ];

        // The week, from the first weekday the daemon starts it on (config.toml's first_weekday).
        let week = arr(v, "week");
        let last_week = arr(v, "last_week");
        let cols: Vec<Col> = week
            .iter()
            .filter_map(|w| {
                let d = date_of(w, "date")?;
                let mut segs: Vec<(usize, f64)> = w["by_project"].as_object().into_iter().flatten().map(|(p, m)| (slot(p), m.as_f64().unwrap_or(0.0))).filter(|(_, m)| *m > 0.0).collect();
                segs.sort_by_key(|(i, _)| *i);
                let total = f(w, "agent_min");
                let done = f(w, "done");
                Some(Col {
                    date: d,
                    dow: d.format("%a").to_string(),
                    num: d.day(),
                    done: if done > 0.0 { format!("{} done", count(done)) } else { String::new() },
                    selected: d == date,
                    future: w["future"].as_bool().unwrap_or(false),
                    tip: format!("{}: {} agent time{}", d.format("%A, %B %-d"), dur(total), if done > 0.0 { format!(", {} done", count(done)) } else { String::new() }),
                    segs,
                    total,
                })
            })
            .collect();
        let y_max = {
            let top = cols.iter().map(|c| c.total / 60.0).fold(0.0, f64::max);
            ((top / 4.0).ceil() * 4.0).max(4.0)
        };
        let week_label = match (cols.first(), cols.last()) {
            (Some(a), Some(b)) => format!("{} – {}", a.date.format("%b %-d"), b.date.format("%b %-d")),
            _ => String::new(),
        };

        // The timeline.
        let (from_hour, to_hour) = (v["from_hour"].as_i64().unwrap_or(9), v["to_hour"].as_i64().unwrap_or(17).max(v["from_hour"].as_i64().unwrap_or(9) + 1));
        let span = ((to_hour - from_hour) * 3600) as f64;
        let x_of = |iso: &str| -> Option<f32> {
            let (t, s0) = (fmt::parse(iso)?, start?);
            Some((((t - s0).num_seconds() as f64 - (from_hour * 3600) as f64) / span).clamp(0.0, 1.0) as f32)
        };
        let lanes: Vec<Lane> = arr(day, "lanes")
            .iter()
            .map(|l| {
                let project = s(l, "project").to_string();
                let bars = arr(l, "bars")
                    .iter()
                    .filter_map(|b| {
                        let (x0, x1) = (x_of(s(b, "from"))?, x_of(s(b, "to"))?);
                        let kind = s(b, "kind").to_string();
                        let what = match kind.as_str() {
                            "done" => "done",
                            "failed" => "failed",
                            "lost" => "terminal lost",
                            "stopped" => "stopped",
                            "needs" => "waiting on you",
                            _ => "working",
                        };
                        Some(Bar {
                            row: 0,
                            task: s(b, "ref").to_string(),
                            label: format!("{} {}", s(b, "ref"), s(b, "title")),
                            tip: format!("{} {} · {} to {} · {what}", s(b, "ref"), s(b, "title"), clock(s(b, "from")), if b["open"].as_bool() == Some(true) && is_today { "now".into() } else { clock(s(b, "to")) }),
                            kind,
                            x0,
                            x1: x1.max(x0 + 0.002),
                        })
                    })
                    .collect();
                let (bars, rows) = stack(bars);
                let marks = arr(l, "marks")
                    .iter()
                    .filter_map(|mk| {
                        let x = x_of(s(mk, "at"))?;
                        Some(Mark { kind: s(mk, "kind").to_string(), x, tip: format!("{} · {} · {}", clock(s(mk, "at")), s(mk, "ref"), s(mk, "text")) })
                    })
                    .collect();
                Lane { color: slot(&project), project, rows, bars, marks }
            })
            .collect();
        let now_x = if is_today { x_of(s(v, "now")) } else { None };
        let run = arr(day, "running");
        let now_slot = if is_today {
            match (fmt::parse(s(v, "now")), start) {
                (Some(n), Some(s0)) => ((n - s0).num_seconds() / 600) as usize,
                _ => SLOTS,
            }
        } else {
            SLOTS
        };
        let running = ((from_hour * 6) as usize..(to_hour * 6) as usize).map(|k| (k <= now_slot).then(|| run.get(k).and_then(|x| x.as_i64()).unwrap_or(0))).collect();
        let peak = day["peak"].as_i64().unwrap_or(0);

        // How long tasks took this week: working, then waiting on you.
        let mut tasks: Vec<TaskRow> = arr(v, "week_tasks")
            .iter()
            .take(8)
            .map(|t| {
                let (work, wait) = (f(t, "work_min"), f(t, "wait_min"));
                let st = s(t, "state");
                let live = date_of(t, "date") == Some(today) && st != "done" && st != "failed";
                TaskRow {
                    task: s(t, "ref").to_string(),
                    title: s(t, "title").to_string(),
                    color: slot(s(t, "project")),
                    work,
                    wait,
                    total: dur(work + wait),
                    state: match st {
                        "needs" if live => "needs you".into(),
                        "lost" => "lost".into(),
                        "failed" => "failed".into(),
                        _ if live => "so far".into(),
                        _ => String::new(),
                    },
                    tip: format!("{} {} · {} · working {}{}", s(t, "ref"), s(t, "title"), s(t, "project"), dur(work), if wait > 0.0 { format!(", waiting on you {}", dur(wait)) } else { String::new() }),
                }
            })
            .collect();
        let task_max = tasks.iter().map(|t| t.work + t.wait).fold(60.0, f64::max) * 1.05;
        tasks.retain(|t| t.work + t.wait > 0.0);
        let med = f(v, "week_task_median");

        // Notable.
        let mut notes = Vec::new();
        let waits_json = arr(day, "waits");
        if let Some(w) = waits_json.iter().max_by(|a, b| f(a, "min").total_cmp(&f(b, "min"))) {
            notes.push((dur(f(w, "min")), format!("longest wait on you · {}", s(w, "ref"))));
        }
        if let Some(t) = arr(day, "tasks").iter().max_by(|a, b| (f(a, "work_min") + f(a, "wait_min")).total_cmp(&(f(b, "work_min") + f(b, "wait_min")))) {
            let m = f(t, "work_min") + f(t, "wait_min");
            if m > 0.0 {
                notes.push((dur(m), format!("longest task · {}", s(t, "ref"))));
            }
        }
        if peak > 1 {
            let first = day["peak_first"].as_i64().unwrap_or(0);
            let at = start.map(|s0| fmt::local(s0 + Duration::minutes(first * 10)).format("%-I:%M %p").to_string()).unwrap_or_default();
            notes.push((format!("{peak} at once"), format!("{} in all, first at {at}", dur(f(day, "peak_slots") * 10.0))));
        }
        let hourly: Vec<i64> = arr(day, "hourly").iter().map(|x| x.as_i64().unwrap_or(0)).collect();
        if let Some((h, n)) = hourly.iter().enumerate().max_by_key(|(i, n)| (**n, -(*i as i64))).filter(|(_, n)| **n > 0) {
            notes.push((hour_label(h as i64), format!("busiest hour · {}", fmt::plural(*n, "event", "events"))));
        }

        // Hours saved: the human estimate less the agents' time on the same tasks, added up
        // through the week, against the week before.
        let saved_of = |w: &Value| (f(w, "human_min") - f(w, "est_agent_min")).max(0.0);
        let mut this = Vec::new();
        let mut last = Vec::new();
        let (mut a, mut b) = (0.0, 0.0);
        for (k, w) in week.iter().enumerate() {
            b += last_week.get(k).map(saved_of).unwrap_or(0.0);
            last.push(b);
            if !w["future"].as_bool().unwrap_or(false) {
                a += saved_of(w);
                this.push(a);
            }
        }
        let (hw, aw) = (week.iter().map(|w| f(w, "human_min")).sum::<f64>(), week.iter().map(|w| f(w, "est_agent_min")).sum::<f64>());
        let any = hw > 0.0 || last_week.iter().any(|w| f(w, "human_min") > 0.0);
        let at_k = this.len().saturating_sub(1);
        let saved = Saved {
            total: hours(a),
            vs_last: if this.is_empty() { String::new() } else { format!("{} on last week", signed(a - last.get(at_k).copied().unwrap_or(0.0), hours)) },
            breakdown: if hw > 0.0 { format!("{} human estimate − {} agent{}", hours(hw), hours(aw), if aw > 0.0 { format!(" · {:.1}×", hw / aw) } else { String::new() }) } else { String::new() },
            selected: cols.iter().position(|c| c.selected),
            this,
            last,
            any,
        };

        // Waiting on you.
        let wait_each: Vec<f64> = waits_json.iter().map(|w| f(w, "min")).collect();
        let usual_each = f(usual, "wait_each");
        let mut waits: Vec<Wait> = waits_json
            .iter()
            .map(|w| Wait { min: dur(f(w, "min")), task: s(w, "ref").to_string(), text: if s(w, "text").is_empty() { s(w, "title").to_string() } else { s(w, "text").to_string() }, x: (f(w, "min").min(60.0) / 60.0) as f32 })
            .collect();
        waits.sort_by(|a, b| b.x.total_cmp(&a.x));
        let mid = {
            let mut e = wait_each.clone();
            e.sort_by(f64::total_cmp);
            if e.is_empty() { 0.0 } else if e.len() % 2 == 1 { e[e.len() / 2] } else { (e[e.len() / 2 - 1] + e[e.len() / 2]) / 2.0 }
        };
        let wait_sub = match (wait_each.is_empty(), usual_each > 0.0) {
            (true, _) => String::new(),
            (false, true) => format!("median {} · usual {}", dur(mid), dur(usual_each)),
            (false, false) => format!("median {}", dur(mid)),
        };

        // Tasks done.
        let (dn, dl) = (week.iter().map(|w| f(w, "done")).sum::<f64>(), last_week.iter().map(|w| f(w, "done")).sum::<f64>());
        let (pn, pl) = (week.iter().map(|w| f(w, "prs")).sum::<f64>(), last_week.iter().map(|w| f(w, "prs")).sum::<f64>());
        let done_bars = week
            .iter()
            .enumerate()
            .map(|(k, w)| {
                let d = date_of(w, "date");
                (d.map(|d| d.format("%a").to_string()[..2].to_string()).unwrap_or_default(), f(w, "done"), last_week.get(k).map(|l| f(l, "done")).unwrap_or(0.0), d == Some(date))
            })
            .collect();

        // Agent time by project.
        let mut split: Vec<(String, usize, f64, String)> = day["by_project"]
            .as_object()
            .into_iter()
            .flatten()
            .map(|(p, m)| (p.clone(), slot(p), m.as_f64().unwrap_or(0.0)))
            .filter(|(_, _, m)| *m > 0.0)
            .map(|(p, i, m)| (p, i, m, format!("{}%", (m / agent.max(1.0) * 100.0).round() as i64)))
            .collect();
        split.sort_by_key(|x| x.1);

        Some(View {
            title: if is_today { "Today".into() } else { date.format("%A, %B %-d").to_string() },
            live: is_today.then(|| clock(s(v, "now"))),
            date,
            today,
            oldest: date_of(v, "oldest"),
            projects,
            kpis,
            week_label,
            cols,
            y_max,
            from_hour,
            to_hour,
            lanes,
            now_x,
            running,
            peak,
            tasks,
            task_median: if med > 0.0 { dur(med) } else { String::new() },
            task_median_x: (med / task_max) as f32,
            task_max,
            notes,
            saved,
            wait_total: dur(f(day, "wait_min")),
            wait_sub,
            waits,
            wait_usual_x: (usual_each > 0.0).then(|| (usual_each.min(60.0) / 60.0) as f32),
            done_week: count(dn),
            done_sub: format!("last week {} · {} PRs, last week {}", count(dl), count(pn), count(pl)),
            done_bars,
            split,
        })
    }
}

/// Give each bar the first row it fits on (a task keeps its row while it goes on); how many rows.
fn stack(mut bars: Vec<Bar>) -> (Vec<Bar>, usize) {
    bars.sort_by(|a, b| a.x0.total_cmp(&b.x0));
    let mut ends: Vec<(f32, String)> = Vec::new();
    for b in &mut bars {
        let row = ends
            .iter()
            .position(|(end, task)| *task == b.task && *end <= b.x0 + 0.001)
            .or_else(|| ends.iter().position(|(end, _)| *end <= b.x0 + 0.001))
            .unwrap_or(ends.len());
        if row == ends.len() {
            ends.push((b.x1, b.task.clone()));
        } else {
            ends[row] = (b.x1, b.task.clone());
        }
        b.row = row;
    }
    (bars, ends.len().max(1))
}

// ------------------------------------------------------------------ actions

fn set_date(m: &mut MainWindow, d: Option<NaiveDate>, cx: &mut Context<MainWindow>) {
    let today = m.data.days.as_ref().and_then(|v| date_of(v, "today"));
    let d = d.filter(|d| Some(*d) != today);
    if m.days.date != d {
        m.days.date = d;
        m.data.days = None;
        m.refresh(cx);
    }
}

fn toggle(m: &mut MainWindow, p: &str, all: &[String], cx: &mut Context<MainWindow>) {
    let mut h = m.days.hidden.clone();
    if let Some(i) = h.iter().position(|x| x == p) {
        h.remove(i);
    } else if all.iter().filter(|x| !h.contains(x)).count() > 1 {
        h.push(p.to_string());
    } else {
        return;
    }
    crate::prefs::set(PREF_HIDDEN, json!(h));
    m.days.hidden = h;
    m.refresh(cx);
}

// ------------------------------------------------------------------ render

fn card(t: &Theme) -> Div {
    div().flex().flex_col().gap(px(10.)).p(px(14.)).bg(t.card).border_1().border_color(t.border).rounded(px(12.))
}

fn h2(text: impl Into<SharedString>) -> Div {
    div().text_size(px(13.5)).font_weight(FontWeight::SEMIBOLD).child(text.into())
}

fn swatch(c: Hsla, size: f32) -> Div {
    div().flex_none().w(px(size)).h(px(size)).rounded(px(2.)).bg(c)
}

fn legend_item(t: &Theme, mark: Div, label: impl Into<SharedString>) -> Div {
    div().flex().items_center().gap(px(6.)).text_size(px(12.)).text_color(t.text_2).child(mark).child(label.into())
}

/// The fill and text of a timeline bar.
fn bar_colors(t: &Theme, kind: &str) -> (Hsla, Hsla) {
    match kind {
        "done" => (t.up_soft, t.up_fg),
        "needs" => (t.warn_soft, t.warn_fg),
        "lost" | "failed" => (t.down_soft, t.down),
        "stopped" => (t.seg, t.muted),
        _ => (t.accent_soft, t.accent_fg),
    }
}

/// A timeline mark: colour and shape (circle, square or ring) so it never rests on colour alone.
fn mark_el(t: &Theme, kind: &str, size: f32) -> Div {
    let d = div().flex_none().w(px(size)).h(px(size));
    match kind {
        "commit" => d.rounded_full().bg(t.faint),
        "question" => d.rounded(px(1.)).bg(t.warn),
        "pr" => d.rounded_full().border_2().border_color(t.text_2),
        "done" => d.rounded_full().bg(t.up),
        _ => d.rounded(px(1.)).bg(t.results),
    }
}

const MARKS: &[(&str, &str)] = &[("commit", "Commit"), ("question", "Question"), ("pr", "PR"), ("done", "Done"), ("found", "Issue found")];

pub fn render(m: &mut MainWindow, _window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    let shell = div().id("days-page").flex().flex_col().flex_1().min_w_0().size_full().overflow_y_scroll();
    let Some(v) = m.data.days.as_ref().and_then(View::of) else {
        let msg = m.data.errs.days.clone().unwrap_or_else(|| "Loading…".into());
        return shell.child(div().p(px(28.)).child(kit::empty(&t, msg))).into_any_element();
    };
    let mode = t.mode;
    let pc = move |i: usize| project_color(mode, i);

    // ---- header
    let all: Vec<String> = v.projects.iter().map(|(p, _, _)| p.clone()).collect();
    let toggles = div().flex().flex_wrap().gap(px(6.)).children(v.projects.iter().map(|(p, i, on)| {
        let (p2, all2, on) = (p.clone(), all.clone(), *on);
        let c = pc(*i);
        div()
            .id(SharedString::from(format!("days-proj-{p}")))
            .flex()
            .items_center()
            .gap(px(7.))
            .h(px(30.))
            .px(px(11.))
            .rounded_full()
            .border_1()
            .border_color(t.border_2)
            .cursor_pointer()
            .text_size(px(12.5))
            .text_color(if on { t.text } else { t.faint })
            .when(on, |d| d.bg(t.panel_2))
            .child(div().w(px(10.)).h(px(10.)).rounded(px(3.)).border_2().border_color(c).when(on, |d| d.bg(c)))
            .child(p.clone())
            .tooltip(kit::tip(if on { format!("Hide {p}") } else { format!("Show {p}") }))
            .on_click(cx.listener(move |m, _, _, cx| toggle(m, &p2, &all2, cx)))
    }));
    let (prev, next) = (v.date - Duration::days(1), v.date + Duration::days(1));
    let at_oldest = v.oldest.is_some_and(|o| v.date <= o);
    let at_today = v.date >= v.today;
    let nav = div()
        .flex()
        .items_center()
        .gap(px(6.))
        .child(kit::btn(&t, "days-prev", "‹").w(px(36.)).when(at_oldest, kit::disabled).tooltip(kit::tip("Earlier day")).on_click(cx.listener(move |m, _, _, cx| {
            if !at_oldest {
                set_date(m, Some(prev), cx)
            }
        })))
        .child(kit::btn(&t, "days-next", "›").w(px(36.)).when(at_today, kit::disabled).tooltip(kit::tip("Later day")).on_click(cx.listener(move |m, _, _, cx| {
            if !at_today {
                set_date(m, Some(next), cx)
            }
        })))
        .child(kit::btn(&t, "days-today", "Today").when(at_today, kit::disabled).on_click(cx.listener(|m, _, _, cx| set_date(m, None, cx))));
    let header = div()
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(14.))
        .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child(v.title.clone()))
        .children(v.live.clone().map(|c| div().flex().items_center().gap(px(6.)).text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).text_color(t.up_fg).child(kit::dot(t.up, 8.)).child(c)))
        .child(div().flex_1())
        .child(toggles)
        .child(nav);

    // ---- key numbers
    let kpis = div().flex().gap(px(10.)).children(v.kpis.iter().map(|k| {
        card(&t)
            .flex_1()
            .min_w_0()
            .gap(px(2.))
            .py(px(10.))
            .child(div().text_size(px(12.)).text_color(t.muted).child(k.label))
            .child(div().text_size(px(22.)).font_weight(FontWeight::BOLD).child(k.value.clone()))
            .child(div().text_size(px(12.)).text_color(t.text_2).min_h(px(16.)).child(k.delta.clone()))
    }));

    let left = div()
        .flex()
        .flex_col()
        .gap(px(16.))
        .flex_1()
        .min_w(px(560.))
        .child(week_card(&v, &t, &pc, cx))
        .child(timeline_card(&v, &t, &pc, cx))
        .child(tasks_card(&v, &t, &pc, cx));
    let rail = div()
        .flex()
        .flex_col()
        .gap(px(12.))
        .w(px(300.))
        .flex_none()
        .children((!v.notes.is_empty()).then(|| notes_card(&v, &t)))
        .child(saved_card(&v, &t))
        .child(waits_card(&v, &t))
        .child(done_card(&v, &t))
        .children((!v.split.is_empty()).then(|| split_card(&v, &t, &pc)));

    shell
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(16.))
                .w_full()
                .max_w(px(1400.))
                .px(px(28.))
                .py(px(22.))
                .child(header)
                .child(kpis)
                .child(div().flex().items_start().gap(px(16.)).child(left).child(rail)),
        )
        .into_any_element()
}

fn project_legend(v: &View, t: &Theme, pc: &impl Fn(usize) -> Hsla) -> Div {
    div().flex().flex_wrap().gap(px(14.)).children(v.projects.iter().filter(|(_, _, on)| *on).map(|(p, i, _)| legend_item(t, swatch(pc(*i), 9.), p.clone())))
}

fn week_card(v: &View, t: &Theme, pc: &impl Fn(usize) -> Hsla, cx: &mut Context<MainWindow>) -> Div {
    const H: f32 = 170.;
    let ticks: Vec<f64> = (0..=4).map(|k| v.y_max / 4.0 * k as f64).collect();
    let axis = div().relative().w(px(30.)).h(px(H)).flex_none().children(ticks.iter().map(|y| {
        div()
            .absolute()
            .right(px(4.))
            .bottom(px((y / v.y_max) as f32 * H - 7.))
            .text_size(px(11.))
            .text_color(t.muted)
            .child(if *y == 0.0 { "0".to_string() } else { format!("{}h", *y as i64) })
    }));
    let grid = ticks.iter().map(|y| div().absolute().left_0().right_0().bottom(px((y / v.y_max) as f32 * H)).h(px(1.)).bg(t.divider)).collect::<Vec<_>>();
    let columns = div().absolute().inset_0().flex().gap(px(10.)).children(v.cols.iter().map(|c| {
        let d = c.date;
        let n = c.segs.len();
        let stack = div().flex().flex_col_reverse().gap(px(2.)).w_full().children(c.segs.iter().enumerate().map(|(k, (i, min))| {
            let h = ((min / 60.0 / v.y_max) as f32 * H).max(2.);
            div().h(px(h)).bg(pc(*i)).when(k + 1 == n, |d| d.rounded_t(px(4.)))
        }));
        div()
            .id(SharedString::from(format!("days-col-{}", c.num)))
            .flex_1()
            .h(px(H))
            .flex()
            .flex_col()
            .justify_end()
            .px(px(10.))
            .rounded_t(px(6.))
            .when(c.selected, |x| x.bg(t.accent_soft))
            .when(!c.future, |x| x.cursor_pointer())
            .child(stack)
            .tooltip(kit::tip(c.tip.clone()))
            .when(!c.future, |x| x.on_click(cx.listener(move |m, _, _, cx| set_date(m, Some(d), cx))))
    }));
    let tiles = div().flex().gap(px(10.)).children(v.cols.iter().map(|c| {
        let d = c.date;
        div()
            .id(SharedString::from(format!("days-tile-{}", c.num)))
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .py(px(6.))
            .rounded(px(8.))
            .border_1()
            .border_color(if c.selected { t.accent } else { t.border })
            .when(c.selected, |x| x.border_2().bg(t.accent_soft))
            .when(c.future, |x| x.text_color(t.faint))
            .when(!c.future, |x| x.cursor_pointer().on_click(cx.listener(move |m, _, _, cx| set_date(m, Some(d), cx))))
            .child(div().text_size(px(11.)).text_color(t.muted).child(c.dow.clone()))
            .child(div().text_size(px(15.)).font_weight(FontWeight::BOLD).child(c.num.to_string()))
            .child(div().text_size(px(11.)).text_color(t.text_2).min_h(px(15.)).child(c.done.clone()))
    }));
    card(t)
        .child(div().flex().flex_wrap().items_center().justify_between().gap(px(12.)).child(h2(format!("Agent time · {}", v.week_label))).child(project_legend(v, t, pc)))
        .child(
            div()
                .flex()
                .gap(px(8.))
                .child(axis)
                .child(div().flex().flex_col().gap(px(6.)).flex_1().min_w_0().child(div().relative().h(px(H)).children(grid).child(columns)).child(tiles)),
        )
}

fn timeline_card(v: &View, t: &Theme, pc: &impl Fn(usize) -> Hsla, cx: &mut Context<MainWindow>) -> Div {
    const LABEL: f32 = 104.;
    let hours: Vec<i64> = (v.from_hour..=v.to_hour).collect();
    let span = (v.to_hour - v.from_hour) as f32;
    let axis = div().flex().child(div().w(px(LABEL)).flex_none()).child(div().relative().flex_1().h(px(18.)).children(hours.iter().map(|h| {
        let x = (*h - v.from_hour) as f32 / span;
        div().absolute().left(relative(x)).ml(px(-16.)).w(px(32.)).flex().justify_center().text_size(px(11.)).text_color(t.muted).child(hour_label(*h))
    })));
    let gridlines = |h: f32| {
        hours.iter().map(move |hh| (*hh - v.from_hour) as f32 / span).map(move |x| div().absolute().top_0().h(px(h)).left(relative(x)).w(px(1.))).collect::<Vec<_>>()
    };
    let lanes = v.lanes.iter().map(|l| {
        const ROW: f32 = 32.;
        let lane_h = l.rows as f32 * ROW + 26.;
        let bars = l.bars.iter().map(|b| {
            let (bg, fg) = bar_colors(t, &b.kind);
            let r = b.task.clone();
            div()
                .id(SharedString::from(format!("days-bar-{}-{}", b.task, (b.x0 * 10000.) as i64)))
                .absolute()
                .top(px(8. + b.row as f32 * ROW))
                .h(px(26.))
                .left(relative(b.x0))
                .w(relative(b.x1 - b.x0))
                .px(px(7.))
                .flex()
                .items_center()
                .rounded(px(6.))
                .bg(bg)
                .text_color(fg)
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .cursor_pointer()
                .child(b.label.clone())
                .tooltip(kit::tip(b.tip.clone()))
                .on_click(cx.listener(move |m, _, _, cx| m.open_task(r.clone(), cx)))
        });
        let marks = l.marks.iter().enumerate().map(|(k, mk)| {
            div()
                .id(SharedString::from(format!("days-mark-{}-{k}", l.project)))
                .absolute()
                .top(px(lane_h - 18.))
                .left(relative(mk.x))
                .ml(px(-6.))
                .w(px(12.))
                .h(px(12.))
                .flex()
                .items_center()
                .justify_center()
                .child(mark_el(t, &mk.kind, 8.))
                .tooltip(kit::tip(mk.tip.clone()))
        });
        let lines = gridlines(lane_h).into_iter().map(|g| g.bg(t.divider));
        div()
            .flex()
            .border_t_1()
            .border_color(t.divider)
            .child(div().w(px(LABEL)).flex_none().flex().items_start().pt(px(12.)).gap(px(8.)).h(px(lane_h)).font_weight(FontWeight::SEMIBOLD).text_size(px(13.)).child(swatch(pc(l.color), 8.)).child(div().truncate().child(l.project.clone())))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .h(px(lane_h))
                    .children(lines)
                    .children(bars)
                    .children(marks)
                    .children(v.now_x.map(|x| div().absolute().top_0().bottom_0().left(relative(x)).w(px(2.)).bg(t.accent))),
            )
    });
    let peak = v.peak.max(3) as f32;
    let running = div()
        .flex()
        .border_t_1()
        .border_color(t.divider)
        .pt(px(8.))
        .child(
            div()
                .w(px(LABEL))
                .flex_none()
                .flex()
                .flex_col()
                .text_size(px(12.))
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(t.text_2).child("Running"))
                .child(div().text_color(t.muted).child(format!("peak {}", v.peak))),
        )
        .child(div().flex_1().flex().items_end().gap(px(1.)).h(px(40.)).border_b_1().border_color(t.border_2).children(v.running.iter().enumerate().map(|(k, n)| {
            let n = n.unwrap_or(0);
            let at = (v.from_hour * 60 + k as i64 * 10) as f64;
            let label = format!("{}: {}", hour_clock(at), if v.running[k].is_none() { "still to come".to_string() } else { fmt::plural(n, "terminal working", "terminals working") });
            div()
                .id(SharedString::from(format!("days-run-{k}")))
                .flex_1()
                .h(px((n as f32 / peak * 38.).max(0.)))
                .rounded_t(px(2.))
                .bg(if n == v.peak && n > 0 { t.accent } else { t.accent_line })
                .tooltip(kit::tip(label))
        })));
    let legend = div().flex().flex_wrap().gap(px(16.)).children(MARKS.iter().map(|(k, label)| legend_item(t, mark_el(t, k, 8.), *label)));
    let body = if v.lanes.is_empty() {
        div().py(px(28.)).flex().justify_center().text_color(t.muted).child("No agent work")
    } else {
        div().flex().flex_col().child(axis).children(lanes).child(running)
    };
    card(t).child(legend).child(body)
}

fn hour_clock(min: f64) -> String {
    let m = min.round() as i64;
    let (h, mm) = (m / 60, m % 60);
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{h12}:{mm:02} {}", if h % 24 < 12 { "AM" } else { "PM" })
}

fn tasks_card(v: &View, t: &Theme, pc: &impl Fn(usize) -> Hsla, cx: &mut Context<MainWindow>) -> Div {
    let rows = v.tasks.iter().map(|r| {
        let (work, wait) = ((r.work / v.task_max) as f32, (r.wait / v.task_max) as f32);
        let task = r.task.clone();
        let state_c = if r.state == "needs you" { t.warn_fg } else { t.muted };
        div()
            .id(SharedString::from(format!("days-task-{}", r.task)))
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(6.))
            .py(px(4.))
            .rounded(px(6.))
            .cursor_pointer()
            .hover(|s| s.bg(t.panel_2))
            .child(
                div()
                    .w(px(230.))
                    .flex_none()
                    .flex()
                    .gap(px(6.))
                    .items_baseline()
                    .text_size(px(13.))
                    .overflow_hidden()
                    .child(div().flex_none().font_family(t.mono_font.clone()).text_size(px(11.)).text_color(t.muted).child(r.task.clone()))
                    .child(div().truncate().child(r.title.clone())),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .h(px(14.))
                    .child(div().absolute().top_0().bottom_0().left_0().w(relative(work)).rounded_l(px(3.)).when(wait == 0., |d| d.rounded_r(px(3.))).bg(pc(r.color)))
                    .child(div().absolute().top_0().bottom_0().left(relative(work)).w(relative(wait)).rounded_r(px(3.)).bg(t.warn))
                    .when(v.task_median_x > 0., |d| d.child(div().absolute().top(px(-3.)).bottom(px(-3.)).left(relative(v.task_median_x)).w(px(2.)).bg(t.text))),
            )
            .child(div().w(px(64.)).flex_none().flex().justify_end().text_size(px(13.)).font_weight(FontWeight::SEMIBOLD).child(r.total.clone()))
            .child(div().w(px(70.)).flex_none().text_size(px(12.)).text_color(state_c).child(r.state.clone()))
            .tooltip(kit::tip(r.tip.clone()))
            .on_click(cx.listener(move |m, _, _, cx| m.open_task(task.clone(), cx)))
    });
    let legend = div()
        .flex()
        .flex_wrap()
        .gap(px(14.))
        .child(legend_item(t, div().flex().gap(px(1.)).children(v.projects.iter().filter(|p| p.2).take(3).map(|(_, i, _)| div().w(px(5.)).h(px(9.)).rounded(px(1.)).bg(pc(*i)))), "agent working"))
        .child(legend_item(t, swatch(t.warn, 9.), "waiting on you"))
        .children((!v.task_median.is_empty()).then(|| legend_item(t, div().w(px(2.)).h(px(12.)).bg(t.text), format!("median {}", v.task_median))));
    let body = if v.tasks.is_empty() { div().text_color(t.muted).child("No tasks this week") } else { div().flex().flex_col().gap(px(2.)).children(rows) };
    card(t).child(div().flex().flex_wrap().items_center().justify_between().gap(px(12.)).child(h2(format!("How long tasks took · {}", v.week_label))).child(legend)).child(body)
}

fn notes_card(v: &View, t: &Theme) -> Div {
    card(t).child(h2("Notable")).children(v.notes.iter().map(|(lead, text)| {
        div()
            .flex()
            .gap(px(10.))
            .items_baseline()
            .text_size(px(13.))
            .child(div().min_w(px(76.)).font_weight(FontWeight::BOLD).child(lead.clone()))
            .child(div().text_color(t.text_2).child(text.clone()))
    }))
}

fn saved_card(v: &View, t: &Theme) -> Div {
    let s = v.saved.clone();
    if !s.any {
        return card(t).child(h2("Hours saved")).child(div().text_size(px(13.)).text_color(t.muted).child("No human estimates yet"));
    }
    let (line, dash, grid, sel) = (t.accent, t.faint, t.divider, s.selected);
    let top = {
        let mx = s.this.iter().chain(s.last.iter()).copied().fold(0.0, f64::max) / 60.0;
        ((mx / 20.0).ceil() * 20.0).max(20.0)
    };
    let (this, last) = (s.this.clone(), s.last.clone());
    let chart = canvas(
        |_, _, _| {},
        move |b, _, window, _| {
            let (w, h) = (b.size.width.as_f32(), b.size.height.as_f32());
            let x = |k: usize| b.origin.x + px(6. + (w - 12.) * k as f32 / 6.);
            let y = |m: f64| b.origin.y + px(h - 4. - (h - 8.) * (m / 60.0 / top) as f32);
            for g in [0.0, top / 2.0 * 60.0, top * 60.0] {
                let mut p = PathBuilder::stroke(px(1.));
                p.move_to(point(b.origin.x, y(g)));
                p.line_to(point(b.origin.x + px(w), y(g)));
                if let Ok(p) = p.build() {
                    window.paint_path(p, grid);
                }
            }
            let path = |pts: &[f64], width: f32, dashed: bool| {
                let mut p = PathBuilder::stroke(px(width));
                if dashed {
                    p = p.dash_array(&[px(4.), px(3.)]);
                }
                for (k, m) in pts.iter().enumerate() {
                    if k == 0 {
                        p.move_to(point(x(k), y(*m)));
                    } else {
                        p.line_to(point(x(k), y(*m)));
                    }
                }
                p.build().ok()
            };
            if last.len() > 1 {
                if let Some(p) = path(&last, 2., true) {
                    window.paint_path(p, dash);
                }
            }
            if this.len() > 1 {
                if let Some(p) = path(&this, 2., false) {
                    window.paint_path(p, line);
                }
            }
            for (k, m) in this.iter().enumerate() {
                let r = if Some(k) == sel { 4.5 } else { 3. };
                let c = point(x(k), y(*m));
                let mut p = PathBuilder::fill();
                let n = 16;
                for i in 0..=n {
                    let a = std::f32::consts::TAU * i as f32 / n as f32;
                    let q = point(c.x + px(r * a.cos()), c.y + px(r * a.sin()));
                    if i == 0 {
                        p.move_to(q);
                    } else {
                        p.line_to(q);
                    }
                }
                if let Ok(p) = p.build() {
                    window.paint_path(p, line);
                }
            }
        },
    )
    .w_full()
    .h(px(110.));
    let days = ["Mo", "Tu", "We", "Th", "Fr", "Sa", "Su"];
    card(t)
        .gap(px(8.))
        .child(h2("Hours saved"))
        .child(div().flex().items_baseline().gap(px(8.)).child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child(s.total.clone())).child(div().text_size(px(13.)).text_color(t.text_2).child(s.vs_last.clone())))
        .children((!s.breakdown.is_empty()).then(|| div().text_size(px(12.)).text_color(t.muted).child(s.breakdown.clone())))
        .child(div().flex().gap(px(6.)).child(div().flex().flex_col().justify_between().h(px(110.)).text_size(px(10.)).text_color(t.muted).child(format!("{}h", top as i64)).child(format!("{}h", (top / 2.0) as i64)).child("0")).child(chart))
        .child(div().flex().justify_between().pl(px(26.)).children(days.iter().enumerate().map(|(k, d)| div().text_size(px(11.)).text_color(if Some(k) == sel { t.text } else { t.muted }).when(Some(k) == sel, |x| x.font_weight(FontWeight::BOLD)).child(*d))))
        .child(div().flex().gap(px(12.)).child(legend_item(t, div().w(px(14.)).h(px(2.)).bg(t.accent), "this week")).child(legend_item(t, div().w(px(14.)).h(px(2.)).bg(t.faint), "last week")))
}

fn waits_card(v: &View, t: &Theme) -> Div {
    let strip = div()
        .relative()
        .h(px(22.))
        .border_b_1()
        .border_color(t.border_2)
        .children(v.waits.iter().enumerate().map(|(k, w)| {
            div()
                .id(SharedString::from(format!("days-wait-{k}")))
                .absolute()
                .bottom(px(4.))
                .left(relative(w.x))
                .ml(px(-5.))
                .w(px(10.))
                .h(px(10.))
                .rounded(px(2.))
                .bg(t.warn)
                .border_2()
                .border_color(t.card)
                .tooltip(kit::tip(format!("{} · {}", w.task, w.min)))
        }))
        .children(v.wait_usual_x.map(|x| div().absolute().top_0().bottom_0().left(relative(x)).w(px(2.)).bg(t.text_2)));
    card(t)
        .gap(px(8.))
        .child(div().flex().justify_between().items_baseline().gap(px(8.)).child(h2("Waiting on you")).child(div().text_size(px(12.)).text_color(t.muted).child(v.wait_sub.clone())))
        .child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child(v.wait_total.clone()))
        .child(strip)
        .child(div().flex().justify_between().text_size(px(11.)).text_color(t.muted).child("0").child("30m").child("60m+"))
        .children(v.waits.iter().map(|w| {
            div()
                .flex()
                .gap(px(8.))
                .text_size(px(12.))
                .child(div().w(px(48.)).flex_none().font_weight(FontWeight::BOLD).child(w.min.clone()))
                .child(div().flex_none().font_family(t.mono_font.clone()).text_size(px(11.)).text_color(t.muted).child(w.task.clone()))
                .child(div().text_color(t.text_2).truncate().child(w.text.clone()))
        }))
}

fn done_card(v: &View, t: &Theme) -> Div {
    let max = v.done_bars.iter().map(|(_, a, b, _)| a.max(*b)).fold(1.0, f64::max);
    card(t)
        .gap(px(8.))
        .child(h2("Tasks done this week"))
        .child(div().flex().items_baseline().gap(px(8.)).child(div().text_size(px(24.)).font_weight(FontWeight::BOLD).child(v.done_week.clone())).child(div().text_size(px(12.)).text_color(t.muted).child(v.done_sub.clone())))
        .child(div().flex().items_end().gap(px(6.)).h(px(44.)).border_b_1().border_color(t.border_2).children(v.done_bars.iter().enumerate().map(|(k, (d, now, last, sel))| {
            let (h, lh) = ((now / max) as f32 * 40., (last / max) as f32 * 40.);
            div()
                .id(SharedString::from(format!("days-done-{k}")))
                .relative()
                .flex_1()
                .h(px(44.))
                .rounded_t(px(3.))
                .when(*sel, |x| x.bg(t.accent_soft))
                .child(div().absolute().left(px(3.)).right(px(3.)).bottom_0().h(px(h)).rounded_t(px(2.)).bg(t.accent))
                .when(*last > 0., |x| x.child(div().absolute().left_0().right_0().bottom(px(lh - 1.)).h(px(2.)).bg(t.text)))
                .tooltip(kit::tip(format!("{d} · this week {} · last week {}", count(*now), count(*last))))
        })))
        .child(div().flex().gap(px(6.)).children(v.done_bars.iter().map(|(d, ..)| div().flex_1().flex().justify_center().text_size(px(11.)).text_color(t.muted).child(d.clone()))))
        .child(div().flex().gap(px(12.)).child(legend_item(t, swatch(t.accent, 9.), "this week")).child(legend_item(t, div().w(px(12.)).h(px(2.)).bg(t.text), "last week")))
}

fn split_card(v: &View, t: &Theme, pc: &impl Fn(usize) -> Hsla) -> Div {
    let total: f64 = v.split.iter().map(|x| x.2).sum::<f64>().max(1.0);
    card(t)
        .child(h2("Agent time by project"))
        .child(div().flex().gap(px(2.)).h(px(12.)).children(v.split.iter().map(|(_, i, m, _)| div().w(relative((m / total) as f32)).rounded(px(3.)).bg(pc(*i)))))
        .children(v.split.iter().map(|(p, i, m, share)| {
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_size(px(13.))
                .child(swatch(pc(*i), 9.))
                .child(div().flex_1().truncate().child(p.clone()))
                .child(div().text_color(t.text_2).child(dur(*m)))
                .child(div().w(px(40.)).flex().justify_end().text_color(t.muted).child(share.clone()))
        }))
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that brings GPUI's `test` attribute.
    use super::{View, dur, hour_label};
    use serde_json::{Value, json};

    fn sample() -> Value {
        let day = |d: &str, agent: f64, done: f64, human: f64, est: f64, future: bool| {
            json!({"date": d, "agent_min": agent, "done": done, "prs": 1.0, "human_min": human, "est_agent_min": est, "future": future,
                   "by_project": {"webapp": agent * 0.75, "api": agent * 0.25}})
        };
        let week = vec![
            day("2026-10-05", 850.0, 7.0, 600.0, 200.0, false),
            day("2026-10-06", 484.0, 5.0, 0.0, 0.0, false),
            day("2026-10-07", 437.0, 5.0, 0.0, 0.0, false),
            day("2026-10-08", 706.0, 3.0, 300.0, 100.0, false),
            day("2026-10-09", 0.0, 0.0, 0.0, 0.0, true),
            day("2026-10-10", 0.0, 0.0, 0.0, 0.0, true),
            day("2026-10-11", 0.0, 0.0, 0.0, 0.0, true),
        ];
        let last_week = vec![
            day("2026-09-28", 500.0, 4.0, 300.0, 100.0, false),
            day("2026-09-29", 500.0, 4.0, 0.0, 0.0, false),
            day("2026-09-30", 500.0, 4.0, 0.0, 0.0, false),
            day("2026-10-01", 500.0, 4.0, 0.0, 0.0, false),
            day("2026-10-02", 500.0, 4.0, 120.0, 60.0, false),
            day("2026-10-03", 0.0, 0.0, 0.0, 0.0, false),
            day("2026-10-04", 0.0, 0.0, 0.0, 0.0, false),
        ];
        let mut hourly = vec![0; 24];
        hourly[10] = 8;
        hourly[11] = 3;
        let waits = json!([{"ref": "T241", "min": 58.0, "text": "Per project or per terminal?", "open": true}, {"ref": "T243", "min": 6.0, "text": "Resolved threads?"}]);
        let bar = json!({"ref": "T238", "title": "Fix scrollback", "from": "2026-10-08T09:14:00Z", "to": "2026-10-08T11:02:00Z", "kind": "done"});
        let mark = json!({"at": "2026-10-08T10:15:00Z", "kind": "commit", "ref": "T238", "text": "Committed: x"});
        let lanes = json!([{"project": "webapp", "bars": [bar], "marks": [mark]}]);
        let mut d = json!({"agent_min": 706.0, "human_min": 300.0, "est_agent_min": 100.0, "done": 3.0, "prs": 3.0, "commits": 9.0, "questions": 2.0,
                           "wait_min": 64.0, "peak": 3, "peak_slots": 5, "peak_first": 60});
        d["by_project"] = json!({"webapp": 500.0, "api": 206.0});
        d["running"] = json!(vec![0; 144]);
        d["hourly"] = json!(hourly);
        d["waits"] = waits;
        d["tasks"] = json!([{"ref": "T241", "work_min": 184.0, "wait_min": 58.0}]);
        d["lanes"] = lanes;
        let tasks = json!([{"ref": "T241", "title": "Retry cap", "project": "webapp", "work_min": 184.0, "wait_min": 58.0, "state": "needs", "date": "2026-10-08"},
                           {"ref": "T238", "title": "Scrollback", "project": "webapp", "work_min": 108.0, "wait_min": 0.0, "state": "done", "date": "2026-10-08"}]);
        let mut v = json!({"date": "2026-10-08", "today": "2026-10-08", "is_today": true, "now": "2026-10-08T15:12:00Z", "day_start": "2026-10-08T00:00:00Z",
                           "from_hour": 9, "to_hour": 17, "projects": ["api", "webapp"], "hidden": [], "oldest": "2026-09-01", "week_task_median": 108.0});
        v["day"] = d;
        v["week"] = json!(week);
        v["last_week"] = json!(last_week);
        v["week_tasks"] = tasks;
        v["usual"] = json!({"days": 10, "agent_min": 450.0, "done": 4.0, "prs": 3.0, "wait_min": 13.0, "wait_each": 18.0});
        v
    }

    #[::core::prelude::v1::test]
    fn the_view_reads_the_day() {
        let v = View::of(&sample()).unwrap();
        assert_eq!(v.title, "Today");
        assert_eq!(v.kpis[0].value, "11h 46m");
        assert_eq!(v.kpis[0].delta, "+4h 16m vs usual");
        assert_eq!(v.kpis[1].delta, "3.0× agent time");
        assert_eq!(v.kpis[2].delta, "−1 vs usual");
        assert_eq!(v.cols.len(), 7);
        assert!(v.cols[3].selected && v.cols[4].future);
        assert_eq!(v.cols[0].done, "7 done");
        assert_eq!(v.y_max, 16.0);
        assert_eq!(v.lanes[0].bars[0].kind, "done");
        assert!((v.lanes[0].bars[0].x0 - 0.0292).abs() < 0.001, "9:14 is just after 9");
        assert_eq!(v.running.len(), 48);
        assert_eq!(v.running[47], None, "after now is still to come");
        assert_eq!(v.tasks[0].state, "needs you");
        assert_eq!(v.tasks[0].total, "4h 02m");
        assert_eq!(v.notes[0], ("58m".to_string(), "longest wait on you · T241".to_string()));
        assert!(v.notes.iter().any(|n| n.0 == "10 AM"));
        assert!(v.notes.iter().any(|n| n.0 == "3 at once"));
    }

    #[::core::prelude::v1::test]
    fn hours_saved_runs_through_the_week() {
        let s = View::of(&sample()).unwrap().saved;
        assert_eq!(s.this, vec![400.0, 400.0, 400.0, 600.0]);
        assert_eq!(s.last.len(), 7);
        assert_eq!(s.total, "10h");
        assert_eq!(s.vs_last, "+7h on last week", "600 against 200 by Thursday");
        assert_eq!(s.breakdown, "15h human estimate − 5h agent · 3.0×");
        assert_eq!(s.selected, Some(3));
    }

    #[::core::prelude::v1::test]
    fn overlapping_bars_take_their_own_rows() {
        let bar = |task: &str, x0: f32, x1: f32| super::Bar { task: task.into(), row: 0, label: String::new(), kind: "working".into(), x0, x1, tip: String::new() };
        let (bars, rows) = super::stack(vec![bar("T1", 0.1, 0.3), bar("T2", 0.2, 0.4), bar("T1", 0.3, 0.35), bar("T3", 0.36, 0.5)]);
        let at: Vec<(String, usize)> = bars.iter().map(|b| (b.task.clone(), b.row)).collect();
        assert_eq!(rows, 2);
        assert_eq!(at, vec![("T1".into(), 0), ("T2".into(), 1), ("T1".into(), 0), ("T3".into(), 0)]);
    }

    #[::core::prelude::v1::test]
    fn durations_read_in_hours() {
        assert_eq!(dur(2477.0), "41h 17m");
        assert_eq!(dur(45.0), "45m");
        assert_eq!(dur(180.0), "3h");
        assert_eq!(hour_label(12), "12 PM");
        assert_eq!(hour_label(9), "9 AM");
    }
}
