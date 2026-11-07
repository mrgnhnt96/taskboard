## Verdict

macOS desktop, pointer and keyboard. Reviewed the Human box from 8 captures (to-do, waiting and done in light and dark at 1360x860; to-do in light at 900x560 and 1728x1084) plus the code excerpts. The app itself wasn't run, so no control was used with real input, and contrast was computed from theme.rs tokens rather than measured on pixels. First impression: the panel reads as organised. There is one warm box, one blue Done and a clear title, and the finished state is calm. But in the to-do state a second peach band sits on top of the box and competes with it, and the waiting state looks identical to a task you can act on. Counts: 0 catastrophe, 5 major, 5 minor, 2 cosmetic. Top problem: the first line of a human task says "It's waiting for you in Midna.", which sends the owner to a terminal the task doesn't have, when the work is in the box right below. The note field is close behind: it is blank and unnamed, yet the steps depend on it and Done sends it to the waiting agents, with no way back.

## Findings

### R1 · 3 Major · The banner sends the owner to Midna for a task that has no terminal
- **Element:** Task panel › banner above Yours to do, state to-do, all sizes and both themes (human-to-do__light__1360x860.png, human-to-do__dark__1360x860.png, human-to-do__light__900x560.png, human-to-do__light__1728x1084.png)
- **Evidence:** Confirmed
- **Problem:** The first thing the owner reads is "It's waiting for you in Midna.", but a human task has no terminal and its actions are in the box below. That is a wrong-path step at the very start of the flow. It also stacks a second peach surface on top of Yours to do, says the same thing twice, and pushes Done down by about 69pt. The cause is the generic fallback at view.rs:1587, which runs before the human return at view.rs:1590-1592. The comment on that return even says a human task has no terminal to answer in.
- **Principle:** Cognitive walkthrough Q3, association of action and result [NNG-CW]; match with the real world [NNG-H2]; one fact, one place [SEC-7]
- **Fix:** In banner() (view.rs:1586-1592), for kind == human, return None unless question or latest has real text, so the fallback string is never shown. Keep the banner only for a real question, and show ask_options there as now. Yours to do then sits directly under the header.

### R2 · 3 Major · The note field has no name, though the steps and Done depend on it
- **Element:** Task panel › Yours to do › note field, states to-do and waiting, all sizes (human-to-do__light__1360x860.png, human-waiting__light__1360x860.png)
- **Evidence:** Confirmed
- **Problem:** Step 4 says "in the note", but nothing on screen is called a note. The field is an empty box with placeholder "" (view.rs:1058). The only statement of what happens to the text is Done's hover tooltip (view.rs:1060). The owner has to guess that this box is where the Key ID goes and that agents will read it. Node::Field has no label property at all, so VoiceOver has no name to read either.
- **Principle:** Visible labels or instructions [WCAG 3.3.2]; name, role, value [WCAG 4.1.2]; recognition rather than recall [NNG-H6]
- **Fix:** Add a visible label above the field, "Note for the agents", in St::Small, and pass the same string as the field's accessible name. A placeholder would not do, because it disappears while typing. The field's placeholder was left blank, which looks tied to the no-filler rule. This label names a control rather than padding the screen, but it touches that rule: needs the owner's OK.

### R3 · 3 Major · A blocked task looks and acts like one you can do now
- **Element:** Task panel › Yours to do, state waiting, both themes (human-waiting__light__1360x860.png, human-waiting__dark__1360x860.png)
- **Evidence:** Confirmed
- **Problem:** The only difference from the to-do state is one small line, "Starts once T5 Set up signing is done". The box still says Yours to do, keeps the alert tone, and offers an enabled primary Done (view.rs:1060 passes disabled = false in every state). So the owner can follow the steps too early, or press Done on a task whose prerequisite isn't finished. "Starts" also misleads, since nothing starts by itself on a human task. T5 is plain text (view.rs:1050), so the owner can't open the task they are waiting on. Whether the server accepts /did in this state wasn't visible.
- **Principle:** Visibility of system status [NNG-H1]; error prevention [NNG-H5]; recognition rather than recall [NNG-H6]
- **Fix:** When waiting is set and status is queued, render the box in BoxTone::Quiet with the label "Waiting on T5" and the title beside it as a link using the existing open-task act (task_panel.rs:435). Keep the steps readable. Set Done to Look::Plain and disabled, with the tip "Opens once T5 is done". Keep the note field so a draft can be started early.

### R4 · 3 Major · Done can't be taken back, and it goes through with an empty note
- **Element:** Task panel › Yours to do › Done, then the Done by you box (human-to-do__light__1360x860.png, human-done__light__1360x860.png)
- **Evidence:** Confirmed
- **Problem:** One click on Done posts /did with whatever is in the field, including nothing (task_panel.rs:572-588), and anything waiting goes on. If the task's steps ask for values, as this one does for the Key ID and Issuer ID, the agents continue without them. The finished box then has no action at all (view.rs:1041-1046), so the owner can't correct or add to the note, or reopen the task. A slip on the primary button of the flow has no way back from this panel. blocked_row wasn't in the excerpts, but no such control shows in the captures.
- **Principle:** User control and freedom [NNG-H3]; error prevention [NNG-H5]
- **Fix:** Keep Done one click, as the owner's rules say, and don't add a confirm dialog. Add a quiet "Reopen" button (Look::Plain) to the Done by you box, wired to the existing requeue act (task_panel.rs:534) or a human-task equivalent, and let the note be edited there. This is a new human-task signal: needs the owner's OK. For the empty case, when the steps mention a note and the field is empty, have the first press show "Send with no note?" through c.note(&grp) and let a second press send it.

### R5 · 3 Major · Several colours on the primary box fall under AA
- **Element:** Task panel › Done (dark), step link (light), note field edge (both), Full log count (both) (human-to-do__dark__1360x860.png, human-to-do__light__1360x860.png)
- **Evidence:** Likely
- **Problem:** Computed from theme.rs: Done's white on accent_btn #4d72f0 in dark is 4.22:1 (theme.rs:144, 169), and it is not large text. The step link #3563e9 on warn_soft #fdece3 in light is 4.45:1 (theme.rs:96, 102). The field's edge warn_line against the box is 1.55:1 in light and 1.56:1 in dark, and its fill against the box is 1.15:1 and 1.14:1, so the field boundary is weak in both themes, most of all in dark. The count "5" in Full log, if it uses faint, is 3.04:1 in light and 3.8:1 in dark. Likely, not Confirmed, because the excerpts don't show which token each widget uses; the colours were matched by eye from the captures.
- **Principle:** Contrast minimum [WCAG 1.4.3]; non-text contrast [WCAG 1.4.11]
- **Fix:** Dark accent_btn: darken to about #4468e6 and recheck for at least 4.5:1 with white. Links inside a warn box: use accent_fg (#2447b8 is 6.85:1 in light). Field edge in a warn box: use a colour at 3:1 or more against warn_soft, such as warn (#c2410c, 4.51:1 in light; #f59e5b, 6.88:1 in dark). Count in Full log: use muted (5.43:1 light, 7.12:1 dark).

### R6 · 2 Minor · At the minimum window, Done and Talk it over are out of view
- **Element:** Task panel › Yours to do, to-do, 900x560 (human-to-do__light__900x560.png)
- **Evidence:** Confirmed
- **Problem:** At the smallest supported window the fold cuts through the note field, and Done and Talk it over aren't visible. The owner has to scroll to reach the one action the panel is for, and no scrollbar shows in the capture. The same thing will happen at any size once the steps are long enough, because the action row sits at the end of the content.
- **Principle:** Walkthrough Q2, will users notice the action [NNG-CW]; frequent in view [SEC-8]
- **Fix:** Removing the banner (R1, about 69pt) and the duplicate link button (R8, about 45pt) brings Done into view for this sample. For longer steps, pin the Done and Talk it over row to the bottom of the panel's scroll area, above the tab strip, so that it stays visible while the steps scroll.

### R7 · 2 Minor · Numbered steps aren't a list
- **Element:** Task panel › Yours to do › steps, all sizes (human-to-do__light__1360x860.png, human-to-do__light__900x560.png)
- **Evidence:** Confirmed
- **Problem:** note_body only recognises -, * and • as list items (view.rs:2982), so "1. Open…" falls through to plain paragraph lines (view.rs:2987). Wrapped lines ("/api", "Manager.") return to the left edge under the numbers, and no gap separates the steps. Following a procedure while switching to another app means finding your place again, and the numbers don't stand out from the wrapped text. The structure isn't exposed either.
- **Principle:** Info and relationships [WCAG 1.3.1]; visual hierarchy [NNG-VH]
- **Fix:** Add an ordered-list branch to note_body that matches ^\d+[.)] and builds a K::MdOl, with the number in its own column of about 20px, a hanging indent for wrapped lines, and a 6px gap between steps.

### R8 · 2 Minor · The link button repeats the inline link and can't tell two links apart
- **Element:** Task panel › Yours to do › Open appstoreconnect.apple.com (human-to-do__light__1360x860.png)
- **Evidence:** Confirmed
- **Problem:** The same URL appears as an underlined link in step 1 and again as a button, which costs about 45pt inside the box. link_label keeps only the host (view.rs:1027-1030), so two different pages on one host would give two identically labelled buttons, and the owner couldn't tell which is which without hovering. Whether the inline link is clickable isn't proven by the excerpts, since inline_text isn't among them; it is drawn as a link.
- **Principle:** One fact, one place [SEC-7]; link purpose and consistency [NNG-H4]; every element earns its place [NNG-H8]
- **Fix:** If inline URLs are clickable, drop the button row (view.rs:1053-1056) and shorten the inline text to the host plus the path, so the raw URL stops wrapping. If the button stays, show it only for URLs the step text doesn't already link, and label by host plus last path segment ("Open appstoreconnect.apple.com/…/api").

### R9 · 2 Minor · The tab strip doesn't show where you are
- **Element:** Task panel › footer tabs What to do, Context, Full log, all states (human-to-do__light__1360x860.png, human-done__light__1360x860.png)
- **Evidence:** Confirmed
- **Problem:** In every capture none of the three tabs is marked, because the box is on a fourth view, TaskTab::Overview, that has no tab of its own (task_panel.rs:436-446). The owner can't tell what the current view is, or see an obvious way back to it after opening a tab. "What to do" also sits right under "Yours to do", so it reads as the same thing. The tab strip's code isn't in the excerpts, so the way back wasn't confirmed.
- **Principle:** Visibility of system status [NNG-H1]; consistency and standards [NNG-H4]
- **Fix:** Add a first tab, "Overview", and mark the selected tab with a bold label and a 2px accent bar. For a human task, the tab can be dropped while the box shows the steps, as long as the done state keeps them reachable (see R11).

### R10 · 2 Minor · The two external actions look different, and the wrong one looks like a command
- **Element:** Task panel › Yours to do › Open appstoreconnect.apple.com and Talk it over (human-to-do__light__1360x860.png, human-to-do__dark__1360x860.png)
- **Evidence:** Confirmed
- **Problem:** Opening a web page is a bordered button with no icon, while Talk it over, a command that opens a terminal, is bare orange text with an external-link icon. Both leave the app, but only one says so, and the styling makes the link look like the action and the action look like a link. The orange is also the tone the app uses for alerts.
- **Principle:** One look per role [SEC-4]; consistency and standards [NNG-H4]
- **Fix:** Give Talk it over the bordered Look::Plain style beside Done, so the action row reads as buttons. Show the external-link icon on anything that leaves the app, or on neither. If R8 removes the link button, only the Talk it over change is needed.

### R11 · 1 Cosmetic · The finished record has no time and no steps
- **Element:** Task panel › Done by you (human-done__light__1360x860.png, human-done__dark__1360x860.png)
- **Evidence:** Confirmed
- **Problem:** The box shows the note but not when it was finished, and the steps are gone from this view. Checking back later, the owner can't see when it happened or what was asked without going to another tab, assuming the What to do tab holds them.
- **Principle:** Visibility of system status [NNG-H1]
- **Fix:** Add the finish time in 12-hour form to the label ("Done by you · 3:12 PM"), and put the steps in a closed fold under the note, using the existing fold node.

### R12 · 1 Cosmetic · Header chips: alert tone on a finished task, and an unlabelled "2 of 4"
- **Element:** Task panel › header chips, all states (human-done__light__1360x860.png, human-to-do__light__1360x860.png)
- **Evidence:** Likely
- **Problem:** "Human task" keeps the warn colour even on a finished task, so a kind reads as an alert. "2 of 4" doesn't say what it counts, and this task happens to have 4 steps, while the sidebar's "1 of 4" for the same goal counts something else. The chip's code isn't in the excerpts, so its meaning is inferred.
- **Principle:** Match with the real world [NNG-H2]; one emphasis [SEC-5]
- **Fix:** Use the neutral chip style for Human task once it is done, or for every state. Label the position "Task 2 of 4".

## What works

- One clear primary action: blue Done stands out on the warm box in both themes, and it is the only strongly emphasised control.
- Drafts are safe: the note is kept in drafts and is only dropped after /did succeeds (task_panel.rs:570-576), so a failed send doesn't lose typed text.
- The finished state is calm and honest: the box itself turns green and carries the owner's note, with no extra toast or filler.
- The panel keeps a readable line length at 1728 wide and its layout doesn't stretch; one home for the task is respected, and the human-task actions stay one-click signals.

## Not checked

- Real input: the app wasn't available to run, so no control was clicked, typed in or scrolled, and the driver's inspect, composition audit and blurred screenshot couldn't be used on a native GPUI window. The browser was only used to compute contrast. The squint judgement comes from looking at the captures.
- Keyboard: Tab order, focus rings, Esc to close, what Enter or Cmd+Enter does in the multi-line note field, and keyboard access to the steps' links.
- VoiceOver output and whether GPUI exposes roles and names, beyond the field having no label property in Node::Field.
- Pixel-measured contrast: ratios come from theme.rs tokens, and which token each widget uses is inferred (see R5).
- Hover, pressed and busy states of Done and Talk it over, and tooltip behaviour.
- Failure paths: /did or /talk failing, and what sent(m) reports when Talk it over is pressed while Midna isn't running. Whether /did is accepted on a waiting task.
- Entry to the flow: the captures overlay fields on T2 and it doesn't appear in the Needs you list, so how the owner reaches a human task from the board wasn't reviewed. The Needs you count and the G1 "1 needs you" line were treated as sample data.
- What the What to do, Context and Full log tabs show for a human task, and blocked_row's output, which no capture reveals.
- Long content: many steps, several links, a long or multi-line note, and whether owner_note keeps its line breaks in the done box. Waiting, done and dark at 900x560 and 1728x1084, 200% text, window resize behaviour and the panel's scroll behaviour.
- Virtualized lists and 12-hour time formatting beyond what the captures show (no times appear in this box).
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
