## Verdict

macOS desktop, pointer and keyboard. I reviewed the Settings › System page at 1400x1000, 900x680 and 720x460, in light and dark, plus the board-stopped state at 900x680. Evidence is six captures and the code excerpts. This is a native GPUI app, so nothing was driven with real input and no contrast was measured. All six captures show Update as Off, so every other update state (checking, up to date, downloading, ready, installing, failed), the update pill and Restart daemon are reviewed from code only. First impression: the page reads as calm and organised. Blurred, it shows two quiet cards and no competing emphasis, and the composition is not the problem. The problems are in the states the captures don't show, and in the board-stopped state, which is the quietest thing on the page when it should be the loudest. Counts: 0 Catastrophe, 3 Major, 8 Minor, 4 Cosmetic. Top problem: the failed path. Every failure, including a rejected signature or a failed install, reads "Check failed at…" with a Retry button, and the real reason sits in the smallest, faintest text on the row. No finding here needs the owner's OK: none of the fixes breaks a stated rule.

## Findings

### R1 · 3 Major · Every update failure is called "Check failed", and the reason is raw, faint and can go stale
- **Element:** Settings › System › Update row, Failed state, plus the sys-note row in the versions card; all sizes, code only (settings.rs L26, L69-71, L88-91; updater.rs L254, L280, L338-340)
- **Evidence:** Confirmed
- **Problem:** State::Failed covers a failed check, a failed download, a rejected signature, a failed swap and a failed relaunch, and all of them show "Check failed at {at}" with Retry. A security refusal ("The new app is signed by X, this one by Y") looks the same as a network blip, and Retry just downloads the same file again. The cause is shown as an 11.5px muted note, and it carries raw ureq, codesign or spctl text. When Restart to update fails, the same error appears twice: in the Update card and again in a red row in the versions card. In the excerpt sys_note is only cleared by restart_daemon, so the red row can stay after a later successful Retry. If only the relaunch fails, the bundle and daemon are already new while the running app is old, and the page still says "Check failed".
- **Principle:** Errors say what happened and what to do [NNG-H9]; system status must be true [NNG-H1]; one fact, one place [SEC-7]; error with no way to recover is 3 Major in the severity guide
- **Fix:** Add a stage to the state: Failed { stage: Check | Download | Verify | Install, error, at }. Titles: "Couldn't check for updates", "Download failed", "Update refused: it didn't pass verification", "Couldn't install {version}", each followed by "· 3:05 PM". Keep the plain sentence in the note at t.text_2, and move raw tool output behind a Copy details link. Label the button by stage: Retry for Check and Download; for Verify, replace it with "Download from GitHub ↗" pointing at the release page. Show apply errors only through the Failed state and drop the duplicate sys_note for them. Clear sys_note in check_now and at the start of apply_update.

### R2 · 3 Major · Update controls and the pill can't be reached from the keyboard
- **Element:** Settings › System › Check now, Retry, Restart to update, Restart daemon, Release notes; sidebar update pill; all sizes (kit.rs L130-150, L175-178; app.rs L2-5, L29-39)
- **Evidence:** Confirmed
- **Problem:** btn_base, link and pill_shell are plain divs with on_click only. They have no focus handle, tab stop, key handler or focus style in the excerpt, although the framework has focus handles (Input::render uses one). So nobody can Tab to Restart to update, press it with Space or Enter, or see where focus is. The page's actions need a pointer, for an owner who works in a keyboard-heavy developer setup. Buttons also have no pressed style.
- **Principle:** All functions work from the keyboard [WCAG 2.1.1]; focus is visible [WCAG 2.4.7]; keyboard rules for desktop [MS-KBD]
- **Fix:** Give btn_base, link and the clickable pill_shell a FocusHandle and tab stop. Fire on_click on Enter and Space. Draw a 2px t.accent ring with a 2px offset on focus. Add an active style (t.seg background) to btn. Also add Check for Updates… and Restart to Update to the Taskboard app menu so both commands have a keyboard route. If the app menu already has them, only the focus work is needed.

### R3 · 3 Major · A stopped board is the calmest thing on the page and offers no way forward
- **Element:** Settings › System › banner and Daemon version row, board stopped, light, 900x680 (down-system-m.png; settings.rs L46-67)
- **Evidence:** Confirmed
- **Problem:** The primary task is to get the board connected, and agents stop reporting when it's down. In the capture the banner says "taskboardd: Can't reach the task board server." with no next step. The Daemon row uses the same grey dot as "Off", and the app row beside it stays green. A version mismatch gets an amber row and a primary button, while a dead daemon gets neither, so the lesser problem is louder. One fact appears four ways with four names: "taskboardd", "task board server", "not running" / "Not connected.", and "Connecting…" in the sidebar, which also implies it is still being tried. In an installed build the fix is a quiet button labelled "Restart daemon" on something the page says isn't running. In a dev build there is no button at all, and in the mismatch case the note still says "Restart it" (settings.rs L50, L53). self.info being None also covers the moment before load_info returns, so "not running" may flash on every page open.
- **Principle:** Error states say what to do [NNG-H9]; same thing, same name [NNG-H4]; one fact, one place [SEC-7]; recovery from the primary task's failure is 3 Major
- **Fix:** Use one name, "daemon". Banner text: "The daemon isn't responding, so the board can't update." In the Daemon row use a t.down dot, a t.down_soft background and the status "Not responding". Make the button primary and label it "Start daemon" when info is None; keep it in this row so the action has one home. In a build with no bundle, replace the button with a note naming the command to run, and drop "Restart it" from the mismatch note. Add a Loading state so the row shows "Checking…" until the first load_info returns.

### R4 · 2 Minor · A ready update disappears when a newer release starts downloading
- **Element:** Settings › System › Update row and sidebar update pill, Ready state, code only (updater.rs L246-281, L189-190)
- **Evidence:** Confirmed
- **Problem:** While Ready, the 6-hourly check can find a newer release. It then sets Downloading, so the pill and Restart to update both vanish with no message. stage() deletes the staged folder first, so a failed download or check of the newer build leaves the owner with "Check failed" and no update, although a good one was ready. On a beta build with frequent releases this is a realistic path: the owner clicks the pill and finds nothing to press.
- **Principle:** Visibility of system status [NNG-H1]; user control [NNG-H3]
- **Fix:** Stage each version in its own folder (staged/{version}) and only swap STAGED and delete the old folder once the new one passes every check. Keep State::Ready visible during that background work, with the row note "Downloading {newer} in the background". If the newer build fails, stay on Ready and log it.

### R5 · 2 Minor · Restart note promises continuity the code doesn't match, and a failed daemon restart is silent
- **Element:** Settings › System › Update row note, Ready state, code only (settings.rs L22; updater.rs L14-15, L326-329)
- **Evidence:** Confirmed
- **Problem:** The note says "The board and its agents keep running through the restart", but apply() restarts the daemon, and the module docs say so. The owner decides when to press this based on whether agents will be hurt, so the sentence has to be exact. If that daemon restart fails, the error only goes to the log and the app quits anyway. The new app then runs against the old daemon, and the only sign is an amber row on a page the owner has no reason to open. Whether agents actually survive a daemon restart could not be checked from these excerpts.
- **Principle:** System status is honest [NNG-H1]; match what actually happens [NNG-H2]; error prevention [NNG-H5]
- **Fix:** Reword to what's true, for example: "Taskboard closes and reopens, and the daemon restarts on {version}. Agents keep working; the board reconnects in a few seconds." Keep the agents clause only if it's verified, and otherwise state what they lose. Show a warn pill in the status area when daemon version != app version after launch, reading "Daemon behind: restart" and opening System, so a failed restart is seen.

### R6 · 2 Minor · Restart to update can be pressed twice and may never show Installing
- **Element:** Settings › System › Restart to update, Ready state, code only (settings.rs L23, L83-96; updater.rs L325)
- **Evidence:** Likely
- **Problem:** apply_update has no guard. State only becomes Applying inside the background task, after the one cx.notify() has already been called, so the button can stay live. A second press fails with "No update is ready." and shows a red error row while the install is actually going ahead. The same timing means "Installing…" may not appear until something else redraws. Whether a timer elsewhere repaints the page isn't in the excerpt.
- **Principle:** Feedback on every action [NNG-H1]; error prevention [NNG-H5]
- **Fix:** Set the state synchronously before spawning, with a sys_applying flag like sys_busy, or make apply() split into a sync set(State::Applying) and the blocking work. Render the disabled "Installing…" button from that flag, and call cx.notify() again from the task when it finishes.

### R7 · 2 Minor · Downloading shows no progress, and verifying is hidden inside it
- **Element:** Settings › System › Update row, Downloading state, code only (settings.rs L18; updater.rs L143-173, L268-272)
- **Evidence:** Confirmed
- **Problem:** The row says "Downloading {version}…" with no amount, no bar and no cancel, even though the code knows both size and total. A stalled download looks the same as a working one. The unpacking and the codesign and spctl checks that follow also read as Downloading. The note "Checked before it's offered." is hard to parse and doesn't say what the owner should do, which is nothing.
- **Principle:** Slow actions show progress [NNG-H1] [mobile:NNG-RESPONSE]; plain words [NNG-H2]
- **Fix:** Add downloaded and total to State::Downloading and show "Downloading 1.2.0 · 14 of 38 MB", updated about once a second. Add State::Verifying { version } shown as "Checking 1.2.0…". Change the note to "Runs in the background. You'll be asked before anything restarts."

### R8 · 2 Minor · Background update failures never leave the Settings page
- **Element:** Sidebar update pill, all non-Ready states, code only (app.rs L29-30; updater.rs L254)
- **Evidence:** Confirmed
- **Problem:** update_pill returns None for everything except Ready, and the updater runs by itself every 6 hours. An owner who leaves the app open all day has no sign that updates have been failing, for example a refused signature or a read-only install folder. They just stay on an old build until they happen to open System.
- **Principle:** Visibility of system status [NNG-H1]
- **Fix:** Keep offline blips quiet, but after two failed checks in a row, or on any Verify or Install failure, show a warn-toned pill reading "Update failed" that opens Settings › System. Use the same pill_shell and tone as accounts_pill, with the reason in the tooltip.

### R9 · 2 Minor · Restart daemon gives no success message and doesn't say what it affects
- **Element:** Settings › System › Daemon version › Restart daemon, installed build, code only (settings.rs L53-61, L99-118)
- **Evidence:** Confirmed
- **Problem:** On success, sys_note stays empty even though a green style exists for it (L70), so the only feedback is the note changing to "Up under a minute" and a new pid. After a fixed 2 second wait the page reads the daemon once, so a slower launchd leaves "not running" with nothing to say it's still starting. The button fires on one click with no word on what happens to agents, on a screen the owner uses while many agents are running.
- **Principle:** Feedback after an action [NNG-H1]; destructive actions are worded clearly or confirmed [NNG-H5]
- **Fix:** After success set sys_note to ("Daemon restarted on {version}.", false) and clear it after about 5 seconds. Poll load_info every 500ms for up to 15 seconds instead of one read at 2 seconds, keeping "Restarting…" until it answers, then show "The daemon didn't come back" in red. Add a tooltip to the button stating the effect on running agents. If agents are interrupted, ask first in a confirm dialog.

### R10 · 2 Minor · The common Up to date row likely overflows the card at the minimum window
- **Element:** Settings › System › Update row, Up to date and Ready states, 720x460, code only, widths estimated (settings.rs L17, L28-33, L1589-1605 excerpt; layout from system--light-s.png)
- **Evidence:** Likely
- **Problem:** At 720 wide the card is about 436px, so about 404px sits inside its padding. The label column has min_w 150 and the controls are flex_none with no wrap. "Up to date · checked at 12:05 PM" plus Check now comes to about 277px. With the 16px gap that needs about 443px, so Check now runs roughly 20px past the card edge, where overflow_hidden cuts it off. Ready is about 2px over. The capture shows only "Off", which is the shortest state, so the problem isn't visible there. A long failure message also gets only a 150px column, which makes a very tall row.
- **Principle:** The minimum window works and no control is lost [Pass 7, desktop window range] [HIG-MACOS]
- **Fix:** Shorten the status to "Up to date · 12:05 PM" and let row wrap: add flex_wrap to the row and justify_end on controls, so controls drop to a second line under about 480px of row width. Put a failure note on its own full-width line under the title and controls.

### R11 · 2 Minor · The update pill's wording is vague and its text colour departs from the pill tones
- **Element:** Sidebar update pill, Ready state, code only (app.rs L32-35; kit.rs L111-114)
- **Evidence:** Likely
- **Problem:** "Taskboard 1.2.0 is ready" doesn't say an update is waiting; that only comes from the tooltip, and tooltips are pointer-only. The pill draws t.accent text on t.accent_soft, whereas tone() pairs accent_soft with t.accent_fg and accounts_pill uses warn_fg. At 12.5px this is the pairing most likely to fall under 4.5:1. It wasn't measured, because no capture shows this pill and the token values aren't in the excerpts.
- **Principle:** Text contrast [WCAG 1.4.3]; label says what it does [NNG-H2]; consistency [NNG-H4]
- **Fix:** Use tone(t, "accent") for the colours, keeping t.accent for the dot only, and label it "Update ready: 1.2.0". Measure the pair in light and dark and confirm at least 4.5:1.

### R12 · 1 Cosmetic · App version's green dot and the page subtitle carry no information
- **Element:** Settings › System › App version row and page header, all sizes (system--light-m.png, down-system-m.png; settings.rs L45)
- **Evidence:** Confirmed
- **Problem:** The App version dot is always t.up, so it can't change, yet it sits in the same column as a real status dot. It stays green while an update waits or the board is down. The subtitle "Updates, and the versions of the app and the daemon." only lists the two cards below it, which goes against the no-filler rule.
- **Principle:** Every element earns its place [SCR-5] [NNG-H8]
- **Fix:** Remove the App version dot and right-align the plain version. Remove the subtitle, or replace it with something the page can't otherwise show.

### R13 · 1 Cosmetic · "Checking…" appears twice, and disabled buttons still react to hover
- **Element:** Settings › System › Update row, Checking state, and any kit::disabled button, code only (settings.rs L16; kit.rs L153-156, L181-183)
- **Evidence:** Confirmed
- **Problem:** While checking, the status and the disabled button both say "Checking…" side by side. kit::disabled only sets opacity and cursor, so the hover background from btn stays and a disabled button still responds to the pointer.
- **Principle:** One fact, one place [SEC-7]; disabled controls look disabled [Pass 3]
- **Fix:** Keep the status text and leave the button out of the Checking state, or keep a disabled "Check now" so the row doesn't shift. Have kit::disabled clear the hover style.

### R14 · 1 Cosmetic · Rows stretch across a wide window
- **Element:** Settings › System › both cards, 1400x1000 (system--light-l.png; settings.rs L4)
- **Evidence:** Confirmed
- **Problem:** The list has no max width, so at 1400 wide each label and its value or button sit about 1000px apart, and the eye has to track across empty space to link them.
- **Principle:** Content has a max width [tablet:AND-T2]; desktop window range [Pass 7]
- **Fix:** Add max_w(px(720.)) to the system-rows content, left-aligned with the page title. Apply it across Settings pages if they share the same stretch.

### R15 · 1 Cosmetic · "Checked at" has no date
- **Element:** Settings › System › Update row, Up to date and Failed states, code only (updater.rs L69-71)
- **Evidence:** Likely
- **Problem:** The time has no day, so after a laptop sleep "checked at 3:05 PM" may be from yesterday and read as today. The 12-hour format itself is correct.
- **Principle:** Status is accurate [NNG-H1]
- **Fix:** Store a timestamp and format it when drawing: "3:05 PM" for today, "Yesterday, 3:05 PM" or "Nov 4, 3:05 PM" for older checks.

## What works

- The update page is calm and well grouped: two cards with one job each, and it holds together in light and dark at all three sizes in the captures.
- The update model respects the owner: downloads run in the background, nothing restarts without a click, and the pill is quiet and persistent rather than a modal.
- Verification and the atomic swap mean Taskboard.app is always the whole old or whole new app, and refusals are written as sentences at the source.
- Off states explain themselves ("The sample board doesn't update."), Release notes follows the ready release's page, times use 12-hour AM/PM, and the 26px small buttons clear the 24px target floor.
- The version mismatch case is handled well: an amber row, a note saying what to do, and the button promoted to primary only when it's needed.

## Not checked

- Real input: this is a native GPUI app with no page for the driver to open, so no control was clicked, tabbed or hovered, and every behaviour finding comes from reading code.
- Update states other than Off in a running app: checking, up to date, downloading, ready, installing and failed have no captures, so their layout and wrapping are estimates (R10 in particular).
- The update pill: no capture shows it, and it isn't confirmed where it appears. The code calls it the status bar pill and the brief says sidebar. Whether it also shows in the Settings window wasn't checked.
- Contrast: no inspect run and no token values in the excerpts. Text in the captures looks readable by eye but wasn't computed, in either theme.
- Whether the System page repaints on its own when the updater thread changes state: no timer or observer is in the excerpt. If there is none, "Checking…" could stay on screen until something else redraws, which would be a further finding.
- Whether load_info or other code clears sys_note, or what load_info does to self.info when a read fails; only the excerpted lines were read.
- Whether running agents survive a daemon restart, which decides how R5 and R9 should be worded.
- App menu items and shortcuts for update commands, Settings navigation by keyboard, and the window's own title and close behaviour: not in the excerpts.
- Unsaved input in other Settings sections or windows when Restart to update quits the app.
- Screen reader output (names, roles, status announcements), 200% text, reduced motion, sizes between the three captured, and very long data such as a multi-line codesign error.
- The Failed, Restart daemon and mismatch states in a real installed build: the captures come from sample and dev builds, where the Restart daemon button doesn't appear.
- Token use and consistency with other Settings pages belong to theme-review. One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
