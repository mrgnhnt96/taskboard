## Verdict

macOS desktop app (pointer and keyboard), reviewed at one size only: 1360x860, dark, expanded width. Evidence was 10 captures, the two named code files, and the app and daemon source they call. No running app was reachable, so I used no real input and ran no blur or `inspect`. First impression: the goal page reads as organised. The work dominates, the header is quiet, and a planned goal gets one clear blue Start. The happy path to Start is sound; the weak part is what comes after it. Counts: 0 Catastrophe, 4 Major, 10 Minor, 0 Cosmetic. The top problem is on a paused goal: the header still offers blue "Start 1 more task" beside blue "Resume the goal". That Start silently unpauses the whole goal while its toast mentions only one queued task. Pause also removes the goal from the sidebar while its tasks still run. Resume then confirms with a stale "Paused" toast and a "Going again" note that never clears.

## Findings

### R1 · 3 Major · On a paused goal, Start silently resumes everything, and the two Resumes differ
- **Element:** Goal page › header and Paused banner, paused goal with planned tasks (g2-paused.png), 1360x860 dark
- **Evidence:** Confirmed
- **Problem:** The banner says 'Nothing new starts', yet a blue 'Start 1 more task' sits above a blue 'Resume the goal'. The Start button posts to goals/G/run, and api.rs run_goal (lines 2853-2854) clears paused. So one click meant to add T11 also releases T9 and T10, and the toast only says 'Started: 1 task queued'. With no planned tasks the header shows 'Resume' (goal-run, which also runs resume_parked) beside the banner's 'Resume the goal' (a plain paused:false). The same word therefore does different things.
- **Principle:** Visibility of status and consistency: one action, one name, one result, and one primary action per screen [NNG-H1] [NNG-H4] [NNG-H8] [SEC-5]
- **Fix:** While paused, the banner holds the only Resume. In run_buttons_view, drop the header's goal-run button when paused, unless the start_failed case applies. Make the banner button call goal-run so parked tasks come back too, and label it 'Resume the goal', or 'Resume and start 1 more task' when planned > 0. Have run_toast say so: 'Going again: 1 task queued'.

### R2 · 3 Major · Pausing removes the goal from the sidebar while its tasks are still running
- **Element:** Sidebar › Active goals and Goals count, after Pause (g2-paused.png against goal-g2.png)
- **Evidence:** Confirmed
- **Problem:** On Pause, G2 disappears from the list and the Goals count drops from 2 to 1, while T8 and T2 still show Working. goal_group returns Paused before Active, and active_goals keeps only Active. A paused goal is the kind most likely to be forgotten, and the surface checked through the day stops showing it. The 'Paused · 2 of 7' branch in goal_sub can't be reached from this list.
- **Principle:** Visibility of system status; recognition rather than recall [NNG-H1] [NNG-H6]
- **Fix:** Render sidebar::pinned_goals filtered to Active and Paused, with paused goals after active ones so the existing 'Paused · 2 of 7' line shows, and count them in the Goals number. At the least, keep a paused goal listed while it has working or needs tasks. The source comment says this exclusion is deliberate, but no listed owner rule forbids the change.

### R3 · 3 Major · Resume confirms with a stale 'Paused' toast and a note that never clears
- **Element:** Goal page › below the status bar, after Resume (g2-resumed.png)
- **Evidence:** Confirmed
- **Problem:** After Resume the screen shows green 'Going again' and the toast 'Paused. Running tasks carry on.' at the same time, so the owner gets two opposite statuses. Resume writes only to the inline ggate note, so nothing replaces the toast. The note also pushes every card down 40px and stays. Only a later Resume or Bring it back clears it (goal.rs:812), reset_for leaves it, and Pause sends no group. It therefore comes back on reopening the goal, and a second Pause shows 'Going again' inside the PAUSED banner (gate() renders the note there).
- **Principle:** Visibility of system status: the status shown must be true [NNG-H1]
- **Fix:** In set_paused, pass grp = None for both directions, so Resume shows a 3s 'Going again' toast that replaces the Paused toast and the layout stays put. Failures then go to the 6s error toast. Do the same for bring_back, or clear ggate:{gr} when the banner state changes.

### R4 · 3 Major · Start, Pause, Resume and their menus can't be reached from the keyboard
- **Element:** Goal page › header buttons, caret, Start menu rows, Start at, banner button; whole flow
- **Evidence:** Confirmed
- **Problem:** kit::btn and the menu rows are plain divs with on_click, and taskboard-app/src contains no tab_index or tab_stop at all. No key binding starts, pauses or resumes a goal (app.rs:1577-1589). Only the Start at field takes focus, because the code focuses it. Pointer is the only route through the primary flow.
- **Principle:** Keyboard operable, and every command reachable from a menu with its shortcut shown [WCAG 2.1.1] [MS-KBD] [HIG-MACOS]
- **Fix:** Add .track_focus and .tab_index to kit::btn and btn_primary, with Enter and Space activating and a visible focus ring. Give menus Up, Down, Enter and Esc handling. Add a Goal menu to the menu bar with Start, Pause and Resume bound to shortcuts, for example Cmd+Return for Start and Cmd+Shift+P for Pause or Resume. The gap is app-wide, so fix it in the kit.

### R5 · 2 Minor · After Start, the app says 'Started' but shows nothing starting or what comes next
- **Element:** Goal page › toast, status bar, queued rows; Sidebar › new row (r-g7-started.png, g2-more-started.png)
- **Evidence:** Confirmed
- **Problem:** The toast says 'Started: 2 tasks queued', but nothing has started. The bar stays the same grey because queued and planned look alike, 0 terminals are in use, and T67 has no line saying why it hasn't begun. The new sidebar row for G7 has an empty bar and no time, while every other row shows one. In g2-more-started.png T9 and T11 are Queued with no reason, with 1 of 3 terminals in use. That missing reason may come from the sample's Midna being down; the rest doesn't.
- **Principle:** Cognitive walkthrough Q4: after the action, users see progress toward the goal [NNG-CW] [NNG-H1]
- **Fix:** Change the toast to '2 tasks queued' and keep 'Started' for when a task goes to working. Give queued bar segments their own tint: t.accent at about 35% against the planned grey. Give the next-up row a line such as 'Next: starts when a terminal is free' when waiting_line returns null. Set last_update when a goal is run so the sidebar shows 'just now'.

### R6 · 2 Minor · Blocker toasts vanish after 3 seconds
- **Element:** Toast after goal-run or goal-run-now when tasks can't start
- **Evidence:** Confirmed
- **Problem:** with_waits can append up to two blocker sentences plus 'N more wait', such as 'T192 waits for you to do H197', but ok toasts stay only 3s (TOAST_OK) at bottom centre, far from the top-right button. That is the one message that tells the owner to act, and it gets the same time as 'Paused'.
- **Principle:** Users need enough time to read; status stays visible [NNG-H1] [WCAG 2.2.1]
- **Fix:** Keep the toast for 6s when `waiting` is non-empty, or until dismissed. Scale the time with length, about 1s per 15 characters, with a 3s minimum.

### R7 · 2 Minor · Start at has no label or time preview, and it covers the status readout
- **Element:** Goal page › Start at popover (g2-start-at.png)
- **Evidence:** Confirmed
- **Problem:** The popover holds a field whose only label is a placeholder ('5pm, tomorrow 9am'), which disappears on typing. The owner can't see how the typed time was read until after it is scheduled, and a wrongly read time on a job that spends agents is costly. On a part-run goal nothing says only the planned task is affected. The popover opens at the click point minus 240px, so it hides '2 of 7' and sits detached from the button.
- **Principle:** Visible label; constraints stated up front; error prevention [WCAG 3.3.2] [NNG-H5]
- **Fix:** Add a visible label above the field: 'Start the planned tasks at', or 'Start 1 more task at' on a part-run goal. Under the field show the parsed time live, such as 'Tomorrow at 9 AM'; this needs a check mode on the at path, or an app-side parse. Anchor the popover to the split button's bottom-right edge, at the button's rect instead of e.position().

### R8 · 2 Minor · Scheduled and work-hours times break the AM/PM rule
- **Element:** Header buttons, tooltips, toasts, placeholder, footer pill (r-g12.png, r-g6-hours-menu.png)
- **Evidence:** Confirmed
- **Problem:** 'Starts tomorrow at 1:55am' and 'Waiting until tomorrow 3am' use lowercase am with no space, on the same screens as '6:25 PM'. The doc comment on scheduled_at even promises '5 PM'. The same moment also appears three ways: 'tomorrow 3am', 'tomorrow at 3am' and 'Sat 3am'.
- **Principle:** Owner rule: 12-hour times with AM/PM; same thing, same form [NNG-H4]
- **Fix:** Change fmt::clock12 to return '3 AM' and '1:55 AM', which also covers day_clock and opens_at. Use 'tomorrow at 3 AM' on the wait button too. Change the placeholder to '5 PM, tomorrow 9 AM' after confirming parse_at accepts that form. Update the tests that pin the old strings.

### R9 · 2 Minor · The button tooltip stays on top of the menu it opened
- **Element:** Start caret and wait button menus (r-g7-start-menu.png, r-g6-hours-menu.png)
- **Evidence:** Confirmed
- **Problem:** With the menu open, the tooltip stays drawn over it. In r-g6-hours-menu.png it clips the top of the first item, 'Start now', and hides part of the goal title and the button. In r-g7-start-menu.png it covers 'Open in Claude'. It repeats what the menu already says.
- **Principle:** Keep the task in view; extra content must not obscure the control [NNG-H8] [WCAG 1.4.13]
- **Fix:** Hide the tooltip while the button's menu is open: skip .tooltip when m.menu_open(id) is set, or clear the active tooltip in toggle_menu. Also open the menu from the button's edge rather than the pointer.

### R10 · 2 Minor · How it runs disagrees with the waves and bar beside it
- **Element:** Goal page › How it runs › Order and Terminals (goal-g2.png)
- **Evidence:** Confirmed
- **Problem:** Order says 'Side by side', but the cards beside it say Wave 3 runs 'After wave 2' and T10 waits for wave 2. how_runs drops waves whenever run_in_order is off. So the owner can't predict what runs next, which is the point of this card. Terminals says '1 in use' while the status line says '2 working', because how_runs skips shared tasks and progress counts them.
- **Principle:** One fact, one place; the copies must not disagree [SEC-7] [NNG-H1]
- **Fix:** Return 'Side by side, by wave' for (false, true) in how_runs, if waves still gate there. For Terminals, count shared tasks too or say so: '3 at most · 1 in use · 1 from another goal'.

### R11 · 2 Minor · 'Waits for wave 2 to start T9' reads backwards
- **Element:** Goal page › Wave 3 › T10 wait line (goal-g2.png, g2-more-started.png)
- **Evidence:** Confirmed
- **Problem:** The line sits on T10 in Wave 3 but names T9, in a different wave, as the thing being started. It reads as though T10 will start T9, or as though wave 2 has a job the owner must do. The owner can't tell whether to wait or act.
- **Principle:** Plain language in the user's words [NNG-H2]
- **Fix:** In waves::hold_reason, use 'Waits for wave 2: T9 hasn't started yet'. Update the tests that pin the string.

### R12 · 2 Minor · Pause's banner and toast leave out that PRs wait too
- **Element:** Goal page › Paused banner and toast; wait and Start menu Pause rows (g2-paused.png, r-g6-hours-menu.png)
- **Evidence:** Confirmed
- **Problem:** The Pause tooltip says PRs wait for fixes, comments and merges, and prflow::goal_paused enforces that. The banner, toast and menu sublines only say 'Nothing new starts… running tasks carry on'. A goal with open PRs goes quiet, and the status that stays on screen never explains why.
- **Principle:** Visibility of system status [NNG-H1]
- **Fix:** Pass `reviewing` into gate_view and the toast. When true, the banner says 'Nothing new starts and PRs wait for fixes, comments and merges. Tasks already running carry on.' The toast says 'Paused. PRs wait too; running tasks carry on.'

### R13 · 2 Minor · Rows can say 'press Start' on a goal that has no Start button
- **Element:** Goal page › header against queued rows, queued-only never-run goal (r-g6-hours-menu.png)
- **Evidence:** Likely
- **Problem:** The wording in this capture is a seed effect: waits_for_start is true for manual pickup in a goal with no ran_at. The button gap is real logic, though. run_buttons_view shows Start only with planned tasks, or when paused or unstarted, so a queued-only goal gets 'Waiting until…' or Pause. The tooltip says the tasks start when work hours open, but waiting_line checks waits_for_start before hours, so they would not. In work hours there would be no route to Start on the page. Whether real goals reach this state wasn't proven.
- **Principle:** Cognitive walkthrough Q2: the correct action is visible; no dead ends [NNG-CW] [NNG-H3]
- **Fix:** Have the goal payload report whether any task waits for Start, and show the primary Start button in that case. Otherwise, if the state can't occur in production, make the daemon rule that out and leave the UI alone.

### R14 · 2 Minor · The sidebar sorts by age, so attention can sit below the fold
- **Element:** Sidebar › Active goals (r-g7.png, g2-more-started.png, g2-resumed.png)
- **Evidence:** Confirmed
- **Problem:** Rows sort by id, newest first, and about five and a half fit at this height. Open goal G2 is out of view in g2-more-started.png, so the owner can't watch the row update after pressing Start. G1, the goal with the needs-you dot in goal-g2.png, sorts last, and the last visible row is cut by the footer. The primary task is seeing which goals need attention; the Needs you count is the workaround.
- **Principle:** Primary content first; frequent in view [SEC-8] [SCR-2] [NNG-H6]
- **Fix:** Sort needs-you and stopped goals first, then the rest newest first, in active_goals. Scroll the open goal's row into view on navigation. Keep the list a virtualized scroller if it grows, per the owner's rule.

## What works

- A planned goal has one clear blue action, and after Start the header drops to Pause and Open in Claude, so the button set always matches the state (r-g7.png, r-g7-started.png).
- The Paused banner is hard to miss, and each held row explains itself with 'Its goal is paused' (g2-paused.png).
- 'Start 1 more task' says exactly how much it will do, and T68's 'Waits for T67 to finish first' makes order visible.
- A scheduled goal swaps the primary Start for a quiet button that states the time and holds Start now, Change the time and Cancel (r-g12.png).

## Not checked

- Real input: the app is a native GPUI window, no page URL was given, and the driver couldn't reach it. Clicks, hover states, Esc on menus and busy labels were read from code or captures only.
- Composition audit, blurred screenshot and `inspect` numbers: not run. Contrast was not measured, including muted text, bar segments and the planned pill outline.
- Other sizes: only 1360x860 was captured. Minimum window, large display, short height, and 200% text are unseen.
- Light theme: every capture is dark.
- Waves actually advancing: Midna isn't running in the sample, so no task moved from Queued to Working or into the next wave. Review stops and live sidebar updates were not seen.
- Not captured: 'Start tomorrow' split button and its toast, Start now's toast, 'Starts…' menu open, Start at error state, a failed Start or Pause, busy labels, and start_failed.
- Screen reader names, roles and status announcements: no accessibility tree available, and GPUI exposure not tested.
- Whether real goals reach the queued-only, manual, never-run state in R13.
- Whether parse_at accepts '5 PM' in the R8 fix, and whether waves gate tasks when run_in_order is off beyond what the G2 captures show (R10).
- Motion, real devices and multiple displays. One reviewer finds only some of the problems; a second reviewer or a usability test will find more.
