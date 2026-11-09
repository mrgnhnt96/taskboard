## Verdict

macOS desktop app (native GPUI, pointer and keyboard), single owner. Reviewed from 11 captures at 900x560, 1360 wide and 1728x1084, dark and light, plus the code. Nothing was driven live, because the browser can't reach a native app, so all keyboard and behaviour findings come from code. First impression: the page reads as organised. Rows, columns and shelves are clean, the type scale is calm, and the blue Continue button is the brightest spot. But the order doesn't serve the primary task. Rows sort by project then id, so needing attention changes neither a row's position nor its action. G1's question sits under an api goal, G4's failure is fifth, and every row but G3 offers Pause. Counts: 0 Catastrophe, 6 Major, 13 Minor, 4 Cosmetic. Top problem: the owner has to read all eight rows, then expand one, to find and act on what needs them (R1, R2). At the smallest window the attention text itself is truncated (R4). None of the fixes need an owner rule changed.

## Findings

### R1 · 3 Major · In-progress rows aren't ordered or marked by what needs attention
- **Element:** Goals › In progress list, all sizes and both themes (r-goals.png, r-goals-light.png, r-goals-small.png, r-goals-large.png)
- **Evidence:** Confirmed
- **Problem:** in_progress() sorts by project then id, so a working api goal (G14) leads, G1 (needs you) is 2nd, G3 (stopped for review) is 4th, G4 (failed) is 5th and G13 (held) is last. All cards look the same; only one line of coloured text differs. At 900x560 about four and a half rows fit, so with more goals an urgent row can sit below the fold. The sidebar lists the same goals in a different order (G14, G13, G6, G5...), so position can't be learned either.
- **Principle:** Primary content first, squint test [SCR-2] [NNG-VH]; Recognition rather than recall [NNG-H6]; Flexibility and efficiency [NNG-H7]
- **Fix:** In in_progress(), sort into stable bands, then by id: needs you or stopped for review, failed, held (R3), working, in review, queued. St::rank() already gives most of this order. The project chip stays on each row, so project sort isn't lost. Give rows in the top two bands a quiet marker: card border in t.warn_line or t.down_line, or a 3px left edge in the status colour. Don't add a new section or labels. Use the same order in the sidebar's Active goals.

### R2 · 3 Major · Rows that need the owner offer Pause, not the action that moves them
- **Element:** Goals › In progress › row action, G1 and G4 (r-goals.png, goals.png)
- **Evidence:** Confirmed
- **Problem:** goal_row() always gives Pause unless a wave is stopped. G1 says 'T3 waits on your answer' but its only button is Pause, and that text isn't clickable. To answer, the owner must expand the row, find the row and press Answer (r-goals-expand-all.png). G4 failed and also only offers Pause. Eight near-identical buttons make the row's one strong control the least useful one, and a misclick pauses the goal that was waiting on you.
- **Principle:** Cognitive walkthrough Q2, Q3 [NNG-CW]; one primary action [NNG-H8]; Frequent in view, rare behind [SEC-8]
- **Fix:** Extend Action with Answer(task ref) and Open(task ref). When the tally has a Needs task, the slot shows 'Answer T3' and calls m.open_task, using the same right panel as the task row. When a task failed, show 'Open T54'. Keep Continue as it is. Show Pause only on rows with no owner action. Pause stays on the goal page for the other rows.

### R3 · 3 Major · A goal held back from finishing just says 'Done' and offers Pause
- **Element:** Goals › In progress › G13 Replay every error (r-goals.png, r-goals-large.png)
- **Evidence:** Confirmed
- **Problem:** G13 shows 1 of 1, a full green bar and 'Done', yet it stays In progress with Pause. goals.rs only leaves a goal unfinished if bits are waiting, an outcome missed, a failure is unaccepted or a PR is open. now_line() knows none of this and falls through to 'Done'. The owner sees a finished goal in the wrong list and can't tell it is blocked, though the goal page already has the words ('Waiting on 1 bit'). The exact reason in this seed wasn't checked.
- **Principle:** Visibility of system status [NNG-H1]; Help users diagnose [NNG-H9]; Walkthrough Q4 [NNG-CW]
- **Fix:** When every task is settled but the goal isn't finished, use the goal page's state instead of 'Done': 'Waiting on 1 bit' or 'Outcome not met' in St::Needs colour, with the reason on the second line. Reuse goal_state from goal/pool.rs. Rank it in the attention bands (R1), and drop Pause, since nothing is left to pause.

### R4 · 3 Major · At the smallest window, truncation removes the attention text
- **Element:** Goals › In progress rows at 900x560, dark (r-goals-small.png)
- **Evidence:** Confirmed
- **Problem:** The row keeps the same five columns at every width, so all of them shrink together. '1 working · 1 need…', 'Stopped for your r…', 'T3 waits on your answ…', 'Sign-in with…', 'Checkout on…' and 'Wav…' are all cut. The words that say a goal needs you are the ones lost, while 20px gaps, the 110px action column and the chevron column stay fixed. Opening the right task panel likely narrows the page further (not captured).
- **Principle:** Every control at every size, narrowing keeps the order [SCR-6]; window reflow [HIG-MACOS]
- **Fix:** Below about 800px of page width, switch goal_head to a compact layout. Name column takes the full width on line 1 with ref and chip. Line 2 is the status line at full width. Progress becomes a 96px bar plus '1 of 4', and the wave text is dropped from the row because it is in the open body. Reduce row gap from 20 to 12. Never truncate row.now before row.name: give it flex_none up to its text width.

### R5 · 3 Major · Nothing on the Goals page can be reached from the keyboard
- **Element:** Goals › every row, ref, Pause, Continue, Answer, Open PR, Expand all, Collapse all, shelf line, earlier link
- **Evidence:** Confirmed
- **Problem:** goals_page.rs and kit's buttons have no focus handle, tab stop or key handler; MainWindow holds the only page-level focus. There is no Tab order, no visible focus, and rows can't be opened or Continue pressed without a pointer. Cmd-number navigation and the project picker's own arrows work, but the primary task doesn't. Found by reading code; Tab wasn't pressed in the app.
- **Principle:** Keyboard [WCAG 2.1.1]; Focus visible [WCAG 2.4.7]; Arrows within groups [MS-KBD]
- **Fix:** Give the list one tab stop with roving selection: Up and Down move a visible selected row (2px t.accent ring), Right or Enter opens it, Left closes it, Cmd+Down opens the goal page. A key for the row's action, such as Space, runs Answer, Continue or Pause. Add focus handles to kit::btn so buttons take Tab and Space. Bind through the existing MainWindow key context.

### R6 · 3 Major · Shared tasks count in the total but aren't listed or tallied
- **Element:** Goals › G2 Faster sign-in, folded and open (goals.png, r-goals-open.png)
- **Evidence:** Confirmed
- **Problem:** G2 says '2 of 7', but the open row lists six tasks and the bar has six units. The summary's total uses counted_tasks (own plus shared). The detail's 'tasks' holds own tasks only, and shared cards arrive in a separate 'shared' key that goal_body, Tally::of and next_line never read. The owner can't find the seventh task. Worse, a shared task that needs you drops out of the row's status once details load, while the goal page does show it. This isn't the known Projects issue.
- **Principle:** One fact, one place [SEC-7]; Visibility of system status [NNG-H1]
- **Fix:** Read tasks and shared together, as goal.rs:122 already does, in Tally::of and next_line. In goal_body, add a 'Shared' section after 'No wave' with the same task_row, as goal/waves.rs does.

### R7 · 2 Minor · Pause can't be undone in place, and paused goals go quiet
- **Element:** Goals › row Pause and Paused shelf (goals-both-paused.png)
- **Evidence:** Confirmed
- **Problem:** One click on a 24px button moves the goal out of the list to a shelf. The toast 'Paused. Running tasks carry on.' has no Undo, and the shelf line has no Resume: the owner must open the goal page and use its banner. The shelf shows only 'Paused · 1 of 4', although G1 had a worker running and an open question. The sidebar's Needs you count also goes from 1 (goals.png) to blank; whether that's intended wasn't checked.
- **Principle:** User control and freedom [NNG-H3]; Visibility of system status [NNG-H1]
- **Fix:** Add Undo to the toast, calling set_paused(false). Add a small Resume button to each Paused shelf line, which is a one-click signal the owner rules allow. Replace the filler 'Paused · ' prefix, since the shelf title already says it, with live state: '1 of 4 · 1 working · 1 needs you'.

### R8 · 2 Minor · The shelf strip never wraps, so narrow tiles lose goal names
- **Element:** Goals › Up next, Paused, Set aside, Finished this week at 900x560
- **Evidence:** Likely
- **Problem:** shelf_strip is one no-wrap row of equal columns, and each line's meta is flex_none. At 900 wide there's about 596px of content, so three shelves leave about 159px inside a tile and four leave about 108px. '2 PRs merged · 6:24 PM' alone is about 127px, leaving no room for the name. Inferred from code and measured widths: no capture shows shelves at this size, or the four-shelf layout.
- **Principle:** Every control at every size [SCR-6]; Recognition rather than recall [NNG-H6]
- **Fix:** Use flex-wrap with a 280px minimum column, so shelves fall to two columns or one. Under about 240px of tile width, put meta on its own line under the name.

### R9 · 2 Minor · Section counts fail text contrast in both themes
- **Element:** Goals › group labels' counts: In progress 8, Up next 4, and so on (r-goals.png, r-goals-light.png)
- **Evidence:** Confirmed
- **Problem:** group_label draws the count in t.faint at 12px on the page background: 2.84:1 in light (#8a95a3 on #f6f7f9) and 4.31:1 in dark (#6f7a88 on #0f1216). Both are under 4.5:1.
- **Principle:** Contrast [WCAG 1.4.3]
- **Fix:** Use t.muted for the count: #5f6b7a gives 5.06:1 on the light background. Keep the weight difference from the title.

### R10 · 2 Minor · Progress bar segments can't be told apart
- **Element:** Goals › row progress bar (r-goals-light.png, r-goals.png)
- **Evidence:** Confirmed
- **Problem:** Queued, Planned and Stopped all use t.border_2, at 1.48:1 on the light card and 1.63:1 on the dark card, so unstarted work barely shows and a stopped task looks queued. Needs and Failed differ by hue only: 1.27:1 against each other in light and 1.35:1 in dark. Segments have no label or tooltip. The status text carries the headline, but the bar's breakdown can't be read.
- **Principle:** Non-text contrast [WCAG 1.4.11]; Use of colour [WCAG 1.4.1]
- **Fix:** Fill Queued and Planned with t.faint: 3.04:1 on the light card, about 3.8:1 on the dark card. Give Planned a hollow outline to separate it from Queued, and Stopped its own solid colour. Add a hover tooltip per segment ('2 queued'). Put Failed and Needs apart in parts(), or raise the lightness gap between them.

### R11 · 2 Minor · G4 reads 'failed' on the page and 'stopped for you' in the sidebar
- **Element:** Goals › G4 Nightly export row against sidebar line (r-goals.png)
- **Evidence:** Confirmed
- **Problem:** The row says '1 failed', driven by wave state. sidebar.rs says '1 of 3 · stopped for you', driven by the goal's stopped field, and goal_row's summary fallback says 'Stopped for your review' until details land. So G4's state changes by place and time, there's no Continue, and the owner can't tell whether to review or to fix. G3 shows the same 'stopped for you' wording with a real Continue.
- **Principle:** One fact, one place [SEC-7]; Consistency [NNG-H4]
- **Fix:** Decide the goal's state once and share it. If a wave failed, say '1 failed' in the row, the sidebar and the summary fallback. Use 'stopped for you' only when a Continue exists.

### R12 · 2 Minor · Folded 'Queued' doesn't say what it's waiting for, and 'press Start' has no Start
- **Element:** Goals › G5, G6 folded rows; open task rows T51, T52, T79 (r-goals.png, r-goals-open.png, r-goals-expand-all.png)
- **Evidence:** Confirmed
- **Problem:** G5 reads only 'Queued', with no second line. next_line never uses the task's waiting text, so a goal held by hours, a wave or the owner looks the same. The 'Waits for you to press Start' wording here is a seed effect: waits_for_start only returns true for pickup=manual, so it isn't rated as a bug. The gap behind it is real: task_row has actions only for Needs and Review, so a manual task says press Start with no Start in the row, and the folded row hides that it waits on you.
- **Principle:** Visibility of system status [NNG-H1]; Walkthrough Q2 [NNG-CW]
- **Fix:** When a goal has nothing running, let next_line fall back to the first queued task's waiting line. If that task waits for Start, add a Start button in task_row's action slot, a one-click signal the rules allow, and rank the goal with those needing the owner.

### R13 · 2 Minor · Finished shelf keeps its 'this week' title when showing older goals
- **Element:** Goals › Finished this week › 'N earlier' link
- **Evidence:** Confirmed
- **Problem:** Expanding earlier adds older goals to the same tile under 'Finished this week', and the count stays at this week's number. With nothing finished this week the shelf reads 'FINISHED THIS WEEK 0' with only a link in it. No capture shows this; it is read from shelf_tile and shelf_strip.
- **Principle:** Match with the real world [NNG-H2]; Visibility of system status [NNG-H1]
- **Fix:** With earlier on, title the shelf 'Finished' and count lines plus earlier, or add a quiet 'Earlier' sub-label above the older lines. With zero this week, title it 'Finished' and show the link alone.

### R14 · 2 Minor · Project filter can disappear while still applied, and its counts don't match the page
- **Element:** Goals › header › project filter (r-goals-filter-menu.png)
- **Evidence:** Confirmed
- **Problem:** The control only renders when tabs.len() > 1, and tabs leave out finished goals. If 'api' is picked and the other project's open goals all finish, only one tab is left: the filter vanishes but stays applied, hiding the other project's finished goals with no way to clear it. Counts also leave out finished goals, so the menu's 'webapp 12' is likely one less than the 13 goals the page shows, if G10 is webapp.
- **Principle:** User control and freedom [NNG-H3]; Readouts tell the truth [NNG-H1]
- **Fix:** Render the filter when tabs.len() > 1 or project != "all". Count every goal the filter would show, finished this week included, or reset to 'all' when the control hides.

### R15 · 2 Minor · A failed detail fetch shows 'Loading…' forever
- **Element:** Goals › open row body
- **Evidence:** Confirmed
- **Problem:** app.rs keeps goal_rows only on Ok and silently drops an Err. goal_body then shows 'Loading…' until a later refresh succeeds, the wave text stays blank, and the row falls back to summary counts. The owner can't tell slow from broken. Read from code; the state wasn't captured.
- **Principle:** Help users recover from errors [NNG-H9]; each state designed [NNG-H1]
- **Fix:** Record failed refs next to details. In goal_body, show 'Couldn't load this goal's tasks' with a Try again link that calls m.refresh. Clear the entry on land().

### R16 · 2 Minor · Goal rows and shelves aren't virtualized
- **Element:** Goals › In progress list, open bodies, shelf tiles, 'N earlier'
- **Evidence:** Confirmed
- **Problem:** render() builds every card, every task of every open goal, and every shelf line in plain loops inside one scroller. The earlier list grows without end and an Up next tile has no cap. This breaks the owner's rule that long lists are virtualized, and Expand all is the worst case.
- **Principle:** Owner rule: long lists virtualized; Aesthetic and minimalist design [NNG-H8]
- **Fix:** Flatten the page into row items (group label, goal head, wave head, task row, shelf strip) and draw them with gpui's list(), as home/view.rs, review.rs and releases.rs do with their virtual lists. Cap each shelf at 8 lines with an 'N more' link.

### R17 · 2 Minor · Tasks aren't indented under their wave header
- **Element:** Goals › open row › wave headers and task rows (r-goals-open.png, r-goals-expand-all.png)
- **Evidence:** Confirmed
- **Problem:** wave_head and task_row both start at INSET (60px): 'Wave 1 · Measure' and the 'Done' chip share a left edge. Only the band colour marks the group, and the owner has already corrected this pattern.
- **Principle:** LESSONS L1: label above members, members indented under it; Grouped by space [SEC-2]
- **Fix:** Set task_row's left padding to INSET + 16 (76px) when it sits under a wave or 'No wave' header. Keep 60px for goals with no waves, which have no header.

### R18 · 2 Minor · The goal ref is a small target and the row gives no hover cue
- **Element:** Goals › row › ref link and row head (r-goals.png)
- **Evidence:** Confirmed
- **Problem:** The ref is the row's only route to the goal page. It is bare 12px text, about 14–21 wide and 17 high, under the 24px floor, and it sits inside a clickable row, so a near miss expands the row. goal_head has a pointer cursor but no hover style, unlike task_row, and the 12px chevron is the only hint that it opens.
- **Principle:** Target size [WCAG 2.5.8]; Frequent targets [NNG-FITTS]; States [NNG-H1]
- **Fix:** Give the ref min 24x24 with padding and a matching negative margin so the layout doesn't move. Add .hover(bg t.panel_2) to goal_head, as task_row has, and enlarge the chevron to 14px.

### R19 · 2 Minor · Expand all and Collapse all don't track state or scope
- **Element:** Goals › header › Expand all, Collapse all (r-goals-filtered.png, r-goals-expand-all.png)
- **Evidence:** Confirmed
- **Problem:** Both show at all times, even for one row (r-goals-filtered.png) and even when they would do nothing. Expand all opens only the filtered goals, but Collapse all clears m.goals.open, including rows open under another project, so switching the filter back finds them shut.
- **Principle:** Consistency [NNG-H4]; Everything earns its place [SCR-5]
- **Fix:** Use one button that reads 'Collapse all' when any shown row is open and 'Expand all' when none is, hidden when going.len() == 1. Make it remove only the refs in going.

### R20 · 1 Cosmetic · Second lines repeat or read oddly
- **Element:** Goals › rows and task lines (r-goals.png, r-goals-open.png)
- **Evidence:** Confirmed
- **Problem:** G4 reads '1 failed' then 'T54 failed'. '1 task in wave 3' doesn't say it comes next. T10's 'Waits for wave 2 to start T9' (waves.rs:127) reads as if wave 2 must start T9, and it sits under a header that already says 'After wave 2'.
- **Principle:** One fact, one place [SEC-7]; Match with the real world [NNG-H2]
- **Fix:** For a failure, make the second line the task's title or reason. Write 'Then 1 task in wave 3'. Drop the wave waiting line when the wave header already says it.

### R21 · 1 Cosmetic · One age column mixes three kinds of time
- **Element:** Goals › open row › task age column (r-goals-expand-all.png)
- **Evidence:** Confirmed
- **Problem:** The same column shows 'now', 'under a minute', '1h 5m' and '7:56 PM': time since asking, elapsed run time and clock time, with no way to tell them apart, and 'now' and 'under a minute' side by side.
- **Principle:** Consistency [NNG-H4]
- **Fix:** Map 'under a minute' to 'now' in short(), and let the status chip explain the rest. If it still confuses, prefix finished times with 'at'.

### R22 · 1 Cosmetic · Filter menu opens at the pointer, not at the button
- **Element:** Goals › header › project filter (r-goals-filter-menu.png)
- **Evidence:** Confirmed
- **Problem:** project_filter places the menu from the click position, so it starts about 43 image px right of the button's left edge, and the spot changes with where you click.
- **Principle:** Alignment [SCR-4]; platform pop-up behaviour [HIG-MACOS]
- **Fix:** Anchor the menu to the button's bounds: left edge aligned, 4px below.

### R23 · 1 Cosmetic · Empty state is bare and off the page's left edge
- **Element:** Goals › no goals (r-empty.png)
- **Evidence:** Confirmed
- **Problem:** 'No goals' floats centred under a left-aligned title on a blank page, and doesn't say how goals appear in a read-only app.
- **Principle:** Designed empty states with a way forward [mobile:NNG-EMPTY]
- **Fix:** Left-align it to the 28px page edge and say what happens: 'No goals yet. Goals your agents start show up here.'

## What works

- The row's three columns (what it is, how far along, what it's doing now) read fast, and aligned bars make scanning easy at 1360 and 1728 wide.
- Continue is the single filled button, so a goal stopped for review stands out and the action is right there.
- Opening in place keeps context: wave headers say when and why ('Done · waiting for your review', 'After wave 1'), and tasks open in the right panel, which keeps to one home per object.
- The roomy Paused shelf (goals-both-paused.png) uses spare space well, with each goal's purpose under its name.
- Both themes hold together: muted text passes on cards and bands (5.06:1 light on page background, 7.12:1 dark on card), and times use 12-hour AM/PM.

## Not checked

- Live use: this is a native GPUI app, so no control was clicked, typed or scrolled. The browser was used only to compute contrast. All behaviour findings come from code, and the composition audit, inspect and blurred screenshot couldn't run.
- Composition blur test: judged from full-size captures, not a blurred image.
- VoiceOver and accessible names: no accessibility code turned up in the app crate, but gpui_kit wasn't searched, so screen reader output, roles and status announcements are unknown.
- Compact Paused tile and the four-shelf layout: no capture shows them, nor shelves at 900x560 (R8 is inferred).
- Right task panel open on this page: not captured, so how far it squeezes rows is unknown.
- Data states with no capture: loading, board down, detail load failed, in review and pending rows, stopped and no-wave goals, 'N earlier' expanded, Open PR.
- Light theme at 900 and 1728, and in the open, filtered, paused and empty states: only one light capture was given.
- Why G13 is held in this seed, and why Needs you goes blank after pausing G1: the reason wasn't traced.
- G10's project, which decides whether the menu's 'webapp 12' really differs from the page (R14).
- Double-click on Continue or Pause: this page doesn't disable them while busy, and whether the request is deduped wasn't traced.
- Motion, 200% text (macOS has no app text scale here) and real display scaling.
- Sample state the brief excludes was not rated: Midna, No work hours, Sample board, identical 'just now' times, 'Done.' summaries (seed.rs:248), and the Projects shared-count issue.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
