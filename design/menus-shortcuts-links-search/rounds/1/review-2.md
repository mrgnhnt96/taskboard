## Verdict

All 13 round-1 findings (R1–R13) are fixed in the rebuilt canvas, but the changes left three new problems, one of them Major. This was a fix check of a macOS desktop design (pointer and keyboard) on the local canvas build. I drove it with real mouse and keyboard input through drive.mjs and read fix-1.diff against src. I tested at 900 × 560, 1440 × 900 and 2560 × 1440, in light and dark, at 100% and 200% text. The fixes check out:
- **Focus:** it now returns to the menu title, to Search ⌘K, to the row that opened the panel, or to the page heading after a menu pick.
- **Modals:** the search palette and the New idea dialog make the background inert and keep Tab inside them. Search is now a combobox whose highlighted row is exposed to screen readers, with a polite "8 results" count.
- **Back and Forward:** they skip Releases.
- **Shortcuts in the dialog:** ⌘K, ⌘1–7 and ⌘[ ⌘] pause while it is open.
- **Menu bar:** it is one Tab stop, and ← → move along it.
- **Everything else:** dark captures swap correctly, the dialog has visible labels, the Terminals rows are one line, and the sample data is consistent.

New problems: 0 Catastrophe, 1 Major, 2 Minor, 0 Cosmetic. The top one (R14): pressing ⌘N while New idea is open wipes the typed title. R5's fix paused every other window command in the dialog but left ⌘N live, and its handler throws the draft away. The two Minor ones are both caused by the changes: with a tooltip showing, Esc takes two presses to close the dialog or palette (R15), and Enter on the search scope chip's × opens the top result instead of widening the search (R16).

## Findings

### R14 · 3 Major · ⌘N inside the New idea dialog wipes what was typed
- **Element:** The app › New idea dialog › Title and Details, with the dialog open, then ⌘N; 1440 × 900, light
- **Evidence:** Confirmed
- **Problem:** I typed 'Dark icon' in Title and pressed ⌘N. The dialog redrew with empty fields. Esc and ⌘N again still showed an empty Title, so the draft was gone for good. R5's fix paused ⌘K, ⌘1–7 and ⌘[ ⌘] in the dialog but left ⌘N live. run('new-idea') sets A.dialog = A.draft || {}, and A.draft is still null while the dialog is open, so the live input is replaced. This is the one data-loss path left in the 'keys while a dialog is open' rule.
- **Principle:** Unsaved input survives in-app actions; error prevention [NNG-H3] [NNG-H5]
- **Fix:** In run('new-idea'), if A.dialog is already open, just focus the Title field and return; never rebuild the dialog from A.draft. Add 'n' to the keys that pause while a dialog is open (the dialog-pause check in the keydown handler and the File › New Idea… item) for consistency, and add a Testing item: '⌘N with New idea open keeps the draft'.

### R15 · 2 Minor · Esc needs two presses when the focused control shows a tooltip
- **Element:** The app › New idea dialog › Close (×); Search palette › scope chip × ('Search everything'); any keyboard-focused control with a focus tooltip; 1440 × 900
- **Evidence:** Confirmed
- **Problem:** Tabbing to the dialog's Close button shows its tooltip ('Close Esc'). The first Esc only hid the tooltip and the dialog stayed open; a second Esc closed it. The tooltip itself says Esc closes the dialog. Before this round, a capture-phase handler hid the tooltip and the same keypress went on to close the layer. The diff removed that handler and made hideTip the first branch of the Escape chain, so it now eats the keypress.
- **Principle:** Esc cancels transient UI in one press; consistency [MS-KBD] [NNG-H4] [WCAG 1.4.13]
- **Fix:** In the Escape handler, call hideTip() when a tooltip shows and then carry on down the chain (menu › context menu › palette › dialog › panel) in the same keypress. Only stop after hiding the tooltip when nothing else is open.

### R16 · 2 Minor · Enter on the scope chip's × opens the top result instead of searching everything
- **Element:** The app › Search palette › scope chip × ('Search everything'), state 'Search, in a goal', 1440 × 900
- **Evidence:** Confirmed
- **Problem:** The new Tab trap makes the × reachable: Tab from the field lands on it. Pressing Enter there opened the highlighted task in the side panel and closed search. Space on the same button correctly widened the search. The palette's Enter handler clicks the selected row whatever has focus, so a keyboard user who chose 'Search everything' gets an unrelated task instead.
- **Principle:** No surprises; a control does what its name says [WCAG 3.2.2] [WCAG 2.1.1] [NNG-H4]
- **Fix:** In the palette's Enter (and ↑ ↓) branch of the keydown handler, act only when the event target is the search input (e.composedPath()[0].matches('.pal-in input')). Otherwise let the focused button's default action run.

## What works

- R1 fixed: Esc from an open menu returns focus to its menu title, and a View pick focuses the new page's h1. Closing search returns to Search ⌘K, closing the panel returns to the row link that opened it, and 'Focus in Midna' keeps focus on the button.
- R2 fixed: with search or the dialog open, .mbar, .side, .main and .panel are inert (inspect shows 1 tab stop in search) and .wtoasts stays live. Tab cycles Title › Details › Choose a project › Add idea › Close and wraps. .pal has aria-modal.
- R3 fixed: the input is role=combobox with aria-controls and aria-expanded. aria-activedescendant follows ↑ ↓ (opt-tasks-T3 › opt-tasks-T4). Rows are div role=option inside role=group labelled by the group header, and a polite live region reads '8 results'.
- R4 fixed: ⌘3 ⌘7 ⌘[ returns to Goals; ⌘7 ⌘2 ⌘[ returns to Goals, and Releases never joins history.
- R5 mostly fixed: with the dialog open, ⌘K, ⌘3, ⌘[ and ⌘← did nothing and no palette stacked; the key table now states the rule. ⌘N is the remaining hole (R14).
- R6 fixed: the menu bar is one Tab stop with a roving tabindex. ← → move between titles, ↓ Enter open a menu, ← → inside an open menu switch to the neighbouring menu, and ⌃F2 is documented.
- R7 fixed: the Solution rule now says 'greyed when nothing is open for it to act on'. Open Pull Request is disabled for T3 in the Task menu, and the context menu uses the same hasPr check. A repeated Midna error flashes the existing toast instead of stacking a second.
- R8 fixed: at 900 × 560 and 200% text the goal column drops, titles wrap, and inspect on the palette reports no cut text.
- R9 fixed: the Midna status is a focusable button with a warning icon, in neutral text, not the 'needs' pill. Its tip shows on click and on keyboard focus, Esc hides it, and aria-describedby points to the sentence.
- R10 fixed: Dark in the toolbar sets .is-dark on the Design page, and all 12 dark captures show and load when scrolled into view; Light switches back.
- R11 fixed: Title and Details have visible labels above the fields, in light and dark.
- R12 fixed: Terminals is now one non-selectable line, 'Terminals · 2 · Midna isn't running, so terminals can't be shown', with no rows to arrow through.
- R13 fixed: the undo toasts use T4, and the sidebar Ideas row shows 3.

## Not checked

- Screen-reader output (VoiceOver): the R3 and R9 fixes were checked from the attributes and the DOM, not by listening.
- Greyed View menu items while a dialog is open: the menu bar is inert then, so they can't be opened to look. Their disabled state was checked in code only.
- The real macOS menu bar and ⌃F2: the in-window strip only stands in for them.
- The live Undo flow (two Done signals, the 8-second timer): no in-app action creates an undo, as in round 1.
- The row context menu's Open Pull Request state was checked in code (same hasPr lookup as the Task menu). I didn't right-click a .trow row, since Needs you has none.
- Everything outside R1–R13 and the diff, per the brief. I didn't review the canvas chrome (toolbar, title bar, resize handles).
- Whether the dark captures match the T1421 capture task; I only checked that they swap and load.
- The dialog's fields still report field-edge 1:1 on the input itself, but the border is drawn by the wrapping .field. This was the same in round 1 and is outside scope.
- LESSONS L1 and L2 (group labels, wave switchers) had nothing in this fix to apply to.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
