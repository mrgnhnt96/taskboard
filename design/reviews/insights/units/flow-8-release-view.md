## Verdict

macOS desktop app (GPUI), pointer and keyboard, dark theme. I reviewed 7 captures at 1728x1084, 1360x860 and 900x560, plus the code for the Releases pages, the sidebar and the app shell. The evidence is code and screenshots only: this is a native app, so nothing was driven with real input. First impression: the project page reads as organised, with a clear order of Next cut, Unreleased, newest release, then months. It is heavy, though, with boxes three deep and an alarm-coloured bar for a normal waiting state. Counts: 0 Catastrophe, 4 Major, 12 Minor, 2 Cosmetic. The top problem is that the flow is pointer-only: release rows, task links, goal links and PR links can't be reached from the keyboard, and the reason a cut can't finish is in a hover tooltip. The next most costly problem is that the task panel covers the state, PR and date columns at 1360 and leaves 172px of the page at 900, so the owner can't watch the Next cut list while reading a task.

## Findings

### R1 · 3 Major · The Releases flow can't be used from the keyboard
- **Element:** Releases overview and project page › all rows and links, at every size (releases.rs goal_link, task_row, next_cut, release_row, overview_card)
- **Evidence:** Confirmed
- **Problem:** Every control here is a div with on_click. None takes focus, and the app crate has no tab_index or tab_stop; track_focus exists only on the window root, text fields and a few panels. Someone on the keyboard can't expand a release, open a task or goal, follow a PR, or use the Releases back link. Only ⌘←, Esc, ⌘K and the page shortcuts work.
- **Principle:** Every function works from the keyboard, with visible focus [WCAG 2.1.1] [WCAG 2.4.7]; arrows move within lists, which take one tab stop [MS-KBD]
- **Fix:** Make the release list one tab stop with a roving selection. ↑/↓ moves between rows, including task rows inside open releases, Return opens the task or goal, and →/← or Space expands and collapses a release row. Draw a 2px t.accent focus ring on the current row. Give the back link and the PR links tab stops as well. Check first whether gpui_kit already offers a focus helper (not read in this review).

### R2 · 3 Major · Next cut rows and task rows open by different parts, and the hover misleads
- **Element:** Project page › Next cut rows and goal-card task rows; Overview › card rows, in rel-webapp-dark-m.png and releases-dark-m.png
- **Evidence:** Confirmed
- **Problem:** In Next cut only the 46px ref opens the item; clicking the title 'Link Releases from the phone app' does nothing and the row has no hover. In task rows it is the opposite: only the title text opens the task and the ref is dead, yet the whole row lights up on hover, so a click on the ref or the blank space does nothing. On overview cards, the big version block and the Unreleased strip are clickable with no hover state, only a cursor change. The Next cut ref also changes colour with state (green, blue, purple, red), so its colour doesn't say 'link'.
- **Principle:** Same thing, same behaviour [NNG-H4]; a hover state must tell the truth about what's pressable, and larger targets are easier to hit [NNG-H1] [NNG-FITTS]
- **Fix:** Make the whole row the target in both lists, with the PR link stopping propagation. A row click opens the task in the right panel, or the goal for G refs. Use the same t.panel_2 hover on Next cut rows, the overview's version block and the Unreleased strip. Keep the ref colour for type (task or goal) and let the mark and label carry the state.

### R3 · 3 Major · A cut that can't finish hides why, and doesn't say the cut will be dropped
- **Element:** Project page › Next cut › a dead row and the header pill; Overview › pending pill (next_cut, wait, card). Not in the captures: code only
- **Evidence:** Confirmed
- **Problem:** When a waited-for item can't be met, the code's own comment says the cut will be dropped, but the page shows only a red 'Can't finish'. The reason sits in a hover tooltip on a 12px label with nothing to show it has one, so keyboard users can't reach it. The header pill still reads '1 of 3 ready' in warn tone and the overview still says 'v0.14.0 pending'. This is the most important thing the Next cut section can tell the owner, and it looks like a normal wait.
- **Principle:** Say what happened and what to do about it [NNG-H9]; show system status [NNG-H1]; content must not exist only on hover [WCAG 1.4.13] [WCAG 2.1.1]
- **Fix:** Show w.why as a second line under the row title in t.down, 12px, wrapping rather than truncating. When any item is Dead, change the header pill to 'Can't finish' in the failed tone and the overview pill to 'v0.14.0 can't finish'. Add one plain line saying the cut will be dropped. No action in the app: cutting stays in tb.

### R4 · 3 Major · The task panel covers the columns the owner came to read, and the open row isn't marked
- **Element:** Project page with task panel open, 1360x860 (rel-panel-dark-m.png) and 900x560 (rel-panel-dark-s.png)
- **Evidence:** Confirmed
- **Problem:** The 480px panel floats over the page with no reflow. At 1360 it hides every Next cut state (Done, Running, 2 of 3), every PR link and date, and the counts. At 900 only 172px of page is left, so titles are cut to 'Add', 'Link' and 'Rele'. The page can't scroll sideways, so those columns can't be reached without closing the panel, although the code comment says the page stays usable. Walking the checklist task by task therefore means open, close, open. T65 also has no selected state, so nothing says which row the panel belongs to.
- **Principle:** Selection is kept and shown in each pane [tablet:HIG-SPLIT] [NNG-H1]; options stay visible rather than remembered [NNG-H6]
- **Fix:** While the panel is open, give the page's list a right inset of 480px so it reflows instead of being covered. At 1360 that leaves about 600px, enough for truncated titles plus the state, PR and date columns. Below that width, drop the date first, then the PR, and keep the ref and the state: about 116px of column at 900 still fits 'T68 Running'. Fill the open task's row with t.panel_2 plus a 2px t.accent left edge in task_row and next_cut, matching on the Panel::Task ref.

### R5 · 2 Minor · Overview cards stay at minimum width beside an empty column, and cut text has no way to be read
- **Element:** Overview › webapp card › v0.11.1 row at 1360x860 (releases-dark-m.png); also project page headlines and task titles
- **Evidence:** Confirmed
- **Problem:** columns() gives 3 columns at 1360 and pads the row with an empty filler, so 2 projects sit at about 342px with the right third of the page blank. In that narrow card the Failed row cuts its time to 'Oct 21, 8:38…', losing the AM/PM the owner requires. Release headlines and task titles also truncate with no tooltip, although kit::clipped exists for this.
- **Principle:** Show more on large displays and reflow across the window range [HIG-MACOS]; the owner's 12-hour AM/PM rule; content isn't lost at any size [WCAG 1.4.10]
- **Fix:** Clamp columns to min(columns, projects.len()) and cap a card at about 520px so the filler goes. In the card's earlier-release rows show the date only ('Oct 21'), since the full time is one click away, or put the Failed pill beside the name. Replace .truncate() on headlines, task titles and Next cut titles with kit::clipped.

### R6 · 2 Minor · Clicking the newest release lands on Unreleased in short windows
- **Element:** Overview › version block → Project page › v0.13.0 at 900x560 (position inferred from rel-panel-dark-s.png)
- **Evidence:** Confirmed
- **Problem:** The newest release shares Item::Top with the header and Unreleased, so focusing it scrolls to item 0. Unreleased alone fills a 560px window, so the release the owner just clicked is below the fold and nothing shows the click did what it said. Older rows have a smaller form of this: scroll_to_reveal_item may leave the row at the bottom of the view with its detail below the fold (not observed).
- **Principle:** After an action, the user sees progress toward the goal [NNG-CW]; keep focus on what was opened [NNG-H1]
- **Fix:** Split Item::Top into Header, Unreleased and Newest list items, and scroll the target to the top of the view for both the newest and older releases. Show a 1-second t.panel_2 highlight on the row reached.

### R7 · 2 Minor · Next cut and the Unreleased list below it don't add up
- **Element:** Project page › Unreleased section at 1360x860 (rel-webapp-dark-m.png)
- **Evidence:** Likely
- **Problem:** T1 is Done in Next cut but is in no group below, and T68 is listed as waited-for but not in the tally of '3 tasks · 1 goal'. G4 reads '2 of 3' above and '2 tasks' below with no stated link, and T68 may be G4's third task, listed as a sibling of its own goal. The owner can't tell what v0.14.0 will contain versus what it merely waits for. This may be the sample's data; the code does show the two lists come from separate sources with nothing tying them together.
- **Principle:** One fact, one place [SEC-7]; match the user's picture of the work [NNG-H2]
- **Fix:** Decide what the section promises and show it. If a task in the Unreleased list is what ships, mark each row that the cut waits for, and list an unfinished waited-for task inside its goal group with its state. If a met item isn't in Unreleased, say where it went, such as its release name in place of 'Done'.

### R8 · 2 Minor · Task rows aren't indented under their goal
- **Element:** Project page › goal cards at all sizes (rel-webapp-dark-m.png: G4 and T65 share a left edge)
- **Evidence:** Confirmed
- **Problem:** The goal's ref and name sit on the same 16px left edge as its tasks' refs, so G4, T65 and T66 read as one column of equal items. The owner has already corrected this pattern.
- **Principle:** LESSONS L1: put the group's label above its members and indent the members under it
- **Fix:** Set task_row's left padding to 42px, so refs start under the goal's name, 26px in from the group's ref. Apply it to 'No goal' groups too. Keep the PR and date columns on their right edge.

### R9 · 2 Minor · Project page is over-boxed, and a normal wait is drawn as an alarm
- **Element:** Project page › Unreleased section (whole surface), 1360x860 and 1728x1084
- **Evidence:** Confirmed
- **Problem:** Blurred, the brown Next cut bar is the brightest thing on the page in a normal state, so a real problem has nothing louder to use. Evidence: boxes run three deep (section, card, row) [SEC-2]. Amber rules separate every Next cut row. The header pill uses warn_soft on a warn_soft bar, so it reads as loose text. The page uses about six text sizes (22, 18, 16, 14, 13, 12) [SCR-3]. The breadcrumb's 'webapp' sits directly above the 'webapp' title [SEC-7].
- **Principle:** Group by space, containers only where space can't do it [SEC-2]; one emphasis [SEC-5]; about three type sizes [SCR-3]
- **Fix:** Remove the outer box from Unreleased and the newest release: use the 18px heading and spacing, and keep cards only for goals. Draw the Next cut header on t.card with a neutral t.divider between rows, and use the warn tone only for the pill, which then shows as a pill. Save the filled bar for the Can't finish state in R3. Use 22, 14 and 12 as the text sizes, and drop the second 'webapp' from the breadcrumb or keep it only as the title.

### R10 · 2 Minor · A failed release doesn't say what failed or where its work went
- **Element:** Project page › v0.11.1 row at 1728x1084 (rel-open-dark-l.png); Overview › v0.11.1; the Failed count in the header
- **Evidence:** Confirmed
- **Problem:** The row shows 'Failed' and '0 tasks'. Opening it gives 'No tasks'. The code comment says the work went back to Unreleased, but the UI never says so, so it reads as an empty release. There is no failure time or reason, and the Failed count in the header doesn't lead to the failed rows in a paged list. 'How the work went' is the primary task.
- **Principle:** Help users recognise and diagnose errors [NNG-H9]; show status [NNG-H1]
- **Fix:** Replace '0 tasks' with 'Work moved to Unreleased' on failed rows, and show the failed_at time in the open row, with the reason if the board has one. Make the header's Failed count scroll to the newest failed release. If the task count before failure isn't in the API, adding it needs a board change.

### R11 · 2 Minor · Unreleased and the newest release aren't virtualized
- **Element:** Project page › Item::Top and open rows (top, work, release_row)
- **Evidence:** Confirmed
- **Problem:** Only the older-release rows are list items. Unreleased and the newest release, with all their task rows, render as one Top item that is remeasured on every sync, and an open older release is one item too. A release with hundreds of tasks is laid out in full. api's v2.3.0 already has 19 tasks in the sample.
- **Principle:** Owner rule: long lists are virtualized; large displays stay responsive [HIG-MACOS]
- **Fix:** Flatten the list: make each goal head and task row its own Item, along with Next cut. This also gives R6 its scroll targets. Keep the card look with first and last flags, as the month boxes already do.

### R12 · 2 Minor · A failed fetch leaves 'Loading…' on screen
- **Element:** Project page › open release, Unreleased; Overview › card (app.rs apply, work, overview_card). Not in the captures: code only
- **Evidence:** Confirmed
- **Problem:** Errors from the release list and detail calls are dropped (if let Ok(v)), so a failing call leaves 'Loading…' indefinitely with no message or retry. Only a whole-board failure shows 'Can't load the board.', and only when the list is missing.
- **Principle:** In the error state, show what happened and a way forward [NNG-H1] [NNG-H9]
- **Fix:** Store the Err per key and show 'Can't load v0.11.0' with a Retry link in place of 'Loading…', keeping any stale data shown. Retrying is a read, so it stays within the read-only rule.

### R13 · 2 Minor · A new release probably throws the reader back to the top
- **Element:** Project page › older releases while scrolled, when the board cuts a release (sync)
- **Evidence:** Likely
- **Problem:** sync only keeps its place when items are added at the end. A new release changes the head of the list, so it calls state.reset, which likely loses the scroll offset. The owner keeps this app open all day, so reading an older release while an agent cuts one would jump the page. Not observed, and the behaviour of GPUI's reset wasn't read.
- **Principle:** Scroll position survives updates [tablet:HIG-MULTI]; user control [NNG-H3]
- **Fix:** When items change, find the first visible release by ref in the new list and restore logical scroll to it. Only reset if that release has gone.

### R14 · 2 Minor · Falling back when releases turn off is silent, and lands on a page that contradicts itself
- **Element:** Needs you after fallback at 1360x860 (rel-off-dark-m.png); app.rs lines 1455–1461
- **Evidence:** Confirmed
- **Problem:** The page swaps to Needs you with no word of why Releases went. The landing shows 'No agents waiting' above 'Loading…' while the sidebar says 1, so the header states a false fact until data lands (capture timing unknown; the header line ignores whether home has loaded). m.page is set directly, so history keeps the Releases entries: ⌘← returns to a page that then bounces away.
- **Principle:** Visibility of system status [NNG-H1]; user control, no dead ends [NNG-H3]
- **Fix:** Show a toast, 'Releases are off for every project', and the same with the project's name when one project's page falls back to the overview. Hide the header's waiting line until data.home is loaded. Remove Releases entries from back and forward when the pages go away, or skip them in go_back.

### R15 · 2 Minor · Releases has no menu item or shortcut
- **Element:** macOS View menu and shortcuts (main.rs, app.rs); sidebar › Releases
- **Evidence:** Confirmed
- **Problem:** Needs you, Projects, Goals, Backlog and Days are in the View menu, and each has a ⌘ shortcut. Releases has neither, so it is the one sidebar page in this flow reachable only by mouse. Ideas has the same gap.
- **Principle:** Every command is reachable from a menu, with its shortcut shown [HIG-MACOS] [MS-KBD]
- **Fix:** Add a GoReleases action bound to ⌘7 and a View › Releases item, disabled while no project has releases on. Ideas can take the unused ⌘4.

### R16 · 2 Minor · Tick and cross marks are under contrast in dark mode
- **Element:** Project page › Next cut marks, dark theme (mark)
- **Evidence:** Confirmed
- **Problem:** White (on_accent) on t.up #4cc38a is about 2.2:1, and white on t.down #f47067 is about 2.85:1, both for a 10px glyph. The Done and Can't finish labels carry the meaning too, so the harm is limited, but the marks are hard to read at a glance. The light theme passes (about 4.4:1 or better).
- **Principle:** Non-text contrast at least 3:1 [WCAG 1.4.11]
- **Fix:** Use a dark glyph on these fills in dark mode: a near-black such as #0f1419 gives about 8:1 on the green and about 6:1 on the red.

### R17 · 1 Cosmetic · 'Unreleased 0 tasks' is filler on the api card
- **Element:** Overview › api card at 1360x860 (releases-dark-m.png)
- **Evidence:** Confirmed
- **Problem:** A full-width strip says nothing is waiting and is still clickable, while the project page hides Unreleased when it is empty. The two pages disagree, and the owner has asked for no filler.
- **Principle:** Every element earns its place [NNG-H8]; owner rule on filler labels
- **Fix:** Drop the strip when there are no unreleased tasks and no pending cut.

### R18 · 1 Cosmetic · At the smallest window the sidebar shows one clipped goal
- **Element:** Sidebar › Active goals at 900x560 (rel-panel-dark-s.png)
- **Evidence:** Confirmed
- **Problem:** The stacked footer pills leave room for one goal, and its last line ('2 of 3', 'just now') is cut in half. The other two goals are out of view, with only that cut to suggest the list scrolls.
- **Principle:** Short heights leave room for content [HIG-MACOS]
- **Fix:** Put the footer pills on two rows by moving Settings beside 'No work hours', which frees about 30px for a complete second goal row.

## What works

- The project page's order fits the job: what's waiting, then what just shipped, then history by month. The Next cut checklist answers 'what does it still wait for' in one look, with a mark and a word for each state, so colour is never the only signal.
- Lesson L2 is respected: months are headings in one list, not tabs. Older releases open in place, and open rows and scroll are kept while the project stays the same.
- The 920px column keeps lines readable, and at 1728 the panel fits beside it without covering anything.
- Times follow the 12-hour AM/PM rule wherever they aren't truncated, and the page keeps to the read-only rule: no cut or edit controls, and all the fixes above stay within it.

## Not checked

- Real input: this is a native GPUI app, so the browser driver can't reach it. No click, hover, scroll, Esc, ⌘← or drag was exercised. Behaviour comes from code and the 7 captures, so the 'used with real input' check is not met for any control.
- VoiceOver and accessible names, roles and state: whether GPUI exposes any of this wasn't checked. gpui_kit's source wasn't read, so R1 rests on the app crate alone.
- Light theme: no captures. Colours were read from theme.rs only, and I worked out by hand only the mark contrast in R16; other text wasn't measured with inspect, and the dark pairs I judged by eye looked comfortable.
- Text scaling, focus appearance, motion, and the scrollers' wheel and trackpad behaviour.
- States missing from the captures: a dead wait, the cutting state, a failed newest release, a project with no releases, an empty pending list ('0 of 0 ready' looks possible from the code), the fetch-error state, and long lists. The sample has only 2 projects and 9 releases.
- Sizes between the three captured, and the overview and an open row at 900x560.
- The goal page and PR destinations: only the links to them were read. Back and forward were read in code, not used.
- The Days and weekly review part of the primary task: outside this flow's scope.
- The task panel's content: it shows Pull request as not reached and no #431 link for T65, while the page lists #431, and 'No summary was given'. This may be the sample's data.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
