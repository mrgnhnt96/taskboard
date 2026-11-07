// Excerpt of crates/taskboard-app/src/app.rs, lines 1847-1930
/// `usageLevel`.
pub fn usage_level(pct: f64) -> &'static str {
    if pct >= 90. {
        "hot"
    } else if pct >= 75. {
        "warm"
    } else {
        "ok"
    }
}

/// A JSON number as JS prints it (`35`, `35.5`).
pub(crate) fn js_num(v: &Value) -> String {
    match v.as_f64() {
        Some(f) if f.fract() == 0. && f.abs() < 1e15 => format!("{}", f as i64),
        Some(f) => format!("{f}"),
        None => v.as_str().unwrap_or("").to_string(),
    }
}

/// `usagePill`: None when there's no reading (or no windows).
pub fn usage_view(u: &Value) -> Option<UsageView> {
    let wins = arr(u, "windows");
    if !u.is_object() || wins.is_empty() {
        return None;
    }
    // The web compares with the reader's own clock (not the board's).
    let stale = fmt::parse(s(u, "seen_at")).is_some_and(|x| (fmt::device_now() - x).num_milliseconds() > 3_600_000);
    let mut title: Vec<String> = wins
        .iter()
        .map(|w| {
            let name = match s(w, "key") {
                "five_hour" => "5-hour".to_string(),
                "seven_day" => "7-day".to_string(),
                _ => s(w, "label").to_string(),
            };
            let resets = fmt::opt_s(w, "resets_at").map(|r| format!(", resets {}", fmt::day_clock(r))).unwrap_or_default();
            format!("{name} limit: {}% used{resets}", js_num(&w["pct"]))
        })
        .collect();
    let today = usage_today(&u["daily"]);
    if today.is_some() {
        title.push(s(&u["daily"], "line").to_string());
    }
    if stale {
        title.push(format!("Last read {}", fmt::ago(s(u, "seen_at"))));
    }
    let windows = wins
        .iter()
        .map(|w| {
            let pct = w["pct"].as_f64().unwrap_or(0.);
            UsageWin { level: usage_level(pct), label: s(w, "label").to_string(), width: pct.clamp(0., 100.), value: format!("{}%", js_num(&w["pct"])) }
        })
        .chain(today)
        .collect();
    Some(UsageView { stale, title: title.join("\n"), windows })
}

/// `usageToday`: today's share of the week while the daily limit is on; the bar is how much of it is
/// used, the value what's left.
fn usage_today(d: &Value) -> Option<UsageWin> {
    if d["on"] != Value::Bool(true) || !d["allowed"].is_number() {
        return None;
    }
    let allowed = d["allowed"].as_f64().unwrap_or(0.);
    let share = if allowed > 0. { d["used"].as_f64().unwrap_or(0.) / allowed * 100. } else { 100. };
    let lifted = d["overridden"] == Value::Bool(true);
    Some(UsageWin {
        level: if lifted { "lifted" } else { usage_level(share) },
        label: "Today".into(),
        width: (share.clamp(0., 100.) * 10.).round() / 10.,
        value: if lifted { "Lifted".into() } else { format!("{}% left", js_num(&d["left"])) },
    })
}

// ------------------------------------------------------------------ chrome drawing

/// `.banner`: 7px 40px, 6/10 gaps, 13px, a bottom line.
fn bar(bg: Hsla, fg: Hsla, line: Hsla) -> Div {
    div().flex().flex_none().items_center().flex_wrap().gap_x(px(10.)).gap_y(px(6.)).px(px(40.)).py(px(7.)).text_size(px(13.)).bg(bg).text_color(fg).border_b_1().border_color(line)
}

/// `.btn.sm` in the banner: 32px tall, 12.5px, 0 10px, radius 7. `soft` is accent on accent-soft
/// (semibold); otherwise `.ghost` (muted, no fill).
