## Verdict

macOS desktop, pointer and keyboard. This is a native GPUI window, so there was no page to drive. I reviewed the 15 captures (QA, Reviewers and Worktrees at 1400x1000, 900x680 and 720x460, in light and dark), the code excerpts, and the wider source in the worktree where the excerpts stopped. I also read one extra capture, design/captures/settings/history--light-s.png, because Clean up now lives on History. Nothing was used with real input. All three pages hold up at 720x460, with no clipping or lost controls. First impression, judged by eye because the blur audit needs a page: each page is calm and organised, but nearly empty. The only bright spot is QA's amber 'Needs Jira' row, for a feature that is optional. The sample board returns no slots or reviewers and has no Jira, so the populated Reviewers and Worktrees pages and QA's On state were judged from code only. Findings: 0 Catastrophe, 5 Major, 7 Minor, 2 Cosmetic. The top problem is that these pages mostly can't do what the brief says the window is for: Jira, pool settings and the reviewer roster can't be set here, and the retention choices on the row that drives Clean up now render as an empty control. Next is staleness: QA and Reviewers load once when the window opens, so a window left open all day shows old data or stays stuck on 'Asking the board…'.

## Findings

### R1 · 3 Major · QA can only be unlocked by hand-editing a config file, and Jira has no home in Accounts
- **Element:** QA › Read QA comments row, no Jira configured, all sizes (qa--light-m.png, qa--dark-s.png)
- **Evidence:** Confirmed
- **Problem:** The only way forward is 'set up [jira] in ~/.config/taskboard/config.toml'. The window can't connect Jira, and CARDS in Accounts lists only GitHub, Bitbucket, Azure and Slack. The path is plain div text, so it can't be selected or copied, and nothing says which keys are needed. The row is also amber, the loudest thing on the page, for a feature the header calls off by default, so an owner who doesn't use Jira carries a permanent warning.
- **Principle:** Brief rule: the window is the owner's only way to set these things, and one home per object; recognition over recall [NNG-H6]; plain recovery path [NNG-H9]; one emphasis [SEC-5]
- **Fix:** Add a Jira card to Accounts (site URL, email, API token), built like the Bitbucket card and stored the same way. In QA, replace the note with 'Connect Jira in Accounts to use this.' plus a kit::link that sets view to Sec::Accounts. Pass warn=false to line() on this row, so it is a quiet neutral row. Keep amber for a Jira sign-in that has stopped working.

### R2 · 3 Major · QA and Reviewers load once, then go stale or stay stuck; failed loads are dropped silently
- **Element:** QA and Reviewers pages, and Worktrees on a failed load, in a long-open window
- **Evidence:** Confirmed
- **Problem:** load_qa and load_reviewers run only in SettingsWindow::new. Sidebar clicks reload System, Worktrees and Usage only, and the poll loop reloads Worktrees and System only. So 'N waiting on you' and 'Last checked Jira' freeze, a roster changed by agents is never shown, and after the owner fixes [jira] the row keeps saying 'Needs Jira' until the window is closed and reopened. If the first load fails, 'if let Ok(v)' drops the error, so the page shows 'Asking the board…' indefinitely. Worktrees also keeps old slots with no sign of a failed refresh.
- **Principle:** Visibility of system status [NNG-H1]; error recovery [NNG-H9]
- **Fix:** In the sidebar on_click, also call load_qa for Sec::Qa and load_reviewers for Sec::Reviewers, and add both to the 30-second poll branch while that page shows. Add a load-error field per page: with no data yet, replace 'Asking the board…' with 'Couldn't reach the board. Retrying…'; with old data, keep it and add a muted line 'Last updated 3:05 PM'.

### R3 · 3 Major · Retention choices render as an empty control, so Clean up now runs against a limit the owner can't see or set
- **Element:** History › Keep every event for and Keep day summaries for, above the Cleanup row (design/captures/settings/history--light-s.png)
- **Evidence:** Confirmed
- **Problem:** history_page reads h['detail_days_choices'] and h['summary_days_choices'], but the daemon's GET /history sends 'detail_choices' and 'summary_choices' (days.rs). The choices come back empty, and each row's seg is a tiny empty dot at the right edge, as the capture shows. No retention value is visible and none can be picked, yet Clean up now deletes by those values. The seg rows sit outside the three named sections, but they decide what Clean up now removes.
- **Principle:** Control missing from the only place it can be set (brief rule); visibility of system status [NNG-H1]
- **Fix:** Read the keys the daemon sends, for example pick("detail_days", "detail_choices", …), or rename the daemon's keys. Add a test that feeds the real GET /history shape through history_page and asserts four segments. Do this before R8.

### R4 · 3 Major · Pool settings and the reviewer roster are read-only here and point the owner at a CLI
- **Element:** Worktrees › Pool row and header line; Reviewers › roster rows and header line (worktrees--light-m.png, reviewers--light-m.png, worktrees_page, reviewers_page)
- **Evidence:** Confirmed
- **Problem:** The headers say 'tb project set sizes a project's pool' and 'Agents and tb reviewers change the roster'. Neither page has a control, so the owner has to leave the window or ask an agent to change a pool limit, pin a reviewer or stop asking one. The Pool sentence also leaves out low_disk_gb, the fifth pool key: free slots are trimmed early when disk runs low, and the owner can't see why.
- **Principle:** Brief rule: the window is the owner's only way to set these things; user control and freedom [NNG-H3]
- **Fix:** Replace the Pool sentence with named rows in each project's card: Min slots, Max slots, Clear build output after (days), Remove slot after (days) and Trim when free disk is under (GB). Use small stepper fields, validated as slots.rs does (min can't exceed max), and show the daemon's error in a note row. On each reviewer row add 24px btn_small actions, Pin/Unpin and Never ask/Ask again. Then rewrite both header lines without the CLI: 'Who reviews each project's PRs.' and 'Each project's reusable worktree slots, kept warm between tasks.'

### R5 · 3 Major · Nothing in these pages can be reached from the keyboard
- **Element:** Sidebar sections, QA Off/On, Clean up now, slot pills, all sizes
- **Evidence:** Confirmed
- **Problem:** Only the window root has track_focus. Sidebar rows, kit::seg items and buttons are click-only divs with no focus handle, tab stop or key handler, and no keymap uses the TaskboardSettings context. The owner can't change section, flip QA or run cleanup from the keyboard, and there is no focus style to see. Voice Control and VoiceOver support couldn't be confirmed either.
- **Principle:** WCAG 2.1.1; WCAG 2.4.7; MS-KBD: arrows move within groups
- **Fix:** Give each sidebar row and seg a FocusHandle with tab_index, and put a 2px t.accent focus ring on .focus_visible in btn_base, seg and the sidebar rows. Enter or Space activates. Up and Down move within the sidebar; Left and Right move within a seg, which takes one tab stop. Add Cmd+1 to Cmd+7 for sections, shown in tooltips, if the owner wants them.

### R6 · 2 Minor · QA's Off/On looks live when it's dead, can get stuck On, and shows no saving state
- **Element:** QA › Off/On control, no Jira or qa_busy (qa--light-s.png, qa_page, kit::seg)
- **Evidence:** Confirmed
- **Problem:** Without Jira, clicks are simply not attached. The segments keep cursor_pointer, the hover colour and full-strength text, so a click does nothing and gives no reason. If Jira is removed while QA is on, the control shows On with no way to turn it off. During a POST the control looks unchanged until the answer arrives.
- **Principle:** Disabled controls look disabled [NNG-H1] [WCAG 1.4.11]; user control [NNG-H3]
- **Fix:** Add a disabled option to kit::seg that applies opacity(0.45) and cursor_default and drops hover, and use it when !jira or qa_busy. Always leave the Off segment clickable while on is true, so the feature can be turned off without Jira. While qa_busy, show the target segment as selected at reduced opacity.

### R7 · 2 Minor · Empty states hide real information and give no way forward
- **Element:** Worktrees and Reviewers, empty (worktrees--light-m.png, reviewers--light-m.png)
- **Evidence:** Confirmed
- **Problem:** pools_vm builds pools only from v['slots'], so a project with pool settings but no slots disappears, along with its limits. Waiting tasks are read from slot rows too, so tasks waiting in a project with no slots are never listed. 'No reviewers' and 'No worktree slots' don't say whether that's normal or what happens next. A project whose reviewers array is empty renders its heading over an empty bordered card. If a project's pool entry is missing, the row is a bare 'Pool' label with no text.
- **Principle:** Designed empty states with a way forward [mobile:NNG-EMPTY]; visibility of status [NNG-H1]
- **Fix:** Build pools from the keys of v['pools'] as well as the slots, so every project keeps its heading and settings, with a row 'No slots yet. One is made when a task starts.' Replace the page text with 'No projects have worktree slots yet. Slots appear when a task starts.' and 'No reviewers yet. They're added from each project's PRs.' (confirm the real trigger). Skip a project's card, or show 'No reviewers' inside it, when its list is empty.

### R8 · 2 Minor · Clean up now deletes permanently with no preview or confirm, and reports less than it removes
- **Element:** History › Cleanup row › Clean up now (design/captures/settings/history--light-s.png, days.rs cleanup)
- **Evidence:** Confirmed
- **Problem:** One click permanently deletes rows, and there is no count beforehand and no undo. The note reports events and day summaries, but cleanup also deletes usage samples, host limits and slot spans. It deletes terminal lines (session_events) by date alone, not just for finished tasks, while the page copy says 'of finished tasks' and never mentions terminal output. The header says 'cleans up old work', yet worktree slots aren't touched, so an owner short of disk may expect otherwise. It stays at 2 because the nightly run would remove the same rows anyway.
- **Principle:** Destructive actions confirm or undo [NNG-H3] [NNG-H5]; no false status [NNG-H1]
- **Fix:** Show a confirm before running: 'Remove 1,240 events older than 90 days and 12 day summaries older than 1 year? This can't be undone.' with Cancel focused and a Remove button using kit::btn_danger, fed by a dry-run count from the daemon. Change the row note to 'Removes events and terminal output older than the limits above. Doesn't touch worktrees.' Report every table in the result note and pluralise it, so it never says '1 old events'.

### R9 · 2 Minor · Pinned and Never asked look identical
- **Element:** Reviewers › reviewer row pills (reviewers_page, kit::tone)
- **Evidence:** Confirmed
- **Problem:** reviewers_page asks for tone 'review', which kit::tone doesn't define, so it falls through to the neutral muted-on-grey style. Reviewers the board always asks and reviewers it never asks get the same grey pill. Only the word differs, and the 'Pinned' emphasis was clearly intended.
- **Principle:** One look per role, and different things look different [SEC-4] [NNG-H4]
- **Fix:** Use an existing tone: kit::tone_pill(t, "accent", "Pinned"), or add a "review" arm to kit::tone. Log or test for unknown tone names so a typo can't silently go neutral.

### R10 · 2 Minor · Slot and reviewer lists aren't virtualized
- **Element:** Reviewers and Worktrees › scrolling lists (reviewers_page, worktrees_page)
- **Evidence:** Confirmed
- **Problem:** Both pages build every project card and every row on each render inside a plain overflow_y_scroll div. Pools default to as many as 10 slots per project, the limit can go up to 50, and Worktrees re-renders every 30 seconds, so the page does work for rows that are off-screen.
- **Principle:** Brief rule: long lists virtualized
- **Fix:** Flatten each page into one row list (project heading, pool row, slot rows, waiting row) and render it with GPUI's list or uniform_list, keeping each heading above its rows [L1]. Draw the card edges per row using first and last flags.

### R11 · 2 Minor · Which task holds or awaits a slot is hidden or reduced to bare refs
- **Element:** Worktrees › slot row pill and 'N waiting' row (worktrees_page, pools_vm)
- **Evidence:** Confirmed
- **Problem:** The holder's title appears only in a hover tooltip on a 20px pill, so scanning a pool means hovering each pill, and the keyboard can't reach it at all. The waiting row is a comma-joined list of refs with no titles, so the owner must remember what T231 is.
- **Principle:** Hover is never the only path to information [NNG-FITTS]; recognition over recall [NNG-H6]
- **Fix:** Put the title in the row itself: the note becomes 'T12 Fix login redirect · 7.7 GB · last used 10:08 AM' with kit::clipped truncation, and the pill says 'In use' or 'Free'. List waiting tasks one per row, as ref plus title, under the 'N waiting' label [L1]. This needs the daemon's slots response to carry the waiting tasks' titles.

### R12 · 2 Minor · Content stretches the full window width
- **Element:** QA, Reviewers and Worktrees content column at 1400x1000 (qa--light-l.png)
- **Evidence:** Confirmed
- **Problem:** Cards fill the window, so at 1400 the 'Read QA comments' label and its Off/On control sit about 1,000px apart, and the eye has to cross a blank row to connect them. Empty messages centre in the wide pane, away from the left-aligned title and text.
- **Principle:** Desktop window range: nothing stretches past a readable length at the maximum (checklist pass 7); alignment [SCR-4]
- **Fix:** Cap the page header and list content at max_w(px(760.)), left-aligned at the existing 28px inset, across all sections. Left-align kit::empty inside it. No change is needed at 900 or 720.

### R13 · 2 Minor · 'Pool' and 'Comments' are filler row labels
- **Element:** Worktrees › Pool row; QA › Comments row (worktrees_page, qa_page)
- **Evidence:** Confirmed
- **Problem:** Each label only names the kind of sentence below it. 'Pool' sits inside a card already headed with the project and slot count, and 'Comments' sits inside a card about QA comments, so both add a line without adding meaning.
- **Principle:** Brief rule: no filler labels; everything earns its place [NNG-H8] [SCR-5]
- **Fix:** For Pool, R4's named rows replace the label. If those wait, promote the sentence to the row's main text and drop 'Pool'. For QA, make the value the row: '3 comments read, 1 waiting on you' as the title and 'Last checked 3:05 PM' as the note.

### R14 · 1 Cosmetic · CLI names read as ordinary words and wrap mid-command
- **Element:** Reviewers and Worktrees header lines (worktrees--light-s.png, reviewers--light-m.png)
- **Evidence:** Confirmed
- **Problem:** 'tb project set sizes a project's pool' and 'Agents and tb reviewers change the roster' have no code styling, so they take a second read. At 720 the Worktrees line breaks right after the command.
- **Principle:** Match with the user's words [NNG-H2]
- **Fix:** R4 removes these lines. If they stay, set the command in kit::mono and keep it on one line.

### R15 · 1 Cosmetic · Tinted row pokes out past the card's rounded corners
- **Element:** QA › Read QA comments row, most visible in dark (qa--dark-m.png, qa--dark-s.png, qa--light-s.png)
- **Evidence:** Confirmed
- **Problem:** The card's overflow_hidden doesn't clip the row's fill to the 10px radius, so square brown corners show outside the rounded border. Worktrees' amber waiting row is the last row in its card, so it will likely show the same at the bottom.
- **Principle:** Polish [NNG-H8]
- **Fix:** Give tinted rows matching corners: .rounded(px(10.)) on a single-row card, and rounded_b on a last row.

## What works

- All three pages keep the same frame (title, one line of purpose, cards) and hold up at 720x460 with nothing clipped or lost. Text wraps instead of truncating.
- Project headings sit above their rows, and row text is inset under them, as lesson L1 asks. The two-option Off/On is a fair use of a segmented control; L2's waves rule doesn't apply here.
- Times follow the 12-hour AM/PM rule in used_at and full_time. Text contrast computed from theme.rs passes: muted on seg is about 4.55:1 in light (narrowly) and comfortable in dark.
- The QA note is honest about what the feature does, including that earlier comments are never read. Slots fetch only while Worktrees shows, which avoids measuring disk for nothing.

## Not checked

- Real input: this is a native GPUI window with no page for the driver, so no control was used. Hover, scroll, wheel chaining and clicks are read from code only.
- The inspect step and the composition audit: neither could run, so the squint judgement and any pixel distances are by eye from captures. Contrast was computed by hand from theme.rs for the main pairs only, and bg-on-text pairs were not all checked.
- Populated pages: no capture shows slot rows, waiting rows, pool rows, reviewer rows or pills, QA's On state, the qa-note row or the Clean up result note. Those were reviewed from code.
- Loading ('Asking the board…') and the board-stopped state for these three pages: the down-* captures named in the brief weren't in this unit's 15 files. Only down-history-m.png was seen to exist, and it wasn't opened.
- VoiceOver and the macOS accessibility tree: whether GPUI exposes roles and names here, and how status notes are announced. Not testable from captures. 4.1.2 and 4.1.3 are unverified.
- Menu commands and window behaviour: Cmd+W, Cmd+, and full screen, and whether the app menu covers these commands. The app menu code wasn't reviewed.
- Very long content: project names, many waiting tasks, or a long reviewer note (ids, emails and aliases) at 720 wide. Text zoom has no system equivalent here.
- The real trigger for adding reviewers and creating slots, which R7's suggested wording depends on, and whether the daemon already has a POST route for pool settings, which R4 needs.
- Slot sort order at 10 or more slots (slot-10 against slot-2): set by the daemon, not reviewed.
- Motion and real-device behaviour. One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
