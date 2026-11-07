## Verdict

macOS desktop app, pointer and keyboard. I reviewed one size only, 1360x860 (expanded, light and dark), from 9 captures plus code excerpts; I could not run the app, so nothing here was tested with real input. I did not run the blur audit. By eye, each overlay reads as organised, quiet and correctly above the work, and text contrast passes in both themes. The problems are in how these overlays behave, not how they look. Counts: 0 Catastrophe, 2 Major, 11 Minor, 1 Cosmetic. The top problem is the keyboard. Menu items, attachment rows and reviewer names have no keyboard route, and Esc only closes the reviewer popover, so with the terminal menu or Attached dropdown open it appears to skip them and close the whole panel. The three overlays also dismiss in three different ways, and Attached is the weakest: it stays open when you move to another task.

## Findings

### R1 · 3 Major · Overlay items can't be reached from the keyboard
- **Element:** Task panel › terminal menu items, Attached dropdown rows and hover cards, reviewer names, at all sizes and in both themes
- **Evidence:** Confirmed
- **Problem:** Close terminal, every attachment row and every reviewer name are plain divs with on_click and no focus handle or tab stop (task_panel.rs:1807, 1818, 1533-1559, 1944-1967). With the keyboard alone you can't close a terminal, open an attachment or open a reviewer's popover. A hover card's summary, folder, link, Added row and Copy are pointer-only too (task_panel.rs:1544, 2686). No excerpt anywhere uses track_focus or tab_index. The renderer for the Attached chip and the ⋯ button isn't in the excerpts, so those two are unknown.
- **Principle:** Every function works from the keyboard, and arrows move within menus and lists [WCAG 2.1.1] [MS-KBD]; hover is never the only path [desktop:HIG-POINTER]
- **Fix:** Give c_menu_item, c_drop_item and c_rev_name a FocusHandle and tab stop, with Enter or Space running the same act. On open, move focus to the first item; Up and Down move between items; on close, return focus to the opener. Show the hover card when a row takes focus, with a key for Copy link (Cmd+C on the focused row).

### R2 · 3 Major · Esc closes the reviewer popover but not the other two overlays
- **Element:** Task panel › terminal menu and Attached dropdown, while open
- **Evidence:** Confirmed
- **Problem:** escape() clears only rev_pop (task_panel.rs:783-789), and c_term_menu has no key handling (task_panel.rs:1773-1797). Its doc comment says Esc reaches the panel next (task_panel.rs:782), so Esc with either overlay open should close the whole task panel and leave the overlay's flag set. That last step is inferred: the caller of escape() and the K::Drop renderer aren't in the excerpts. Since the menu reset only runs when the task changes (task_panel.rs:923-927), reopening the same task may show the menu already open.
- **Principle:** Esc cancels transient UI and doesn't navigate [MS-KBD]; users need a clear way out [NNG-H3]; same behaviour for the same kind of thing [NNG-H4]
- **Fix:** Extend escape() to close the top overlay first and return true: if term_menu is set, clear it; else if att_drop is set, clear it; else take rev_pop. Clear all three in close_panel as well.

### R3 · 2 Minor · Attached dropdown stays open across tasks and after use
- **Element:** Task panel › header › Attached dropdown
- **Evidence:** Confirmed
- **Problem:** Changing task resets term_menu and rev_pop but not att_drop (task_panel.rs:923-927), so the next task with attachments opens with its dropdown already covering the timeline. Clicking a row opens the item and leaves the dropdown up (task_panel.rs:1559). The chip's action has no just-closed guard (task_panel.rs:613), though the other two overlays need one for their outside-click close. That suggests Attached has no outside-click close at all; this part is Likely, because its renderer isn't in the excerpts. In attached-open__light__1360x860.png it hides the You and Review steps, which are what the task needs.
- **Principle:** Consistency between like controls [NNG-H4]; transient UI is dismissed the expected way [MS-KBD] [NNG-H3]
- **Fix:** Add u.att_drop = false to the reset at task_panel.rs:923-927. Give K::Drop the same on_mouse_down_out and 300ms just-closed guard as c_term_menu. Close it after a row opens its item.

### R4 · 2 Minor · Dropdown is placed by a fixed offset, not by the chip
- **Element:** Task panel › header › Attached chip and dropdown, when the chips wrap to two lines
- **Evidence:** Likely
- **Problem:** The dropdown is pinned at right_0, top 34px of a wrapping chip row (task_panel.rs:1498). A task with a kind pill, project, Jira status, wave and Attached can wrap in the panel, leaving Attached on row two with the dropdown drawn over it. The chip is then covered, and it may be the only way to close the dropdown (see R3). Even on one row the dropdown lines up with neither chip edge: the left edges are about 26 image px apart in attached-open__light__1360x860.png. No capture shows a wrapped row.
- **Principle:** The control and what it opens stay related and reachable [NNG-H3] [SCR-4]
- **Fix:** Anchor the dropdown to the chip the way c_rev_name does: anchored() from the chip's bottom-left, 6px below it, with snap_to_window_with_margin(px(8.)).

### R5 · 2 Minor · Task and goal attachments are mixed in one ungrouped list
- **Element:** Task panel › Attached dropdown › rows
- **Evidence:** Confirmed
- **Problem:** The list chains the task's attachments and the goal's into one run (view.rs:715), and a row's owner appears, when it's set, only in the caption under the title (view.rs:752-754). The owner can't tell what belongs to this task and what was inherited. attached-open__light__1360x860.png shows only Design, Evidence and Doc, with no owner on any row. This is the problem the owner already corrected in L1.
- **Principle:** Put a group's label above its members and indent the members under it [LESSONS L1]; one section, members grouped by function [SEC-1]
- **Fix:** Add a heading for each owner, 'T2 Sign in with a passkey' then 'G1 Sign-in with passkeys', at 12px muted, with rows indented 8px under it. Remove the ref from the row's meta line. Omit the headings when only one owner has attachments.

### R6 · 2 Minor · Close terminal takes effect at once with no undo
- **Element:** Task panel › Done or Failed step › ⋯ › Close terminal
- **Evidence:** Confirmed
- **Problem:** One click posts close-terminal (task_panel.rs:565), with no confirm or undo. On a failed task, the terminal may be the best record of why it failed, and it sits one menu away from Try again. Merge in the same panel does ask first (task_panel.rs:537). The ⋯ menu is a deliberate guard (view.rs:1373-1374), which is why this rates 2. The same item on a running task sends force in one click (view.rs:1655-1656). What the backend keeps after closing isn't in the excerpts.
- **Principle:** Destructive actions can be undone or ask to confirm [NNG-H3] [NNG-H5]
- **Fix:** Needs the owner's OK, since it changes a one-click signal: route Close terminal through the Modal::finish confirm that Merge uses, at least on failed tasks and for force closes, with the wording 'Close terminal for T2?'. Without that OK, keep the one click and show the terminal's last output in the Failed step, so closing it loses nothing.

### R7 · 2 Minor · Menu helper text repeats the label, and can be false
- **Element:** Terminal menu › Close terminal › second line
- **Evidence:** Confirmed
- **Problem:** 'Closes the terminal it ran in' (view.rs:1382) repeats the label and shows no data, which the owner's rule forbids; see terminal-menu__light__1360x860.png. In the same menu on a live task, 'Stops the command that is running' is always shown, even when the session is idle and no command is running (view.rs:1655-1656).
- **Principle:** Owner's rule: no helper text that shows no data; every element earns its place [NNG-H8]; plain truthful status [NNG-H1]
- **Fix:** Remove the line from view.rs:1382 and have c_menu_item skip the sub div when sub is empty (task_panel.rs:1967). If a second line stays, make it data, such as 'Idle 12m'. At view.rs:1656 show 'Stops the command that is running' only when arg is force.

### R8 · 2 Minor · Empty reviewer popover says the same thing twice
- **Element:** Reviewer popover › head and body, with no open asks
- **Evidence:** Confirmed
- **Problem:** The head says '0 open' and the body says 'No open reviews' (view.rs:2069, 2078), so a 480px card spends its whole body on a fact already stated. See reviewer-popover__light__1360x860.png.
- **Principle:** One fact, one place [SEC-7]; owner's rule against filler; minimalist design [NNG-H8]
- **Fix:** When open is empty, don't push 'No open reviews', and drop the head's bottom border when no list follows. The card then collapses to the name and '0 open', plus 'Reviewed this week' when there is any.

### R9 · 2 Minor · Reviewer names don't look clickable until hovered
- **Element:** Checks and review › Reviewers rows › name
- **Evidence:** Confirmed
- **Problem:** At rest a name is plain text. The underline appears only on hover (task_panel.rs:1816), while nearby links such as Passed, 3 new comments and Merge now are underlined at rest. In reviewer-popover__light__1360x860.png, Dana Reyes and Lee Park read as labels, so the popover is easy to miss.
- **Principle:** Will users notice the action is available? [NNG-CW]; links look like links [NNG-H4]; recognition over recall [NNG-H6]
- **Fix:** Give the name a persistent 1px underline in t.border_2 at rest, turning to t.text on hover, or set the name in t.accent_fg to match the panel's other links.

### R10 · 2 Minor · Reviewer popover doesn't fit the smallest window
- **Element:** Reviewer popover at 900x560
- **Evidence:** Likely
- **Problem:** The ask list has a fixed max height of 520px under a head of about 75px (task_panel.rs:1835), about 595px in all, in a 560px window with 8px margins. The last rows of a busy reviewer's list would end up out of reach. The card is 480px wide and sits left of the name. If the panel stays 480px wide at 900 (not confirmed), the name is near x=476, so the 8px snap slides the card right over the name it came from (task_panel.rs:1821). No capture exists at this size.
- **Principle:** The layout reflows across the whole window range [HIG-MACOS]; focus and content are not obscured [WCAG 2.4.11]
- **Fix:** Set the list's max height from the window: min(520, viewport height - 16 - head height). Below about 1000px wide, drop the card's width to min(480, name x - 26), or open it below the name instead of beside it.

### R11 · 2 Minor · Reviewer and attachment lists aren't virtualized
- **Element:** Reviewer popover › ask list; Attached dropdown › list
- **Evidence:** Confirmed
- **Problem:** rev-list is a plain scrolling div that builds every open ask and every review from this week (task_panel.rs:1835-1842), which breaks the owner's rule. Attached builds a row for every task and goal attachment with no cap (view.rs:715, 719). Whether the dropdown scrolls or has a max height isn't known, as its renderer isn't in the excerpts.
- **Principle:** Owner's rule: long lists virtualized; a list stays usable at many items [pass 7, data states]
- **Fix:** Render both with the virtualized list the task log already uses (sync_log_list). Give the dropdown a max height of min(360, window height - its top - 8) with its own scroller.

### R12 · 2 Minor · Reviewer popover has no retry and no refresh cue
- **Element:** Reviewer popover › error state and reopening
- **Evidence:** Confirmed
- **Problem:** A failed fetch shows 'Couldn't load their reviews: {e}' with the raw backend message and nothing to press (view.rs:2066). The only retry is to close and reopen. Each reopen refetches (task_panel.rs:556-558), but 'Loading…' shows only when nothing is cached (view.rs:2065), so old asks or the old error sit there with no sign of a refresh. Counts can be stale for a moment and look current. No capture shows these states.
- **Principle:** Errors say what happened and what to do [NNG-H9]; system status is visible [NNG-H1]
- **Fix:** Use 'Couldn't load Sam's reviews. Check that the board is running.', keep the raw text in the log, and add a Try again link that calls load_rev_asks. While refetching over cached data, show 'Updating…' at the end of the counts line.

### R13 · 2 Minor · Hover feedback in the menu is almost invisible, and misleads on some rows
- **Element:** Terminal menu items; Attached rows
- **Evidence:** Confirmed
- **Problem:** Menu items hover to t.tint on a t.panel_2 card (task_panel.rs:1785, 1965), which computes to 1.08:1 in light and 1.17:1 in dark, so the destructive item barely responds to the pointer. Attachment rows use the same tint (task_panel.rs:1543); the dropdown's background isn't in the excerpts, though it looks the same in the captures. The tint also applies to rows with no link, which can't be clicked, because only the cursor and click depend on href (task_panel.rs:1557-1560).
- **Principle:** Hover and pressed states are designed and visible [NNG-H1]; looks like what it is [SEC-4]
- **Fix:** Add a menu_hover token, about #dde2e8 light and #343c47 dark, for roughly 1.2:1 against the card, and switch the sub line to t.text_2 on hover so it stays above 4.5:1. Apply the row hover only when href is Some.

### R14 · 1 Cosmetic · Tooltip sits on the open menu, and open state is shown unevenly
- **Element:** Done or Failed step › ⋯ button and terminal menu; Attached chip chevron
- **Evidence:** Confirmed
- **Problem:** The 'Terminal actions' tooltip stays up after the click and overlaps the menu's top edge, in both terminal-menu captures. The ⋯ button shows no open state, while the Attached chip and reviewer name do. The Attached chevron points down whether open or closed (attached-chip__light__1360x860.png, attached-open__light__1360x860.png).
- **Principle:** Selected and open states are designed [NNG-H1]; same thing, same look [NNG-H4]
- **Fix:** Pass None as the tooltip while term_menu is set (view.rs:1380, 1651), give the ⋯ button the on-state look that Look::Chip uses, and flip the chevron up when att_drop is set.

## What works

- Text contrast passes on the overlays: muted on panel_2 is 4.84:1 light and 6.30:1 dark, and Close terminal red is 5.86:1 light and 5.13:1 dark.
- The reviewer name's open state (accent border and soft fill) ties the popover to its source, and the popover leaves the panel's rows readable beside it.
- Hover-card dates and reviewer times use 12-hour AM/PM, as the owner's rule asks.
- Terminal and reviewer overlays reset when the task changes, and their just-closed guards stop a toggle from reopening them on the same click.
- Close terminal is kept off the main row and marked in red, away from Try again; the ⋯ only appears when a live session exists.

## Not checked

- Real input: this is a native GPUI app, so no control was used. The browser driver served only for contrast arithmetic. Pointer, keyboard, focus and dismissal findings come from reading code and captures.
- The hover card itself: attached-hover__light__1360x860.png shows no card and no row hover, so its look, placement, timing and whether it covers the next row are unseen. Its Esc dismissal is also unknown.
- Code not in the excerpts: the K::Drop renderer, the Btn renderer (focus on the Attached chip and ⋯), the caller of escape(), close_panel, the TlActs layout, and the bodies of ask_row and answered_row.
- Reviewer popover with real asks, the Reviewed this week fold, the tinted current-task row, Loading and error: captures show only the empty state.
- Terminal menu open on a failed task: only the closed state is captured, though it uses the same code as the Done menu.
- Sizes other than 1360x860, including the 900x560 minimum, short windows and very wide windows. Text scaling and window zoom were not tested.
- Exact target sizes: the ⋯ button measures about 24px from the capture, right at the WCAG floor, and its Look::Icon size isn't in the excerpts.
- Focus ring appearance, and screen reader output (VoiceOver), including names, roles and announcements of notes such as 'Copied'.
- What the backend does on close-terminal, including whether terminal output is kept, and motion or open/close timing.
- Attachment edit, remove and the att-menu actions in the same code: outside this unit's scope, and not reviewed against the read-only rule.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
