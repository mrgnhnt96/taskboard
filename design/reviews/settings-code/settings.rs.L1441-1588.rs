// Excerpt of crates/taskboard-app/src/settings.rs, lines 1441-1588
    fn reviewers_page(&self, t: &Theme) -> impl IntoElement {
        let mut list = div().id("reviewer-rows").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(20.)).px(px(28.)).pt(px(4.)).pb(px(28.));
        let Some(v) = self.reviewers.clone() else {
            return list.child(kit::empty(t, "Asking the board…"));
        };
        let projects = v["projects"].as_array().cloned().unwrap_or_default();
        if projects.is_empty() {
            return list.child(kit::empty(t, "No reviewers"));
        }
        for p in projects {
            let name = p["name"].as_str().unwrap_or("").to_string();
            let mut card = kit::card(t).overflow_hidden();
            for (i, r) in p["reviewers"].as_array().cloned().unwrap_or_default().iter().enumerate() {
                let id = format!("rev-{}", r["id"]);
                let removed = r["removed"] == true;
                let mut l = line(t, &id, i == 0, false, r["name"].as_str().unwrap_or(""), &reviewer_note(r), None).when(removed, |d| d.text_color(t.muted));
                if r["pinned"] == true {
                    l = l.child(kit::tone_pill(t, "review", "Pinned"));
                }
                if removed {
                    l = l.child(kit::tone_pill(t, "neutral", "Never asked"));
                }
                card = card.child(l);
            }
            let heading = div().px(px(4.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(name);
            list = list.child(div().flex().flex_col().gap(px(8.)).child(heading).child(card));
        }
        list
    }
}

impl SettingsWindow {
    fn worktrees_page(&self, t: &Theme) -> impl IntoElement {
        let mut list = div().id("worktree-rows").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(20.)).px(px(28.)).pt(px(4.)).pb(px(28.));
        let Some(v) = self.slots.as_ref() else {
            return list.child(kit::empty(t, "Asking the board…"));
        };
        let pools = pools_vm(v);
        if pools.is_empty() {
            return list.child(kit::empty(t, "No worktree slots"));
        }
        for p in pools {
            let mut card = kit::card(t).overflow_hidden().child(line(t, &format!("pool-{}", p.project), true, false, "Pool", &p.settings, None));
            for r in &p.slots {
                let pill = match &r.held_by {
                    Some(task) => kit::tone_pill(t, "working", task.clone()).id(SharedString::from(format!("slot-tip-{}", r.key))).when_some(r.tip.clone(), |d, tip| d.tooltip(kit::tip(tip))).into_any_element(),
                    None => kit::tone_pill(t, "neutral", "Free").into_any_element(),
                };
                card = card.child(line(t, &format!("slot-{}", r.key), false, false, &r.name, &r.note, None).child(controls().child(pill)));
            }
            if !p.waiting.is_empty() {
                card = card.child(line(t, &format!("waiting-{}", p.project), false, true, &format!("{} waiting", p.waiting.len()), &p.waiting.join(", "), None));
            }
            let heading = div().px(px(4.)).text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).text_color(t.text).child(format!("{} · {}", p.project, p.count));
            list = list.child(div().flex().flex_col().gap(px(8.)).child(heading).child(card));
        }
        list
    }
}

/// One project's pool in Worktrees.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PoolVm {
    pub project: String,
    /// "6 slots".
    pub count: String,
    /// "Keeps 2 to 10 slots. Build output is cleared after 2 idle days, the slot removed after 7."
    pub settings: String,
    pub slots: Vec<SlotVm>,
    /// The tasks waiting for a slot of this project.
    pub waiting: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SlotVm {
    pub key: String,
    pub name: String,
    /// "7.7 GB · last used 10:08 AM".
    pub note: String,
    /// "T12", the task holding it; None when it's free.
    pub held_by: Option<String>,
    /// The holder's title.
    pub tip: Option<String>,
}

/// `GET /slots` grouped by project, in the daemon's order (project, then slot number).
pub(crate) fn pools_vm(v: &Value) -> Vec<PoolVm> {
    let mut out: Vec<PoolVm> = vec![];
    for x in v["slots"].as_array().cloned().unwrap_or_default() {
        let project = x["project"].as_str().unwrap_or("").to_string();
        let i = match out.iter().position(|p| p.project == project) {
            Some(i) => i,
            None => {
                out.push(PoolVm { project: project.clone(), count: String::new(), settings: pool_settings(&v["pools"][&project]), slots: vec![], waiting: vec![] });
                out.len() - 1
            }
        };
        let p = &mut out[i];
        let held = x["held_by"].as_str().map(str::to_string);
        let size = x["size_kb"].as_i64().map(disk_size);
        let used = x["used_at"].as_str().map(used_at).filter(|u| !u.is_empty()).map(|u| format!("last used {u}"));
        p.slots.push(SlotVm {
            key: x["path"].as_str().unwrap_or("").to_string(),
            name: x["name"].as_str().unwrap_or("").to_string(),
            note: [size, used].into_iter().flatten().collect::<Vec<_>>().join(" · "),
            tip: held.as_ref().and_then(|_| x["title"].as_str()).filter(|t| !t.is_empty()).map(str::to_string),
            held_by: held,
        });
        // Every slot of a repo lists the same waiting tasks; each shows once.
        for w in x["waiting"].as_array().cloned().unwrap_or_default().iter().filter_map(|w| w.as_str()) {
            if !p.waiting.iter().any(|have| have == w) {
                p.waiting.push(w.to_string());
            }
        }
    }
    for p in &mut out {
        p.count = crate::fmt::plural(p.slots.len() as i64, "slot", "slots");
    }
    out
}

/// "Keeps 2 to 10 slots. Build output is cleared after 2 idle days, the slot removed after 7."
fn pool_settings(c: &Value) -> String {
    let (Some(min), Some(max), Some(trim), Some(remove)) = (c["min_slots"].as_i64(), c["max_slots"].as_i64(), c["idle_trim_days"].as_i64(), c["idle_remove_days"].as_i64()) else {
        return String::new();
    };
    format!("Keeps {min} to {max} slots. Build output is cleared after {}, the slot removed after {remove}.", crate::fmt::plural(trim, "idle day", "idle days"))
}

/// "512 KB", "840 MB", "2.3 GB".
fn disk_size(kb: i64) -> String {
    match kb {
        k if k >= 1024 * 1024 => format!("{:.1} GB", k as f64 / (1024.0 * 1024.0)),
        k if k >= 1024 => format!("{} MB", k / 1024),
        k => format!("{k} KB"),
    }
}

/// "3:05 PM" today, "Oct 7, 3:05 PM" another day.
fn used_at(iso: &str) -> String {
    let Some(t) = crate::fmt::parse(iso) else { return String::new() };
    let l = crate::fmt::local(t);
    if crate::fmt::is_today(t) { l.format("%-I:%M %p").to_string() } else { l.format("%b %-d, %-I:%M %p").to_string() }
}

// ------------------------------------------------------------------ rows

/// A row's shell: padding and the line above every row but the first.
