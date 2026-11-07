## Verdict

macOS desktop Settings window, History and Usage sections, pointer and keyboard. I judged 12 captures at 1400x1000, 900x680 and 720x460 in light and dark, plus the code excerpts and the daemon source. I did not run the app: it is native, so nothing was driven with real input. First impression: both pages read as calm and organised, with one card and a quiet sidebar, and the eye goes straight to the rows. But the History rows have nothing to act on. Counts: 1 Catastrophe, 5 Major, 6 Minor, 2 Cosmetic. The top problem is that History's two retention controls render as an empty 4px dot. The app asks for `detail_days_choices` and `summary_days_choices`, while the daemon sends `detail_choices` and `summary_choices`, so the owner can neither see nor change how long history is kept. This looks like a bug on the real board as well, not a sample artefact. Next come Usage's Today row going stale in a window left open all day, the 25% option cut off at the minimum window, shortening retention with no warning, and no keyboard route at all.

## Findings

### R1 · 4 Catastrophe · History's retention controls are empty, so retention can't be seen or set
- **Element:** Settings › History › 'Keep every event for' and 'Keep day summaries for' › segmented control, every size and scheme (history--light-l.png, history--light-m.png, history--light-s.png, history--dark-m.png, history--dark-s.png)
- **Evidence:** Confirmed
- **Problem:** Each row ends in a faint dot where the choices should be: an empty `kit::seg` track with only its padding. The current value (default 90 days and 1 year) isn't shown either, so the owner can't tell or change how long history is kept, and this window is the only place to do it. `history_page` reads `h[format!("{key}_choices")]`, which is `detail_days_choices`. `days.rs` L824-825 and docs/API.md send `detail_choices` and `summary_choices`, so the lookup should fail on the real daemon too, not only the sample board. `unwrap_or_default()` then hides the miss.
- **Principle:** Pass 1 walkthrough question 2, the correct action must be visible [NNG-CW]; visibility of system status [NNG-H1]; severity guide: primary task can't be finished.
- **Fix:** Pass the JSON key separately: `pick("detail_days", "detail_choices", …)` and `pick("summary_days", "summary_choices", …)`. Add a test that feeds `days::history`'s shape to `history_page` and asserts four options per row. If choices are ever empty, show the current value as plain text ('90 days') rather than an empty track. Use a pop-up button rather than `kit::seg`: by estimate, four segments ('30 days', '90 days', '6 months', '1 year') need about 270px, and only about 238px is free at 720 wide, so they would clip as R3 does.

### R2 · 3 Major · Usage and History never refresh while open, and hang on 'Asking the board…' if the first load fails
- **Element:** Settings › Usage › Today row, and History › Space used and Cleanup rows, in a window left open; both pages when the board is down at open
- **Evidence:** Confirmed
- **Problem:** The poll loop refreshes accounts, slots and info, but not `daily` or `history`. `load_daily` runs only at open and on a sidebar click, and `load_history` runs only at open. An owner resting on Usage never sees the amber 'used up' row appear, and 'Lifted' stays on screen past midnight after the lift has expired, while the board's pill shows the true numbers. `load_history` and `load_daily` also drop errors with `if let Ok`. If the board is stopped when the window opens, History says 'Asking the board…' indefinitely, even after the board returns, and clicking History doesn't retry. Usage retries only on a sidebar click.
- **Principle:** Visibility of system status [NNG-H1]; one fact, one place: copies must not disagree [SEC-7]; each data state has a way forward [mobile:NNG-EMPTY].
- **Fix:** In the existing poll closure, add `if s.view == Sec::Usage && tick % IDLE_TICKS == 0 { s.load_daily(cx) }` and the same for History, which gives a 30s refresh. Call `load_history` on a click of the History sidebar item. On `Err`, replace the placeholder with 'The board isn't answering. This page fills in when it's back.' and keep retrying on the poll.

### R3 · 3 Major · The Daily limit control is cut off at the minimum window, and the 25% option can't be reached
- **Element:** Settings › Usage › Daily limit › base control at 720x460, light and dark (usage--light-s.png, usage--dark-s.png)
- **Evidence:** Confirmed
- **Problem:** The six-option control runs past the card's right edge, leaving only a '2' of '25%', and the card's `overflow_hidden` removes the rest, so 25% can't be clicked or seen as selected. Measured from the captures, the control is about 284px wide with six options and about 330px with seven. It therefore clips below a window width of about 766, or about 812 once a share set elsewhere adds a seventh (usage-daily-light-m.png). The note is squeezed to four lines at the same time. A share set elsewhere, like 12%, also disappears from the row once the owner picks another option, and the window can't set it again.
- **Principle:** Option count fits the control at the narrowest width, then becomes a menu [HIG-SEGMENTED] [NNG-LISTBOX]; every control at every size [NNG-H6]; LESSONS L2: a set that grows gets a pop-up menu, not a segmented control.
- **Fix:** Replace `daily-base` with a pop-up button built from `kit::popover`, `kit::menu_box` and `kit::menu_item`, listing Off, Even, 10%, 15%, 20%, 25% and any other current share. The button is about 90px wide, so the row fits at 720 and the note drops to two lines. Keep 'Next day / Spread' as a segmented control, since two options fit.

### R4 · 3 Major · Shortening retention or cleaning up deletes history with no warning, and the note understates what goes
- **Element:** Settings › History › retention rows and Cleanup › 'Clean up now'
- **Evidence:** Confirmed
- **Problem:** Once R1 is fixed, one click on '30 days' saves at once and the 3 AM run permanently deletes everything older, with no count, confirm or undo. 'Clean up now' does the same immediately, and neither it nor its row says what will go. The note says 'of finished tasks', but `cleanup` also deletes terminal lines for all tasks by date, plus usage samples, host limits and slot spans. The result message counts all of it as 'old events'.
- **Principle:** Error prevention: confirm or undo destructive actions [NNG-H5]; user control [NNG-H3]; destructive actions worded clearly (pass 3).
- **Fix:** When a choice is shorter than the current one, keep the old value and show an amber row in the card: 'Shortening to 30 days removes 1,204 events (2.1 MB) at the next cleanup', with 'Shorten' and 'Keep 90 days' buttons. This needs a count from the daemon, for example `GET history?preview_days=30`. Give 'Clean up now' the same confirm when anything would be removed. Reword the note: 'Commits, questions, checkpoints and status changes of finished tasks, plus all terminal output and usage readings of that age.'

### R5 · 3 Major · Nothing on these pages can be reached from the keyboard
- **Element:** Settings › sidebar items; History › 'Clean up now'; Usage › Daily limit, Unused share and Today controls, all sizes
- **Evidence:** Confirmed
- **Problem:** `kit::seg`, `kit::btn` and the sidebar rows are divs with `on_click` only. No `tab_index` or key handler appears in `settings.rs` outside the text fields, and only the root takes focus. The owner can't Tab to a control, arrow through a group or press Space to lift a limit, and no focus ring exists.
- **Principle:** Keyboard operable [WCAG 2.1.1]; focus visible [WCAG 2.4.7]; arrows move inside groups, which take one tab stop [MS-KBD].
- **Fix:** Give `kit::btn` and sidebar rows `.tab_index(0)` with Space and Enter activation. Make each `kit::seg` one tab stop where ← and → move the selection. Sidebar ↑ and ↓ switch sections. Draw a 2px `t.accent` outline on the focused element. Doing this in `kit` covers every Settings page at once.

### R6 · 3 Major · The selected segment is barely distinct, most of all in dark
- **Element:** Settings › Usage › all segmented controls, dark (usage-lifted-dark-m.png, usage--dark-m.png); same pairs in light (usage-daily-light-m.png)
- **Evidence:** Confirmed
- **Problem:** In dark, the selected segment is a slightly darker patch on the track, 1.13:1 (#1a1f26 on #232931), and its text differs from unselected text by only 1.91:1. Weight is the same in both states. Whether '12%' or 'Lifted' is the live choice is hard to read at a glance, and it reads more like a recess than a selection. Light is 1.19:1 fill and 2.98:1 text. It reads better thanks to the white fill and shadow, but it is still under 3:1. No non-colour cue exists.
- **Principle:** Non-text contrast for state [WCAG 1.4.11]; use of colour [WCAG 1.4.1]. Computed from theme.rs values.
- **Fix:** In `kit::seg`, give the selected item `FontWeight::SEMIBOLD` and a 1px border in `t.muted`, which is 6.3:1 on the dark track and 4.58:1 on the light one. Keep the fill quiet. Don't use the accent colour, so each row keeps at most one emphasis [SEC-5].

### R7 · 2 Minor · Changing a limit setting silently restarts today's count and drops the carry
- **Element:** Settings › Usage › Daily limit and Unused share, any change mid-day (limit on)
- **Evidence:** Confirmed
- **Problem:** The plan is keyed to the setting (`sig`), so `advance` discards it on any change and starts fresh with `day_pct` at the current reading. Today's used amount returns to 0, and carried share is lost. Switching from Next day to Spread at 4 PM can hand agents a full extra day's share, or a limit that was reached stops holding, and the page gives no hint.
- **Principle:** No surprises from a control [WCAG 3.2.2 in spirit]; error prevention [NNG-H5]; visibility of status [NNG-H1].
- **Fix:** In `advance`, when only the setting changed within the same weekly window, keep the kept plan's `day_pct` so today's used count survives. If restarting is intended, say so in the Daily limit note: 'Changing this restarts today's count.'

### R8 · 2 Minor · The Today row repeats its label, uses a mode switch for a one-day action, and sits last
- **Element:** Settings › Usage › Today row, limit on and lifted (usage-daily-light-m.png, usage-lifted-dark-m.png)
- **Evidence:** Confirmed
- **Problem:** The label 'Today' sits above text that starts 'Today: …' or 'Today's limit…', and `usage_daily::line` confirms the real wording does too. 'Limit / Lifted' looks like a lasting mode, but lifting expires at midnight, and 'Limit' doesn't say what clicking it does. This is the row checked all day and the only one that goes amber, yet it comes below two rarely changed settings.
- **Principle:** Project rule: no filler labels; frequent in view [SEC-8]; label matches the result [NNG-CW]; consistent wording [NNG-H4].
- **Fix:** Have the daemon add a `today_line` without the prefix ('7% used of 12%, 5% left'; 'Lifted until midnight: 7% used of 12%') and keep `line` for the pill tooltip. Replace the toggle with one button: 'Lift for today', or 'Put limit back' while lifted. Move the row to the top of the card when the limit is on, followed by Daily limit and Unused share.

### R9 · 2 Minor · With no weekly reading, Limit / Lifted looks live but does nothing visible
- **Element:** Settings › Usage › Today row, limit on with no 7-day reading
- **Evidence:** Confirmed
- **Problem:** `state()` returns before adding `overridden` when `now()` is None, so the row shows 'No 7-day reading yet.' beside an enabled toggle. Clicking 'Lifted' posts the override, but the answer has no `overridden`, so the toggle snaps back to 'Limit' with no message. 'Daily limit' can also be set with nothing to measure it against, and nothing says the limit holds nothing until a reading arrives. '7-day' is also a different word from the page's 'weekly'.
- **Principle:** Feedback after an action [NNG-H1]; a state must not contradict its controls (checklist, simulated state rule); consistent words [NNG-H4].
- **Fix:** When `d["allowed"]` is missing, hide the toggle and show: 'No weekly reading yet. Nothing is held until one arrives.'

### R10 · 2 Minor · Saving shows no pending state, and clicks during a call are dropped
- **Element:** Settings › Usage › all controls while `daily_busy`; History › retention rows while `history_busy`
- **Evidence:** Confirmed
- **Problem:** On Usage, `busy` only removes the click handler. Segments keep their pointer cursor and hover, and nothing shows that a save is under way, so a click on a slow board seems to do nothing. On History, `post_history` returns early when busy, so a retention click during 'Cleaning up…' is dropped silently and the selection stays as it was.
- **Principle:** States are designed, a slow action shows progress [NNG-H1]; disabled controls look disabled (pass 3).
- **Fix:** Add a `busy` argument to `kit::seg` that applies the same `opacity(0.45)` and `cursor_default` as `kit::disabled` to unselected items. Mark the clicked option as selected straight away, and revert with the error note on failure. Pass `history_busy` into `pick` so retention rows disable during a cleanup.

### R11 · 2 Minor · The percent choices give no sense of what they mean for the week
- **Element:** Settings › Usage › Daily limit, off and on (usage--light-m.png, usage-daily-light-m.png)
- **Evidence:** Confirmed
- **Problem:** The note says a percent gives each day that share of the week, but over seven days 15% is 105%, 20% is 140% and 25% is 175%, so those choices can run out before the reset. 10% leaves 30% unused. Choosing needs the week's use and reset, which the page doesn't show, though `resets_at` and `days_left` are already in the response. With the limit off the page shows no numbers at all. Owner decision: the weekly reading's home is the board's pill.
- **Principle:** Recognition rather than recall [NNG-H6]; error prevention [NNG-H5].
- **Fix:** Make the note follow the choice, as Unused share's already does. For example, 20%: 'Each day gets 20% of the week, so the week lasts about 5 days.' Even: 'Splits what's left of the week over the days to its reset.' Optional, needs the owner's OK under one home per object: add a line to the Usage row, 'Week: 42% used, resets Sun 3:00 PM.'

### R12 · 2 Minor · Rows stretch with the window and part labels from their controls
- **Element:** Settings › History and Usage › cards at 1400x1000 (history--light-l.png, usage--light-l.png)
- **Evidence:** Confirmed
- **Problem:** Cards fill the whole pane, so at 1400 wide the row's text and its control sit roughly 1,000px apart for 'Cleanup' and over 700px for 'Daily limit'. The eye has to track a long way along a row to pair a setting with its value, and notes run to long lines. The rest of the window stays empty.
- **Principle:** Desktop window range: nothing stretches past a readable length at the maximum [tablet:AND-T2] [NNG-FITTS].
- **Fix:** Cap the rows with `.max_w(px(680.))` on the `history-rows` and `usage-rows` content, left-aligned with the page title, across all Settings pages for consistency.

### R13 · 1 Cosmetic · History never says how far back it actually goes
- **Element:** Settings › History › Space used row (history--light-m.png)
- **Evidence:** Confirmed
- **Problem:** The page intro promises how far back the Days page goes, but the row shows counts and sizes only, and the `oldest` date the daemon sends goes unused. The count text also reads wrongly at one ('1 events'), and that count includes status changes and terminal lines.
- **Principle:** Visibility of status [NNG-H1]; plain language [NNG-H2].
- **Fix:** Rename the row 'Stored' and write: 'Back to Aug 12, 2026: 247 events (16 KB), 10 day summaries (27 KB).' Handle the singular.

### R14 · 1 Cosmetic · History and Usage have near-identical sidebar glyphs
- **Element:** Settings › sidebar › History ◷ and Usage ◔, adjacent rows (history--light-s.png)
- **Evidence:** Confirmed
- **Problem:** Both are small circles with a wedge or hand, directly above one another, so the icons don't help tell the two sections apart.
- **Principle:** One look per role, distinct where meaning differs [NNG-H4].
- **Fix:** Keep ◷ for History and use a gauge or bar glyph for Usage, or draw both with `kit::icon` for a matched set.

## What works

- The page structure is clear: name, one-line purpose and control per row, one card, and a quiet sidebar that keeps the page itself in charge.
- The Limit rows appear only when the limit is on, so the Off state is one simple row (usage--light-m.png).
- Unused share's note changes with the choice and states the effect in plain words.
- Text contrast passes in both themes: muted is 5.43:1 on card and 4.58:1 on the segment track in light, 7.12:1 and 6.3:1 in dark; warn text on warn fill is 6.37:1 and 8.5:1.
- Times follow the 12-hour rule ('3 AM', and `full_time` uses AM/PM), and a share set elsewhere appears in order among the options rather than as nothing selected.
- The intro states the key consequence up front: new work stops, running agents carry on.

## Not checked

- Real input: this is a native GPUI window, so the input driver can't reach it. No control was clicked or tabbed; behaviour comes from reading code. The browser was used only for contrast maths.
- The composition audit and blurred screenshot weren't run on the native window; composition was judged by eye from the captures.
- Whether the sample board's data has the same key mismatch as the daemon: R1 rests on the capture plus the app and daemon source, not a run against a live board.
- No capture exists for: the amber limit-reached row, History's result and error notes, 'Cleaning up…', 'No 7-day reading yet', or Usage at 720x460 with the limit on, so the Today row's position at minimum height is unseen. No down-* capture was in this unit; the stopped-board behaviour in R2 is from code.
- VoiceOver and accessibility names: no accessibility tree was visible in these excerpts, and screen reader output couldn't be tested.
- Text scaling and 200% zoom: no capture or setting was available, and px sizes in code suggest it doesn't scale.
- Window maximum and full screen beyond 1400x1000, live resize, traffic-light clearance (not shown in captures), hover tooltips and motion.
- Work hours: named in the primary task but outside this scope. They live in the status-bar menu (ui/hours.rs), so how they interact with the daily limit wasn't reviewed.
- Brief versus code: the brief says this window is the only way to set these things, but code comments mention `tb usage daily`. Whether agents or the owner use that path wasn't checked.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
