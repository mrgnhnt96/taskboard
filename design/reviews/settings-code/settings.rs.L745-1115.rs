// Excerpt of crates/taskboard-app/src/settings.rs, lines 745-1115
    fn accounts_page(&self, t: &Theme, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = div().id("settings-rows").flex_1().min_h_0().overflow_y_scroll().track_scroll(&self.scroll).flex().flex_col().gap(px(20.)).px(px(28.)).pt(px(4.)).pb(px(28.));
        if !self.loaded && self.error.is_none() {
            return list.child(kit::empty(t, "Asking the board…"));
        }
        for (id, mark, color, about) in CARDS {
            let a = self.account(id);
            let rows: Vec<AnyElement> = match id {
                "github" => self.github_rows(t, &a, window, cx),
                "bitbucket" => self.bitbucket_rows(t, &a, window, cx),
                "azure" => self.azure_rows(t, &a, window, cx),
                _ => self.slack_rows(t, &a, window, cx),
            };
            let mut card = kit::card(t).overflow_hidden();
            for r in rows {
                card = card.child(r);
            }
            if let Some((text, bad)) = self.notes.get(id) {
                card = card.child(row(t, &format!("{id}-note"), false).bg(if *bad { t.down_soft } else { t.up_soft }).text_color(if *bad { t.down } else { t.up_fg }).text_size(px(12.)).child(text.clone()));
            }
            if self.cli {
                card = card.child(self.cli_line(t, id, "tb accounts", cx)).child(self.cli_line(t, &format!("{id}-use"), agent_cli(id), cx));
            }
            let label = a["label"].as_str().unwrap_or(id).to_string();
            let heading = div()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(4.))
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(t.muted)
                .child(
                    div()
                        .size(px(18.))
                        .flex()
                        .flex_none()
                        .items_center()
                        .justify_center()
                        .rounded(px(5.))
                        .bg(rgb(color))
                        .text_color(t.on_accent)
                        .text_size(px(9.))
                        .font_weight(FontWeight::BOLD)
                        .child(mark),
                )
                .child(div().text_color(t.text).child(label))
                .child(div().font_weight(FontWeight::NORMAL).text_size(px(11.5)).truncate().child(format!("· {about}")));
            list = list.child(div().flex().flex_col().gap(px(8.)).child(heading).child(card));
        }
        list.child(
            div()
                .px(px(4.))
                .text_size(px(11.5))
                .line_height(px(16.))
                .text_color(t.muted)
                .child("Tokens are checked with the service, then kept in your login Keychain as Taskboard's own. Your gh and git sign-ins aren't changed. The board never shows tokens again; agents get them with tb token, and push with them while on a task."),
        )
    }

    // -------------------------------------------------------------- accounts

    fn github_rows(&self, t: &Theme, a: &Value, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let busy = self.busy == Some("github");
        let has_gh = a["gh"] != false;
        let mut rows = vec![];
        if let Some((code, url)) = self.login() {
            rows.push(self.github_code(t, code, url, cx));
            return rows;
        }
        if a["connected"] == true {
            rows.push(self.signed_in_row(t, "github", a, "Taskboard's own token", "Disconnect", cx));
            if a["reauth"] == true {
                let btn = if !has_gh {
                    kit::btn_primary(t, "gh-reauth", "Use a new token").h(px(26.)).text_size(px(12.)).on_click(cx.listener(|s, _, window, cx| {
                        s.gh_form = true;
                        window.focus(&s.gh_token.focus, cx);
                        cx.notify();
                    }))
                } else if busy {
                    kit::disabled(kit::btn_primary(t, "gh-reauth", "Starting…").h(px(26.)).text_size(px(12.)))
                } else {
                    kit::btn_primary(t, "gh-reauth", "Sign in again").h(px(26.)).text_size(px(12.)).on_click(cx.listener(|s, _, _, cx| s.sign_in(cx)))
                };
                rows.push(reauth_row(t, "github", a, "Sign in again (or use a token that has them).", btn.into_any_element()));
            }
            rows.push(push_row(t, "github", "github.com"));
        } else {
            let sign_in = if busy { kit::disabled(kit::btn_primary(t, "gh-sign-in", "Starting…").h(px(26.)).text_size(px(12.))) } else {
                kit::btn_primary(t, "gh-sign-in", "Sign in with GitHub").h(px(26.)).text_size(px(12.)).on_click(cx.listener(|s, _, _, cx| s.sign_in(cx)))
            };
            let note = if has_gh {
                "Opens GitHub in your browser with a one-time code. Taskboard keeps its own token; gh's sign-in isn't changed."
            } else {
                "Paste a token, or install the GitHub CLI (brew install gh) to sign in with the browser."
            };
            rows.push(
                line(t, "gh-out", true, false, "Not signed in", note, None)
                    .child(
                        controls()
                            .when(has_gh, |d| {
                                d.child(
                                    kit::btn_small(t, "gh-import", "Use my gh sign-in")
                                        .tooltip(kit::tip("Copies the token gh already uses into Taskboard. gh isn't changed."))
                                        .on_click(cx.listener(|s, _, _, cx| s.import_gh(cx))),
                                )
                            })
                            .when(!self.gh_form, |d| {
                                d.child(kit::btn_small(t, "gh-use-token", "Use a token").on_click(cx.listener(|s, _, window, cx| {
                                    s.gh_form = true;
                                    window.focus(&s.gh_token.focus, cx);
                                    cx.notify();
                                })))
                            })
                            .when(has_gh, |d| d.child(sign_in)),
                    )
                    .into_any_element(),
            );
        }
        if self.gh_form {
            rows.push(self.token_row(t, "github", "Token", "A classic token with repo, read:org and workflow, or a fine-grained one with Contents, Pull requests and Workflows (read and write).", &self.gh_token, window, cx, a["connected"] != true));
        }
        rows.extend(error_row(t, "github", a));
        rows
    }

    fn bitbucket_rows(&self, t: &Theme, a: &Value, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![];
        let connected = a["connected"] == true;
        if connected {
            rows.push(self.signed_in_row(t, "bitbucket", a, "API token in the Keychain (taskboard-bitbucket)", "Disconnect", cx));
            if a["reauth"] == true && !self.bb_form {
                let btn = kit::btn_primary(t, "bb-reauth", "Use a new token").h(px(26.)).text_size(px(12.)).on_click(cx.listener(|s, _, window, cx| {
                    s.bb_form = true;
                    window.focus(&s.bb_email.focus, cx);
                    cx.notify();
                }));
                rows.push(reauth_row(t, "bitbucket", a, "Make a new API token with them and connect it.", btn.into_any_element()));
            } else if let Some(note) = a["note"].as_str() {
                rows.push(off_row(t, "bitbucket", a, note));
            }
            rows.push(push_row(t, "bitbucket", "bitbucket.org"));
        }
        if !connected || self.bb_form {
            let email = self.bb_email.render(t, "bb-email", window).w(px(260.)).on_key_down(self.field_keys("bitbucket", &self.bb_email, cx));
            rows.push(line(t, "bb-email-row", !connected, false, "Atlassian email", "The email you sign in to Bitbucket with.", None).child(email).into_any_element());
            rows.push(self.token_row(
                t,
                "bitbucket",
                "API token",
                "An Atlassian API token with Bitbucket scopes: read and write repository, read and write pullrequest, read:user for your name, write:pipeline to cancel builds.",
                &self.bb_token,
                window,
                cx,
                false,
            ));
            rows.push(link_row(t, "bb-create", "Create an API token", taskboardd::accounts::BITBUCKET_TOKEN_URL, cx));
        }
        rows.extend(error_row(t, "bitbucket", a));
        rows
    }

    fn azure_rows(&self, t: &Theme, a: &Value, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![];
        let connected = a["connected"] == true;
        if connected {
            rows.push(self.signed_in_row(t, "azure", a, "Personal access token in the Keychain (taskboard-azure-devops)", "Disconnect", cx));
            if a["reauth"] == true && !self.az_form {
                let btn = kit::btn_primary(t, "az-reauth", "Use a new token").h(px(26.)).text_size(px(12.)).on_click(cx.listener(|s, _, window, cx| {
                    s.az_form = true;
                    window.focus(&s.az_token.focus, cx);
                    cx.notify();
                }));
                rows.push(reauth_row(t, "azure", a, "Edit the token in Azure DevOps (or make a new one) to add them, and connect it.", btn.into_any_element()));
            } else if let Some(note) = a["note"].as_str() {
                rows.push(off_row(t, "azure", a, note));
            }
        }
        if !connected || self.az_form {
            let org = self.az_org.render(t, "az-org", window).w(px(260.)).on_key_down(self.field_keys("azure", &self.az_org, cx));
            rows.push(line(t, "az-org-row", !connected, false, "Organization", "The organization your pipelines run in.", None).child(org).into_any_element());
            rows.push(self.token_row(t, "azure", "Personal access token", "A token with Build (Read) and Test Management (Read).", &self.az_token, window, cx, false));
            rows.push(link_row(t, "az-create", "How to make a personal access token", taskboardd::accounts::AZURE_TOKEN_URL, cx));
        }
        rows.extend(error_row(t, "azure", a));
        rows
    }

    fn slack_rows(&self, t: &Theme, a: &Value, window: &Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut rows = vec![];
        if a["connected"] == true {
            rows.push(self.signed_in_row(t, "slack", a, "Token in the Keychain (taskboard-slack)", "Disconnect", cx));
        } else {
            rows.push(self.token_row(
                t,
                "slack",
                "Token",
                "A user (xoxp-) or bot (xoxb-) token from your Slack app's OAuth & Permissions page.",
                &self.slack_token,
                window,
                cx,
                true,
            ));
            rows.push(link_row(t, "slack-apps", "Open your Slack apps", taskboardd::accounts::SLACK_APPS_URL, cx));
        }
        rows.extend(error_row(t, "slack", a));
        rows
    }

    /// GitHub's browser sign-in: the code (already on the clipboard, and it says so), then a button
    /// that opens github.com/login/device, then what happens next.
    fn github_code(&self, t: &Theme, code: String, url: String, cx: &mut Context<Self>) -> AnyElement {
        // 1 is done as soon as the code is copied; 2 is what's left.
        let done = div().size(px(22.)).flex().flex_none().items_center().justify_center().rounded_full().bg(t.up_soft).text_color(t.up_fg).text_size(px(12.)).font_weight(FontWeight::BOLD).child("✓");
        let step = |n: &str, on: bool| {
            div()
                .size(px(22.))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .rounded_full()
                .text_size(px(12.))
                .font_weight(FontWeight::BOLD)
                .map(|d| if on { d.bg(t.accent_btn).text_color(t.on_accent) } else { d.bg(t.card).border_1().border_color(t.accent_line).text_color(t.accent_fg) })
                .child(n.to_string())
        };
        let head = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(div().flex_1().text_size(px(14.)).font_weight(FontWeight::SEMIBOLD).child("Finish signing in on GitHub"))
            .child(kit::btn_small(t, "gh-cancel", "Cancel").on_click(cx.listener(|s, _, _, cx| {
                s.opened_code = None;
                s.browser_opened = false;
                s.post("github", "accounts/github/cancel".into(), json!({}), cx, |_, _| {});
            })));
        let panel = div().id("gh-code").flex().flex_col().gap(px(14.)).px(px(16.)).py(px(14.)).bg(t.accent_soft).child(head);
        if code.is_empty() {
            return panel.child(div().text_size(px(12.5)).text_color(t.muted).child("Getting a code from GitHub…")).into_any_element();
        }
        let flashed = self.copied.as_ref().is_some_and(|(c, _)| *c == code);
        let c = code.clone();
        let chip = div()
            .id("gh-code-chip")
            .px(px(14.))
            .py(px(6.))
            .rounded(px(9.))
            .border_1()
            .border_color(t.accent_line)
            .bg(t.card)
            .font_family(t.mono_font.clone())
            .text_size(px(24.))
            .line_height(px(30.))
            .font_weight(FontWeight::SEMIBOLD)
            .cursor_pointer()
            .tooltip(kit::tip("Copy it again"))
            .on_click(cx.listener(move |s, _, _, cx| s.copy(c.clone(), cx)))
            .child(code.clone());
        let copied = kit::pill(t.up_fg, t.up_soft, if flashed { "✓ Copied again" } else { "✓ Copied to your clipboard" }).h(px(26.)).px(px(11.)).text_size(px(12.5));
        let opened = self.browser_opened;
        let open = kit::btn_primary(t, "gh-open", if opened { "Open GitHub again ↗" } else { "Open GitHub and paste it ↗" }).on_click(cx.listener(move |s, _, _, cx| {
            if let Some((code, _)) = s.login() {
                cx.write_to_clipboard(ClipboardItem::new_string(code));
            }
            cx.open_url(&url);
            s.browser_opened = true;
            cx.notify();
        }));
        let line1 = div().flex().items_center().gap(px(12.)).child(done).child(chip).child(copied);
        let line2 = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(step("2", true))
            .child(open)
            .child(div().flex_1().min_w_0().text_size(px(12.5)).text_color(t.text_2).child("Press ⌘V in the code boxes, then Continue and Authorize."));
        let after = if opened {
            "Waiting for you to approve on GitHub. This updates by itself. Page didn't load? Open it again: the code works for 15 minutes."
        } else {
            "Nothing to type: the code is already copied, so you only paste it."
        };
        panel.child(line1).child(line2).child(div().text_size(px(11.5)).line_height(px(15.)).text_color(t.muted).child(after)).into_any_element()
    }

    /// "Name · detail · scopes" with Check and Sign out / Disconnect.
    fn signed_in_row(&self, t: &Theme, id: &'static str, a: &Value, how: &str, out: &str, cx: &mut Context<Self>) -> AnyElement {
        let busy = self.busy == Some(id);
        let name = a["name"].as_str().or(a["user"].as_str()).unwrap_or("Connected").to_string();
        let scopes: Vec<&str> = a["scopes"].as_array().map(|s| s.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
        let detail = a["detail"].as_str().unwrap_or("");
        let note = [detail, how].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(" · ");
        let check = kit::btn_small(t, SharedString::from(format!("{id}-check")), if busy { "Checking…" } else { "Check" });
        let check = if busy { kit::disabled(check) } else { check.tooltip(kit::tip("Ask the service whether the token still works")).on_click(cx.listener(move |s, _, _, cx| s.post(id, format!("accounts/{id}/check"), json!({}), cx, |_, _| {}))) };
        let out_btn = kit::btn_small(t, SharedString::from(format!("{id}-out")), out.to_string()).text_color(t.down);
        let out_btn = if busy { kit::disabled(out_btn) } else { out_btn.on_click(cx.listener(move |s, _, _, cx| s.post(id, format!("accounts/{id}/disconnect"), json!({}), cx, |_, _| {}))) };
        let tip = "Forgets Taskboard's token. gh and git on this Mac aren't touched.";
        line(t, &format!("{id}-in"), true, false, "", &note, Some((&name, true)))
            .when(!scopes.is_empty(), |d| d.child(div().flex().flex_wrap().gap(px(4.)).max_w(px(260.)).justify_end().children(scopes.into_iter().take(6).map(|s| kit::chip(t, s.to_string())))))
            .child(controls().child(check).child(out_btn.tooltip(kit::tip(tip))))
            .into_any_element()
    }

    /// A secret field with Connect (↩).
    #[allow(clippy::too_many_arguments)]
    fn token_row(&self, t: &Theme, id: &'static str, label: &str, note: &str, field: &Input, window: &Window, cx: &mut Context<Self>, first: bool) -> AnyElement {
        let busy = self.busy == Some(id);
        let input = field.render(t, SharedString::from(format!("{id}-token")), window).w(px(260.)).on_key_down(self.field_keys(id, field, cx));
        let btn = kit::btn_primary(t, SharedString::from(format!("{id}-connect")), if busy { "Checking…" } else { "Connect" }).h(px(32.)).text_size(px(12.5));
        let btn = if busy { kit::disabled(btn) } else { btn.on_click(cx.listener(move |s, _, _, cx| s.connect(id, cx))) };
        line(t, &format!("{id}-token-row"), first, false, label, note, None).child(controls().child(input).child(btn)).into_any_element()
    }

    fn cli_line(&self, t: &Theme, id: &str, cli: &str, cx: &mut Context<Self>) -> impl IntoElement {
        let copied = self.copied.as_ref().is_some_and(|(c, _)| c == cli);
        let text = cli.to_string();
        let (raised, fg) = (t.panel_2, t.text);
        div().px(px(16.)).pb(px(8.)).child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .pl(px(10.))
                .pr(px(4.))
                .py(px(3.))
                .rounded(px(6.))
                .bg(t.col)
                .font_family(t.mono_font.clone())
                .text_size(px(11.5))
                .child(div().flex_none().text_color(t.accent).child("$"))
                .child(div().flex_1().min_w_0().truncate().child(cli.to_string()))
                .child(
                    div()
                        .id(SharedString::from(format!("copy-{id}")))
                        .flex_none()
                        .h(px(20.))
                        .px(px(6.))
                        .flex()
                        .items_center()
                        .rounded(px(5.))
                        .font_family(t.ui_font.clone())
                        .text_size(px(11.))
                        .text_color(if copied { t.up } else { t.muted })
                        .cursor_pointer()
                        .hover(move |s| s.bg(raised).text_color(fg))
                        .on_click(cx.listener(move |s, _, _, cx| s.copy(text.clone(), cx)))
                        .child(if copied { "Copied" } else { "Copy" }),
                ),
        )
    }
}

// ------------------------------------------------------------------ history

/// "30 days", "6 months", "1 year", "Always".
fn days_label(n: i64) -> String {
    match n {
        0 => "Always".into(),
        365 => "1 year".into(),
        730 => "2 years".into(),
        n if n % 30 == 0 && n >= 180 => format!("{} months", n / 30),
        n => format!("{n} days"),
    }
}

/// "4.2 MB", "820 KB".
fn bytes_label(b: i64) -> String {
    if b >= 1_000_000 { format!("{:.1} MB", b as f64 / 1_000_000.0) } else { format!("{} KB", (b as f64 / 1000.0).ceil() as i64) }
}

impl SettingsWindow {
