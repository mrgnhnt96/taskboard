## Verdict

macOS desktop app, pointer and keyboard. I reviewed the PR part of the right task panel from 16 captures (11 states in light at 1360x860, 2 in dark, 2 at 900x560, 1 at 1728x1084) and the Rust excerpts. The app itself could not be driven. Blurred, the panel reads as organised at the timeline level: clear stops and a clean step row. Inside Checks and review it gets busy, because orange marks several different things and no action stands out as the next one. Counts: 0 Catastrophe, 6 Major, 4 Minor, 2 Cosmetic. The top problem is that the panel doesn't reliably answer "what is waiting on whom, and what do I do now?". Orange means both "needs you" and "someone else is on it". The next action is a small link with the same weight as Merge now. With merge steps, the step row runs off the panel at every window size, because the panel stays about 480px wide.

## Findings

### R1 · 3 Major · Step row runs off the panel when a project has merge steps
- **Element:** Task panel › Checks and review › step row, with merge steps. pr-merge-steps__light__1360x860.png and pr-merge-steps__light__900x560.png. Measured from the captures, the panel is about 480px wide at 900, 1360 and 1728, so a wider window doesn't help.
- **Evidence:** Confirmed
- **Problem:** With six steps, the last one is cut at the panel edge and reads 'Merg' / 'To d'. A seventh step (the author-side review step, or another merge step) would be fully out of view. Nothing shows that more steps exist, and the code adds no wrap or scroll. Those are the steps that hold the PR back, so the owner can't see what it still needs. Cause: steps are flex_none (task_panel.rs:2013), and the row has no wrap or overflow handling (task_panel.rs:1596), with connectors keeping a 14px minimum (task_panel.rs:1599).
- **Principle:** Every control at every size: nothing is lost at a supported width [NNG-H6]; narrowing keeps sections recognisable [SCR-6]; visibility of system status [NNG-H1]
- **Fix:** When there are more than 4 steps, render them as a vertical list in c_steps: one row of about 28px per step, with a 16px dot, the name on the left and the state words on the right. As a lighter option, collapse finished steps into one 'Checks, You, Review done' stop and draw only the open steps in the horizontal row. Keep the current horizontal row for 4 steps or fewer.

### R2 · 3 Major · Orange 'needs you' also marks work that isn't the owner's
- **Element:** Task panel › step row and reviewer rows. Review step in pr-changes-asked__light__1360x860.png and pr-long__light__1360x860.png; merge steps in pr-merge-steps__light__1360x860.png; reviewer pill in pr-rereview__light__1360x860.png.
- **Evidence:** Confirmed
- **Problem:** Flow 4 depends on knowing what waits on whom, but the Ask look is used for three different situations. (1) Review shows orange '3 new comments' while the row below says an agent is already 'Answering 3 review comments' (view.rs:1920). (2) Every unfinished merge step shows orange 'To do' with a speech-bubble icon, but the panel offers no control for it and doesn't say who does it (view.rs:1944). (3) 'Re-review waiting' is an orange outlined pill although it waits on Dana, not the owner (view.rs:2021). The same state also has three wordings in one view: 'Awaiting re-review', 'Re-review waiting' and 'Re-review queued'. The owner has to read every row to work out which orange is theirs.
- **Principle:** Consistency: one meaning per look [NNG-H4]; at most one strong emphasis, selected states quiet [SEC-5]; one fact in one place [SEC-7]
- **Fix:** Keep StepSt::Ask and orange only for steps the owner must act on. When a live stage session is on the comments, draw Review as StepSt::Now with the same '3 new comments' link. Draw merge steps as StepSt::Wait, or Now for the running one, with words that name the actor, such as 'Agent next'. Keep Ask only if the owner runs that step, and then add the way to do it. Make the re-review pill Tone::Queued, and use one wording throughout: 'Re-review waiting' on the pill, plus 'Queued' as the note.

### R3 · 3 Major · Checks and review has no single next action
- **Element:** Task panel › Checks and review, whole section. pr-changes-asked__light__1360x860.png, pr-long__light__1360x860.png and pr-stopped__light__1360x860.png.
- **Evidence:** Confirmed
- **Problem:** The primary task is to do the one thing that moves the PR on, but nothing is emphasised as that one thing. These checks fail together. (a) One emphasis: Mark reviewed, Terminal, Merge now, Passed and '3 new comments' are all small blue links, and the owner's action sits at the right end of a grey row with the same weight as the agent's row above it. (b) One look per role: links come in four looks: underlined, underlined with a chevron, bold with no underline ('Terminal ›'), and the bold #944 with a separate external-link icon. State words change colour by whether they link, not by state: Done reads blue 'Passed' beside green 'Reviewed', and a failed Checks step reads blue 'Being fixed' under a red name (view.rs:1871). (c) Order: the owner's row comes below the agent's row, under the reviewer rows, so what needs the owner is the last thing read. In pr-changes-asked the section stacks 7 rows before the next stop.
- **Principle:** One emphasis [SEC-5]; one look per role [SEC-4]; frequent in view [SEC-8]; aesthetic and minimalist design [NNG-H8]
- **Fix:** Directly under the step row, put the owner's row first. When the You step is 'waiting', draw Mark reviewed as a soft button (Look::SoftSmall already exists) and keep it a link in the 'early' state. Follow with the agent's stage row, then the reviewer rows. Use one link look for every 'x ›' command by giving Terminal › the same look as Mark reviewed ›. Colour a step's link with the step's state colour, keeping the underline as the link cue.

### R4 · 3 Major · Merge now shows when it can't work or is already happening
- **Element:** Task panel › Merge now. pr-stacked__light__1360x860.png, pr-stopped__light__1360x860.png (branch has a conflict), and phase 'merge' from code.
- **Evidence:** Confirmed
- **Problem:** The code comment says only conflicts and the stack's order still stop a merge, but the condition checks neither (view.rs:2385). So it's offered on a stacked PR, next to 'Waits for #940 to merge into this one first', and on a branch with an unresolved conflict. Because Merging is StepSt::Now, the guard also leaves it visible while the merge is running. In the failed-checks state it reads the same as in a clean state, and that it bypasses checks and reviews appears only in a hover tooltip. I didn't see the confirm step or what a failed attempt shows. The control also sits under Checks and review, while the Pending merge stop stays empty.
- **Principle:** Error prevention: constraints rather than error messages [NNG-H5]; visibility of system status [NNG-H1]; match the action to its result [NNG-CW]
- **Fix:** Hide Merge now in phase 'merge', and in phase 'stack' and on a conflict, where the existing line already gives the reason. Move it into the Pending merge stop's body. Make the label name what it skips: 'Merge past failed checks ›', 'Merge without review ›', and plain 'Merge now ›' only when nothing is skipped.

### R5 · 3 Major · Step state text and the not-started ring fail contrast
- **Element:** Task panel › step row. 'After checks' in pr-checks-running__light__1360x860.png and pr-checks-failed__light__1360x860.png; 'Reviewed' in pr-rereview__light__1360x860.png; empty ring on You. Dark for 'After checks' and the ring from theme values.
- **Evidence:** Confirmed
- **Problem:** Computed from theme.rs against the card colour. The Todo sub-label (t.faint) is 3.04:1 in light and 3.80:1 in dark, which fails 4.5:1 at this small size. The Done sub-label (t.up) is 4.38:1 in light, which fails narrowly. It also applies to merge steps' 'Passed' and 'Skipped: why'. The Todo ring (t.border_2) is 1.48:1 in light and 1.63:1 in dark, under 3:1. A You or merge step with no words has only that ring to show its state.
- **Principle:** Contrast (minimum) [WCAG 1.4.3]; non-text contrast [WCAG 1.4.11]
- **Fix:** In c_step (task_panel.rs:2000-2004), use t.muted for the Todo sub-label (5.43:1 light, 7.12:1 dark) and t.up_fg for the Done sub-label (6.73:1 light, 9.78:1 dark). In step_dot (task_panel.rs:2807), use t.faint for the Todo ring border (3.04:1 light, 3.80:1 dark).

### R6 · 3 Major · Reviewing means going below the actions to find the PR link
- **Element:** Task panel › Pull request row, against the Mark reviewed and Merge now rows. pr-green-waiting__light__1360x860.png, pr-changes-asked__light__900x560.png.
- **Evidence:** Confirmed
- **Problem:** The flow is see, open, mark reviewed, merge, but the panel orders it see, mark reviewed, merge, open. 'Waiting on your review' has no way to open the PR on its row. The clear PR link is one stop down, after Merge now. At the 900x560 minimum, the Pull request stop is below the fold and Merge now is cut by the footer. The small '1 of 1' link does open the PR, but nothing says so, and it has no PR link in states such as Review off or Not asked. This step is taken on every PR review.
- **Principle:** Cognitive walkthrough: will users notice the correct action? [NNG-CW]; frequent in view [SEC-8]; recognition rather than recall [NNG-H6]
- **Fix:** Add 'Open PR ›' to the owner's row in view.rs:2369-2375, before Mark reviewed, so it reads 'Waiting on your review · Open PR › · Mark reviewed ›'. Use the same url as the Pull request head. The Pull request stop stays where it is, as the PR's one home.

### R7 · 2 Minor · Stage row says the agent is working after it stopped
- **Element:** Task panel › stage row and banner, stopped. pr-stopped__light__1360x860.png.
- **Evidence:** Confirmed
- **Problem:** The banner says the fix agent stopped, yet the row below still reads 'Fixing the merge conflict'. Only the dot and box colour change (view.rs:2355-2357), and the stopped wording is only in a hover tip. The banner's heading says the terminal 'asks' but the text is a statement. The owner also gets four prompts at once: the banner, the You step, 'Waiting on your review' and a 'Waiting on you' stop, with the review offered although the conflict is what blocks progress.
- **Principle:** Visibility of system status [NNG-H1]; one fact in one place [SEC-7]
- **Fix:** When stage.stopped is set, change the row's words to 'Stopped: merge conflict not fixed', and keep Terminal ›. Keep the banner as the single call to action in this state, and drop the row's orange fill so the banner is the only strong emphasis.

### R8 · 2 Minor · Pending merge's 'Waits on steps' hides who it waits on
- **Element:** Task panel › Pending merge stop. pr-green-waiting__light__1360x860.png and __dark__.
- **Evidence:** Confirmed
- **Problem:** This capture has no merge steps, and the only open step is You, yet the header says 'Waits on steps' (view.rs:1954). That's the board's internal phase word. The same wait appears three ways: 'Waits on steps', 'Waiting on you' and 'Waiting on your review'. Pending merge also gets a blue current dot directly above the orange one, so two stops look current.
- **Principle:** Match between the system and the real world [NNG-H2]; one fact in one place [SEC-7]
- **Fix:** Derive the sub-label from the first open step: 'Waits on you' when You is 'waiting', 'Waits on Rebase on main' when a merge step is first. Keep the Pending merge dot as StepSt::Wait with the plain hollow look, so only the stop that needs attention has a filled, haloed dot.

### R9 · 2 Minor · Blocking PR #940 can't be opened from the panel
- **Element:** Task panel › 'Waits for #940 to merge into this one first'. pr-stacked__light__1360x860.png.
- **Evidence:** Confirmed
- **Problem:** The owner learns what blocks the merge but has to find #940 by hand, because waits_on_stack is drawn as plain text (view.rs:2348-2349). The sibling cases do link: 'Stacks on T3' and 'Waits on T3'. The fact is also split between 'Waits on stack' at the top and this line.
- **Principle:** Recognition rather than recall [NNG-H6]; user control and freedom [NNG-H3]; consistency [NNG-H4]
- **Fix:** Have the board send waits_on_stack as an object with ref, num, url and line. Render it like stacks_line: the task ref opens in this panel (Act 'open-task') and 'PR #940' opens on the host. Change the Pending merge sub-label to 'Waits on T7' to match the 'Waits on T3' pattern.

### R10 · 2 Minor · Reviewer rows mislead on time and hide the popover
- **Element:** Task panel › reviewer rows. pr-changes-asked__light__1360x860.png, pr-long__light__1360x860.png.
- **Evidence:** Confirmed
- **Problem:** 'Changes asked · Asked 7:12 AM' reads as when the changes were asked, but it is when the reviewer was asked (view.rs:2038). On an approved row that time is no use, and on a Waiting row it repeats the pill. It also uses hhmm with no date, while the re-review branch uses day_time, so a wait of several days likely shows only a clock time. The name opens a useful popover of the reviewer's open asks, but it's a plain button with no tooltip and nothing marks it as clickable (view.rs:2010). Pills start at a different x on each row, and a long name drops 'Asked 7:12 AM' to its own line.
- **Principle:** Match with the real world [NNG-H2]; recognition rather than recall [NNG-H6]; alignment [SCR-4]; no helper text that shows no data (owner's rule)
- **Fix:** Show 'Asked …' only on Waiting rows, using day_time so it has a date once it's not today. Show the review's own time on Approved and Changes asked rows if the board has it, otherwise show nothing. Give the name a tip ('Their open reviews') and an underline on hover. Lay the rows out as a grid with a name column capped at about 150px, ellipsis on overflow and the full name in the tip.

### R11 · 2 Minor · Merged-without-a-note state is a dead end
- **Element:** Task panel › Merged › Note for testers, waiting. pr-merged-note-waiting__light__1360x860.png.
- **Evidence:** Confirmed
- **Problem:** An orange 'Waiting' pill with 'Merged without one; the ticket waits' signals that something needs doing, but offers no action and no owner. 'One' makes the reader go back to the label to decode it. The same Needs tone is also used for 'Posting it, then moving the ticket', which needs nothing from the owner (view.rs:1707-1708).
- **Principle:** Help users recover [NNG-H9]; no dead ends [NNG-H3]; plain language [NNG-H2]
- **Fix:** Reword to 'No note yet, so the ticket stays where it is', and link to where the note gets written (Terminal › when a session exists). Use Tone::Queued for 'Posting it…' and keep Tone::Needs only when the owner can act. A one-click 'Write the note' signal would be cleaner, but it adds a signal to the read-only rule, so it needs the owner's OK.

### R12 · 1 Cosmetic · Note preview counts source lines, not visible lines
- **Element:** Task panel › Merged › Note for testers. pr-merged__light__1360x860.png.
- **Evidence:** Confirmed
- **Problem:** NOTE_PREVIEW_LINES is 3 source lines (view.rs:1721), so the capture shows 5 visible lines. A note written as one long paragraph would show in full with no Show all, pushing Checks and review down.
- **Principle:** Aesthetic and minimalist design [NNG-H8]
- **Fix:** Clamp the preview to 3 rendered lines, or cap it by characters at about 240, and show Show all when anything is cut.

### R13 · 1 Cosmetic · Merged stop doesn't say when
- **Element:** Task panel › Merged stop. pr-merged__light__1360x860.png.
- **Evidence:** Confirmed
- **Problem:** The Started stop has a time, but Merged has none, because its head's time slot is passed empty (view.rs:2411). An owner checking in through the day can't tell when it landed.
- **Principle:** Visibility of system status [NNG-H1]
- **Fix:** Pass the merge time into tl_head's time slot, in the same 12-hour format as Started, if the board has it.

## What works

- The step row pairs colour with an icon and words, so state never depends on colour alone, and it holds up well in dark mode.
- The stage row with Terminal › ties an agent's work to where it's happening in one click.
- 'Pending merge' stays named for what hasn't happened yet, then becomes 'Merged' or 'Closed without merging', so the timeline never overclaims.
- Waiting pills show how long a reviewer has been waiting (for example 'Waiting 12h 23m'), which is what the owner needs when deciding whether to chase. Times follow the owner's 12-hour AM/PM rule.

## Not checked

- Real input: the app couldn't be driven (captures and excerpts only), so no click, hover, keyboard or scroll was tested. Tab order, focus visibility, Esc behaviour and whether the panel scrolls sideways are unknown.
- Merge now's confirm step and failure message, and Mark reviewed's pending, success and error feedback (c.note output): no capture shows them.
- The reviewer popover, the Show all dialog and every tooltip.
- VoiceOver names and roles for steps, dots and links.
- 'Updated 4m ago': no capture shows it. Also not captured: Merging, Closed without merging, Waits on base, the 'not this PR's' box, the author-review step, a 'Stacks on' line, Needs setup, and a note in the 'Posting it' state. Their code was read only.
- Who runs merge steps, and what the board can send (merged time, review time, a structured waits_on_stack). The fixes that use them depend on this.
- Contrast was computed from theme.rs hex values, assuming the panel background is the card colour as the captures show. The maths ran through the driver's eval step because other shell commands were denied. Pills and other text not cited were not measured.
- 200% text and page zoom: not captured. Dark mode was seen in 2 states only, and 900x560 and 1728x1084 in only 2 and 1 states.
- Motion, including the Running spinner.
- Stopped state's restart path: no restart control was visible, and whether one exists elsewhere is unknown.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
