## Verdict

macOS desktop app (GPUI), pointer and keyboard, reviewed from code plus 8 captures at 900x560, 1360x860, 1568x1250 and 1728x1084, in dark and light. Nothing was driven with real input, because this is a native app and the browser driver can't reach it. First impression: the page reads as organised. Judged by eye (the blur audit wasn't run), the week, four KPIs, one primary button and the stacked bar come through in the right order, and the chrome stays quiet. Counts: 0 Catastrophe, 2 Major, 11 Minor, 3 Cosmetic. The top problem is that the page answers only half of "how the work went and what shipped". It reports time and nothing about output, and it colours less agent time green, so a week where nothing finished looks like a good one. The other Major is that no control on the page can be reached from the keyboard.

## Findings

### R1 · 3 Major · No control on the page can be reached by keyboard
- **Element:** Weekly review › all controls (‹, All weeks and its menu, ›, Day/Week, Plan a goal, fix rows, task rows, Commands tabs), all sizes and themes
- **Evidence:** Confirmed
- **Problem:** Every control is a clickable div with no tab stop, focus state or key handler, so week navigation and acting on fixes need a mouse. The only keys that work here are global ones such as Cmd+R, Cmd+Left and Esc. A search of taskboard-app/src found no tab_index, tab_stop or focus-style calls, and track_focus appears only on MainWindow and Settings. The cause is in the shared kit, so this is likely app-wide.
- **Principle:** Keyboard operable, focus visible, arrows within groups [WCAG 2.1.1] [WCAG 2.4.7] [MS-KBD]
- **Fix:** Give kit::btn, kit::seg items, kit::menu_item and clickable rows a tab index and Enter/Space activation, with a 2px accent focus ring at 3:1 or better. Make each seg and the week menu one tab stop with arrow keys inside, opening the menu on the selected week. Add `[` and `]` for earlier and later week, shown in the ‹ › tooltips.

### R2 · 3 Major · The week shows time spent but nothing about what got done
- **Element:** Weekly review › KPIs and deltas (review-dark-m.png, review-light-m.png)
- **Evidence:** Confirmed
- **Problem:** The owner opens this page to see how the work went and what shipped, but all four KPIs and every card measure time or failures. The review data has no tasks finished, goals completed or releases. Delta code treats less as better for every number, so Agent time '↓ 1.0 h less' is green even if output fell. An idle week would look like the best week, which is a wrong reading on the page's main job.
- **Principle:** Walkthrough question 4, progress toward the goal [NNG-CW]; visibility of system status [NNG-H1]; match to the real world [NNG-H2]
- **Fix:** Add a read-only 'Shipped' KPI and card first: tasks finished, goals completed and releases in the period, each with its change. Tasks open in the right panel, goals go to their page, and releases link to the Releases page. This needs new fields in the review data. Make the Agent time delta neutral (t.muted, arrow kept) and keep green and red for Waiting on you, Timeouts and Failed, then rerun.

### R3 · 2 Minor · Changing week blanks the page, and a failed load leaves no week controls
- **Element:** Weekly review › header › ‹, All weeks, › and Week, during load or error
- **Evidence:** Confirmed
- **Problem:** pick() clears the review, so each step renders an empty title, 'Loading…' and only Day/Week. ‹ and › vanish under the pointer, so stepping back several weeks can miss, and the scroll position is lost with the content. If the load fails, the error shows with no week controls and no retry. Clicking Week on this page does nothing, so the way out is Day then Week, or Cmd+R. Latency wasn't measured, so how long the blank lasts is unknown.
- **Principle:** User control and freedom [NNG-H3]; error recovery [NNG-H9]; system status [NNG-H1]
- **Fix:** Keep the previous review on screen, dimmed to 60%, until the new one lands. Always render the nav from the list, with the title from the list entry's period. On failure, keep the nav and show the error with a Try again button that calls refresh. Make Week on the Review page return to the newest review when an older one is open.

### R4 · 2 Minor · Narrow windows orphan a KPI and strand the rail
- **Element:** Weekly review › KPIs and rail at 900x560 (review-dark-s.png), and at windows below about 1080 wide
- **Evidence:** Confirmed
- **Problem:** Four cards at min width 170 need 716px but about 597px is available, so 'Failed, then rerun' takes a full-width row for one digit. That card pushes Suggested fixes down, and only two fix rows fit the fold at the minimum window. From code, the 340px rail also wraps below the left column and stays 340 wide, leaving about 257px empty beside it. An open right panel probably triggers both at larger windows.
- **Principle:** Narrowing keeps the order [SCR-6]; the work dominates [SCR-1]
- **Fix:** Set the KPI min width to 130 so four fit at 597px (4 x 130 + 36 = 556), or use a fixed 2x2 grid below that width. Drop the delta row's reserved height when the row wraps. When the rail wraps, set it to w_full, or lay its two cards side by side.

### R5 · 2 Minor · Category colours change meaning between weeks
- **Element:** Weekly review › Where the time went (review-dark-m.png against review-older-dark-m.png)
- **Evidence:** Confirmed
- **Problem:** category_color uses rank in the week, not the category. Load wait is teal in Oct 22 – Oct 29 and grey in Oct 29 – Nov 5, where Git takes teal. Stepping with ‹ › to compare weeks, the owner tracks a colour that now means something else. Every category past the fifth is the same grey, so those segments can't be told apart.
- **Principle:** Consistency and standards [NNG-H4]; recognition rather than recall [NNG-H6]
- **Fix:** Map colour by category name (build, test, edit, read, git, load wait and so on) from a fixed table, shared with any other page that shows categories. Use grey only for unknown names, and merge the tail into one 'Other' row past six.

### R6 · 2 Minor · Plan a goal overwrites Backlog picks and its label hides what it does
- **Element:** Weekly review › Suggested fixes › Plan a goal
- **Evidence:** Confirmed
- **Problem:** plan() assigns m.backlog.picked = refs and resets the project and kind filters, so any issues already checked on the Backlog are silently dropped. The button only goes to the Backlog with fixes checked, which only the hover tooltip says. The pill says 3 while the button acts on 2, because B4 is already in G1, and nothing visible explains the gap.
- **Principle:** Unsaved work survives in-app actions [NNG-H3] [NNG-H5]; walkthrough question 3 [NNG-CW]
- **Fix:** Merge the refs into the existing picks without duplicates. Label the button with its effect and count, for example 'Plan 2 fixes'. Count only open, untriaged fixes in the pill, or show '2 of 3 open'.

### R7 · 2 Minor · Timeouts KPI and Timed out tab show different numbers
- **Element:** Weekly review › KPIs › Timeouts and Commands › Timed out (review-dark-m.png, review-800.png)
- **Evidence:** Confirmed
- **Problem:** The KPI says 2 and the tab says 'Timed out 1', because one counts events and the other counts distinct commands. Neither says so, and the two sit a full screen apart, so they read as a contradiction. Rerun happens to match, which makes the mismatch more surprising.
- **Principle:** One fact, one place [SEC-7]; consistency [NNG-H4]
- **Fix:** Make the tab count the same unit as the KPI by summing each command's count, so it reads 'Timed out 2'. The row's '2 timeouts' detail then explains it.

### R8 · 2 Minor · A failed filing leaves suggestions with no next step
- **Element:** Weekly review › Suggested fixes, filing failed (code only)
- **Evidence:** Confirmed
- **Problem:** The failure shows in the same muted 13px text as a normal note, so it is easy to miss. With nothing filed there are no fix rows and no Plan a goal, so the suggestions can only be read. The card that should be acted on becomes a dead end, with no retry or next step. 'Filing these…' while running also has no indicator beyond its text.
- **Principle:** Help users recover from errors [NNG-H9]; system status [NNG-H1]
- **Fix:** Show the failure in t.warn with a warning mark, and say what to do next in words. Add a one-click 'File again' signal. That is a new write action, so it needs the owner's OK. Without it, name the `tb` command that retries.

### R9 · 2 Minor · 'Weekly reviews are off' gives no way forward
- **Element:** Weekly review › first review callout, reviews off (code only)
- **Evidence:** Confirmed
- **Problem:** The state says reviews are off but not why, or where to turn them on. A search of the app's source found no Weekly review setting outside Home, so the owner has nowhere obvious to go. In the same function, a load error is shown only when no time is tracked, so with tracked time the error is hidden.
- **Principle:** Empty states have a way forward [mobile:NNG-EMPTY]; help in context [NNG-H10]; system status [NNG-H1]
- **Fix:** Add one muted line naming the exact setting or `tb` command that enables reviews. An in-app toggle would be a new write, so it needs the owner's OK. Show errs.review whether or not time is tracked.

### R10 · 2 Minor · Future days in the first-review strip fail text contrast
- **Element:** Weekly review › first review callout › day strip, future days (code only), both themes
- **Evidence:** Confirmed
- **Problem:** Future days use t.faint on t.card at 10.5px and 14px. That is about 3.0:1 in light (#8a95a3 on #ffffff) and 3.8:1 in dark (#6f7a88 on #1a1f26), so the dates leading up to the review are hard to read. Ratios were worked out by hand from the theme values.
- **Principle:** Contrast minimum 4.5:1 [WCAG 1.4.3]
- **Fix:** Use t.muted for future days (#5f6b7a in light, #a1abb8 in dark), and keep the past/future difference through the fill and border already in place.

### R11 · 2 Minor · Selected segment is nearly invisible in dark mode
- **Element:** Weekly review › header › Day/Week and Commands tabs, dark (review-dark-m.png, review-800.png)
- **Evidence:** Confirmed
- **Problem:** The selected item's fill (#1a1f26) on the track (#232931) is about 1.13:1, and weight stays medium, so selection rests almost entirely on a text colour change. The tab that decides what the Commands list shows is easy to misread. Light mode is clearer because white and a shadow do the work.
- **Principle:** Non-text contrast and use of colour [WCAG 1.4.11] [WCAG 1.4.1]
- **Fix:** In kit::seg, make the selected label semibold and add a 1px t.border_2 border to the selected item. Check that the border reaches 3:1 against the track in dark, and raise it if not.

### R12 · 2 Minor · Command row details are misleading or repeated
- **Element:** Weekly review › Commands › Slow repeated and Rerun rows (review-800.png; Rerun from code)
- **Evidence:** Confirmed
- **Problem:** 'median · p90 12 min · 14 runs   7 min' puts the word median beside the p90 value while its own value sits at the far right, so it reads as a label with no number. Rerun rows show the task ref twice, once as the mono ref and again in the detail ('T12 · 3 runs, then passed').
- **Principle:** Match the real world [NNG-H2]; one fact, one place [SEC-7]
- **Fix:** Change the detail to '14 runs · p90 12 min' and the right-hand value to 'median 7 min', or add a small column label. Remove the task ref from the rerun detail so it reads '3 runs, then passed', and update the test that asserts the old string.

### R13 · 2 Minor · Row bars use a different scale from the stacked bar
- **Element:** Weekly review › Waiting on you, By project, Slowest tasks (review-tall.png)
- **Evidence:** Confirmed
- **Problem:** shares() scales each bar to the longest row, so Question and webapp show full bars at 67% and 71% of their totals. Just beside them, Where the time went shows true share of the total. A full bar reads as 100%, so the size of the top item is overstated.
- **Principle:** Consistency and standards [NNG-H4]; clear hierarchy [NNG-VH]
- **Fix:** For Waiting on you and By project, divide by the section total instead of the max. Keep longest-row scaling only for Slowest tasks and Commands, whose rows aren't parts of one total.

### R14 · 1 Cosmetic · A lone Commands tab looks like a button that does nothing
- **Element:** Weekly review › Commands, one list present (review-older-dark-m.png)
- **Evidence:** Confirmed
- **Problem:** With one list, the segmented control has a single item, 'Rerun 2', which looks like a button but has no effect. The tabs are filtered by content, so this can happen with real data.
- **Principle:** One look per role [SEC-4]
- **Fix:** With one list, drop the control and show plain muted text in its place, for example 'Rerun · 2'.

### R15 · 1 Cosmetic · An unchanged KPI looks like missing data
- **Element:** Weekly review › KPIs › Failed, then rerun (review-dark-m.png)
- **Evidence:** Confirmed
- **Problem:** The other three cards show a change, and this one has a blank slot because the number didn't move. The owner can't tell 'no change' from 'not compared'.
- **Principle:** Visibility of system status [NNG-H1]
- **Fix:** When a comparison exists and the delta is zero, show muted 'no change' in that slot. Keep it blank on the first review.

### R16 · 1 Cosmetic · Adjacent weeks appear to share a day
- **Element:** Weekly review › header, 'Oct 29 – Nov 5  vs Oct 22 – Oct 29' (review-dark-m.png)
- **Evidence:** Likely
- **Problem:** Oct 29 appears in both ranges, so it isn't clear which week holds it. If period_end is an exclusive boundary, the shown end is a day past the data. The boundary time wasn't confirmed.
- **Principle:** Match the real world [NNG-H2]
- **Fix:** If the end is exclusive at midnight, show the last included day, 'Oct 29 – Nov 4'. If weeks cut at the review time, keep the dates and put the cut time in the ‹ › and menu context, with AM/PM.

## What works

- The first fold has a clear order, with the week, KPIs, then the one thing to act on, and one primary button per screen.
- All weeks is a pop-up with a virtualized list rather than tabs, as L2 asks, and Commands uses a segmented control only for three fixed lists.
- Deltas pair an arrow and words with colour, so they don't rely on colour alone, and the category deltas and percentages add up correctly in the sample.
- Empty sections disappear rather than showing filler, and content is capped at 1180px, so nothing stretches at 1728 wide. Tasks open in the right panel, as the one-home rule asks.

## Not checked

- Real input: nothing was driven. The page is native GPUI, which the browser driver can't reach, so behaviour comes from code and captures. That covers hover, drag-selecting text inside clickable rows, wheel scrolling and Esc on the menu.
- Measured checks: the inspect step and composition audit weren't run. Contrast was worked out by hand for the pairs named in findings, and the first impression comes from looking at the captures, not a blurred screenshot.
- States seen only in code, with no capture: All weeks menu open, Timed out and Rerun tabs with real rows, first review, reviews off, no time yet, filing running and filing failed, loading and error.
- Sample data: the blank rerun rows ('· 0 runs, never passed', 0 s) and 'Timeouts 3' with no Timed out tab in review-older-dark-m.png come from seed.rs (`reruns: [{}, {}]`, `commands: []`), so they aren't reported as findings. The daemon code that builds real reviews sets these fields. Missing Suggested fixes, Slowest tasks and By project on that week are also the seed's.
- Capture labels: review-tall.png is listed as dark but shows light, and is cut off at Commands. review-800.png and review-dark.png look identical, so the lower page wasn't seen in light beyond that one shot.
- Long content: commands run up to 200 characters. Whether they wrap or truncate, and whether the full text can be read, needs a run with long data. Week menu behaviour with more than 12 weeks and whether it scrolls to the selected week also weren't seen.
- Layout with the right panel open, at any window size, and window sizes between the captured ones. The 1080 and 1019 wrap points are calculated from a sidebar of about 247px measured from a capture.
- Screen reader output: no accessibility roles or names were found in taskboard-app/src, but the gpui_kit crate wasn't searched and VoiceOver wasn't run. The R1 search also covered only taskboard-app/src.
- macOS menu bar coverage of these commands, large text settings, motion, and real devices.
- Release checks: what is in each release and what the next cut waits for aren't on this page and weren't reviewed. R2 covers only the missing week-level link. Token and component reuse belong to theme-review.
- Process note: one read-only `wc -l` on review.rs ran in the shell, outside the brief's driver-only rule. It changed nothing and no finding depends on it.
- One reviewer finds only some of the problems. A second reviewer or a usability test will find more [NNG-HE].
