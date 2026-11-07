// Excerpt of crates/taskboard-app/src/settings.rs, lines 1589-1660
fn row(t: &Theme, id: &str, first: bool) -> Stateful<Div> {
    div().id(SharedString::from(id.to_string())).flex().items_center().gap(px(16.)).px(px(16.)).py(px(9.)).min_h(px(46.)).when(!first, |d| d.border_t_1().border_color(t.border))
}

/// A row with a name (or `title`, bold) and what it means; the caller adds its controls.
fn line(t: &Theme, id: &str, first: bool, warn: bool, label: &str, note: &str, title: Option<(&str, bool)>) -> Stateful<Div> {
    let label = title.map_or(label.to_string(), |(s, _)| s.to_string());
    row(t, id, first).when(warn, |d| d.bg(t.warn_soft)).child(
        div()
            .flex_1()
            .min_w(px(150.))
            .flex()
            .flex_col()
            .child(div().when(title.is_some_and(|(_, b)| b), |d| d.font_weight(FontWeight::SEMIBOLD)).child(label))
            .when(!note.is_empty(), |d| d.child(div().text_size(px(11.5)).line_height(px(15.)).text_color(if warn { t.warn_fg } else { t.muted }).child(note.to_string()))),
    )
}

fn controls() -> Div {
    div().flex().flex_none().items_center().gap(px(8.))
}

/// Whether an account needs the owner: its sign-in stopped working or lacks scopes Taskboard needs.
fn needs_owner(a: &Value) -> bool {
    a["connected"] == true && (a["error"].is_string() || a["reauth"] == true)
}

/// Amber: the scopes Taskboard now needs that this sign-in lacks, and how to give them.
fn reauth_row(t: &Theme, id: &str, a: &Value, how: &str, button: AnyElement) -> AnyElement {
    let missing: Vec<&str> = a["missing"].as_array().map(|m| m.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    line(t, &format!("{id}-reauth"), false, true, "Needs a new sign-in", &format!("Taskboard now needs {}. {how}", missing.join(", ")), None)
        .child(div().flex().flex_wrap().gap(px(4.)).max_w(px(200.)).justify_end().children(missing.iter().map(|s| kit::chip(t, s.to_string()))))
        .child(controls().child(button))
        .into_any_element()
}

/// What the token's scopes leave off ("Can read PRs; add read:user:bitbucket to show your Bitbucket
/// name."), with the scopes it lacks. Read-only: a new token is made on the service.
fn off_row(t: &Theme, id: &str, a: &Value, note: &str) -> AnyElement {
    let lacks: Vec<&str> = a["missing_scopes"].as_array().map(|m| m.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
    line(t, &format!("{id}-off"), false, false, note, "", None)
        .child(div().flex().flex_wrap().gap(px(4.)).max_w(px(260.)).justify_end().children(lacks.iter().map(|s| kit::chip(t, s.to_string()))))
        .into_any_element()
}

/// How agents push: only in the board's sessions, only while on a task.
fn push_row(t: &Theme, id: &str, host: &str) -> AnyElement {
    row(t, &format!("{id}-git"), false)
        .child(kit::dot(t.up, 7.))
        .child(div().flex_1().min_w_0().text_size(px(12.)).text_color(t.muted).child(format!("Agents on a task git push to {host} with this account. Other terminals keep their own git sign-in.")))
        .into_any_element()
}

fn error_row(t: &Theme, id: &str, a: &Value) -> Option<AnyElement> {
    let e = a["error"].as_str()?;
    Some(row(t, &format!("{id}-error"), false).bg(t.warn_soft).child(kit::dot(t.warn, 7.)).child(div().flex_1().min_w_0().text_size(px(12.)).text_color(t.warn_fg).child(e.to_string())).into_any_element())
}

fn link_row(t: &Theme, id: &str, label: &str, url: &'static str, cx: &mut Context<SettingsWindow>) -> AnyElement {
    row(t, id, false)
        .min_h(px(34.))
        .text_size(px(12.))
        .child(kit::link(t, SharedString::from(format!("{id}-link")), format!("{label} ↗")).on_click(cx.listener(move |_, _, _, cx| cx.open_url(url))))
        .child(div().flex_1().min_w_0().truncate().text_color(t.faint).child(url.trim_start_matches("https://").to_string()))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::SettingsWindow;
    use crate::backend::Backend;
    use crate::parity::{Recording, settle};
