## Verdict

macOS desktop app (GPUI), pointer and keyboard. Reviewed at 1360×860 in dark and light, and at 900×560 in dark. Evidence is the panel's code excerpt plus 8 captures. The native app could not be driven, so no finding comes from real input, and contrast was sampled from the captures. First impression: the panel reads as organised. The title, the blue action box and three quiet sections fall in a clear order, and the primary button is the one bright spot, so composition is not the main problem. The main problem is fit with the job. The panel's one prominent action, Make it a task, sidesteps the goal flow the owner is there to do, and the panel offers no way to pick the issue for a goal. Moving through a 121-issue backlog costs extra clicks, because nothing steps to the next issue and the open row isn't marked. Counts: 0 Catastrophe, 4 Major, 9 Minor, 1 Cosmetic. Top problem: R1, no route from the panel to a goal.

## Findings

### R1 · 3 Major · The panel has no route to a goal
- **Element:** Issue panel › action box, all open issues, 1360×860 and 900×560 (issue-b1.png, small-issue.png)
- **Evidence:** Confirmed
- **Problem:** The owner opens an issue from 'Plan from the backlog' to decide whether it belongs in a goal. The panel offers only Make it a task, Create ticket (with Jira) and Won't do. It can't tick the issue for the plan, and it doesn't show whether the issue is already in a goal. So the one bright button sends issues into single tasks outside any goal, and the owner must close the panel and find the row's checkbox. The file header promises 'move to a goal', but neither `panel_sheet` nor `detail_sections` draws it.
- **Principle:** Cognitive walkthrough: will users notice the correct action is available? [NNG-CW]; recognition rather than recall [NNG-H6]; frequent in view [SEC-8]
- **Fix:** Add a 'Select for goal' button to the action row that mirrors the row's checkbox and reads 'Selected' once ticked. On the Plan page, make it the primary button and drop Make it a task to the plain style. Show the goal ref in the subline when `goal_id` is set. Selection is local, so this keeps the read-only rule. A direct move to a goal through the picker would be a write: it needs the owner's approval as a one-click signal unless it already is one.

### R2 · 3 Major · Moving from issue to issue is slow, and the open row isn't marked
- **Element:** Issue panel › scrim and the page behind it, 1360×860 (issue-b2.png, issue-history.png)
- **Evidence:** Likely
- **Problem:** The undimmed page looks live, but the scrim closes the panel on mouse-down, so a click on another row or a sidebar item probably only closes it. Reading three issues then takes about six clicks, and the panel has no previous or next control. The open issue's row looks the same as the others: in issue-b2.png the B2 row is identical to B3, so the owner loses their place in a list of 121. How `kit::scrim` handles the click isn't in the excerpt, which is why this is Likely.
- **Principle:** Flexibility and efficiency of use [NNG-H7]; visibility of system status: selected row marked [NNG-H1]; selection is kept and shown in each pane [tablet:AND-CANON]
- **Fix:** Let a click on a list row swap the panel to that issue, and close the panel only on a click on anything else. This narrows the stated 'a click outside closes it' for rows, so the owner must accept that change. Add ↑ and ↓ to step through the visible list order. Give the open row the selected fill while its panel is open, and scroll it into view as the owner steps.

### R3 · 3 Major · Panel controls show no keyboard path
- **Element:** Issue panel › close button, Won't do, links; panel root, all states
- **Evidence:** Confirmed
- **Problem:** `close_btn` and the ghost Won't do button are plain divs with only a click handler. They have no focus handle, tab stop, key handler or focus style. The panel root has no `track_focus` or key context, so focus doesn't move in when it opens and can't return to the row when it closes. A keyboard user can't reach the panel's close, triage or log controls from this code. Esc may be handled globally, and `kit::btn` and `kit::link` aren't in the excerpt, so those two need checking.
- **Principle:** Keyboard [WCAG 2.1.1]; focus order, including focus moving into and back out of overlays [WCAG 2.4.3]; focus visible [WCAG 2.4.7]; Esc cancels transient UI [MS-KBD]
- **Fix:** Give the panel a focus handle and move focus to it on open. Make each button and link a tab stop with a 2px focus ring in the accent colour. Trap Tab inside the panel only if it stays modal. Handle Esc on the panel to close it, and return focus to the row that opened it. Enter and Space activate the focused control.

### R4 · 3 Major · Won't do closes an issue in one click, with no undo
- **Element:** Issue panel › action box › Won't do, open state (issue-b1.png, light-issue.png)
- **Evidence:** Confirmed
- **Problem:** Won't do sits 8pt from Make it a task and posts on the first click, with no undo. Afterwards `open_box` is gone, so the panel offers nothing to reverse it, and the 'Closed as won't do' note disappears after 5 seconds. While Make it a task shows 'Sending…', Won't do stays live because `busy` is keyed per action, so both can fire on one issue. Rated 3 rather than 4 because the issue is closed, not deleted; whether it can be reopened elsewhere is not known from this unit.
- **Principle:** User control and freedom [NNG-H3]; error prevention [NNG-H5]; destructive actions separated from safe ones (checklist pass 3)
- **Fix:** Keep the single click, as the owner's one-click rule requires, and don't add a confirm dialog. Put an 'Undo' link in the note and keep it until the next action or until the panel closes. This needs a reopen signal, which is a new write the owner must approve. Move Won't do to the far right of the row with `justify_between`, and disable every action on an issue while any one of them is busy.

### R5 · 2 Minor · Action results are quiet, differ by page, and can be missed
- **Element:** Issue panel › action box and notes, after Make it a task, Create ticket or Won't do (code only)
- **Evidence:** Confirmed
- **Problem:** On the Board, Make it a task opens the new task. On the Backlog page the same button shows no note, because `promote` returns a note only when the page is Board. The box just disappears and a muted 13px line changes. Success notes last 5 seconds and error notes 15. If the owner clicks outside to move on, as fast triage invites, a failed call shows nothing, because the note is drawn only inside this panel.
- **Principle:** Visibility of system status [NNG-H1]; consistency [NNG-H4]; errors must be recognised and recoverable [NNG-H9]; status messages [WCAG 4.1.3]
- **Fix:** Always show 'Made into task T12' with an 'Open it' link, on every page. Keep error notes until dismissed. When the panel has closed, show a failed action on the issue's row or in an app-level toast with the issue ref.

### R6 · 2 Minor · The panel hides the row data used to judge an issue
- **Element:** Issue panel over the Plan list: row ends at 1360×860 (issue-b1.png, issue-history.png); list at 900×560 (small-issue.png)
- **Evidence:** Confirmed
- **Problem:** At 1360 the fixed 480pt panel cuts off every row's priority bars, and the panel itself doesn't show priority, although waves follow priority. At 900 wide it covers about 74% of the main pane, leaving about 173pt: titles are gone and the heading reads 'Plan from the'. The owner can't compare the open issue with its neighbours at the smallest window.
- **Principle:** The work stays the biggest thing [NNG-H8]; frequent in view [SEC-8]; narrowing keeps the order [SCR-6]
- **Fix:** Show priority as a pill in the panel header, in the slot freed by removing the Backlog pill (R7). Below about 1100pt of window width, set the panel to `min(480, 44% of the window)` so about 400pt at 900 wide, which leaves about 250pt of list. Alternatively, collapse the sidebar to icons while the panel is open. Keep the priority bars ahead of the title at narrow widths, or let the row shrink to the panel's left edge.

### R7 · 2 Minor · Filler text and empty sections break the owner's rule
- **Element:** Issue panel › Backlog pill, box sentence, empty sections (issue-b2.png, issue-b4.png)
- **Evidence:** Confirmed
- **Problem:** 'Make it a task to queue it.' repeats the button beside it. The Backlog pill labels every issue the same and looks like the page's clickable kind chips. In issue-b2.png, two of the three sections hold only 'No snapshot was saved with this issue.' and 'Nothing yet.'. These show no data and push real content down.
- **Principle:** Owner's rule: no filler labels or helper text that shows no data; everything earns its place [SCR-5]; aesthetic and minimalist design [NNG-H8]; one look per role [SEC-4]
- **Fix:** Remove the Backlog pill. Cut the box sentence, or keep only the fact 'Not in a task yet'. Omit What was happening and History entirely when they have no rows. A 'Loading…' row still shows while history is loading. The `parity/golden/backlog.json` text checks will need updating.

### R8 · 2 Minor · Older issues lose their time and read wrongly
- **Element:** Issue panel › subline, History, log link, issues older than today (issue-snapshot.png, issue-ticket.png, issue-history.png)
- **Evidence:** Confirmed
- **Problem:** The subline says 'reported at Nov 5', which is ungrammatical and has no time. The log link has the same format, so it would read 'Open T1's log at Nov 5'. History shows 'Nov 5' on all three rows, so the owner can't see the gaps between events. The time appears only in a hover tooltip, which the owner's 12-hour AM/PM rule doesn't reach in practice.
- **Principle:** Owner's rule: 12-hour times with AM/PM; match the real world [NNG-H2]; hover is never the only path [NNG-FITTS]
- **Fix:** Use 'reported Nov 5 at 6:48 PM' for older issues and 'reported at 6:48 PM' for today, and the same in the log link. In History, put a day label above each day's rows and show '6:48 PM' on each row, which follows L1 (label above members). Widen the 52px time column to about 64px so '12:48 PM' fits.

### R9 · 2 Minor · The panel's content shrinks to fit the title, so close moves
- **Element:** Issue panel › header and action box, short titles (issue-b1.png, issue-b2.png, issue-b4.png, small-issue.png)
- **Evidence:** Confirmed
- **Problem:** The first list item shrink-wraps to its widest content instead of filling the panel. The close button and action box end at a different place on each issue: the right gutter is 110pt on B4, 46pt on B1 and 24pt on long titles, a move of up to 86pt. Close is never in the same spot between issues, and the panel looks broken on short titles.
- **Principle:** Alignment [SCR-4]; consistency [NNG-H4]; frequent targets stay put [NNG-FITTS]
- **Fix:** Add `.w_full()` to the `panel_sheet` column, and to the list item wrapper if needed, so every issue fills the panel's 480pt less its 24pt padding on each side.

### R10 · 2 Minor · A long history scrolls away the close button and actions, and hides new events
- **Element:** Issue panel › list, issues with long history, worst at 900×560 (code only)
- **Evidence:** Confirmed
- **Problem:** The pills, close button and action box are item 0 of the history list, so they scroll out of view with it. History is oldest first and pages forward, so on a 450-event issue the newest events, the ones that matter for triage, sit at the far end after several fetches.
- **Principle:** User control and freedom [NNG-H3]; frequent in view, rare behind [SEC-8]; visibility of system status [NNG-H1]
- **Fix:** Move the top row (pills and close) out of the list into a fixed header above it. Keep paging and virtualization, but load and show the newest page first, with older pages fetched on scroll. This needs the history endpoint to page backwards, and `oldest first` in the tests changes.

### R11 · 2 Minor · Load failures look like loading or a complete list
- **Element:** Issue panel › body Loading state and history end (code only)
- **Evidence:** Confirmed
- **Problem:** If the issue fails to load or no longer exists, `render` has no error branch and shows 'Loading…' for good, although `render_aside` takes an error. If a later history page fails, `failed` stops fetching and drops the Loading row, so a partial history looks complete, with no retry.
- **Principle:** Visibility of system status [NNG-H1]; help users recover from errors [NNG-H9]; each pane has its own error state [tablet:AND-CANON]
- **Fix:** Show the load error in the panel in `t.down` with a Try again button, as the aside's `err` does. At the end of a failed history, show 'Couldn't load the rest of the history' with Try again, and clear `failed` on that click. Retrying is a read, so the read-only rule holds.

### R12 · 2 Minor · Following the task leaves the triage page with no way back
- **Element:** Issue panel › Open T1's log link, snapshot Task row, history refs (issue-b1.png, issue-snapshot.png, issue-history.png)
- **Evidence:** Confirmed
- **Problem:** 'Open T1's log' calls `m.go(Page::Board)`, so the owner leaves the Plan page and has no route back to the issue. Whether ticked rows survive is unknown. The task refs in the snapshot's Task row and in History are plain text. In issue-snapshot.png, T88 has no route at all, because the log link needs `found_by_task`.
- **Principle:** User control and freedom [NNG-H3]; recognition rather than recall [NNG-H6]; owner's rule: tasks open in the right panel
- **Fix:** Open the log in the right panel over the current page, without `m.go`, which keeps the one home for tasks. Make the Task row's ref a link to that task's panel. Add a 'Back to B1' control in the task panel when it was opened from an issue.

### R13 · 2 Minor · A ticketed issue names its Jira key but gives no link
- **Element:** Issue panel › subline, ticket state (code only; no capture shows it)
- **Evidence:** Confirmed
- **Problem:** The ticket state shows 'Jira PROJ-12' as plain text, so the owner has to copy the key and find the ticket by hand. The task state, by contrast, has an Open it link. The Board panel's 'Open it' and the aside's 'Open task T7' also name the same action differently.
- **Principle:** Consistency [NNG-H4]; flexibility and efficiency of use [NNG-H7]
- **Fix:** Make the key a link that opens the ticket in the browser, if the Jira base URL is available. Use 'Open task T7' in both the panel and the aside.

### R14 · 1 Cosmetic · Six type sizes in one panel
- **Element:** Issue panel, whole panel (code)
- **Evidence:** Confirmed
- **Problem:** The panel uses 12, 12.5, 13, 13.5, 14 and 20px text. The half-steps add no hierarchy a reader can see, and neighbouring text differs slightly for no reason.
- **Principle:** About three type sizes [SCR-3]
- **Fix:** Use 20 for the title, 14 for body text and 12 for pills, times and labels, and map 12.5, 13 and 13.5 onto those. Web parity text checks aren't affected, since they compare text, not sizes.

## What works

- Clear order: the title, one bright primary button and three quiet sections read correctly at a glance in both themes.
- Sampled text contrast is comfortable: about 7.4:1 or better in dark and 6:1 or better in light, including the muted labels, ref pill and link.
- History is virtualized and paged, as the owner's rule asks, and scrolling keeps its place as later events arrive.
- The panel fits at 900×560 with its action box and snapshot in view, and 'Sending…' stays until the board confirms, so buttons don't flash back.

## Not checked

- Real input: the panel is a native GPUI app, so I could not use it with trusted pointer or keyboard events. The browser only sampled capture pixels. Clicks, Esc, Tab order, wheel, text selection and scroll chaining are read from code or not verified.
- `inspect` and the composition audit: neither can run on a native window or a PNG, so composition was judged from the captures and code.
- Ticket state: issue-ticket.png is labelled as linked to Jira but shows an open issue with no key and no Create ticket button, so R13 rests on code alone.
- In a task state: no capture shows 'Made into task' or Open it.
- Won't do, Deferred, Blocks PR and Blocks goal states, Jira-on actions, Sending…, notes, error notes, the Loading body and a long history: none are captured.
- Scrim behaviour, global key handling, and `kit::btn` and `kit::link` focus support: these are outside the excerpt, so R2 and part of R3 need checking in the app.
- The shared pickers, left out of the excerpt.
- History cache reset between issues: it happens outside the excerpt (`open_issue`), so I could not tell whether one issue's events can appear under another.
- Contrast figures are approximate: the captures are 1568×992 rather than true 2x, so antialiasing lowers the sampled values. Hit sizes and positions come from code, or are converted from those pixels. History dot colours, the error note and 'Blocks' text weren't sampled.
- VoiceOver names, roles and announcements: the code shows no accessibility labels, but I can't tell what GPUI exposes without running it.
- Larger text, browserless zoom equivalents, window maximum size, and dark and light at 900×560 beyond the one dark capture.
- AI grouping in progress and the 'vol' board: no capture in this unit is named vol, and grouping belongs to the page behind the panel.
- The Backlog page's aside (`render_aside`): it's in the code but wasn't in scope and has no capture.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
