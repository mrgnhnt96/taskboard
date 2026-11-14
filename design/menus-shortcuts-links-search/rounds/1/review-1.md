## Verdict

This is a macOS desktop app used with pointer and keyboard. I reviewed it from the local canvas build, driving it with trusted mouse and keyboard input through drive.mjs and reading setup.js, markup.html and own.css. I tested at 900 × 560, 1440 × 900 and 2560 × 1440, in light and dark, at 100% and 200% text, on both screens and across most of the 16 states. First impression: the Design page is well organised. It has a clear contents list, a TL;DR, today-vs-fix pairs and one settled key table. The app window is calm: the page content leads and the sidebar and menu strip stay quiet. Contrast passes everywhere except disabled menu items, which WCAG exempts. Targets pass 2.5.8. The problems are in how the drawn app behaves for a keyboard or screen-reader user, which is exactly what this section is about. Every re-render drops focus to the page. The search palette and the New idea dialog don't trap focus or make the background inert. The search results are never announced. Back and Forward around Releases work the opposite way to the design's own rule. Counts: 0 Catastrophe, 3 Major, 9 Minor, 1 Cosmetic. Top problem (R1): after closing a menu, picking a menu item, pressing Esc, closing search or the dialog, or pressing any in-window button, focus falls to the page body. That breaks the keyboard flow this design sets out to fix.

## Findings

### R1 · 3 Major · Focus falls to the page body after almost every action
- **Element:** The app › menu bar menus, search palette, New idea dialog, side-panel buttons, all sizes, light and dark
- **Evidence:** Confirmed
- **Problem:** Each of these left focus on body: Esc from an open menu, Enter on a View menu item, Tab inside a menu, Esc from the dialog, and clicking 'Focus in Midna' in the panel. The cause is that render() replaces the whole window with innerHTML and only refocuses in a few cases. A keyboard user has to Tab from the top of the document again after each command. Opening a search drawing from the Design page also leaves focus on body, so typing does nothing.
- **Principle:** Focus order: closing a menu or dialog returns focus to what opened it, and focus is never lost mid-page [WCAG 2.4.3] [MS-KBD] [desktop:APG-DIALOG]
- **Fix:** Record document.activeElement (or its data-mb, data-act or data-open key) before each render() and restore it afterwards. Esc or Tab from a menu should focus its [data-mb] button. The current code reads A.menu after the capture handler has already set it to null, so pass the id instead. Closing the palette or dialog should focus the control that opened it: .sfind, the File menu button or the row. A menu pick should focus the main heading (tabindex=-1) of the page it opened. Render panel actions without rebuilding the panel. In onState, focus .pal-in input whenever A.pal is set.

### R2 · 3 Major · Search palette and New idea dialog leave the background in the Tab order
- **Element:** The app › Search palette (all search states) and New idea dialog, 900 × 560 and 1440 × 900
- **Evidence:** Confirmed
- **Problem:** With search open, inspect still lists 24 tab stops behind the scrim: all 8 menu-bar buttons, the sidebar and the Needs you card. Tab from the search field went straight to the canvas Size select. In the dialog, which has aria-modal=true, four Tabs left it for body. Keyboard and screen-reader users end up behind a scrim they can't see past.
- **Principle:** While a modal is open, focus stays inside it: the background is inert and Tab wraps [WCAG 2.4.3] [desktop:APG-DIALOG] [WCAG 2.1.2]
- **Fix:** While .pscrim or .dscrim is open, set inert on .mbar, .side, .main, .panel and .wtoasts, but leave .wtoasts live for error toasts. Add aria-modal="true" to .pal. Trap Tab inside the layer: in the palette, Tab moves between the field and the scope chip's ×; in the dialog it cycles Title › Details › Choose a project › Add idea › Close.

### R3 · 3 Major · Search results are not exposed to screen readers
- **Element:** The app › Search palette › field and result list, all search states
- **Evidence:** Confirmed
- **Problem:** Focus stays in the input, which has no combobox role, aria-controls or aria-activedescendant. Rows are tabindex=-1 buttons with role=option, and group headers are role=presentation. When ↑ ↓ move the single highlight, nothing is announced. A screen-reader user hears 'Search, edit text' and never learns what is selected or how many results there are. 'Searching…' and 'Nothing matches' are role=status, but the result count isn't.
- **Principle:** Name, role and value; status messages [WCAG 4.1.2] [WCAG 4.1.3] [WCAG 1.3.1]
- **Fix:** Make the input role="combobox" with aria-expanded="true", aria-controls pointing to the .pal-list id and aria-autocomplete="list". Give each row a stable id and set aria-activedescendant to the highlighted row's id on every move. Wrap each group in role="group" with aria-labelledby pointing to its header ("Tasks · 4"), using divs with role=option instead of buttons. Put a polite live region under the field that says "8 results" after typing settles.

### R4 · 2 Minor · Back and Forward around Releases work the wrong way round
- **Element:** The app › View › Back ⌘[ / Forward ⌘], Releases page with releases off, 1440 × 900
- **Evidence:** Confirmed
- **Problem:** The design says Releases with releases off 'doesn't join Back and Forward'. With real keys, ⌘3 › ⌘7 › ⌘[ went to Needs you, so Goals was lost from history. ⌘7 › ⌘2 › ⌘[ went to Releases, so Releases did join history. go() checks the destination page when it should check the page being left.
- **Principle:** Predictable navigation and user control [NNG-H3] [NNG-H4]
- **Fix:** In go(), push the current page unless the current page is Releases-off: `if (A.page !== 'releases') A.back.push(A.page)`. Then ⌘3 › ⌘7 › ⌘[ returns to Goals, and Releases never appears in Back. Add both sequences to the Testing checklist.

### R5 · 2 Minor · ⌘K opens search underneath the New idea dialog
- **Element:** The app › New idea dialog open, then ⌘K (also ⌘R, ⌘[ and ⌘]), 900 × 560
- **Evidence:** Confirmed
- **Problem:** With the dialog open, ⌘K drew the search palette behind the dialog with a second scrim, while focus stayed in Title. The user sees a stacked mess and can't use search. The key table says only that page keys pause while a dialog is open; it doesn't say what ⌘K, ⌘R or ⌘[ ⌘] do then.
- **Principle:** Error prevention and consistency [NNG-H5] [NNG-H4]
- **Fix:** Pick one rule and state it in the key table. Either every window command (⌘1–⌘7, ⌘K, ⌘R, ⌘[ ⌘], ⌘N) pauses while a dialog is open and its menu items grey out, or ⌘K closes the dialog first, keeping the draft and toasting 'Idea draft kept · Reopen ⌘N' as the link flow does. Then add the matching case to the Testing checklist.

### R6 · 2 Minor · Menu bar can't be crossed with arrow keys and takes 8 Tab stops
- **Element:** The app › menu-bar strip (Task board … Help) and its open menus, 1440 × 900
- **Evidence:** Confirmed
- **Problem:** With the Task or View menu open, → and ← left the same menu open and moved focus to the menu container, so the next menu never opened. With menus closed, each of the 8 menu titles is its own Tab stop ahead of the sidebar, so reaching Search takes 9 Tabs.
- **Principle:** Arrow keys move inside groups, which take one tab stop [MS-KBD] [WCAG 2.1.1]
- **Fix:** Make .mbar one tab stop with a roving tabindex (only the current title has tabindex=0). Use ←/→ to move between titles, and ↓ or Enter to open a menu. Inside an open menu, ←/→ should close it and open the neighbour with its first item focused. Fix the onSide path so it survives the global Escape and capture handler, which currently re-renders first. On a real Mac this is Ctrl-F2; say so in the doc.

### R7 · 2 Minor · Commands that can't run stay enabled, against the design's own rule
- **Element:** The app › Task menu (Focus in Midna, Start, Stop) and the row context menu (Focus in Midna), state 'Midna isn't running', 1440 × 900
- **Evidence:** Confirmed
- **Problem:** The Solution says every command is 'greyed when it can't run'. Yet with Midna stopped, Task ▸ Focus in Midna, Start and Stop are enabled, and the sidebar tip says starting waits for Midna. Open Pull Request ⌘↩ is enabled in the Task menu but greyed in the row context menu. Clicking 'Focus in Midna' twice stacked two identical error toasts.
- **Principle:** Consistency and error prevention [NNG-H4] [NNG-H5]
- **Fix:** With Midna down, grey Focus in Midna, Start and Stop everywhere and rename the first item 'Focus in Midna — Midna isn't running'. Alternatively keep them enabled and change the rule in the Solution text to 'enabled, and a toast says why it can't run'. Pick one. Make Open Pull Request's state the same in both menus. Don't add an error toast when the same text is already showing; flash the existing one.

### R8 · 2 Minor · At 200% text, search titles and their match highlight are cut while the goal column keeps full width
- **Element:** The app › Search palette › result rows, state 'Search, typed', 900 × 560, 200% text
- **Evidence:** Confirmed
- **Problem:** inspect flags 12 cut texts. The title 'Settings page: ma…' loses 136px, and the highlighted 'pass' itself is cut on 4 rows. Meanwhile 'Sign-in with passkeys' in the goal column stays whole. The user can't see why a row matched. The design says the goal drops below 640pt, but at 200% text it doesn't.
- **Principle:** Resize text without losing content [WCAG 1.4.4]; most important content gets the space [SCR-1]
- **Fix:** Drive the goal-drop rule with a container query on .pal (container-type: inline-size), dropping .gn at 40em instead of at a fixed 640px, so it also triggers when text grows. Let .tl wrap to two lines (-webkit-line-clamp: 2) rather than truncating to one, and keep the status pill at intrinsic width.

### R9 · 2 Minor · Midna status is explained only on hover, on a pill that looks like a task status
- **Element:** The app › sidebar footer › 'Midna isn't running' pill, all sizes
- **Evidence:** Confirmed
- **Problem:** The explanation ('Starting, messaging and showing terminals wait until it's back') is a data-tip on a span with tabIndex −1, so keyboard and touch users never see it. The pill also uses the same orange 'needs' style as the 'Needs you' task status, so a service state reads as task status.
- **Principle:** Keyboard access to all content; one look per role [WCAG 2.1.1] [WCAG 1.4.13] [SEC-4]
- **Fix:** Make it a focusable button with an alert icon in the neutral or warning style, not .pill.needs. Clicking or focusing it should show the tip, and Esc dismisses it. Put the same explanation in the error toast, too. Give it aria-describedby so screen readers read the sentence.

### R10 · 2 Minor · Dark mode never shows today's dark captures on the Design page
- **Element:** Design › every Today column (Search, Midna, Releases), dark scheme, 900 × 560
- **Evidence:** Confirmed
- **Problem:** Only light captures ever show: with the canvas set to dark, img.shot.lt has display:block and .dk has display:none. The Setup note says 'the light and dark switch in the toolbar swaps both', and the brief asks for light and dark beside each capture. own.css uses :host([data-scheme="dark"]), but the shadow host #uh-host has no data-scheme attribute.
- **Principle:** Visibility of system status; dark mode rechecked [NNG-H1] [mobile:HIG-DARK]
- **Fix:** Key the swap on what the canvas really sets. Either use `@media (prefers-color-scheme: dark)` inside the screen, or read the computed color-scheme that setScheme applies (the .content colorScheme is 'dark'). For example, toggle a `.is-dark` class on [data-screen=doc] from canvas.onResize/size.scheme and select `.is-dark .shot.lt {display:none}`.

### R11 · 2 Minor · New idea fields use placeholders as labels
- **Element:** The app › New idea dialog › Title and Details, 900 × 560
- **Evidence:** Confirmed
- **Problem:** 'Title' and 'Details' are placeholders inside label.field elements with no visible text. Once someone types, as in the draft-kept flow this section draws, nothing on screen names either field.
- **Principle:** Labels or instructions [WCAG 3.3.2] [mobile:NNG-PLACEHOLDER]
- **Fix:** Add visible labels above each field ('Title', 'Details') as <span class="t-label-sm"> inside the existing <label>, and keep the placeholders empty or as an example ('e.g. Dark app icon'). This dialog may belong to another section; if so, carry the fix there.

### R12 · 2 Minor · With Midna down, Terminal rows repeat the Task rows
- **Element:** The app › Search palette › Terminals group, 'Search, typed' and 'Search, nothing typed', 1440 × 900
- **Evidence:** Confirmed
- **Problem:** T3 and T2 appear once under Tasks and again under Terminals, with the same title, the same status pill, and the same action (open the task). That's two to three extra rows of noise on every search, and the Terminals header warning sits below the fold at 900 × 560.
- **Principle:** One fact, one place; every element earns its place [SEC-7] [NNG-H8]
- **Fix:** While Midna isn't running, replace the Terminals rows with one non-selectable line: 'Terminals · 2 · Midna isn't running, so terminals can't be shown'. Drop the rows from arrow navigation. Show the rows again when Midna is back.

### R13 · 1 Cosmetic · Sample data contradicts itself
- **Element:** The app › Undo option toast ('Marked T6 done', 'T6 is open again') and sidebar › Ideas count
- **Evidence:** Confirmed
- **Problem:** T6 isn't on the sample board (T1–T5, T8–T10). The Ideas row shows no count although the board has 3 ideas, while every row above it shows one.
- **Principle:** Match between system and the real world; consistency [NNG-H2] [NNG-H4]
- **Fix:** Use a real task in the Undo option ('Marked T4 done'), and show '3' on Ideas or drop counts from rows where they aren't actionable, consistently.

## What works

- The key table settles the clashes in one place, with a reason for each (⌘4 Backlog / ⌘5 Ideas by sidebar order, ⌘[ ⌘] plus ⌘← ⌘→, plain [ ] only on Days, ⌘7 always on). Every key I pressed did what its menu item says.
- Search has one highlight: pointer and arrow keys move the same aria-selected row, and nothing else lights up. Right-clicking a row opens a context menu next to it, and Open from that menu puts the task in the side panel.
- Unsaved input survives: closing New idea with Esc or the scrim keeps the draft, and ⌘N reopens it with the text intact. Error toasts are role=alert and stay until closed.
- Contrast passes in light and dark at all three sizes (the only flags are exempt disabled menu items). Targets pass WCAG 2.5.8. The Design page reads well at 900 × 560, with a working contents list that moves focus to each section heading.

## Not checked

- Screen-reader output (VoiceOver): R3 comes from reading the code, not from listening.
- The Mac App Fix Plan artifact and the shared-look design weren't available here, so I couldn't check that the drawings cover every fix and proposal in the plan's section, or match the shared look.
- Whether today's captures (T1421) are accurate; I only checked that they load.
- The real macOS system menu bar and Ctrl-F2 behaviour: the in-window strip only stands in for it. Real notification clicks are simulated by the 'Link over a dialog' state.
- Stubbed actions with no handler in the prototype: ⌘↩ Open Pull Request, Task ▸ Start/Stop/Done, Idea menu items, Choose a project, Settings window. I couldn't run the live Undo flow (two Done signals in a row, the 8-second timer) because no in-app action creates an undo. I only saw it in the static 'With Undo' state.
- Long lists: virtualised search rows with 2,000 tasks; the sample board has 8.
- The composition audit script (shared/composition/audit.mjs) wasn't run; I judged composition from screenshots.
- The 'Search, typed a goal ref', 'Nothing found' and 'still looking' states, and Releases-off, at 2560 × 1440 in dark mode. Motion and reduced motion.
- Per the brief, I didn't review the canvas chrome itself (toolbar, title bar, resize handles). LESSONS L1 and L2 had nothing to apply to in this section: there are no grouped file lists and no wave switcher.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
