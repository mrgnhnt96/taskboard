// Excerpt of crates/taskboard-app/src/settings.rs, lines 1116-1241
    fn history_page(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let list = div().id("history-rows").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(20.)).px(px(28.)).pt(px(4.)).pb(px(28.));
        let Some(h) = self.history.clone() else {
            return list.child(kit::empty(t, "Asking the board…"));
        };
        let pick = |key: &'static str, label: &str, note: &str, first: bool, cx: &mut Context<Self>| {
            let choices: Vec<i64> = h[format!("{key}_choices")].as_array().map(|a| a.iter().filter_map(|x| x.as_i64()).collect()).unwrap_or_default();
            let cur = h[key].as_i64().unwrap_or(0);
            let labels: Vec<String> = choices.iter().map(|n| days_label(*n)).collect();
            let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
            let on = choices.iter().position(|n| *n == cur).unwrap_or(usize::MAX);
            let control = kit::seg(t, &format!("hist-{key}"), &refs, on, |i, item| {
                let n = choices[i];
                item.on_click(cx.listener(move |s, _, _, cx| s.post_history("history", json!({key: n}), cx)))
            });
            line(t, &format!("hist-{key}-row"), first, false, label, note, None).child(controls().child(control))
        };
        let detail = pick("detail_days", "Keep every event for", "Commits, questions, checkpoints and status changes of finished tasks: the marks on a day's timeline and its task log.", true, cx);
        let summary = pick("summary_days", "Keep day summaries for", "One small record per day and project: the timeline bars and totals the week views use.", false, cx);
        let i = |k: &str| h[k].as_i64().unwrap_or(0);
        let used = line(
            t,
            "hist-used",
            false,
            false,
            "Space used",
            &format!("{} events ({}) and {} day summaries ({})", i("events"), bytes_label(i("events_bytes")), i("summaries"), bytes_label(i("summaries_bytes"))),
            None,
        );
        let last = h["last_cleanup"].as_str().map(crate::fmt::full_time).unwrap_or_else(|| "not yet".into());
        let button = if self.history_busy {
            kit::disabled(kit::btn_small(t, "hist-clean", "Cleaning up…"))
        } else {
            kit::btn_small(t, "hist-clean", "Clean up now").on_click(cx.listener(|s, _, _, cx| s.post_history("history/cleanup", json!({}), cx)))
        };
        let clean = line(t, "hist-clean-row", false, false, "Cleanup", &format!("Runs every night after 3 AM. Last run: {last}."), None).child(controls().child(button));
        let mut card = kit::card(t).overflow_hidden().child(detail).child(summary).child(used).child(clean);
        if let Some((text, bad)) = &self.history_note {
            card = card.child(row(t, "hist-note", false).bg(if *bad { t.down_soft } else { t.up_soft }).text_color(if *bad { t.down } else { t.up_fg }).text_size(px(12.)).child(text.clone()));
        }
        list.child(card)
    }
}

// ------------------------------------------------------------------ usage

/// `GET /usage` answers `null` while there's no reading and the limit is off: that's the plan switched off.
fn daily_of(usage: &Value) -> Value {
    match &usage["daily"] {
        Value::Null => serde_json::json!({"base": "off", "carry": "next_day", "on": false}),
        d => d.clone(),
    }
}

/// The daily limit's base choices: off, even, the fixed shares, and a fixed share set elsewhere
/// (`tb usage daily`) in its place among them.
fn daily_bases(cur: &Value) -> Vec<Value> {
    let mut pcts: Vec<f64> = vec![10., 15., 20., 25.];
    if let Some(n) = cur.as_f64().filter(|n| !pcts.contains(n)) {
        pcts.push(n);
        pcts.sort_by(f64::total_cmp);
    }
    [json!("off"), json!("even")].into_iter().chain(pcts.into_iter().map(|n| json!(n))).collect()
}

fn daily_base_label(b: &Value) -> String {
    match b.as_f64() {
        Some(n) => format!("{}%", crate::app::js_num(&json!(n))),
        None if b == "even" => "Even".into(),
        None => "Off".into(),
    }
}

impl SettingsWindow {
    fn usage_page(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let list = div().id("usage-rows").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(20.)).px(px(28.)).pt(px(4.)).pb(px(28.));
        let Some(d) = self.daily.clone() else {
            return list.child(kit::empty(t, "Asking the board…"));
        };
        let busy = self.daily_busy;
        let bases = daily_bases(&d["base"]);
        let labels: Vec<String> = bases.iter().map(daily_base_label).collect();
        let refs: Vec<&str> = labels.iter().map(|s| s.as_str()).collect();
        let on = bases.iter().position(|b| b.as_f64().zip(d["base"].as_f64()).map_or(*b == d["base"], |(a, c)| a == c)).unwrap_or(0);
        let base = kit::seg(t, "daily-base", &refs, on, |i, item| {
            let b = bases[i].clone();
            if busy || i == on {
                item
            } else {
                item.on_click(cx.listener(move |s, _, _, cx| s.post_daily("usage/daily", json!({"base": b}), cx)))
            }
        });
        let note = "Even splits what's left of the week over the days to its reset; a percent gives each day that share of the week.";
        let mut card = kit::card(t).overflow_hidden().child(line(t, "daily-base-row", true, false, "Daily limit", note, None).child(controls().child(base)));
        if d["on"] == true {
            let spread = d["carry"] == "spread";
            let carry = kit::seg(t, "daily-carry", &["Next day", "Spread"], spread as usize, |i, item| {
                if busy || (i == 1) == spread {
                    item
                } else {
                    item.on_click(cx.listener(move |s, _, _, cx| s.post_daily("usage/daily", json!({"carry": if i == 1 { "spread" } else { "next_day" }}), cx)))
                }
            });
            let carry_note = if spread { "What a day leaves is shared over the rest of the week." } else { "What a day leaves goes to the next day." };
            card = card.child(line(t, "daily-carry-row", false, false, "Unused share", carry_note, None).child(controls().child(carry)));
            let lifted = d["overridden"] == true;
            let today = d["line"].as_str().unwrap_or("No 7-day reading yet.");
            let toggle = kit::seg(t, "daily-override", &["Limit", "Lifted"], lifted as usize, |i, item| {
                if busy || (i == 1) == lifted {
                    item
                } else {
                    item.on_click(cx.listener(move |s, _, _, cx| s.post_daily("usage/override", json!({"off": i == 0}), cx)))
                }
            });
            card = card.child(line(t, "daily-today-row", false, d["limited"] == true, "Today", today, None).child(controls().child(toggle)));
        }
        if let Some((text, bad)) = &self.daily_note {
            card = card.child(row(t, "daily-note", false).bg(if *bad { t.down_soft } else { t.up_soft }).text_color(if *bad { t.down } else { t.up_fg }).text_size(px(12.)).child(text.clone()));
        }
        list.child(card)
    }
}

// ------------------------------------------------------------------ QA

impl SettingsWindow {
