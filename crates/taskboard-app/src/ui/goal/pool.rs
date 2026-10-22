//! The goal page's Devices and Bits sections. Read-only like the rest of the app: a device can be
//! focused (its window raised) and a bit's create link opened; adding and changing devices and bits
//! go through `tb device` and `tb bit`.
use super::*;

/// One device in the aside.
#[derive(Debug, PartialEq)]
pub struct DeviceRow {
    pub name: String,
    /// The name with its kind and target: "dev-a (Android emulator, emulator-5554)".
    pub label: String,
    pub tags: String,
    /// The task that has it (T4), or None when it's free.
    pub held_by: Option<String>,
    /// "Free", "Off", or the holder's title.
    pub state: String,
    pub ours: bool,
    pub can_focus: bool,
    /// In this goal's own pool: its purpose and whether it's reserved here ("for measure, reserved");
    /// reserved for another goal: "reserved for G4".
    pub pool: String,
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
            let mut pool: Vec<String> = vec![];
            if b(d, "in_pool") {
                pool.extend(fmt::opt_s(d, "purpose").map(|p| format!("for {p}")));
                if b(d, "reserved") {
                    pool.push("reserved".into());
                }
            } else if let Some(g) = fmt::opt_s(d, "reserved_for") {
                pool.push(format!("reserved for {g}"));
            }
            DeviceRow {
                pool: pool.join(", "),
                name: s(d, "name").to_string(),
                label: fmt::opt_s(d, "label").unwrap_or(s(d, "name")).to_string(),
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
    /// Its note (what the flag is for).
    pub note: Option<String>,
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
                note: fmt::opt_s(x, "note").map(str::to_string),
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
    let n = rows.len();
    let open = m.goal_page.devices_open;
    let head = side_head(t, "Devices", (n > 0).then(|| n.to_string())).child(div().flex_1()).child(fold_toggle(t, "goal-devices-toggle", open, cx, |m| &mut m.goal_page.devices_open));
    if !open {
        return Some(side_box(t).child(head));
    }
    let mut list = div().flex().flex_col().gap(px(8.)).text_size(px(13.));
    if let Some(n) = fmt::opt_s(&g["devices"], "needs_text") {
        list = list.child(div().text_size(px(12.)).text_color(t.muted).child(format!("Each task: {n}")));
    }
    if let Some(why) = fmt::opt_s(&g["devices"], "empty_why") {
        list = list.child(div().text_size(px(12.)).text_color(t.muted).child(why.to_string()));
    }
    for (ix, d) in rows.into_iter().enumerate() {
        let holder: AnyElement = match d.held_by.clone() {
            Some(r) => {
                let target = r.clone();
                kit::link(t, SharedString::from(format!("dev-holder-{ix}")), r)
                    .font_family(t.mono_font.clone())
                    .text_size(px(11.))
                    .tooltip(kit::tip(d.state.clone()))
                    .on_click(cx.listener(move |m, _, _, cx| m.open_task(target.clone(), cx)))
                    .into_any_element()
            }
            None => div().flex_none().text_size(px(12.)).text_color(if d.state == "Off" { t.faint } else { t.up_fg }).child(d.state.clone()).into_any_element(),
        };
        let name = d.name.clone();
        let busy = m.goal_page.busy.contains(&format!("device-focus:{name}"));
        let sub = [d.tags.clone(), d.pool.clone()].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
        list = list.child(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(div().truncate().child(d.label.clone()))
                        .when(!sub.is_empty(), |x| x.child(div().text_size(px(12.)).text_color(t.muted).truncate().child(sub))),
                )
                .child(holder)
                .when(d.can_focus, |x| {
                    let b = kit::btn_small(t, SharedString::from(format!("dev-focus-{ix}")), if busy { "Focusing…" } else { "Focus" });
                    x.child(if busy { kit::disabled(b) } else { b.on_click(cx.listener(move |m, _, _, cx| focus_device(m, &name, cx))) })
                }),
        );
    }
    Some(side_box(t).child(head.pb(px(8.))).child(list))
}

pub fn bits_aside(t: &Theme, g: &Value) -> Option<Div> {
    let rows = bit_rows(g);
    if rows.is_empty() {
        return None;
    }
    let mut list = div().flex().flex_col().gap(px(10.)).text_size(px(13.));
    for (ix, x) in rows.into_iter().enumerate() {
        let fg = match x.tone {
            "up" => t.up_fg,
            "warn" => t.warn_fg,
            _ => t.muted,
        };
        let mut row = div()
            .flex()
            .flex_col()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(div().flex_1().min_w_0().truncate().font_family(t.mono_font.clone()).text_size(px(12.)).child(format!("⚑ {}", x.name)))
                    .when_some(x.create.clone(), |d, (label, url)| d.child(kit::link(t, SharedString::from(format!("bit-create-{ix}")), label).text_size(px(12.)).on_click(move |_, _, cx| cx.open_url(&url)))),
            )
            .child(div().text_size(px(11.)).font_weight(FontWeight::BOLD).text_color(fg).child(if x.uses.is_empty() { x.state.clone() } else { format!("{} · {}", x.state, x.uses) }));
        if let Some(note) = x.note.clone() {
            row = row.child(div().text_size(px(12.)).text_color(t.muted).child(note));
        }
        list = list.child(row);
    }
    Some(side(t, "Bits", bits_summary(g), list))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[::core::prelude::v1::test]
    fn rows_say_who_has_each_device_and_what_each_bit_needs() {
        let g = json!({
            "tasks": [{"ref": "T4"}],
            "devices": {"needs_text": "ios", "devices": [
                {"name": "sim-a", "label": "sim-a (iOS simulator, iOS 17.5)", "tags": ["ios"], "held_by": {"ref": "T4", "title": "Sim test"}, "can_focus": true},
                {"name": "pixel-7", "tags": ["android", "phone"], "held_by": null, "off": true, "can_focus": false, "in_pool": true, "purpose": "measure", "reserved": true},
                {"name": "pixel-9", "tags": [], "held_by": null, "reserved_for": "G4"},
                {"name": "pixel-8", "tags": [], "held_by": {"ref": "T9", "title": "Other"}}]},
            "bits": {"tool": "Flagsmith", "backend": 2, "made": 1, "list": [
                {"name": "beta-banner", "kind": "local", "made": false, "tasks": ["T4"], "goals": []},
                {"name": "newCheckout", "kind": "backend", "made": false, "note": "Gate the new checkout","create_url": "https://f.example/new?key=newCheckout", "tasks": ["T4"], "goals": ["G1"]},
                {"name": "oldCheckout", "kind": "backend", "made": true, "create_url": null, "tasks": [], "goals": []}]},
        });
        let d = device_rows(&g);
        assert_eq!(d[0], DeviceRow { name: "sim-a".into(), label: "sim-a (iOS simulator, iOS 17.5)".into(), tags: "ios".into(), held_by: Some("T4".into()), state: "Sim test".into(), ours: true, can_focus: true, pool: String::new() });
        assert_eq!((d[1].state.as_str(), d[1].tags.as_str(), d[1].pool.as_str()), ("Off", "android, phone", "for measure, reserved"));
        assert_eq!(d[2].pool, "reserved for G4");
        assert!(!d[3].ours);
        assert_eq!(d[3].label, "pixel-8", "no kind or target: just the name");
        let b = bit_rows(&g);
        assert_eq!((b[0].state.as_str(), b[0].create.clone()), ("Local, not in Flagsmith", None));
        assert_eq!(b[1].note.as_deref(), Some("Gate the new checkout"));
        assert_eq!(b[0].note, None);
        assert_eq!(b[1].create, Some(("Create in Flagsmith".into(), "https://f.example/new?key=newCheckout".into())));
        assert_eq!((b[1].state.as_str(), b[1].uses.as_str()), ("Not created", "T4, G1"));
        assert_eq!((b[2].state.as_str(), b[2].tone), ("Created", "up"));
        assert_eq!(bits_summary(&g).as_deref(), Some("1 of 2 created"));
    }

    #[::core::prelude::v1::test]
    fn a_stopped_goal_and_one_waiting_on_bits_wait_on_you() {
        let g = json!({"tasks": [{"status": "done"}, {"status": "queued"}], "stopped": "Wave 1 is done. Review it, then continue"});
        assert_eq!(goal_state(&g), ("Waiting on you".to_string(), "warn"));
        // Stopped by a failed wave, with a task taken by hand still working: stopped comes first.
        let g = json!({"tasks": [{"status": "working"}, {"status": "queued"}], "stopped": "Wave 1 stopped: T2 failed"});
        assert_eq!(goal_state(&g), ("Waiting on you".to_string(), "warn"));
        let g = json!({"tasks": [{"status": "done"}], "bits_waiting": 1});
        assert_eq!(goal_state(&g), ("Waiting on 1 bit".to_string(), "warn"));
    }
}

