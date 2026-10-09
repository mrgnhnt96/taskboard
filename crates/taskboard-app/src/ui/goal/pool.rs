//! The goal page's Devices and Bits asides. Read-only like the rest of the app: devices can be
//! focused (their window raised) and bits' names copied or their tool's "new flag" page opened; adding
//! devices and bits, and recording a bit made, go through `tb device` and `tb bit`.
use super::*;

/// One device in the aside.
#[derive(Debug, PartialEq)]
pub struct DeviceRow {
    pub name: String,
    pub tags: String,
    /// The task that has it (T4), or None when it's free.
    pub held_by: Option<String>,
    /// "Free", "Off", or the holder's title.
    pub state: String,
    pub ours: bool,
    pub can_focus: bool,
}

pub fn device_rows(g: &Value) -> Vec<DeviceRow> {
    let ours: Vec<String> = arr(g, "tasks").iter().map(|t| fmt::ref_of(t, "T")).collect();
    arr(&g["devices"], "devices")
        .iter()
        .map(|d| {
            let held_by = fmt::opt_s(&d["held_by"], "ref").map(str::to_string);
            let state = match &held_by {
                Some(_) => s(&d["held_by"], "title").to_string(),
                None if b(d, "off") => "Off".into(),
                None => "Free".into(),
            };
            DeviceRow {
                name: s(d, "name").to_string(),
                tags: arr(d, "tags").iter().filter_map(|x| x.as_str()).collect::<Vec<_>>().join(", "),
                ours: held_by.as_ref().is_some_and(|r| ours.contains(r)),
                held_by,
                state,
                can_focus: b(d, "can_focus"),
            }
        })
        .collect()
}

/// One bit in the aside: its name, and what it needs (made, a create link, or local).
#[derive(Debug, PartialEq)]
pub struct BitRow {
    pub name: String,
    /// "Created", "Not created" or "Local, not in Flagsmith".
    pub state: String,
    /// up, warn or muted.
    pub tone: &'static str,
    /// "Create in Flagsmith" and its link, for a backend bit not made yet.
    pub create: Option<(String, String)>,
    pub uses: String,
}

pub fn bit_rows(g: &Value) -> Vec<BitRow> {
    let tool = s(&g["bits"], "tool").to_string();
    arr(&g["bits"], "list")
        .iter()
        .map(|x| {
            let local = s(x, "kind") == "local";
            let made = b(x, "made");
            let (state, tone) = if local {
                (format!("Local, not in {tool}"), "muted")
            } else if made {
                ("Created".to_string(), "up")
            } else {
                ("Not created".to_string(), "warn")
            };
            let uses: Vec<&str> = ["tasks", "goals"].iter().flat_map(|k| arr(x, k).iter().filter_map(|v| v.as_str())).collect();
            BitRow {
                name: s(x, "name").to_string(),
                state,
                tone,
                create: (!local && !made).then(|| fmt::opt_s(x, "create_url").map(|u| (format!("Create in {tool}"), u.to_string()))).flatten(),
                uses: uses.join(", "),
            }
        })
        .collect()
}

/// "1 of 3 created" (backend bits).
pub fn bits_summary(g: &Value) -> Option<String> {
    let n = i(&g["bits"], "backend");
    (n > 0).then(|| format!("{} of {n} created", i(&g["bits"], "made")))
}

fn focus_device(m: &mut MainWindow, name: &str, cx: &mut Context<MainWindow>) {
    let n = name.to_string();
    run(m, format!("device-focus:{name}"), None, true, format!("devices/{name}/focus"), json!({}), cx, move |_, _, _| format!("Raising {n}"));
}

pub fn devices_aside(m: &MainWindow, t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Option<Div> {
    if !g["devices"].is_object() {
        return None;
    }
    let rows = device_rows(g);
    let mut head = div().flex().items_center().gap(px(8.)).child(h3(t, 12.5, "Devices"));
    if let Some(n) = fmt::opt_s(&g["devices"], "needs_text") {
        head = head.child(div().flex_1()).child(div().text_size(px(12.)).text_color(t.muted).child(format!("Each task: {n}")));
    }
    let mut list = div().flex().flex_col();
    for (ix, d) in rows.into_iter().enumerate() {
        let busy = m.goal_page.busy.contains(&format!("device-focus:{}", d.name));
        let holder: AnyElement = match d.held_by.clone() {
            Some(r) => {
                let (fg, bg) = if d.ours { (t.accent_fg, t.accent_soft) } else { (t.text_2, t.col) };
                let target = r.clone();
                div()
                    .id(SharedString::from(format!("dev-holder-{ix}")))
                    .flex_none()
                    .cursor_pointer()
                    .px(px(5.))
                    .rounded(px(5.))
                    .bg(bg)
                    .text_color(fg)
                    .font_family(t.mono_font.clone())
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_size(px(11.5))
                    .line_height(relative(1.5))
                    .hover(|s| s.underline())
                    .child(r)
                    .tooltip(kit::tip(d.state.clone()))
                    .on_click(cx.listener(move |m, _, _, cx| m.open_task(target.clone(), cx)))
                    .into_any_element()
            }
            None => div().flex_none().text_size(px(12.)).text_color(if d.state == "Off" { t.faint } else { t.up_fg }).child(d.state.clone()).into_any_element(),
        };
        let name = d.name.clone();
        list = list.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .py(px(6.))
                .when(ix > 0, |x| x.border_t_1().border_color(t.divider))
                .text_size(px(13.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_family(t.mono_font.clone()).font_weight(FontWeight::SEMIBOLD).truncate().child(d.name.clone()))
                        .when(!d.tags.is_empty(), |x| x.child(div().text_size(px(12.)).text_color(t.muted).truncate().child(d.tags.clone()))),
                )
                .child(holder)
                .when(d.can_focus, |x| {
                    let b = kit::btn_small(t, SharedString::from(format!("dev-focus-{ix}")), if busy { "Focusing…" } else { "Focus" });
                    x.child(if busy { kit::disabled(b) } else { b.on_click(cx.listener(move |m, _, _, cx| focus_device(m, &name, cx))) })
                }),
        );
    }
    Some(aside_card(t).child(head).child(list))
}

pub fn bits_aside(t: &Theme, g: &Value, cx: &mut Context<MainWindow>) -> Option<Div> {
    let rows = bit_rows(g);
    if rows.is_empty() {
        return None;
    }
    let mut head = div().flex().items_center().gap(px(8.)).child(h3(t, 12.5, "Bits"));
    if let Some(sum) = bits_summary(g) {
        head = head.child(div().flex_1()).child(div().text_size(px(12.)).text_color(t.muted).child(sum));
    }
    let mut list = div().flex().flex_col();
    for (ix, x) in rows.into_iter().enumerate() {
        let (fg, bg) = match x.tone {
            "up" => (t.up_fg, t.up_soft),
            "warn" => (t.warn_fg, t.warn_soft),
            _ => (t.muted, t.panel_2),
        };
        let copy = x.name.clone();
        let mut line = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .child(div().flex_none().text_color(t.warn_fg).child("⚑"))
            .child(div().flex_1().min_w_0().font_family(t.mono_font.clone()).font_weight(FontWeight::SEMIBOLD).truncate().child(x.name.clone()))
            .child(
                kit::btn_small(t, SharedString::from(format!("bit-copy-{ix}")), "Copy")
                    .tooltip(kit::tip(format!("Copy {}", x.name)))
                    .on_click(cx.listener(move |m, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.clone()));
                        m.toast(format!("Copied {copy}"), false, cx);
                    })),
            );
        if let Some((label, url)) = x.create.clone() {
            line = line.child(kit::btn_small(t, SharedString::from(format!("bit-create-{ix}")), label).on_click(move |_, _, cx| cx.open_url(&url)));
        }
        let mut meta = div().flex().items_center().gap(px(8.)).pl(px(18.)).text_size(px(12.)).child(chip(fg, bg, x.state.clone()));
        if !x.uses.is_empty() {
            meta = meta.child(div().text_color(t.muted).truncate().child(x.uses.clone()));
        }
        list = list.child(div().flex().flex_col().gap(px(4.)).py(px(6.)).when(ix > 0, |d| d.border_t_1().border_color(t.divider)).child(line).child(meta));
    }
    Some(aside_card(t).child(head).child(list))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn rows_say_who_has_each_device_and_what_each_bit_needs() {
        let g = json!({
            "tasks": [{"ref": "T4"}],
            "devices": {"needs_text": "ios", "devices": [
                {"name": "sim-a", "tags": ["ios"], "held_by": {"ref": "T4", "title": "Sim test"}, "can_focus": true},
                {"name": "pixel-7", "tags": ["android", "phone"], "held_by": null, "off": true, "can_focus": false},
                {"name": "pixel-8", "tags": [], "held_by": {"ref": "T9", "title": "Other"}}]},
            "bits": {"tool": "Flagsmith", "backend": 2, "made": 1, "list": [
                {"name": "beta-banner", "kind": "local", "made": false, "tasks": ["T4"], "goals": []},
                {"name": "newCheckout", "kind": "backend", "made": false, "create_url": "https://f.example/new?key=newCheckout", "tasks": ["T4"], "goals": ["G1"]},
                {"name": "oldCheckout", "kind": "backend", "made": true, "create_url": null, "tasks": [], "goals": []}]},
        });
        let d = device_rows(&g);
        assert_eq!(d[0], DeviceRow { name: "sim-a".into(), tags: "ios".into(), held_by: Some("T4".into()), state: "Sim test".into(), ours: true, can_focus: true });
        assert_eq!((d[1].state.as_str(), d[1].tags.as_str()), ("Off", "android, phone"));
        assert!(!d[2].ours);
        let b = bit_rows(&g);
        assert_eq!((b[0].state.as_str(), b[0].create.clone()), ("Local, not in Flagsmith", None));
        assert_eq!(b[1].create, Some(("Create in Flagsmith".into(), "https://f.example/new?key=newCheckout".into())));
        assert_eq!((b[1].state.as_str(), b[1].uses.as_str()), ("Not created", "T4, G1"));
        assert_eq!((b[2].state.as_str(), b[2].tone), ("Created", "up"));
        assert_eq!(bits_summary(&g).as_deref(), Some("1 of 2 created"));
    }

    #[::core::prelude::v1::test]
    fn a_stopped_goal_and_one_waiting_on_bits_wait_on_you() {
        let g = json!({"tasks": [{"status": "done"}, {"status": "queued"}], "stopped": "Wave 1 is done. Review it, then continue"});
        assert_eq!(goal_state(&g), ("Waiting on you".to_string(), "warn"));
        let g = json!({"tasks": [{"status": "done"}], "bits_waiting": 1});
        assert_eq!(goal_state(&g), ("Waiting on 1 bit".to_string(), "warn"));
    }
}

