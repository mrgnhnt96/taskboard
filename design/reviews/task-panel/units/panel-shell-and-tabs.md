## Verdict

macOS desktop app, pointer and keyboard. Reviewed at 900x560 (compact height), 1360x860 and 1728x1084, in light, plus dark at 1360x860. Evidence was 15 still captures and code excerpts; the app was not run, so nothing was used with real input and no control was actually pressed. First impression: the panel reads calm and organised, with a clear header, one column and quiet chrome. But the emphasis is in the wrong place on the Timeline tab. The eye lands on the title and then on three empty future stops before reaching the Now card, which is the thing the owner opened the panel to see. Counts: 0 Catastrophe, 4 Major, 13 Minor, 2 Cosmetic. The top problem is the footer tab row: Timeline disappears when it is the active tab, so every tab jumps about 72 CSS px sideways on the first click, and the pointer ends up on a different tab. The selected state is also only a text colour, the same as hover, so two tabs look selected at once (R1, R2). Close behind is the Timeline order, which spends about 180px, half the body at 900x560, on stops with no data before showing the current state (R3). R4 (keyboard) is the only Major rated Likely rather than Confirmed.

## Findings

### R1 · 3 Major · The tab row shifts under the pointer when you leave Timeline
- **Element:** Task panel › footer tab row, all sizes, both themes, moving from Timeline to any tab and back
- **Evidence:** Confirmed
- **Problem:** The Timeline tab is left out while Timeline is showing, so the row has three items on Timeline and four everywhere else. Clicking What to do moves that label right by about 72 CSS px (x 1043 to 1126 in the 1568px-wide images timeline-working__light__1360x860.png and what-to-do__light__1360x860.png), and Timeline now sits under the pointer. A second click, or the next tab you meant to reach, lands on the wrong tab. The captures show this: the pointer is left on Timeline in what-to-do, on What to do in context, and on Context in full-log. Tabs also never hold a fixed position, so muscle memory doesn't build for something the owner does all day.
- **Principle:** Same thing, same place: navigation keeps a stable position and order [NNG-H4] [NNG-FITTS]. Cause: view.rs:892-894 skips Timeline when it is the active tab.
- **Fix:** Delete the `continue` branch at view.rs:892-894 so all four tabs always draw in the same order and position, with Timeline marked selected by the R2 treatment. This adds one 22px-gapped label to the Timeline row, and four tabs already fit at 480px.

### R2 · 3 Major · The selected tab is marked by text colour alone, and hover looks identical
- **Element:** Task panel › footer tab row: what-to-do__light__1360x860.png, context__light__1360x860.png, full-log__light__1360x860.png, full-log__dark__1360x860.png, timeline-working captures
- **Evidence:** Confirmed
- **Problem:** Zoomed in, the active tab differs from the others only by a darker text colour, with no underline, fill or weight change. Hover uses the same dark colour, so after any click two tabs look selected (Context and Full log in full-log__light__1360x860.png), and you can't tell which tab you are on without reading the body. On Timeline no tab is marked at all. The selected and unselected colours differ by only 2.98:1 in light (#1c2129 vs #5f6b7a) and 1.91:1 in dark (#e6e9ee vs #a1abb8). The same panel marks its selected log filter with a heavy inverted pill, so one panel has two unrelated selected styles, and the weaker one is on the more important control.
- **Principle:** Current location is always visible [NNG-H1]; colour is not the only signal, and a colour-only cue needs 3:1 against its neighbours [WCAG 1.4.1]; one look per role [SEC-4].
- **Fix:** Give Look::Foot { on: true } a 2px t.accent bar on the tab's top edge, sitting on the footer's border, plus semibold text, the same idea as `tab_btn` at task_panel.rs:2298. Make hover on an unselected tab t.text_2 (#3d4652) with no bar, so hover can never read as selected. Quieten the log's selected chip to t.accent_soft with t.accent_fg text so the tab stays the stronger signal.

### R3 · 3 Major · Empty future stops come before what the task is doing now
- **Element:** Task panel › Timeline › Pending merge, Checks and review, Pull request, then the Now node; no-PR task, all sizes, worst at 900x560
- **Evidence:** Confirmed
- **Problem:** The primary task is to open a task and see what it needs, but the panel leads with three stops that hold no data, including a Checks, You, Review stepper of empty rings. In timeline-working__light__1360x860.png they take about 180 CSS px between the header and the Now card; at 900x560 that is about half of the 368px body. With a long title at 900x560 (long-text__light__900x560.png) the Now card shows only its pill and two lines, and Started is out of view. The code comment at view.rs:1094-1095 says the stop it's at reads first, which the layout doesn't do. The rows are also the kind of label with no data the owner's rule rules out, and the main line reads bottom to top while the stepper inside it reads left to right.
- **Principle:** The work dominates and the primary content draws the eye first [SCR-1] [SCR-2]; every element earns its place [SCR-5] [NNG-H8]; owner's rule: no filler labels that show no data. Source: no_pr_nodes at view.rs:2425-2429.
- **Fix:** For a task with no PR, replace the three nodes at view.rs:2427-2429 with one quiet row above Now, about 28px tall, in 13px t.muted: `Next: Pull request › Checks › You › Review › Merge`. Keep the full nodes once a PR exists, where they carry data. That puts the Now card about 150px higher and removes the empty stepper. Keep any `stack_on` line and author-review step as a second line of that row when present.

### R4 · 3 Major · No keyboard route to tabs, close, Started or log filters is visible in the code
- **Element:** Task panel › footer tabs, close, Started row, log filters; all sizes
- **Evidence:** Likely
- **Problem:** The brief includes moving between tabs by keyboard, but no focus handling for these controls appears in anything supplied. `tab_btn` (task_panel.rs:2288-2306) and kit.rs's buttons are divs with an id, a pointer cursor and a hover style. Across all supplied code the only focus handling is on text fields, there is no track_focus or tab_index, and `on_key` (task_panel.rs:2182-2199) only handles Enter in fields. No capture shows a focus ring. The code that draws `c.btn` wasn't supplied, so focus could be added there, which is why this is Likely. If it isn't, the owner must reach for the pointer to change tab, and a panel opened from search can't be worked by keyboard.
- **Principle:** Every function works from the keyboard [WCAG 2.1.1]; focus is visible [WCAG 2.4.7]; arrows move within groups, which take one tab stop [MS-KBD]; shortcuts for frequent users [NNG-H7].
- **Fix:** Check the `c.btn` draw path first. If it has no focus, give the panel controls a FocusHandle with track_focus and a tab_index, plus a 2px t.accent focus ring. Make the tab row one tab stop with Left and Right moving between tabs, and add Cmd+1 to Cmd+4 for the four tabs, shown in each tab's tooltip. Make the Started row toggle on Space and Enter.

### R5 · 2 Minor · The panel stays 480px at every window size and covers the page instead of sharing it
- **Element:** Task panel › frame, 900x560 to 1728x1084
- **Evidence:** Confirmed
- **Problem:** Measured from the captures, the panel is about 480 CSS px wide at all three sizes. At 1728x1084 there is plenty of empty page, yet log filters still wrap to two rows and the prompt in Context stays narrow. The panel also overlays rather than pushes: at 1360x860 it hides the fourth stat card completely and cuts the Agents waiting card, at 1728x1084 it cuts the Waiting on you card, and at 900x560 the heading is cut to `1 a` and the card text mid-word.
- **Principle:** Large displays show more, and the layout reflows across the whole window range [HIG-MACOS]; no control or text lost at a size [SCR-6].
- **Fix:** Set the panel width to clamp(480px, 36% of the window, 640px): 480 at 900 and 1360, about 620 at 1728. From 1200px wide up, reserve that width in the page layout so page content reflows beside the panel instead of under it. Below 1200 keep the overlay.

### R6 · 2 Minor · A long title takes nearly half the window and squeezes the body
- **Element:** Task panel › header › title: long-text__light__900x560.png, long-text__light__1360x860.png
- **Evidence:** Confirmed
- **Problem:** The title has no line limit and sits outside the scrolling body. A five-line title takes the header to about 245 of 560 CSS px at 900x560, leaving about 263px for the body, and the fixed header stays on screen while you scroll. Agent-written titles can be this long, and the cost lands on the smallest window, where the Now card is what gets pushed down.
- **Principle:** Short heights: fixed areas leave room for content [WCAG 1.4.10]; the work dominates [SCR-1].
- **Fix:** Clamp the title to two lines with an ellipsis, and show the whole title in a tooltip only when cut, using the truncating text helper at kit.rs:385. Show the full title as the first line of the What to do tab, so it's always readable there.

### R7 · 2 Minor · The `2 of 4` chip is unlabelled, sits beside a different `1 of 4`, and looks like a button
- **Element:** Task panel › header › chips, all sizes
- **Evidence:** Confirmed
- **Problem:** `2 of 4` is the task's position in the goal (the comment at view.rs:684 calls it the goal position). The sidebar shows `1 of 4` for the same goal on the same screen, meaning tasks done. Two near-identical numbers with different meanings invite a wrong read of progress. Both chips are filled, rounded grey shapes that look like soft buttons; whether they are clickable couldn't be read, because head_chips wasn't supplied.
- **Principle:** Same thing, same name; different thing, different name [NNG-H4]; no status that looks like a button [SEC-4]; one fact, one place [SEC-7].
- **Fix:** Change the chip text to `Task 2 of 4`. If the chips aren't clickable, draw them as plain t.muted text separated by ` · `, so that fill is kept for controls.

### R8 · 2 Minor · Close has no name or tooltip
- **Element:** Task panel › header › close icon button, all sizes
- **Evidence:** Confirmed
- **Problem:** `close_btn` passes None for its tip (view.rs:665), while the back button beside it gets `Back to …`. With an empty label too, the control has no text name for assistive technology and no tooltip to show a shortcut. The remove icon in Details has the same gap (view.rs:2657).
- **Principle:** Controls have an accessible name [WCAG 4.1.2]; a shortcut is shown in the tooltip [MS-KBD].
- **Fix:** Pass Some("Close") at view.rs:665, adding the shortcut to the text if Esc closes the panel, and Some("Remove this row") at view.rs:2657.

### R9 · 2 Minor · Agent text in the Now card shows raw markdown
- **Element:** Task panel › Timeline › Now node: long-text__light__900x560.png
- **Evidence:** Confirmed
- **Problem:** The capture shows a literal `**What to test:**` at the top of the Working card, because `latest` is drawn as plain text (view.rs:1428), while the What to do tab renders notes through note_body. Long `latest` text also has no height limit, so one update can fill the body and push Started out of view. The same state at 1360x860 doesn't show that line, so the two captures may hold different text; the plain-text path is proven by the code.
- **Principle:** The user's words, not raw system output [NNG-H2]; minimalist design [NNG-H8].
- **Fix:** Render `latest` with note_body as in `what_view` (view.rs:904-909), and clamp it to 4 lines in the Now card with a `Show all` link to the existing Show all dialog.

### R10 · 2 Minor · Timeline times carry no date
- **Element:** Task panel › Timeline › Started row and its Changes row, all sizes
- **Evidence:** Likely
- **Problem:** Started shows `7:31 PM` and Changes shows `last commit 6:25 PM`, both from fmt::hhmm (view.rs:1140, view.rs:1176-1182). Every example of hhmm in the code is a bare time, and the row has no tooltip, while log rows get a full-time tooltip (view.rs:2752). A task started yesterday or last week would read like today, which matters for someone checking in through the day. The 12-hour format itself follows the owner's rule. The body of fmt::hhmm wasn't supplied.
- **Principle:** Accurate system status [NNG-H1]; information not hidden behind recall [NNG-H6].
- **Fix:** When the time isn't today, prefix it: `Yesterday 7:31 PM` or `Tue 7:31 PM`, and give the Started time the same `fmt::full_time` tooltip the log uses.

### R11 · 2 Minor · The Full log count fails contrast and isn't part of the tab
- **Element:** Task panel › footer › `5` after Full log, every capture, both themes
- **Evidence:** Confirmed
- **Problem:** The count is 13px t.faint (task_panel.rs:1983): #8a95a3 on #ffffff is 3.04:1, and #6f7a88 on #1a1f26 is 3.80:1, both under 4.5:1. It is also a separate text node beside the button (view.rs:896-898), so clicking the 5 does nothing and the tab's name doesn't include the count.
- **Principle:** Text contrast at least 4.5:1 [WCAG 1.4.3]; the control's hit area covers what looks like the control [NNG-FITTS].
- **Fix:** Use t.muted (5.43:1 light, 7.12:1 dark) and draw the count inside the Full log button, so that it's part of the label and hit area.

### R12 · 2 Minor · `What to do` holds the agent's brief, not what the owner should do
- **Element:** Task panel › footer › What to do tab and its body: what-to-do__light__1360x860.png
- **Evidence:** Confirmed
- **Problem:** The owner's task is to see what the task needs from them, and this tab name promises exactly that. It actually shows the task's description (`Add the login ceremony and the button on the sign-in page.`), which is the instruction for the agent. What the owner must do appears elsewhere, in the banner. The label sends the owner to the wrong tab in the moment they most need the right one.
- **Principle:** The label says what the result is [NNG-CW]; the user's words [NNG-H2].
- **Fix:** Rename the tab `Brief` at view.rs:891.

### R13 · 2 Minor · Context contains edit controls the read-only rule doesn't allow
- **Element:** Task panel › Context › Details rows and Bits row; shown only when the task has them
- **Evidence:** Confirmed
- **Problem:** Details draws editable Name and Value fields with a delete icon (view.rs:2652-2659) and saves on Enter (task_panel.rs:2191-2196). Bits has a `Mark created` button (view.rs:2565). Neither is in the owner's list of allowed one-click signals, and editing Details is not a one-click signal at all. The fields also use placeholders as their only labels. No capture reaches this state, so this comes from the code alone.
- **Principle:** Owner's rule: the app is read-only apart from one-click signals; a placeholder isn't a label [WCAG 3.3.2].
- **Fix:** Draw Details as read-only K::KvRow rows like the rows above it and remove the fields and `meta-del`. Keeping Mark created, or editing Details, would need the owner's OK; if it stays, add it to the rule's list explicitly.

### R14 · 2 Minor · Branch and commit facts come from different sources on two tabs
- **Element:** Task panel › Timeline › Started expansion and Context › detail rows: started-expanded__light__1360x860.png, context__light__1360x860.png
- **Evidence:** Confirmed
- **Problem:** Started's `last commit` falls back to the log (view.rs:1176-1179), but Context reads only `where.last_commit` (view.rs:2587), so a task can show a last commit in one place and no Last commit row in the other. Context in the capture says Branch `Not reported yet` and Files touched `None yet.` while the log lists three commits; that gap may come from the overlaid capture data, but the code difference is real.
- **Principle:** One fact, one place; no copies that can disagree [SEC-7].
- **Fix:** Compute the last commit once in a shared helper using the fallback at view.rs:1176-1179 and call it from both tabs. Better, drop Branch and Changes from one of the two places: keep them in Context and let the Started expansion hold only terminals.

### R15 · 2 Minor · Seven log filters wrap to two rows at every size, and some can't find what they suggest
- **Element:** Task panel › Full log › filter chips: full-log__light__900x560.png, full-log__light__1728x1084.png
- **Evidence:** Confirmed
- **Problem:** The seven chips never fit the 480px panel, so they take two rows even at 1728x1084; at 900x560 that is about 70 CSS px of a 368px body. Questions, Found and Jira show with nothing behind them for this task. The names don't match the row tags: Commit rows are under Turns (view.rs:2677). Note rows belong to no filter (view.rs:2682), so the Note row in the capture disappears under every filter except All.
- **Principle:** A row of options that doesn't fit becomes a select or menu [HIG-SEGMENTED] [NNG-LISTBOX]; the user's words [NNG-H2]; every element earns its place [SCR-5].
- **Fix:** Replace the chips at view.rs:2764 with one pop-up menu reading `Show: All`, on a single row, with each option carrying its count and kinds with no entries dimmed. Rename Turns to `Turns and commits`, and add `Notes` as a group in log_group and LOG_FILTERS (view.rs:2671-2684).

### R16 · 2 Minor · The log's helper sentence breaks the no-filler rule
- **Element:** Task panel › Full log › text under the rows: full-log__light__1360x860.png, full-log__dark__1360x860.png
- **Evidence:** Confirmed
- **Problem:** `The log is saved as it happens, so it survives a closed terminal or a restart.` shows no data and tells the owner nothing they can act on. It repeats on every task, in the rows view and the empty view.
- **Principle:** Owner's rule: no filler labels or helper text that shows no data; minimalist design [NNG-H8].
- **Fix:** Delete LOG_HELP (view.rs:2743), its push at view.rs:2770 and its drawing after the Node::Log rows.

### R17 · 2 Minor · The log filter probably carries over to the next task
- **Element:** Task panel › Full log › filters, when opening another task
- **Evidence:** Likely
- **Problem:** The filter lives in shared UI state (`u.log_filter`, task_panel.rs:448), and the reset on a task change clears term_menu, tl_open and rev_pop but not the filter (task_panel.rs:923-927). A filter set on one task would then hide rows on the next, which could show `Nothing of this kind yet.` beside a Full log count of 5. Nothing was run, and a reset elsewhere isn't ruled out.
- **Principle:** Sensible defaults and visible status [NNG-H5] [NNG-H1].
- **Fix:** Add `u.log_filter = "all".into()` to the reset block at task_panel.rs:923-927.

### R18 · 1 Cosmetic · `3m · updated 3m ago` has an unlabelled number
- **Element:** Task panel › Timeline › Now node sub-line, all sizes
- **Evidence:** Confirmed
- **Problem:** The first `3m` has no label, so it isn't clear that it is run time (view.rs:1402). The equal values here come from the sample data, but the missing label doesn't.
- **Principle:** The user's words [NNG-H2].
- **Fix:** Write `Running 3m · updated 3m ago` at view.rs:1402.

### R19 · 1 Cosmetic · The least-used block on Context has the most emphasis
- **Element:** Task panel › Context › What a new terminal gets: context__light__1360x860.png, context__light__900x560.png
- **Evidence:** Confirmed
- **Problem:** The only tinted box and blue heading on the tab go to a long prompt the owner rarely needs. At 900x560 only about a line and a half of it shows, cut by the footer. The blue heading also doesn't match Files touched, which is the same level (view.rs:2665).
- **Principle:** One emphasis, kept for what matters [SEC-5]; consistent labels [SEC-6].
- **Fix:** Use the neutral box tone and t.muted heading, matching Files touched.

## What works

- The panel is one calm column with quiet chrome. The header, body and fixed footer read the same in all three sizes and both themes, and nothing overlaps or clips inside the panel at 900x560 with normal text.
- The Now card is the one filled block on the Timeline, with a clear status pill and the agent's latest line, so it stands out once you reach it. The Started expansion opens in place and keeps its context.
- Dark mode holds up: body text and muted text pass (muted is 7.12:1 on the card), borders survive, and no light colours are left behind. The only contrast failure found is the tab count, in both themes.
- Times follow the 12-hour AM/PM rule throughout, log rows keep a full-time tooltip, and the log uses a virtualized list that fetches older pages, as the owner's rules ask. Group headings sit above their members (L1), and nothing here uses tabs for a growing set (L2).

## Not checked

- Real use: the app wasn't available to run. The browser was used only to zoom captures and compute contrast, so no control was clicked, typed in, dragged, scrolled or tabbed to. Every finding comes from captures and code.
- The composition audit and `inspect` couldn't run on a native app. Sizes were measured from the images, so they are approximate. Colours come from theme.rs; which text style maps to which colour was read from code only where supplied.
- Keyboard behaviour: the code that draws `c.btn` and Look::Foot wasn't supplied, nor was the caller of `escape` (task_panel.rs:783-789), so whether Esc closes the panel, Tab order, focus on opening and focus return on closing are all unknown. R4 depends on this.
- VoiceOver and accessibility tree output, status announcements and role or state exposure.
- Whether the tab, scroll position and Started expansion survive switching tabs, resizing or closing and reopening the panel. Window sizes between the three captured, the 900px minimum edge while dragging, and short-window behaviour other than 900x560.
- Dark mode at 900x560 and 1728x1084, and dark for What to do, Context, Started expanded and long text. macOS text size and zoom settings. Hover and pressed states other than the leftover hover seen in the tab row.
- States not in the captures: the back button, a long goal name in the breadcrumb (c_top wraps at task_panel.rs:1475 and close could drop to a second line), Jira and Attached chips, terminal rows and Focus in the Started expansion, Now-node actions, filled Files touched, Done so far, Next, Decisions, Your answers, Found, Details, loading and error, an empty log, a filter with no matches, and a very long log.
- Whether Focus or other terminal actions are reachable from a working task without opening Started: `term_actions` wasn't in this unit's excerpts and the sample task has no terminals.
- Whether `2 of 4` and the other chips are clickable: head_chips wasn't supplied. fmt::hhmm and fmt::log_time bodies weren't supplied either, so R10 is inferred from their use.
- The long-text captures differ: only the 900x560 one shows `**What to test:**`, and the Branch and Files values differ between the Started and Context captures. Both may come from the overlaid data; R9 and R14 rest on the code.
- Scope note: besides the listed shell excerpts I read action-handler and kit.rs excerpts from sibling folders under the same code directory. I also ran one `ls`/`wc` of the excerpts outside the stated browser-only shell rule, and wrote a zoomed image to /private/tmp/claude-501/ultrahand-review-shots/tabs-zoom.png. Neither changed anything in the project.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
