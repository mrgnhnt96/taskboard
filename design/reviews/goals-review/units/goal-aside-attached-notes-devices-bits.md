## Verdict

macOS desktop, pointer and keyboard. I reviewed the Goal page's right column from six dark captures (1360x860 and 1360x1100) and the code: attached.rs, lists.rs, pool.rs, goal.rs, kit.rs and app.rs. I didn't run the native app, so nothing was used with real input. Blurred, the page reads as organised: the wave cards dominate and the aside is a calm single card with clear sections. The aside's problems are about priority and truth, not clutter. Counts: 0 Catastrophe, 1 Major, 10 Minor, 3 Cosmetic. The top problem is that nothing in the aside can be reached from the keyboard: the app has no tab stops at all, and file previews exist only on hover. The top design problem is Attached, where the Wave 2 picker sits above files that aren't in Wave 2, so the numbers on screen (4 of 7, All waves 5, Wave 2 2) never add up. L2 is met: waves use a pop-up menu. L1 is met for tasks, but the GOAL group's files aren't indented. No fix below needs an owner rule changed, except keeping the Jira control in R11.

## Findings

### R1 · 3 Major · No aside control can be reached or used from the keyboard
- **Element:** Goal page › aside › wave picker, kinds menu, file rows, task labels, Show/Hide, Focus, device holder links, issue rows, both menus; all sizes
- **Evidence:** Confirmed
- **Problem:** A search of taskboard-app/src finds no tab_index, tab_stop or .focusable(, so Tab can't land on any of these controls. The wave and kinds menus take no arrow keys; Esc closes them (app.rs escape) but they can only be opened with a click. Image previews and the file's name, date and summary appear only in a hover card, so those have no non-hover route.
- **Principle:** Every function works from the keyboard, with a visible focus state [WCAG 2.1.1] [WCAG 2.4.7]; arrows move inside groups and menus [MS-KBD]; hover is never the only path [desktop:HIG-POINTER]
- **Fix:** Give menu_btn, wave_btn, fold_toggle, file_row, the task label and issue_piece a FocusHandle with tab_stop(true) and a 2px t.accent focus ring, and make Enter and Space run their click. When a menu opens, move focus to its selected row; Up and Down move, Enter picks, Esc closes and returns focus to the button. Show the hover card on focus too. Add F6 to jump between the wave list and the aside.

### R2 · 2 Minor · Attached: the wave picker's scope and the counts disagree
- **Element:** Goal page › aside › Attached (goal-g2.png, g2-wave-menu.png, g2-att-kinds.png), 1360x860 dark
- **Evidence:** Confirmed
- **Problem:** The picker says Wave 2, but the first group under it is GOAL, whose files belong to no wave. The numbers then don't match: the head says 4 of 7 and the kinds menu says All kinds 4, while the wave menu says Wave 2 2 and All waves 5, because the menu leaves out the goal's 2 files. Picking All waves gives 7, not the 5 the row promised. GOAL also uses the same 11px bold capitals as the ATTACHED section head, so it reads as a peer section, and its files aren't indented under it while T8's are.
- **Principle:** One fact, one place [SEC-7]; visibility of system status [NNG-H1]; a group's label sits above its members and they indent under it [LESSONS L1]
- **Fix:** In attached_piece, put the GOAL group directly under the head and the wave picker below it, so the picker only heads what it filters. Show the wave's count at the picker's right edge, matching the menu row, so the card reads GOAL 2 + Wave 2 2 = 4 of 7 (or 2 + 5 = 7 on All waves). Draw the Goal label like a task label (12px medium, t.text_2) and indent its files with pl(12) as task_group does.

### R3 · 2 Minor · What needs the owner sits at the bottom of the aside, under static settings
- **Element:** Goal page › aside › section order (g2-devices.png, r-g13.png, g1-notes.png)
- **Evidence:** Confirmed
- **Problem:** The fixed order puts How it runs first and Bits and Backlog last, yet those two hold what stops a goal: Misses goal and Blocks goal pills, and Not created bits, which goal.rs says hold back a finished goal. In g2-devices.png, opening Devices pushes Bits to the fold at 860 tall, with its heading cut off. On r-g13.png the header says Outcome not met, and the two issues that explain it come after settings and Devices. This works against the primary task: see what needs attention.
- **Principle:** Frequent in view, rare behind [SEC-8]; the most important content draws the eye first [SCR-2] [NNG-VH]
- **Fix:** In lists.rs build(), order the aside by need: Backlog first when any open issue has misses_goal or blocks, and Bits next when any is Not created. Then Notes, Attached and Devices, with How it runs last. With nothing needing the owner, keep Backlog and Bits after Devices. Sort rows inside Bits the same way (R4).

### R4 · 2 Minor · Bits: the count doesn't match the list, and a Not created bit has no way forward
- **Element:** Goal page › aside › Bits (g1-notes.png), 1360x1100 dark
- **Evidence:** Confirmed
- **Problem:** The head says 1 of 2 created above three rows, because the count skips local bits and nothing says so. The one row that needs the owner, passkeys.settings Not created, comes last. It also shows no action: the link appears only when [bits] create_url is set, and otherwise the row gives no next step, even though this state can hold a finished goal open.
- **Principle:** Visibility of system status [NNG-H1]; help users recover [NNG-H9]; recognition rather than recall [NNG-H6]
- **Fix:** Change the head to name what's left: 1 not created, with no count when none are waiting. Sort bit_rows with warn first, then up, then muted. When create is None on a Not created bit, add a muted 12px line with the exact tb bit command that marks it made. This stays read-only.

### R5 · 2 Minor · Older notes fail silently, and filtered counts only cover what's fetched
- **Element:** Goal page › aside › Notes, goals with more than one page of notes (lists.rs load_older_notes, goal.rs notes_head)
- **Evidence:** Confirmed
- **Problem:** When an older page fails to load, Err(_) sets done = true: Loading… disappears and the list just ends, with no error and no retry, while the head still shows the full total. The owner can't tell that notes are missing. Paging stays stopped until the goal is opened again. While pages are unfetched, a filter's N of total and the set of kind tabs reflect only what has loaded, so 3 of 300 can be far too low.
- **Principle:** Visibility of system status [NNG-H1]; recognise and recover from errors [NNG-H9]
- **Fix:** Add failed: bool to NoteCache. On Err, keep the notes and replace the Loading… row with Couldn't load older notes, plus a Try again link that clears failed and refetches (the fetch itself is read-only). While more is true under a filter, show the count as 3 so far of 300.

### R6 · 2 Minor · Notes show no time
- **Element:** Goal page › aside › Notes › each note (g1-notes.png)
- **Evidence:** Confirmed
- **Problem:** A note's head gives kind, pinned and source, but not when it was written, although each note has an at value that note_order sorts by. Someone checking in through the day can't see what's new since their last look, or how old a decision is.
- **Principle:** Visibility of system status [NNG-H1]; recognition rather than recall [NNG-H6]
- **Fix:** Add the time to NoteItem and show it at the end of the note's head, in the format Attached uses (%b %-d, %-I:%M %p), for example Decision · pinned · you · Oct 17, 3:42 PM. Use t.muted at normal weight so the kind stays the label.

### R7 · 2 Minor · Backlog: stray divider above N more, no way back, and cut titles with no tooltip
- **Element:** Goal page › aside › Backlog, more than 5 open issues, or an issue with two pills (lists.rs line 350, goal.rs issue_piece)
- **Evidence:** Confirmed
- **Problem:** With more than 5 open issues, the 5th row is marked last, so it draws its bottom padding and divider. N more then sits alone below that line and reads as a separate section. Once expanded, nothing collapses the list until the owner leaves the goal. Titles truncate with no tooltip, and a row with both pills leaves the title only around 90px of the 292px row, so the issue can't be read without opening it. No capture shows these states; they come from the code.
- **Principle:** User control and freedom [NNG-H3]; grouped by space [SEC-2]; recognition rather than recall [NNG-H6]
- **Fix:** In lists.rs, set last to k + 1 == issues.len() only, because BacklogMore already closes the section. When backlog_view is on and more than 5 issues are open, add a Show fewer link in the same place. Add .tooltip(kit::tip(title)) to the issue row, and when a row has two pills put them on a second line under the title.

### R8 · 2 Minor · The same refs are links in some sections and plain text in others
- **Element:** Goal page › aside › Bits uses (T2, G1, T3), Notes source (Finding · T1), against the Devices holder and Attached task label (g1-notes.png, g2-devices.png)
- **Evidence:** Confirmed
- **Problem:** T2 opens the task from Devices and T8 opens it from Attached, but T2 in Bits and T1 in Notes are plain text inside a bold coloured status line, so the owner has to search for the task. The Devices holder is also a bare T2 with no word for its relation; only the tooltip gives the task's title.
- **Principle:** Consistency and standards [NNG-H4]; flexibility and efficiency of use [NNG-H7]
- **Fix:** Draw refs in Bits and Notes with kit::link in the mono 11px style Devices uses. T refs call m.open_task (the task's right panel) and G refs go to the goal page, which keeps one home per object. BitRow.uses needs to become a list of refs rather than a joined string, and the note's source needs the same split where it's a ref. In Devices, show In use by T2.

### R9 · 2 Minor · A wave's files, the Bits list and the Devices list aren't virtualized
- **Element:** Goal page › aside › Attached under a wave, Bits, Devices (lists.rs build)
- **Evidence:** Confirmed
- **Problem:** The aside list makes one item per wave, so every task group and file in a wave is drawn at once, including when the kinds or waves change. Bits and Devices are each a single item too. A long goal with many evidence files per wave breaks the owner's rule, and the list has to measure one very tall item.
- **Principle:** Owner rule: long lists are virtualized
- **Fix:** Replace AItem::Att(k) with items per task group or file: a head, a wave label, then one item per task label and per file row, keyed with the wave, the kind and the file's id so the choice still remeasures. Do the same for Bits and Devices rows. side_piece already handles the first and last slices.

### R10 · 2 Minor · Device state Off fails text contrast
- **Element:** Goal page › aside › Devices › state label, a device that's off (pool.rs line 140)
- **Evidence:** Confirmed
- **Problem:** Off is drawn at 12px in t.faint on t.card: #6f7a88 on #1a1f26 is 3.8:1 in dark, and #8a95a3 on #ffffff is 3.0:1 in light. Both are under 4.5:1. The label is information, not a disabled control, so the exemption doesn't apply. No capture shows it; the ratios come from theme.rs.
- **Principle:** Contrast minimum [WCAG 1.4.3]
- **Fix:** Use t.muted for Off: #a1abb8 is 7.1:1 in dark and #5f6b7a is 5.4:1 in light.

### R11 · 2 Minor · The Jira On/Off control in How it runs writes a goal setting
- **Element:** Goal page › aside › How it runs › Jira tickets, when Jira is on (goal.rs jira_row, set_jira_off)
- **Evidence:** Confirmed
- **Problem:** The rest of the section is read-only, but this segmented control POSTs jira_off to the goal. That isn't one of the listed signals (start, stop, answer, idea, human-task actions), and one click changes whether new tasks ask for tickets. It isn't in the captures, so its look wasn't reviewed.
- **Principle:** Owner rule: read-only apart from one-click signals; consistency and standards [NNG-H4]
- **Fix:** Show it as a plain row like the others (Jira tickets: On or Off) and set it through tb. Keeping the control needs the owner's OK.

### R12 · 1 Cosmetic · Two pop-up buttons with two looks, and Notes filters a third way
- **Element:** Goal page › aside › Attached kinds button and wave picker, Notes kind tabs (goal-g2.png, g1-notes.png)
- **Evidence:** Confirmed
- **Problem:** All kinds is a 12px borderless button. The wave picker right below it is a 13px semibold bordered box whose -6px margin pushes its border 6px past the section's edge. Notes narrows by kind with a segmented control and no counts, while Attached does the same job with a menu that has counts.
- **Principle:** One look per role [SEC-4]; alignment [SCR-4]
- **Fix:** Drop the -6px margin on wave_btn so its border lines up with the section edges. Use menu_btn for the Notes kind choice too, placed left of Hide, with the same counts as the Attached kinds menu. This also saves the tab row's height.

### R13 · 1 Cosmetic · Menus open from the click point, and the wave tooltip lands on the open menu
- **Element:** Goal page › aside › Attached › wave menu and kinds menu (g2-wave-menu.png, g2-att-kinds.png)
- **Evidence:** Confirmed
- **Problem:** Both menus sit at the click position minus 150px, so they land somewhere different each time, and a click at the picker's left end pushes the wave menu over the wave cards. The kinds menu covers the wave picker. A Wave 2 tooltip repeats the button's own label and floats over the open menu.
- **Principle:** Consistency and standards [NNG-H4]; aesthetic and minimalist design [NNG-H8]
- **Fix:** Anchor each menu to its button's bounds: wave menu left-aligned to the picker and as wide as it, kinds menu right-aligned to its button. Show the picker's tooltip only when the label is truncated, and never while its menu is open.

### R14 · 1 Cosmetic · How it runs: tautological wording and an unexplained in-use count
- **Element:** Goal page › aside › How it runs (g1-notes.png, r-g13.png, goal-g2.png)
- **Evidence:** Confirmed
- **Problem:** Order: In order says nothing. On G2 the section says 1 in use while the header says 2 working, because in_use counts only the goal's own tasks and T2 comes from another goal. Review stop After wave 3 also repeats the pill on the wave 3 card.
- **Principle:** Match with the real world [NNG-H2]; one fact, one place [SEC-7]
- **Fix:** Change In order to One at a time. Change the terminals value to 1 in use here when the goal has shared tasks working.

## What works

- L2 is met: waves use a pop-up menu that scrolls at 360px, with each wave's phase and count. Notes keeps a segmented control only for a fixed set of at most four kinds, so L2 isn't broken there.
- L1 is met for tasks: the ref and title sit above the files, which are indented, and the label opens the task in the right panel.
- Sections with nothing to show disappear rather than showing filler, and the aside stays quiet beside the wave cards.
- Attached opens on the wave in progress, and the wave and kind choices don't reset each other. Menu rows that hold nothing are dimmed and inert.
- Times in the Attached file details use the 12-hour format with AM/PM, as the owner's rule asks.

## Not checked

- Real input: this is a native GPUI app with no page for the browser driver to open, so no control was used with real pointer or keyboard input and inspect wasn't run. Findings come from the six captures and the code.
- The composition audit wasn't run. The squint judgement comes from looking at the captures.
- Sizes: only 1360x860 and 1360x1100. The minimum window, short windows, very wide windows and larger text weren't seen.
- Light theme: not captured. Light values were read from theme.rs only for R10.
- Text contrast in the captures wasn't measured beyond the Off label; only that pair was computed from the theme.
- States with no capture: All waves view, a kind filter in force, hover preview cards, Held, Failed and Your review wave phases, the Create in Flagsmith link, Jira row, more than 5 issues, Blocks pills, notes paging and its failure, a bit list or wave menu with many entries, loading and error states.
- 'Waits for you to press Start' appears in none of the six captures, so it isn't rated. The code shows it's a real server message (waitsfor.rs), not text made only by the seed.
- Scrolling feel, and whether the aside shows any scrollbar in use: lists.rs adds none.
- Screen reader output: I found no accessibility labels in the code, but didn't establish what GPUI exposes to VoiceOver.
- Shared tasks' files: goal_detail leaves them out of Attached, so T2's files aren't reachable from G2. I didn't rate this because the intended behaviour isn't stated.
- Motion and real devices. One reviewer finds only some of the problems; a second reviewer or a usability test will find more.
