// Excerpt of crates/taskboard-app/src/settings.rs, lines 1242-1286
    fn qa_page(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let list = div().id("qa-rows").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(20.)).px(px(28.)).pt(px(4.)).pb(px(28.));
        let Some(q) = self.qa.clone() else {
            return list.child(kit::empty(t, "Asking the board…"));
        };
        let jira = q["jira"] == true;
        let on = q["on"] == true;
        let note = if jira {
            "The board reads new comments on its tickets every few minutes. A clear direction becomes a high-priority follow-up task; \
             a question waits for you on the goal's QA tab and in the alerts. Comments from before you switch it on are never read."
        } else {
            "Needs Jira: set up [jira] in ~/.config/taskboard/config.toml first."
        };
        let control = kit::seg(t, "qa-on", &["Off", "On"], if on { 1 } else { 0 }, |i, item| {
            if jira && !self.qa_busy {
                item.on_click(cx.listener(move |s, _, _, cx| s.post_qa(i == 1, cx)))
            } else {
                item
            }
        });
        let switch = line(t, "qa-on-row", true, !jira, "Read QA comments", note, None).child(controls().child(control));
        let mut card = kit::card(t).overflow_hidden().child(switch);
        if on {
            let i = |k: &str| q[k].as_i64().unwrap_or(0);
            let checked = q["checked_at"].as_str().map(crate::fmt::full_time).unwrap_or_else(|| "not yet".into());
            card = card.child(line(
                t,
                "qa-state",
                false,
                false,
                "Comments",
                &format!("{} read, {} waiting on you. Last checked Jira: {checked}.", i("comments"), i("waiting")),
                None,
            ));
        }
        if let Some((text, bad)) = &self.qa_note {
            card = card.child(row(t, "qa-note", false).bg(if *bad { t.down_soft } else { t.up_soft }).text_color(if *bad { t.down } else { t.up_fg }).text_size(px(12.)).child(text.clone()));
        }
        list.child(card)
    }
}

// ------------------------------------------------------------------ system

impl SettingsWindow {
