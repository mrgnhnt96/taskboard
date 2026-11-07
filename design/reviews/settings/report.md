## Units

- **Settings shell:** 14 findings. macOS desktop Settings window, reviewed for a single pointer-and-keyboard owner at 1400x1000, 900x680 and 720x460, in light and dark, with the board live and stopped. Evidence is 10 captures plus code: the listed excerpts and the full settings.rs, main.rs, app.rs, text_input.rs and days.rs in the worktree. The native app wasn't driven, so no control was used with real input. Behaviour findings are read from code, and ratios are computed from theme.rs. First impression: the live shell reads as organised. The sidebar is quiet, the page title is clear and the content sits in tidy cards, so this is not a clutter problem. The problems are in the states and edges of the frame. Counts: 0 Catastrophe, 8 Major, 4 Minor, 2 Cosmetic. The top problem is that the stopped-board state misleads. Opened while the board is down, the window says every account is 'Not signed in', sits on 'Connecting…' and 'Asking the board…' forever beside a 'Can't reach' banner, and offers no retry. History, QA and Reviewers then stay stuck even after the sidebar returns to Live. Close behind: below about 830 wide, sign-in buttons are cut off with no way to reach them, and no control in the window works from the keyboard except by clicking into a field first.
- **Accounts:** 16 findings. macOS desktop, pointer and keyboard. This is a native GPUI Settings window, reviewed from the 12 captures (1400x1000, 900x680, 720x460; light and dark; board up, stopped and stand-in) plus the code excerpts. Nothing was driven with real input, because there is no page to open. The blur audit could not be run, so this first impression comes from viewing the captures: each card is tidy, but the screen reads as a stack of forms rather than a status view. With nothing connected, three always-open token forms fill the default window, Slack is below the fold, and four blue buttons compete. Counts: 1 Catastrophe, 5 Major, 9 Minor, 1 Cosmetic. Top problem: Disconnect fires on one click, with no confirm or undo, 8px from Check, and it forgets a token the app says it never shows again. Close behind: at the 720x460 minimum, Sign in with GitHub is cut off and Connect is gone. When the board is stopped, every account falsely reads as not signed in.
- **History and Usage:** 14 findings. macOS desktop Settings window, History and Usage sections, pointer and keyboard. I judged 12 captures at 1400x1000, 900x680 and 720x460 in light and dark, plus the code excerpts and the daemon source. I did not run the app: it is native, so nothing was driven with real input. First impression: both pages read as calm and organised, with one card and a quiet sidebar, and the eye goes straight to the rows. But the History rows have nothing to act on. Counts: 1 Catastrophe, 5 Major, 6 Minor, 2 Cosmetic. The top problem is that History's two retention controls render as an empty 4px dot. The app asks for `detail_days_choices` and `summary_days_choices`, while the daemon sends `detail_choices` and `summary_choices`, so the owner can neither see nor change how long history is kept. This looks like a bug on the real board as well, not a sample artefact. Next come Usage's Today row going stale in a window left open all day, the 25% option cut off at the minimum window, shortening retention with no warning, and no keyboard route at all.
- **QA Reviewers Worktrees:** 15 findings. macOS desktop, pointer and keyboard. This is a native GPUI window, so there was no page to drive. I reviewed the 15 captures (QA, Reviewers and Worktrees at 1400x1000, 900x680 and 720x460, in light and dark), the code excerpts, and the wider source in the worktree where the excerpts stopped. I also read one extra capture, design/captures/settings/history--light-s.png, because Clean up now lives on History. Nothing was used with real input. All three pages hold up at 720x460, with no clipping or lost controls. First impression, judged by eye because the blur audit needs a page: each page is calm and organised, but nearly empty. The only bright spot is QA's amber 'Needs Jira' row, for a feature that is optional. The sample board returns no slots or reviewers and has no Jira, so the populated Reviewers and Worktrees pages and QA's On state were judged from code only. Findings: 0 Catastrophe, 5 Major, 7 Minor, 2 Cosmetic. The top problem is that these pages mostly can't do what the brief says the window is for: Jira, pool settings and the reviewer roster can't be set here, and the retention choices on the row that drives Clean up now render as an empty control. Next is staleness: QA and Reviewers load once when the window opens, so a window left open all day shows old data or stays stuck on 'Asking the board…'.
- **System and Update:** 15 findings. macOS desktop, pointer and keyboard. I reviewed the Settings › System page at 1400x1000, 900x680 and 720x460, in light and dark, plus the board-stopped state at 900x680. Evidence is six captures and the code excerpts. This is a native GPUI app, so nothing was driven with real input and no contrast was measured. All six captures show Update as Off, so every other update state (checking, up to date, downloading, ready, installing, failed), the update pill and Restart daemon are reviewed from code only. First impression: the page reads as calm and organised. Blurred, it shows two quiet cards and no competing emphasis, and the composition is not the problem. The problems are in the states the captures don't show, and in the board-stopped state, which is the quietest thing on the page when it should be the loudest. Counts: 0 Catastrophe, 3 Major, 8 Minor, 4 Cosmetic. Top problem: the failed path. Every failure, including a rejected signature or a failed install, reads "Check failed at…" with a Retry button, and the real reason sits in the smallest, faintest text on the row. No finding here needs the owner's OK: none of the fixes breaks a stated rule.
- **Hours menu:** 15 findings. macOS desktop, pointer and keyboard. Reviewed at 1360x860 and the 900x560 minimum window, in light and dark, using 6 captures plus the code excerpts. The app is native, so no control was used with real input; colours were sampled from the capture pixels. First impression: the menu is compact and reads as organised, and it fits at both sizes. No blurred audit was run; this comes from the captures. The weak spots are behaviour, not layout. Counts: 0 Catastrophe, 3 Major, 11 Minor, 1 Cosmetic. Top problem: the pill, checkbox, days and am/pm chips take no keyboard focus and no menu bar item opens the menu, so the keyboard path in this review's scope doesn't exist in the code seen. Close behind: "Today until" can quietly run agents 12 hours longer than meant, and a save that fails as the menu closes is lost without a message.
- **Usage pill and footer:** 10 findings. macOS desktop (pointer and keyboard), sidebar footer at 1360x860 and 900x560, reviewed from 7 captures plus code excerpts. The app itself wasn't run, so nothing was clicked, hovered or tabbed; the browser was used only to compute contrast. First impression: the main window is calm, and the footer reads as a loose stack of unrelated pills rather than one organised health area. In the normal state it holds together, but as soon as the daily limit is on, the usage pill runs off the sidebar and is cut at "Toda". Counts: 1 Catastrophe, 3 Major, 5 Minor, 1 Cosmetic. Top problem (R3): the pill is too wide for the fixed 248px sidebar once Today is added, so the daily reading and the Lifted state never show at any window size, and the visible part can stay green while the day's allowance is the thing running out. R3's 4 is a Major raised one step under the severity guide, because it shows on every page at every size; a hover tooltip still carries the daily line. Next in line: a click goes to Days rather than to the Settings window where limits are set, and the window labels and the stale state fail contrast.
- **First launch:** 16 findings. macOS desktop, pointer and keyboard. I reviewed Flow 14 (First launch, Settings side) from the 5 captures (main window 1360x860, Settings 900x680, light only) and from code. The app is native, so nothing was driven with real input, and 720x460 was worked out from code, not seen. First impression: each surface is calm and tidy, but the flow as a whole isn't organised. No single place lists what a new owner must set up. The five steps are spread across a main-window banner, a plain text item in the sidebar footer, a tooltip-only pill, Settings › Accounts, and one step (the tb link) that never appears at all. Settings › System, the obvious home, shows only updates and versions. Counts: 0 Catastrophe, 8 Major, 7 Minor, 1 Cosmetic. Top problem (R36): no setup home and no order, so the owner can't tell what is done, what is next, or why agents aren't reporting. The other Majors are mostly dead ends: a banner that stays after the owner has acted, failures with no retry, and a hooks error that disappears after 6 seconds.

## Findings

### R1 · 4 Catastrophe · Disconnect removes the token on one click, with no confirm or undo
- **Unit:** Accounts
- **Element:** Accounts › any signed-in card › Disconnect, all sizes (acct-normal-light-m.png, acct-warm-light-m.png; settings.rs signed_in_row)
- **Evidence:** Confirmed
- **Problem:** One click posts accounts/{id}/disconnect immediately. The button is 24px tall and sits 8px from Check, the safe action the owner reaches for most on this row. A slip forgets the token, and agents on a task lose their push and API access while they are running. Bitbucket, Azure and Slack then need a token re-pasted or a new one made, because the app never shows tokens again. No message says what happened; the card just turns back into a form.
- **Principle:** Destructive actions are separated from safe ones and can be undone or ask to confirm [NNG-H3] [NNG-H5]; severity anchor: destructive action with no confirm or undo next to a frequent one
- **Fix:** Make Disconnect two-step, inline in the row. The first click swaps Check and Disconnect for 'Disconnect GitHub? Agents on a task lose access.' with Cancel and a kit::btn_danger 'Disconnect'. Move Disconnect apart from Check with at least 16px, or put it behind a '…' menu. After it completes, show a note 'Disconnected {name}.' Also replace the hard-coded tooltip, which talks about gh and git on the Azure and Slack cards too, with per-service text.

### R2 · 4 Catastrophe · History's retention controls are empty, so retention can't be seen or set
- **Unit:** History and Usage
- **Element:** Settings › History › 'Keep every event for' and 'Keep day summaries for' › segmented control, every size and scheme (history--light-l.png, history--light-m.png, history--light-s.png, history--dark-m.png, history--dark-s.png)
- **Evidence:** Confirmed
- **Problem:** Each row ends in a faint dot where the choices should be: an empty `kit::seg` track with only its padding. The current value (default 90 days and 1 year) isn't shown either, so the owner can't tell or change how long history is kept, and this window is the only place to do it. `history_page` reads `h[format!("{key}_choices")]`, which is `detail_days_choices`. `days.rs` L824-825 and docs/API.md send `detail_choices` and `summary_choices`, so the lookup should fail on the real daemon too, not only the sample board. `unwrap_or_default()` then hides the miss.
- **Principle:** Pass 1 walkthrough question 2, the correct action must be visible [NNG-CW]; visibility of system status [NNG-H1]; severity guide: primary task can't be finished.
- **Fix:** Pass the JSON key separately: `pick("detail_days", "detail_choices", …)` and `pick("summary_days", "summary_choices", …)`. Add a test that feeds `days::history`'s shape to `history_page` and asserts four options per row. If choices are ever empty, show the current value as plain text ('90 days') rather than an empty track. Use a pop-up button rather than `kit::seg`: by estimate, four segments ('30 days', '90 days', '6 months', '1 year') need about 270px, and only about 238px is free at 720 wide, so they would clip as R18 does.

### R3 · 4 Catastrophe · The Today segment is cut off at the sidebar edge, so the daily limit and Lifted never show
- **Unit:** Usage pill and footer
- **Element:** Main window › sidebar footer › usage pill › Today segment, whenever the daily limit is on, at 1360x860 and 900x560, light and dark (pill-daily-light-m.png, pill-warm-light-m.png, pill-hot-light-m.png, pill-hot-dark-s.png)
- **Evidence:** Confirmed
- **Problem:** The pill shows '5h … | 7d … | Toda' and stops at the sidebar's edge: the Today bar and value are gone, and the pill loses its rounded end. The two-window pill is already about 211px of the 224px available, and pill_shell is flex_none and whitespace_nowrap, so it can neither wrap nor shrink. Because the sidebar is a fixed 248px, no window size fixes this. The value that says how much of today is left, or 'Lifted', is visible only by hovering. In pill-daily-light-m.png both visible windows are green, so the pill reads as fine whatever Today's level is.
- **Principle:** Nothing is cut off or lost, and every state of a control is visible at every size [WCAG 1.4.10] [NNG-H1]; recognition rather than recall [NNG-H6]; narrowing keeps the order [SCR-6]
- **Fix:** Change usage_pill from one inline row to a full-width block (w_full, so it fills the 224px) with one flex_1 min_w_0 column per window. Each column has a 15px line with the label on the left and the value on the right, and a 4px full-width bar under it. Cut the segment padding from 7px to 5px. Use short values: 'Today 88%' or 'Today Lifted', with whole percents only. That comes to about 204-214px wide and only about 5px taller than today, which matters because 900x560 has only about 142px left between the nav and the footer. Avoid one row per window: it would add about 36px, and that space comes out of the Active Goals list. Check the width with 100% in all three columns.

### R4 · 3 Major · Board stopped: Accounts claims every account is signed out and keeps the sign-in forms live
- **Unit:** Settings shell
- **Element:** Settings › Accounts › every account card, board stopped at window open (down-accounts-m.png, down-accounts-s.png)
- **Evidence:** Confirmed
- **Problem:** With no answer from the board, accounts_page draws its rows from empty data. Every card says 'Not signed in', the headings fall back to raw ids ('github', 'bitbucket', 'azure'), and Sign in, Connect and the token fields stay active. An owner whose accounts are all fine is told they are gone, may paste a token again, and only then gets an error. The one fact the page exists to show is false.
- **Principle:** Visibility of system status: don't report a state you don't have [NNG-H1]; the simulated or empty state must not contradict itself [checklist: data states]; the user's words, not internal ids [NNG-H2]
- **Fix:** In accounts_page, change the early return from `!self.loaded && self.error.is_none()` to `!self.loaded`, so no cards render until a real answer has arrived. Show the shared failed state from R5 instead. Take display names from CARDS ('GitHub', 'Bitbucket', 'Azure DevOps', 'Slack'), never `unwrap_or(id)`. If the board drops after a good load, keep the last accounts on screen, disable Connect, Sign in and Disconnect, and add 'As of 3:05 PM' to the banner.

### R5 · 3 Major · Failed state contradicts itself and offers no way forward
- **Unit:** Settings shell
- **Element:** Settings › sidebar status, banner and page body, board stopped (down-history-m.png, down-system-m.png, down-accounts-m.png)
- **Evidence:** Confirmed
- **Problem:** One condition is reported five ways at once: banner 'taskboardd: Can't reach the task board server.', sidebar 'Connecting…', body 'Asking the board…', System 'not running' and 'Not connected.'. 'Connecting…' and 'Asking the board…' never end, because `live`/`self.loaded` only reach 'Not connected' after a first success and the load_* functions drop their errors. The banner names a binary, gives no next step and has no retry. The only fix, Restart daemon, sits on System and appears only for an installed bundle (it is absent in down-system-m.png, a dev build). System also shows a green dot beside App version while everything is down. The product is called board, daemon, taskboardd and task board server in different places.
- **Principle:** Help users recognize, diagnose and recover from errors [NNG-H9]; system status [NNG-H1]; one fact, one place [SEC-7]; same thing, same name [NNG-H4]
- **Fix:** Make one shared failed state owned by the shell. The banner reads 'The board isn't running. Settings can't be read or changed until it's back.' with a 'Try again' button (reloads every section now) and, when installed_bundle() exists, 'Restart board'. Sidebar status: 'Not connected' as soon as any error arrives; keep 'Connecting…' only until the first failure or for at most 5 seconds. When the failure is known, replace 'Asking the board…' with nothing: the banner alone speaks. Track per-section load errors instead of `if let Ok`. Pick one name, 'the board', for the UI and keep 'daemon' only in the System version row. Drop the green dot on App version.

### R6 · 3 Major · Sections don't recover or refresh in a window left open all day
- **Unit:** Settings shell
- **Element:** Settings › section loading: History, QA, Reviewers, Usage, Worktrees, sidebar status
- **Evidence:** Confirmed
- **Problem:** load_history, load_qa and load_reviewers run once, in SettingsWindow::new. If the board was down at open, those three show 'Asking the board…' forever even after the sidebar returns to Live; only closing and reopening fixes it. Reviewers can go stale for a whole day although its own text says agents change the roster. Usage reloads only when its sidebar row is clicked, so 'Today' never moves while the owner watches it, and a limit reached in that time isn't shown. Only accounts are polled, every 30 seconds (POLL 2s x IDLE_TICKS 15), so the sidebar can say Live for up to 30 seconds after the board dies.
- **Principle:** Visibility of system status [NNG-H1]; readouts tell the truth [checklist pass 8]; no dead ends [NNG-H3]
- **Fix:** Add one `reload_section(sec)` used in three places: on sidebar click for every section, on the poll tick for the visible section (10 seconds is enough for Usage), and on the error-to-live transition for all sections. On the transition, also clear the banner immediately. While an error is showing, poll every 2 seconds instead of 30 so recovery appears quickly. Do the same on window activation, so returning to the window is never stale.

### R7 · 3 Major · Below about 830 wide, sign-in controls are cut off with no way to reach them
- **Unit:** Settings shell
- **Element:** Settings › Accounts › GitHub 'Sign in with GitHub', Bitbucket/Azure/Slack token field and Connect, at 720x460 (accounts--light-s.png, accounts--dark-s.png, down-accounts-s.png)
- **Evidence:** Confirmed
- **Problem:** At the supported minimum, 'Sign in with GitHub' shows only 'Sign' and the API token field runs out of the card. Connect sits past the edge entirely (field position seen in the capture, Connect's place worked out from the row layout). The row is a non-wrapping flex with a 150px text minimum, flex_none controls and a fixed 260px field, inside a card with overflow_hidden, and the page only scrolls vertically. Measured from the captures, GitHub needs about 546px of card, so anything under a roughly 830px window clips. Enter in the field and widening the window are the only workarounds, and nothing tells the owner to do either.
- **Principle:** Every control at every size; the layout reflows across the whole window range [HIG-MACOS] [NNG-H6]; primary action must be noticeable at each step [NNG-CW]
- **Fix:** In `row()` add `.flex_wrap()` and give `line()`'s text block `flex_basis(px(200.))`, so controls drop to their own right-aligned line when they don't fit. In token_row and the email/org fields replace `.w(px(260.))` with `.flex_1().min_w(px(140.)).max_w(px(260.))`. Also check Usage's six-option segmented control at 720, which has the same row. If you would rather not wrap, raising window_min_size to 840 wide also fixes it, but that changes the stated minimum and needs the owner's OK.

### R8 · 3 Major · The window can't be used from the keyboard, and nothing shows focus
- **Unit:** Settings shell
- **Element:** Settings › sidebar rows, Agent commands, every kit button, segmented control and link, all sizes
- **Evidence:** Confirmed
- **Problem:** Sidebar rows and every button are plain divs with on_click. No tab_index, focus_next or focus_prev exists anywhere in taskboard-app, TextField doesn't handle Tab, and the TaskboardSettings key context has no bindings or actions. So sections can't be switched, a button can't be pressed, and Tab won't even move from Atlassian email to API token. The only focus cue in the kit is a text field's border. ⌘W and ⌘, do work, but Esc does nothing at window level. The owner, a developer who lives on the keyboard, must reach for the pointer for every change.
- **Principle:** Keyboard [WCAG 2.1.1]; focus visible [WCAG 2.4.7]; arrows move within groups, one tab stop per group [MS-KBD]; flexibility and efficiency [NNG-H7]
- **Fix:** Give kit::btn, kit::link, kit::seg items and sidebar rows a focus handle with tab_index, plus a 2px t.accent ring on focus (3.04:1 or better on all backgrounds used). Bind tab/shift-tab to window.focus_next/focus_prev in the TaskboardSettings context, and enter/space to the focused control's click. Make the sidebar one tab stop with up/down to change section. Add ⌘1 to ⌘7 for sections in that context, matching the main window's ⌘-number pattern, and show them in row tooltips. Bind esc to close the window when no field has focus, and ⌘] / ⌘[ optional.

### R9 · 3 Major · Work hours, half of the primary task, has no place in Settings
- **Unit:** Settings shell
- **Element:** Settings › sidebar and Usage section (whole window)
- **Evidence:** Confirmed
- **Problem:** The primary task is to check and adjust work hours and usage limits, and the brief calls this window the owner's only way to set these things. settings.rs has no work-hours section, row or link; they are set from a status-bar pill menu in the main window (ui/hours.rs). An owner who opens Settings for this fails the walkthrough's second question: the action isn't there, and nothing says where it is. The Usage text also doesn't mention that work hours affect when agents run.
- **Principle:** Will users notice the correct action is available [NNG-CW]; one home per object (project rule); options visible [NNG-H6]
- **Fix:** Needs the owner's OK, because it moves the home of work hours: add a 'Work hours' section directly under Accounts with start, end, days and today's status in 12-hour times, and make the status-bar pill a read-only readout whose click opens that section. If the pill menu is meant to stay as the home, then the brief's rule needs correcting, and the Usage page should carry one line instead: 'Work hours are set from the status bar' with a link that opens the menu.

### R10 · 3 Major · History's retention controls render empty, so neither setting can be changed
- **Unit:** Settings shell
- **Element:** Settings › History › 'Keep every event for' and 'Keep day summaries for', right edge of the rows (history--light-m.png)
- **Evidence:** Confirmed
- **Problem:** Where each segmented control belongs there is only a faint 4px dot. history_page looks up `{key}_choices` with key 'detail_days', so it reads `detail_days_choices`; the daemon's GET /history sends `detail_choices` and `summary_choices` (days.rs). The list comes back empty and the control draws its padding alone. The owner can't see the current retention or change it, and this window is the only place to do so. This sits inside a section rather than the frame, but it shows in the shell capture and is too serious to leave out.
- **Principle:** Visibility of system status [NNG-H1]; feature missing at a size or state [severity guide]
- **Fix:** In history_page, pass the choices key separately: `pick("detail_days", "detail_choices", …)` and `pick("summary_days", "summary_choices", …)`. Add a parity test that a real GET /history yields four labels for each row. The summary choices include 0, so confirm days_label shows 'Always'.

### R11 · 3 Major · Contrast failures on shared tokens used across the sign-in flow
- **Unit:** Settings shell
- **Element:** Settings › sidebar attention badge (dark), primary buttons (dark), field placeholders and link-row URLs (both themes), field and button borders (both themes)
- **Evidence:** Confirmed
- **Problem:** Computed from theme.rs. The sidebar badge, the only summary that an account needs attention, is white on #f59e5b in dark: 2.12:1, at 11px. Primary button text in dark is white on #4d72f0: 4.22:1. Placeholders (t.dim) and URLs (t.faint) are 3.04:1 on card in light and 3.80:1 in dark, and those placeholders carry the token format hints. Field edges are 1.48:1 in light (#cfd5dd on white) and 1.63:1 in dark, so the white field on a white card relies on a border that barely shows.
- **Principle:** Contrast minimum [WCAG 1.4.3]; non-text contrast [WCAG 1.4.11]
- **Fix:** Badge text in dark: #0f1216 on #f59e5b, 8.87:1 (add a `on_warn` token). Dark accent_btn: #4468e6, 4.81:1. faint and dim: #6b7686 in light (4.60:1), #8d98a6 in dark (5.66:1). In kit::Input use a dedicated field border: #8a95a3 in light (3.04:1), #6f7a88 in dark (3.80:1). These tokens are shared, so recheck the main window after changing them.

### R12 · 3 Major · At the minimum window, Sign in with GitHub is clipped and Connect is gone
- **Unit:** Accounts
- **Element:** Accounts › GitHub, Bitbucket, Azure and Slack cards › controls at 720x460, both themes (accounts--light-s.png, accounts--dark-s.png, down-accounts-s.png)
- **Evidence:** Confirmed
- **Problem:** The card is about 436px wide with 404px inside its padding. The row needs a 150px label minimum, a 16px gap and flex_none controls of about 340px, and kit::card's overflow_hidden cuts the excess with no scrolling. Only 'Sign' of the primary button shows. The token field runs off the card, and Connect starts past the card's edge. Enter in the field still submits, and widening the window fixes it, which is why this is 3 and not 4, but nothing tells the owner either.
- **Principle:** Every control at every supported size; the layout reflows across the whole window range [HIG-MACOS] [NNG-H6]; severity anchor: feature missing at one size class
- **Fix:** Let rows wrap: add flex_wrap to row() and make controls() wrap too. Give the field flex_1 with min_w(140.) and max_w(260.) instead of w(260.). Below about 560px of content width, stack the controls under the label, full width, with the primary button first. Remove overflow_hidden from the card, or clip only the corners, so a future overflow can't hide a control.

### R13 · 3 Major · The page is a stack of open forms, so account status can't be read at a glance
- **Unit:** Accounts
- **Element:** Accounts › whole page and the Bitbucket, Azure and Slack cards at 900x680 and 720x460 (accounts--light-m.png, accounts--light-s.png, acct-normal-light-m.png)
- **Evidence:** Confirmed
- **Problem:** Each unconnected service permanently shows a full form with a four-line scope paragraph, even if the owner never uses it. Connected accounts get pushed down, Slack and the Keychain note fall below the fold at the default size, and only about 1.5 cards show at the minimum. A sidebar badge of 1 can point at a card that is off screen. Checks that fail: frequent in view and rare behind [SEC-8]; one emphasis, with four primary fills on one screen [SEC-5]; one fact, one place, with 'gh isn't changed' in the GitHub note, the tooltips and the footer [SEC-7]; and one shape per role, since GitHub says 'Not signed in' while the other three show no status line at all [SEC-6].
- **Principle:** The work dominates and everything earns its place [SCR-1] [SCR-5]; frequent in view, rare behind [SEC-8]; recognition rather than recall [NNG-H6]
- **Fix:** Give every card the same first row: a status ('Not connected', or the signed-in name) with its actions on the right. Collapse Bitbucket, Azure and Slack to that row plus a secondary 'Connect…' button that expands the form and focuses its first field. All four cards then fit at 900x680. Keep a primary fill only on an account that needs the owner, such as Sign in again. State the Keychain and gh facts once, in the footer, and cut them from the GitHub note. Put the scope text in the expanded form only.

### R14 · 3 Major · Buttons and sidebar items can't be reached from the keyboard
- **Unit:** Accounts
- **Element:** Accounts › Sign in with GitHub, Use my gh sign-in, Use a token, Connect, Check, Disconnect, Cancel, Copy, links, Agent commands, sidebar sections (kit.rs btn_base and link; settings.rs sidebar, page_header, cli_line)
- **Evidence:** Confirmed
- **Problem:** These are all plain divs with on_click: no focus handle, no tab stop, no key handler, no focus style. Only text fields take focus. In a token form Enter works from the field, but Sign in with GitHub, Check, Disconnect and switching section need the mouse. The field's focus state is also only a 1px border colour change. This is a keyboard-first owner on a keyboard platform.
- **Principle:** Keyboard reach and operation [WCAG 2.1.1] [MS-KBD]; focus visible [WCAG 2.4.7]; role and state exposed [WCAG 4.1.2]
- **Fix:** Fix it once in kit::btn_base and kit::link: give each button a FocusHandle with a tab stop, activate on Enter and Space, and draw a 2px t.accent ring on focus. Apply the same to Agent commands, the Copy control and the code chip. Make the sidebar one tab stop with Up and Down moving between sections. Check that Tab follows reading order within each card.

### R15 · 3 Major · With the board stopped, every account reads 'Not signed in' and the forms stay live
- **Unit:** Accounts
- **Element:** Accounts › whole page, board stopped (down-accounts-m.png, down-accounts-s.png; accounts_page early return, account() returning Null)
- **Evidence:** Confirmed
- **Problem:** accounts_page only stops early when there is no error. On error it renders Null accounts, so a signed-in owner sees 'Not signed in' and empty forms, and may start re-entering tokens that are fine. Card headings fall back to raw ids ('github', 'azure'). 'Use my gh sign-in' shows without knowing gh is installed. Every action looks enabled but can only fail. The banner uses the daemon name 'taskboardd', offers no next step, and the sidebar says 'Connecting…' while the banner says it can't reach the server.
- **Principle:** Status must be true: in the error state, commands never report what they didn't have [NNG-H1]; help users recover from errors [NNG-H9]; plain language [NNG-H2]; a simulated or error state that contradicts itself is a finding
- **Fix:** When !loaded and error is set, replace the cards with one panel: 'Can't reach the board, so sign-ins can't be shown. Your saved accounts aren't affected.', plus the way to start it and a Try again button that calls load(true). Set the sidebar to 'Not connected' in this state. If accounts were already loaded when the board drops, keep them but disable their actions. Take heading names from CARDS rather than the answer's label.

### R16 · 3 Major · Green push row stays on when the sign-in has been refused
- **Unit:** Accounts
- **Element:** Accounts › GitHub card › push row, reauth state, 900x680 (acct-warm-light-m.png; github_rows calling push_row, push_row uses t.up)
- **Evidence:** Confirmed
- **Problem:** push_row is added for every connected GitHub or Bitbucket account and always has a green dot. In acct-warm it reads 'Agents on a task git push to github.com with this account' directly under 'GitHub refused the saved sign-in' and an amber row. The owner's job here is to keep agents working, and the page says both that pushes work and that they won't. The refusal itself is small muted text joined to 'Taskboard's own token'.
- **Principle:** Visibility of system status [NNG-H1]; one fact, one place, no copies that can disagree [SEC-7]
- **Fix:** Pass the account's health into push_row. When reauth is true or error is set, use the t.warn dot and 'Agents can't push to github.com until you sign in again.' Keep the green dot and current sentence for the healthy state only. Move the refusal reason from the signed-in row's note into the amber row.

### R17 · 3 Major · Usage and History never refresh while open, and hang on 'Asking the board…' if the first load fails
- **Unit:** History and Usage
- **Element:** Settings › Usage › Today row, and History › Space used and Cleanup rows, in a window left open; both pages when the board is down at open
- **Evidence:** Confirmed
- **Problem:** The poll loop refreshes accounts, slots and info, but not `daily` or `history`. `load_daily` runs only at open and on a sidebar click, and `load_history` runs only at open. An owner resting on Usage never sees the amber 'used up' row appear, and 'Lifted' stays on screen past midnight after the lift has expired, while the board's pill shows the true numbers. `load_history` and `load_daily` also drop errors with `if let Ok`. If the board is stopped when the window opens, History says 'Asking the board…' indefinitely, even after the board returns, and clicking History doesn't retry. Usage retries only on a sidebar click.
- **Principle:** Visibility of system status [NNG-H1]; one fact, one place: copies must not disagree [SEC-7]; each data state has a way forward [mobile:NNG-EMPTY].
- **Fix:** In the existing poll closure, add `if s.view == Sec::Usage && tick % IDLE_TICKS == 0 { s.load_daily(cx) }` and the same for History, which gives a 30s refresh. Call `load_history` on a click of the History sidebar item. On `Err`, replace the placeholder with 'The board isn't answering. This page fills in when it's back.' and keep retrying on the poll.

### R18 · 3 Major · The Daily limit control is cut off at the minimum window, and the 25% option can't be reached
- **Unit:** History and Usage
- **Element:** Settings › Usage › Daily limit › base control at 720x460, light and dark (usage--light-s.png, usage--dark-s.png)
- **Evidence:** Confirmed
- **Problem:** The six-option control runs past the card's right edge, leaving only a '2' of '25%', and the card's `overflow_hidden` removes the rest, so 25% can't be clicked or seen as selected. Measured from the captures, the control is about 284px wide with six options and about 330px with seven. It therefore clips below a window width of about 766, or about 812 once a share set elsewhere adds a seventh (usage-daily-light-m.png). The note is squeezed to four lines at the same time. A share set elsewhere, like 12%, also disappears from the row once the owner picks another option, and the window can't set it again.
- **Principle:** Option count fits the control at the narrowest width, then becomes a menu [HIG-SEGMENTED] [NNG-LISTBOX]; every control at every size [NNG-H6]; LESSONS L2: a set that grows gets a pop-up menu, not a segmented control.
- **Fix:** Replace `daily-base` with a pop-up button built from `kit::popover`, `kit::menu_box` and `kit::menu_item`, listing Off, Even, 10%, 15%, 20%, 25% and any other current share. The button is about 90px wide, so the row fits at 720 and the note drops to two lines. Keep 'Next day / Spread' as a segmented control, since two options fit.

### R19 · 3 Major · Shortening retention or cleaning up deletes history with no warning, and the note understates what goes
- **Unit:** History and Usage
- **Element:** Settings › History › retention rows and Cleanup › 'Clean up now'
- **Evidence:** Confirmed
- **Problem:** Once R2 is fixed, one click on '30 days' saves at once and the 3 AM run permanently deletes everything older, with no count, confirm or undo. 'Clean up now' does the same immediately, and neither it nor its row says what will go. The note says 'of finished tasks', but `cleanup` also deletes terminal lines for all tasks by date, plus usage samples, host limits and slot spans. The result message counts all of it as 'old events'.
- **Principle:** Error prevention: confirm or undo destructive actions [NNG-H5]; user control [NNG-H3]; destructive actions worded clearly (pass 3).
- **Fix:** When a choice is shorter than the current one, keep the old value and show an amber row in the card: 'Shortening to 30 days removes 1,204 events (2.1 MB) at the next cleanup', with 'Shorten' and 'Keep 90 days' buttons. This needs a count from the daemon, for example `GET history?preview_days=30`. Give 'Clean up now' the same confirm when anything would be removed. Reword the note: 'Commits, questions, checkpoints and status changes of finished tasks, plus all terminal output and usage readings of that age.'

### R20 · 3 Major · Nothing on these pages can be reached from the keyboard
- **Unit:** History and Usage
- **Element:** Settings › sidebar items; History › 'Clean up now'; Usage › Daily limit, Unused share and Today controls, all sizes
- **Evidence:** Confirmed
- **Problem:** `kit::seg`, `kit::btn` and the sidebar rows are divs with `on_click` only. No `tab_index` or key handler appears in `settings.rs` outside the text fields, and only the root takes focus. The owner can't Tab to a control, arrow through a group or press Space to lift a limit, and no focus ring exists.
- **Principle:** Keyboard operable [WCAG 2.1.1]; focus visible [WCAG 2.4.7]; arrows move inside groups, which take one tab stop [MS-KBD].
- **Fix:** Give `kit::btn` and sidebar rows `.tab_index(0)` with Space and Enter activation. Make each `kit::seg` one tab stop where ← and → move the selection. Sidebar ↑ and ↓ switch sections. Draw a 2px `t.accent` outline on the focused element. Doing this in `kit` covers every Settings page at once.

### R21 · 3 Major · The selected segment is barely distinct, most of all in dark
- **Unit:** History and Usage
- **Element:** Settings › Usage › all segmented controls, dark (usage-lifted-dark-m.png, usage--dark-m.png); same pairs in light (usage-daily-light-m.png)
- **Evidence:** Confirmed
- **Problem:** In dark, the selected segment is a slightly darker patch on the track, 1.13:1 (#1a1f26 on #232931), and its text differs from unselected text by only 1.91:1. Weight is the same in both states. Whether '12%' or 'Lifted' is the live choice is hard to read at a glance, and it reads more like a recess than a selection. Light is 1.19:1 fill and 2.98:1 text. It reads better thanks to the white fill and shadow, but it is still under 3:1. No non-colour cue exists.
- **Principle:** Non-text contrast for state [WCAG 1.4.11]; use of colour [WCAG 1.4.1]. Computed from theme.rs values.
- **Fix:** In `kit::seg`, give the selected item `FontWeight::SEMIBOLD` and a 1px border in `t.muted`, which is 6.3:1 on the dark track and 4.58:1 on the light one. Keep the fill quiet. Don't use the accent colour, so each row keeps at most one emphasis [SEC-5].

### R22 · 3 Major · QA can only be unlocked by hand-editing a config file, and Jira has no home in Accounts
- **Unit:** QA Reviewers Worktrees
- **Element:** QA › Read QA comments row, no Jira configured, all sizes (qa--light-m.png, qa--dark-s.png)
- **Evidence:** Confirmed
- **Problem:** The only way forward is 'set up [jira] in ~/.config/taskboard/config.toml'. The window can't connect Jira, and CARDS in Accounts lists only GitHub, Bitbucket, Azure and Slack. The path is plain div text, so it can't be selected or copied, and nothing says which keys are needed. The row is also amber, the loudest thing on the page, for a feature the header calls off by default, so an owner who doesn't use Jira carries a permanent warning.
- **Principle:** Brief rule: the window is the owner's only way to set these things, and one home per object; recognition over recall [NNG-H6]; plain recovery path [NNG-H9]; one emphasis [SEC-5]
- **Fix:** Add a Jira card to Accounts (site URL, email, API token), built like the Bitbucket card and stored the same way. In QA, replace the note with 'Connect Jira in Accounts to use this.' plus a kit::link that sets view to Sec::Accounts. Pass warn=false to line() on this row, so it is a quiet neutral row. Keep amber for a Jira sign-in that has stopped working.

### R23 · 3 Major · QA and Reviewers load once, then go stale or stay stuck; failed loads are dropped silently
- **Unit:** QA Reviewers Worktrees
- **Element:** QA and Reviewers pages, and Worktrees on a failed load, in a long-open window
- **Evidence:** Confirmed
- **Problem:** load_qa and load_reviewers run only in SettingsWindow::new. Sidebar clicks reload System, Worktrees and Usage only, and the poll loop reloads Worktrees and System only. So 'N waiting on you' and 'Last checked Jira' freeze, a roster changed by agents is never shown, and after the owner fixes [jira] the row keeps saying 'Needs Jira' until the window is closed and reopened. If the first load fails, 'if let Ok(v)' drops the error, so the page shows 'Asking the board…' indefinitely. Worktrees also keeps old slots with no sign of a failed refresh.
- **Principle:** Visibility of system status [NNG-H1]; error recovery [NNG-H9]
- **Fix:** In the sidebar on_click, also call load_qa for Sec::Qa and load_reviewers for Sec::Reviewers, and add both to the 30-second poll branch while that page shows. Add a load-error field per page: with no data yet, replace 'Asking the board…' with 'Couldn't reach the board. Retrying…'; with old data, keep it and add a muted line 'Last updated 3:05 PM'.

### R24 · 3 Major · Retention choices render as an empty control, so Clean up now runs against a limit the owner can't see or set
- **Unit:** QA Reviewers Worktrees
- **Element:** History › Keep every event for and Keep day summaries for, above the Cleanup row (design/captures/settings/history--light-s.png)
- **Evidence:** Confirmed
- **Problem:** history_page reads h['detail_days_choices'] and h['summary_days_choices'], but the daemon's GET /history sends 'detail_choices' and 'summary_choices' (days.rs). The choices come back empty, and each row's seg is a tiny empty dot at the right edge, as the capture shows. No retention value is visible and none can be picked, yet Clean up now deletes by those values. The seg rows sit outside the three named sections, but they decide what Clean up now removes.
- **Principle:** Control missing from the only place it can be set (brief rule); visibility of system status [NNG-H1]
- **Fix:** Read the keys the daemon sends, for example pick("detail_days", "detail_choices", …), or rename the daemon's keys. Add a test that feeds the real GET /history shape through history_page and asserts four segments. Do this before R65.

### R25 · 3 Major · Pool settings and the reviewer roster are read-only here and point the owner at a CLI
- **Unit:** QA Reviewers Worktrees
- **Element:** Worktrees › Pool row and header line; Reviewers › roster rows and header line (worktrees--light-m.png, reviewers--light-m.png, worktrees_page, reviewers_page)
- **Evidence:** Confirmed
- **Problem:** The headers say 'tb project set sizes a project's pool' and 'Agents and tb reviewers change the roster'. Neither page has a control, so the owner has to leave the window or ask an agent to change a pool limit, pin a reviewer or stop asking one. The Pool sentence also leaves out low_disk_gb, the fifth pool key: free slots are trimmed early when disk runs low, and the owner can't see why.
- **Principle:** Brief rule: the window is the owner's only way to set these things; user control and freedom [NNG-H3]
- **Fix:** Replace the Pool sentence with named rows in each project's card: Min slots, Max slots, Clear build output after (days), Remove slot after (days) and Trim when free disk is under (GB). Use small stepper fields, validated as slots.rs does (min can't exceed max), and show the daemon's error in a note row. On each reviewer row add 24px btn_small actions, Pin/Unpin and Never ask/Ask again. Then rewrite both header lines without the CLI: 'Who reviews each project's PRs.' and 'Each project's reusable worktree slots, kept warm between tasks.'

### R26 · 3 Major · Nothing in these pages can be reached from the keyboard
- **Unit:** QA Reviewers Worktrees
- **Element:** Sidebar sections, QA Off/On, Clean up now, slot pills, all sizes
- **Evidence:** Confirmed
- **Problem:** Only the window root has track_focus. Sidebar rows, kit::seg items and buttons are click-only divs with no focus handle, tab stop or key handler, and no keymap uses the TaskboardSettings context. The owner can't change section, flip QA or run cleanup from the keyboard, and there is no focus style to see. Voice Control and VoiceOver support couldn't be confirmed either.
- **Principle:** WCAG 2.1.1; WCAG 2.4.7; MS-KBD: arrows move within groups
- **Fix:** Give each sidebar row and seg a FocusHandle with tab_index, and put a 2px t.accent focus ring on .focus_visible in btn_base, seg and the sidebar rows. Enter or Space activates. Up and Down move within the sidebar; Left and Right move within a seg, which takes one tab stop. Add Cmd+1 to Cmd+7 for sections, shown in tooltips, if the owner wants them.

### R27 · 3 Major · Every update failure is called "Check failed", and the reason is raw, faint and can go stale
- **Unit:** System and Update
- **Element:** Settings › System › Update row, Failed state, plus the sys-note row in the versions card; all sizes, code only (settings.rs L26, L69-71, L88-91; updater.rs L254, L280, L338-340)
- **Evidence:** Confirmed
- **Problem:** State::Failed covers a failed check, a failed download, a rejected signature, a failed swap and a failed relaunch, and all of them show "Check failed at {at}" with Retry. A security refusal ("The new app is signed by X, this one by Y") looks the same as a network blip, and Retry just downloads the same file again. The cause is shown as an 11.5px muted note, and it carries raw ureq, codesign or spctl text. When Restart to update fails, the same error appears twice: in the Update card and again in a red row in the versions card. In the excerpt sys_note is only cleared by restart_daemon, so the red row can stay after a later successful Retry. If only the relaunch fails, the bundle and daemon are already new while the running app is old, and the page still says "Check failed".
- **Principle:** Errors say what happened and what to do [NNG-H9]; system status must be true [NNG-H1]; one fact, one place [SEC-7]; error with no way to recover is 3 Major in the severity guide
- **Fix:** Add a stage to the state: Failed { stage: Check | Download | Verify | Install, error, at }. Titles: "Couldn't check for updates", "Download failed", "Update refused: it didn't pass verification", "Couldn't install {version}", each followed by "· 3:05 PM". Keep the plain sentence in the note at t.text_2, and move raw tool output behind a Copy details link. Label the button by stage: Retry for Check and Download; for Verify, replace it with "Download from GitHub ↗" pointing at the release page. Show apply errors only through the Failed state and drop the duplicate sys_note for them. Clear sys_note in check_now and at the start of apply_update.

### R28 · 3 Major · Update controls and the pill can't be reached from the keyboard
- **Unit:** System and Update
- **Element:** Settings › System › Check now, Retry, Restart to update, Restart daemon, Release notes; sidebar update pill; all sizes (kit.rs L130-150, L175-178; app.rs L2-5, L29-39)
- **Evidence:** Confirmed
- **Problem:** btn_base, link and pill_shell are plain divs with on_click only. They have no focus handle, tab stop, key handler or focus style in the excerpt, although the framework has focus handles (Input::render uses one). So nobody can Tab to Restart to update, press it with Space or Enter, or see where focus is. The page's actions need a pointer, for an owner who works in a keyboard-heavy developer setup. Buttons also have no pressed style.
- **Principle:** All functions work from the keyboard [WCAG 2.1.1]; focus is visible [WCAG 2.4.7]; keyboard rules for desktop [MS-KBD]
- **Fix:** Give btn_base, link and the clickable pill_shell a FocusHandle and tab stop. Fire on_click on Enter and Space. Draw a 2px t.accent ring with a 2px offset on focus. Add an active style (t.seg background) to btn. Also add Check for Updates… and Restart to Update to the Taskboard app menu so both commands have a keyboard route. If the app menu already has them, only the focus work is needed.

### R29 · 3 Major · A stopped board is the calmest thing on the page and offers no way forward
- **Unit:** System and Update
- **Element:** Settings › System › banner and Daemon version row, board stopped, light, 900x680 (down-system-m.png; settings.rs L46-67)
- **Evidence:** Confirmed
- **Problem:** The primary task is to get the board connected, and agents stop reporting when it's down. In the capture the banner says "taskboardd: Can't reach the task board server." with no next step. The Daemon row uses the same grey dot as "Off", and the app row beside it stays green. A version mismatch gets an amber row and a primary button, while a dead daemon gets neither, so the lesser problem is louder. One fact appears four ways with four names: "taskboardd", "task board server", "not running" / "Not connected.", and "Connecting…" in the sidebar, which also implies it is still being tried. In an installed build the fix is a quiet button labelled "Restart daemon" on something the page says isn't running. In a dev build there is no button at all, and in the mismatch case the note still says "Restart it" (settings.rs L50, L53). self.info being None also covers the moment before load_info returns, so "not running" may flash on every page open.
- **Principle:** Error states say what to do [NNG-H9]; same thing, same name [NNG-H4]; one fact, one place [SEC-7]; recovery from the primary task's failure is 3 Major
- **Fix:** Use one name, "daemon". Banner text: "The daemon isn't responding, so the board can't update." In the Daemon row use a t.down dot, a t.down_soft background and the status "Not responding". Make the button primary and label it "Start daemon" when info is None; keep it in this row so the action has one home. In a build with no bundle, replace the button with a note naming the command to run, and drop "Restart it" from the mismatch note. Add a Loading state so the row shows "Checking…" until the first load_info returns.

### R30 · 3 Major · The menu can't be opened or fully used from the keyboard
- **Unit:** Hours menu
- **Element:** Main window › footer › work hours pill, and Hours menu › checkbox, day buttons, am/pm chips; all sizes and schemes
- **Evidence:** Confirmed
- **Problem:** The pill, checkbox, seven days and chips are click-only divs with no focus handle or key handler (hours.rs 220-247, 361, 461-488, 509). The menu bar has no work hours item (main.rs 29-70). Opening the menu moves focus nowhere, and Enter or Esc in a field sends focus to the main window (m.focus) while the menu stays open, so focus falls behind it. Rated 3 rather than 4 because the only user has a pointer. Key handling in app.rs and Tab handling in text_input.rs are outside the excerpts; hours.rs itself has no Tab handling.
- **Principle:** Every function works from the keyboard [WCAG 2.1.1]; focus moves into opened UI and returns to the opener [WCAG 2.4.3]; every command is reachable from a menu [HIG-MACOS] [MS-KBD]
- **Fix:** Give the pill a FocusHandle with tab_index, opening on Space or Enter, and add a View menu item 'Work Hours…'. On open, focus the checkbox. Make the checkbox, the From and to fields, the days (one tab stop, Left and Right move, Space toggles) and Today until one Tab loop inside the menu. Keep Space on the chips, or rely on typing a or p and take them out of the Tab order. After Enter, keep focus on the field. On close, return focus to the pill.

### R31 · 3 Major · Today until can run agents 12 hours too long, and doesn't say what it will do
- **Unit:** Hours menu
- **Element:** Hours menu › Today until field; hours-today-field-light-m.png, hours-on-light-m.png
- **Evidence:** Confirmed
- **Problem:** An unset field always starts on pm (hours.rs 64-65), and a time earlier than now becomes tomorrow's (hours.rs 5-6). At 11pm, typing 1 to mean 1am gives 1pm tomorrow; at 3pm, typing 12 to mean midnight gives noon tomorrow. Agents keep starting overnight, and the only sign is the pm chip and the pill text below the menu. The placeholder 'Usual end' hides the actual time and shows even in a closed state (Agents off until Mon 9am), and clearing an override has no control: you must delete the text and leave the field. What the board does with Today until on a day off isn't in the excerpts.
- **Principle:** Prevent errors with sensible defaults [NNG-H5]; show the result of the action [NNG-CW] [NNG-H1]; recognition rather than recall [NNG-H6]
- **Fix:** When no half is typed, pick the half that gives the soonest time after now, so 1 at 11pm is 1 AM and 12 at 3pm is 12 AM, with the chip following. While the field is focused or set, show one line under it with the resolved end, such as 'Until tomorrow 1 AM', in 12.5px muted text. Make the placeholder the real value ('5:00 PM'), or 'Not a work day' when today has no hours. When a value is set, add a 24px ✕ in the field, with the tooltip 'Back to usual end'.

### R32 · 3 Major · A failed save leaves unsaved values on show, or is lost when the menu closes
- **Unit:** Hours menu
- **Element:** Hours menu › every control; the save() error path; any size
- **Evidence:** Confirmed
- **Problem:** Every change writes to the menu's copy first. If POST /hours fails, the error text shows but the checkbox, day or time stays in its unsaved state, so the menu and the pill disagree (hours.rs 252-270). When the menu closes, close_fields commits the typed time, but the error then lands in m.hours.err with no menu to show it, and the next open rebuilds from the board. The owner walks away believing the hours changed, which decides whether agents start. How the failure looks with the board stopped wasn't captured.
- **Principle:** System status is shown and commands never report success they didn't have [NNG-H1]; errors say what happened and how to recover [NNG-H9]; unsaved input survives in-app actions [NNG-H3]
- **Fix:** On failure, put the failed field back to the value in m.state()['work_hours'] and show 'Couldn't save 7:30 PM: <reason>' with a Retry link in the menu. If the menu has closed, reopen it at the pill with that message, so a failure can't go unseen.

### R33 · 3 Major · Clicking the pill goes to Days, not to where limits are set
- **Unit:** Usage pill and footer
- **Element:** Main window › sidebar footer › usage pill (usage-pill-go), all states
- **Evidence:** Confirmed
- **Problem:** The click runs m.go(Page::Days) (sidebar.rs line 241). The primary task is to check and then adjust usage limits, and the Settings window is the only place to set them. So an owner who sees 96% and clicks lands on a page that, under that rule, can't change the limit, and must then find Settings separately. It also breaks the footer's own pattern: the accounts pill opens Settings and the update pill opens Settings › System. Nothing announces the destination either. The tooltip lists only readings, and the pill has no hover style, unlike the Settings link beside it, so it looks like a passive meter.
- **Principle:** Will users associate the action with the result they want? [NNG-CW]; consistency and standards [NNG-H4]; one home per object (brief rule); hover and pressed states designed [NNG-H1]
- **Fix:** Dispatch a new OpenUsageSettings action, modelled on OpenSystemSettings, that opens the Settings window on the pane holding the usage limits. Add a last tooltip line, 'Click to change limits', and a hover style (background t.border, or a t.border_2 border in the pill's existing transparent 1px border). If Days holds usage history worth keeping, link to it from that Settings pane.

### R34 · 3 Major · The window labels fail text contrast in every state
- **Unit:** Usage pill and footer
- **Element:** Main window › sidebar footer › usage pill › '5h', '7d', 'Today' labels, and the 'Lifted' value, light and dark (pill-normal-light-m.png, pill-hot-dark-s.png)
- **Evidence:** Confirmed
- **Problem:** Labels are 11.5px in t.faint on t.seg: 2.57:1 in light and 3.36:1 in dark, against the 4.5:1 required. These labels are the only thing that says which limit each number belongs to, and the owner reads them all day. 'Lifted' in t.accent is 4.31:1 in light, also under. The empty bar track is nearly invisible too (1.25:1 light, 1.44:1 dark) and the green fill is 2.96:1 on it in light, though the number beside it carries the same information.
- **Principle:** Contrast (minimum) [WCAG 1.4.3]; non-text contrast [WCAG 1.4.11]
- **Fix:** Set the labels to t.muted (4.58:1 light, 6.30:1 dark). Set the Lifted value to t.accent_fg (6.64:1 light, 7.90:1 dark), keeping t.accent for the bar.

### R35 · 3 Major · The stale state is too faint to read and never says it is stale
- **Unit:** Usage pill and footer
- **Element:** Main window › sidebar footer › usage pill, stale state (pill-stale-light-m.png)
- **Evidence:** Confirmed
- **Problem:** A reading over an hour old only gets .opacity(0.6). Computed over the sidebar, values fall to 2.99:1 in light and labels to 1.69:1 (4.17:1 and 2.05:1 in dark). The pill then looks disabled rather than out of date, and it still shows '34%' as a plain number. 'Last read …' exists only in the tooltip. While many agents run, usage can climb a long way in an hour, so an owner glancing at a faded 34% can trust a number that is no longer true, and the fade also weakens warm and hot colours just when a stale reading is riskiest.
- **Principle:** Visibility of system status [NNG-H1]; contrast [WCAG 1.4.3]; colour isn't the only signal [WCAG 1.4.1]
- **Fix:** Remove .opacity(0.6). When stale, draw values in t.muted, draw bars in t.faint, and add a line inside the pill under the columns: 'Last read 3h ago' at 11.5px in t.warn_fg (6.18:1 light, 8.56:1 on dark seg). Use the pill's transparent 1px border in t.warn_line. Stale is rare, so the extra line costs little height.

### R36 · 3 Major · First launch has no setup home, no order and no progress
- **Unit:** First launch
- **Element:** Whole flow › Settings › System, the sidebar footer and the banners (system--light-m.png, main-sample-light-m.png, down-main-light-m.png), 900x680 and 1360x860
- **Evidence:** Confirmed
- **Problem:** Nothing lists the five setup steps or says which are done. Settings › System covers only updates and versions, with no row for the login item, tb, hooks or Midna. Each step reports somewhere different: a banner for the service, footer text for hooks, a pill for Midna, nothing for tb. The accounts pill only covers saved accounts (accounts::attention skips the rest), so an owner with no accounts gets no prompt to sign in. Steps also depend on each other without saying so: hooks report only from Midna terminals, and accounts need the service. Install hooks also lives in the main window rather than in Settings.
- **Principle:** Users must notice the next action and see progress toward the goal [NNG-CW]; visibility of system status [NNG-H1]; recognition rather than recall [NNG-H6]; everything frequent in view and in one place [SEC-8]. The brief's rule: one home per object.
- **Fix:** Add a 'Setup' card at the top of Settings › System, with one row per step in dependency order: 1 Background service, 2 tb command, 3 Midna, 4 Claude Code hooks, 5 Accounts. Each row has a state dot, one plain sentence, and one action (Open Login Items, Try again, Install hooks, Go to Accounts). Label the card above its rows, per L1. Give the System nav item the existing amber count of incomplete steps. Open Settings on this card at launch while a required step is incomplete. The footer hooks item becomes a status link that opens this card, so hooks have one home. Update Sec::System's about text to match.

### R37 · 3 Major · The Login Items banner stays after the owner approves
- **Unit:** First launch
- **Element:** Main window › banner › 'Switch on Taskboard in System Settings' (LoginItem::RequiresApproval)
- **Evidence:** Confirmed
- **Problem:** The owner follows the banner and switches Taskboard on, but the app never looks again. set_status runs only at launch and in daemon_unreachable, and that function returns early unless the status is already Enabled. The banner keeps telling the owner to do something already done until the app is relaunched, and nothing says a relaunch is needed. After the step, the owner sees no sign it worked.
- **Principle:** After the action, users see progress toward the goal [NNG-CW]; visibility of system status [NNG-H1].
- **Fix:** While the status is RequiresApproval or NotRegistered, re-read svc.status() every 3 seconds (the existing POLL) and whenever the app regains focus, and call set_status with the result. Clear the banner on Enabled and show the toast 'Background service is on'. Let daemon_unreachable run for these two states as well as Enabled.

### R38 · 3 Major · Blocked and failed service states are dead ends with conflicting advice
- **Unit:** First launch
- **Element:** Main window › banners › LoginItem::Failed, Blocked, NotRegistered and in_the_way rows
- **Evidence:** Confirmed
- **Problem:** Failed has no button, no Copy and no next step, and it is never retried, even for 'The launch agent isn't in the app bundle.' NotRegistered shows no banner at all (the match falls to `_ => {}`). Blocked shows two amber bars for one cause, on top of any red 'Can't reach' bar. One says stop the other board and reopen; the other says launchctl bootout or edit config.toml. The second sends the owner outside the window, against the brief's rule. The launchctl and claude plugin uninstall commands sit in bars with no Copy, unlike the down bar. in_the_way is only computed at launch, so a fixed problem stays listed until relaunch.
- **Principle:** Help users recognise, diagnose and recover from errors [NNG-H9]; no dead ends [NNG-H3]; one fact, one place [SEC-7].
- **Fix:** Show one row per cause, in the R36 Setup card, with the banner linking to it. Blocked: 'Port 8792 is in use by <what>. Stop it, then try again.' with Try again (re-runs check_in_the_way and register, no relaunch) and Copy command. Drop the separate Blocked sentence so port_taken's text is the only copy. Failed: show the reason plus Try again and Copy. NotRegistered: show 'Background service isn't registered' with Register. Re-run in_the_way on Try again and on focus. Needs the owner's OK: replace the config.toml advice with a Port field in Settings › System.

### R39 · 3 Major · Hooks failures vanish in 6 seconds, and the stated reason can mislead
- **Unit:** First launch
- **Element:** Main window › sidebar footer › 'Install hooks' / 'Reinstall hooks'
- **Evidence:** Confirmed
- **Problem:** This step decides whether any session reports to the board, yet it is small 12.5px text in the footer with its explanation in a tooltip. A failure such as 'Couldn't find the claude command.' appears only as a TOAST_ERR toast for 6 seconds, then the item returns to 'Install hooks' with no reason kept. The Reinstall tooltip always opens with 'The hooks are out of date', even when the cause is a settings.json that doesn't parse or a plugin switched off in Claude Code. Reinstall can't fix the unparseable file, because read_settings refuses it, so the owner loops. The reason sentences start with 'It' or 'Its' with nothing to refer to.
- **Principle:** Plain errors that say what happened and what to do [NNG-H9]; status stays visible [NNG-H1]; match the real cause [NNG-H2].
- **Fix:** Move Install to the R36 Setup row and keep the last error there until the next attempt, with Copy. Split NeedsReinstall into causes with their own action: 'settings.json has a syntax error at <path>. Fix it, then check again' (Check again, no Reinstall); 'The plugin is off in Claude Code' (Turn on); 'Hooks run another copy of tb' (Reinstall). Name the subject in each sentence ('The task-board plugin…'). For 'claude not found', say 'Install Claude Code, then try again' with Try again.

### R40 · 3 Major · 'Midna isn't running' gives a new owner no way forward
- **Unit:** First launch
- **Element:** Main window › sidebar footer › conn-pill (main-sample-light-m.png)
- **Evidence:** Confirmed
- **Problem:** The pill can't be clicked and its help is a tooltip. midna::up only means Midna was heard from recently, so an owner who has never installed it sees the same label as one whose Midna has quit. The tooltip then reads 'Not heard from yet… wait until it's back', which is wrong for something that was never there. Nothing says what Midna is, that sessions report only from Midna terminals, or how to get or start it.
- **Principle:** Users must know the step is needed and notice the action [NNG-CW]; help in context [NNG-H10]; recognition rather than recall [NNG-H6].
- **Fix:** Make the pill open the R36 Setup row. Split the label on seen_at: null gives 'Midna not set up' and 'Taskboard starts terminals through Midna. Install it, then open it.' with a link to its install page; a past value gives 'Midna isn't running' and 'Last heard from 5 minutes ago.' Needs the owner's OK: an 'Open Midna' button, which adds a launch action the app doesn't have today.

### R41 · 3 Major · Sign-in and Connect are likely cut off below about 830 wide
- **Unit:** First launch
- **Element:** Settings › Accounts › GitHub row and token rows, 720x460 up to about 830 wide
- **Evidence:** Likely
- **Problem:** Rows don't wrap: line() keeps a 150px minimum, controls() is flex_none, and the card has overflow_hidden. Measured from accounts--light-m.png, the GitHub row needs about 511px and the token row about 507px, but the card has about 403px at the 720 minimum. So roughly 106px of 'Sign in with GitHub' and all of Connect (about 73px wide) would be clipped, with no sideways scroll. Enter still submits a token field, but the GitHub browser sign-in has no such alternative. Widening the window is the workaround. I worked this out from code and capture measurements; there is no 720 capture.
- **Principle:** The layout reflows across the whole window range [HIG-MACOS]; every control at every size [NNG-H6].
- **Fix:** Add flex_wrap to row(), so controls drop under the text when they don't fit. Change the 260px fields to flex_1 with min 160 and max 260. In the GitHub row, keep Sign in with GitHub on the right and move 'Use my gh sign-in' and 'Use a token' to a second line. Add a 720x460 capture to the review set.

### R42 · 3 Major · Placeholder and URL text fail contrast on the sign-in forms
- **Unit:** First launch
- **Element:** Settings › Accounts › field placeholders (theme.dim) and link-row URLs (t.faint), light and dark (accounts--light-m.png)
- **Evidence:** Confirmed
- **Problem:** #8a95a3 on the #ffffff card is about 3.04:1 in light. #6f7a88 on #1a1f26 is about 3.80:1 in dark. Both are under 4.5:1 at 12–13px. The placeholders carry format hints the owner needs, such as 'ATATT…' and 'your-org or dev.azure.com/your-org'. Ratios were worked out by hand from the theme values with the WCAG formula.
- **Principle:** Text contrast at least 4.5:1, placeholders included [WCAG 1.4.3].
- **Fix:** Use t.muted (#5f6b7a, about 5.4:1 on white) for dim and for the URL text in light mode. In dark, use #a1abb8, the existing muted value, which clears 4.5:1 on #1a1f26. Recheck any other use of faint on cards.

### R43 · 3 Major · Setup controls can't be reached from the keyboard
- **Unit:** First launch
- **Element:** Main window › Install hooks, Open Login Items, accounts pill; Settings › nav items, Sign in with GitHub, Connect, Check, Agent commands
- **Evidence:** Confirmed
- **Problem:** These controls are plain divs with on_click. In the app's source only the window roots, text fields and the backlog call track_focus, and there is no tab_index or tab_stop. Text fields submit with Enter, but the rest of setup needs the mouse, including GitHub sign-in and moving between Settings sections. This owner is a keyboard-heavy developer on a desktop app.
- **Principle:** Every function works from the keyboard [WCAG 2.1.1]; focus is visible [WCAG 2.4.7]; keyboard conventions [MS-KBD].
- **Fix:** Give kit::btn_base, kit::link and the Settings nav rows a focus handle with tab_index(0), Enter and Space activation, and a 2px accent focus ring. Make the nav list one tab stop with Up and Down arrows. Do the footer items and banner buttons the same way. Start with the R36 Setup card and Accounts.

### R44 · 2 Minor · Fixed page header takes a third of the minimum window
- **Unit:** Settings shell
- **Element:** Settings › page header and banner at 720x460 (accounts--light-s.png, down-accounts-s.png)
- **Evidence:** Confirmed
- **Problem:** At minimum size the Accounts description wraps to three lines because Agent commands takes width beside it. The header is flex_none outside the scroller, so it uses about 100px of 460, and with the banner about 150px. Only one card and part of the next are visible, and the content the owner came for gets the leftovers. Agent commands also sits low, aligned to the last text line rather than the title (`items_end`).
- **Principle:** The work dominates [SCR-1]; short heights leave room for content [checklist pass 7]
- **Fix:** Keep only the title pinned and move the about text into the top of each page's scroller. Put Agent commands on the title row with `items_center` so the description gets the full width (two lines at 720). That returns about 50px at minimum height, and the text scrolls away after that.

### R45 · 2 Minor · Sidebar order and landing section don't match how the window is used
- **Unit:** Settings shell
- **Element:** Settings › sidebar order; default view Sec::Accounts
- **Evidence:** Confirmed
- **Problem:** Accounts is one-time setup, and the recurring job is checking usage. Yet Usage sits third, below History, a rarely touched retention page, and the window always opens on Accounts. Every daily visit takes an extra click and passes a rarely used item. Usage also has no attention count, so a day that hit its limit isn't visible from the sidebar, though Accounts and System each get one.
- **Principle:** Frequent in view, rare behind [SEC-8]; members grouped by function then frequency [SEC-1]
- **Fix:** Reorder SECS to Accounts, Usage, Reviewers, Worktrees, QA, History, System (with Work hours after Accounts if R9 is accepted). Open on the last section used, or on Accounts only while an account is missing or needs_owner is true. Show the amber count on Usage when `limited` is true.

### R46 · 2 Minor · Window size, position and section are forgotten on every open
- **Unit:** Settings shell
- **Element:** Settings › open(): window bounds and view
- **Evidence:** Confirmed
- **Problem:** Each open uses Bounds::centered at 900x680 and resets to Accounts with Agent commands off. An owner who keeps it beside the board on a wide display has to place it again each time. The default also hides the Slack card and the Keychain note below the fold (accounts--light-m.png).
- **Principle:** Windows can be resized and moved and should behave like standard Mac windows [HIG-MACOS]; flexibility for frequent users [NNG-H7]
- **Fix:** Save window bounds, the last section and `cli` in prefs.rs on close and on section change, and restore them in open(), clamped to the current display and to 720x460.

### R47 · 2 Minor · Typed sign-in details vanish on Esc or close without warning
- **Unit:** Settings shell
- **Element:** Settings › Accounts › token, email and organization fields; window close
- **Evidence:** Confirmed
- **Problem:** field_keys clears the field on Esc at once, with no undo, including a long pasted token or a filled email. Closing with ⌘W or the red button drops everything typed, because the Inputs live in the window. Getting a token back can mean creating a new one, since services show it only once. Switching sections does keep drafts, which is right.
- **Principle:** User control and freedom; unsaved input survives or asks first [NNG-H3] [NNG-H5]
- **Fix:** Make Esc in a field blur it and leave the text. Offer clearing only through a small clear button inside the field. When any token field has text at close, use on_window_should_close to ask 'Discard the token you entered?' with Discard and Keep editing, Keep editing as the default.

### R48 · 2 Minor · The Needs a new sign-in row can read as broken and offers a route that doesn't exist
- **Unit:** Accounts
- **Element:** Accounts › GitHub card › Needs a new sign-in row (acct-warm-light-m.png; reauth_row, github_rows)
- **Evidence:** Confirmed
- **Problem:** The sentence is built as 'Taskboard now needs {list}.', so an empty list gives 'Taskboard now needs . Sign in again'. That text is in acct-warm, which is stand-in data, so whether the real board sends an empty list is unknown, but the template has no guard. It also explains missing scopes when the shown cause is a refused sign-in. It says 'or use a token that has them', yet with gh present the row has only Sign in again and no way to open the token form. When the list isn't empty, the chips repeat it.
- **Principle:** Help users diagnose and recover from errors [NNG-H9]; users notice the correct action is available [NNG-CW]; one fact, one place [SEC-7]
- **Fix:** Branch on the cause. Empty list: 'GitHub refused the saved sign-in. Sign in again to keep agents working.' List present: 'Taskboard now needs repo, workflow.' with no chips. Add a secondary 'Use a token' button beside Sign in again, setting gh_form, or drop the phrase.

### R49 · 2 Minor · A connected account with a bad token can only be fixed by disconnecting it first
- **Unit:** Accounts
- **Element:** Accounts › signed-in cards, error or expiring token (github_rows, bitbucket_rows, azure_rows, slack_rows; needs_owner)
- **Evidence:** Confirmed
- **Problem:** The token form opens on a connected account only through a reauth button. If error is set without reauth, the card shows an amber message and only Check and Disconnect. slack_rows has no reauth branch at all, yet needs_owner still counts it, so the sidebar badge could point at a card with no amber row. Rotating a working token also means disconnecting first, which leaves agents without access in between. Whether the board sets error without reauth for an expired token wasn't visible.
- **Principle:** Error with a way to recover [NNG-H9]; user control [NNG-H3]
- **Fix:** Add a 'Replace token' item to every signed-in row (a secondary button or a '…' menu) that opens that service's form and keeps the current token until the new one passes its check. Show the same as the primary action in the error row. Give Slack the same reauth row as the others.

### R50 · 2 Minor · Check shows no result, and success notes never clear
- **Unit:** Accounts
- **Element:** Accounts › signed-in row › Check, and the card note row (signed_in_row, post, connect)
- **Evidence:** Confirmed
- **Problem:** Check's success callback is empty, so a healthy result looks like nothing happened: 'Checking…' just goes back to 'Check'. The owner can't tell a good result from a dropped click, and this is the main way to confirm that agents will keep working. In the other direction, notes such as 'Connected as…' stay until the next post for that account. In a window left open all day, they become stale claims.
- **Principle:** Visibility of system status; after the action, users see progress [NNG-H1] [NNG-CW]
- **Fix:** On a successful check, set a note 'Token works · checked 2:41 PM' (12-hour, with AM/PM). Clear success notes after about 8 seconds or when the owner leaves the section; keep error notes until resolved.

### R51 · 2 Minor · Actions are silently dropped while another account is busy
- **Unit:** Accounts
- **Element:** Accounts › Connect, Check, Disconnect, Sign in, Cancel across cards (post; busy compared per id)
- **Evidence:** Confirmed
- **Problem:** post() returns at once if any account is busy, but each button only disables for its own id. Pressing Connect on Slack while GitHub is checking looks enabled and does nothing, with no message. Cancel in the code panel clears opened_code before its post is dropped, so the panel stays and the next accounts answer copies the code to the clipboard again.
- **Principle:** Pressing a control responds visibly [NNG-H1]; error prevention [NNG-H5]
- **Fix:** Make busy a set of account ids so accounts run independently. If one at a time must stay, disable every account action while busy is set, and in Cancel do the local reset only after the post is accepted.

### R52 · 2 Minor · Connect's validation error is far from the field and reports one problem at a time
- **Unit:** Accounts
- **Element:** Accounts › Bitbucket card › Connect with empty fields, 900x680 (accounts-connect-empty-light-m.png; note row in accounts_page)
- **Evidence:** Confirmed
- **Problem:** 'Paste a token first.' appears at the very bottom of the card, under the link row, and the fields aren't marked. Focus doesn't move. In the capture the email is empty too, but only the token is mentioned. At short window heights the note can land below the fold with nothing scrolling to it (inferred from the layout, not captured). The check also needs a round trip to the board.
- **Principle:** Errors name the field, say how to fix it, next to the field [WCAG 3.3.1] [WCAG 3.3.3] [NNG-H9]; prevent errors [NNG-H5]
- **Fix:** Validate empty fields in connect() before posting. Show the message directly under the empty field, in t.down with a t.down field border. Cover every empty field at once, for example 'Enter your Atlassian email' and 'Paste an API token'. Focus the first empty field and scroll it into view. Keep the card-level note for errors from the service.

### R53 · 2 Minor · An opened token form can't be closed
- **Unit:** Accounts
- **Element:** Accounts › GitHub Token form and Use a new token forms (accounts-use-token-light-m.png; github_rows, field_keys)
- **Evidence:** Confirmed
- **Problem:** Use a token disappears once the form opens, and nothing replaces it. Esc only clears the field, so the form stays until a successful Connect. In the reauth path the amber row's explanation also disappears for Bitbucket and Azure while the form is open. A successful connect on any account closes all three forms, which hides text typed into another form. Esc also wipes a pasted token with no undo.
- **Principle:** User control and freedom [NNG-H3]; unsaved input survives in-app actions [NNG-H5]
- **Fix:** Add a Cancel button beside Connect that sets the form flag false and clears only that field. Make Esc do the same on an empty field, and only clear on a first press if the field has text. In connect(), reset only the flag for the id that connected. Keep the amber row visible above the form.

### R54 · 2 Minor · Agent commands repeats, truncates and hides its own toggle
- **Unit:** Accounts
- **Element:** Accounts › Agent commands toggle and command lines, 900x680 (accounts-agent-commands-light-m.png; cli_line, page_header)
- **Evidence:** Confirmed
- **Problem:** 'tb accounts' is repeated in every card, adding two boxes per card to an already long page. The useful line is cut off ('repos/OWNER/REPO/…') with no way to read the rest. The tooltip covers the toggle's own label and still says 'Show' while the commands are on. Copied state is matched on the text, so copying one 'tb accounts' flips all four to Copied. Commands also show on accounts that aren't connected, where they can't work.
- **Principle:** One fact, one place [SEC-7]; recognition rather than recall [NNG-H6]; state is shown [NNG-H1]
- **Fix:** Show 'tb accounts' once at the top of the page and keep only the agent_cli line in each card, and only when that account is connected. Use kit::clipped so a cut line shows in full on hover, or wrap it. Key the copied flash on the line id. Make the tooltip 'Hide the commands…' when on, and anchor it below the button so it doesn't cover the label.

### R55 · 2 Minor · Scope chips look like buttons and silently drop scopes past six
- **Unit:** Accounts
- **Element:** Accounts › signed-in row › scope chips, 900x680 (acct-normal-light-m.png, acct-warm-light-m.png; signed_in_row, kit::chip)
- **Evidence:** Confirmed
- **Problem:** The chips share the outlined rounded look of Check and Disconnect, sit right next to them and wrap to two rows, so read-only labels look clickable and take the row's main space. take(6) hides any further scopes with no '+n', which matters when the question is whether a token has a scope.
- **Principle:** One look per role; no label that looks like a button [SEC-4]; frequent in view, rare behind [SEC-8]
- **Fix:** Drop the chips from the signed-in row. Show scopes as plain muted text in the note line ('repo, read:org, workflow, gist…') with the whole list in a kit::clipped tooltip, or behind a 'Scopes' disclosure. Use the same treatment in off_row.

### R56 · 2 Minor · Placeholders and link-row URLs look under 4.5:1
- **Unit:** Accounts
- **Element:** Accounts › field placeholders and the URL beside each link, light theme (accounts--light-m.png; t.faint in link_row)
- **Evidence:** Likely
- **Problem:** Placeholders such as 'ATATT…' carry format hints, and the URLs show where a link goes, but both are pale grey on white, around 3:1 by eye. Theme values weren't in the excerpts, so this is not measured.
- **Principle:** Text contrast at least 4.5:1 [WCAG 1.4.3]
- **Fix:** Compute t.faint and the placeholder colour against t.card in both themes. If either is under 4.5:1, use t.muted for link_row's URL and the placeholder.

### R57 · 2 Minor · Changing a limit setting silently restarts today's count and drops the carry
- **Unit:** History and Usage
- **Element:** Settings › Usage › Daily limit and Unused share, any change mid-day (limit on)
- **Evidence:** Confirmed
- **Problem:** The plan is keyed to the setting (`sig`), so `advance` discards it on any change and starts fresh with `day_pct` at the current reading. Today's used amount returns to 0, and carried share is lost. Switching from Next day to Spread at 4 PM can hand agents a full extra day's share, or a limit that was reached stops holding, and the page gives no hint.
- **Principle:** No surprises from a control [WCAG 3.2.2 in spirit]; error prevention [NNG-H5]; visibility of status [NNG-H1].
- **Fix:** In `advance`, when only the setting changed within the same weekly window, keep the kept plan's `day_pct` so today's used count survives. If restarting is intended, say so in the Daily limit note: 'Changing this restarts today's count.'

### R58 · 2 Minor · The Today row repeats its label, uses a mode switch for a one-day action, and sits last
- **Unit:** History and Usage
- **Element:** Settings › Usage › Today row, limit on and lifted (usage-daily-light-m.png, usage-lifted-dark-m.png)
- **Evidence:** Confirmed
- **Problem:** The label 'Today' sits above text that starts 'Today: …' or 'Today's limit…', and `usage_daily::line` confirms the real wording does too. 'Limit / Lifted' looks like a lasting mode, but lifting expires at midnight, and 'Limit' doesn't say what clicking it does. This is the row checked all day and the only one that goes amber, yet it comes below two rarely changed settings.
- **Principle:** Project rule: no filler labels; frequent in view [SEC-8]; label matches the result [NNG-CW]; consistent wording [NNG-H4].
- **Fix:** Have the daemon add a `today_line` without the prefix ('7% used of 12%, 5% left'; 'Lifted until midnight: 7% used of 12%') and keep `line` for the pill tooltip. Replace the toggle with one button: 'Lift for today', or 'Put limit back' while lifted. Move the row to the top of the card when the limit is on, followed by Daily limit and Unused share.

### R59 · 2 Minor · With no weekly reading, Limit / Lifted looks live but does nothing visible
- **Unit:** History and Usage
- **Element:** Settings › Usage › Today row, limit on with no 7-day reading
- **Evidence:** Confirmed
- **Problem:** `state()` returns before adding `overridden` when `now()` is None, so the row shows 'No 7-day reading yet.' beside an enabled toggle. Clicking 'Lifted' posts the override, but the answer has no `overridden`, so the toggle snaps back to 'Limit' with no message. 'Daily limit' can also be set with nothing to measure it against, and nothing says the limit holds nothing until a reading arrives. '7-day' is also a different word from the page's 'weekly'.
- **Principle:** Feedback after an action [NNG-H1]; a state must not contradict its controls (checklist, simulated state rule); consistent words [NNG-H4].
- **Fix:** When `d["allowed"]` is missing, hide the toggle and show: 'No weekly reading yet. Nothing is held until one arrives.'

### R60 · 2 Minor · Saving shows no pending state, and clicks during a call are dropped
- **Unit:** History and Usage
- **Element:** Settings › Usage › all controls while `daily_busy`; History › retention rows while `history_busy`
- **Evidence:** Confirmed
- **Problem:** On Usage, `busy` only removes the click handler. Segments keep their pointer cursor and hover, and nothing shows that a save is under way, so a click on a slow board seems to do nothing. On History, `post_history` returns early when busy, so a retention click during 'Cleaning up…' is dropped silently and the selection stays as it was.
- **Principle:** States are designed, a slow action shows progress [NNG-H1]; disabled controls look disabled (pass 3).
- **Fix:** Add a `busy` argument to `kit::seg` that applies the same `opacity(0.45)` and `cursor_default` as `kit::disabled` to unselected items. Mark the clicked option as selected straight away, and revert with the error note on failure. Pass `history_busy` into `pick` so retention rows disable during a cleanup.

### R61 · 2 Minor · The percent choices give no sense of what they mean for the week
- **Unit:** History and Usage
- **Element:** Settings › Usage › Daily limit, off and on (usage--light-m.png, usage-daily-light-m.png)
- **Evidence:** Confirmed
- **Problem:** The note says a percent gives each day that share of the week, but over seven days 15% is 105%, 20% is 140% and 25% is 175%, so those choices can run out before the reset. 10% leaves 30% unused. Choosing needs the week's use and reset, which the page doesn't show, though `resets_at` and `days_left` are already in the response. With the limit off the page shows no numbers at all. Owner decision: the weekly reading's home is the board's pill.
- **Principle:** Recognition rather than recall [NNG-H6]; error prevention [NNG-H5].
- **Fix:** Make the note follow the choice, as Unused share's already does. For example, 20%: 'Each day gets 20% of the week, so the week lasts about 5 days.' Even: 'Splits what's left of the week over the days to its reset.' Optional, needs the owner's OK under one home per object: add a line to the Usage row, 'Week: 42% used, resets Sun 3:00 PM.'

### R62 · 2 Minor · Rows stretch with the window and part labels from their controls
- **Unit:** History and Usage
- **Element:** Settings › History and Usage › cards at 1400x1000 (history--light-l.png, usage--light-l.png)
- **Evidence:** Confirmed
- **Problem:** Cards fill the whole pane, so at 1400 wide the row's text and its control sit roughly 1,000px apart for 'Cleanup' and over 700px for 'Daily limit'. The eye has to track a long way along a row to pair a setting with its value, and notes run to long lines. The rest of the window stays empty.
- **Principle:** Desktop window range: nothing stretches past a readable length at the maximum [tablet:AND-T2] [NNG-FITTS].
- **Fix:** Cap the rows with `.max_w(px(680.))` on the `history-rows` and `usage-rows` content, left-aligned with the page title, across all Settings pages for consistency.

### R63 · 2 Minor · QA's Off/On looks live when it's dead, can get stuck On, and shows no saving state
- **Unit:** QA Reviewers Worktrees
- **Element:** QA › Off/On control, no Jira or qa_busy (qa--light-s.png, qa_page, kit::seg)
- **Evidence:** Confirmed
- **Problem:** Without Jira, clicks are simply not attached. The segments keep cursor_pointer, the hover colour and full-strength text, so a click does nothing and gives no reason. If Jira is removed while QA is on, the control shows On with no way to turn it off. During a POST the control looks unchanged until the answer arrives.
- **Principle:** Disabled controls look disabled [NNG-H1] [WCAG 1.4.11]; user control [NNG-H3]
- **Fix:** Add a disabled option to kit::seg that applies opacity(0.45) and cursor_default and drops hover, and use it when !jira or qa_busy. Always leave the Off segment clickable while on is true, so the feature can be turned off without Jira. While qa_busy, show the target segment as selected at reduced opacity.

### R64 · 2 Minor · Empty states hide real information and give no way forward
- **Unit:** QA Reviewers Worktrees
- **Element:** Worktrees and Reviewers, empty (worktrees--light-m.png, reviewers--light-m.png)
- **Evidence:** Confirmed
- **Problem:** pools_vm builds pools only from v['slots'], so a project with pool settings but no slots disappears, along with its limits. Waiting tasks are read from slot rows too, so tasks waiting in a project with no slots are never listed. 'No reviewers' and 'No worktree slots' don't say whether that's normal or what happens next. A project whose reviewers array is empty renders its heading over an empty bordered card. If a project's pool entry is missing, the row is a bare 'Pool' label with no text.
- **Principle:** Designed empty states with a way forward [mobile:NNG-EMPTY]; visibility of status [NNG-H1]
- **Fix:** Build pools from the keys of v['pools'] as well as the slots, so every project keeps its heading and settings, with a row 'No slots yet. One is made when a task starts.' Replace the page text with 'No projects have worktree slots yet. Slots appear when a task starts.' and 'No reviewers yet. They're added from each project's PRs.' (confirm the real trigger). Skip a project's card, or show 'No reviewers' inside it, when its list is empty.

### R65 · 2 Minor · Clean up now deletes permanently with no preview or confirm, and reports less than it removes
- **Unit:** QA Reviewers Worktrees
- **Element:** History › Cleanup row › Clean up now (design/captures/settings/history--light-s.png, days.rs cleanup)
- **Evidence:** Confirmed
- **Problem:** One click permanently deletes rows, and there is no count beforehand and no undo. The note reports events and day summaries, but cleanup also deletes usage samples, host limits and slot spans. It deletes terminal lines (session_events) by date alone, not just for finished tasks, while the page copy says 'of finished tasks' and never mentions terminal output. The header says 'cleans up old work', yet worktree slots aren't touched, so an owner short of disk may expect otherwise. It stays at 2 because the nightly run would remove the same rows anyway.
- **Principle:** Destructive actions confirm or undo [NNG-H3] [NNG-H5]; no false status [NNG-H1]
- **Fix:** Show a confirm before running: 'Remove 1,240 events older than 90 days and 12 day summaries older than 1 year? This can't be undone.' with Cancel focused and a Remove button using kit::btn_danger, fed by a dry-run count from the daemon. Change the row note to 'Removes events and terminal output older than the limits above. Doesn't touch worktrees.' Report every table in the result note and pluralise it, so it never says '1 old events'.

### R66 · 2 Minor · Pinned and Never asked look identical
- **Unit:** QA Reviewers Worktrees
- **Element:** Reviewers › reviewer row pills (reviewers_page, kit::tone)
- **Evidence:** Confirmed
- **Problem:** reviewers_page asks for tone 'review', which kit::tone doesn't define, so it falls through to the neutral muted-on-grey style. Reviewers the board always asks and reviewers it never asks get the same grey pill. Only the word differs, and the 'Pinned' emphasis was clearly intended.
- **Principle:** One look per role, and different things look different [SEC-4] [NNG-H4]
- **Fix:** Use an existing tone: kit::tone_pill(t, "accent", "Pinned"), or add a "review" arm to kit::tone. Log or test for unknown tone names so a typo can't silently go neutral.

### R67 · 2 Minor · Slot and reviewer lists aren't virtualized
- **Unit:** QA Reviewers Worktrees
- **Element:** Reviewers and Worktrees › scrolling lists (reviewers_page, worktrees_page)
- **Evidence:** Confirmed
- **Problem:** Both pages build every project card and every row on each render inside a plain overflow_y_scroll div. Pools default to as many as 10 slots per project, the limit can go up to 50, and Worktrees re-renders every 30 seconds, so the page does work for rows that are off-screen.
- **Principle:** Brief rule: long lists virtualized
- **Fix:** Flatten each page into one row list (project heading, pool row, slot rows, waiting row) and render it with GPUI's list or uniform_list, keeping each heading above its rows [L1]. Draw the card edges per row using first and last flags.

### R68 · 2 Minor · Which task holds or awaits a slot is hidden or reduced to bare refs
- **Unit:** QA Reviewers Worktrees
- **Element:** Worktrees › slot row pill and 'N waiting' row (worktrees_page, pools_vm)
- **Evidence:** Confirmed
- **Problem:** The holder's title appears only in a hover tooltip on a 20px pill, so scanning a pool means hovering each pill, and the keyboard can't reach it at all. The waiting row is a comma-joined list of refs with no titles, so the owner must remember what T231 is.
- **Principle:** Hover is never the only path to information [NNG-FITTS]; recognition over recall [NNG-H6]
- **Fix:** Put the title in the row itself: the note becomes 'T12 Fix login redirect · 7.7 GB · last used 10:08 AM' with kit::clipped truncation, and the pill says 'In use' or 'Free'. List waiting tasks one per row, as ref plus title, under the 'N waiting' label [L1]. This needs the daemon's slots response to carry the waiting tasks' titles.

### R69 · 2 Minor · Content stretches the full window width
- **Unit:** QA Reviewers Worktrees
- **Element:** QA, Reviewers and Worktrees content column at 1400x1000 (qa--light-l.png)
- **Evidence:** Confirmed
- **Problem:** Cards fill the window, so at 1400 the 'Read QA comments' label and its Off/On control sit about 1,000px apart, and the eye has to cross a blank row to connect them. Empty messages centre in the wide pane, away from the left-aligned title and text.
- **Principle:** Desktop window range: nothing stretches past a readable length at the maximum (checklist pass 7); alignment [SCR-4]
- **Fix:** Cap the page header and list content at max_w(px(760.)), left-aligned at the existing 28px inset, across all sections. Left-align kit::empty inside it. No change is needed at 900 or 720.

### R70 · 2 Minor · 'Pool' and 'Comments' are filler row labels
- **Unit:** QA Reviewers Worktrees
- **Element:** Worktrees › Pool row; QA › Comments row (worktrees_page, qa_page)
- **Evidence:** Confirmed
- **Problem:** Each label only names the kind of sentence below it. 'Pool' sits inside a card already headed with the project and slot count, and 'Comments' sits inside a card about QA comments, so both add a line without adding meaning.
- **Principle:** Brief rule: no filler labels; everything earns its place [NNG-H8] [SCR-5]
- **Fix:** For Pool, R25's named rows replace the label. If those wait, promote the sentence to the row's main text and drop 'Pool'. For QA, make the value the row: '3 comments read, 1 waiting on you' as the title and 'Last checked 3:05 PM' as the note.

### R71 · 2 Minor · A ready update disappears when a newer release starts downloading
- **Unit:** System and Update
- **Element:** Settings › System › Update row and sidebar update pill, Ready state, code only (updater.rs L246-281, L189-190)
- **Evidence:** Confirmed
- **Problem:** While Ready, the 6-hourly check can find a newer release. It then sets Downloading, so the pill and Restart to update both vanish with no message. stage() deletes the staged folder first, so a failed download or check of the newer build leaves the owner with "Check failed" and no update, although a good one was ready. On a beta build with frequent releases this is a realistic path: the owner clicks the pill and finds nothing to press.
- **Principle:** Visibility of system status [NNG-H1]; user control [NNG-H3]
- **Fix:** Stage each version in its own folder (staged/{version}) and only swap STAGED and delete the old folder once the new one passes every check. Keep State::Ready visible during that background work, with the row note "Downloading {newer} in the background". If the newer build fails, stay on Ready and log it.

### R72 · 2 Minor · Restart note promises continuity the code doesn't match, and a failed daemon restart is silent
- **Unit:** System and Update
- **Element:** Settings › System › Update row note, Ready state, code only (settings.rs L22; updater.rs L14-15, L326-329)
- **Evidence:** Confirmed
- **Problem:** The note says "The board and its agents keep running through the restart", but apply() restarts the daemon, and the module docs say so. The owner decides when to press this based on whether agents will be hurt, so the sentence has to be exact. If that daemon restart fails, the error only goes to the log and the app quits anyway. The new app then runs against the old daemon, and the only sign is an amber row on a page the owner has no reason to open. Whether agents actually survive a daemon restart could not be checked from these excerpts.
- **Principle:** System status is honest [NNG-H1]; match what actually happens [NNG-H2]; error prevention [NNG-H5]
- **Fix:** Reword to what's true, for example: "Taskboard closes and reopens, and the daemon restarts on {version}. Agents keep working; the board reconnects in a few seconds." Keep the agents clause only if it's verified, and otherwise state what they lose. Show a warn pill in the status area when daemon version != app version after launch, reading "Daemon behind: restart" and opening System, so a failed restart is seen.

### R73 · 2 Minor · Restart to update can be pressed twice and may never show Installing
- **Unit:** System and Update
- **Element:** Settings › System › Restart to update, Ready state, code only (settings.rs L23, L83-96; updater.rs L325)
- **Evidence:** Likely
- **Problem:** apply_update has no guard. State only becomes Applying inside the background task, after the one cx.notify() has already been called, so the button can stay live. A second press fails with "No update is ready." and shows a red error row while the install is actually going ahead. The same timing means "Installing…" may not appear until something else redraws. Whether a timer elsewhere repaints the page isn't in the excerpt.
- **Principle:** Feedback on every action [NNG-H1]; error prevention [NNG-H5]
- **Fix:** Set the state synchronously before spawning, with a sys_applying flag like sys_busy, or make apply() split into a sync set(State::Applying) and the blocking work. Render the disabled "Installing…" button from that flag, and call cx.notify() again from the task when it finishes.

### R74 · 2 Minor · Downloading shows no progress, and verifying is hidden inside it
- **Unit:** System and Update
- **Element:** Settings › System › Update row, Downloading state, code only (settings.rs L18; updater.rs L143-173, L268-272)
- **Evidence:** Confirmed
- **Problem:** The row says "Downloading {version}…" with no amount, no bar and no cancel, even though the code knows both size and total. A stalled download looks the same as a working one. The unpacking and the codesign and spctl checks that follow also read as Downloading. The note "Checked before it's offered." is hard to parse and doesn't say what the owner should do, which is nothing.
- **Principle:** Slow actions show progress [NNG-H1] [mobile:NNG-RESPONSE]; plain words [NNG-H2]
- **Fix:** Add downloaded and total to State::Downloading and show "Downloading 1.2.0 · 14 of 38 MB", updated about once a second. Add State::Verifying { version } shown as "Checking 1.2.0…". Change the note to "Runs in the background. You'll be asked before anything restarts."

### R75 · 2 Minor · Background update failures never leave the Settings page
- **Unit:** System and Update
- **Element:** Sidebar update pill, all non-Ready states, code only (app.rs L29-30; updater.rs L254)
- **Evidence:** Confirmed
- **Problem:** update_pill returns None for everything except Ready, and the updater runs by itself every 6 hours. An owner who leaves the app open all day has no sign that updates have been failing, for example a refused signature or a read-only install folder. They just stay on an old build until they happen to open System.
- **Principle:** Visibility of system status [NNG-H1]
- **Fix:** Keep offline blips quiet, but after two failed checks in a row, or on any Verify or Install failure, show a warn-toned pill reading "Update failed" that opens Settings › System. Use the same pill_shell and tone as accounts_pill, with the reason in the tooltip.

### R76 · 2 Minor · Restart daemon gives no success message and doesn't say what it affects
- **Unit:** System and Update
- **Element:** Settings › System › Daemon version › Restart daemon, installed build, code only (settings.rs L53-61, L99-118)
- **Evidence:** Confirmed
- **Problem:** On success, sys_note stays empty even though a green style exists for it (L70), so the only feedback is the note changing to "Up under a minute" and a new pid. After a fixed 2 second wait the page reads the daemon once, so a slower launchd leaves "not running" with nothing to say it's still starting. The button fires on one click with no word on what happens to agents, on a screen the owner uses while many agents are running.
- **Principle:** Feedback after an action [NNG-H1]; destructive actions are worded clearly or confirmed [NNG-H5]
- **Fix:** After success set sys_note to ("Daemon restarted on {version}.", false) and clear it after about 5 seconds. Poll load_info every 500ms for up to 15 seconds instead of one read at 2 seconds, keeping "Restarting…" until it answers, then show "The daemon didn't come back" in red. Add a tooltip to the button stating the effect on running agents. If agents are interrupted, ask first in a confirm dialog.

### R77 · 2 Minor · The common Up to date row likely overflows the card at the minimum window
- **Unit:** System and Update
- **Element:** Settings › System › Update row, Up to date and Ready states, 720x460, code only, widths estimated (settings.rs L17, L28-33, L1589-1605 excerpt; layout from system--light-s.png)
- **Evidence:** Likely
- **Problem:** At 720 wide the card is about 436px, so about 404px sits inside its padding. The label column has min_w 150 and the controls are flex_none with no wrap. "Up to date · checked at 12:05 PM" plus Check now comes to about 277px. With the 16px gap that needs about 443px, so Check now runs roughly 20px past the card edge, where overflow_hidden cuts it off. Ready is about 2px over. The capture shows only "Off", which is the shortest state, so the problem isn't visible there. A long failure message also gets only a 150px column, which makes a very tall row.
- **Principle:** The minimum window works and no control is lost [Pass 7, desktop window range] [HIG-MACOS]
- **Fix:** Shorten the status to "Up to date · 12:05 PM" and let row wrap: add flex_wrap to the row and justify_end on controls, so controls drop to a second line under about 480px of row width. Put a failure note on its own full-width line under the title and controls.

### R78 · 2 Minor · The update pill's wording is vague and its text colour departs from the pill tones
- **Unit:** System and Update
- **Element:** Sidebar update pill, Ready state, code only (app.rs L32-35; kit.rs L111-114)
- **Evidence:** Likely
- **Problem:** "Taskboard 1.2.0 is ready" doesn't say an update is waiting; that only comes from the tooltip, and tooltips are pointer-only. The pill draws t.accent text on t.accent_soft, whereas tone() pairs accent_soft with t.accent_fg and accounts_pill uses warn_fg. At 12.5px this is the pairing most likely to fall under 4.5:1. It wasn't measured, because no capture shows this pill and the token values aren't in the excerpts.
- **Principle:** Text contrast [WCAG 1.4.3]; label says what it does [NNG-H2]; consistency [NNG-H4]
- **Fix:** Use tone(t, "accent") for the colours, keeping t.accent for the dot only, and label it "Update ready: 1.2.0". Measure the pair in light and dark and confirm at least 4.5:1.

### R79 · 2 Minor · An invalid time snaps back with no message
- **Unit:** Hours menu
- **Element:** Hours menu › From, to and Today until fields; on Enter, on blur, on chip click and on menu close
- **Evidence:** Confirmed
- **Problem:** commit() puts the old value back for anything that doesn't parse (hours.rs 315-327). Typing 73 shows 7:3, and typing 1375 shows 13:75; both vanish on Enter or on leaving the field with no explanation. The owner can miss that the time never changed, especially when closing the menu straight after.
- **Principle:** Errors name the field, say what's wrong and how to fix it, next to the field [WCAG 3.3.1] [WCAG 3.3.3] [NNG-H9]
- **Fix:** When the text doesn't parse and focus stays, keep it, set the field border to t.down and show 'Enter a time like 7:30' in the existing err row. Only put the old value back on Esc. If the menu is closing, keep the revert, but mention it the next time the menu opens, or commit 7:3 as 7:30, which is the likely intent.

### R80 · 2 Minor · All seven days can be turned off with no warning
- **Unit:** Hours menu
- **Element:** Hours menu › day buttons; footer › pill
- **Evidence:** Confirmed
- **Problem:** toggle_day has no guard, so the last selected day can be removed and saved (hours.rs 96-102, 508-514). With the checkbox on, agents then may never start, with nothing said in the menu. Without a next_open the pill label trims to a dangling 'Agents off until' (hours.rs 125-126). The code path is confirmed; whether the board sends no next_open for zero days isn't in the excerpts.
- **Principle:** Constraints instead of error messages [NNG-H5]; visibility of system status [NNG-H1]
- **Fix:** When no days are selected, show 'No days picked: agents won't start' in the menu, and make the pill read 'Agents off: no days picked'. In pill_view, use the label 'Agents off' whenever next_open is missing, so it never ends on 'until'.

### R81 · 2 Minor · 'No work hours' reads as the opposite of what it means
- **Unit:** Hours menu
- **Element:** Footer › pill, off state; hours-menu-light-m.png
- **Evidence:** Confirmed
- **Problem:** 'No work hours' can be read as agents having no hours to work, but it means agents start any time. Only the tooltip says so, and a glance at the footer is how this pill gets used all day. The green dot helps but is the only visible hint.
- **Principle:** Match the user's words and the real world [NNG-H2]; hover is never the only path to meaning [NNG-FITTS]
- **Fix:** Change the label to 'Agents start any time' in pill_view (hours.rs 117), which is the tooltip's own wording, and drop the now redundant tooltip text.

### R82 · 2 Minor · Day selection is shown by a nearly invisible fill and looks like a single-choice control
- **Unit:** Hours menu
- **Element:** Hours menu › day buttons; hours-on-light-m.png, hours-on-dark-m.png, hours-on-dark-s.png
- **Evidence:** Confirmed
- **Problem:** Selected and unselected fills measure #ffffff against #e9ecf0 in light (about 1.2:1) and #1a1f26 against #232931 in dark (about 1.1:1). In dark the selected day is the darker one, so it looks pressed away, and the text difference is about 1.9:1 (#ffffff against #b3bdcb). The control copies the segmented look, which usually means pick one, but this is a multi-select.
- **Principle:** State indicators reach 3:1 and colour isn't the only signal [WCAG 1.4.11] [WCAG 1.4.1]; looks like what it is [NNG-H4]
- **Fix:** Drop the track background. Draw selected days with a t.accent_btn fill and t.on_accent text, the same pair as the checked checkbox, which measures 5.1:1 in light and 4.2:1 in dark. Draw unselected days with no fill, muted text and a 1px border at 3:1. Keep the seven buttons: the set is fixed, so lesson L2 doesn't apply.

### R83 · 2 Minor · Checkbox edge, field edges and the light placeholder are under contrast minimums
- **Unit:** Hours menu
- **Element:** Hours menu › unchecked checkbox (hours-menu-light-m.png), time fields, Usual end placeholder (hours-on-light-m.png)
- **Evidence:** Likely
- **Problem:** Sampled from the captures: the unchecked box border is #cfd5dd on white at 1.48:1, the field border is 1.46:1 in light and 1.63:1 in dark, and the placeholder is #768393 on white at 3.86:1 (dark passes at 4.72:1). The unchecked box is the master switch, and its outline is all that shows it. The numbers come from scaled, antialiased pixels because the theme tokens weren't in the excerpts, so the text figure may read slightly low.
- **Principle:** Control boundaries reach 3:1 [WCAG 1.4.11]; text reaches 4.5:1 [WCAG 1.4.3]
- **Fix:** Use a border of #8792a2 in light (3.2:1 on white) and #6b7685 in dark (3.6:1 on #1a1f26) for the checkbox and time fields. Set the placeholder to t.muted, which samples near #4b5869, about 7:1 on white.

### R84 · 2 Minor · Times break the 12-hour AM/PM rule and differ from the rest of the app
- **Unit:** Hours menu
- **Element:** Footer › pill label; Hours menu › am/pm chips; fmt.rs clock12
- **Evidence:** Confirmed
- **Problem:** The chips say am and pm, and the pill says 9am, 5:30pm (fmt.rs 178-184, hours.rs 359). The same app writes 3:05 PM in hhmm and log_time (fmt.rs 86, 94). The owner's rule is 12-hour times with AM/PM, so one clock has two spellings.
- **Principle:** Same thing, same name and look everywhere [NNG-H4]; the brief's rule: 12-hour times with AM/PM
- **Fix:** Make clock12 return '9 AM' and '9:30 AM', and make the chips read AM and PM. Check other clock12 and day_clock callers, and do this together with R85 because the labels get longer.

### R85 · 2 Minor · The longest pill labels probably run past the sidebar
- **Unit:** Hours menu
- **Element:** Footer › pill, 'Work hours until tomorrow 10:30pm'; any window size
- **Evidence:** Likely
- **Problem:** The pill is nowrap with no max width (hours.rs 234). The sidebar is about 247px wide at both captured sizes. Scaling from the captured 24-character label (about 194px) gives about 246px for this 33-character label, starting 12px in, so it ends around 258px, past the sidebar edge. No capture shows this state, and the parent's clipping isn't in the excerpts.
- **Principle:** Text isn't cut off or lost [WCAG 1.4.4]; every control fits at every window size [NNG-H6]
- **Fix:** Shorten the labels to 'Until 7:30 PM today' and 'Until tomorrow 10:30 PM', and keep the full sentence in the tooltip. Add max_w to the sidebar width minus its padding, and truncate the label with kit::clipped as a backstop.

### R86 · 2 Minor · The menu opens wherever the pointer is, and the tooltip stays on top of the footer
- **Unit:** Hours menu
- **Element:** Footer › pill and Hours menu position; hours-menu-light-m.png, hours-on-light-m.png
- **Evidence:** Confirmed
- **Problem:** The menu is placed from the click position (hours.rs 245-246), so it lands in a different place depending on where the pill was clicked. In the captures it starts about 53px right of the pill and cuts the pill above to 'Midn'. In hours-menu-light-m.png the pill's tooltip also stays up with the menu open, covering the Sample board and Settings row.
- **Principle:** A popover is anchored to its opener (kit::popover's own comment); consistency [NNG-H4]; hover content doesn't get in the way [WCAG 1.4.13]
- **Fix:** Anchor the menu to the pill's bounds: left edges aligned, bottom 6px above the pill's top, whatever the click point. Attach the tooltip only when the menu is closed, with .when(!open, …).

### R87 · 2 Minor · The one-day override reads as part of the weekly schedule
- **Unit:** Hours menu
- **Element:** Hours menu › row layout; hours-on-light-m.png
- **Evidence:** Confirmed
- **Problem:** Today until only lasts today, but it sits at the same 10px gap as the weekly From, to and days, so the menu reads as one list. The day track also breaks the shared edge: rows are inset 4px and the track isn't (hours.rs 460, 490), so it starts at 87 against 91 and ends at 384 against 379 in the capture.
- **Principle:** Grouped by space [SEC-2]; alignment [SCR-4]; lesson L1 on showing what belongs to a group
- **Fix:** Add a 1px t.border divider with 10px above and below Today until, the treatment the keep-awake block already uses. Add .mx(px(4.)) to the day track. If the override is used more than the schedule, keep it last, since that is the row closest to the pill.

### R88 · 2 Minor · The am/pm chip looks like a static suffix, and the menu's controls have no hover or pressed state
- **Unit:** Hours menu
- **Element:** Hours menu › am/pm chips, day buttons, checkbox row
- **Evidence:** Confirmed
- **Problem:** The chip is a grey label that gives no sign it flips the half, and only a pointer cursor appears on hover. The chip, day buttons and checkbox have no .hover or .active style (hours.rs 347-365, 461-468, 493-507), unlike kit::btn and kit::seg. Typing a or p also sets the half, but nothing tells you that.
- **Principle:** Each state is designed, and pressing responds visibly [NNG-H1]; looks like what it is [NNG-H6]
- **Fix:** Add hover and active styles to all three: the chip goes to t.border_2 background on hover, days to t.panel_2. Give the chip the tooltip 'Switch to PM (or type p)'.

### R89 · 2 Minor · Controls expose no role, name or state to assistive tech
- **Unit:** Hours menu
- **Element:** Hours menu › checkbox, days, chips, fields; footer › pill
- **Evidence:** Likely
- **Problem:** The checkbox state is a drawn ✓ in a div, the days are a letter in a div with T and S repeated, and nothing in the excerpts sets a role, label or value. Save errors also appear with no announcement. Rated 2 because the only user is a sighted owner, and because this may be a limit of the whole framework, not of this menu. VoiceOver wasn't run.
- **Principle:** Name, role and value are exposed [WCAG 4.1.2]; status messages are announced [WCAG 4.1.3]
- **Fix:** If gpui_kit has an accessibility API, give the checkbox the checkbox role with its state, days the names Monday to Sunday with pressed state, and chips the name 'From: AM'. If it has none, record that as a known app-wide limit with the owner rather than patching this menu.

### R90 · 2 Minor · Warm and hot barely differ and never rise above the footer
- **Unit:** Usage pill and footer
- **Element:** Main window › sidebar footer › usage pill, warm and hot states (pill-warm-light-m.png, pill-hot-light-m.png)
- **Evidence:** Confirmed
- **Problem:** The whole escalation is a recoloured 5px bar and a 12.5px number on the same grey pill. Warm and hot are close in hue: the bars are 1.27:1 apart in light and 1.35:1 in dark, and the values 1.11:1 in light, so only the semibold weight and the digits separate them. In pill-hot-light-m.png the filled '2 accounts need you' pill below is louder than 96% used. For an app left open all day, the state most likely to stop agents is easy to miss from the corner of the eye.
- **Principle:** Visibility of system status [NNG-H1]; one emphasis, with the strongest signal on what matters most [SEC-5] [NNG-VH]
- **Fix:** Tint the whole pill by its worst level: hot uses background t.down_soft with border t.down_line, and warm uses t.warn_soft with t.warn_line. Contrast holds: t.down on down_soft is 5.60:1 light and 5.46:1 dark; t.warn_fg on warn_soft is 6.37:1 and 8.50:1; t.muted labels are at least 4.62:1 on either. Put the soonest reset in the hot state, for example 'Resets 3 PM' on the stale-style second line.

### R91 · 2 Minor · The footer mixes looks and has no fixed places
- **Unit:** Usage pill and footer
- **Element:** Main window › sidebar footer, all states (pill-normal-light-m.png, pill-warm-light-m.png, pill-hot-light-m.png, down-main-light-m.png)
- **Evidence:** Confirmed
- **Problem:** Footer items flow in one flex_wrap, so position depends on what is showing. Settings sits at the far left in the normal state (x≈18 in the capture), moves to x≈222 beside the GitHub pill and x≈219 beside '2 accounts need you', so a fixed command drifts. Midna sits alone on a row with empty space beside it in these captures. Looks don't match role: Midna (status, no click), accounts (click) and update (click) share one filled pill shape, and only some have a hover style. Hooks is plain text with a dot, the Sample pill is 20px tall against 25-27px, and dots are 7px or 8px.
- **Principle:** One look per role [SEC-4]; grouped by function, with one job per section [SEC-1] [SEC-2]; alignment [SCR-4]; consistency [NNG-H4]
- **Fix:** Use three fixed rows. Row 1: Midna, work hours, hooks, then Settings pinned right with ml_auto. Row 2: the usage pill at full width (R3). Row 3, only when something needs attention: accounts, update, PR feed, Sample board. That is one row shorter in the normal state, which helps at 900x560. Give every clickable pill the same hover style, keep passive status without one, and use one dot size (7px) and one pill height.

### R92 · 2 Minor · The pill and its tooltip can't be reached from the keyboard
- **Unit:** Usage pill and footer
- **Element:** Main window › sidebar footer › usage pill and tooltip
- **Evidence:** Confirmed
- **Problem:** In the excerpts the pill is a div with on_click and a hover tooltip: no focus handle, tab stop or key handler. So the click can't be triggered from the keyboard, and reset times, the daily line and 'Last read' can't be reached without a pointer. The same goes for the accounts and update pills. This is rated 2 rather than 3 because the single owner has a pointer; the excerpts don't show whether a key binding exists elsewhere.
- **Principle:** Keyboard [WCAG 2.1.1]; content on hover or focus [WCAG 1.4.13]; name, role, value [WCAG 4.1.2]
- **Fix:** Give each clickable footer item a focus handle and tab stop, activate on Enter and Space, and draw a 2px t.accent focus ring. Show the same tooltip on focus and dismiss it with Esc. The readings then also need to exist in the Settings pane R33 opens, so they aren't locked in a tooltip.

### R93 · 2 Minor · Today counts down while the other windows count up
- **Unit:** Usage pill and footer
- **Element:** Main window › sidebar footer › usage pill › Today segment (code; hidden today by R3)
- **Evidence:** Confirmed
- **Problem:** usage_today draws the bar as the share used but prints what's left, so a nearly full bar reads '12% left' next to '96%', which means used. Higher means worse in two segments and better in the third, and a glance at the row gives the wrong picture. js_num also prints the raw number, so a fractional 'left' would widen the segment. This will surface as soon as R3 is fixed.
- **Principle:** Consistency and standards [NNG-H4]; match with the real world [NNG-H2]
- **Fix:** Print used share rounded to a whole percent, 'Today 88%', matching the bar and the other windows. Keep the 'left' wording in the tooltip line ('12% of today left').

### R94 · 2 Minor · Tooltip reset times break the AM/PM rule
- **Unit:** Usage pill and footer
- **Element:** Main window › sidebar footer › usage pill › tooltip, 'resets …'
- **Evidence:** Confirmed
- **Problem:** usage_view builds reset times with day_clock, which uses clock12 and gives '3pm' or 'Thu 6am'. The brief's rule is 12-hour times with AM/PM, and hhmm elsewhere in the same file prints '3:05 PM', so two styles coexist. Reset time is what the owner plans around when usage is hot.
- **Principle:** 12-hour times with AM/PM (brief rule); consistency [NNG-H4]
- **Fix:** Change clock12 to return '3 PM' and '9:30 AM' (uppercase, with a space). That also updates day_clock ('Thu 6 AM') and day_time. Update the parity goldens if they pin the old text.

### R95 · 2 Minor · The stopped-board screen gives no cause, and reports 'No agents waiting'
- **Unit:** First launch
- **Element:** Main window › Needs you, board stopped (down-main-light-m.png)
- **Evidence:** Confirmed
- **Problem:** The header says 'No agents waiting' while the body says 'Can't load the board.' The app doesn't know that, so the good news is false, and it sits right where the owner looks. The red banner says only that the server can't be reached and that it is retrying. It doesn't say why or point to Settings › System, where Restart daemon lives. Its only button is Copy.
- **Principle:** Commands and status never report what they don't know [NNG-H1]; say what to do next [NNG-H9].
- **Fix:** When m.down is set and there is no home data, drop the waiting line. Replace the empty text with 'The background service isn't answering.' and an 'Open Setup' button that opens the R36 card. Word the banner by LoginItem state, for example 'Waiting for you to switch on Login Items' when RequiresApproval, and drop the red bar in that state so the amber cause leads.

### R96 · 2 Minor · One service goes by four names
- **Unit:** First launch
- **Element:** Banner, System page and tooltips (down-main-light-m.png, system--light-m.png, app.rs, Sec::System)
- **Evidence:** Confirmed
- **Problem:** The same thing is called 'task board server', 'background service', 'login item' and 'daemon', and 'Daemon version' and 'Restart daemon' appear in Settings. Nothing tells the owner these are one thing, so linking the red banner to the amber banner and the System row takes guesswork. The app is also 'Task board' in menus and 'Taskboard' in banners.
- **Principle:** Same thing, same name everywhere [NNG-H4].
- **Fix:** Use 'background service' in all UI text: 'Can't reach the background service.', 'Background service version', 'Restart background service'. Keep 'login item' only where naming the macOS setting. Pick one app name, 'Taskboard', for the menus and window title.

### R97 · 2 Minor · A failed tb link is silent
- **Unit:** First launch
- **Element:** First launch › install::start › link_tb; no UI
- **Evidence:** Confirmed
- **Problem:** If linking ~/.local/bin/tb fails, the error goes only to the log, so nothing tells the owner. The plugin's tb shim looks for tb at that path, so agents' tb commands can then fail with no visible cause in the app. A foreign file at that path also returns Ok; it surfaces only as the PATH banner, whose 'put ~/.local/bin first' advice can't help when the foreign file is in that folder. The check uses a widened PATH, so it says nothing about the owner's real shell.
- **Principle:** Visibility of system status [NNG-H1]; errors are reported with a way to recover [NNG-H9].
- **Fix:** Store the link result next to STATUS and show it in the R36 'tb command' row: 'Linked at ~/.local/bin/tb', or 'Couldn't link tb: <reason>' with Try again and Copy. For a foreign file, say 'Something else is at ~/.local/bin/tb. Rename it, then try again.'

### R98 · 2 Minor · Reinstall removes the working plugin first and doesn't say what it changes
- **Unit:** First launch
- **Element:** Main window › footer › Reinstall hooks (hooks::install)
- **Evidence:** Confirmed
- **Problem:** One click runs 'claude plugin marketplace remove', which also uninstalls the plugin, and only then adds it back. If the add or install fails, the owner has lost a plugin that was working and sees only the 6-second toast. It also rewrites ~/.claude/settings.json with no notice, and the .before-taskboard backup is never mentioned. Separately and likely: claude_bin finds the file, but the child process keeps Finder's bare PATH, so a claude script that needs node from Homebrew may fail.
- **Principle:** Error prevention [NNG-H5]; user control and freedom [NNG-H3].
- **Fix:** Check first by running 'claude --version', and run the remove only once that passes. If add or install then fails, restore the previous marketplace path. In the Setup row, state before the click: 'Adds Taskboard's hooks to ~/.claude/settings.json and installs its plugin. Your other settings stay; a backup is kept as settings.json.before-taskboard.' Run claude with agent_path() as PATH.

### R99 · 2 Minor · The accounts pill can land on the wrong Settings page
- **Unit:** First launch
- **Element:** Main window › footer › accounts-pill → Settings
- **Evidence:** Confirmed
- **Problem:** The pill dispatches OpenSettings. If Settings is already open, settings::open only brings the window forward and leaves self.view alone, so an owner left on System or Usage sees no account problem. The update pill avoids this with show_system.
- **Principle:** Users see the result they expected from the action [NNG-CW]; consistency [NNG-H4].
- **Fix:** Add an OpenAccountsSettings action with show_accounts, mirroring OpenSystemSettings and show_system, and scroll to the first account where needs_owner is true.

### R100 · 2 Minor · A refused sign-in still shows a green dot, and may have no re-sign-in button
- **Unit:** First launch
- **Element:** Settings › Accounts › GitHub card (acct-warm-light-m.png)
- **Evidence:** Confirmed
- **Problem:** push_row always draws a green dot and 'Agents on a task git push… with this account', even when the sign-in was refused and pushes would fail. Green sits directly under the warning. Also, the GitHub 'Sign in again' button appears only when reauth is true. An account with just an error gets the pill 'check sign-in', and the card offers only Check and Disconnect, so the owner has to disconnect to recover.
- **Principle:** Status must be true [NNG-H1]; recover without a detour [NNG-H9]; colour isn't the only signal [WCAG 1.4.1].
- **Fix:** Pass the account state to push_row: on an error or reauth, use the amber dot and 'Agents can't push to github.com until you sign in again.' In github_rows, show Sign in again when a["error"] is a string as well as when reauth is true.

### R101 · 2 Minor · Accounts gives every service equal weight, with no sign of which ones matter
- **Unit:** First launch
- **Element:** Settings › Accounts page, not signed in, 900x680 (accounts--light-m.png)
- **Evidence:** Confirmed
- **Problem:** Blurred, three solid blue buttons compete, and the page opens with every form expanded. Slack is below the fold at the default size, marked only by a sliver of its icon. Nothing says the owner needs only the services their projects use, or what is lost if one is skipped, and 'Sign in once' implies all of them. GitHub offers three routes side by side. Smaller noise: the 'Personal access token' placeholder repeats its label, and the raw URL repeats the link beside it, cut off in the Azure row. The Connect button sits in the token row, though it submits the email or organization row too.
- **Principle:** One emphasis per surface [SEC-5]; frequent in view, rare behind [SEC-8]; every element earns its place [SCR-5]; the brief's rule: no filler labels.
- **Fix:** Collapse each unconnected service to one row: logo, what it gives, and a quiet 'Connect' button that opens its form. Keep one primary at a time: GitHub's sign-in until a host is connected. Change the page note to 'Connect the services your projects use.' Replace the placeholder with an example or remove it, and delete the URL text beside the link. Keep the form's rows together under the service heading, per L1.

### R102 · 1 Cosmetic · Sidebar glyphs are mixed font characters with uneven weight and size
- **Unit:** Settings shell
- **Element:** Settings › sidebar icons, all sizes and themes (accounts--light-m.png)
- **Evidence:** Confirmed
- **Problem:** The icons are Unicode characters from fallback fonts: the ✓ is heavy, the ⑂ is tiny and faint, and ◷ History and ◔ Usage are almost the same shape. Labels carry the meaning, so nothing is lost, but the column looks uneven.
- **Principle:** One look per role [SEC-4]
- **Fix:** Draw seven 16px stroked icons with the kit's existing canvas `Icon` set, at one stroke width. Give History a clock and Usage a gauge so they differ.

### R103 · 1 Cosmetic · Buttons and sidebar rows have no pressed state
- **Unit:** Settings shell
- **Element:** Settings › kit::btn, btn_primary, sidebar rows
- **Evidence:** Confirmed
- **Problem:** The kit defines hover but no `.active` style, so a click gives no immediate response until the state changes. Slow calls do show 'Checking…' or 'Starting…', which covers the important cases.
- **Principle:** Feedback; states are designed [NNG-H1]
- **Fix:** Add `.active(|s| s.bg(t.seg))` to kit::btn and sidebar rows, and `.active(|s| s.opacity(0.8))` to btn_primary.

### R104 · 1 Cosmetic · Small inconsistencies in the cards
- **Unit:** Accounts
- **Element:** Accounts › GitHub Token row and button heights (accounts-use-token-light-m.png; kit.rs, settings.rs)
- **Evidence:** Confirmed
- **Problem:** The GitHub Token row has no divider above it because first is passed as true, unlike every other multi-row card. Buttons in one card are 24, 26 and 32px tall. The Azure placeholder 'Personal access token' only repeats the label above it, which breaks the no-filler rule.
- **Principle:** One look per role [SEC-4]; alignment [SCR-4]; no filler labels (brief rule)
- **Fix:** Pass first as false for the GitHub token row. Use one height for buttons in a row, 26px beside small buttons and 32px beside fields. Replace the placeholder with a format hint, or leave it empty.

### R105 · 1 Cosmetic · History never says how far back it actually goes
- **Unit:** History and Usage
- **Element:** Settings › History › Space used row (history--light-m.png)
- **Evidence:** Confirmed
- **Problem:** The page intro promises how far back the Days page goes, but the row shows counts and sizes only, and the `oldest` date the daemon sends goes unused. The count text also reads wrongly at one ('1 events'), and that count includes status changes and terminal lines.
- **Principle:** Visibility of status [NNG-H1]; plain language [NNG-H2].
- **Fix:** Rename the row 'Stored' and write: 'Back to Aug 12, 2026: 247 events (16 KB), 10 day summaries (27 KB).' Handle the singular.

### R106 · 1 Cosmetic · History and Usage have near-identical sidebar glyphs
- **Unit:** History and Usage
- **Element:** Settings › sidebar › History ◷ and Usage ◔, adjacent rows (history--light-s.png)
- **Evidence:** Confirmed
- **Problem:** Both are small circles with a wedge or hand, directly above one another, so the icons don't help tell the two sections apart.
- **Principle:** One look per role, distinct where meaning differs [NNG-H4].
- **Fix:** Keep ◷ for History and use a gauge or bar glyph for Usage, or draw both with `kit::icon` for a matched set.

### R107 · 1 Cosmetic · CLI names read as ordinary words and wrap mid-command
- **Unit:** QA Reviewers Worktrees
- **Element:** Reviewers and Worktrees header lines (worktrees--light-s.png, reviewers--light-m.png)
- **Evidence:** Confirmed
- **Problem:** 'tb project set sizes a project's pool' and 'Agents and tb reviewers change the roster' have no code styling, so they take a second read. At 720 the Worktrees line breaks right after the command.
- **Principle:** Match with the user's words [NNG-H2]
- **Fix:** R25 removes these lines. If they stay, set the command in kit::mono and keep it on one line.

### R108 · 1 Cosmetic · Tinted row pokes out past the card's rounded corners
- **Unit:** QA Reviewers Worktrees
- **Element:** QA › Read QA comments row, most visible in dark (qa--dark-m.png, qa--dark-s.png, qa--light-s.png)
- **Evidence:** Confirmed
- **Problem:** The card's overflow_hidden doesn't clip the row's fill to the 10px radius, so square brown corners show outside the rounded border. Worktrees' amber waiting row is the last row in its card, so it will likely show the same at the bottom.
- **Principle:** Polish [NNG-H8]
- **Fix:** Give tinted rows matching corners: .rounded(px(10.)) on a single-row card, and rounded_b on a last row.

### R109 · 1 Cosmetic · App version's green dot and the page subtitle carry no information
- **Unit:** System and Update
- **Element:** Settings › System › App version row and page header, all sizes (system--light-m.png, down-system-m.png; settings.rs L45)
- **Evidence:** Confirmed
- **Problem:** The App version dot is always t.up, so it can't change, yet it sits in the same column as a real status dot. It stays green while an update waits or the board is down. The subtitle "Updates, and the versions of the app and the daemon." only lists the two cards below it, which goes against the no-filler rule.
- **Principle:** Every element earns its place [SCR-5] [NNG-H8]
- **Fix:** Remove the App version dot and right-align the plain version. Remove the subtitle, or replace it with something the page can't otherwise show.

### R110 · 1 Cosmetic · "Checking…" appears twice, and disabled buttons still react to hover
- **Unit:** System and Update
- **Element:** Settings › System › Update row, Checking state, and any kit::disabled button, code only (settings.rs L16; kit.rs L153-156, L181-183)
- **Evidence:** Confirmed
- **Problem:** While checking, the status and the disabled button both say "Checking…" side by side. kit::disabled only sets opacity and cursor, so the hover background from btn stays and a disabled button still responds to the pointer.
- **Principle:** One fact, one place [SEC-7]; disabled controls look disabled [Pass 3]
- **Fix:** Keep the status text and leave the button out of the Checking state, or keep a disabled "Check now" so the row doesn't shift. Have kit::disabled clear the hover style.

### R111 · 1 Cosmetic · Rows stretch across a wide window
- **Unit:** System and Update
- **Element:** Settings › System › both cards, 1400x1000 (system--light-l.png; settings.rs L4)
- **Evidence:** Confirmed
- **Problem:** The list has no max width, so at 1400 wide each label and its value or button sit about 1000px apart, and the eye has to track across empty space to link them.
- **Principle:** Content has a max width [tablet:AND-T2]; desktop window range [Pass 7]
- **Fix:** Add max_w(px(720.)) to the system-rows content, left-aligned with the page title. Apply it across Settings pages if they share the same stretch.

### R112 · 1 Cosmetic · "Checked at" has no date
- **Unit:** System and Update
- **Element:** Settings › System › Update row, Up to date and Failed states, code only (updater.rs L69-71)
- **Evidence:** Likely
- **Problem:** The time has no day, so after a laptop sleep "checked at 3:05 PM" may be from yesterday and read as today. The 12-hour format itself is correct.
- **Principle:** Status is accurate [NNG-H1]
- **Fix:** Store a timestamp and format it when drawing: "3:05 PM" for today, "Yesterday, 3:05 PM" or "Nov 4, 3:05 PM" for older checks.

### R113 · 1 Cosmetic · Keep-awake status may have two homes
- **Unit:** Hours menu
- **Element:** Hours menu › keep-awake block; footer › Midna pill
- **Evidence:** Likely
- **Problem:** The menu appends a read-only Midna keep-awake line and schedule (hours.rs 437-449, 538), right next to a footer Midna pill. If that pill shows the same status, this breaks one home per object. The block appears in no capture and the Midna pill's code isn't in the excerpts.
- **Principle:** One fact, one place [SEC-7]; the brief's rule: one home per object
- **Fix:** If the Midna pill already carries this status, remove the block from the Hours menu; if it doesn't, move the block into the Midna pill's tooltip or menu.

### R114 · 1 Cosmetic · Settings shifts 6px when the board is down
- **Unit:** Usage pill and footer
- **Element:** Main window › sidebar footer › Settings (down-main-light-m.png against pill-normal-light-m.png)
- **Evidence:** Likely
- **Problem:** Settings starts at x≈25 in the capture with the board stopped and x≈18 otherwise, about 6 CSS px. That matches the footer's 6px gap, which suggests an empty hours pill still takes a slot before it. Separately, with the board down the pill vanishes and no last reading stays; the banner explains why, so this is acceptable. Note that usage_view also returns None, with no message, when the board is up but has no reading.
- **Principle:** Alignment [SCR-4]; visibility of system status [NNG-H1]
- **Fix:** Have hours::pill return an Option and skip it when empty; R91's fixed rows also remove the shift. For a running board with no reading, show a muted 'No usage reading' pill with the reason in its tooltip.

### R115 · 1 Cosmetic · The App version row shows a meaningless green dot
- **Unit:** First launch
- **Element:** Settings › System › App version (system--light-m.png)
- **Evidence:** Confirmed
- **Problem:** The dot is hard-coded to t.up, so it can never change. It looks like a health signal and weakens the Daemon version dot beside it, which does change.
- **Principle:** Every element earns its place [NNG-H8].
- **Fix:** Remove the dot from the App version row, and keep it on the background service row only.

## What works

- Settings shell: The live shell is calm and organised: a quiet sidebar, one clear title per page, and cards that hold their structure from 900 to 1400 wide without stretching awkwardly.
- Settings shell: Opening Settings twice brings the existing window forward rather than making a second one, and ⌘, and ⌘W behave as Mac users expect.
- Settings shell: Group labels sit above their cards (GitHub, Bitbucket, and so on), matching the owner's lesson L1, and the sidebar is a short fixed list, so it doesn't run into lesson L2.
- Settings shell: Field drafts live on the window, so switching sections doesn't lose typed text, and the Mac-style drag areas and zoom on double click are handled.
- Settings shell: The 12-hour rule holds ('after 3 AM'), and the Sample pill makes the sample board clear.
- Accounts: The GitHub code panel flow, as read in code, is well thought out: the code is shown and copied before the browser opens, it says what to press, and it says what happens next, including the 15-minute limit.
- Accounts: Each card heading sits above its members with the service mark and a one-line purpose, which follows lesson L1 and makes the cards easy to scan.
- Accounts: 'Use my gh sign-in' with a primary browser sign-in is a good set of routes for a developer, with the primary one clearly marked.
- Accounts: Dark mode holds up in the captures: no lost borders or hard-coded light surfaces, and error and warning rows stay readable by eye.
- Accounts: Token fields are secret and never copy out, and a failed connect keeps the typed text.
- History and Usage: The page structure is clear: name, one-line purpose and control per row, one card, and a quiet sidebar that keeps the page itself in charge.
- History and Usage: The Limit rows appear only when the limit is on, so the Off state is one simple row (usage--light-m.png).
- History and Usage: Unused share's note changes with the choice and states the effect in plain words.
- History and Usage: Text contrast passes in both themes: muted is 5.43:1 on card and 4.58:1 on the segment track in light, 7.12:1 and 6.3:1 in dark; warn text on warn fill is 6.37:1 and 8.5:1.
- History and Usage: Times follow the 12-hour rule ('3 AM', and `full_time` uses AM/PM), and a share set elsewhere appears in order among the options rather than as nothing selected.
- History and Usage: The intro states the key consequence up front: new work stops, running agents carry on.
- QA Reviewers Worktrees: All three pages keep the same frame (title, one line of purpose, cards) and hold up at 720x460 with nothing clipped or lost. Text wraps instead of truncating.
- QA Reviewers Worktrees: Project headings sit above their rows, and row text is inset under them, as lesson L1 asks. The two-option Off/On is a fair use of a segmented control; L2's waves rule doesn't apply here.
- QA Reviewers Worktrees: Times follow the 12-hour AM/PM rule in used_at and full_time. Text contrast computed from theme.rs passes: muted on seg is about 4.55:1 in light (narrowly) and comfortable in dark.
- QA Reviewers Worktrees: The QA note is honest about what the feature does, including that earlier comments are never read. Slots fetch only while Worktrees shows, which avoids measuring disk for nothing.
- System and Update: The update page is calm and well grouped: two cards with one job each, and it holds together in light and dark at all three sizes in the captures.
- System and Update: The update model respects the owner: downloads run in the background, nothing restarts without a click, and the pill is quiet and persistent rather than a modal.
- System and Update: Verification and the atomic swap mean Taskboard.app is always the whole old or whole new app, and refusals are written as sentences at the source.
- System and Update: Off states explain themselves ("The sample board doesn't update."), Release notes follows the ready release's page, times use 12-hour AM/PM, and the 26px small buttons clear the 24px target floor.
- System and Update: The version mismatch case is handled well: an amber row, a note saying what to do, and the button promoted to primary only when it's needed.
- Hours menu: The pill states the outcome in plain words ('Agents off until Mon 9am'), and the closed state stands out from open.
- Hours menu: Time entry is forgiving: 730, 7p and 17:30 all work, with a grey preview of the rest and the chip following what's typed.
- Hours menu: Changes save at once, with no Save button, and typed text is committed on close, which suits a small popover. Esc in a field puts the value back without closing the menu.
- Hours menu: The whole schedule fits in one 280px menu at the 900x560 minimum, dims clearly when off, and the focused field has a clear accent border and chip.
- Usage pill and footer: The normal state is compact and quiet, and each window pairs a label, a bar and a number, so level never depends on colour alone.
- Usage pill and footer: Warm and hot recolour both the bar and the number, and hot adds weight, in both themes.
- Usage pill and footer: Value colours pass contrast in every live state: 8.07:1 normal, 6.18:1 warm and 5.55:1 hot in light; 8.88:1, 8.56:1 and 5.14:1 in dark.
- Usage pill and footer: The whole pill is one click target about 27px tall by code, above the 24px floor.
- Usage pill and footer: The tooltip spells out '5-hour limit: 34% used, resets …', so the short labels have a full form on hover.
- First launch: The Accounts nav badge and amber reauth row put a sign-in problem in the right place, with its action beside it.
- First launch: GitHub's browser sign-in gives clear numbered steps, offers a token route when gh is missing, and states that gh's own sign-in isn't changed.
- First launch: Hook safety is thoughtful: settings.json that doesn't parse is never rewritten, writes are atomic, a backup is kept, and the user's own hooks are left alone.
- First launch: Hook state is re-read during polling and judged by content rather than version, so outside changes show up without a relaunch.
- First launch: Token rows say exactly which scopes are needed and link to where to create the token, and Enter submits.

## Not checked

- Settings shell: Real input: the app is native and no running build was reachable, so the input driver was used only to compute contrast. Clicks, wheel scrolling, drag areas, ⌘W and closing are read from code, not exercised.
- Settings shell: VoiceOver and accessible names and roles: nothing in the app's code sets them, but whether gpui_kit exposes an accessibility tree wasn't visible. Status announcements (WCAG 4.1.3) are unchecked for the same reason.
- Settings shell: Traffic lights: none appear in any capture, so their fit in the 48px sidebar space and the sidebar's top alignment weren't seen.
- Settings shell: Capture label mismatch: captures.txt lists down-accounts-s.png as light, but the image is dark, so a light stopped-board view at 720x460 wasn't seen.
- Settings shell: Usage, QA, Reviewers and Worktrees pages, the stopped state on them, the sidebar's 'Not connected' wording, the sidebar badge, the acct-*, pill-* and usage-* captures: not in this set of 10, so checked from code only or not at all. Normal loading ('Asking the board…' while the board is healthy) wasn't captured either.
- Settings shell: Long lists: Reviewers and Worktrees build every row in a plain loop rather than a virtualized list, which breaks the project rule if those lists grow. That belongs to those sections' reviews, so it isn't rated here.
- Settings shell: Window maximum and full screen, multiple displays, Dark mode at 1400x1000, macOS Increase Contrast and Reduce Motion: no evidence provided.
- Settings shell: Contrast of the service marks (for example the GitHub tile nearly vanishes on the dark background): decorative beside a text name, so not rated.
- Settings shell: One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
- Accounts: Real input: this is a native GPUI app with no page or URL, so the driver wasn't used and no control was operated. Behaviour findings come from code and captures. inspect, the composition audit and the blurred screenshot were not run.
- Accounts: Contrast was not measured: theme colours weren't in the excerpts, so R56 is an estimate. Icon, focus-ring, badge and chip contrast are also unmeasured.
- Accounts: The GitHub code panel has no capture (code only): its look, its fit at 720x460, the expired-code state, and how the 'updates by itself' polling works. Nothing in the excerpts shows an expired-code message.
- Accounts: Signed-in, reauth, error and Agent commands states at 720x460 and 1400x1000 and in dark mode: only light 900x680 captures exist, so chip squeezing and reauth row fit at the minimum are unseen. Bitbucket connected, Slack connected and the no-gh path have no capture either.
- Accounts: Screen reader output and the macOS accessibility tree: GPUI divs seem unlikely to expose roles or names, but that wasn't tested. Status announcement (WCAG 4.1.3) is unchecked for the same reason.
- Accounts: TextField was not in the excerpts: Tab between fields, paste, and any app-level key bindings or menu commands. No menu-bar coverage or shortcuts were seen.
- Accounts: How often the page refreshes, what load() is called from, whether anything retries when the board is down, and whether the board sends reauth with an empty list or error without reauth for expired tokens.
- Accounts: Agent_cli text, CARDS and the Slack reauth payload were not in the excerpts. Numbers and reason wording in acct-* captures are stand-in data.
- Accounts: Capture labelling: down-accounts-s.png is listed as light in captures.txt but the image is dark, so no light capture of the stopped board at 720x460 was seen.
- Accounts: Larger text, a very long account name or error text, motion, and real devices or displays. The usual checks for touch, orientation, 320-wide reflow and on-screen keyboard don't apply to this macOS window.
- Accounts: One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
- History and Usage: Real input: this is a native GPUI window, so the input driver can't reach it. No control was clicked or tabbed; behaviour comes from reading code. The browser was used only for contrast maths.
- History and Usage: The composition audit and blurred screenshot weren't run on the native window; composition was judged by eye from the captures.
- History and Usage: Whether the sample board's data has the same key mismatch as the daemon: R2 rests on the capture plus the app and daemon source, not a run against a live board.
- History and Usage: No capture exists for: the amber limit-reached row, History's result and error notes, 'Cleaning up…', 'No 7-day reading yet', or Usage at 720x460 with the limit on, so the Today row's position at minimum height is unseen. No down-* capture was in this unit; the stopped-board behaviour in R17 is from code.
- History and Usage: VoiceOver and accessibility names: no accessibility tree was visible in these excerpts, and screen reader output couldn't be tested.
- History and Usage: Text scaling and 200% zoom: no capture or setting was available, and px sizes in code suggest it doesn't scale.
- History and Usage: Window maximum and full screen beyond 1400x1000, live resize, traffic-light clearance (not shown in captures), hover tooltips and motion.
- History and Usage: Work hours: named in the primary task but outside this scope. They live in the status-bar menu (ui/hours.rs), so how they interact with the daily limit wasn't reviewed.
- History and Usage: Brief versus code: the brief says this window is the only way to set these things, but code comments mention `tb usage daily`. Whether agents or the owner use that path wasn't checked.
- History and Usage: One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
- QA Reviewers Worktrees: Real input: this is a native GPUI window with no page for the driver, so no control was used. Hover, scroll, wheel chaining and clicks are read from code only.
- QA Reviewers Worktrees: The inspect step and the composition audit: neither could run, so the squint judgement and any pixel distances are by eye from captures. Contrast was computed by hand from theme.rs for the main pairs only, and bg-on-text pairs were not all checked.
- QA Reviewers Worktrees: Populated pages: no capture shows slot rows, waiting rows, pool rows, reviewer rows or pills, QA's On state, the qa-note row or the Clean up result note. Those were reviewed from code.
- QA Reviewers Worktrees: Loading ('Asking the board…') and the board-stopped state for these three pages: the down-* captures named in the brief weren't in this unit's 15 files. Only down-history-m.png was seen to exist, and it wasn't opened.
- QA Reviewers Worktrees: VoiceOver and the macOS accessibility tree: whether GPUI exposes roles and names here, and how status notes are announced. Not testable from captures. 4.1.2 and 4.1.3 are unverified.
- QA Reviewers Worktrees: Menu commands and window behaviour: Cmd+W, Cmd+, and full screen, and whether the app menu covers these commands. The app menu code wasn't reviewed.
- QA Reviewers Worktrees: Very long content: project names, many waiting tasks, or a long reviewer note (ids, emails and aliases) at 720 wide. Text zoom has no system equivalent here.
- QA Reviewers Worktrees: The real trigger for adding reviewers and creating slots, which R64's suggested wording depends on, and whether the daemon already has a POST route for pool settings, which R25 needs.
- QA Reviewers Worktrees: Slot sort order at 10 or more slots (slot-10 against slot-2): set by the daemon, not reviewed.
- QA Reviewers Worktrees: Motion and real-device behaviour. One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
- System and Update: Real input: this is a native GPUI app with no page for the driver to open, so no control was clicked, tabbed or hovered, and every behaviour finding comes from reading code.
- System and Update: Update states other than Off in a running app: checking, up to date, downloading, ready, installing and failed have no captures, so their layout and wrapping are estimates (R77 in particular).
- System and Update: The update pill: no capture shows it, and it isn't confirmed where it appears. The code calls it the status bar pill and the brief says sidebar. Whether it also shows in the Settings window wasn't checked.
- System and Update: Contrast: no inspect run and no token values in the excerpts. Text in the captures looks readable by eye but wasn't computed, in either theme.
- System and Update: Whether the System page repaints on its own when the updater thread changes state: no timer or observer is in the excerpt. If there is none, "Checking…" could stay on screen until something else redraws, which would be a further finding.
- System and Update: Whether load_info or other code clears sys_note, or what load_info does to self.info when a read fails; only the excerpted lines were read.
- System and Update: Whether running agents survive a daemon restart, which decides how R72 and R76 should be worded.
- System and Update: App menu items and shortcuts for update commands, Settings navigation by keyboard, and the window's own title and close behaviour: not in the excerpts.
- System and Update: Unsaved input in other Settings sections or windows when Restart to update quits the app.
- System and Update: Screen reader output (names, roles, status announcements), 200% text, reduced motion, sizes between the three captured, and very long data such as a multi-line codesign error.
- System and Update: The Failed, Restart daemon and mismatch states in a real installed build: the captures come from sample and dev builds, where the Restart daemon button doesn't appear.
- System and Update: Token use and consistency with other Settings pages belong to theme-review. One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
- Hours menu: Real input: the native app wasn't available, so no control was used. Findings come from code and captures, and passes 3 and 7 are incomplete.
- Hours menu: Composition audit and blurred screenshot: these need a browser page. Composition was judged from the captures only.
- Hours menu: App-level key handling (app.rs) and text_input.rs: Esc with no field focused, Tab between fields, caret position after the colon is inserted, and macOS editing shortcuts.
- Hours menu: Click-outside dismissal, including whether clicking the pill while the menu is open closes it or closes and reopens it.
- Hours menu: Board rules for overnight hours (From later than to), From equal to to, Today until on a day off or before From, and rapid saves arriving out of order.
- Hours menu: Board stopped and save-error states: no down-* capture or error capture was supplied, so R32 and R79 are from code only.
- Hours menu: States with no capture: set Today until, 'until tomorrow' and open pill labels, the keep-awake block, and Today until focused in dark or at 900x560.
- Hours menu: Measured contrast comes from scaled capture pixels, not theme tokens. Pill text in the off state wasn't measured because the tooltip overlapped the sample. Focus ring contrast wasn't computed.
- Hours menu: Text scaling, zoom and the maximum window size: not captured. Motion: not checked.
- Hours menu: VoiceOver output and real hardware.
- Hours menu: The Settings window: out of scope, including whether it also holds these settings, which would affect the one-home rule.
- Hours menu: Users and task were taken from the brief. One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
- Usage pill and footer: Real input: the native app wasn't available, so no click, hover, tooltip display, focus or tab order was exercised. Click targets and keyboard findings come from the code excerpts.
- Usage pill and footer: Lifted state: the brief mentions usage-lifted, but captures.txt lists no such capture; it was reviewed from code only, and Lifted would be clipped by R3 anyway.
- Usage pill and footer: Stale in dark, warm in dark, and every state except hot at 900x560: no captures. Stale dark contrast is computed from theme tokens. The sidebar's fixed width means R3 should hold at any size.
- Usage pill and footer: What the Days page shows, and the Settings window's panes: not in the excerpts, so R33's claim rests on the brief's rule that Settings is the only place to set limits, and the pane name for OpenUsageSettings is unknown.
- Usage pill and footer: The work-hours pill: the code puts it in the footer, but it isn't visible in any capture, and hours::pill isn't in the excerpts. A filled footer (work hours, hooks, PR feed, update, Sample board together) wasn't seen, so wrapping height at 900x560 is unconfirmed.
- Usage pill and footer: Active Goals sharing the sidebar with the footer: the captures have no goals, so the roughly 142px free at 900x560 is measured from pill-hot-dark-s.png without a populated list.
- Usage pill and footer: Stand-in data: numbers and reasons in pill-* captures are made up, and the daily 'line' tooltip text and how a reached daily limit affects agents aren't in the code given.
- Usage pill and footer: Screen reader output, including whether the UI framework exposes any accessibility tree; motion; 200% text; the macOS Increase Contrast setting; real devices.
- Usage pill and footer: Composition audit and inspect: they need a web page, so composition was judged from the captures, sizes by eye and from code, and contrast was computed from theme tokens.
- Usage pill and footer: Skipped as not applicable: forms, drag, scrolling, destructive actions, long-list virtualization.
- Usage pill and footer: One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
- First launch: Real input: this is a native GPUI app, so the browser driver had nothing to open. No control was used, and the inspect and composition audit tools weren't run. The blur judgement in R101 is by eye from a capture. Keyboard findings come from code.
- First launch: 720x460 minimum: no capture. R41 is worked out from code and measurements of the 900-wide capture, and needs checking on the real window.
- First launch: Settings with the board stopped: Accounts and System have no down-* capture, so what the forms do, and how 'Not connected', 'taskboardd: <error>' and Restart daemon read there, wasn't seen.
- First launch: Banner states in a real installed app: RequiresApproval, Blocked, Failed, in_the_way rows, and Install or Reinstall hooks have no captures. They were judged from code. How often macOS asks for approval on first launch is unknown.
- First launch: The footer in the down capture shows no hooks item. It may be hidden because the capture isn't an installed build, so its look there is unseen.
- First launch: 'Taskboard now needs .' in acct-warm-light-m.png has an empty scope list. In the product reauth is true only when missing has entries, so I treated this as a stand-in data artefact, not a finding.
- First launch: Dark mode: no dark captures. Contrast for dark was calculated by hand from theme values for placeholders only. Field edge contrast (1.4.11), icons and focus rings weren't measured.
- First launch: Screen reader output: whether GPUI exposes names, roles and status announcements to VoiceOver for these controls, including toasts [WCAG 4.1.3].
- First launch: 200% text, long error strings in banners and toasts, and very short windows.
- First launch: Work hours and usage limits: part of the primary task, but outside Flow 14 and with no capture in this set. The shell banners were judged only as steps of this flow.
- First launch: Midna's real install and launch route, and whether the app can detect it, weren't available, so R40's fix needs that confirmed.
- First launch: Tool limits: the brief allowed only the driver command in the shell. I also ran two read-only shell commands (a line count and listing, and a grep of the source), which went through; a third was denied, so I used Grep and worked out contrast by hand. None changed anything.
- First launch: One reviewer finds only some of the problems. A second reviewer, or a real first install on a clean Mac, will find more [NNG-HE].
