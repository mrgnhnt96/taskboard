//! The dialog over the main window: the alerts list (read-only; esc or Close closes it, and it
//! closes by itself once nothing needs you), the New idea form (`ui::ideas`), and the confirm for a
//! link menu's Mark done / Cancel task…. Tasks, goals and issues are made and changed through
//! Claude (`tb`), never typed in here.
use crate::app::{CloseOverlay, MainWindow, alert_row};
use crate::fmt::arr;
use crate::theme::Theme;
use crate::ui::kit;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::cmp::Ordering;

pub enum Modal {
    Alerts,
    /// The ⌘K palette (`ui::search`).
    Search,
    /// A task's whole note for testers (the panel's Show all).
    Note(String),
    /// The Ideas page's New idea form (`ui::ideas::form`).
    NewIdea,
    /// A terminal link menu's Mark done or Cancel task… (`taskboard://task/T6?do=done|cancel`).
    /// The menu skips Midna's approval, so nothing happens until the owner confirms here.
    Finish(Finish),
}

pub struct Finish {
    pub r: String,
    pub cancel: bool,
    /// Why it can't go ahead: it's already done, or the board refused.
    pub note: Option<String>,
    pub busy: bool,
}

impl Modal {
    pub fn finish(r: String, cancel: bool) -> Modal {
        Modal::Finish(Finish { r, cancel, note: None, busy: false })
    }
}

/// JavaScript's `a.localeCompare(b)` (ICU root collation) for the names the board shows:
/// whitespace, then punctuation in ICU's order, then digits, then letters ignoring case; accents
/// break ties before case, and lower case sorts before upper case.
pub fn locale_cmp(a: &str, b: &str) -> Ordering {
    const PUNCT: &str = "_-,;:!?.'\"()[]{}@*/\\&#%`^+<=>|~$";
    fn base(c: char) -> (char, bool) {
        let plain = match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' => 'a',
            'À' | 'Á' | 'Â' | 'Ã' | 'Ä' | 'Å' => 'A',
            'ç' => 'c',
            'Ç' => 'C',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'È' | 'É' | 'Ê' | 'Ë' => 'E',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'Ì' | 'Í' | 'Î' | 'Ï' => 'I',
            'ñ' => 'n',
            'Ñ' => 'N',
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' => 'o',
            'Ò' | 'Ó' | 'Ô' | 'Õ' | 'Ö' => 'O',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'Ù' | 'Ú' | 'Û' | 'Ü' => 'U',
            'ý' | 'ÿ' => 'y',
            'Ý' => 'Y',
            _ => return (c, false),
        };
        (plain, true)
    }
    fn keys(s: &str) -> (Vec<(u8, u32)>, Vec<u8>, Vec<u8>) {
        let (mut p, mut sec, mut ter) = (Vec::new(), Vec::new(), Vec::new());
        for c in s.chars() {
            let (c, accent) = base(c);
            let primary = if c.is_whitespace() {
                (0, 0)
            } else if let Some(i) = PUNCT.find(c) {
                (1, i as u32)
            } else if c.is_ascii_digit() {
                (2, c as u32)
            } else if c.is_alphabetic() {
                (3, c.to_lowercase().next().unwrap_or(c) as u32)
            } else {
                (4, c as u32)
            };
            p.push(primary);
            sec.push(accent as u8);
            ter.push(c.is_uppercase() as u8);
        }
        (p, sec, ter)
    }
    let (ka, kb) = (keys(a), keys(b));
    ka.0.cmp(&kb.0).then_with(|| ka.1.cmp(&kb.1)).then_with(|| ka.2.cmp(&kb.2))
}

fn close(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    m.set_modal(None, window, cx);
}

/// The dialog over a dimmed backdrop. As on the web, clicks on the backdrop do nothing.
pub fn render(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    let t = cx.global::<Theme>().clone();
    // The alerts dialog closes itself once nothing needs you (renderBanner).
    if matches!(m.modal, Some(Modal::Alerts)) && arr(m.state(), "alerts").is_empty() && m.data.state.is_some() {
        m.modal = None;
        window.focus(&m.focus, cx);
        return div().into_any_element();
    }
    let body = match &m.modal {
        Some(Modal::Alerts) => alerts(m, &t, cx),
        Some(Modal::Search) => return crate::ui::search::render(m, window, cx),
        Some(Modal::Note(text)) => {
            let text = text.clone();
            note(m, &t, &text, window, cx)
        }
        Some(Modal::NewIdea) => crate::ui::ideas::form(m, &t, window, cx),
        Some(Modal::Finish(f)) => finish(m, &t, f, cx),
        None => return div().into_any_element(),
    };
    deferred(
        kit::scrim(&t, "modal-scrim")
            .flex()
            .items_center()
            .justify_center()
            .on_action(cx.listener(|m, _: &CloseOverlay, window, cx| on_escape(m, window, cx)))
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().id("modal-body").flex().flex_col().max_h(relative(0.92)).child(body)),
    )
    .with_priority(1)
    .into_any_element()
}

/// Esc in the dialog closes it.
pub fn on_escape(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    if m.modal.is_some() {
        close(m, window, cx);
    }
}

/// Title row with a close button.
pub fn head(t: &Theme, title: impl Into<SharedString>, cx: &mut Context<MainWindow>) -> Div {
    div()
        .flex()
        .flex_none()
        .items_center()
        .gap(px(10.))
        .px(px(20.))
        .pt(px(18.))
        .pb(px(12.))
        .child(div().flex_1().text_size(px(17.)).font_weight(FontWeight::BOLD).child(title.into()))
        .child(kit::btn_small(t, "modal-close", "Close").on_click(cx.listener(|m, _, window, cx| close(m, window, cx))))
}

fn alerts(m: &MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> AnyElement {
    let list = arr(m.state(), "alerts").to_vec();
    let (title, _) = crate::app::alerts_dialog_view(&list);
    let mut col = div().id("alerts-list").flex().flex_col().overflow_y_scroll();
    for a in &list {
        col = col.child(alert_row(t, a, cx));
    }
    kit::modal_box(t, 560.)
        .child(head(t, title, cx))
        .child(col)
        .when(list.iter().any(|a| !crate::app::alert_stays(a)), |d| {
            d.child(
                div().flex().justify_end().p(px(16.)).child(
                    kit::btn_small(t, "alerts-dismiss-all", "Dismiss all")
                        // The dialog closes by itself once the alerts are gone.
                        .on_click(cx.listener(|m, _, _, cx| crate::app::dismiss_all_alerts(m, cx))),
                ),
            )
        })
        .into_any_element()
}

/// The whole note for testers, scrolling when it's taller than the window.
fn note(m: &MainWindow, t: &Theme, text: &str, window: &mut Window, cx: &mut Context<MainWindow>) -> AnyElement {
    kit::modal_box(t, 640.)
        .child(head(t, "Note for testers", cx))
        .child(
            div()
                .id("note-all")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .px(px(20.))
                .pb(px(20.))
                .line_height(relative(1.5))
                .child(crate::ui::task_panel::draw_note(m, text, window, cx)),
        )
        .into_any_element()
}

/// The task's own answer, once the panel has it.
fn shown_task<'a>(m: &'a MainWindow, r: &str) -> Option<&'a serde_json::Value> {
    m.data.task.as_ref().filter(|t| t["ref"].as_str() == Some(r))
}

/// Why a task that's no longer open can't be marked done or cancelled.
fn already(t: &serde_json::Value) -> Option<String> {
    let r = t["ref"].as_str().unwrap_or("It");
    if t["failed"] == true {
        Some(format!("{r} already failed."))
    } else if t["status"] == "done" {
        Some(format!("{r} is already done."))
    } else {
        None
    }
}

fn finish(m: &MainWindow, t: &Theme, f: &Finish, cx: &mut Context<MainWindow>) -> AnyElement {
    let shown = shown_task(m, &f.r).or(m.data.task_card.as_ref().filter(|c| c["ref"].as_str() == Some(f.r.as_str())));
    let name = shown.and_then(|v| v["title"].as_str()).unwrap_or("").to_string();
    let note = f.note.clone().or_else(|| shown_task(m, &f.r).and_then(already));
    let title = if f.cancel { format!("Cancel {}?", f.r) } else { format!("Mark {} done?", f.r) };
    let go = match (f.cancel, f.busy) {
        (true, false) => kit::btn_danger(t, "finish-go", "Cancel task"),
        (true, true) => kit::btn_danger(t, "finish-go", "Cancelling…").opacity(0.6),
        (false, false) => kit::btn_primary(t, "finish-go", "Mark done"),
        (false, true) => kit::btn_primary(t, "finish-go", "Marking done…").opacity(0.6),
    };
    let go = (!f.busy && note.is_none()).then(|| go.on_click(cx.listener(|m, _, _, cx| confirm_finish(m, cx))));
    kit::modal_box(t, 440.)
        .child(head(t, title, cx))
        .child(
            div()
                .flex()
                .flex_none()
                .flex_col()
                .gap(px(10.))
                .px(px(20.))
                .pb(px(20.))
                .when(!name.is_empty(), |d| d.child(div().id("finish-name").line_height(relative(1.4)).child(name)))
                .children(note.map(|n| div().id("finish-note").text_size(px(12.5)).text_color(t.down).child(n)))
                .children(go.map(|go| div().flex().flex_none().justify_end().pt(px(4.)).child(go))),
        )
        .into_any_element()
}

/// The owner's yes: looks the task up again and only marks it done (or failed) while it's still
/// open; a task finished meanwhile gets a note in the dialog instead.
pub fn confirm_finish(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let Some(Modal::Finish(f)) = &mut m.modal else { return };
    if f.busy {
        return;
    }
    f.busy = true;
    f.note = None;
    let (r, cancel, backend) = (f.r.clone(), f.cancel, m.backend.clone());
    cx.spawn(async move |this, cx| {
        let task = r.clone();
        let answer = cx
            .background_executor()
            .spawn(async move {
                let t = backend.get(&format!("tasks/{task}"), &[]).map_err(|e| e.message)?;
                if let Some(note) = already(&t) {
                    return Err(note);
                }
                let (path, body) = if cancel {
                    (format!("tasks/{task}/fail"), serde_json::json!({"reason": "Cancelled by you"}))
                } else {
                    (format!("tasks/{task}/done"), serde_json::json!({}))
                };
                backend.post(&path, body).map_err(|e| e.message)
            })
            .await;
        let _ = this.update(cx, |m, cx| {
            let Some(Modal::Finish(f)) = &mut m.modal else { return };
            if f.r != r {
                return;
            }
            match answer {
                Ok(_) => {
                    m.modal = None;
                    m.toast(if cancel { format!("Cancelled {r}") } else { format!("Marked {r} done") }, false, cx);
                }
                Err(note) => {
                    f.busy = false;
                    f.note = Some(note);
                }
            }
            m.refresh(cx);
        });
    })
    .detach();
    cx.notify();
}

#[cfg(test)]
mod tests;
