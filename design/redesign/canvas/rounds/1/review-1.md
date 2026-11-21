## Verdict

This is a macOS desktop app: a sidebar, a single main column and a right details panel. I reviewed the ultrahand canvas build with real input in headless Chrome at the laptop-125 preset (1536 × 864, large width) and the 720 × 480 minimum window, in light and dark, at 100% and 200% text, and in the default, down, empty, setup and loading states. I sampled about 15 of the 58 screens with inspect and screenshots and read every source file. First impression: the blurred Needs-you screen reads as calm and ordered. The work column dominates, the chrome is quiet, there is one ink primary (Send) and four type sizes, and status always pairs a glyph with words. Composition is not the problem. Behaviour and scope are. Counts: 2 Catastrophe, 3 Major, 6 Minor, 1 Cosmetic. The top problem is that a typed answer to an agent's question is silently wiped by any other action on the screen, including picking an answer option and the 7-second toast expiring. A close second: the sidebar pages, active goals and every Settings section are links that ignore Enter and Space, so a keyboard user can't reach 8 of the 9 Settings sections. The design also offers many changes the owner's read-only rule doesn't allow, and DESIGN.md says it doesn't.

## Findings

### R1 · 4 Catastrophe · A typed answer to an agent's question is wiped by any other action
- **Element:** Needs you › Decide › T3 question › Other answer field (same in the panel's attention banner), laptop-125, light, default
- **Evidence:** Confirmed
- **Problem:** I typed "Only for admins, ask Sam" into Other answer, then pressed Dismiss on another row: the field came back empty. I typed again and picked the "Yes, always" option: empty again. The whole page re-renders on every action and on every toast expiry, which runs a 7-second timer, so the owner's answer is lost without warning in the middle of the main task.
- **Principle:** Unsaved input survives in-app actions [NNG-H3] [NNG-H5]. Severity guide: data lost in an in-app action is 4.
- **Fix:** Keep the draft in state per question (S.answerDraft[ref]), save it on input and restore it on every render, both inline and in the panel. Keep it until Send succeeds or the user clears it. Show both as one choice: a typed answer deselects the options and vice versa, and the Send label names what will be sent.

### R2 · 4 Catastrophe · Sidebar pages, active goals and Settings sections ignore Enter and Space
- **Element:** Sidebar › page list and Active goals; Settings › section list; Settings › Back to the board; goal page › breadcrumb, all sizes
- **Evidence:** Confirmed
- **Problem:** These are <a role="link" tabindex="0"> with no href and no key handler. Tab reaches "Goals, 4", but Enter and Space do nothing (the screen stayed on needs). In Settings, Enter on History left Accounts current. ⌘1–⌘7 cover the 7 pages, but the active-goal links, all nine Settings sections and Back to the board can only be used with a mouse. The breadcrumb links aren't focusable at all.
- **Principle:** Keyboard [WCAG 2.1.1]; role and behaviour match [WCAG 4.1.2]; desktop keyboard [MS-KBD]
- **Fix:** Render these as <button> (or <a href> with a real target) so Enter and Space activate them. Keep aria-current="page". Make the sidebar and the Settings list one tab stop each, with ↑/↓ moving inside them. Give Settings sections ⌘1–⌘9 while the Settings window is open, and make the breadcrumb a focusable button.

### R3 · 3 Major · Many changes the owner's read-only rule doesn't allow are offered as buttons
- **Element:** Whole app: Needs you rows and ⋯ menus, the task panel, Backlog bulk bar and row menus, the Plan page, Goals, the goal page, Ideas, the merge dialogs, the Health popover
- **Evidence:** Confirmed
- **Problem:** The owner's rule allows only start/stop/ack, New idea, idea close and answer, human-task answers, Undo, typed answers and Settings hours, pool and Jira. The canvas also offers: Merge now (merge a PR), Make a task, Make it a Jira ticket, Won't do and Defer on issues, bulk Move into goal, Priority, Defer and Won't do, Plan into waves, Plan an idea, Plan anyway, Pause, Resume and Start a goal, Bring back from Set aside, Continue to wave 3 and Continue anyway, Try again, Close and Detach terminal, Check and finish, Mark T2 done from a link, Ask it to rebase, Shelve and Dig again, and Restart the board. DESIGN.md says "Everything else shows the tb command", so the design contradicts both its own rationale and the owner's rule, and the owner can't approve it as is.
- **Principle:** The owner's stated rule; consistency between the design and its stated model [NNG-H4]
- **Fix:** Remove or convert each of these. Replace the action with a read-only line plus a copyable command (for example "Merge: tb pr merge 412", with a Copy button) in the same place the button sat. Keep only the allowed set. Then reconcile DESIGN.md's list with the canvas, and list any action you still want as an open question for Morgan instead of building it.

### R4 · 3 Major · With the board down, menu and dialog commands still report success
- **Element:** Needs you › Cache session lookups › ⋯ › Mark reviewed, State = down, laptop-125
- **Evidence:** Confirmed
- **Problem:** In the down state, row buttons show "The board is offline". The same command run from the ⋯ menu instead showed "Marked PR #412 reviewed · Undo" and removed the row. The offline guard only covers .main .end, .main .body, the panel's attention banner and .ph .acts. Menus, context menus, dialogs (Merge now, Finish G9), the popovers, the backlog bulk bar and the wave gate's "Continue to wave 3" all bypass it. Every control also looks enabled while the board is offline.
- **Principle:** Visibility of system status: in the offline state, commands never report success they didn't have [NNG-H1]; error prevention [NNG-H5]
- **Fix:** Enforce offline in one place: in the down state, give every mutating command (buttons, menu items, dialog OK buttons) aria-disabled="true" and a disabled look, with a tooltip "The board is offline". Keep Open, Copy and Restart the board enabled. Never run the success path while isDown().

### R5 · 3 Major · Undo only covers the last action, expires in 7 s, and focus is lost after a removal
- **Element:** Needs you rows › Dismiss, I reviewed it, Done, Make a task and every toast with Undo, laptop-125
- **Evidence:** Confirmed
- **Problem:** I dismissed T5 and then marked T6 reviewed. Undo brought back only T6; T5 stayed gone (S.lastGone holds one ref). The toast closes after a fixed 7 s with no pause on hover or focus and no ⌘Z. After each removal and after Undo, focus went to body, so a keyboard user loses their place in the list. Undo is the design's stated substitute for confirmations, so these gaps carry weight.
- **Principle:** Undo holds up: lasts, stacks, can be extended [WCAG 2.2.1] [NNG-H3]; focus moves to the next item [WCAG 2.4.3]
- **Fix:** Keep an undo stack and bind ⌘Z and Edit › Undo to it. Pause the toast timer while the pointer or focus is on it, and lengthen it to 10 s. After a row is removed, move focus to the next row's title, or the previous one at the end. After Undo, put the row back in place and focus its title.

### R6 · 2 Minor · Closing a dialog drops focus to the page instead of the control that opened it
- **Element:** Merge PR #412 without waiting? (from ⋯ › Merge anyway…), New idea, and the other dialogs, laptop-125
- **Evidence:** Confirmed
- **Problem:** Esc on the Merge dialog and Esc on New idea both left focus on body. The keyboard user has to tab again from the top of the window to get back to the row.
- **Principle:** Focus order: closing a dialog returns focus to its opener [WCAG 2.4.3] [desktop:APG-DIALOG]
- **Fix:** Record the opener when a dialog opens (the menu's anchor button, or the toolbar's New idea button). In close-dlg and dlg-done, set S.focusNext to it. If the opener is gone (the row was removed), focus the next row's title.

### R7 · 2 Minor · Bulk Defer and Won't do disappear below 840 px with no overflow
- **Element:** Backlog · 3 selected › bulk bar, min window 720 × 480
- **Evidence:** Confirmed
- **Problem:** Below 840 px the bulk bar sets display:none on Defer (D) and Won't do (X) and adds no ⋯ overflow. The commands still fire from their hidden key shortcuts, but nobody can see them. DESIGN.md says secondary actions go behind ⋯ at compact width.
- **Principle:** Every control at every size [NNG-H6]; feature missing at one size class [tablet:AND-WSC]
- **Fix:** Below 840 px, replace the hidden pair with a ⋯ button that opens a menu holding Defer (D) and Won't do (X). Or drop both actions if R3 removes them.

### R8 · 2 Minor · At 200% text, row and goal titles are cut short and overlap their refs and actions
- **Element:** Needs you › rows; Sidebar › Active goals, laptop-125, text 200%
- **Evidence:** Confirmed
- **Problem:** inspect lists ellipsis cuts of 91–234 px ("Rate lim…", "Passwo…", "Review found an SQL strin…"). The row titles overlap the ref, the age and the main button (B44, B41, H1, T10), and in the panel "Ran on commit" overlaps "Started". At this size the owner can't tell the goals apart in the sidebar.
- **Principle:** Resize text: nothing cut off or overlapping [WCAG 1.4.4]
- **Fix:** Let row titles wrap to 2 lines (no white-space:nowrap or text-overflow:ellipsis). When the font scale is above 1.5, move the age and the action buttons to a second line under the meta. Let the sidebar goal names wrap and put the "1 of 5" count below the name.

### R9 · 2 Minor · In dark mode, answer option and field edges are too faint on the amber card
- **Element:** Needs you › T3 question (inline and in the panel banner) › answer options and Other answer field, dark
- **Evidence:** Confirmed
- **Problem:** inspect measures the edges of the three answer options and the Other answer field at 2.52:1 against the dark amber card (#3a2c14), so the choices barely read as separate controls.
- **Principle:** Non-text contrast 3:1 [WCAG 1.4.11]
- **Fix:** In dark mode, raise the option and field border on the needs card to at least 3:1 against #3a2c14, for example #7a6a4c. Check the value with the contrast formula. Or give the options a solid surface fill (--surface-raised) inside the card.

### R10 · 2 Minor · The empty state contradicts the open panel
- **Element:** Needs you · T3 asks, State = empty, laptop-125
- **Evidence:** Confirmed
- **Problem:** The page says "Nothing needs you", while the panel beside it shows T3 "Needs you" with an open question. The sidebar still says "Midna at its limit" and "4 agents are working" with no active goals listed. The two areas disagree about the same fact.
- **Principle:** One fact, one place [SEC-7]; visibility of system status [NNG-H1]
- **Fix:** In the empty state, close the panel (or show its own empty state: "Pick something to see it here"). Make the sidebar footer and the empty message read from the same data, for example "No agents working" with no goals.

### R11 · 2 Minor · Settings replaces the board instead of opening its own window
- **Element:** Settings (all nine sections), ⌘,
- **Evidence:** Confirmed
- **Problem:** ⌘, swaps the main window for a Settings layout with "Back to the board" (⌘[). On macOS, Settings is a separate window and the app's window stays as it was, so the owner loses their place and can't look at the board while changing work hours. It also takes ⌘[ for Back, which in this app already means history Back.
- **Principle:** macOS conventions for Settings [HIG-MACOS] [desktop:HIG-WINDOWS]; consistency [NNG-H4]
- **Fix:** Open Settings in its own window, about 720 × 520, with the nine sections in a sidebar or as toolbar tabs. Close it with ⌘W or Esc. Leave the main window untouched and drop "Back to the board".

### R12 · 2 Minor · "Plan these" only selects the group
- **Element:** Backlog › group header › Plan these
- **Evidence:** Confirmed
- **Problem:** The button runs pickgrp: it ticks the group's checkboxes and leaves the user on the list. The label promises planning, which happens only after a second click on Plan into waves. It also does the same as the group checkbox beside it.
- **Principle:** Users associate the action with the result [NNG-CW]; match with the real world [NNG-H2]
- **Fix:** Either make it go straight to the Plan page with that group's issues selected, or rename it "Select group" and remove it, since the checkbox already does that.

### R13 · 1 Cosmetic · DESIGN.md describes different home groups than the canvas
- **Element:** DESIGN.md › Information architecture and Key decision 1 vs Needs you
- **Evidence:** Confirmed
- **Problem:** The rationale lists Answer, Review, Fix, Start, Your to-do and Goal outcomes. The canvas shows Agents waiting, Decide, Your to-dos, Stopped, Ideas waiting and Later. The reviewer of the rationale gets a different IA than the design.
- **Principle:** Consistency [NNG-H4]
- **Fix:** Update DESIGN.md's table and decision 1 to the groups the canvas uses, or change the canvas to match.

## What works

- The Needs-you screen passes the squint test. The work column dominates and there is one ink primary (Send). The composition audit found 4 type sizes, one UI font plus mono, no accent sprawl and no lookClickable labels.
- Status is never colour alone: every state has a distinct glyph plus words.
- The list keyboard model works. ↑/↓ move between row titles, Shift+F10 opens the row menu with focus on its first item, arrows move inside it, and Esc returns focus to the ⋯ button. Esc on the panel returns focus to the row.
- The New idea draft survives Esc: a toast offers Reopen (⌘N) and the text comes back. The Merge dialog opens with focus on Cancel, not on the destructive Merge now.

## Not checked

- Presets laptop-hd, macbook, desktop-fhd, desktop-qhd (the largest size), snap-half, mac-half and browser: only laptop-125 and the 720 × 480 minimum window were measured. The composition audit ran at laptop-125 only, not at the largest and smallest sizes.
- Not every screen and state: of the 58 screens × 5 states, about 15 combinations were inspected or screenshotted. Releases, Projects, Days and Week, Recaps, Ideas, Plan, the Settings sub-pages and most panel variants were read in code but not measured.
- Whether FEATURES.md (217 KB) is fully covered: I didn't check its feature list item by item against the canvas. DESIGN.md's mapping was taken as given.
- The macOS menu bar (File, Edit, View, Go, Task, Window, Help) and its shortcuts: the canvas doesn't draw it, so 'every command in a menu' couldn't be checked.
- The canvas reports platform "windows" and draws a Windows title bar for the desktop presets. That is review chrome, but it means macOS window chrome, traffic lights and the full-screen behaviour weren't seen.
- The native select popup in New idea › Project (headless can't open it); screen-reader output, VoiceOver and announcements of the role=status toasts; motion; real devices and trackpad gestures.
- Dragging the panel divider: keyboard ←/→ is in the code but wasn't driven. It also lacks Home and End and Enter to collapse.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
