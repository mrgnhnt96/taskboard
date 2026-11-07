## Verdict

macOS desktop, pointer and keyboard, reviewed at 1360x860, 1360x1800 and 900x560. Evidence is 12 dark-theme captures plus backlog.rs, paged.rs, kit.rs, theme.rs and triage.rs. The app is native GPUI, so nothing was driven with real input: behaviour findings are confirmed from code, and contrast was computed from the theme hex values. First impression, judged by eye because the blur audit can't run on a native app: the list reads as one even stack of identical bright cards with no hierarchy, and the strongest outline on the screen is the purple border of an empty card. The page looks tidy at sample size and turns monotonous and hard to scan at 120 issues. Counts: 1 Catastrophe, 7 Major, 6 Minor, 1 Cosmetic. Top problem: at the smallest window the 380pt goal card stays reserved while empty, the list shrinks to 196pt, and every issue title disappears, so the page can't do its job. Close behind: at 120 issues the group counts, Select all and the P1 group cover only the first 100 issues, and nothing says so.

## Findings

### R1 · 4 Catastrophe · Issue titles vanish at the smallest window
- **Element:** Backlog › list › rows and group headers, 900x560, 123 issues (vol-small.png)
- **Evidence:** Confirmed
- **Problem:** The sidebar (248pt), padding (56pt), gap (20pt) and fixed goal card (380pt) leave the list 196pt. Rows then show only kind, ref, NEW and area: the title collapses to nothing, P pills and impact bars are cut off, and the group header clips to 'Test gaps in the daemon' with its count and description gone. The empty card is nearly twice as wide as the list. The header, lenses, chips and a three-line help text also push the first row down to about 296pt of 560. The owner can't scan or pick at this size.
- **Principle:** The work dominates and narrowing keeps the order [SCR-1] [SCR-6]; every control at every size, and the desktop window range works across the whole range [HIG-MACOS] [NNG-H6]
- **Fix:** Don't render #plan-draft while d.count == 0, which gives the list all 596pt at this size. Below about 1100pt of window width, stack the draft under the toolbar at full width as a one-line strip ('3 issues · not planned') that expands in place, rather than a side column. Give the title a min width of 240pt and drop columns in a set order as the list narrows: area first, then NEW. Add truncate to the group label so count and description survive. This respects the one-home rule: the draft isn't a goal peek.

### R2 · 3 Major · Group counts, Select all and the P1 group silently cover only the loaded page
- **Element:** Backlog › list › group headers and rows, all lenses, 123 issues (vol-priority.png, vol-impact.png, vol-base.png)
- **Evidence:** Confirmed
- **Problem:** groups() runs on held rows only, and PAGE is 100, so with 121 untriaged the first view holds 100. 'P1 32' or 'High impact 35' may be short, and older P1s stay hidden until the owner scrolls to within 40 lines of the end. The Priority help says 'P1 first', but it isn't the full set. Select all picks only held rows, so a goal can silently miss part of a group. When the older page lands, its rows join groups above the viewport. sync_view keeps item_ix rather than anchoring to an issue, so the list jumps, and Similar groups can reorder because they sort by size.
- **Principle:** Visibility of system status [NNG-H1]; readouts tell the truth [NNG-H1]; no surprises while working [WCAG 3.2.2 in spirit]
- **Fix:** Keep paging and virtualization (owner's rule) but make the server page in lens order: add sort=similar|area|impact|priority to /backlog/triage, ordered by group then newest, so a group is complete before the next begins. Send per-group totals with the first page ([{key, count}]) so headers show true counts. Make group Select all pick by group key on the server, or show '32 of 41 loaded'. Anchor the scroll to the top issue's ref rather than item_ix.

### R3 · 3 Major · Nothing on the page can be reached from the keyboard
- **Element:** Backlog › lenses, kind chips, group checks, row checks, titles, 1360x860 and all sizes
- **Evidence:** Confirmed
- **Problem:** There is no tab_index or focusable anywhere in ui/, and these controls are on_click-only divs with no focus style. on_key returns early when nothing is selected, so M, P, D and X work only after a mouse pick. The owner can't move through rows, check one, open one, change lens or toggle a kind from the keyboard, and there is no focus indicator. The controls also expose no role or name in this code. Under the guide a keyboard-only user is blocked, which would be a 4. It is rated 3 because the one named user has a pointer, so for him it costs speed on every triage rather than blocking him.
- **Principle:** Keyboard [WCAG 2.1.1], focus visible [WCAG 2.4.7], name, role, value [WCAG 4.1.2], arrow keys inside groups [MS-KBD], flexibility [NNG-H7]
- **Fix:** Add a focused-row cursor to the list: Up/Down (or J/K) move it and scroll it into view, Space toggles the check, Shift+Up/Down extends, Return opens the issue in its panel, and Left/Right on a group header collapses it. Draw a 2pt t.accent ring on the focused row. Give the lens segment and kind chips tab stops: Left/Right moves the lens, Space toggles a chip. Add Cmd+Alt+1-4 for lenses, avoiding the Cmd+1-6 page keys already bound.

### R4 · 3 Major · Rows have no column rhythm and one orange means four things
- **Element:** Backlog › list › rows, 123 issues, 1360x860 and 1360x1800 (vol-base.png, vol-impact.png, vol-tall.png)
- **Evidence:** Confirmed
- **Problem:** The row is the primary surface and it resists scanning. Evidence: (1) kind pills vary in width, so refs and titles start at four x positions about 39pt apart (B9, B20, B73, B76), and area text starts wherever its word ends [SCR-4]. (2) The warn hue marks Test gap, NEW, high impact, P1 and the header count, so a row such as B73 or B12 carries orange that can't be told apart, and P1 doesn't stand out [SEC-5]. (3) Every row is a bordered card at a 46pt pitch, so only about 13 of 121 issues fit at 860pt [SEC-2]. (4) Priority and Impact lenses still repeat a value that is constant inside the group, while the Area lens hides its column [SEC-7].
- **Principle:** Alignment [SCR-4], one emphasis [SEC-5], grouped by space [SEC-2], one fact one place [SEC-7], everything earns its place [SCR-5] [NNG-H8]
- **Fix:** Lay the row out as fixed columns: check 18, kind 76, ref 40 right-aligned, title flex, area 96, impact 20, priority 32. Replace per-row cards with flat rows and a 1pt t.divider line at about a 32pt pitch, which fits about 22 rows at 860pt. Keep warn for P1 and high impact only. Move Test gap to the results hue (#4fc3d9 on #123139, 6.65:1) and make NEW a plain muted text tag after the title. Hide the column the lens already states: priority pill in Priority, bars in Impact. Make the header count t.muted unless there is something urgent to signal.

### R5 · 3 Major · Helper text and an empty card hold prime space at rest
- **Element:** Backlog › New backlog goal card and lens help line, all lenses and sizes (backlog.png, lens-area.png, vol-base.png)
- **Evidence:** Confirmed
- **Problem:** At rest, 380pt of a 1112pt content area holds one sentence, 'Check issues on the left to add them to this goal', and its purple outline is the strongest edge on the screen, so the eye goes to nothing. Meanwhile titles truncate beside it (B20, B51, B76, B82 in vol-base.png). This breaks the owner's rule against helper text that shows no data. The lens help line breaks it too, on every lens, and it can be false: with AI grouping off, the Similar lens still says 'Claude groups issues', though groups come from stored group or kind. triage.rs sends an ai flag that the page ignores.
- **Principle:** Owner's rule: no filler labels or helper text that shows no data; the work dominates [SCR-1]; squint test [SCR-2]; aesthetic and minimalist design [NNG-H8]
- **Fix:** Render the card only when d.count > 0. The check column doesn't move when it appears, because the list is left-anchored. Delete Line::Help and put Lens::help() in a tooltip on each segment. Read the ai flag and word the Similar and Priority tooltips to match, for example 'Grouped by kind' when AI is off. At 1360pt this gives the list 1112pt instead of 712pt and removes all the truncation in vol-base.png.

### R6 · 3 Major · Refs, counts, off chips and checkbox edges fail contrast
- **Element:** Backlog › rows, group headers, kind chips, dark and light themes
- **Evidence:** Confirmed
- **Problem:** Computed from theme.rs. t.faint text: ref on card 3.80:1 dark and 3.04:1 light; group count and off-chip label on page 4.31:1 dark and 2.84:1 light, all under 4.5:1. Off chips aren't disabled, so they aren't exempt. The checkbox border, on the page's most-used control: t.border_2 on card is 1.63:1 dark and 1.48:1 light, and 1.85:1 on the dark page, under 3:1. Refs are how the owner names issues to agents, and an unchecked box barely shows (visible in vol-base.png).
- **Principle:** Contrast minimum [WCAG 1.4.3]; non-text contrast [WCAG 1.4.11]
- **Fix:** Use t.muted for refs, group counts and off-chip labels (7.12:1 on dark card, 5.43:1 on white). Give unchecked boxes a border of #6f7a88 in dark (3.80:1 on card, 4.31:1 on page) and #5f6b7a in light (5.43:1 on white). Light-theme t.faint at 3.04:1 should be raised at the token for every page, not just this one.

### R7 · 3 Major · At 120 issues the list can't be scanned by group or by what matters first
- **Element:** Backlog › list › group headers and row order, 123 issues (vol-area.png, vol-impact.png, vol-tall.png)
- **Evidence:** Confirmed
- **Problem:** Group headers scroll away, so three screens into 'High impact 35' the owner has lost the label and its Select all. Groups can't collapse, so there is no overview of what groups exist: even 1800pt tall shows only two. Inside a group, rows stay newest first, so in the app group of vol-area.png priorities run P1, P3, P1, P1, P1, P3 and the important ones are scattered. There is no find within the backlog. Scan, the first step of the primary task, becomes a long read.
- **Principle:** Flexibility and efficiency of use [NNG-H7]; recognition rather than recall [NNG-H6]; frequent in view, rare behind [SEC-8]
- **Fix:** Pin the current group's header to the top of the list, drawn as an overlay from logical_scroll_top() so virtualization stays. Make headers collapsible with a chevron, plus Collapse all, and keep that state per lens. Sort rows inside a group by priority, then impact, then newest, on the server with R2's lens ordering. Add a filter field beside the kind chips that matches title and ref. All of this is view state, so read-only and paging rules hold.

### R8 · 3 Major · Rows don't show their project, so Select all can dead-end
- **Element:** Backlog › rows and group check, All projects view, 123 issues (vol-base.png, vol-area.png)
- **Evidence:** Confirmed
- **Problem:** A goal holds one project, but in the default All projects view rows show area, never project. Groups key on area or group alone, so one group can span projects. The owner ticks a group, and only then does the card say 'A goal holds one project's work. Check issues from one project.', with Create blocked and no sign of which rows to remove. triage.rs also falls back to the project name when area is empty, so the area column mixes two meanings, and the Area lens says 'part of the app' while grouping some rows by project.
- **Principle:** Error prevention [NNG-H5]; cognitive walkthrough, will users see the right action [NNG-CW]; consistency [NNG-H4]
- **Fix:** While the project filter is 'All projects' and more than one project is present, show the project in the row's area column as 'project · area', and sub-group or split groups by project in the Area and Similar lenses. Make group Select all pick from one project at a time. Name the projects in the mixed message, for example 'webapp 3, api 2', and show an '(no area)' label instead of falling back to the project name.

### R9 · 2 Minor · Truncated titles can't be read, and the tooltip covers them
- **Element:** Backlog › rows › title, hover, 1360x860 (hover-row.png, vol-base.png)
- **Evidence:** Confirmed
- **Problem:** Every title has the same static tooltip, 'Open the issue: what was said, its history, reopen'. It never shows the full title, so a cut title such as B20 or B82 can only be read by opening the panel. It also sits on the second half of the title and the NEW pill (hover-row.png), it is helper text with no data, and 'reopen' doesn't apply to an open issue.
- **Principle:** Owner's rule on helper text; recognition rather than recall [NNG-H6]; hover content must not obscure [WCAG 1.4.13]
- **Fix:** Replace the title's text and tooltip with kit::clipped(), which already shows the whole text only when it was cut. Drop the static string. Position the tooltip below the row so the title stays readable.

### R10 · 2 Minor · Picking is a small target with no row hit area or range select
- **Element:** Backlog › rows › check box, hover state (hover-row.png)
- **Evidence:** Confirmed
- **Problem:** Picking is the page's most frequent action, yet the box is 18x18pt. It passes the 24pt floor only by spacing. It has no hover state, and the row has none either: only the title changes colour on hover. Clicking the rest of the row does nothing, and there is no Shift-click range, so picking eight neighbours in a 35-row group takes eight precise clicks.
- **Principle:** Target size and frequent targets larger [WCAG 2.5.8] [NNG-FITTS]; states are designed [NNG-H1]; flexibility [NNG-H7]
- **Fix:** Extend the check's hit area to the full row height and about 40pt wide. Add a t.panel_2 row hover. Make a click on empty row space toggle the check, leaving the title as the open action. Add Shift-click for a range in the visible order.

### R11 · 2 Minor · Kind chips hide their state and count, and the empty filter has no way back
- **Element:** Backlog › kind chips and filtered empty state (kinds-none.png, backlog.png)
- **Evidence:** Confirmed
- **Problem:** Chips look the same as the static kind pills in rows, but they toggle, and they have no hover state. They carry no counts, so the owner can't tell which kinds have issues: in kinds-none.png Test gap and Clean-up stay on but match nothing. Clean-up's on state is a fill at 1.07:1 against the page, so it reads almost like the off outline at 1.41:1. Hiding everything but Bugs takes three clicks. The empty message 'No issues of the kinds shown.' offers no way forward while the header still says 2 issues.
- **Principle:** One look per role [SEC-4]; use of colour and non-text contrast [WCAG 1.4.1] [WCAG 1.4.11]; empty states have a way forward [mobile:NNG-EMPTY]; user control [NNG-H3]
- **Fix:** Add a count to each chip ('Bug 58'), from the server totals in R2. Add a hover state and a check mark on active chips so state doesn't rest on the fill. Give Clean-up's on state a t.border_2 outline or t.seg fill. Option-click solos a kind. Change the empty message to '2 issues hidden by the kind filter' with a 'Show all kinds' link that resets the chips.

### R12 · 2 Minor · Loading and errors show as success, and refresh failures are silent
- **Element:** Backlog › heading status and progress meter, list end; loading and error states (from code)
- **Evidence:** Confirmed
- **Problem:** While data is None, untriaged is 0 and total is 0, so 'Loading…' or the error text draws in green t.up_fg over a full green meter (pct = 1), which says all triaged. This recurs on each project switch, which clears data. Once data exists, a failed refresh sets err but nothing shows it, so the list goes stale without notice. A failed older page sets next to None in land_older, so the list just ends, and 'No issues of the kinds shown.' can then be false.
- **Principle:** Visibility of system status [NNG-H1]; help users recognize and recover from errors [NNG-H9]
- **Fix:** While data is None, draw the meter empty in t.seg and the status in t.muted, and draw an error in t.down. Show err in the header whenever it is set, with a Retry. Keep the older-page error in Paged and show 'Couldn't load older issues · Retry' in Line::End instead of clearing next.

### R13 · 2 Minor · Rows aren't indented under their group label
- **Element:** Backlog › list › group header and rows, all lenses (backlog.png, vol-base.png)
- **Evidence:** Confirmed
- **Problem:** The header's checkbox lines up with the row checkboxes and the group label lines up with the kind pills, so a header reads as one more row. Only bold type marks the hierarchy, which is what the owner corrected before on the goal page.
- **Principle:** LESSONS L1: a group's label goes above its members and the members are indented under it; grouped by space [SEC-2]
- **Fix:** Indent rows 28pt so their left edge sits under the group label at x = 362 in the capture, leaving the group check in its own gutter. Raise the gap above a header from 14pt to 20pt. Do this with R5, so the 28pt comes out of the width that change frees.

### R14 · 2 Minor · AI grouping status scrolls away while groups shift
- **Element:** Backlog › list › 'Claude is grouping new issues…' line, Similar lens (judged from code; no capture)
- **Evidence:** Confirmed
- **Problem:** Line::Grouping is a list item near the top, so it is out of view once the owner scrolls, and it has no spinner or count. It shows on every lens, though only Similar is affected. Ungrouped issues sit in kind-guessed groups, with no marker for that: the server sends grouped and the page ignores it. Groups then reshuffle on a 3-second sync while the scroll position stays by index, so rows move under the pointer, risking a click on the wrong check.
- **Principle:** Visibility of system status [NNG-H1]; no surprises [WCAG 3.2.2 in spirit]; status messages [WCAG 4.1.3]
- **Fix:** Move the status to the header line beside the count, for example 'Grouping 12 new issues…', shown on the Similar lens only. Hold regrouping while the pointer is over the list or anything is checked, and show 'Regrouped · Refresh' instead. Anchor scroll to the top issue, as in R2.

### R15 · 1 Cosmetic · The progress meter says little at volume
- **Element:** Backlog › heading › meter, 123 issues (vol-base.png)
- **Evidence:** Confirmed
- **Problem:** At 2 of 123 the fill is a dot, and the numbers exist only in a hover tooltip the keyboard can't reach. The total also counts every issue ever, and the tooltip's 'in a goal, deferred or dropped' doesn't cover closed ones. The heading 'Plan from the backlog' also differs from the nav label 'Backlog'.
- **Principle:** Visibility of system status [NNG-H1]; consistency [NNG-H4]
- **Fix:** Replace the tooltip with visible text only if it earns the space: fold it into the status as '121 of 123 not in a goal yet' and drop the bar until triaged share passes about 5%. Correct the tooltip's wording, and title the page 'Backlog'.

## What works

- The lens control is a fixed set of four, so a segmented control fits and doesn't break LESSONS L2. Kinds are fixed too.
- The list is virtualized and paged as the owner requires, the lens or filter change returns to the top while a refresh keeps the place, and checks survive a lens change.
- Kind pills all pass contrast (5.46:1 to 10.6:1 dark) and carry text, and impact uses bar count plus a tooltip, so neither relies on colour alone.
- The project filter appears only with more than one project, and page shortcuts stand down while its search field is open, with a test covering it.

## Not checked

- Real input: the app is native GPUI and the browser driver can't reach it, so no control was used with trusted events. Keyboard, hover, scroll and paging findings come from code and captures. The browser was used only to compute contrast.
- The composition audit and blurred squint test: audit.mjs needs a web page. The first impression was judged by eye from the captures.
- VoiceOver output: no accessibility code was found in ui/, but GPUI framework behaviour wasn't tested.
- Light theme: no captures. Light contrast comes from theme.rs hex values only.
- AI grouping in progress: judged from code as the brief says. Real Similar groups with AI on weren't seen at volume, since 'vol' captures have AI grouping off.
- Fully triaged empty state, loading and error states: no captures, code only.
- All kind chips off: kinds-none.png is listed as every chip off, but Test gap and Clean-up are still on in the image. Code shows the same message either way.
- Windows wider than 1360pt: the 1320pt max width and left-anchored layout weren't captured.
- 200% text, reduced motion and scroll or wheel behaviour on the real app.
- Selection bar, draft planning and Create are outside this at-rest unit and weren't rated. Two things noticed in code for the review that covers them: the X key drops all checked issues at once, with no confirm or undo in this file, and checked rows hidden by a kind filter stay selected, so bulk actions reach rows the owner can't see.
- 12-hour time rule: this page shows no clock times, so it doesn't apply.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
