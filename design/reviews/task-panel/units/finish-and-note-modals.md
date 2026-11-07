## Verdict

macOS desktop app, pointer and keyboard, compact-to-large window. Reviewed the Finish confirm (Mark done, Cancel, Merge now) and the Note for testers dialog from 10 captures of the real app (1360x860 in light and dark; Mark done also at 900x560 and 1728x1084) plus the code. Busy, failed and already-finished states were not captured, so they were judged from code alone. No browser page exists for this native app, so nothing was driven with real input; the browser was used only to compute contrast. First impression: the dialogs are calm and well organised, with one heading, one line of identity and one action, and the board drops back cleanly behind the scrim. The trouble is in what they leave out and how they behave. Counts: 0 Catastrophe, 4 Major, 3 Minor, 2 Cosmetic. Top problem: closing or pressing Esc while Merging… looks like backing out, but the override merge still goes ahead, and its result is then dropped with no toast, no refresh and no error. Merge now also never says what it overrides.

## Findings

### R1 · 3 Major · Closing during busy doesn't stop the action and throws away its result
- **Element:** Finish confirm › Close button and Esc, in the busy state (Marking done…, Cancelling…, Merging…), all sizes and themes
- **Evidence:** Confirmed
- **Problem:** Close stays live and Esc still closes while the request runs, so it reads as backing out, but the request has already been sent and carries on. When it returns, the handler finds no Finish modal and returns early: no success toast, no failure note and no m.refresh. A merge past review can land, or fail, with no word to the owner. If a new Finish dialog for the same task is open by then, the old result closes it. Proven by code: close() at modals.rs:101-103 and on_escape at 139-143 ignore f.busy; the early return is at modals.rs:294, and the ref check at 295-297 passes for the same task. Not seen running.
- **Principle:** Visibility of system status, and user control: a command must report its outcome, and an exit must not imply a cancel that didn't happen [NNG-H1] [NNG-H3]
- **Fix:** Pick one of two. Simplest: while f.busy, make on_escape do nothing and draw Close with kit::disabled and no on_click, so the dialog stays until the answer arrives. Alternative: let it close, but move the toast and m.refresh(cx) ahead of the modal check at modals.rs:294, and show failures as an error toast naming the task ("Couldn't merge T2's PR: <reason>"). Either way, carry an id on the request so a late result can't close a newer dialog.

### R2 · 3 Major · The confirm repeats the title instead of showing what the action will do
- **Element:** Finish confirm › body, Merge now most of all (merge-now-modal__light__1360x860.png, merge-now-modal__dark__1360x860.png); also finish-done and finish-cancel in both themes
- **Evidence:** Confirmed
- **Problem:** All three forms show only the task title. Merge now is an override past whatever holds the PR (modals.rs:31, 288), and in the capture the panel says 'Waiting on you' and 'Waiting on your review', yet the dialog names neither the hold nor the PR's own title. It also uses the same layout as a routine confirm. Mark done and Cancel likewise skip Midna's approval (modals.rs:22) on a task whose agent is still Working in the capture, and the dialog doesn't say so. With many agents running, the owner confirms an irreversible step from a line that tells them nothing new.
- **Principle:** Error prevention: a destructive confirm must state the specific consequence [NNG-H5] [NNG-H3]
- **Fix:** Add one line of live data, not helper text, so it fits the owner's no-filler rule. Merge: the PR title, then the holds it skips, such as 'Add passkey sign-in to the account page' and 'Skips: your review' (the Checks and Review values the panel already has, listing only what is unmet). Done and Cancel: the current state, such as 'Working · 5m'. Put it under finish-name at modals.rs:259 in muted 12.5px, and omit the line when there's nothing to skip.

### R3 · 3 Major · Dialog boxes are cut 10% short and sit off-centre, clipping the note's last line
- **Element:** kit::modal_box, all dialogs: Note for testers › text (note-modal__light__1360x860.png, note-modal__dark__1360x860.png) and Finish confirm › bottom padding (all finish captures)
- **Evidence:** Confirmed
- **Problem:** The Note dialog ends mid-line through 'audit log lists each change with its time.' at about 245px tall in an 860px window, with no scrollbar, fade or padding to show there is more. It is the view you open to read the whole note, and it is the one thing it fails to show cleanly. Measured from the captures, the boxes are 414 and 601px wide, not the 440 and 640 coded: 94% of the width, as in max_w(0.94). The same happens to height. Bottom padding under the Finish button is about 6px, not the 20 coded (modals.rs:258), the box's centre sits about 3% of its width left of the window's and about 6% of its height above, and all three window sizes agree. That points to the percentages in kit.rs:232-233 resolving against #modal-body's content-sized box (modals.rs:132), not the window; that cause is inferred from the measurements. A long failure note makes the Finish box taller, so the clip can eat into the action button, since its body is flex_none with no scroller. Alerts and New idea share this shell.
- **Principle:** Text must not be cut off or lost, and the system should show there is more [WCAG 1.4.4] [NNG-H1] [SCR-4]
- **Fix:** Remove max_w(relative(0.94)) and max_h(relative(0.9)) from kit::modal_box (kit.rs:232-233). Set the limits once on #modal-body (modals.rs:132), which sits in the full-size scrim: max_w(relative(0.94)), max_h(relative(0.9)), and make the box a shrinking flex child with min_h_0. Then the note grows to about 774px at 860 tall before it scrolls, the Finish box keeps its 20px padding, and the box centres. Add a visible scrollbar to #note-all for notes that really do overflow, and make the Finish body scroll so the button can't be clipped.

### R4 · 3 Major · Dark theme's Mark done label fails AA contrast
- **Element:** Finish confirm › Mark done, dark, 1360x860 (finish-done__dark__1360x860.png); every kit::btn_primary in dark
- **Evidence:** Confirmed
- **Problem:** White 13px medium text on accent_btn #4d72f0 computes to 4.22:1, under the 4.5:1 needed for text this size. Light passes at 5.11:1 on #3563e9. It's the main action of this dialog and the token is shared with every primary button in dark, at theme.rs:144.
- **Principle:** Contrast minimum 4.5:1 [WCAG 1.4.3]
- **Fix:** Set dark accent_btn to #4166ea at theme.rs:144, which gives 4.87:1 with white text. Its fill stays 3.40:1 against the #1a1f26 card, so the button still reads as a button.

### R5 · 2 Minor · The dialog's action can't be reached from the keyboard in the code given
- **Element:** Finish confirm › Mark done, Cancel task, Merge now, Close; Note for testers › Close and text scroller
- **Evidence:** Likely
- **Problem:** Nothing in the supplied code gives the dialogs focus. Merge now and Show all open by setting m.modal and calling cx.notify (task_panel.rs:537-540 and 688-692), so focus doesn't move in. btn_base (kit.rs:129-149) has no focus handle or focus style, finish() binds only on_click, and #note-all can't take focus, so Tab, Return, Page Down and arrows have no target here. A person running this all day with hands on the keys has to reach for the mouse to confirm. The success path at modals.rs:300 also clears the modal without the window.focus call that the alerts path uses at line 111. Likely, because app.rs wasn't supplied and confirm_finish is pub, so a binding might exist there. Rated by the code seen: if none exists this is a 3 Major, as a WCAG failure on a primary flow.
- **Principle:** All functions work from the keyboard, and focus moves into a dialog and returns to the opener [WCAG 2.1.1] [WCAG 2.4.3] [MS-KBD] [desktop:APG-DIALOG]
- **Fix:** Give the modal a FocusHandle and focus it on open, through set_modal rather than a bare assignment. Bind Return to confirm_finish for Mark done only; leave Cancel task and Merge now off Return, so a stray keypress can't trigger them, and make them Tab-reachable with a 2px accent focus ring. Focus #note-all on open so arrows and Page Up/Down scroll it. Return focus to m.focus after success.

### R6 · 2 Minor · A failed attempt removes the button, so there's no retry
- **Element:** Finish confirm › failure state (finish-note), judged from code; not captured
- **Evidence:** Confirmed
- **Problem:** Any failure sets f.note (modals.rs:308-311), and line 248 removes the button whenever a note exists. That is right for 'T2 is already done', but a dropped connection or board refusal gets the same treatment, so the owner has to Close and find the link again. The heading still asks 'Merge PR #944 now?' with nothing left to answer it, and the raw backend message is the only text.
- **Principle:** Help users recover from errors; no dead ends [NNG-H9] [NNG-H3]
- **Fix:** Separate the two kinds of note: keep already() results as final, and hold transport or refusal errors in a new field such as f.error. For f.error, keep the button and relabel it 'Try again', with the note above it. For final notes, change the title to the fact, such as 'T2 is already done', since there's no question left.

### R7 · 2 Minor · A confirm opened from a terminal link can show only a ref
- **Element:** Finish confirm › finish-name, when the task isn't the one in the panel; not captured
- **Evidence:** Likely
- **Problem:** The title comes from the open task or task_card (modals.rs:230-231), and finish-name is simply left out when neither matches (line 259). A link for T6 opened while T2 is in the panel can read 'Cancel T6?' above a button, with T2's panel still visible behind it, so the owner may cancel the wrong agent's task. The already() check is also skipped until the panel has that task (line 232), so the button can show for a finished task; confirm_finish does recheck before acting. Whether the link path loads the task first isn't in the code given.
- **Principle:** Recognition rather than recall: the confirm must identify its object [NNG-H6] [NNG-H5]
- **Fix:** Fetch tasks/{ref} when the dialog opens. Until it returns, show the button as disabled with kit::disabled, then fill in the title and run already(). If the lookup fails, show 'Couldn't load T6' with no action.

### R8 · 1 Cosmetic · Busy button doesn't use the kit's disabled look
- **Element:** Finish confirm › action button, busy state; not captured
- **Evidence:** Confirmed
- **Problem:** Busy uses opacity(0.6) (modals.rs:241-245) while keeping the pointer cursor and hover fill, so it still looks clickable though no handler is attached. Its danger label drops to 3.08:1 in light and 2.91:1 in dark, which is exempt as disabled but is the only status on screen.
- **Principle:** Disabled controls look disabled, and one look per role [NNG-H1] [SEC-4]
- **Fix:** Use kit::disabled for the cursor, and drop the hover with a plain non-hover variant. Set opacity to about 0.7 so the 'Merging…' text stays readable.

### R9 · 1 Cosmetic · The way out sits apart from the action and isn't a clear 'no'
- **Element:** Finish confirm › Close (24px, title row) and action button (30px, bottom right), all captures
- **Evidence:** Confirmed
- **Problem:** A macOS confirm puts decline next to confirm. Here the decline is a small button in the heading row and the answer to 'Cancel T2?' is 'Close', so the pointer travels and the pair doesn't read as a choice. It is learnable, and avoiding 'Cancel' next to 'Cancel task' is sensible.
- **Principle:** Platform convention for alert buttons [NNG-H4] [HIG-MACOS]
- **Fix:** In the Finish form only, drop the head's Close and put a kit::btn labelled 'Keep task' (Cancel form) or 'Not now' (Done, Merge) left of the action in the bottom row, gap 8px.

## What works

- Titles carry both the verb and the object ('Cancel T2?', 'Merge PR #944 now?'), and button labels repeat the verb, so the action reads without the surrounding context.
- 'Close' rather than 'Cancel' avoids a Cancel versus Cancel task collision, and the backdrop ignores clicks, so a stray click can't lose or fire anything.
- The re-check in confirm_finish looks the task up again before acting, so a task finished in the meantime gets a note rather than a double action, and the busy flag blocks double submits.
- Risk is well weighted: routine Mark done is filled, while Cancel and Merge use red outlined buttons. Body text is 16.17:1 in light and 13.61:1 in dark, the error note colour is 6.57:1 and 5.80:1, and the dialog holds at 900x560 with no overlap.
- No owner rule is broken: no filler text, nothing editable beyond the signals, and the dialogs stay within the right-panel model.

## Not checked

- Busy, failed, and already-finished states were not captured; R1, R6 and R8 are judged from code, with their rendering unseen.
- Real input: this is a native GPUI app, so no control was clicked, typed or scrolled, and Esc behaviour was not exercised. The browser was used only to compute contrast.
- app.rs and set_modal were not supplied: the CloseOverlay key binding, how Esc is ordered against task_panel::escape and the panel underneath, any Return, Tab or focus handling, whether keyboard shortcuts reach the board behind an open dialog, and whether toast messages are announced.
- Note dialog at 900x560 and 1728x1084, and notes much longer than the sample. Whether #note-all already scrolls the clipped line into view, and draw_note's behaviour (text selection, copy, links), since its code wasn't supplied.
- Long titles and long backend errors in the Finish confirm; the clipping of a long failure note in R3 is inferred from the shell, not seen.
- Dialog sizes are measured from the captures at their image scale, because shell image tools were unavailable; figures are accurate to about 1-2px.
- VoiceOver output, roles and names (the supplied code shows no accessibility labels). 200% text and system text-size settings, motion, and real-device display.
- Composition audit script and blurred screenshot: not runnable on a native app, so composition was judged by eye from the captures.
- Sources: HIG-MACOS and desktop:APG-DIALOG are cited from the checklist and were not re-read. One reviewer misses problems others catch, so a second reviewer or a usability test is worthwhile.
