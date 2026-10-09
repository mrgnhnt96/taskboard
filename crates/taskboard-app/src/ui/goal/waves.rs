//! The goal page's wave rail (the web's `waves.js`): one node per wave down a line, each wave's header
//! (its state, counts and review stop), the gate under a wave the goal stopped at ("Continue to wave
//! 2") or that's held, its tasks when open with the owner's "Stop after this wave for my review", the tasks with no wave ("Post"), then tasks of other goals that also finish
//! this one branching in, and where the goal finishes. Views are pure (`rail_view`) and checked
//! against the web's own output in `goal::tests::views_match_web` (`waveRail`).
use super::*;

/// A task on the rail (`waveTaskLine` + its facts).
pub struct WTask {
    pub r: String,
    pub title: String,
    pub planned: bool,
    pub jira: Option<(String, String)>,
    pub pr_text: Option<String>,
    pub pr_label: Option<String>,
    pub pr_phase: String,
    pub when: Option<String>,
    /// (`.wchip` class, text): warn, bad, quiet, term.
    pub facts: Vec<(&'static str, String)>,
    /// The terminal it runs in (the `term` fact opens it on the Sessions page).
    pub term: Option<String>,
}

pub struct Gate {
    pub cls: &'static str,
    pub text: String,
    pub button: Option<(&'static str, String)>,
    pub wave: i64,
}

pub enum Item {
    Wave {
        wave: i64,
        state: String,
        open: bool,
        name: Option<String>,
        count: Vec<(String, String)>,
        stop: bool,
        gate: Option<Gate>,
        tasks: Vec<WTask>,
        /// The owner's "Stop after this wave for my review" checkbox (None once the wave is done or stopped).
        stop_box: Option<bool>,
    },
    Post { open: bool, tasks: Vec<WTask> },
    Ref { done: bool, badge: String, home: Option<String>, landed: bool, task: WTask },
    Finish { done: bool, text: String },
}

fn opened(open: &HashMap<String, bool>, g: &Value, key: &str, default: bool) -> bool {
    open.get(&format!("{}:{key}", fmt::ref_of(g, "G"))).copied().unwrap_or(default)
}

/// `waveOpen(g, w)`: running, stopped, failed and held waves start open.
fn wave_open(open: &HashMap<String, bool>, g: &Value, w: &Value) -> bool {
    opened(open, g, &num_text(&w["wave"]), matches!(s(w, "state"), "running" | "stopped" | "failed" | "held"))
}

/// `PR_STAGE_CLS`.
fn stage_cls(phase: &str) -> &'static str {
    match phase {
        "checks" => "st-queued",
        "fix" => "st-failed",
        "review" | "rereview" => "st-review",
        "comments" => "st-needs",
        "merge" => "st-working",
        "merged" => "st-done",
        "declined" => "neutral",
        _ => "st-working",
    }
}

/// `prStatus(t)`: (label, class).
fn pr_status(t: &Value) -> (String, &'static str) {
    if pr_stopped(t) {
        return ("Needs you".into(), "st-needs");
    }
    let st = &t["pr"]["stage"];
    if st.is_object() {
        return (s(st, "label").to_string(), stage_cls(s(st, "phase")));
    }
    ("Awaiting merge".into(), "st-working")
}

/// `waveCount(w, tasks)`: the header's pills.
fn wave_count(w: &Value, tasks: &[&Value]) -> Vec<(String, String)> {
    let one = |label: &str, cls: &str| vec![(cls.to_string(), label.to_string())];
    match s(w, "state") {
        "stopped" => return one("Waiting for your review", "st-needs"),
        "failed" => return one("A task failed", "st-failed"),
        "held" => return one("Held", "st-needs"),
        st if s(w, "held_by") == "stopped" && st != "done" => return one("Stopped for your review", "st-needs"),
        _ => {}
    }
    let mut counts: Vec<(String, &'static str, usize)> = vec![];
    let mut add = |label: String, cls: &'static str| match counts.iter_mut().find(|c| c.0 == label) {
        Some(c) => c.2 += 1,
        None => counts.push((label, cls, 1)),
    };
    let starting = i(w, "starting").max(0);
    let running = i(w, "active") - starting;
    for _ in 0..running.max(0) {
        add("Running".into(), "st-working");
    }
    for _ in 0..starting {
        add("Starting".into(), "st-queued");
    }
    for _ in tasks.iter().filter(|t| s(t, "status") == "queued" && b(t, "blocked")) {
        add("Blocked".into(), "st-blocked");
    }
    for t in tasks.iter().filter(|t| awaiting_merge(t)) {
        let (label, cls) = pr_status(t);
        add(label, cls);
    }
    for _ in tasks.iter().filter(|t| s(t, "status") == "done" && !b(t, "failed") && !awaiting_merge(t)) {
        add("Done".into(), "st-done");
    }
    if counts.is_empty() {
        return if s(w, "state") == "done" { one("Done", "st-done") } else { vec![] };
    }
    counts.into_iter().map(|(label, cls, n)| (cls.to_string(), if n > 1 { format!("{label} ×{n}") } else { label })).collect()
}

fn is_live(t: &Value) -> bool {
    matches!(s(t, "status"), "working" | "needs")
}

/// `waveTaskLine` and `attentionFacts`.
fn wtask(t: &Value) -> WTask {
    let key = status_key(t);
    let mut facts = vec![];
    if key == "needs" {
        facts.push(("warn", if fmt::opt_s(t, "question").is_some() { "Asked you something" } else { "Needs you" }.to_string()));
    }
    if key == "failed" {
        facts.push(("bad", "Failed".into()));
    }
    if b(t, "lost") {
        facts.push(("warn", "Terminal lost".into()));
    }
    if let Some(c) = fmt::compacting(t) {
        facts.push(("compact", c));
    }
    let term = (fmt::opt_s(t, "who").is_some() && fmt::opt_s(t, "session_id").is_some() && !matches!(s(t, "status"), "planned" | "queued"))
        .then(|| s(t, "session_id").to_string());
    if term.is_some() {
        facts.push(("term", s(t, "who").to_string()));
    }
    let jira = t.get("jira").filter(|j| fmt::opt_s(j, "key").is_some() && fmt::opt_s(j, "url").is_some()).map(|j| {
        let u = s(j, "url");
        (s(j, "key").to_string(), if u.starts_with("http://") || u.starts_with("https://") { u.to_string() } else { "#".into() })
    });
    let stage = t["pr"]["stage"].clone();
    WTask {
        r: fmt::ref_of(t, "T"),
        title: s(t, "title").to_string(),
        planned: s(t, "status") == "planned",
        jira,
        pr_text: has_pr(t).then(|| format!("#{}", num_text(&t["pr"]["num"]))),
        pr_label: (has_pr(t) && stage.is_object()).then(|| s(&stage, "label").to_string()),
        pr_phase: if stage.is_object() { s(&stage, "phase").to_string() } else { String::new() },
        when: is_live(t).then(|| fmt::running_for(t).trim_start_matches("running ").to_string()).filter(|w| !w.is_empty()),
        facts,
        term,
    }
}

/// `planFacts(t)`: what an unfinished task waits for (planned or queued), the locks it holds and
/// whether it runs alone. The bool marks the "Waits for" fact, which the card's waiting line replaces.
pub fn plan_facts(t: &Value) -> Vec<(String, bool)> {
    if s(t, "status") == "done" {
        return vec![];
    }
    let mut out = vec![];
    let open: Vec<&str> = arr(t, "waits_for_state").iter().filter(|x| !b(x, "done")).map(|x| s(x, "ref")).collect();
    if !open.is_empty() && matches!(s(t, "status"), "planned" | "queued") {
        out.push((format!("Waits for {}", open.join(", ")), true));
    }
    for n in arr(t, "locks").iter().filter_map(|x| x.as_str()) {
        out.push((format!("Holds {n}"), false));
    }
    match t.get("alone").and_then(|a| a.as_str()) {
        Some("goal") => out.push(("Alone in the goal".into(), false)),
        Some("board") => out.push(("Alone on the board".into(), false)),
        Some(_) => out.push(("Alone".into(), false)),
        None => {}
    }
    out.extend(pool_facts(t));
    out
}

/// The devices a task has (or asks for) and its bits: "pixel-7", "Needs 2 android", "⚑ newCheckout".
pub fn pool_facts(t: &Value) -> Vec<(String, bool)> {
    let mut out = vec![];
    let lent: Vec<&str> = arr(&t["devices"], "lent").iter().filter_map(|x| x.as_str()).collect();
    if !lent.is_empty() {
        out.extend(lent.iter().map(|d| (format!("Has {d}"), false)));
    } else if let Some(n) = fmt::opt_s(&t["devices"], "needs_text") {
        out.push((format!("Needs {n}"), false));
    }
    for bit in arr(t, "bits") {
        out.push((format!("⚑ {}", s(bit, "name")), false));
    }
    out
}

/// `waveTask(t, w)`: the task line, its attention facts, and what it waits for.
fn wave_task(t: &Value, w: Option<&Value>) -> WTask {
    let mut x = wtask(t);
    if is_live(t) && fmt::opt_s(t, "when").is_some() {
        let stale = fmt::parse(s(t, "when")).map(|at| (fmt::now() - at).num_seconds() as f64 / 60. > 10.).unwrap_or(false);
        x.facts.push((if stale { "warn" } else { "quiet" }, format!("{} {}", if fmt::opt_s(t, "question").is_some() { "asked" } else { "updated" }, fmt::ago(s(t, "when")))));
    }
    let hold = w.and_then(|w| fmt::opt_s(w, "hold"));
    if s(t, "status") == "queued" && !b(t, "starting") && !(hold.is_some() && fmt::opt_s(t, "waiting") == hold) {
        let why = fmt::opt_s(t, "waiting").unwrap_or("Starts when a terminal is free");
        let warn = b(t, "blocked") || why.to_lowercase().contains("jira");
        x.facts.push((if warn { "warn" } else { "quiet" }, why.to_string()));
    }
    if s(t, "status") == "queued" && b(t, "starting") {
        x.facts.push(("quiet", "Starting".into()));
    }
    if s(t, "priority") == "high" && s(t, "status") != "done" {
        x.facts.push(("warn", "High priority".into()));
    }
    let also: Vec<&str> = arr(t, "also").iter().map(|a| s(a, "ref")).collect();
    if !also.is_empty() {
        x.facts.push(("quiet", format!("Also for {}", also.join(", "))));
    }
    let waiting = fmt::opt_s(t, "waiting").is_some();
    for (text, refs) in plan_facts(t) {
        if !(refs && waiting) {
            x.facts.push(("quiet", text));
        }
    }
    x
}

/// `landed(t)`: done, not failed, and its PR (if any) merged.
fn landed(t: &Value) -> bool {
    s(t, "status") == "done" && !b(t, "failed") && !awaiting_merge(t)
}

/// `waveGate(g, w, next)`.
fn wave_gate(w: &Value, next: Option<&Value>) -> Option<Gate> {
    let wave = i(w, "wave");
    let on = next.map(|n| format!("wave {}", num_text(&n["wave"]))).unwrap_or_else(|| "the rest of the goal".into());
    let name = format!("Wave {}{}", num_text(&w["wave"]), fmt::opt_s(w, "name").map(|n| format!(" · {n}")).unwrap_or_default());
    let failed: Vec<&str> = arr(w, "failed").iter().filter_map(|x| x.as_str()).collect();
    if s(w, "state") == "held" || (b(w, "held") && s(w, "state") == "running") {
        return Some(Gate { cls: "ask", text: format!("{name} is held."), button: Some(("wave-continue", format!("Let wave {} start", num_text(&w["wave"])))), wave });
    }
    if s(w, "state") == "stopped" {
        return Some(Gate { cls: "ask", text: format!("Stop point. {name} is done. Look it over, then let {on} start."), button: Some(("wave-continue", format!("Continue to {on}"))), wave });
    }
    if s(w, "state") == "failed" || (!failed.is_empty() && !b(w, "passed")) {
        return Some(Gate {
            cls: "bad",
            text: format!("{} failed. {} waits. Try the task again from its ⋯ menu, or go on without it.", failed.join(", "), fmt::cap(&on)),
            button: Some(("wave-continue", "Continue anyway".into())),
            wave,
        });
    }
    if b(w, "stop_after") && fmt::opt_s(w, "released_at").is_some() {
        return Some(Gate { cls: "ok", text: format!("You let the goal go on at {}.", fmt::hhmm(s(w, "released_at"))), button: None, wave });
    }
    None
}

/// `waveFinish(g, refs, wavesDone)`.
fn finish(g: &Value, refs: &[Value], waves_done: bool) -> Item {
    let waiting: Vec<&Value> = refs.iter().filter(|t| !landed(t)).collect();
    let mut whose: Vec<String> = vec![];
    for t in &waiting {
        let w = format!("{}’s", t.get("goal").filter(|g| g.is_object()).map(|g| s(g, "ref").to_string()).unwrap_or("another goal".into()));
        if !whose.contains(&w) {
            whose.push(w);
        }
    }
    let names = if whose.len() > 1 { format!("{} and {}", whose[..whose.len() - 1].join(", "), whose[whose.len() - 1]) } else { whose.first().cloned().unwrap_or_default() };
    let done = fmt::opt_s(g, "finished_at").is_some();
    let prs = arr(g, "prs_open").len();
    let text = if done {
        "Goal finished".to_string()
    } else if !waiting.is_empty() {
        format!("Goal finishes when {names} {}", if waiting.len() == 1 { "task lands" } else { "tasks land" })
    } else if waves_done && prs > 0 {
        format!("Goal finishes when {}", if prs == 1 { "its PR merges" } else { "its PRs merge" })
    } else {
        "Goal finishes after its last wave".to_string()
    };
    Item::Finish { done, text }
}

/// `waveRail(g)`.
pub fn rail_view(g: &Value, open: &HashMap<String, bool>) -> Vec<Item> {
    let tasks = arr(g, "tasks");
    let ws = arr(g, "waves");
    let mut items = vec![];
    for (k, w) in ws.iter().enumerate() {
        let mine: Vec<&Value> = tasks.iter().filter(|t| t["wave"] == w["wave"]).collect();
        items.push(Item::Wave {
            wave: i(w, "wave"),
            state: fmt::opt_s(w, "state").unwrap_or("waiting").to_string(),
            open: wave_open(open, g, w),
            name: fmt::opt_s(w, "name").map(str::to_string),
            count: wave_count(w, &mine),
            stop: b(w, "stop_after") && fmt::opt_s(w, "released_at").is_none() && s(w, "state") != "stopped",
            gate: wave_gate(w, ws.get(k + 1)),
            tasks: mine.iter().map(|t| wave_task(t, Some(w))).collect(),
            stop_box: (!matches!(s(w, "state"), "done" | "stopped")).then(|| b(w, "stop_after")),
        });
    }
    if tasks.iter().any(|t| t["wave"].is_null()) {
        items.push(Item::Post { open: opened(open, g, "none", true), tasks: tasks.iter().filter(|t| t["wave"].is_null()).map(|t| wave_task(t, None)).collect() });
    }
    let refs = arr(g, "shared");
    if !refs.is_empty() {
        let mut rail_done = ws.iter().all(|w| s(w, "state") == "done") && tasks.iter().all(|t| !t["wave"].is_null() || landed(t));
        let waves_done = rail_done;
        for t in refs {
            rail_done = rail_done && landed(t);
            let home = t.get("goal").filter(|g| g.is_object());
            items.push(Item::Ref {
                done: rail_done,
                badge: home.map(|h| s(h, "ref").to_string()).unwrap_or("G?".into()),
                home: home.map(|h| s(h, "ref").to_string()),
                landed: landed(t),
                task: wtask(t),
            });
        }
        items.push(finish(g, refs, waves_done));
    }
    items
}

// ------------------------------------------------------------------ render

const NODE: f32 = 20.;

/// `WAVE_NODE[state]`, drawn: a filled check (done), a ring with a dot (running), a ring with a
/// pause (stopped), a filled "!" (failed), an empty ring (waiting) or a ring with a flag (finish).
fn node(t: &Theme, state: &str) -> impl IntoElement {
    let (color, fill_soft) = match state {
        "done" => (t.up, None),
        "running" => (t.accent, Some(t.accent_soft)),
        "stopped" | "held" => (t.warn, Some(t.warn_soft)),
        "failed" => (t.down, None),
        _ => (t.border_2, Some(t.card)),
    };
    let card = t.card;
    let state = state.to_string();
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let k = bounds.size.width.as_f32() / 20.;
            let o = bounds.origin;
            let u = |x: f32, y: f32| point(o.x + px(x * k), o.y + px(y * k));
            let disc = |cx: f32, cy: f32, r: f32, c: Hsla, window: &mut Window| {
                let mut p = PathBuilder::fill();
                p.move_to(u(cx + r, cy));
                for n in 1..=24 {
                    let a = std::f32::consts::TAU * n as f32 / 24.;
                    p.line_to(u(cx + r * a.cos(), cy + r * a.sin()));
                }
                p.close();
                if let Ok(p) = p.build() {
                    window.paint_path(p, c);
                }
            };
            let stroke = |pts: &[(f32, f32)], w: f32, c: Hsla, window: &mut Window| {
                let mut p = PathBuilder::stroke(px(w * k));
                p.move_to(u(pts[0].0, pts[0].1));
                for (x, y) in &pts[1..] {
                    p.line_to(u(*x, *y));
                }
                if let Ok(p) = p.build() {
                    window.paint_path(p, c);
                }
            };
            match state.as_str() {
                "done" => {
                    disc(10., 10., 9., color, window);
                    stroke(&[(6., 10.3), (8.6, 12.9), (14., 7.6)], 2., card, window);
                }
                "failed" => {
                    disc(10., 10., 9., color, window);
                    stroke(&[(10., 5.5), (10., 11.)], 2., card, window);
                    disc(10., 14.2, 1.2, card, window);
                }
                _ => {
                    let r = if state == "waiting" || state == "loose" { 7. } else { 8. };
                    disc(10., 10., r + 1., color, window);
                    disc(10., 10., r - 1., fill_soft.unwrap_or(card), window);
                    match state.as_str() {
                        "running" => disc(10., 10., 3.5, color, window),
                        "stopped" | "held" => {
                            stroke(&[(8., 6.5), (8., 13.5)], 2., color, window);
                            stroke(&[(12., 6.5), (12., 13.5)], 2., color, window);
                        }
                        "finish" => {
                            stroke(&[(7.5, 14.5), (7.5, 5.5)], 1.6, color, window);
                            stroke(&[(7.5, 6.), (13., 6.), (11.6, 8.), (13., 10.), (7.5, 10.)], 1.6, color, window);
                        }
                        _ => {}
                    }
                }
            }
        },
    )
    .size(px(NODE))
}

/// `BRANCH_IN`: another goal's task curving in from the right to join the rail (dashed until it lands).
fn branch(t: &Theme, landed: bool) -> impl IntoElement {
    let color = if landed { t.up } else { t.goal_line };
    canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let o = bounds.origin;
            let u = |x: f32, y: f32| point(o.x + px(x), o.y + px(y));
            // M49 36 C49 54 10 46 10 64, sampled.
            let pts: Vec<(f32, f32)> = (0..=24)
                .map(|n| {
                    let s = n as f32 / 24.;
                    let m = 1. - s;
                    let x = m * m * m * 49. + 3. * m * m * s * 49. + 3. * m * s * s * 10. + s * s * s * 10.;
                    let y = m * m * m * 36. + 3. * m * m * s * 54. + 3. * m * s * s * 46. + s * s * s * 64.;
                    (x, y)
                })
                .collect();
            for (n, seg) in pts.windows(2).enumerate() {
                if !landed && n % 3 == 2 {
                    continue;
                }
                let mut p = PathBuilder::stroke(px(2.));
                p.move_to(u(seg[0].0, seg[0].1));
                p.line_to(u(seg[1].0, seg[1].1));
                if let Ok(p) = p.build() {
                    window.paint_path(p, color);
                }
            }
        },
    )
    .absolute()
    .left_0()
    .top_0()
    .w(px(64.))
    .h(px(64.))
}

/// `.pill.sm.st-*` in a wave's header.
fn count_pill(t: &Theme, cls: &str, text: &str) -> Div {
    let tone = match cls {
        "st-working" => "accent",
        "st-needs" | "st-blocked" => "warn",
        "st-done" => "up",
        "st-failed" => "down",
        "st-review" => "accent",
        _ => "queued",
    };
    let (fg, bg, _) = st_colors(t, tone);
    div().flex_none().px(px(8.)).rounded_full().text_size(px(12.)).line_height(px(20.)).font_weight(FontWeight::SEMIBOLD).text_color(fg).bg(bg).child(text.to_string())
}

/// `.wchip`.
fn wchip(t: &Theme, cls: &str, text: &str) -> Div {
    let (fg, bg) = match cls {
        "warn" => (t.warn_fg, t.warn_soft),
        "bad" => (t.down, t.down_soft),
        "quiet" => (t.muted, t.panel_2),
        "compact" => (t.accent_fg, t.accent_soft),
        _ => (t.text_2, t.panel_2),
    };
    div().flex_none().px(px(8.)).py(px(2.)).rounded(px(6.)).text_size(px(12.5)).text_color(fg).bg(bg).whitespace_nowrap().child(text.to_string())
}

/// The `term` fact: Midna's mark and the terminal's name; it shows the terminal's tab in Midna.
fn term_chip(t: &Theme, text: &str) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(5.))
        .px(px(8.))
        .py(px(2.))
        .rounded(px(6.))
        .text_size(px(12.5))
        .text_color(t.text_2)
        .bg(t.panel_2)
        .whitespace_nowrap()
        .child(kit::icon(kit::Icon::Midna, 13., t.text_2))
        .child(text.to_string())
}

fn task_el(m: &MainWindow, t: &Theme, x: &WTask, ix: usize, in_ref: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let hover = accent_tint(t);
    let target = x.r.clone();
    let on = matches!(&m.panel, Some(Panel::Task { r, .. }) if *r == x.r);
    let pr_color = match x.pr_phase.as_str() {
        "merged" => t.up_fg,
        "declined" => t.muted,
        "fix" => t.down,
        "comments" => t.warn_fg,
        _ => t.accent_fg,
    };
    let mut l1 = div()
        .flex()
        .items_center()
        .gap(px(8.))
        .min_w_0()
        .child(div().flex_none().min_w(px(30.)).text_size(px(12.5)).text_color(t.faint).child(x.r.clone()));
    if let Some((key, url)) = x.jira.clone() {
        l1 = l1.child(
            div()
                .id(SharedString::from(format!("wt-jira-{}", x.r)))
                .flex_none()
                .cursor_pointer()
                .child(ico(Ico::Jira, 14., t.jira))
                .tooltip(kit::tip(format!("{key} in Jira")))
                .on_click(move |_, _, cx| cx.open_url(&url)),
        );
    }
    if let Some(txt) = x.pr_text.clone() {
        l1 = l1.child(
            div()
                .id(SharedString::from(format!("wt-pr-{}", x.r)))
                .flex()
                .flex_none()
                .items_center()
                .gap(px(3.))
                .text_size(px(12.))
                .text_color(pr_color)
                .child(ico(Ico::PrOpen, 13., pr_color))
                .child(txt)
                .when_some(stage_icon(&x.pr_phase), |d, st| d.child(div().ml(px(3.)).child(ico(Ico::Stage(st), 13., pr_color))))
                .when_some(x.pr_label.clone(), |d, l| d.tooltip(kit::tip(l))),
        );
    }
    l1 = l1
        .child(div().flex_1().min_w_0().truncate().text_size(px(14.)).text_color(if x.planned { t.text_2 } else { t.text }).child(x.title.clone()))
        .children(x.when.clone().map(|w| div().flex_none().text_size(px(12.5)).text_color(t.text_2).child(w)));
    let mut facts = div().flex().flex_wrap().gap(px(6.)).when(!in_ref, |d| d.pl(px(24.)));
    for (fi, (cls, text)) in x.facts.iter().enumerate() {
        if *cls == "term" {
            let sid = x.term.clone().unwrap_or_default();
            facts = facts.child(
                div()
                    .id(SharedString::from(format!("wt-term-{}-{fi}", x.r)))
                    .cursor_pointer()
                    .child(term_chip(t, text))
                    .tooltip(kit::tip("Show in Midna"))
                    .on_click(cx.listener(move |m, _, _, cx| {
                        cx.stop_propagation();
                        crate::ui::sessions::menu_focus(m, &sid, cx)
                    })),
            );
        } else {
            facts = facts.child(wchip(t, cls, text));
        }
    }
    div()
        .id(SharedString::from(format!("wt-{}", x.r)))
        .flex()
        .flex_col()
        .gap(px(6.))
        .when(!in_ref, |d| d.px(px(14.)).py(px(11.)).when(ix > 0, |d| d.border_t_1().border_color(t.divider)))
        .cursor_pointer()
        .when(on && !in_ref, |d| d.bg(hover))
        .when(!in_ref, |d| d.hover(move |d| d.bg(hover)))
        .child(l1)
        .when(!x.facts.is_empty(), |d| d.child(facts))
        .on_click(cx.listener(move |m, _, _, cx| m.open_task(target.clone(), cx)))
}

/// `.wv-box`: a wave's tasks in a tinted box.
fn task_box(m: &MainWindow, t: &Theme, tasks: &[WTask], cx: &mut Context<MainWindow>) -> Div {
    let mut b = div().flex().flex_col().mb(px(8.)).bg(t.tint).border_1().border_color(t.divider).rounded(px(10.)).overflow_hidden();
    for (ix, x) in tasks.iter().enumerate() {
        b = b.child(task_el(m, t, x, ix, false, cx));
    }
    b
}

/// The rail: `.wrail`.
pub fn rail(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Div {
    let gr = fmt::ref_of(g, "G");
    let items = rail_view(g, &m.goal_page.wave_open);
    let n = items.len();
    let mut out = div().flex().flex_col().px(px(18.)).py(px(14.)).bg(t.card).border_1().border_color(t.border).rounded(px(12.));
    for (ix, item) in items.into_iter().enumerate() {
        let last = ix + 1 == n;
        let (node_state, done) = match &item {
            Item::Wave { state, .. } => (state.clone(), state == "done"),
            Item::Post { .. } => ("loose".to_string(), false),
            Item::Ref { done, .. } => (String::new(), *done),
            Item::Finish { done, .. } => (if *done { "done".into() } else { "finish".into() }, *done),
        };
        // `.wv`: the node column (with the line down to the next item) and the main column.
        let is_ref = matches!(item, Item::Ref { .. });
        let line_color = if done { t.up } else { t.border };
        let left = div()
            .flex()
            .flex_col()
            .items_center()
            .flex_none()
            .w(px(NODE))
            .when(!is_ref, |d| d.pt(px(10.)).child(node(t, &node_state)))
            .when(!last, |d| d.child(div().flex_1().min_h(px(8.)).w(px(2.)).rounded(px(1.)).bg(line_color).when(!is_ref, |d| d.mt(px(2.)))));
        let main = match item {
            Item::Wave { wave, state, open, name, count, stop, gate, tasks, stop_box } => {
                let key = format!("{gr}:{wave}");
                let mut head = div()
                    .id(SharedString::from(format!("wv-head-{wave}")))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .min_h(px(40.))
                    .px(px(8.))
                    .mx(px(-8.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(|d| d.bg(t.tint))
                    .child(div().flex_none().text_size(px(12.5)).text_color(t.faint).child(format!("Wave {wave}")))
                    .children(name.map(|nm| {
                        div().text_size(px(14.5)).font_weight(FontWeight::SEMIBOLD).truncate().text_color(if matches!(state.as_str(), "waiting" | "ready") { t.text_2 } else { t.text }).child(nm)
                    }))
                    .child(div().flex().flex_none().items_center().gap(px(6.)).children(count.iter().map(|(cls, text)| count_pill(t, cls, text))))
                    .child(div().flex_1());
                if stop {
                    head = head.child(div().flex().flex_none().items_center().gap(px(5.)).text_size(px(12.)).text_color(t.warn_fg).child(ico(Ico::Flag, 12., t.warn_fg)).child("Review stop"));
                }
                head = head
                    .child(div().flex_none().when(open, |d| d.child(ico(Ico::Chev, 16., t.faint))).when(!open, |d| d.child(ico(Ico::Right, 16., t.faint))))
                    .on_click(cx.listener(move |m, _, _, cx| {
                        m.goal_page.wave_open.insert(key.clone(), !open);
                        cx.notify();
                    }));
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .min_w_0()
                    .pb(px(8.))
                    .child(head)
                    .children(gate.map(|gt| gate_el(m, t, &gr, gt, cx)))
                    .when(open, |d| d.child(task_box(m, t, &tasks, cx)))
                    .when_some(stop_box.filter(|_| open), |d, on| d.child(stop_box_el(m, t, &gr, wave, on, cx)))
            }
            Item::Post { open, tasks } => {
                let key = format!("{gr}:none");
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .min_w_0()
                    .pb(px(8.))
                    .child(
                        div()
                            .id("wv-head-post")
                            .flex()
                            .items_center()
                            .gap(px(10.))
                            .min_h(px(40.))
                            .px(px(8.))
                            .mx(px(-8.))
                            .rounded(px(8.))
                            .cursor_pointer()
                            .hover(|d| d.bg(t.tint))
                            .child(div().text_size(px(14.5)).font_weight(FontWeight::SEMIBOLD).text_color(t.text_2).child("Post"))
                            .child(div().flex_1())
                            .child(div().flex_none().when(open, |d| d.child(ico(Ico::Chev, 16., t.faint))).when(!open, |d| d.child(ico(Ico::Right, 16., t.faint))))
                            .on_click(cx.listener(move |m, _, _, cx| {
                                m.goal_page.wave_open.insert(key.clone(), !open);
                                cx.notify();
                            })),
                    )
                    .when(open, |d| d.child(task_box(m, t, &tasks, cx)))
            }
            Item::Ref { badge, home, landed, task, .. } => {
                let badge_el = div()
                    .id(SharedString::from(format!("wv-badge-{}", task.r)))
                    .absolute()
                    .left(px(34. - NODE - 14.))
                    .top(px(14.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .w(px(30.))
                    .h(px(22.))
                    .rounded(px(6.))
                    .text_size(px(11.5))
                    .font_weight(FontWeight::BOLD)
                    .text_color(t.goal)
                    .bg(t.goal_soft)
                    .border_1()
                    .border_color(t.goal_line)
                    .child(badge)
                    .when_some(home, |d, h| d.cursor_pointer().on_click(cx.listener(move |m, _, _, cx| crate::ui::goal::open(m, &h, false, cx))));
                div()
                    .relative()
                    .min_h(px(64.))
                    .min_w_0()
                    .pt(px(14.))
                    .pb(px(12.))
                    .pl(px(42.))
                    .child(div().absolute().left(px(-(NODE + 14.))).top_0().child(branch(t, landed)))
                    .child(badge_el)
                    .child(task_el(m, t, &task, 0, true, cx))
            }
            Item::Finish { done, text } => div().flex().items_center().min_h(px(40.)).min_w_0().pb(px(8.)).child(
                div().text_size(px(13.5)).font_weight(FontWeight::MEDIUM).text_color(if done { t.text_2 } else { t.muted }).child(text),
            ),
        };
        out = out.child(div().flex().gap(px(14.)).child(left).child(main.flex_1()));
    }
    out
}

/// `.wgate`: the stop point or failure under a wave, with "Continue to wave 2".
fn gate_el(m: &MainWindow, t: &Theme, gr: &str, g: Gate, cx: &mut Context<MainWindow>) -> Div {
    let (bg, fg) = match g.cls {
        "ask" => (Some(t.warn_soft), t.warn_text),
        "bad" => (Some(t.down_soft), t.text),
        _ => (None, t.muted),
    };
    let busy_key = format!("wave-continue:{gr}:{}", g.wave);
    let busy = m.goal_page.busy.contains(&busy_key);
    let mut d = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(12.))
        .text_size(px(13.))
        .text_color(fg)
        .rounded(px(10.))
        .when_some(bg, |d, bg| d.bg(bg).px(px(14.)).py(px(10.)))
        .when(bg.is_none(), |d| d.pb(px(4.)))
        .child(div().flex_1().min_w_0().child(g.text.clone()));
    if let Some((_, label)) = g.button {
        let (gr, n) = (gr.to_string(), g.wave);
        let primary = g.cls == "ask";
        let btn = if busy {
            kit::disabled(if primary { kit::btn_primary(t, SharedString::from(format!("wave-continue-{n}")), "Continuing…") } else { kit::btn_small(t, SharedString::from(format!("wave-continue-{n}")), "Continuing…") })
        } else if primary {
            kit::btn_primary(t, SharedString::from(format!("wave-continue-{n}")), label)
        } else {
            kit::btn_small(t, SharedString::from(format!("wave-continue-{n}")), label)
        };
        d = d.child(div().flex_none().child(btn.on_click(cx.listener(move |m, _, _, cx| continue_wave(m, &gr, n, cx)))));
    }
    d.children(inline_note(m, t, &format!("wgate:{gr}")))
}

/// `wave-continue`: `POST goals/:g/waves/:n/continue`; "The goal goes on" under the gate.
pub fn continue_wave(m: &mut MainWindow, gr: &str, n: i64, cx: &mut Context<MainWindow>) {
    run(m, format!("wave-continue:{gr}:{n}"), Some(format!("wgate:{gr}")), false, format!("goals/{gr}/waves/{n}/continue"), json!({}), cx, move |_, v, _| {
        if v["let_start"] == true { format!("Wave {n} can start") } else { "The goal goes on".into() }
    });
}

/// The owner's "Stop after this wave for my review" (the Python board's checkbox): their own word,
/// so the app is the only place it's set. `POST goals/:g/waves/:n {stop_after}`.
fn stop_box_el(m: &MainWindow, t: &Theme, gr: &str, wave: i64, on: bool, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let gr = gr.to_string();
    let busy = m.goal_page.busy.contains(&format!("wave-stop:{gr}:{wave}"));
    div()
        .id(SharedString::from(format!("wave-stop-{wave}")))
        .flex()
        .items_center()
        .gap(px(8.))
        .text_size(px(13.))
        .text_color(t.text_2)
        .cursor_pointer()
        .when(busy, |d| d.opacity(0.6))
        .child(checkbox(t, on, false))
        .child("Stop after this wave for my review")
        .on_click(cx.listener(move |m, _, _, cx| set_stop(m, &gr, wave, !on, cx)))
}

/// `POST goals/:g/waves/:n {stop_after}` from the checkbox.
pub fn set_stop(m: &mut MainWindow, gr: &str, n: i64, on: bool, cx: &mut Context<MainWindow>) {
    if let Some(w) = m.data.goal.as_mut().filter(|g| fmt::ref_of(g, "G") == gr).and_then(|g| g["waves"].as_array_mut()).and_then(|ws| ws.iter_mut().find(|w| i(w, "wave") == n)) {
        w["stop_after"] = json!(on);
    }
    run(m, format!("wave-stop:{gr}:{n}"), Some(format!("wgate:{gr}")), false, format!("goals/{gr}/waves/{n}"), json!({"stop_after": on}), cx, move |_, _, _| {
        if on { format!("The goal stops after wave {n} for your review") } else { format!("The goal goes on after wave {n}") }
    });
}
