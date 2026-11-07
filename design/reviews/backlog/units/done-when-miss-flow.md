## Verdict

macOS desktop, pointer and keyboard. I reviewed Flow 11 (Done-when miss) from 6 dark captures (needs-miss.png, needs-miss-open.png, miss-row-click.png, miss-more.png at 1360x1400; goal-miss.png, goal-miss-menu.png at 1360x860) and from code, including the daemon code behind each choice. The app is native and no page was given, so nothing was driven with real input and no action was performed. The surfaces read as organised: blurred, Decide shows goal rows with misses nested under them, and the warning colour lands on "3 missed". The owner can act on a miss, but can't tell what it means or what each choice does to the goal. Counts: 0 Catastrophe, 4 Major, 6 Minor, 0 Cosmetic. The top problem is that the done-when itself never appears on this path, and no choice says what it does to the goal. Won't do, Create ticket and Finish the goal without it all release the goal; on the last hold, any of them finishes it with no warning. Won't do and Create ticket also sit in the same action set as Make it a task, which keeps the goal open.

## Findings

### R1 · 3 Major · The done-when that was missed is never shown
- **Element:** Needs you › Decide › goal row and miss rows; Issue panel; Goal page › header › Outcome not met (needs-miss-open.png, miss-row-click.png, goal-miss.png)
- **Evidence:** Confirmed
- **Problem:** The row says "3 missed" and the goal page says "Outcome not met", but neither says which outcome. The goal's outcome field is read only by goals_page.rs (as a fallback for tldr); goal.rs, the Issue panel and Needs you never show it. The panel doesn't name the goal either: it shows Backlog, the kind, the ref, the project and "Open · reported at 7:51 PM", so it reads like any backlog issue. A real filing does add a Blocks goal pill and its why (blocks_label), but that names no goal or outcome. The owner can't judge whether a miss matters without leaving for the Goals page.
- **Principle:** Match with the real world and recognition over recall: the decision needs the target and the miss side by side [NNG-H2] [NNG-H6]; walkthrough question 3 [NNG-CW].
- **Fix:** In the Issue panel, when misses_goal is set, add one line under the title: "Misses G3's done-when: <outcome>", with G3 linking to the goal page. On the goal page, show the outcome under the title and next to Outcome not met. Omit the outcome text when the goal has none, so no empty label appears. This adds data only, and the goal keeps its one home on its page.

### R2 · 3 Major · No choice says what it does to the goal
- **Element:** Needs you › miss row › Make it a task, Won't do, More › Finish the goal without it; Issue panel › action box (needs-miss-open.png, miss-more.png, miss-row-click.png)
- **Evidence:** Confirmed
- **Problem:** The four outcomes differ a lot, and only one label hints at it. Make it a task keeps the goal open until the task is done (open_misses). Won't do and Create ticket release the goal at once. Finish the goal without it also releases the goal but keeps the issue open in the backlog. Won't do and Finish the goal without it therefore look alike, and the difference between them is unstated. The panel's only guidance is "Not part of any task yet. Make it a task to queue it.", which is the generic backlog sentence and ignores the goal. The one goal-aware line in the code, "X can't finish until B5 is settled", appears only in an opened row's body and in none of the captures.
- **Principle:** Walkthrough questions 3 and 4 [NNG-CW]; status and error prevention before a consequential action [NNG-H1] [NNG-H5].
- **Fix:** For a miss, replace the panel sentence with one built from data: "Holds G3 open, 2 other misses. A task keeps it open until done. Won't do or a ticket lets it go." Add the same effect as tooltips on the row buttons: Won't do "Closes the issue. G3 stops waiting for it"; Finish the goal without it "Keeps the issue in the backlog. G3 stops waiting for it". This replaces an existing line and carries real refs and counts, so it respects the no-filler rule.

### R3 · 3 Major · Settling the last hold finishes the goal with no warning or undo
- **Element:** Needs you › miss row › Won't do; More › Create ticket, Finish the goal without it; Issue panel › Won't do, Create ticket (needs-miss-open.png, miss-row-click.png; board.rs miss_settled, view.rs run_do)
- **Evidence:** Confirmed
- **Problem:** When the settled miss is the last thing open, miss_settled runs on_goal_finished straight away: the goal.finished hook, the chain, quality and retro steps, and ran_at is cleared. goal-miss.png shows this goal's When done as Terminals close. Won't do is one click with no confirm, a few points from Make it a task on the row. The toast says only "Closed as won't do", so the owner isn't told the goal just finished with its outcome reported as not met. Reopening the issue afterwards can't take back the hook or chain. I rated this 3, not 4, because it needs the last hold and the issue itself is recoverable.
- **Principle:** Destructive actions are separated and can be undone or confirmed [NNG-H3] [NNG-H5]; system status [NNG-H1].
- **Fix:** Only when the click would finish the goal, confirm with the existing Modal: title "Finish G3 now?", body "This is the last thing holding it. Its outcome stays unmet." Otherwise keep it one click, and use the existing toast_undo with backlog/{ref}/reopen, as ideas do. When the goal did finish, make the toast say so: "Closed as won't do. G3 finished." Note for the owner: the confirm adds a step to a one-click signal in this one case. If that breaks the one-click rule, use a delayed finish with Undo in the toast instead.

### R4 · 3 Major · After a choice, nothing says what comes next
- **Element:** Needs you › toast after Make it a task; Issue panel after Make it a task; Goal page › Outcome not met menu (view.rs settle_issue, issue_panel.rs promote, goal.rs misses_menu, goal-miss-menu.png)
- **Evidence:** Confirmed
- **Problem:** From Needs you the row leaves and the toast reads "Made into a task", with no task ref, no link and no word that the goal still waits. In the panel, promote returns no note off the Board page, so the only feedback is the state line changing. The goal page keeps saying "Outcome not met · 3 misses" while the Backlog drops to fewer open, so the two disagree with no explanation. The misses menu renders only ref and title, so a miss already being worked looks the same as one still undecided, and each row opens the issue rather than its task. As it stands, the menu only repeats the Backlog list below it.
- **Principle:** Walkthrough question 4 [NNG-CW]; visibility of status [NNG-H1]; one fact, one place [SEC-7].
- **Fix:** Toast: "Made into T9. G3 stays open until it's done", with T9 opening the right panel. Give the panel the same note off the Board (remove the page check in promote's result). In misses_menu, add a right-aligned state per row: "Undecided", or "T9 · working" from state and task_ref, and open the task's panel for tasked rows. Split the header count when both exist: "Outcome not met · 1 undecided, 2 in tasks".

### R5 · 2 Minor · Finish the goal without it is missing from the panel
- **Element:** Issue panel › action box, reached from Goal page › Outcome not met menu or Backlog (miss-row-click.png, goal-miss-menu.png; issue_panel.rs issue_actions)
- **Evidence:** Confirmed
- **Problem:** issue_actions gives the panel Make it a task, Create ticket and Won't do. Finish the goal without it exists only in a Needs you row's More menu. An owner coming from the goal page who wants to ship and keep the issue has no such choice, and Won't do is the nearest button, which closes the issue instead. A miss put off with Later is also out of the Needs you list, so that route is gone until it returns.
- **Principle:** Consistency across routes to the same object [NNG-H4]; nothing frequent hidden [SEC-8].
- **Fix:** When misses_goal is set, add a ghost "Finish the goal without it" after Won't do in the panel, calling backlog/{ref}/without-goal. It is the same one-click signal, so the read-only rule holds.

### R6 · 2 Minor · Make it a task queues even when the goal isn't running
- **Element:** Needs you › Make it a task; Issue panel › Make it a task (view.rs Do::MakeTask, issue_panel.rs panel_head, api.rs promote)
- **Evidence:** Confirmed
- **Problem:** Both routes post where "board". That skips joining_status, which would make the task planned when the goal isn't running, so the task is always created queued, while still attached to the goal. A miss on a goal that has stopped running can therefore queue work the owner didn't start, and nothing on screen says so. The panel's "queue it" is the only hint. Whether a paused goal holds that task wasn't checked.
- **Principle:** No surprises and user control [NNG-H3]; status [NNG-H1].
- **Fix:** Send where "goal" whenever the issue has a goal_id, so the task joins as planned or queued by the goal's state, and report the result: "Made into T9, queued: G3 is running" or "Made into T9, planned: G3 isn't running". The API already returns this as why.

### R7 · 2 Minor · Backing out of a miss decision is weak
- **Element:** Needs you › More › Finish the goal without it toast; Backlog › reopen (api.rs finish_goal_without, reopen_issue; view.rs line 844)
- **Evidence:** Confirmed
- **Problem:** Finish the goal without it clears misses_goal, and I found no app route that restores it, so a wrong pick can't be undone. Its toast, "The goal finishes without it", also reads as if the goal is finishing even when other misses or tasks keep it open. Reopening a Won't do or ticketed miss sets it open and it holds the goal again, but reopen_issue doesn't raise the alert again, so the goal is held with no row in Needs you.
- **Principle:** User control and freedom [NNG-H3]; truthful status [NNG-H1].
- **Fix:** Change the toast to "G3 won't wait for B5", with Undo via a new route that restores misses_goal, blocks and blocks_why. In reopen_issue, call blocks::miss_alert when misses_goal is 1 and the issue has a goal.

### R8 · 2 Minor · One idea goes by six names
- **Element:** Needs you › "3 missed", "Holds the goal"; Goal page › "Outcome not met · 3 misses", "Misses goal"; Issue panel › "Blocks goal"; alert text "misses its done-when" (needs-miss.png, goal-miss.png; goal_misses.excerpt.rs, issue_panel.rs)
- **Evidence:** Confirmed
- **Problem:** The same state uses missed, misses goal, outcome not met, done-when, holds and blocks. "3 missed" alone doesn't say what was missed, and "Misses goal" sounds like the issue failed to reach a goal. The owner has to learn that these are one thing, and the panel pill won't match the pill on the goal page that led there.
- **Principle:** Consistency and standards [NNG-H4]; consistent labels [SEC-6].
- **Fix:** Use one term throughout, for example: row tag "3 miss done-when" or "Done-when: 3 missed"; goal header "Done-when not met · 3 misses"; Backlog and panel pill "Misses done-when". Keep "Holds the goal", since it describes the effect rather than renaming the state.

### R9 · 2 Minor · Miss rows give too little to decide from
- **Element:** Needs you › Decide › expanded goal row › miss rows (needs-miss-open.png, miss-row-click.png)
- **Evidence:** Confirmed
- **Problem:** Each row shows a title and four buttons, with no ref, kind or why, although the panel has them. The title is the way into the panel, but it reads as plain text until hover, when an underline appears. Opening the panel then covers the row's buttons and the panel offers a different set. The folded goal row's More has only Pause and Set aside, so three misses take three separate decisions, and it shows no G ref. A route to the goal page from that row isn't visible in the captures, and G3 isn't in the sidebar's Active goals.
- **Principle:** Recognition over recall and efficiency for frequent use [NNG-H6] [NNG-H7]; walkthrough question 2 [NNG-CW].
- **Fix:** Show the ref before the title in the muted mono style the goal page uses, and show blocks_why as a second line when it differs from the title. Give the title a resting underline or a chevron. Add G3 as a link on the folded row, and put "Make all tasks" and "Finish without all" in its More menu, each as one signal per miss. Keep the Later and More order.

### R10 · 2 Minor · Outcome not met looks like status and likely has no keyboard route
- **Element:** Goal page › header › Outcome not met; misses menu rows; Backlog rows (goal-miss.png, goal-miss-menu.png; goal_misses.excerpt.rs)
- **Evidence:** Likely
- **Problem:** The only way into the menu is bold warning text that shows no affordance until hover, and it has no chevron, unlike the app's other menus. The excerpts show these as plain divs with on_click: no focus handle, tab stop, key handler or expanded state. If the app has no shared handling for them, the goal-page route to a miss can't be used from the keyboard. No shared handling was seen, but not all of the app was read.
- **Principle:** Keyboard operable [WCAG 2.1.1]; role and state [WCAG 4.1.2]; looks like what it is [NNG-H4].
- **Fix:** Make it a real menu button with the same small caret as Later and More. Add a focus handle so Enter or Space opens it, arrows move through rows, Enter opens the issue, Esc closes it and focus returns to the button.

## What works

- The goal row sits above its misses, indented, with the count on the parent. This follows LESSONS L1, and the group count of 6 counts items correctly.
- "Holds the goal" states the effect in three quiet words, and the warning colour stays on the count, so each row has one emphasis.
- The Issue panel opens in the right panel beside the list, so the expanded group stays in view and the one-home rule holds.
- Misses sort first in the goal's Backlog, and the Outcome not met link leads straight to each one with full titles.
- Client and daemon agree on what holds a goal: open_misses counts a miss made into a task until that task is done, not failed.

## Not checked

- Real input: this is a native app and no page or URL was given, so the driver wasn't used. No control was clicked, typed or tabbed, and nothing was measured with inspect.
- Post-action states: no capture shows a toast, a settled row, a tasked miss in the menu, a finished goal or a failed action. These are judged from code.
- Create ticket: Jira is off in the captures, so that path is from code only.
- Whether a paused goal holds a task queued by Make it a task, and what the chain, quality and retro steps do after a goal finishes.
- Whether the row's "can't finish until" line can be reached on a nested miss row: no capture shows it and I didn't trace the row's open behaviour.
- Real-filing panel: the sample's misses have no blocks value, so the Blocks goal pill and its why aren't shown. They are read from code. The sample's goals also have no outcome text.
- Sample limits on the goal page: no tasks (per the brief), so a real goal's layout and the finish rule with tasks present weren't seen.
- Whether the misses popover stays on screen when clicked at the far right of the label: its position is the click point offset by a fixed amount, and kit::popover wasn't read.
- Contrast, target sizes, light theme, other window sizes, minimum window, 200% text and page zoom: only dark captures at 1360 wide were available.
- VoiceOver and other screen reader output.
- Volume: no 'vol' capture is in this unit, so many misses on one goal and the AI grouping state weren't seen. The AI grouping state isn't on this path in the code I read.
- Needs you as a whole: left to its own reviewer, per the brief.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
