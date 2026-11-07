// Excerpt of crates/taskboard-app/src/settings.rs, lines 417-744
    fn account(&self, id: &str) -> Value {
        self.accounts.iter().find(|a| a["id"] == id).cloned().unwrap_or(Value::Null)
    }

    /// GitHub's browser sign-in, while it waits: (code, url).
    fn login(&self) -> Option<(String, String)> {
        let l = &self.account("github")["login"];
        Some((l["code"].as_str()?.to_string(), l["url"].as_str().unwrap_or(taskboardd::accounts::GITHUB_DEVICE_URL).to_string()))
    }

    fn load(&self, fresh: bool, cx: &mut Context<Self>) {
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let q: Vec<(&str, String)> = if fresh { vec![("fresh", "1".into())] } else { vec![] };
            let r = cx.background_executor().spawn(async move { backend.get("accounts", &q) }).await;
            let _ = this.update(cx, |s, cx| {
                match r {
                    Ok(v) => s.got(v, cx),
                    Err(e) => s.error = Some(e.message),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// A fresh `accounts` answer: notes what changed and copies a new sign-in code. GitHub opens only
    /// from the code panel's button, so the code is seen (and known to be copied) before the browser
    /// takes over.
    fn got(&mut self, v: Value, cx: &mut Context<Self>) {
        let before = self.account("github")["connected"] == true;
        self.accounts = v["accounts"].as_array().cloned().unwrap_or_default();
        self.loaded = true;
        self.error = None;
        let gh = self.account("github");
        if !before && gh["connected"] == true && self.opened_code.take().is_some() {
            self.notes.insert("github", (format!("Signed in as @{}.", gh["user"].as_str().unwrap_or("")), false));
        }
        if let Some((code, _)) = self.login().filter(|(c, _)| !c.is_empty())
            && self.opened_code.as_deref() != Some(code.as_str())
        {
            cx.write_to_clipboard(ClipboardItem::new_string(code.clone()));
            self.opened_code = Some(code);
            self.browser_opened = false;
        }
    }

    /// `POST accounts/<path>` for an account; the answer is every account's state.
    fn post(&mut self, id: &'static str, path: String, body: Value, cx: &mut Context<Self>, ok: impl FnOnce(&mut Self, &mut Context<Self>) + 'static) {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(id);
        self.notes.remove(id);
        cx.notify();
        let backend = self.backend.clone();
        cx.spawn(async move |this, cx| {
            let r = cx.background_executor().spawn(async move { backend.post(&path, body) }).await;
            let _ = this.update(cx, |s, cx| {
                s.busy = None;
                match r {
                    Ok(v) => {
                        s.got(v, cx);
                        ok(s, cx);
                    }
                    Err(e) => {
                        s.notes.insert(id, (e.message, true));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn sign_in(&mut self, cx: &mut Context<Self>) {
        self.opened_code = None;
        self.browser_opened = false;
        self.post("github", "accounts/github/login".into(), json!({}), cx, |_, _| {});
    }

    /// Copies the token `gh` already uses; gh's own sign-in isn't changed.
    fn import_gh(&mut self, cx: &mut Context<Self>) {
        self.post("github", "accounts/github/import".into(), json!({}), cx, |s, _| {
            let a = s.account("github");
            s.notes.insert("github", (format!("Using gh's sign-in: {}.", a["detail"].as_str().unwrap_or("connected")), false));
        });
    }

    fn connect(&mut self, id: &'static str, cx: &mut Context<Self>) {
        let body = match id {
            "github" => json!({"token": self.gh_token.text(cx)}),
            "bitbucket" => json!({"email": self.bb_email.text(cx), "token": self.bb_token.text(cx)}),
            "azure" => json!({"organization": self.az_org.text(cx), "token": self.az_token.text(cx)}),
            _ => json!({"token": self.slack_token.text(cx)}),
        };
        self.post(id, format!("accounts/{id}"), body, cx, move |s, cx| {
            let field = match id {
                "github" => &s.gh_token,
                "bitbucket" => &s.bb_token,
                "azure" => &s.az_token,
                _ => &s.slack_token,
            };
            field.clear(cx);
            s.gh_form = false;
            s.bb_form = false;
            s.az_form = false;
            let a = s.account(id);
            s.notes.insert(id, (format!("Connected as {}.", a["detail"].as_str().or(a["user"].as_str()).unwrap_or("you")), false));
        });
    }

    fn copy(&mut self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
        self.copied = Some((text, Instant::now()));
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(1500)).await;
            let _ = this.update(cx, |s, cx| {
                if s.copied.as_ref().is_some_and(|(_, at)| at.elapsed() >= Duration::from_millis(1400)) {
                    s.copied = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// ↩ connects, esc clears the field.
    fn field_keys(&self, id: &'static str, field: &Input, cx: &mut Context<Self>) -> impl Fn(&KeyDownEvent, &mut Window, &mut App) + 'static {
        let entity = cx.entity().downgrade();
        let input = field.field.clone();
        move |ev, _, cx| {
            let ks = &ev.keystroke;
            let outcome = match ks.key.as_str() {
                "enter" => KeyOutcome::Submit,
                "escape" => KeyOutcome::Cancel,
                _ => KeyOutcome::Ignored,
            };
            match outcome {
                KeyOutcome::Submit => {
                    let _ = entity.update(cx, |s, cx| s.connect(id, cx));
                }
                KeyOutcome::Cancel => input.update(cx, |f, cx| f.clear(cx)),
                KeyOutcome::Ignored => cx.propagate(),
            }
        }
    }
}

// ------------------------------------------------------------------ render

const SIDEBAR_W: f32 = 228.;

/// Drags the window from empty space; a double click zooms, like a title bar.
pub(crate) fn drag_window(ev: &MouseDownEvent, window: &mut Window, _: &mut App) {
    if ev.click_count >= 2 {
        window.titlebar_double_click();
    } else {
        window.start_window_move();
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.global::<Theme>().clone();
        let body = match self.view {
            Sec::Accounts => self.accounts_page(&t, window, cx).into_any_element(),
            Sec::History => self.history_page(&t, cx).into_any_element(),
            Sec::Usage => self.usage_page(&t, cx).into_any_element(),
            Sec::Qa => self.qa_page(&t, cx).into_any_element(),
            Sec::Reviewers => self.reviewers_page(&t).into_any_element(),
            Sec::Worktrees => self.worktrees_page(&t).into_any_element(),
            Sec::System => self.system_page(&t, cx).into_any_element(),
        };
        div()
            .id("settings-root")
            .track_focus(&self.focus)
            .key_context("TaskboardSettings")
            .size_full()
            .flex()
            .bg(t.bg)
            .text_color(t.text)
            .font_family(t.ui_font.clone())
            .text_size(px(13.))
            .line_height(px(13. * 1.45))
            .child(self.sidebar(&t, cx))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .child(self.page_header(&t, cx))
                    .when_some(self.error.clone(), |d, e| {
                        d.child(div().mx(px(28.)).mb(px(10.)).px(px(12.)).py(px(8.)).rounded(px(8.)).bg(t.warn_soft).text_color(t.warn_fg).text_size(px(12.)).child(format!("taskboardd: {e}")))
                    })
                    .child(body),
            )
    }
}

impl SettingsWindow {
    fn sidebar(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().id("sections").flex_1().min_h_0().overflow_y_scroll().flex().flex_col().gap(px(1.)).px(px(10.));
        for sec in SECS {
            let on = self.view == sec;
            // amber count: accounts whose sign-in stopped working or lacks scopes
            let n = match sec {
                Sec::Accounts => self.accounts.iter().filter(|a| needs_owner(a)).count(),
                // an update waiting for a restart
                Sec::System => matches!(crate::updater::state(), crate::updater::State::Ready { .. }) as usize,
                _ => 0,
            };
            let hover = t.panel_2;
            list = list.child(
                div()
                    .id(SharedString::from(format!("sec-{}", sec.label().to_lowercase())))
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(9.))
                    .h(px(30.))
                    .px(px(9.))
                    .rounded(px(7.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(t.panel_2).font_weight(FontWeight::SEMIBOLD))
                    .when(!on, |d| d.hover(move |s| s.bg(hover.opacity(0.6))))
                    .child(div().w(px(15.)).flex().justify_center().font_weight(FontWeight::BOLD).text_color(if on { t.accent } else { t.muted }).child(sec.glyph()))
                    .child(div().flex_1().min_w_0().truncate().child(sec.label()))
                    .when(n > 0, |d| {
                        d.child(
                            div().min_w(px(18.)).h(px(18.)).px(px(5.)).rounded(px(9.)).flex().items_center().justify_center().text_size(px(11.)).font_weight(FontWeight::BOLD).bg(t.warn).text_color(t.on_accent).child(n.to_string()),
                        )
                    })
                    .on_click(cx.listener(move |s, _, _, cx| {
                        s.view = sec;
                        if sec == Sec::System {
                            s.load_info(cx);
                        }
                        if sec == Sec::Worktrees {
                            s.load_slots(cx);
                        }
                        if sec == Sec::Usage {
                            s.load_daily(cx);
                        }
                        cx.notify();
                    })),
            );
        }
        let live = self.error.is_none() && self.loaded;
        div()
            .w(px(SIDEBAR_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .bg(t.tint)
            .border_r_1()
            .border_color(t.border)
            // room for the traffic lights; drags the window like a title bar
            .child(div().id("sidebar-drag").h(px(48.)).flex_none().on_mouse_down(MouseButton::Left, drag_window))
            .child(list)
            .child(
                div()
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(7.))
                    .px(px(19.))
                    .py(px(12.))
                    .text_size(px(12.))
                    .text_color(t.muted)
                    .child(kit::dot(if live { t.up } else { t.warn }, 7.))
                    .child(div().flex_1().child(if live { "Live" } else if self.loaded { "Not connected" } else { "Connecting…" }))
                    .when(self.backend.label() == "fake", |d| d.child(kit::tone_pill(t, "goal", "Sample"))),
            )
    }

    fn page_header(&self, t: &Theme, cx: &mut Context<Self>) -> impl IntoElement {
        let on = self.cli;
        let text = t.text;
        div()
            .id("page-header")
            .flex()
            .flex_none()
            .items_end()
            .gap(px(16.))
            .px(px(28.))
            .pt(px(22.))
            .pb(px(14.))
            .on_mouse_down(MouseButton::Left, drag_window)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(div().text_size(px(20.)).line_height(px(26.)).font_weight(FontWeight::BOLD).truncate().child(self.view.label()))
                    .child(div().text_size(px(12.5)).line_height(px(17.)).text_color(t.muted).child(self.view.about())),
            )
            .when(self.view == Sec::Accounts, |d| d.child(
                div()
                    .id("agent-commands")
                    .flex()
                    .flex_none()
                    .items_center()
                    .gap(px(6.))
                    .h(px(28.))
                    .px(px(10.))
                    .rounded(px(7.))
                    .border_1()
                    .text_size(px(12.))
                    .cursor_pointer()
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .map(|d| if on { d.border_color(t.accent).bg(t.accent_soft).text_color(t.text) } else { d.border_color(t.border).bg(t.card).text_color(t.muted).hover(move |s| s.text_color(text)) })
                    .tooltip(kit::tip("Show the commands agents run with these accounts"))
                    .on_click(cx.listener(|s, _, _, cx| {
                        s.cli = !s.cli;
                        cx.notify();
                    }))
                    .child(div().font_family(t.mono_font.clone()).text_color(if on { t.accent } else { t.muted }).child("</>"))
                    .child("Agent commands"),
            ))
    }

