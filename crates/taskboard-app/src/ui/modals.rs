//! The dialog over the main window: the alerts list (read-only; esc or Close closes it, and it
//! closes by itself once nothing needs you). The app has no forms: tasks, goals and issues are
//! made and changed through Claude (`tb`), never typed in here.
use crate::app::{CloseOverlay, MainWindow, alert_row};
use crate::fmt::arr;
use crate::theme::Theme;
use crate::ui::kit;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::cmp::Ordering;

pub enum Modal {
    Alerts,
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
        .child(
            div().flex().justify_end().p(px(16.)).child(
                kit::btn_small(t, "alerts-dismiss-all", "Dismiss all")
                    // The dialog closes by itself once the alerts are gone.
                    .on_click(cx.listener(|m, _, _, cx| crate::app::dismiss_all_alerts(m, cx))),
            ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests;
