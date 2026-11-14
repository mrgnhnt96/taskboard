//! The dialog over the main window: the alerts list (read-only; esc or Close closes it, and it
//! closes by itself once nothing needs you), the New idea form (`ui::ideas`), and the confirm for a
//! link menu's Mark done / Cancel task… and the PR's Merge now (which names the holds it skips and
//! keeps a refusal on screen). Tasks, goals and issues are made and changed through
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
    /// The menu skips Midna's approval, so nothing happens until the owner confirms here. Also the
    /// PR's Merge now, the owner's override.
    Finish(Finish),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ask {
    Done,
    Cancel,
    /// Merge the task's PR past whatever holds it (`tb pr merge --override`).
    Merge,
}

pub struct Finish {
    pub r: String,
    pub ask: Ask,
    /// Why it can't go ahead: it's already done, or the board refused.
    pub note: Option<String>,
    pub busy: bool,
    /// A merge that was tried and refused: the dialog keeps the answer until it's closed.
    pub refused: Option<Refused>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Refused {
    pub why: String,
    /// When it was refused, "3:16 PM".
    pub at: String,
}

impl Modal {
    pub fn finish(r: String, ask: Ask) -> Modal {
        Modal::Finish(Finish { r, ask, note: None, busy: false, refused: None })
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
        m.set_modal(None, window, cx);
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
            .track_focus(&m.modal_focus)
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

/// Esc in the dialog closes its top layer: a menu open in it, else the dialog.
pub fn on_escape(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    if m.modal.is_some() {
        m.escape(window, cx);
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

/// Why a task that's no longer open can't be marked done or cancelled, or its PR merged.
fn already(t: &serde_json::Value, ask: Ask) -> Option<String> {
    let r = t["ref"].as_str().unwrap_or("It");
    if ask == Ask::Merge {
        let p = &t["pr"];
        let state = p["state"].as_str().unwrap_or("OPEN").to_uppercase();
        return if p["num"].is_null() {
            Some(format!("{r} has no PR."))
        } else if state == "MERGED" {
            Some(format!("PR #{} already merged.", p["num"]))
        } else if state != "OPEN" {
            Some(format!("PR #{} isn't open.", p["num"]))
        } else {
            None
        };
    }
    if t["failed"] == true {
        Some(format!("{r} already failed."))
    } else if t["status"] == "done" {
        Some(format!("{r} is already done."))
    } else {
        None
    }
}

/// What an open PR's merge would skip: the checks still running or failing, and the review it
/// still waits for.
pub(crate) fn holds(pr: &serde_json::Value) -> Vec<String> {
    let mut v = vec![];
    match pr["checks"].as_str() {
        Some("pending") => v.push("CI, still running".to_string()),
        Some("fail") => v.push("CI, failing".to_string()),
        _ => {}
    }
    let (have, need) = (pr["approvals"]["have"].as_i64().unwrap_or(0), pr["approvals"]["need"].as_i64().unwrap_or(0));
    match pr["review"].as_str() {
        Some("changes") => v.push("Review, changes asked for".to_string()),
        Some("pending") if need > have => v.push(format!("Review, {have} of {need} approvals")),
        Some("pending") => v.push("Review, not approved yet".to_string()),
        _ => {}
    }
    v
}

/// Who refused a merge: the PR's host, else the board.
fn refuser(pr: &serde_json::Value) -> &'static str {
    match pr["host"].as_str().map(str::to_lowercase).as_deref() {
        Some("github") => "GitHub",
        Some("bitbucket") => "Bitbucket",
        Some("gitlab") => "GitLab",
        _ => "The board",
    }
}

/// The step after a refused merge, when the reason names one.
fn after_refusal(why: &str) -> Option<&'static str> {
    let w = why.to_lowercase();
    (w.contains("behind") || w.contains("conflict") || w.contains("out of date") || w.contains("not up to date")).then_some("Rebase it from its terminal, then merge again.")
}

/// A merge the host refused: what it said and when, and what to do next. Close is the only button.
fn refused_view(t: &Theme, pr_word: &str, pr: Option<&serde_json::Value>, r: &Refused, cx: &mut Context<MainWindow>) -> AnyElement {
    let by = pr.map_or("The board", refuser);
    let next = after_refusal(&r.why);
    kit::modal_box(t, 440.)
        .child(head(t, format!("{pr_word} didn't merge"), cx))
        .child(
            div()
                .flex()
                .flex_none()
                .flex_col()
                .gap(px(10.))
                .px(px(20.))
                .pb(px(20.))
                .line_height(relative(1.4))
                .child(
                    div()
                        .id("finish-refused")
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .p(px(12.))
                        .rounded(px(8.))
                        .border_1()
                        .border_color(t.down_line)
                        .bg(t.down_soft)
                        .text_size(px(13.))
                        .child(div().font_weight(FontWeight::SEMIBOLD).text_color(t.down).child(format!("{by} refused the merge at {}", r.at)))
                        .child(div().child(r.why.clone())),
                )
                .children(next.map(|n| div().id("finish-next").child(n))),
        )
        .into_any_element()
}

fn finish(m: &MainWindow, t: &Theme, f: &Finish, cx: &mut Context<MainWindow>) -> AnyElement {
    let shown = shown_task(m, &f.r).or(m.data.task_card.as_ref().filter(|c| c["ref"].as_str() == Some(f.r.as_str())));
    let note = f.note.clone().or_else(|| shown_task(m, &f.r).and_then(|t| already(t, f.ask)));
    let pr_facts = shown.map(|v| &v["pr"]).filter(|p| p.is_object());
    let num = pr_facts.and_then(|p| p["num"].as_i64());
    if let Some(r) = &f.refused {
        let word = num.map(|n| format!("#{n}")).unwrap_or_else(|| "The PR".into());
        return refused_view(t, &word, pr_facts, r, cx);
    }
    let title = match f.ask {
        Ask::Cancel => format!("Cancel {}?", f.r),
        Ask::Done => format!("Mark {} done?", f.r),
        Ask::Merge => format!("Merge {} without waiting?", num.map(|n| format!("#{n}")).unwrap_or_else(|| "its PR".into())),
    };
    // What the yes skips or changes, not the task's name again.
    let does = what_it_does(f.ask);
    let go = match (f.ask, f.busy) {
        (Ask::Cancel, false) => kit::btn_danger(t, "finish-go", "Cancel task"),
        (Ask::Cancel, true) => kit::btn_danger(t, "finish-go", "Cancelling…").opacity(0.6),
        (Ask::Done, false) => kit::btn_primary(t, "finish-go", "Mark done"),
        (Ask::Done, true) => kit::btn_primary(t, "finish-go", "Marking done…").opacity(0.6),
        (Ask::Merge, false) => kit::btn_danger(t, "finish-go", "Merge now"),
        (Ask::Merge, true) => kit::btn_danger(t, "finish-go", "Merging…").opacity(0.6),
    };

    let merge = f.ask == Ask::Merge;
    // The button stays while the answer is on its way, saying so; it goes only when it can't go ahead.
    let go = match (f.busy, note.is_none()) {
        (true, _) => Some(go),
        (false, true) => Some(go.on_click(cx.listener(|m, _, _, cx| confirm_finish(m, cx)))),
        (false, false) => None,
    };
    let skips = if merge && note.is_none() { pr_facts.map(holds).unwrap_or_default() } else { vec![] };
    let warn = (merge && note.is_none()).then(|| {
        let mut d = div().id("finish-merge").flex().flex_col().gap(px(6.)).line_height(relative(1.4));
        if !skips.is_empty() {
            d = d.child(div().child("It merges now and skips:")).children(skips.into_iter().map(|h| div().flex().gap(px(8.)).pl(px(4.)).child(div().text_color(t.muted).child("•")).child(h)));
        }
        d.child(div().text_color(t.muted).child("You can't undo a merge from the app."))
    });
    let cancel = (merge && go.is_some() && !f.busy).then(|| kit::btn(t, "finish-cancel", "Cancel").on_click(cx.listener(|m, _, window, cx| close(m, window, cx))));
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
                .when(warn.is_none(), |d| d.child(div().id("finish-does").line_height(relative(1.4)).text_color(t.text_2).child(does)))
                .children(warn)
                .children(note.map(|n| div().id("finish-note").text_size(px(12.5)).text_color(t.down).child(n)))
                .children(go.map(|go| div().flex().flex_none().justify_end().gap(px(8.)).pt(px(4.)).children(cancel).child(go))),
        )
        .into_any_element()
}

/// The one line a Finish dialog says: what the yes skips or changes.
pub fn what_it_does(ask: Ask) -> &'static str {
    match ask {
        Ask::Cancel => "Marks it failed as cancelled by you, without waiting for its agent.",
        Ask::Done => "Marks it done now, without waiting for its agent to hand it in.",
        Ask::Merge => "Merges past its checks and reviews; a conflict with its base still stops it.",
    }
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
    let (r, ask, backend) = (f.r.clone(), f.ask, m.backend.clone());
    cx.spawn(async move |this, cx| {
        let task = r.clone();
        let answer = cx
            .background_executor()
            .spawn(async move {
                let t = backend.get(&format!("tasks/{task}"), &[]).map_err(|e| (false, e.message))?;
                if let Some(note) = already(&t, ask) {
                    return Err((false, note));
                }
                let (path, body) = match ask {
                    Ask::Cancel => (format!("tasks/{task}/fail"), serde_json::json!({"reason": "Cancelled by you"})),
                    Ask::Done => (format!("tasks/{task}/done"), serde_json::json!({})),
                    Ask::Merge => (format!("tasks/{task}/pr/merge"), serde_json::json!({"override": true})),
                };
                backend.post(&path, body).map_err(|e| (true, e.message))
            })
            .await;
        let _ = this.update(cx, |m, cx| {
            let said = match ask {
                Ask::Cancel => format!("Cancelled {r}"),
                Ask::Done => format!("Marked {r} done"),
                Ask::Merge => format!("Merged {r}'s PR"),
            };
            // Closed while it was busy: the answer still shows, as a toast.
            let Some(Modal::Finish(f)) = m.modal.as_mut().filter(|x| matches!(x, Modal::Finish(f) if f.r == r)) else {
                match answer {
                    Ok(_) => m.toast(said, false, cx),
                    Err((_, note)) => m.toast(note, true, cx),
                }
                m.refresh(cx);
                return;
            };
            match answer {
                Ok(_) => {
                    m.modal = None;
                    m.toast(said, false, cx);
                }
                // A merge the host refused keeps its answer in the dialog.
                Err((true, why)) if ask == Ask::Merge => {
                    f.busy = false;
                    f.refused = Some(Refused { why: crate::ui::status::plain_error(&why), at: chrono::Local::now().format("%-I:%M %p").to_string() });
                }
                Err((_, note)) => {
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
