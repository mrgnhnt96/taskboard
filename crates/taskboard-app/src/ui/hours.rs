//! Work hours: the status-bar pill (`hoursPill`: "Work hours 9am–5pm", "Agents off until Thu 6am")
//! and its menu (`hoursMenuHtml`: the on/off check, From / to, the days, "Today until"). Every
//! change is saved at once (`saveHours`: `POST /hours`, `today_until` only when it changed).
use crate::app::{MainWindow, Pill};
use crate::fmt::{self, arr, b, s};
use crate::theme::Theme;
use crate::ui::kit;
use gpui_kit::prelude::*;
use gpui_kit::*;
use serde_json::{Value, json};

pub const DAYS: [(&str, &str); 7] = [("sun", "S"), ("mon", "M"), ("tue", "T"), ("wed", "W"), ("thu", "T"), ("fri", "F"), ("sat", "S")];
const MENU: &str = "hours";
/// A time list opened inside the menu: which select.
const SELECTS: [&str; 3] = ["start", "end", "today"];

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
    /// The select whose option list is open.
    pub open: Option<&'static str>,
}

impl State {
    /// `openHoursMenu`.
    pub fn from_hours(h: &Value) -> State {
        State {
            on: b(h, "on"),
            start: s(h, "start").into(),
            end: s(h, "end").into(),
            days: arr(h, "days").iter().filter_map(|d| d.as_str().map(str::to_string)).collect(),
            today: s(h, "today_until").into(),
            err: None,
            open: None,
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

/// `hourOptions(cur)`: every half hour, plus `cur` when it's off the grid; (value, label).
pub fn hour_options(cur: &str) -> Vec<(String, String)> {
    let mut times: Vec<String> = (0..48).map(|k| format!("{:02}:{:02}", k / 2, (k % 2) * 30)).collect();
    if !cur.is_empty() && !times.iter().any(|x| x == cur) {
        times.push(cur.to_string());
        times.sort();
    }
    times.into_iter().map(|v| (v.clone(), fmt::clock12(&v))).collect()
}

/// The "Today until" options: "Usual end" first.
pub fn today_options(cur: &str) -> Vec<(String, String)> {
    let mut v = vec![(String::new(), "Usual end".to_string())];
    v.extend(hour_options(cur));
    v
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
            if let Some(st) = m.data.state.as_mut() {
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

/// A select: the chosen option's label and ▾; it opens its option list in place.
fn select(t: &Theme, which: &'static str, value: &str, options: &[(String, String)], enabled: bool, open: bool, cx: &mut Context<MainWindow>) -> Div {
    let label = options.iter().find(|(v, _)| v == value).map(|(_, l)| l.clone()).unwrap_or_default();
    let button = div()
        .id(SharedString::from(format!("hours-{which}")))
        .flex()
        .flex_1()
        .min_w_0()
        .items_center()
        .justify_between()
        .gap(px(4.))
        .h(px(40.))
        .px(px(12.))
        .rounded(px(9.))
        .border_1()
        .border_color(if open { t.accent } else { t.border_2 })
        .bg(t.card)
        .text_size(px(14.))
        .child(div().min_w_0().truncate().child(label))
        .child(kit::icon(kit::Icon::Chev, 12., t.muted))
        .when(!enabled, |d| d.opacity(0.5))
        .when(enabled, |d| {
            d.cursor_pointer().on_click(cx.listener(move |m, _, _, cx| {
                m.hours.open = if m.hours.open == Some(which) { None } else { Some(which) };
                cx.notify();
            }))
        });
    let mut col = div().flex().flex_col().flex_1().min_w_0().child(button);
    if open {
        let mut list = div().id(SharedString::from(format!("hours-{which}-list"))).flex().flex_col().max_h(px(200.)).overflow_y_scroll().mt(px(4.)).p(px(2.)).rounded(px(6.)).border_1().border_color(t.border).bg(t.card);
        for (v, l) in options {
            let (v2, sel) = (v.clone(), v == value);
            list = list.child(kit::menu_item(t, SharedString::from(format!("hours-{which}-{v}")), l.clone(), sel).h(px(26.)).on_click(cx.listener(move |m, _, _, cx| {
                match which {
                    "start" => m.hours.start = v2.clone(),
                    "end" => m.hours.end = v2.clone(),
                    _ => m.hours.today = v2.clone(),
                }
                m.hours.open = None;
                save(m, cx);
                cx.notify();
            })));
        }
        col = col.child(list);
    }
    col
}

pub fn render_menu(m: &mut MainWindow, _window: &mut Window, cx: &mut Context<MainWindow>) -> Option<AnyElement> {
    let at = m.menu_open(MENU)?;
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
    for (d, l) in DAYS {
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
            .when(sel, |x| x.bg(t.card).shadow(crate::ui::sidebar::shadow_seg(&t)))
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
    let (start_o, end_o, today_o) = (hour_options(&st.start), hour_options(&st.end), today_options(&st.today));
    let (start, end, today, open, err) = (st.start.clone(), st.end.clone(), st.today.clone(), st.open, st.err.clone());
    let menu = kit::menu_box(&t, 280.)
        .id("hours-menu")
        .shadow(crate::ui::sidebar::shadow_modal(&t))
        .p(px(10.))
        .gap(px(10.))
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .child(check)
        .child(
            row()
                .items_start()
                .child(div().pt(px(10.)).child("From"))
                .child(select(&t, SELECTS[0], &start, &start_o, on, open == Some("start"), cx))
                .child(div().pt(px(10.)).child("to"))
                .child(select(&t, SELECTS[1], &end, &end_o, on, open == Some("end"), cx)),
        )
        .child(days)
        .child(row().items_start().child(div().pt(px(10.)).child("Today until")).child(select(&t, SELECTS[2], &today, &today_o, on, open == Some("today"), cx)))
        .children(err.map(|e| div().text_size(px(12.5)).font_weight(FontWeight::MEDIUM).text_color(t.down).child(e)));
    // Opens upward from the pill.
    Some(deferred(anchored().anchor(Anchor::BottomLeft).position(at).snap_to_window_with_margin(px(8.)).child(menu)).with_priority(3).into_any_element())
}

#[cfg(test)]
mod tests {
    use super::{DAYS, State, hour_options, pill_view, save, today_options};
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
        golden("chrome").only("hoursMenu").check(|i| {
            let m = &i["menu"];
            let st = State {
                on: b(m, "on"),
                start: s(m, "start").into(),
                end: s(m, "end").into(),
                days: arr(m, "days").iter().filter_map(|d| d.as_str().map(str::to_string)).collect(),
                today: s(m, "today").into(),
                err: fmt::opt_s(m, "err").map(str::to_string),
                open: None,
            };
            let sel = |opts: Vec<(String, String)>, cur: &str| {
                json!({"disabled": !st.on, "options": opts.iter().map(|(v, l)| json!([v, l])).collect::<Vec<_>>(),
                       "selected": opts.iter().any(|(v, _)| v == cur).then(|| cur.to_string())})
            };
            json!({
                "on": st.on,
                "check": "Only start agents during work hours",
                "start": sel(hour_options(&st.start), &st.start),
                "end": sel(hour_options(&st.end), &st.end),
                "today": sel(today_options(&st.today), &st.today),
                "days": DAYS.iter().map(|(d, l)| json!({"day": d, "on": st.days.iter().any(|x| x == d), "title": fmt::cap(d), "disabled": !st.on, "label": l})).collect::<Vec<_>>(),
                "err": st.err,
                "labels": ["From", "to", "Today until"],
            })
        });
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
                err: None,
                open: None,
            };
            json!([["/hours", st.body(i["today_until"].as_str().unwrap_or(""))]])
        });
        g.only("openHoursMenu").check(|i| {
            let st = State::from_hours(&i["hours"]);
            json!({"on": st.on, "start": st.start, "end": st.end, "days": st.days, "today": st.today})
        });
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
}
