## Verdict

macOS desktop, pointer and keyboard; a native GPUI app. Reviewed at 1360x860 and 1360x1100 in dark from 5 captures, plus code for the other window sizes, light theme, keyboard and failure states. The app could not be driven, so nothing was used with real input. First impression, judged by eye from the captures because the blur audit could not run: the page reads as organised and calm, with a clear list, header and cards, and the orange Needs you pills are the only warm spots. Its order does not serve the primary task, though. Goals sort by group, then newest id, so what needs the owner lands mid-list, and the project list is alphabetical. Counts: 0 Catastrophe, 5 Major, 10 Minor, 2 Cosmetic. Top problem (R1): nothing on the page ranks by attention, and the page opens on the first project by name even when another one needs the owner. R2 and R3 compound it: a Needs you pill gives no reason or action, and the project list can disagree with the goals it summarises.

## Findings

### R1 · 3 Major · Work that needs you is not first, in the goals or in the project list
- **Element:** Projects › Active card, Tasks card and project list, all sizes (projects.png, r-projects-open.png, r-projects.png)
- **Evidence:** Confirmed
- **Problem:** In projects.png, G2 Working sits above G1 Needs you. In r-projects-open.png, G13 Planned, G6 Queued and G5 Queued come before G4 and G3 Needs you, and G1 Needs you comes further down, because active() sorts by group and then newest id. The project list is sorted by name, so with no project picked the page opens on the first name: r-projects.png shows api, with nothing to do, while webapp holds the orange dot. Tasks sit after every goal, including set-aside ones, so a task that needs you can be far below the fold. With many projects the owner has to scan every row at every check.
- **Principle:** Primary task and visual hierarchy: what matters most comes first [SCR-2] [NNG-VH]; frequent need in view [SEC-8]; status visible [NNG-H1]. status.rs St::rank already defines this order and loose() already uses it.
- **Fix:** In active(), sort by (group_rank, pill.0.rank(), newest id), so within Active it runs Needs you, Failed, Working, In review, then the rest. In list(), sort open rows by needs first, then running, then name; selected() then opens on the project that needs attention with no other change. Render the Tasks card above Active when any loose row is St::Needs. Update the tests active_goals_list_started_first_with_their_status, the_list_puts_projects_without_open_goals_last and the_picked_project_falls_back_to_the_first.

### R2 · 3 Major · A Needs you pill does not say what is needed, and offers nothing to do about it
- **Element:** Projects › Active › goal row and its opened tasks, e.g. G3 Checkout on mobile and G4 Nightly export (r-projects-open.png)
- **Evidence:** Confirmed
- **Problem:** G3 says Needs you, but opening it shows Done, Done, Queued, Queued: no task needs you and nothing explains why. The reason is in the goal's stopped field, which pill() reads and never shows; the sidebar says 'stopped for you' and the Goals page has Continue. Here the owner has to find the small G3 ref and go to the goal page. A task's question appears only after opening the goal. The sidebar's Needs you count reads 1 while this page shows at least 3 Needs you pills, which may partly be the seed.
- **Principle:** Walkthrough Q2-Q4: the action must be noticed and tied to the result [NNG-CW]; recognition over recall [NNG-H6]; visibility of status [NNG-H1].
- **Fix:** Add a reason field to GoalLine and draw it as a 12px line under the name in the warn colour: the stopped text (e.g. 'Wave 1 is done. Review it, then continue'), else the first St::Needs task's row line. Reuse goals_page's Action::Continue button on stopped goals, which is a one-click signal the owner's rules allow. Keep the pill as it is.

### R3 · 3 Major · The project list's status can contradict the goals it summarises
- **Element:** Projects › project list › row (r-projects-api.png, r-projects-open.png)
- **Evidence:** Confirmed
- **Problem:** Three separate faults. (1) api reads '1 goal · queued' while its only goal says Working with a blue bar: running counts sessions, and queued wins when none is found, so the absence of a session may be a seed effect, but the row can show this whenever the two sources differ. (2) The dot uses g.needs only, while the goal pill also fires on stopped, so a project whose only attention item is a stopped goal shows no dot, even though its goal says Needs you. (3) Needs you is shown only by an 8px dot: sub() has no words for it, and warn against accent is a colour-only difference.
- **Principle:** Use of colour [WCAG 1.4.1]; one fact must not disagree with itself [SEC-7]; system status [NNG-H1].
- **Fix:** Build the row's status from the same facts as the pill: needs |= i(g,"needs") > 0 || opt_s(g,"stopped").is_some(); count working from the goals' active minus needs as well as sessions. Put the count first in sub(), in words and the warn colour: '12 goals · 3 need you · 1 running'. Keep the dot as a second signal.

### R4 · 3 Major · Goal and task rows lose their name and status at narrow widths and under the task panel
- **Element:** Projects › Active, Tasks and Finished rows at the 900x560 minimum window, and with the task panel open at 1360 wide
- **Evidence:** Confirmed
- **Problem:** Computed from code, not seen in a capture. At 900 wide the fixed columns (sidebar 248, list 280) leave a card of about 306px. A goal row's fixed parts need 388px, so the name column shrinks to 0 and the card's overflow_hidden cuts the pill to about 30px. A task row needs 448px, so its title goes to 0 and its chip is clipped away. Goal names vanish below about 982 wide and task titles below about 1042. Separately, the 480px task panel floats over the page's right edge, so at 1360 it covers the bars and pills from x=880, and scrolling can't reveal them. The list has no planned collapse.
- **Principle:** Layout reflows across the window range [HIG-MACOS]; the order holds as it narrows [SCR-6]; every feature reachable at every size [NNG-H6].
- **Fix:** Choose from the width left for the main pane, with the task panel's 480 subtracted when it is open. Under 640: hide the 150px bar and keep the 'n of m' text. Under 520: drop the 190px meta column and put meta under the title. Under 460: replace the 280px list with a project pop-up menu above the header, which follows L2 for a set that grows. The pill and name stay visible at every width.

### R5 · 3 Major · The page cannot be used from the keyboard
- **Element:** Projects › every control: project rows, Nothing open toggle, goal rows, refs, task links, Backlog, release and Show all links
- **Evidence:** Confirmed
- **Problem:** Every control is a div with on_click. projects.rs has no track_focus, key handler or focus style, and the app source has no tab_index or accessibility calls at all. A Mac keyboard user can reach the page with a shortcut, but can't pick a project, open a goal or open a task, and no focus is shown. Assistive tech gets no names or roles. Rated 3 rather than 4 because the owner also has a pointer and the gap is app-wide. Backlog already has page focus and on_key.
- **Principle:** Keyboard [WCAG 2.1.1]; focus visible [WCAG 2.4.7]; name, role, value [WCAG 4.1.2]; arrows move inside groups [MS-KBD].
- **Fix:** Follow backlog.rs: give the page a focus handle and on_key. Up and Down move through project rows and then goal and task rows, Left and Right fold or open a goal, Enter opens a task or goal page, and F6 or Tab moves between the list and the main pane. Draw a 2px accent ring on the current row. Screen reader roles depend on what gpui_kit can expose, so that part may need separate work.

### R6 · 2 Minor · 'Waits for you to press Start' has no Start, and reads as quiet Queued
- **Element:** Projects › Tasks › T5 Upgrade the HTTP client (projects.png); also T51 and T52 in r-projects-open.png
- **Evidence:** Confirmed
- **Problem:** T5 on the sample board tells the owner it is waiting for them, but the chip is grey Queued, it ranks near the bottom (St::Queued rank 6), adds no dot, and the row has no Start. The owner has to open the task panel to find it. Code: waits_for_start is true for manual pickup when no running goal holds the task, so T5 with no goal is real. T51 and T52 are in the extra seed and may be a seed effect, so they are not counted here.
- **Principle:** Walkthrough Q2: is the correct action visible [NNG-CW]; status honesty [NNG-H1]; recognition [NNG-H6].
- **Fix:** When the waiting line is the Start case, draw a small Start button in the row's meta column, posting tasks/{r}/start with mode new as home.rs Act::Start does. This is an allowed one-click signal. Have the daemon send a flag such as waits_for_start, rather than matching the text, and rank these rows with Needs in loose().

### R7 · 2 Minor · A project with only loose tasks looks idle and never shows its task count
- **Element:** Projects › project list › row for a project with 0 open goals and 1 or more open tasks
- **Evidence:** Confirmed
- **Problem:** list() puts it in the open group, but project_row sets idle = r.open == 0, so it draws the compact muted row with no subtitle. sub()'s '2 tasks' case is tested and never drawn. The project sits among open projects looking like a 'Nothing open' one, perhaps with 'N done' on it, so its open work is easy to miss. Proven in code; no capture shows this state.
- **Principle:** System status [NNG-H1]; same thing, same look [NNG-H4] [SEC-4].
- **Fix:** In project_row, use let idle = r.open == 0 && r.tasks == 0, matching the partition in list(). Add a render-level test for a tasks-only project.

### R8 · 2 Minor · A goal with every task done says Planned
- **Element:** Projects › Active › G13 Replay every error, '1 of 1', full green bar (r-projects-open.png)
- **Evidence:** Confirmed
- **Problem:** pill() has no case for all tasks done on an unfinished goal, so it falls through to Planned. The row says complete and not started at once, and the owner can't tell whether it needs closing or is about to run. R1's order would place Planned low, so the goal would also drop out of sight.
- **Principle:** Match status to reality [NNG-H1]; one fact, one place [SEC-7].
- **Fix:** In pill(), before the Planned fallback, add: if the total is above 0 and done equals total, return (St::Done, "All done"). Rank it with In review if it needs a human to close it. Add a case to a_goal_past_its_failed_tasks_says_in_review or a new test.

### R9 · 2 Minor · 'Active' holds goals that are not active, and pushes Tasks and Finished out of view
- **Element:** Projects › Active section heading and card (r-projects-open.png, 1360x1100)
- **Evidence:** Confirmed
- **Problem:** The card lists Planned, Paused and Set aside goals under 'Active', and has no count, while Tasks and Finished both have one. The sidebar's ACTIVE GOALS shows 8 while webapp alone reads 12 goals, so the same word means two things. With 12 goals and 2 opened, Tasks and Finished are below the fold even at 1100 high, and the set-aside rows sit between the owner and them.
- **Principle:** Consistent labels [SEC-6]; same thing, same name [NNG-H4]; rare behind [SEC-8] [NNG-PD]; group label above members (LESSONS L1).
- **Fix:** Rename the heading 'Open · 12'. Fold Paused and Set aside into one row at the end of the card, 'Paused or set aside · N' with a chevron, in the same pattern as 'Nothing open', closed by default. Only started goals and up-next goals show at first.

### R10 · 2 Minor · Goal page link is a hidden 17px target; task rows are mostly dead
- **Element:** Projects › Active and Finished › G ref; Tasks › task row
- **Evidence:** Confirmed
- **Problem:** The goal page, one of a goal's two homes, is reached here only through the G ref: 44x17px (12px text at 1.4 line height), muted, with no link look until hover, and inside a row that does something else when clicked. A near miss folds the row instead of navigating. The 17px height is under the 24px minimum and it overlaps the row target. Goal rows have no hover fill although they are clickable. Task rows are the reverse: only the ref and title respond, and the meta and chip do nothing.
- **Principle:** Target size [WCAG 2.5.8]; adjacent targets [NNG-FITTS]; looks like what it is [NNG-CLICK]; hover state [NNG-H1].
- **Fix:** Make the goal ref min_h(24) with items_center and show it in t.accent at rest, as kit::link does. Add .hover(bg panel_2) to goal and done rows. Move the task's on_click to the whole row with the same hover, keeping the ref and title link hover.

### R11 · 2 Minor · Faint text fails contrast
- **Element:** Projects › Finished › G ref; project list › 'N done'. Dark and light
- **Evidence:** Confirmed
- **Problem:** t.faint text is 12px. Dark #6f7a88 gives 3.80:1 on card #1a1f26, 4.31:1 on bg #0f1216 and 3.36:1 on a selected row #232932. Light #8a95a3 gives 3.04:1 on white and about 2.84:1 on #f6f7f9. All are under 4.5:1, and these are references the owner uses to tell goals apart. Computed from theme.rs; no capture shows them.
- **Principle:** Contrast [WCAG 1.4.3]
- **Fix:** Use t.muted for both: #a1abb8 gives 7.1:1 on the dark card, and #5f6b7a gives 5.4:1 on white and about 5.0:1 on #f6f7f9. Keep the finished rows quiet through text_2 on the names, as they are now.

### R12 · 2 Minor · Failed fetches look like loading forever, or like no tasks
- **Element:** Projects › opened goal body and Tasks section, when GET /goals/:id or GET /tasks fails
- **Evidence:** Confirmed
- **Problem:** app.rs keeps only Ok answers for goal details and loose tasks, and drops the Err. An opened goal then says 'Loading…' for good. Tasks simply doesn't appear while the list still says '1 task', and with no goals the pane is blank: 'No goals or tasks' needs loose to be Some. While the first answer is on its way, the Tasks section is also missing and then pops in, moving Finished.
- **Principle:** Status [NNG-H1]; help recover from errors [NNG-H9]; each pane has its own loading and error state [tablet:AND-CANON].
- **Fix:** Store the error by ref and for loose, as d.errs does for other fetches. Show 'Couldn't load this goal's tasks' or 'Couldn't load tasks' with a Try again link that calls m.refresh, in the same place as the rows. Show the Tasks heading with one muted 'Loading…' row while loose is None and the list's count is above 0.

### R13 · 2 Minor · Long lists on this page are not virtualized
- **Element:** Projects › Active, Tasks and Finished (Show all); project list
- **Evidence:** Confirmed
- **Problem:** Each list is built with a for loop into a plain div, so every row is laid out on every draw. Show all puts every finished goal in, Tasks has no limit, and opened goals add their tasks. Finished only grows. This breaks the owner's rule, and Backlog, Needs you and Releases already use virtual lists. Effect on speed was not measured.
- **Principle:** Owner rule: long lists virtualized. Responsiveness [mobile:NNG-RESPONSE].
- **Fix:** Flatten the main pane into a line enum (Head, SectionLabel, GoalRow, TaskRow, DoneRow, ShowAll) and render it through gpui_kit::list as backlog.rs does, drawing card edges per line. Same for the project list when it can pass about 50 rows.

### R14 · 2 Minor · Opened goals don't group tasks by wave, and wave wording is unclear
- **Element:** Projects › Active › opened goal › task rows, e.g. G2's T8-T11 (r-projects-open.png)
- **Evidence:** Confirmed
- **Problem:** Tasks are sorted by wave, but the wave is named only in the meta of some open rows: T9 says 'wave 2', T8 says 'for 1h 5m', T11 is blank and done rows say nothing. The owner can't see where one wave ends, while the goal row says 'wave 2'. T10's 'Waits for wave 2 to start T9' is also hard to parse.
- **Principle:** Group label above members, members indented (LESSONS L1); proximity [NNG-PROX]; plain words [NNG-H2].
- **Fix:** When a goal has more than one wave, draw a small 'Wave 2' label row above each wave's tasks in goal_tasks and drop the 'wave N' case from task_meta. Reword the daemon line in waitsfor.rs, e.g. 'Waits for T9 to start'.

### R15 · 2 Minor · The no-projects state is a dead end
- **Element:** Projects › no projects (r-empty-projects.png)
- **Evidence:** Confirmed
- **Problem:** 'No projects' sits in 13px muted text near the top of the right pane, with an empty 280px list beside it. It doesn't say how a project gets onto the board. The page is read-only, so the owner has nothing to act on and no hint where to go next. It is rare for the owner, hence 2.
- **Principle:** Empty states need a way forward [mobile:NNG-EMPTY]; help in context [NNG-H10].
- **Fix:** When there are no rows, drop the list column and centre one block in the page: 'No projects yet', then one line naming how a project appears (confirm the real route first; it was not found in this review). No button is needed, so the read-only rule stands.

### R16 · 1 Cosmetic · The status column doesn't hold together
- **Element:** Projects › Active › opened goal rows, dark (r-projects-open.png; projects.png)
- **Evidence:** Confirmed
- **Problem:** The Queued chip's fill (#232931) equals the open-goal background (#232932), so Queued loses its pill shape there while Done and Working keep theirs. Goal pills are 96px and task chips 104px in the same column, and the meta column starts left of the bar column.
- **Principle:** One look per role [SEC-4]; aligned to a grid [SCR-4] [NNG-GRID].
- **Fix:** Use one width, 104px, for both. Give Queued a 1px border_2 line, as Planned has one, or use a fill one step lighter on panel_2.

### R17 · 1 Cosmetic · Long names cut off with no full text; rows stretch; dates have no year
- **Element:** Projects › Active › G6 (r-projects-open.png); projects-main; Finished › date
- **Evidence:** Confirmed
- **Problem:** G6's name is cut at 'so the wh…' and neither opening the row nor hovering shows the rest. projects-main has no max width, unlike Backlog's 1320, so name and pill drift apart on a large display. Finished dates use '%b %-d', which is ambiguous after a year.
- **Principle:** Recognition [NNG-H6]; content has max widths [tablet:AND-T2].
- **Fix:** Add a tooltip with the full name on truncated rows. Set max_w(px(1320.)) on main's content. Add the year to dates outside the current year: 'Oct 18, 2025'.

## What works

- The layout is calm and scans quickly: one list, one header, cards with a fixed column for progress and status, and orange reserved for what needs the owner.
- Goals open in place while tasks open in the right panel and goals keep their page, so the one-home rule holds; the Nothing open fold keeps the picked project in view.
- Text plus a bar for progress ('2 of 6 · wave 2') and a labelled pill mean goal status does not rely on colour alone.
- Times and metadata stay spare: no filler labels, and the header shows only what the project has (release, waits-for and backlog facts).

## Not checked

- Real input: this is a native GPUI app with no page or URL for the driver, so no control was used with real clicks, keys, wheel or scroll, and inspect and the composition audit (including the blurred screenshot) could not run. Sizes, contrast and clipping come from code and theme values, and first impressions from the captures by eye.
- r-projects.png is listed as the extra seed's webapp project but shows api, and looks identical to r-projects-api.png with the same file size (a byte compare was not permitted). So webapp in its default folded state with the extra seed was not captured.
- Finished section: no capture shows it, and r-projects-open.png ends at G9 even at 1100 high. Tasks in the extra seed and the Show all link are also unseen. Findings on them come from code.
- Header details not seen in a capture: release link and date, 'unreleased' and 'Waits for'.
- Sizes other than 1360 wide, the 900x560 minimum, large displays, and the task panel open: reasoned from code only. R4's numbers are computed, not rendered.
- Light theme: no capture. Contrast for t.faint was computed; other light and status-pill colours were not checked. Bar segments and icon contrast were not measured.
- Loading and error states, including 'Can't load the board.': read in code only. Scroll position when switching projects, wheel behaviour, and hover and selected states were not exercised.
- Screen reader output, and whether gpui_kit can expose roles. Real devices, motion and performance with long lists.
- Whether stopped goals are meant to count in the sidebar's Needs you number, how a project first gets onto the board, and how many 'Waits for you to press Start' rows in the extra seed are seed effects.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
