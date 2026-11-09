## Verdict

macOS desktop, pointer and keyboard. Reviewed at 1360x860, 1360x1500 and 900x560, in dark and one light capture, from 9 captures plus the source. The app is native, so nothing was driven with real input and the blurred-screen audit was not run. First impression: the header is calm and organised, with one clear blue action, a dominant title and a quiet readout, but it gets cluttered the moment a menu opens. Counts: 0 catastrophe, 4 major, 10 minor, 1 cosmetic. The top problem is R1: the button's tooltip stays up over its own open menu, hiding the button label, part of the first menu row and even the goal title. The most serious problem for the primary task is R3: a goal stopped for your review shows no sign of it in the header and offers only Pause. No fix here needs the owner's OK. 'Waits for you to press Start' is a seed effect and is not rated (see not_checked).

## Findings

### R1 · 3 Major · The tooltip covers the menu it belongs to
- **Element:** Goal › header › Start caret, scheduled button, waiting button, at 1360x860 dark, menu open (g2-start-menu.png, r-g7-start-menu.png, r-g12-menu.png, r-g6-hours-menu.png)
- **Evidence:** Confirmed
- **Problem:** With the pointer still on the button, its tooltip draws over the open menu. In r-g12-menu.png it hides most of 'Starts tomorrow at 1:55am' and touches the 'Start now' row. In r-g6-hours-menu.png it hides the 'Waiting until' label and part of the goal title. In g2-start-menu.png it cuts across Pause and Open in Claude. This happens on the menus that start work, and each tooltip only repeats what the menu already says.
- **Principle:** Hover content must not obscure the task; every element earns its place [WCAG 1.4.13] [NNG-H8] [SCR-2]
- **Fix:** In buttons() (goal.rs:970-1015), attach the tooltip only while that button's menu is closed, for example .when(m.menu_open(SCHED_MENU).is_none(), |b| b.tooltip(...)). Do the same for START_MENU on goal-run-caret and WAIT_MENU on goal-wait. Also keep it closed when the menu is dismissed, until the pointer leaves and comes back.

### R2 · 3 Major · Header buttons and menus can't be used from the keyboard
- **Element:** Goal › crumbs, header buttons, Outcome not met, all menu rows, at every size
- **Evidence:** Confirmed
- **Problem:** Start, Pause, Open in Claude, the crumbs and every menu row are mouse-only. The app source has no tab_index or tab_stop, and track_focus appears only on the root, text fields and a few panels. Esc closes a menu, and the Start at field does take focus, type and submit, but the only way to reach that field is a click. Rated 3 rather than 4 because the owner works with a pointer and `tb start` exists outside the app.
- **Principle:** All functions work from the keyboard; arrows move inside menus [WCAG 2.1.1] [MS-KBD] [WCAG 2.4.7]
- **Fix:** Give kit::btn, kit::link and the crumb links a FocusHandle with tab_index(0), a visible 2px accent focus ring, and Enter and Space activation. In menus, move focus to the first row on open, use Up and Down between rows, Enter to run, and return focus to the trigger on Esc. Add a goal-page shortcut for the primary action, for example Cmd+Return for Start. Open the menu from the button's bounds, not e.position(), so a keyboard open lands in the right place (see R6).

### R3 · 3 Major · A goal stopped for your review looks like a running goal in the header
- **Element:** Goal › header › buttons and progress readout, stopped goal G3 (r-g3-light.png, r-g3-small.png)
- **Evidence:** Confirmed
- **Problem:** The header reads '2 of 4 done · 2 queued' and offers Pause, which suggests the goal is moving and might need slowing. In fact nothing will happen until you press 'Continue to wave 2', and that button lives inside a wave card that scrolls. The owner's main question is which goals need them, and the header doesn't answer it. On a goal stopped at a later wave the action would likely start below the fold (Likely, not captured). The same state also has three names: 'stopped for you' in the sidebar, 'waiting for your review' on the wave, and 'Review stop' in How it runs.
- **Principle:** Visibility of system status; the correct action must be noticeable [NNG-H1] [NNG-CW] [SEC-8]
- **Fix:** When a wave is held for review, add a branch to gate_view so the existing pinned gate banner shows: label 'Waiting for your review', text 'Wave 1 is done. Look it over, then let wave 2 start.', button 'Continue to wave 2'. Remove the same banner from the wave card so the action has one home. Hide Pause in this state, since nothing new can start. Append ' · waiting for your review' to the readout in warn_text, and use that one phrase in the sidebar too.

### R4 · 3 Major · At the smallest window the aside squeezes the work until task titles disappear
- **Element:** Goal › body under the header, 900x560 dark (r-g3-small.png)
- **Evidence:** Confirmed
- **Problem:** The wave column shrinks to about 234px while How it runs keeps about 330px, so the secondary panel is wider than the work. Task rows show only 'Done T49' with no title, the wave heading cuts off at 'waiting for y', and the review banner wraps to one or two words per line beside its button. The pinned header also takes about 190 of 560px. This sits just below the header, but it is in the supplied capture and holds the action a stopped goal needs.
- **Principle:** The work dominates; narrowing keeps the order [SCR-1] [SCR-6] [HIG-MACOS]
- **Fix:** When the main area is under about 760px wide, stack the aside below the waves (or collapse it to a 'How it runs' row with Show), so the waves get the full width of about 580px at 900. Let the banner use flex_wrap with the button on its own line below the text. Keep the task title as the last column to give up space: hide the status line and 'Ships one PR' first.

### R5 · 2 Minor · The Start caret opens a one-item menu that opens another popover
- **Element:** Goal › header › Start caret › Start menu, inside work hours (g2-start-menu.png, r-g7-start-menu.png)
- **Evidence:** Confirmed
- **Problem:** Outside the after-hours case, start_menu_items returns only 'Start at…', so the menu is a single row that must be clicked to reach the real field. That is one extra click and a jump to a new place on screen, with a tooltip saying the same thing as the row.
- **Principle:** Flexibility and efficiency; every element earns its place [NNG-H7] [NNG-H8] [SCR-5]
- **Fix:** When start_menu_items returns one item, have the caret call open_at_menu directly. Keep the sentence 'It starts then, whatever the work hours' as help text in the popover (see R8). Keep the menu for the after-hours case, where it has two or three real choices.

### R6 · 2 Minor · Menus open at the pointer, not under their button
- **Element:** Goal › header › all menus, 1360x860 (g2-start-menu.png, g2-start-at.png, r-g12-menu.png, r-g13-misses.png)
- **Evidence:** Confirmed
- **Problem:** Every menu opens at click x minus 240 (minus 300 for misses), so where it lands depends on where on the button you clicked, and on a wide button that can shift it a long way. The menus don't line up with their triggers and they cover nearby facts: the '2 of 7 done' readout, and the How it runs heading and task time in r-g13-misses.png. Start at jumps again, to below the row that was clicked, about 390px left of the caret, with nothing connecting it to Start. kit::popover's own comment says it is 'anchored under the element that opened it', which isn't what happens.
- **Principle:** Consistency; menus sit next to what opened them [NNG-H4] [desktop:HIG-POINTER]
- **Fix:** Anchor each menu to its trigger's bounds: right edge flush with the button's right edge, 6px below it. Capture the bounds with on_prepaint or a canvas measure rather than e.position(). Open Start at in the same anchor as the menu it replaces, so it swaps in place. Align the misses menu's right edge with the end of 'Outcome not met'.

### R7 · 2 Minor · 'Cancel' in the scheduled menu reads as close, but drops the schedule at once
- **Element:** Goal › header › scheduled menu › Cancel (r-g12-menu.png)
- **Evidence:** Confirmed
- **Problem:** In a menu, 'Cancel' usually means 'never mind'. Here it removes the scheduled start immediately, with no confirm and no Undo, and the time has to be retyped to get it back. It is the last row, right where someone looking to dismiss would click.
- **Principle:** User control and freedom; destructive actions are worded clearly and can be undone [NNG-H3] [NNG-H5]
- **Fix:** Rename the row 'Remove start time', with the sub 'It stays planned until you press Start'. Separate it from the two rows above with a divider. Use m.toast_undo for 'G12 isn't scheduled any more', with an Undo that posts the previous start_at back to goals/{gr}/run.

### R8 · 2 Minor · The Start at field has no label, loses its format hint and has a faint edge
- **Element:** Goal › Start at popover (g2-start-at.png)
- **Evidence:** Confirmed
- **Problem:** The only guidance is the placeholder '5pm, tomorrow 9am', which goes away as soon as you type. The 'whatever the work hours' note stayed behind in the menu. In Change the time the field opens empty, so the current time isn't shown and the button still says 'Start at'. The field border #3a424d on #1a1f26 is about 1.6:1 (#cfd5dd on white about 1.5:1), and the field fill matches the popover. The caret also sits on the placeholder's first character, so '5pm' is hard to read.
- **Principle:** Visible labels; formats stated before typing; 3:1 for control boundaries [WCAG 3.3.2] [WCAG 1.4.11] [NNG-H5] [mobile:NNG-PLACEHOLDER]
- **Fix:** Add a 12px label above the field, 'Start at', and a help line below it, 'Like 5pm or tomorrow 9am. It starts then, whatever the work hours.' Rename the button to 'Schedule', since the label now carries 'Start at'. From Change the time, prefill the field with the current time, select it, and label the button 'Change'. Use a border colour with at least 3:1 against the card, such as #6f7a88 (t.faint) on dark, plus an accent border on focus. Put 2px of padding before the placeholder.

### R9 · 2 Minor · Scheduled and waiting times break the AM/PM rule and read three ways
- **Element:** Goal › header › scheduled button, waiting button and their tooltips, toasts (r-g12-menu.png, r-g6-hours-menu.png)
- **Evidence:** Confirmed
- **Problem:** The header shows 'Starts tomorrow at 1:55am', 'Waiting until tomorrow 3am' and '(tomorrow at 3am)', while the same screen shows 'Done 6:25 PM'. The sidebar pill gives the same moment as 'Sat 3am'. The owner's rule is 12-hour times with AM/PM. fmt::clock12 returns lowercase with no space, although the comment on scheduled_at promises 'today at 5 PM'.
- **Principle:** Consistency: same thing, same form [NNG-H4]; owner rule on 12-hour times with AM/PM
- **Fix:** Change fmt::clock12 to return '1:55 AM' and '3 PM', and update its tests at fmt.rs:376-378 and any strings that rely on it, including the hours pill. Check the daemon's hours::say_when for the same format. Use one pattern in the button labels: 'Starts tomorrow at 1:55 AM' and 'Waiting until tomorrow at 3 AM'.

### R10 · 2 Minor · A start more than a week away shows only a weekday
- **Element:** Goal › header › scheduled button and its toast (scheduled_at, goal.rs:240)
- **Evidence:** Confirmed
- **Problem:** parse_at accepts a full date such as 2026-11-20T09:00, but scheduled_at prints any date past tomorrow with %A. A start two weeks out reads 'Starts Friday at 9am', which looks like this week's Friday, so the owner could believe work starts sooner than it will. opens_at has the same pattern. Seen in code only, with no capture of this state.
- **Principle:** Visibility of system status; readouts tell the truth [NNG-H1]
- **Fix:** Use the weekday only within the next 6 days. Beyond that, use 'Nov 20 at 9 AM'. Share one helper between scheduled_at and opens_at, since they repeat the same day logic.

### R11 · 2 Minor · 'Outcome not met' doesn't look clickable
- **Element:** Goal › header › progress readout › Outcome not met (r-g13-misses.png)
- **Evidence:** Confirmed
- **Problem:** It renders as coloured status text beside the count, and the underline appears only on hover, so nothing says it opens a menu. Every other menu trigger in this header has a ▾. The menu then repeats the same rows as the Backlog section right below it, with the same issues shown twice on screen and the menu covering the top of that aside.
- **Principle:** Looks like what it is; one fact, one place [WCAG 1.4.11] [NNG-H6] [SEC-4] [SEC-7]
- **Fix:** Add a ▾ after the text and a permanent underline, matching the other triggers. Keep the menu, because the Backlog section shows only the first few issues, and anchor it under the trigger as in R6.

### R12 · 2 Minor · The misses menu has no height limit
- **Element:** Goal › misses menu (misses_menu, goal view lines 209-236)
- **Evidence:** Confirmed
- **Problem:** Every open miss becomes a row in a plain div, with no max height, scrolling or virtualizing. A goal with many misses gets a menu taller than the window; it snaps to the window and the rows past the edge can't be reached. Titles also truncate with no tooltip. Only two rows were captured; the overflow comes from reading the code.
- **Principle:** Owner rule: long lists virtualized; every item reachable [NNG-H6]
- **Fix:** Cap the menu at min(360px, window height minus 80px) and render rows through the app's virtual list, as the goal page's lists do. Add a tooltip with the full title on truncated rows.

### R13 · 2 Minor · Crumbs and the ref row say the same thing back to back
- **Element:** Goal › crumbs and header ref row, all captures
- **Evidence:** Confirmed
- **Problem:** 'Goals › webapp › G2' is followed 40px lower by 'G2 [webapp]'. Both the project crumb and the project pill go to the same place, so two rows of chrome carry one fact and sit above the title.
- **Principle:** One fact, one place; everything earns its place [SEC-7] [SCR-5]
- **Fix:** Remove the ref row's G2 and project pill and keep the crumbs. Make the crumb's G2 selectable (sel 'goal-ref'), move the repo_path tooltip to the project crumb, and put the epic link at the end of the crumb row. This also returns about 28px to the pinned header.

### R14 · 2 Minor · A long goal name makes the pinned header eat the page
- **Element:** Goal › header › title, long name G6 (r-g6.png, r-g6-hours-menu.png), projected to 900x560
- **Evidence:** Likely
- **Problem:** The title has no line limit and the header doesn't scroll. The 86-character name takes two lines at 1360 (confirmed). At 900 wide, with the buttons beside it, the title has about 330px and would take about five lines. Added to a tldr and a gate banner, that would leave little of the 560px height for the waves.
- **Principle:** Sticky areas leave room for content [WCAG 1.4.10] [SCR-1]
- **Fix:** Clamp the title to two lines with line_clamp(2) and show the full name in a tooltip. When the window height is under about 640, let the tldr scroll with the body rather than stay pinned.

### R15 · 1 Cosmetic · Typing 'cancel' in Start at may report 'Scheduled'
- **Element:** Goal › Start at popover › schedule_goal
- **Evidence:** Likely
- **Problem:** The daemon's is_off treats off, none, cancel and clear as dropping the schedule, but the client checks only for exactly 'off'. If the route uses is_off, typing 'cancel' removes the schedule and the toast falls back to 'Scheduled'.
- **Principle:** Commands never report success they didn't have [NNG-H1]
- **Fix:** Have the toast read the response: if it has no start_at, say 'G12 isn't scheduled any more'.

## What works

- One blue primary action per header, with the state in the button itself: 'Start 1 more task', 'Starts tomorrow…', 'Waiting until…'. The label says what happens next.
- Menu rows pair a bold action with a one-line consequence ('Nothing new starts; running tasks carry on'), which suits one-click signals.
- Start at's flow is sound: the field opens focused, Enter submits, Esc cancels and returns focus, and errors appear in the popover in plain words.
- The Attached wave picker is a pop-up rather than tabs (L2), and T8's files sit indented under their task label (L1). Light theme holds up well in r-g3-light.png.

## Not checked

- Real input: the app is native, so no control was clicked, typed or hovered by me, and the browser driver can't reach it. Findings come from captures and code.
- The composition audit and blurred-screen check, which need a web page. The first impression comes from the captures by eye.
- 'Waits for you to press Start' (r-g6.png, r-g3-light.png): not rated. waits_for_start is true only for manual-pickup tasks on a goal that has never run, and chain.rs asserts that a goal's own queued tasks don't show it, so this is a seed effect.
- Screen reader output: whether these controls expose any name or role to VoiceOver.
- States with no capture: the after-hours split button and its Start tomorrow menu, a paused or deprioritized gate, the epic link, busy labels, the Start at error line, toasts, and the header while loading.
- Whether the goal run route uses is_off (R15), and whether the time parser accepts '5 PM' with a space.
- Contrast beyond the Start at field border. Text colours were not measured, and focus indicators don't exist yet (R2).
- Window sizes between 900 and 1360, very large displays, system text size and motion.
- Hover, pressed and disabled looks of the header buttons, which appear in no capture.
- One reviewer finds only some of the problems. A second reviewer or a usability test will find more.
