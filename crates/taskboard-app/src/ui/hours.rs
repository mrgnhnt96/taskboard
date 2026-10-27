//! Work hours: the status-bar pill (`hoursPill`: "Work hours 9am–5pm", "Agents off until Thu 6am")
//! and its menu (`hoursMenuHtml`: the on/off check, From / to, the days, "Today until"). Every
//! change is saved at once (`saveHours`: `POST /hours`, `today_until` only when it changed).
//! The times are typed ("7p", "730pm", "17:30") with an am/pm chip beside each.
use crate::app::{MainWindow, Pill};
use crate::fmt::{self, arr, b, s};
use crate::theme::Theme;
use crate::ui::kit;
use crate::ui::text_input::FieldChanged;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};

pub const DAYS: [(&str, &str); 7] = [("sun", "S"), ("mon", "M"), ("tue", "T"), ("wed", "W"), ("thu", "T"), ("fri", "F"), ("sat", "S")];
const MENU: &str = "hours";
/// The menu's time fields, by index.
const START: usize = 0;
const END: usize = 1;
const TODAY: usize = 2;

/// The menu's edits (a copy of `work_hours` taken when it opened: `openHoursMenu`).
#[derive(Default)]
pub struct State {
    pub on: bool,
    pub start: String,
    pub end: String,
    pub days: Vec<String>,
    /// "" = the usual end.
    pub today: String,
    pub err: Option<String>,
    /// The half each time field's chip shows (true = pm): From, to, Today until.
    pub pm: [bool; 3],
    /// The time fields, made when the menu first draws.
    pub fields: Option<Fields>,
    /// The day the week starts on (`work_hours.week_days[0]`, config.toml's `first_weekday`).
    pub first_day: String,
}

pub struct Fields {
    inputs: [kit::Input; 3],
    _subs: Vec<Subscription>,
}

/// The day buttons in week order, starting on `first` (`sun` when it isn't a day).
pub fn week_order(first: &str) -> Vec<(&'static str, &'static str)> {
    let k = DAYS.iter().position(|(d, _)| *d == first).unwrap_or(0);
    (0..7).map(|i| DAYS[(k + i) % 7]).collect()
}

impl State {
    /// `openHoursMenu`.
    pub fn from_hours(h: &Value) -> State {
        let mut st = State {
            on: b(h, "on"),
            start: s(h, "start").into(),
            end: s(h, "end").into(),
            days: arr(h, "days").iter().filter_map(|d| d.as_str().map(str::to_string)).collect(),
            today: s(h, "today_until").into(),
            first_day: arr(h, "week_days").first().and_then(|d| d.as_str()).unwrap_or("sun").into(),
            ..Default::default()
        };
        // An unset "Today until" is usually an evening, so its chip starts on pm.
        st.pm = [0, 1, 2].map(|i| shown(st.value(i)).1.unwrap_or(i == TODAY));
        st
    }

    /// A time field's value ("HH:MM", "" for the usual end).
    pub fn value(&self, i: usize) -> &str {
        match i {
            START => &self.start,
            END => &self.end,
            _ => &self.today,
        }
    }

    fn value_mut(&mut self, i: usize) -> &mut String {
        match i {
            START => &mut self.start,
            END => &mut self.end,
            _ => &mut self.today,
        }
    }

    /// `saveHours`'s body: `today_until` only when it differs from the board's (`""` → `"off"`).
    pub fn body(&self, current_today: &str) -> Value {
        let mut body = json!({"on": self.on, "start": self.start, "end": self.end, "days": self.days});
        if self.today != current_today {
            body["today_until"] = json!(if self.today.is_empty() { "off".to_string() } else { self.today.clone() });
        }
        body
    }

    /// `hours-day`: toggle a day (added at the end, as the web did).
    pub fn toggle_day(&mut self, d: &str) {
        if let Some(i) = self.days.iter().position(|x| x == d) {
            self.days.remove(i);
        } else {
            self.days.push(d.to_string());
        }
    }
}

/// `hoursPill`, as data (`cls` is `hours` or `hours closed`).
pub fn pill_view(h: &Value) -> Pill {
    let (cls, label) = if !b(h, "on") {
        ("hours", "No work hours".to_string())
    } else if b(h, "open") && fmt::opt_s(h, "today_until").is_some() {
        ("hours", format!("Work hours until {} today", fmt::clock12(s(h, "today_until"))))
    } else if b(h, "open") {
        ("hours", format!("Work hours {}–{}", fmt::clock12(s(h, "start")), fmt::clock12(s(h, "end"))))
    } else {
        let when = fmt::opt_s(h, "next_open").map(fmt::day_clock).unwrap_or_default();
        ("hours closed", format!("Agents off until {when}").trim_end().to_string())
    };
    Pill { cls, label, title: s(h, "line").to_string() }
}

/// A value as its field shows it: "19:30" → ("7:30", Some(pm)); "" → ("", None).
pub fn shown(hhmm: &str) -> (String, Option<bool>) {
    let mut it = hhmm.split(':').map(|p| p.parse::<u32>().ok());
    let (Some(Some(h)), Some(Some(m))) = (it.next(), it.next()) else {
        return (String::new(), None);
    };
    (format!("{}:{m:02}", if h % 12 == 0 { 12 } else { h % 12 }), Some(h >= 12))
}

/// Splits a typed time into its digits and the half it names: "7:30p" → ("7:30", Some(pm)).
fn split_half(text: &str) -> (String, Option<bool>) {
    let t: String = text.to_lowercase().chars().filter(|c| !c.is_whitespace() && *c != '.').collect();
    for (suffix, pm) in [("am", false), ("pm", true), ("a", false), ("p", true)] {
        if let Some(core) = t.strip_suffix(suffix) {
            return (core.to_string(), Some(pm));
        }
    }
    (t, None)
}

/// The half a typed time names, if it names one ("7p" → pm), so the chip can follow the typing.
pub fn typed_half(text: &str) -> Option<bool> {
    split_half(text).1
}

/// Reads a typed time: "7", "730", "0730", "7:30", "7p", "7:30 pm", "17:30". A 1–12 hour with
/// no am/pm takes the chip's half (`pm`); 0 and 13–23 are read as a 24-hour clock.
/// Returns "HH:MM".
pub fn parse_time(text: &str, pm: bool) -> Option<String> {
    let (core, half) = split_half(text);
    let (h, m) = match core.split_once(':') {
        Some((h, "")) if !h.is_empty() => (h, "00"),
        Some((h, m)) if !h.is_empty() && m.len() == 2 => (h, m),
        Some(_) => return None,
        None => match core.len() {
            1 | 2 => (core.as_str(), "00"),
            3 | 4 => core.split_at(core.len() - 2),
            _ => return None,
        },
    };
    if !h.chars().chain(m.chars()).all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    if m > 59 || h > 23 {
        return None;
    }
    let h = match half {
        Some(_) if h == 0 || h > 12 => return None,
        Some(pm) => h % 12 + if pm { 12 } else { 0 },
        None if h == 0 || h > 12 => h,
        None => h % 12 + if pm { 12 } else { 0 },
    };
    Some(format!("{h:02}:{m:02}"))
}

/// The grey rest drawn after what's typed while it reads as a time: "7" → ":00", "7:3" → nothing.
pub fn ghost(text: &str, pm: bool) -> Option<String> {
    let typed = text.trim();
    if typed.is_empty() || typed_half(typed).is_some() {
        return None;
    }
    let full = shown(&parse_time(typed, pm)?).0;
    full.strip_prefix(typed).filter(|rest| !rest.is_empty()).map(str::to_string)
}

pub fn pill(m: &mut MainWindow, t: &Theme, cx: &mut Context<MainWindow>) -> AnyElement {
    let Some(h) = m.state().get("work_hours").filter(|v| v.is_object()).cloned() else {
        return div().into_any_element();
    };
    let p = pill_view(&h);
    let closed = p.cls == "hours closed";
    let (fg, bg, dot) = if closed { (t.warn_fg, t.warn_soft, t.warn) } else { (t.muted, t.seg, t.up) };
    let open = m.menu_open(MENU).is_some();
    div()
        .id("hours-pill")
        .flex()
        .items_center()
        .gap(px(6.))
        .py(px(3.))
        .px(px(10.))
        .line_height(px(18.75))
        .rounded_full()
        .border_1()
        .border_color(if open { fg } else { gpui_kit::transparent_black() })
        .hover(move |d| d.border_color(fg))
        .cursor_pointer()
        .text_size(px(12.5))
        .whitespace_nowrap()
        .text_color(fg)
        .bg(bg)
        .child(kit::dot(dot, 8.))
        .child(p.label)
        .child(div().ml(px(-1.)).opacity(0.65).child(kit::icon(kit::Icon::Chev, 14., fg)))
        .tooltip(kit::tip(p.title))
        .on_click(cx.listener(move |m, e: &ClickEvent, _, cx| {
            if m.menu_open(MENU).is_none() {
                m.hours = State::from_hours(&h);
            }
            let p = e.position();
            m.toggle_menu(MENU, point(p.x - px(12.), p.y - px(14.)), cx);
        }))
        .into_any_element()
}

/// `saveHours`.
pub fn save(m: &mut MainWindow, cx: &mut Context<MainWindow>) {
    let body = m.hours.body(s(&m.state()["work_hours"], "today_until"));
    m.post_or(
        "hours",
        body,
        cx,
        |m, v, cx| {
            if let Some(st) = m.data.state.as_mut().map(std::rc::Rc::make_mut) {
                st["work_hours"] = v;
            }
            m.hours.err = None;
            cx.notify();
        },
        |m, e, cx| {
            m.hours.err = Some(e.message);
            cx.notify();
        },
    );
}

/// The menu's time fields, filled with the values; typing an am/pm moves the chip, leaving a
/// field saves it.
fn make_fields(st: &State, window: &mut Window, cx: &mut Context<MainWindow>) -> Fields {
    let inputs = [START, END, TODAY].map(|i| kit::Input::with_text(cx, if i == TODAY { "Usual end" } else { "" }, false, &shown(st.value(i)).0));
    let mut subs = Vec::new();
    for (i, input) in inputs.iter().enumerate() {
        subs.push(cx.subscribe(&input.field, move |m: &mut MainWindow, field, _: &FieldChanged, cx| {
            if let Some(pm) = typed_half(field.read(cx).text()) {
                m.hours.pm[i] = pm;
            }
            cx.notify();
        }));
        subs.push(cx.on_blur(&input.focus, window, move |m, _, cx| commit(m, i, cx)));
    }
    Fields { inputs, _subs: subs }
}

/// The menu closed: keep what was being typed, then drop the fields.
fn close_fields(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) {
    let Some(f) = m.hours.fields.as_ref() else { return };
    if let Some(i) = f.inputs.iter().position(|input| input.focus.is_focused(window)) {
        commit(m, i, cx);
        window.focus(&m.focus, cx);
    }
    m.hours.fields = None;
}

/// Show field `i`'s value again (its chip too), dropping what was typed.
fn show_value(m: &mut MainWindow, i: usize, cx: &mut Context<MainWindow>) {
    let (text, pm) = shown(m.hours.value(i));
    if let Some(pm) = pm {
        m.hours.pm[i] = pm;
    }
    if let Some(f) = m.hours.fields.as_ref() {
        f.inputs[i].set_text(&text, cx);
    }
}

/// Take what's typed in field `i`: a time saves (when it changed), an empty "Today until" is the
/// usual end, anything else puts the value back.
fn commit(m: &mut MainWindow, i: usize, cx: &mut Context<MainWindow>) {
    let Some(f) = m.hours.fields.as_ref() else { return };
    let text = f.inputs[i].text(cx);
    let next = if i == TODAY && text.trim().is_empty() { Some(String::new()) } else { parse_time(&text, m.hours.pm[i]) };
    let changed = next.filter(|v| v != m.hours.value(i)).map(|v| *m.hours.value_mut(i) = v).is_some();
    show_value(m, i, cx);
    if changed {
        save(m, cx);
    }
    cx.notify();
}

/// The chip: flip field `i` between am and pm, keeping the time typed.
fn flip(m: &mut MainWindow, i: usize, cx: &mut Context<MainWindow>) {
    m.hours.pm[i] = !m.hours.pm[i];
    let Some(f) = m.hours.fields.as_ref() else { return };
    let digits = split_half(&f.inputs[i].text(cx)).0;
    f.inputs[i].set_text(&digits, cx);
    commit(m, i, cx);
}

/// A typed time: the text, the grey rest of the time while typing, and the am/pm chip ("Today
/// until" shows no chip while it's the usual end). ↩ takes it, esc puts the value back.
fn time_field(m: &MainWindow, t: &Theme, i: usize, window: &Window, cx: &mut Context<MainWindow>) -> Stateful<Div> {
    let (on, pm) = (m.hours.on, m.hours.pm[i]);
    let input = &m.hours.fields.as_ref().expect("made before the menu draws").inputs[i];
    let focused = input.focus.is_focused(window);
    let text = input.text(cx);
    let rest = if focused { ghost(&text, pm) } else { None };
    let chip = (i != TODAY || focused || !text.is_empty()).then(|| {
        div()
            .id(("hours-half", i))
            .flex_none()
            .flex()
            .items_center()
            .h(px(28.))
            .px(px(7.))
            .rounded(px(6.))
            .bg(if focused { t.accent_line } else { t.seg })
            .text_color(if focused { t.accent_fg } else { t.text_2 })
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(if pm { "pm" } else { "am" })
            .when(on, |d| {
                d.cursor_pointer().on_click(cx.listener(move |m, _, _, cx| {
                    cx.stop_propagation();
                    flip(m, i, cx);
                }))
            })
    });
    let focus = input.focus.clone();
    let typed = div()
        .relative()
        .flex_1()
        .min_w_0()
        .overflow_hidden()
        .child(input.field.clone())
        .children(rest.map(|rest| {
            div()
                .absolute()
                .top_0()
                .left_0()
                .flex()
                .whitespace_nowrap()
                .child(div().text_color(gpui_kit::transparent_black()).child(text))
                .child(div().text_color(t.faint).child(rest))
        }));
    div()
        .id(("hours-time", i))
        .relative()
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .gap(px(4.))
        .h(px(40.))
        .pl(px(10.))
        .pr(px(4.))
        .rounded(px(9.))
        .border_1()
        .border_color(if focused { t.accent } else { t.border_2 })
        .bg(t.card)
        .text_size(px(14.))
        .line_height(px(20.))
        .text_color(t.text)
        .child(typed)
        .children(chip)
        .when(!on, |d| d.opacity(0.5).child(div().id(("hours-time-off", i)).absolute().inset_0().occlude()))
        .when(on, |d| d.cursor_text().on_click(move |_, window, cx| window.focus(&focus, cx)))
        .on_key_down(cx.listener(move |m, ev: &KeyDownEvent, window, cx| {
            let Some(f) = m.hours.fields.as_ref() else { return };
            match f.inputs[i].on_key(ev, cx) {
                kit::KeyOutcome::Submit => {
                    commit(m, i, cx);
                    window.focus(&m.focus, cx);
                }
                kit::KeyOutcome::Cancel => {
                    show_value(m, i, cx);
                    window.focus(&m.focus, cx);
                    cx.stop_propagation();
                }
                kit::KeyOutcome::Ignored => {}
            }
        }))
}

/// Midna's keep-awake, read-only (changed with `tb keep-awake`): whether the Mac is held awake now,
/// and its schedule when it's on.
pub struct KeepAwake {
    pub held: bool,
    pub line: String,
    pub schedule: Option<String>,
}

pub fn keep_awake_view(k: &Value) -> Option<KeepAwake> {
    let line = fmt::opt_s(k, "line")?.to_string();
    let schedule = if b(&k["settings"], "enabled") { fmt::opt_s(k, "schedule").map(str::to_string) } else { None };
    Some(KeepAwake { held: b(k, "held"), line, schedule })
}

fn keep_awake(t: &Theme, k: KeepAwake) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .pt(px(10.))
        .px(px(4.))
        .border_t_1()
        .border_color(t.border)
        .text_size(px(12.5))
        .child(div().flex().items_start().gap(px(6.)).child(div().flex_none().pt(px(5.)).child(kit::dot(if k.held { t.up } else { t.border_2 }, 8.))).child(div().flex_1().min_w_0().child(k.line)))
        .children(k.schedule.map(|s| div().pl(px(14.)).text_color(t.muted).child(s)))
}

pub fn render_menu(m: &mut MainWindow, window: &mut Window, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let Some(at) = m.menu_open(MENU) else {
        close_fields(m, window, cx);
        return None;
    };
    let t = cx.global::<Theme>().clone();
    let st = &m.hours;
    let on = st.on;
    // `.hours-row`: 8px gaps, 13px, 0 4px.
    let row = || div().flex().items_center().gap(px(8.)).px(px(4.)).text_size(px(13.));
    let check = div()
        .id("hours-on")
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(4.))
        .text_size(px(13.))
        .cursor_pointer()
        .child(
            div()
                .size(px(16.))
                .rounded(px(4.))
                .border_1()
                .border_color(if on { t.accent_btn } else { t.border_2 })
                .bg(if on { t.accent_btn } else { t.card })
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.on_accent)
                .text_size(px(11.))
                .child(if on { "✓" } else { "" }),
        )
        .child("Only start agents during work hours")
        .on_click(cx.listener(|m, _, _, cx| {
            m.hours.on = !m.hours.on;
            save(m, cx);
            cx.notify();
        }));
    // `.seg.sm`: 3px padding, radius 9; buttons 30px, radius 7, 13px semibold.
    let mut days = div().flex().gap(px(2.)).p(px(3.)).rounded(px(9.)).bg(t.seg);
    for (d, l) in week_order(&st.first_day) {
        let sel = st.days.iter().any(|x| x == d);
        let mut item = div()
            .id(SharedString::from(format!("hours-day-{d}")))
            .flex_1()
            .h(px(30.))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .text_size(px(13.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(if sel { t.text } else { t.muted })
            .when(sel, |x| x.bg(t.card).shadow(crate::ui::kit::shadow_seg(&t)))
            .when(!on, |x| x.opacity(0.5))
            .tooltip(kit::tip(fmt::cap(d)))
            .child(l);
        if on {
            item = item.cursor_pointer().on_click(cx.listener(move |m, _, _, cx| {
                m.hours.toggle_day(d);
                save(m, cx);
                cx.notify();
            }));
        }
        days = days.child(item);
    }
    let err = st.err.clone();
    if m.hours.fields.is_none() {
        m.hours.fields = Some(make_fields(&m.hours, window, cx));
    }
    let menu = kit::menu_box(&t, 280.)
        .id("hours-menu")
        .shadow(crate::ui::kit::shadow_modal(&t))
        .p(px(10.))
        .gap(px(10.))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(check)
        .child(
            row()
                .child("From")
                .child(time_field(m, &t, START, window, cx))
                .child("to")
                .child(time_field(m, &t, END, window, cx)),
        )
        .child(days)
        .child(row().child(div().whitespace_nowrap().child("Today until")).child(time_field(m, &t, TODAY, window, cx)))
        .children(err.map(|e| div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(t.down).child(e)))
        .children(m.state().get("keep_awake").and_then(keep_awake_view).map(|k| keep_awake(&t, k)));
    // Opens upward from the pill.
    Some(deferred(anchored().anchor(Anchor::BottomLeft).position(at).snap_to_window_with_margin(px(8.)).child(menu)).with_priority(3).into_any_element())
}

#[cfg(test)]
mod tests {
    use super::{DAYS, State, ghost, parse_time, pill_view, save, shown, typed_half, week_order};
    use crate::fmt::{self, arr, b, s};
    use crate::parity::golden;
    use serde_json::json;

    #[::core::prelude::v1::test]
    fn pill_matches_web() {
        golden("chrome").only("hoursPill").check(|i| {
            let p = pill_view(&i["hours"]);
            json!({"closed": p.cls == "hours closed", "label": p.label, "title": p.title})
        });
    }

    #[::core::prelude::v1::test]
    fn menu_matches_web() {
        // The time lists left the web's on purpose (`time_lists_*` below); the rest still matches.
        let mut g = golden("chrome").only("hoursMenu");
        for c in &mut g.cases {
            for k in ["start", "end", "today"] {
                c["expect"][k].as_object_mut().unwrap().remove("options");
            }
        }
        g.check(|i| {
            let m = &i["menu"];
            let st = State {
                on: b(m, "on"),
                start: s(m, "start").into(),
                end: s(m, "end").into(),
                days: arr(m, "days").iter().filter_map(|d| d.as_str().map(str::to_string)).collect(),
                today: s(m, "today").into(),
                err: fmt::opt_s(m, "err").map(str::to_string),
                first_day: "sun".into(),
                ..Default::default()
            };
            let sel = |cur: &str, usual: bool| json!({"disabled": !st.on, "selected": (usual || !cur.is_empty()).then(|| cur.to_string())});
            json!({
                "on": st.on,
                "check": "Only start agents during work hours",
                "start": sel(&st.start, false),
                "end": sel(&st.end, false),
                "today": sel(&st.today, true),
                "days": DAYS.iter().map(|(d, l)| json!({"day": d, "on": st.days.iter().any(|x| x == d), "title": fmt::cap(d), "disabled": !st.on, "label": l})).collect::<Vec<_>>(),
                "err": st.err,
                "labels": ["From", "to", "Today until"],
            })
        });
    }

    #[::core::prelude::v1::test]
    fn typed_times_read_loosely() {
        let am = |t: &str| parse_time(t, false);
        let pm = |t: &str| parse_time(t, true);
        assert_eq!(pm("7").as_deref(), Some("19:00"), "a bare hour takes the chip's half");
        assert_eq!(am("7").as_deref(), Some("07:00"));
        assert_eq!(am("7p").as_deref(), Some("19:00"), "a typed half beats the chip");
        assert_eq!(pm("7:30 AM").as_deref(), Some("07:30"));
        assert_eq!(am("730pm").as_deref(), Some("19:30"));
        assert_eq!(am("0730").as_deref(), Some("07:30"));
        assert_eq!(am("1145p.m.").as_deref(), Some("23:45"));
        assert_eq!(am("17:30").as_deref(), Some("17:30"), "13–23 is a 24-hour time");
        assert_eq!(pm("0:15").as_deref(), Some("00:15"));
        assert_eq!(am("12a").as_deref(), Some("00:00"));
        assert_eq!(am("12p").as_deref(), Some("12:00"));
        assert_eq!(pm("12").as_deref(), Some("12:00"));
        for bad in ["", "7:3", "7:60", "24", "13pm", "0am", "12345", "seven", ":30", "7:30x"] {
            assert_eq!(am(bad), None, "{bad:?}");
        }
    }

    #[::core::prelude::v1::test]
    fn fields_show_twelve_hour_times() {
        assert_eq!(shown("19:30"), ("7:30".into(), Some(true)));
        assert_eq!(shown("00:00"), ("12:00".into(), Some(false)));
        assert_eq!(shown("12:00"), ("12:00".into(), Some(true)));
        assert_eq!(shown(""), (String::new(), None));
        assert_eq!(typed_half("7 P"), Some(true));
        assert_eq!(typed_half("7:30"), None);
        let st = State::from_hours(&json!({"start": "09:00", "end": "17:00", "today_until": ""}));
        assert_eq!(st.pm, [false, true, true], "an unset Today until starts on pm");
    }

    #[::core::prelude::v1::test]
    fn ghost_finishes_the_time_being_typed() {
        assert_eq!(ghost("7", true).as_deref(), Some(":00"));
        assert_eq!(ghost("7:", true).as_deref(), Some("00"));
        assert_eq!(ghost("12", false).as_deref(), Some(":00"));
        assert_eq!(ghost("7:30", true), None, "nothing left to finish");
        assert_eq!(ghost("730", true), None, "digits that don't lead the time get none");
        assert_eq!(ghost("7p", true), None);
        assert_eq!(ghost("", true), None);
    }

    #[::core::prelude::v1::test]
    fn save_body_matches_web() {
        let g = golden("chrome");
        g.only("saveHours").check(|i| {
            let m = &i["menu"];
            let st = State {
                on: b(m, "on"),
                start: s(m, "start").into(),
                end: s(m, "end").into(),
                days: arr(m, "days").iter().filter_map(|d| d.as_str().map(str::to_string)).collect(),
                today: s(m, "today").into(),
                first_day: "sun".into(),
                ..Default::default()
            };
            json!([["/hours", st.body(i["today_until"].as_str().unwrap_or(""))]])
        });
        g.only("openHoursMenu").check(|i| {
            let st = State::from_hours(&i["hours"]);
            json!({"on": st.on, "start": st.start, "end": st.end, "days": st.days, "today": st.today})
        });
    }

    #[::core::prelude::v1::test]
    fn days_follow_the_first_weekday() {
        let order = |f: &str| week_order(f).iter().map(|(d, _)| *d).collect::<Vec<_>>();
        assert_eq!(order("sun"), DAYS.iter().map(|(d, _)| *d).collect::<Vec<_>>());
        assert_eq!(order("mon"), vec!["mon", "tue", "wed", "thu", "fri", "sat", "sun"]);
        assert_eq!(order("nope")[0], "sun");
        let st = State::from_hours(&json!({"on": true, "days": ["mon"], "week_days": ["mon", "tue", "wed", "thu", "fri", "sat", "sun"]}));
        assert_eq!(st.first_day, "mon");
        assert_eq!(State::from_hours(&json!({"on": true})).first_day, "sun");
    }

    #[::core::prelude::v1::test]
    fn keep_awake_shows_midnas_line() {
        let k = super::keep_awake_view(&json!({"held": true, "line": "Keeping awake until 6 PM: 1 agent working",
            "schedule": "9 AM–6 PM weekdays", "settings": {"enabled": true}})).unwrap();
        assert!(k.held);
        assert_eq!(k.schedule.as_deref(), Some("9 AM–6 PM weekdays"));
        let off = super::keep_awake_view(&json!({"held": false, "line": "Keep-awake is off", "schedule": "9 AM–6 PM weekdays", "settings": {"enabled": false}})).unwrap();
        assert_eq!(off.schedule, None, "no schedule while it's off");
        assert!(super::keep_awake_view(&json!(null)).is_none());
    }

    #[gpui_kit::test]
    fn changing_a_day_posts_hours(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = crate::parity::window(cx);
        w.update(cx, |m, _, cx| {
            let h = m.state()["work_hours"].clone();
            m.hours = State::from_hours(&h);
            m.hours.on = true;
            m.hours.toggle_day("sat");
            save(m, cx);
        })
        .unwrap();
        crate::parity::settle(cx);
        let body = rec.last("hours").expect("posted /hours");
        assert_eq!(body["on"], json!(true));
        assert!(body["days"].as_array().unwrap().contains(&json!("sat")));
        assert!(body.get("today_until").is_none(), "today_until only when it changed");
    }

    #[gpui_kit::test]
    fn typing_a_time_posts_hours(cx: &mut gpui_kit::TestAppContext) {
        let (w, rec) = crate::parity::window(cx);
        let typed = |field: usize, text: &'static str, cx: &mut gpui_kit::TestAppContext| {
            w.update(cx, |m, window, cx| {
                if m.hours.fields.is_none() {
                    let h = m.state()["work_hours"].clone();
                    m.hours = State::from_hours(&h);
                    m.hours.on = true;
                    m.hours.fields = Some(super::make_fields(&m.hours, window, cx));
                    m.toggle_menu(super::MENU, gpui_kit::point(gpui_kit::px(0.), gpui_kit::px(0.)), cx);
                }
                m.hours.fields.as_ref().unwrap().inputs[field].set_text(text, cx);
                super::commit(m, field, cx);
            })
            .unwrap();
            crate::parity::settle(cx);
        };
        typed(super::TODAY, "6p", cx);
        assert_eq!(rec.last("hours").expect("posted /hours")["today_until"], json!("18:00"));
        typed(super::START, "8:30", cx);
        assert_eq!(rec.last("hours").unwrap()["start"], json!("08:30"), "the chip's am");
        typed(super::END, "nonsense", cx);
        w.update(cx, |m, _, cx| {
            let text = m.hours.fields.as_ref().unwrap().inputs[super::END].text(cx);
            assert_eq!(text, super::shown(&m.hours.end).0, "a bad time puts the value back");
        })
        .unwrap();
    }
}
