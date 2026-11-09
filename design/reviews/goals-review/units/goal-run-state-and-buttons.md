## Verdict

macOS desktop app, pointer and keyboard, reviewed for one owner running many agents. Evidence is 16 captures (15 dark at 1360x860, plus 900x560, 1728x1084 and one light) and the code: goal.rs, plus the goal/waves.rs, sidebar.rs, fmt.rs and taskboardd sources I traced from it. It is a native app with no page to open, so nothing was driven with real input. First impression: the page is calm and organised. The work dominates, waves read in order and How it runs stays quiet, so the problem is what the page says rather than clutter. Counts: 0 Catastrophe, 6 Major, 8 Minor, 3 Cosmetic. Top problem: the app doesn't reliably tell the owner which goals need attention. The Needs you badge stays at 1 while goals sit stopped, failed, held or short of their outcome (R1). The goal header never states the goal's state, and goal_state(), which holds all those labels, is only called from tests (R2). Close behind: Start on a paused or set-aside goal silently lifts the pause or brings the goal back (R3).

## Findings

### R1 · 3 Major · Needs you and the sidebar miss goals that are waiting on the owner
- **Element:** Sidebar › Needs you badge and Active goals rows, all extra-seed captures (r-g3.png, r-g4.png, r-g5.png, r-g13.png), dark, 1360x860
- **Evidence:** Confirmed
- **Problem:** The badge reads 1 while G3 waits for a review, G4 has a failed task, G5 is held and G13 missed its outcome. Rows are no better: the orange dot only shows for needs > 0, so G4 ('stopped for you') has none. G5 reads a plain '1 of 2'. G13 reads '1 of 1' under a full green bar, which looks finished. The owner scanning the sidebar concludes one thing needs them. Code: waiting_count() counts only tasks in needs with question or attention, plus gone-quiet alerts, so stage items on the Needs you list are left out. goal_sub() has no held or miss branch.
- **Principle:** Visibility of system status [NNG-H1]; one fact, one place [SEC-7]; the brief's primary task is seeing which goals need attention
- **Fix:** Add stopped stages to the count the badge reads, so it matches the Needs you list: add stopped, non-resting goals to waiting_count(), or give the badge its own count if the agent count must stay. Show the dot when needs > 0, or when stopped is set, a wave is held or misses > 0. Extend goal_sub(): '1 of 2 · wave 2 held', '1 of 1 · 2 misses', '1 of 3 · T54 failed'. Paint a miss as a warn segment at the end of the sidebar bar, or at least stop it reading as all green.

### R2 · 3 Major · The goal header never says what state the goal is in
- **Element:** Goal page › header and progress line, all states; clearest in r-g3.png, r-g4.png, r-g5.png, r-g10.png
- **Evidence:** Confirmed
- **Problem:** Nothing at the top says 'Waiting on you', 'Held' or 'Done'. r-g5.png reads '1 of 2 done · 1 queued', which sounds healthy. r-g10.png only implies finished through '2 of 2 done', with no finish time. In r-g3.png, r-g4.png and r-g5.png the one header control is Pause, whose tooltip says running tasks carry on, on a goal that has already stopped with 0 terminals in use. The real action sits in a wave card, which can be well down a long goal: in goal-g2.png the wave 3 stop sits under two other cards. goal_state() has all these labels, but only tests call it, despite its comment saying the Goals list shows it.
- **Principle:** Visibility of system status [NNG-H1]; squint test [SCR-2]; walkthrough question 2, will users notice the correct action
- **Fix:** Render goal_state() as a tone-coloured chip in the ref and project row of header(): 'Waiting on you', 'Held', 'Done', 'Blocked', 'Outcome not met'. Add a Held case. First fix a bug in it: line 129 returns 'Done' with tone up when every task has status done, even if one has failed, so check b(t,'failed') before returning Done. For a finished goal, add the finish time next to it, as 'Done 6:25 PM'. Hide Pause while the goal is stopped at a gate with nothing running.

### R3 · 3 Major · Start on a paused or set-aside goal silently undoes the pause or brings it back
- **Element:** Goal page › header Start button beside the gate banner, g2-paused.png and r-g9.png
- **Evidence:** Confirmed
- **Problem:** Two blue buttons compete. The banner says 'Nothing new starts', yet 'Start 1 more task' or 'Start' sits above it. The tooltip only says it queues planned tasks, but POST goals/:g/run also clears paused and deprioritized (api.rs line 2853). One click meant for one task sets the whole goal running, and the toast only says 'Started: 1 task queued'. With no planned tasks, a header 'Resume' sits beside the banner's 'Resume the goal', and the two differ: only the header one also resumes parked tasks.
- **Principle:** Consistency and standards [NNG-H4]; error prevention [NNG-H5]; one emphasis [SEC-5]
- **Fix:** In run_buttons_view, push no run button while gate_view(g) is Some, so the banner holds the only primary action. Send Resume through goals/:g/run, or make the banner's call also resume parked tasks, so there is one meaning. If Start stays on a paused goal, label it 'Resume and start 1 more task' and say so in the toast, and do the same for a set-aside goal with 'Bring back and start'.

### R4 · 3 Major · A chained goal looks exactly like one waiting for Start
- **Element:** Goal page › header and How it runs, r-g8.png compared with r-g7.png
- **Evidence:** Confirmed
- **Problem:** G8 runs after another goal, but its page shows nothing about that: same primary Start, same How it runs as an ordinary planned goal. goal.rs never reads after or after_blocker; only the Goals page, Projects and the sidebar line do, and a chained goal is in the Next group, so it isn't in the sidebar. Start can't start it early either: the tasks queue and hold until the earlier goal finishes (test a_chained_goal_run_early_holds_its_tasks), and the board runs it anyway once that goal finishes. The owner is told to press something that isn't needed and can't do what it says.
- **Principle:** Visibility of system status [NNG-H1]; walkthrough question 3, does the label match the result
- **Fix:** When after_blocker is set, replace primary Start with a quiet non-primary label 'Starts after G7', with a link to that goal. Add a How it runs row: 'Runs after  G7 Search rewrite'. Keep the caret menu for a set time only if a scheduled start can really go ahead of the chain; otherwise drop it.

### R5 · 3 Major · The failed gate offers only the bypass, and no way to retry
- **Element:** Goal page › Wave 1 gate, r-g4.png
- **Evidence:** Confirmed
- **Problem:** 'T54 failed. Wave 2 waits.' has one button, blue 'Continue anyway', which skips the failure with no confirmation and no word on what wave 2 will inherit. The backend's own text says 'Try again, or continue past it', but the row's Try again only appears for start failures (start_failed), not failed work. The only retry is in the task panel, so the owner must know to open the row. The failure reason is cut to 'The bucket refused the uplo…' at 1360, right where it's needed.
- **Principle:** Help users recognize, diagnose and recover from errors [NNG-H9]; error prevention [NNG-H5]; one emphasis [SEC-5]
- **Fix:** In the failed gate, make 'Try again' the primary button, with the same requeue signal the panel uses (one failed task; 'Try again all' for several). Make 'Continue anyway' secondary, with a tooltip: 'Wave 2 starts without T54's work'. Show the reason on its own line in the gate, up to 2 lines, instead of relying on the truncated row cell.

### R6 · 3 Major · At the smallest window, task titles disappear
- **Element:** Goal page › wave rows and wave header, g2-small.png, 900x560
- **Evidence:** Confirmed
- **Problem:** How it runs keeps its width and the wave cards shrink to about 235 pt. The rows' fixed columns need about 370 pt (status 92, ref 36, PR 120, age 44, plus padding and gaps), so titles and reason lines get no room. Rows show only 'Done T6' and 'Queued T9', the PR column is cut at 'Ships on', and 'Review stop after this wave' is cut to 'Re'. The owner can't tell tasks apart. A gate's text and button would have the same width; that is Likely, as no gate was captured at this size.
- **Principle:** Every control at every size; the work stays the biggest thing [SCR-1] [SCR-6] [NNG-H8]
- **Fix:** When the main column is under about 900 pt wide, move the aside below the waves at full width, or fold it to a How it runs line. Under about 520 pt per card, drop the PR column and age from rows so titles keep at least 180 pt. Let the review-stop pill wrap below the wave title instead of being clipped.

### R7 · 2 Minor · 'Side by side' contradicts the waves on the same screen
- **Element:** How it runs › Order, goal-g2.png and g2-large.png
- **Evidence:** Confirmed
- **Problem:** Order says 'Side by side', while wave 3 reads 'After wave 2' and T10 reads 'Waits for wave 2 to start T9'. how_runs maps run_in_order false to 'Side by side' even when waves exist, but runner.rs holds tasks by wave regardless ('Waves take the place of running in order'). The owner can't trust the panel that explains how the goal runs.
- **Principle:** One fact, one place [SEC-7]; consistency [NNG-H4]
- **Fix:** Change the match in how_runs so any waved goal reads 'By wave'; 'In order, by wave' only adds noise. Keep 'In order' and 'Side by side' for goals without waves.

### R8 · 2 Minor · Two time formats on one page, one against the owner's AM/PM rule
- **Element:** Header scheduled button r-g12.png ('Starts tomorrow at 1:55am'); sidebar in r-g3-hours.png ('Agents off until Sat 3am'); tooltips, menus and toasts via fmt::clock12
- **Evidence:** Confirmed
- **Problem:** Run signals use lowercase '1:55am' while task rows and wave headers use '6:25 PM'. The owner's rule is 12-hour times with AM/PM, and the doc comment on scheduled_at even promises 'today at 5 PM'. clock12 is the source, so every run message has the mismatch.
- **Principle:** Owner rule: 12-hour times with AM/PM; consistency [NNG-H4]
- **Fix:** Make clock12 return '1:55 AM' and '3 PM', with a space and capitals, and update its tests and doc comment. Check callers such as the sidebar work hours chip for width. The Start at field's placeholder '5pm, tomorrow 9am' can stay, as it's an input hint.

### R9 · 2 Minor · Outside work hours, Pause is hidden and the time is cut off
- **Element:** Goal page › header and wave rows, r-g3-hours.png
- **Evidence:** Confirmed
- **Problem:** Pause vanishes from the header when the split button and waiting state coincide (line 333, !(split && waiting)). It moves into the caret menu, whose tooltip only says 'Start now or at a set time instead'. 'Start tomorrow: 1 more task' also reads as if the goal is idle until tomorrow, while T8 is Working and wave 2 says Now. The row reason is cut to 'Waits for work hours (tomorr…', losing the time, and the page says 'tomorrow' while the sidebar says 'Sat'.
- **Principle:** Recognition rather than recall [NNG-H6]; frequent in view [SEC-8]
- **Fix:** When split && waiting, still push the plain Pause button instead of hiding it, and remove Pause from start_menu_view. Or, at minimum, change the caret tooltip to 'Start now, at a set time, or pause'. State the hours once on the goal, for example a line under the progress bar, 'Queued tasks start tomorrow at 3 AM', and shorten the row reason to 'Work hours' so it fits.

### R10 · 2 Minor · A missed outcome reads as finished and has no next step
- **Element:** Goal page › progress line, r-g13.png
- **Evidence:** Confirmed
- **Problem:** The bar is full green and the only signal is right-aligned amber text. Every other stopped state gets a banner or gate, but this one has none, and the only button is Open in Claude. 'Outcome not met · 2 misses' is plain text that opens a menu with no affordance beyond hover, and that menu repeats the Backlog list in the aside.
- **Principle:** Visibility of system status [NNG-H1]; looks like what it is [SEC-4]; one fact, one place [SEC-7]
- **Fix:** Extend gate_view with a third banner: label 'Outcome not met', text 'Every task is done, but 2 misses hold this goal open.', with the two misses linked. Drop the click menu from the progress line, since the Backlog section already opens each issue. Add a trailing warn segment to the bar so it no longer reads as complete.

### R11 · 2 Minor · A goal with no tasks has no empty state
- **Element:** Goal page › main column, r-g11.png
- **Evidence:** Confirmed
- **Problem:** The header shows an empty bar and '0 of 0 done', then the left column is blank beside a How it runs panel for work that doesn't exist. goal_state already has 'No tasks yet', but it never reaches the screen. Nothing says what to do next, and Open in Claude isn't presented as that step.
- **Principle:** Empty states with a way forward [mobile:NNG-EMPTY]; help users know what's next [NNG-H1]
- **Fix:** When there are no tasks, replace the bar and count with 'No tasks yet', and fill the main column with one line: 'This goal has no plan yet. Open it in Claude to add tasks.' Make Open in Claude the primary button in this state only. Hide the Terminals row while there is nothing to run.

### R12 · 2 Minor · 'Resume' appears on a goal that isn't paused, and Pause goes away
- **Element:** Goal page › header buttons, state with a task whose start failed (goal.rs lines 310-333); no capture
- **Evidence:** Confirmed
- **Problem:** With a start_failed task and nothing planned, the label falls through to 'Resume' on a goal that was never paused, while the tooltip talks about tasks that couldn't start. The !unstarted guard also removes Pause, so one failed start leaves no way to pause the rest of the goal. A goal with a lost task, which goal_state calls 'Needs a restart', gets only Pause in the header, though goal-level Restart exists on Home.
- **Principle:** Match between the system and the real world [NNG-H2]; user control and freedom [NNG-H3]
- **Fix:** For unstarted, label the button 'Try again' (or 'Try 2 again'), and drop !unstarted from the Pause condition. For lost tasks, show 'Restart' on the goal header using the same goals/:g/restart signal as Home.

### R13 · 2 Minor · 'Waits for you to press Start' can appear with no Start on the goal
- **Element:** Goal page › queued rows and header, r-g3.png, r-g4.png, r-g5.png
- **Evidence:** Confirmed
- **Problem:** On these three captures the line is a seed effect: waits_for_start needs manual pickup in a goal with no ran_at, which a goal that really ran doesn't have, so don't read those pages as the live text. The gap behind it is real. Agents add queued tasks with manual pickup (reports.rs line 2965). In a never-run goal with nothing planned, run_buttons_view shows no Start, only Pause, because the run branch needs planned > 0 or a paused or unstarted goal. goals/:g/run would resume these tasks, but nothing offers it. The task panel has its own Start, though I didn't trace whether it shows in this state.
- **Principle:** Walkthrough question 2, will users notice the correct action [NNG-CW]; consistency [NNG-H4]
- **Fix:** Add a parked count to run_buttons_view (status queued, pickup manual, no line_session, goal not run). When it's above 0, show the primary 'Start' with the title 'Queue the tasks waiting for Start'. This needs pickup and ran_at in the goal payload if they aren't already there.

### R14 · 2 Minor · 'Wave 2 is held.' doesn't say why or until when
- **Element:** Goal page › Wave 2 gate, r-g5.png
- **Evidence:** Confirmed
- **Problem:** The review stop gate explains itself ('Look it over, then let wave 2 start'), but the held gate only restates the header's 'Held'. The owner can't tell who held it, whether anything else will release it, or whether it's safe to let go. The backend's wording is more useful ('is held until it's continued') but doesn't reach this gate.
- **Principle:** Visibility of system status [NNG-H1]; no filler labels (owner rule)
- **Fix:** Change the gate text to 'Wave 2 won't start until you let it.' If the hold has a recorded reason or author, append it, for example 'Held by Claude: waiting on the API change'. Keep the 'Let wave 2 start' button.

### R15 · 1 Cosmetic · '2 working' and '1 in use' disagree
- **Element:** Progress line and How it runs › Terminals, goal-g2.png
- **Evidence:** Confirmed
- **Problem:** The progress line counts the shared T2 and Terminals doesn't, because how_runs reads tasks and progress reads counted(). It's explainable, but looks like a bug on first read.
- **Principle:** One fact, one place [SEC-7]
- **Fix:** Optional: when shared tasks are running, show '3 at most · 1 in use · 1 shared'.

### R16 · 1 Cosmetic · The split button shows no busy state for menu actions
- **Element:** Header › Start button and caret (goal.rs lines 961-971)
- **Evidence:** Confirmed
- **Problem:** Start now and Start at use their own busy keys, but the main button only checks goal-run and the caret is never disabled. While those requests run, the button stays live with no feedback, and a second Start can be sent.
- **Principle:** Visibility of system status [NNG-H1]
- **Fix:** Optional: treat goal-run-now and goal-at as busy for the split button, as sched-menu already does, and disable the caret too.

### R17 · 1 Cosmetic · 'Deprioritized' is a heavier word than the app's own 'set aside'
- **Element:** Gate banner, r-g9.png, and the row reason 'Its goal is deprioritized'
- **Evidence:** Confirmed
- **Problem:** The API and Home code call this 'set aside', and the banner's button says 'Bring it back', but the label on screen is 'Deprioritized'.
- **Principle:** Match between the system and the real world [NNG-H2]
- **Fix:** Optional: label the banner 'Set aside' and the row reason 'Its goal is set aside'.

## What works

- The review stop gate is the model for the others: it says what happened, what to do and what happens next, and the button names the result ('Continue to wave 2').
- Button labels follow the run state closely: 'Start 1 more task', 'Start tomorrow: 1 more task' and 'Starts tomorrow at 1:55am' each say what will really happen, and tooltips cover the PR follow-ups a pause holds.
- Composition holds across sizes and themes from 1360 up: the waves dominate, How it runs stays quiet, and light and dark read the same (goal-g2.png, g2-light.png, g2-large.png).
- The owner's lessons are kept: Attached uses a pop-up for waves (L2) and puts the task caption above its indented files (L1).
- Fixes above stay within the owner's rules: each new control is a one-click signal, so none needs the owner's OK for a rule change.

## Not checked

- Real input: this is a native gpui app with no page or URL, so the browser driver had nothing to open. No control was clicked, typed or tabbed; behaviour comes from reading code. Target sizes and contrast were not measured.
- Keyboard and VoiceOver: focus order, focus rings, whether the caret, the menus or the 'Outcome not met' text can be reached by keyboard, and accessible names. Not visible in captures.
- Open menus and transient states: the Start, Waiting, Scheduled and Start at menus, tooltips, busy labels and most toasts were read from code; only the Paused toast was seen.
- States with no capture: 'Waiting until tomorrow 3am' with nothing planned, start_failed, lost tasks, awaiting merge, Blocked, Waiting on bits, a gate at 900x560, and any state other than running in light, small or large windows.
- Whether the task panel's Start (view.rs line 1466) shows for a manual task in a never-run goal, and what each failed task's panel offers beyond Try again.
- Whether a schedule on a paused or set-aside goal still fires, and whether a scheduled start beats a chain. Backend scheduled logic was not traced.
- The Needs you list itself and the Goals page; only the sidebar badge and rows were reviewed. Whether a failed or held wave raises a separate alert in that list was not traced.
- Goal-page data states such as offline or a failed request; 200% text or zoom; motion; real long goals with many waves.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
