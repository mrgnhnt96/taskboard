## Verdict

Reviewed the macOS desktop Releases overview (GPUI, pointer and keyboard) at 900x560, 1360x860 and 1728x1084, in dark at all three sizes and light at 1360x860. Evidence is four captures plus releases.rs and the app.rs, kit.rs and theme.rs code around it. The native app could not be driven, so behaviour comes from reading the code. First impression: the page reads as organised and calm. Version numbers and the blue Unreleased tile lead the eye, and chrome stays quiet, but the two cards fill only part of the space at the larger sizes. Counts: 0 catastrophe, 3 major, 8 minor, 1 cosmetic. The top problem is that the overview does not answer one of its primary questions: "v0.14.0 pending" gives no progress, and a cut that can no longer finish looks the same as a healthy one. The other majors are that no control on the page can be reached by keyboard, and that a failed project fetch shows "Loading…" forever. Data-related findings note where the sample may be the cause.

## Findings

### R1 · 3 Major · The card hides how far the next cut has got, and whether it is dead
- **Element:** Releases › project card › Unreleased tile › pending pill, all sizes and themes (webapp in releases-dark-m.png, releases-light-m.png)
- **Evidence:** Confirmed
- **Problem:** The pill says only "v0.14.0 pending". To learn what the cut waits for, the owner must open each project page. A cut with a Dead item ("the cut will be dropped") looks the same as a healthy one, so the one state that needs attention is invisible on the overview. While cutting, the pill also keeps the warn tone (line 774), although the project page uses the working tone for the same state (line 454).
- **Principle:** Visibility of system status [NNG-H1]; recognition rather than recall [NNG-H6]; frequent in view, rare behind [SEC-8]
- **Fix:** Build the pill from the existing Pending::progress(): "v0.14.0 · 1 of 3 ready", or "v0.14.0 · Cutting" in the working tone. If any item is Standing::Dead, use the failed tone and "v0.14.0 · Can't finish". Make the header summary count these too, for example "1 pending cut · 1 can't finish". This is display only and adds no actions.

### R2 · 3 Major · Nothing on the overview works from the keyboard
- **Element:** Releases › every card control ("N releases" link, latest block, Unreleased tile, previous release rows), all sizes
- **Evidence:** Confirmed
- **Problem:** Every control is a div with on_click, with no focus handle. A search of crates/taskboard-app/src finds no tab_index or tab stop at all, so this limit is probably app-wide. Unlike Board, Projects, Goals, Backlog and Days, Releases also has no Cmd-number shortcut. A keyboard user cannot reach the page's controls, open a release or see focus. This is an all-day tool for a developer on a Mac.
- **Principle:** Keyboard [WCAG 2.1.1]; focus visible [WCAG 2.4.7]; arrows move inside groups [MS-KBD]
- **Fix:** Give each card target a tab stop with a 2px accent focus ring, with Enter opening it. Inside a card, make the rows one stop that Up and Down arrows move through. Add a GoReleases action on Cmd-4, which is unbound today, and list it in the View menu. The same pattern is needed across the app, so the kit is the place to add it.

### R3 · 3 Major · A failed project fetch shows "Loading…" forever
- **Element:** Releases › project card › loading state, from code (overview_card lines 736-738; app.rs lines 1462-1466)
- **Evidence:** Confirmed
- **Problem:** A failed GET /releases is dropped by `if let Ok(v)`, so the list stays None and the card reads "Loading…" with no end and no error. The header summary also stays hidden. If the board is down, the banner explains it, but the card still claims to be loading. If only that project's call fails, nothing on screen says anything is wrong.
- **Principle:** Visibility of system status [NNG-H1]; help users recognize and recover from errors [NNG-H9]
- **Fix:** Keep the last error per project in releases::State. In the card, show "Can't load api's releases." with a Try again link that calls refresh. Refreshing is not an edit, so it fits the read-only rule. When m.down is set, show "Can't load the board." instead of "Loading…".

### R4 · 2 Minor · Header claims "0 unreleased tasks" before anything has loaded
- **Element:** Releases › page header summary › Loading and Can't load states, from code (line 823)
- **Evidence:** Confirmed
- **Problem:** With no state yet, projects is empty, so `lists.iter().all(Option::is_some)` is true and the summary renders "0 unreleased tasks" beside "Loading…" or "Can't load the board.". The page reports a clean count it does not have.
- **Principle:** Commands and status never report what they didn't have [NNG-H1]
- **Fix:** Add `!projects.is_empty() &&` to the condition, so the summary appears only once every listed project has answered.

### R5 · 2 Minor · Cards do not say what shipped
- **Element:** Releases › project card › latest release block, all sizes
- **Evidence:** Confirmed
- **Problem:** The primary task includes checking what is in each release, but the card gives only the version, time and task count. Row.headline, the first line of the notes, is computed and never shown on the overview. The owner has to open each project to learn what a release was about. The sample may have no notes, but the code never shows them either way.
- **Principle:** Recognition rather than recall [NNG-H6]; frequent in view [SEC-8]
- **Fix:** Add the headline under the version at 13px text_2, one truncated line, and render it only when notes exist so no blank line or filler appears.

### R6 · 2 Minor · Failed row cuts the time and loses PM
- **Element:** Releases › project card › previous release row at 1360x860 ("Oct 21, 8:38…" in releases-dark-m.png, releases-light-m.png)
- **Evidence:** Confirmed
- **Problem:** At a 341px card, the Failed pill squeezes the flexible time column, so the time truncates and loses AM/PM. At 344px (releases-dark-l.png) it just fits, so 3px decides it. Rows from a previous year ("Oct 21, 2025, 8:38 PM") are longer and will probably cut even without a pill.
- **Principle:** Owner rule: 12-hour times with AM/PM; sections align and hold at each width [SCR-4]
- **Fix:** Stop the time from shrinking (flex_none, whitespace_nowrap), and put the Failed pill in the count's slot, since a failed release's work has gone back to Unreleased and "0 tasks" adds little. Also raise CARD_MIN from 340 to 380 so a prior-year date fits beside the count.

### R7 · 2 Minor · Grid reserves columns for projects that don't exist
- **Element:** Releases › card grid at 1360x860 and 1728x1084 (releases-dark-m.png, releases-dark-l.png)
- **Evidence:** Confirmed
- **Problem:** columns() counts what fits, not how many projects there are, so two projects get a 3-column grid at 1360 and a 4-column grid at 1728, padded with empty spacers. At 1728 about half the main area is empty, while the cards stay about 344px wide and tight enough to nearly cut text. The two-project count is the sample's, but any board with fewer projects than columns does this. The task panel floats over the right edge, which may explain some of the free room, but its 480px does not account for the empty columns.
- **Principle:** The work dominates [SCR-1]; large displays used to show more [HIG-MACOS]
- **Fix:** Use cols = min(fit, projects.len()), remove the spacers when there is only one row, and cap cards at max_w 560. That gives two 520px cards at 1360 and two 560px cards at 1728.

### R8 · 2 Minor · Latest block and Unreleased tile give no sign they are clickable
- **Element:** Releases › project card › latest release block and Unreleased tile, pointer hover
- **Evidence:** Confirmed
- **Problem:** Both have a pointer cursor but no hover state, while the previous rows just below do highlight (lines 788-791 against 747-775). The Unreleased tile reads as a status strip, so the owner may not find the route to what the next cut waits for, and the same card treats clickable items in two ways.
- **Principle:** States are designed [NNG-H1]; one look per role [SEC-4]; consistency [NNG-H4]
- **Fix:** Give both the same hover as the rows: panel_2 on the latest block, and one step brighter on the tile. Add a trailing "›" in muted to the tile.

### R9 · 2 Minor · "Unreleased 0 tasks" takes a full tile and leads nowhere useful
- **Element:** Releases › api card › Unreleased tile (releases-dark-m.png, releases-light-m.png, releases-dark-s.png)
- **Evidence:** Confirmed
- **Problem:** With nothing waiting, the tile still takes about 44px at the same size as a live one and carries no news, which works against the owner's no-filler rule. Clicking it opens the project page, where top() leaves out the Unreleased section altogether, so the click does not lead to anything about Unreleased. At 900x560 it also helps push webapp's rows below the fold.
- **Principle:** Everything earns its place [SCR-5]; aesthetic and minimalist design [NNG-H8]; owner rule: no filler labels
- **Fix:** Render the tile only when unreleased > 0 or a cut is pending. The header summary already gives the total across projects.

### R10 · 2 Minor · "N releases" link is under the 24px target floor
- **Element:** Releases › project card › header › "1 release" / "9 releases" link, all sizes
- **Evidence:** Confirmed
- **Problem:** The link's height is its 13px text at 1.4 line height, about 18px. The head has no bottom padding, so it sits directly against the latest block's click area and fails the spacing exception. A slightly low click opens the release in place of the list. Figures are read from code, not measured.
- **Principle:** Target size minimum [WCAG 2.5.8]; adjacent targets spaced [NNG-FITTS]
- **Fix:** Give the link h 24 with items_center, and add 4px of padding under the head so the two targets don't touch.

### R11 · 2 Minor · A project with no releases gets no empty state
- **Element:** Releases › project card › no releases state, from code (lines 745-758)
- **Evidence:** Confirmed
- **Problem:** A project that has releases turned on but no cuts shows a blue "0 releases" link and a grey "Unreleased 0 tasks" tile, with no version line and no message. It looks broken or half loaded. The project page does say "No releases yet", but the overview doesn't.
- **Principle:** Empty states are designed [mobile:NNG-EMPTY]; consistency [NNG-H4]
- **Fix:** When latest is None, show kit::help("No releases yet") where the version sits, and replace the "0 releases" link with plain muted text or remove it.

### R12 · 1 Cosmetic · Slashed zeros in running text
- **Element:** Releases › card meta and counts ("0 tasks", "3:50 PM", "v0.14.0" in the pill), all captures
- **Evidence:** Confirmed
- **Problem:** Zeros in the proportional UI font draw with a slash, so "0 tasks" reads a little like "Ø tasks". A slashed zero helps in mono refs but is noise in prose and times. The theme sets no such feature, so the source is the font or a kit default.
- **Principle:** Type scale and one UI font role [SCR-3]
- **Fix:** Turn off the slashed-zero variant for ui_font and keep it on mono_font only.

## What works

- Text contrast passes in both themes, computed from theme.rs. Dark: muted on card 7.12:1, link 6.35:1, warn pill 8.5:1, failed pill 5.46:1. Light: muted 5.43:1, link 5.11:1, warn pill 6.37:1, failed pill 5.6:1.
- Hierarchy on the card is clear: project, then a large mono version, then time and count, then Unreleased, then history. The group label sits above its members, as LESSONS L1 asks.
- The page keeps to read-only, with no cut or edit controls, and full dates use 12-hour times with AM/PM. Failed is shown in words as well as colour.
- At 900x560 the grid falls to one column cleanly, and the cards keep their structure without cutting text.

## Not checked

- Real input: this is a native GPUI app, so the browser driver can't reach it. Clicks, hover, wheel and scroll were not exercised, and behaviour is from code. The driver was used only to compute contrast.
- The composition audit and blurred screenshot were not run, because they need a page. The first impression comes from looking at the captures.
- Loading, can't load, no releases, cutting, and a failed newest release have no captures. They were read from code only, as the brief set out.
- Light theme at 900x560 and 1728x1084, and the sizes between the captures. Column changes were worked out from columns(), not seen.
- Whether the task panel stays open on the overview, and what it covers. It floats at 480px over the right edge, so at 1360 it could cover a third column. Boards with three or more projects were not seen.
- Very long project or version names, large counts and prior-year dates: the sample doesn't show them, so R6's prior-year case is reasoned from the code.
- Whether the sample has release notes, and whether two projects, nine releases or "0 tasks" on a failed release are only the sample's data.
- VoiceOver and accessibility names: no accessibility API use turned up in the code search, but no screen reader was run. Text scaling, macOS Increase Contrast and Reduce Motion were not checked either.
- The project page (Page::ReleasesOf), Days and the weekly review are outside this unit, so the "read a day or a week" and "act on the weekly review" tasks were not walked. The sidebar was seen only as context.
- Theme token use and reuse across screens belong to theme-review. One reviewer finds only some of the problems, so a second reviewer or a usability test will find more [NNG-HE].
