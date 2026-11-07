// Excerpt of crates/taskboard-app/src/app.rs, lines 1781-1847
pub fn banner_view(down: Option<&str>, alerts: &[Value]) -> Vec<BannerRow> {
    let mut out = Vec::new();
    if let Some(e) = down.filter(|e| !e.is_empty()) {
        out.push(BannerRow { kind: "down", text: format!("{e} Trying again every few seconds."), ago: None, buttons: Vec::new() });
    }
    let (urgent, alerts): (Vec<Value>, Vec<Value>) = alerts.iter().cloned().partition(|a| a["urgent"] == true);
    for a in &urgent {
        out.push(BannerRow { kind: "urgent", ..alert_row_view(a) });
    }
    if alerts.len() > 1 {
        out.push(BannerRow {
            kind: "alert",
            text: format!("{} your attention.", fmt::plural(alerts.len() as i64, "task needs", "tasks need")),
            ago: Some(fmt::ago(s(alerts.last().unwrap_or(&Value::Null), "at"))),
            buttons: vec![("alerts-open", "Show all".into())],
        });
    } else {
        for a in &alerts {
            out.push(BannerRow { kind: "alert", ..alert_row_view(a) });
        }
    }
    out
}

/// `alertsDialogHtml`: (title, rows); its foot is "Dismiss all" while any alert can be dismissed. (For the dialog in modals.rs.)
pub fn alerts_dialog_view(alerts: &[Value]) -> (String, Vec<BannerRow>) {
    let title = format!("{} your attention", fmt::plural(alerts.len() as i64, "task needs", "tasks need"));
    let rows = alerts.iter().map(alert_row_view).collect();
    (title, rows)
}

/// A status-bar pill: its class (`up`, `down`, `hours`, `hours closed`), label and tooltip.
#[derive(Clone, Debug, PartialEq)]
pub struct Pill {
    pub cls: &'static str,
    pub label: String,
    pub title: String,
}

/// `connPill`.
pub fn conn_pill(m: &Value) -> Pill {
    let heard = fmt::opt_s(m, "seen_at").map(|x| format!("Last heard from {}", fmt::ago(x))).unwrap_or_else(|| "Not heard from yet".into());
    if b(m, "up") {
        Pill { cls: "up", label: "Connected to Midna".into(), title: heard }
    } else {
        Pill { cls: "down", label: "Midna isn’t running".into(), title: format!("{heard}. Starting, messaging and closing terminals wait until it’s back.") }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageWin {
    /// ok, warm (75 %+), hot (90 %+); `lifted` for today's share while its limit is off for the day.
    pub level: &'static str,
    pub label: String,
    /// Bar width in percent (0–100).
    pub width: f64,
    pub value: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct UsageView {
    pub stale: bool,
    pub title: String,
    pub windows: Vec<UsageWin>,
}

/// `usageLevel`.
