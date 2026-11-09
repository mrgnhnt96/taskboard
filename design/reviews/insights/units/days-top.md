## Verdict

macOS desktop app (GPUI), pointer and keyboard, reviewed with the desktop rules at 900x560 (compact height), 1360x860 and 1728x1084, dark and light at 1360. Evidence is days.rs lines 1-1138, the slot, host and usage row code in days-lower.rs, theme.rs, kit.rs and five screenshots; nothing was driven with real input, because this is a native app and not a browser page. First impression: the page reads as organised, with a clear header, an even tile row and consistent cards. No blurred audit was run, but by eye the saturated week chart takes the attention, while the timeline, which is the day itself, is pale and mostly empty. Counts: 0 Catastrophe, 4 Major, 11 Minor, 3 Cosmetic. The top problem is that nothing on the page can be reached from the keyboard. Close behind, the timeline's state colours have no key, its fills are almost invisible when unlabelled, and the first KPI row shows numbers that don't add up.

## Findings

### R1 · 3 Major · No Days control can be reached from the keyboard
- **Element:** Days › header (project toggles, ‹ › Today, Day/Week), week card (columns, day tiles), timeline (bars, marks), at every size and theme
- **Evidence:** Confirmed
- **Problem:** None of these controls has tab_index or track_focus, and no key is bound for stepping dates; the only Days binding is Cmd-6 to open the page. A keyboard user can open the page but can't change the day, filter a project, open a task from a bar or read any tooltip. There is no focus indicator because nothing takes focus.
- **Principle:** Every function works from the keyboard, with a visible focus indicator; arrows move inside groups [WCAG 2.1.1] [WCAG 2.4.7] [MS-KBD]
- **Fix:** Give toggles, ‹ ›, Today, Day/Week, day tiles and bars a tab stop with a 2px t.accent focus ring, and make Enter or Space activate them. Treat the day tiles as one group with Left and Right arrows. Bind Left and Right to earlier and later day and T to Today when no text field has focus (Cmd-Left and Cmd-Right are already NavBack and NavForward). Show the tooltip on focus as well as on hover.

### R2 · 3 Major · Timeline bar states are colour-only and have no key
- **Element:** Days › timeline card › bars and legend, all sizes, both themes (days-dark-m.png, days-light-m.png)
- **Evidence:** Confirmed
- **Problem:** The legend covers only the five marks. Green, blue, orange, red and grey bars are never explained, so reading T1 as done or the second T3 bar as waiting on you means hovering each bar. Colour is the only resting signal, and pairs share a colour: failed and lost are both red, stopped and away are both grey.
- **Principle:** Colour isn't the only signal [WCAG 1.4.1]; recognition rather than recall [NNG-H6]
- **Fix:** Add a bar key to the legend row as a separate group: Working, Done, Waiting on you, Failed or lost, Stopped or away. Add a non-colour cue on bars: a diagonal hatch on waiting, a left 2px edge on failed or lost, and a dashed outline on stopped or away. This keeps the tooltip's wording for the exact state.

### R3 · 3 Major · Timeline fills nearly vanish against the card
- **Element:** Days › timeline card › unlabelled bars, Running bars, slot-pool bands; both themes, clearest on the sliver after T1 in days-light-m.png
- **Evidence:** Confirmed
- **Problem:** Bars have soft fills and no border, so their edge against the card is about 1.1-1.2:1: accent_soft is 1.16:1 in light and 1.17:1 in dark, seg 1.19:1 and 1.13:1, up_soft about 1.14:1. Labelled bars survive on their text. The code comment says most bars are too narrow for a label, so most of a real day is a barely visible clickable shape. Non-peak Running bars are 1.32:1 in light and 1.49:1 in dark.
- **Principle:** Graphics needed to understand the content, and control boundaries, reach 3:1 [WCAG 1.4.11]
- **Fix:** Keep the soft fill for labelled bars and add a 1px border in each state's strong colour (t.accent, t.up, t.warn, t.down, t.faint), which all reach 3:1 or close to it on the card; check t.faint in light, at 3.04:1. Paint bars under the label width solid in that strong colour. Replace accent_line on Running with t.faint or a mid accent at 3:1, keeping t.accent for the peak. Use the same strong edge on slot-pool bands.

### R4 · 3 Major · KPI tiles show a saving their own numbers contradict
- **Element:** Days › KPI tiles › Agent time and Human estimate; also the week's Hours saved line (days-dark-m.png)
- **Evidence:** Confirmed
- **Problem:** The tiles read Agent time 5h 36m, Human estimate 6h, 4h saved, yet 6h minus 5h 36m is 24m. The saving uses est_agent_min while the tile beside it shows agent_min, and both are simply called agent time. The same gap shows on the week: the card says 24h agent while the chart's bars add up to about 27h. The owner can't tell which number to trust when judging how the work went. The values are sample data, but the two fields are in the code.
- **Principle:** One fact, one place, and no copies that can disagree [SEC-7]; match the user's words [NNG-H2]
- **Fix:** Show the sum the saving really uses, for example delta text "4h saved vs 2h estimated agent time", or compute saved from the agent time on display. Apply the same name in the Hours saved breakdown: "79h human estimate − 24h estimated agent". If the two agent times stay separate, name them differently everywhere.

### R5 · 2 Minor · Bar labels are cut mid-word while most of the axis sits empty
- **Element:** Days › timeline card › lane bars at 1360 and 1728 (days-dark-m.png, days-dark-l.png)
- **Evidence:** Confirmed
- **Problem:** Labels end as "T1 Add the WebA", "T2 Sign in with a pas", "T3 Setting" and "T3 S", with no ellipsis: text_ellipsis on the flex bar doesn't reach the bare text child. Meanwhile the axis runs 9 AM to 9 PM, so about three quarters of the width is empty and the work is squeezed into the last quarter. The hover capture shows the same at 12:01 PM with the empty part on the right. The sample's late, short day exaggerates this, but the axis isn't fitted to the work.
- **Principle:** Text is not lost; primary content gets the space [WCAG 1.4.4] [SCR-1]
- **Fix:** Wrap the label in an inner div with truncate so it ends in an ellipsis. Fit the axis to the day's first and last activity plus one hour each side, with a minimum span of 6 hours and always including the now line. Keep the tooltip as the full name.

### R6 · 2 Minor · Reaching an earlier day is slow and the date control is far from the date
- **Element:** Days › header › title, ‹ ›, Today, at all sizes; over 1,000px apart at 1728 (days-dark-l.png)
- **Evidence:** Confirmed
- **Problem:** The page exists to reach any earlier day, but the only routes are one day per click or the seven tiles of the current week; three weeks back is 21 clicks, each with a reload. The stepper sits at the far right while the date it changes is at the far left. The title "Today" also never states the date, so the reader has to find the selected tile.
- **Principle:** Flexibility and efficiency of use [NNG-H7]; related controls grouped and close together [NNG-FITTS]
- **Fix:** Move ‹ › directly before the title, so a longer title doesn't move them. Make the title open a month calendar pop-up limited to oldest through today, with past weeks reachable from it. Show "Today · Fri, Nov 6" in the title. This is navigation only, so it stays within the read-only rule.

### R7 · 2 Minor · Changing the date removes the whole page, and an error strands it
- **Element:** Days › page shell during a date change and on a failed load
- **Evidence:** Confirmed
- **Problem:** set_date clears m.data.days, so render returns only "Loading…" or the error text: header, stepper, tiles and chart all disappear until the answer lands. Repeated ‹ clicks fall on nothing, the page blinks on each step, and if the load fails there is no retry and no way to return to today on the page; only Cmd-R or leaving and returning remain. Read from code, not observed.
- **Principle:** Visibility of system status; no dead ends [NNG-H1] [NNG-H3] [NNG-H9]
- **Fix:** Keep the previous view on screen until the new answer lands, updating the title and tile selection at once and dimming the content to about 60%. Always render the header. On an error, show it inline under the header with a Retry button and leave ‹ › and Today working.

### R8 · 2 Minor · Week bars can't be read per project
- **Element:** Days › week card › stacked columns and tooltip (hover-days-bar-dark-m.png)
- **Evidence:** Confirmed
- **Problem:** The chart is stacked by project, but the tooltip gives only the total: "Wednesday, November 4: 7h 18m agent time, 4 done". Project share can only be estimated by eye, and only by colour. From the fifth project on, every project is the same grey, so their segments merge and can't be told apart, which matters for an owner running many agents. The sample has two projects, so this wasn't seen on screen.
- **Principle:** Colour isn't the only signal [WCAG 1.4.1]; recognition rather than recall [NNG-H6]
- **Fix:** Add each project to the tooltip on separate lines, for example "api 2h 55m" and "webapp 4h 23m", in stack order. Past four projects, add palette slots checked for separation, or fold the smallest into one "Other" segment that the tooltip itemises.

### R9 · 2 Minor · Project toggles grow with the board and repeat as a legend
- **Element:** Days › header › project toggles; week card › legend
- **Evidence:** Confirmed
- **Problem:** There is one 30px chip per project in a wrapping row with no limit. Two fit at 900 wide (days-dark-s.png), but more will wrap and push the date controls onto a new line, moving them between boards. The week card then repeats the same names and colours as a legend directly below. The sample has two projects, so the wrap wasn't seen.
- **Principle:** Owner lesson L2: a set that grows uses a pop-up menu; one fact, one place [SEC-7]
- **Fix:** Show chips up to four projects. Past that, use one pop-up menu button labelled "Projects · 3 of 7" with checkable colour-swatched rows. Keep the week card's legend only in the pop-up case, and remove it while the chips are visible, since they already act as the key.

### R10 · 2 Minor · A hidden project's toggle fails text contrast
- **Element:** Days › header › project toggle, off state, both themes
- **Evidence:** Confirmed
- **Problem:** An off chip uses t.faint on the page background: about 2.8:1 in light (#8a95a3 on #f6f7f9) and about 4.3:1 in dark (#6f7a88 on #0f1216), at 12.5px. The chip is still a working control, which is how the project is brought back, so the disabled exemption doesn't apply. Ratios were computed by hand from theme.rs; this state isn't in the captures.
- **Principle:** Text contrast at least 4.5:1 [WCAG 1.4.3]
- **Fix:** Use t.muted for off chips: #5f6b7a in light and #a1abb8 in dark, both above 4.5:1 on the page background. The outlined swatch and missing fill still mark the off state.

### R11 · 2 Minor · The last visible project's toggle silently does nothing
- **Element:** Days › header › project toggle when only one project is shown
- **Evidence:** Confirmed
- **Problem:** toggle() returns without change when a click would hide the last project, but the chip keeps its pointer cursor and its "Hide webapp" tooltip. The click looks broken.
- **Principle:** Visibility of system status; constraints shown before the error [NNG-H1] [NNG-H5]
- **Fix:** When one project is left, change its tooltip to "At least one project stays shown", use the default cursor and skip the click handler.

### R12 · 2 Minor · At the smallest window the day's timeline falls below the fold
- **Element:** Days › KPI tiles, week card, rail at 900x560 (days-dark-s.png)
- **Evidence:** Confirmed
- **Problem:** Tile labels wrap, "5h 36m" breaks over two lines, and values no longer share a baseline. The tile row grows to about 135px, so the header, tiles and the fixed 170px chart fill the window, and the timeline isn't visible at all. By code, the rail also wraps under all four left cards below about 1,040 wide, as a 300px column beside empty space. Even at larger sizes the week chart is the loudest thing on a page about one day.
- **Principle:** The work dominates; narrowing keeps the order [SCR-1] [SCR-2] [SCR-6]
- **Fix:** Set whitespace_nowrap on tile values and shorten labels to one line, for example "Human est." and "Waiting". Under 1,040 wide, lay the tiles out 3 by 2 or drop the tile padding to 8px. Under 700 tall, reduce the chart's H to 100. Consider moving the timeline above the week card, and when the rail wraps lay its cards in two columns across the full width.

### R13 · 2 Minor · Clustered marks can't be read
- **Element:** Days › timeline card › marks row, 1360 (days-dark-m.png) and 1728
- **Evidence:** Confirmed
- **Problem:** Marks are 8px dots with 12x12 hover targets that overlap when events are close, as in the Question and Commit pair near 8 PM and the Commit run near 7 PM. Only the topmost target gets a tooltip, so the others can't be read. The tooltip is also the only route to a mark's text.
- **Principle:** Targets at least 24 x 24 CSS px or spaced apart [WCAG 2.5.8]; no hover-only path [NNG-FITTS]
- **Fix:** Merge marks within 12px into one 24px-tall hover target whose tooltip lists each mark in time order, with its 12-hour time, ref and text. Make the row's target 24px tall even for single marks.

### R14 · 2 Minor · The timeline has no heading and the date scrolls away
- **Element:** Days › timeline card, all sizes
- **Evidence:** Confirmed
- **Problem:** Every neighbouring card has a title, but this one opens with its legend. The page header scrolls with the content, so once the reader is on the timeline nothing says which day it shows. That matters because the card above it covers a week, and a past day looks identical to today apart from the now line.
- **Principle:** Headings describe their content [WCAG 2.4.6]; every group labelled the same way [SEC-6]
- **Fix:** Add an h2 carrying the fact itself, "Fri, Nov 6", which is not a filler label, with the legend at the right of that row like the other cards. Alternatively make the header sticky inside the scroller.

### R15 · 2 Minor · Usage, host and slot rows squeeze their readings into a 104px label column
- **Element:** Days › timeline card › Usage, host and slot rows
- **Evidence:** Likely
- **Problem:** Lines such as "full 2 times · 45m", "1 task waited 30m", every "T12 idle 2h" and "Hourly budget used up" sit in a 104px column with no truncation, so they probably wrap. The idle list has no limit, so the label column can grow while the band stays 22px tall. The Usage chart has dashed lines at 50% and 100% with no scale labels, so a value can't be read without hovering. None of these rows appear in the sample, so this comes from code only.
- **Principle:** Text is not lost, and recognition rather than recall [WCAG 1.4.4] [NNG-H6]
- **Fix:** Show the slot and host detail as one line under the band, spanning the full card width, and cap idle tasks at three with "+2 more" in a tooltip. Label the Usage gridlines 50% and 100% at the left of the plot, and add the project swatch to slot rows to match the lanes.

### R16 · 1 Cosmetic · The selected day tile shifts its content by 1px
- **Element:** Days › week card › selected tile (Fri 6) in all captures
- **Evidence:** Confirmed
- **Problem:** The selected tile switches to a 2px border, so its three lines sit 1px lower than the other tiles and jump when the day changes.
- **Principle:** Sections and rows share edges [SCR-4]
- **Fix:** Keep border_2 on every tile with a transparent or t.border colour, or draw the selected ring as an outer shadow.

### R17 · 1 Cosmetic · The last hour label reaches the card edge
- **Element:** Days › timeline card › hour axis, right end (days-dark-m.png, days-dark-l.png)
- **Evidence:** Confirmed
- **Problem:** The final label is centred on the plot's right edge, so it overhangs the 14px card padding and stops about 2px short of the border.
- **Principle:** Alignment [SCR-4]
- **Fix:** Right-align the last hour label to the plot edge, using right(0) in place of the centred -16px offset.

### R18 · 1 Cosmetic · Clickable chips, tiles and bars have no hover state
- **Element:** Days › project toggles, week columns and tiles, timeline bars
- **Evidence:** Confirmed
- **Problem:** Only the pointer cursor changes. kit buttons brighten on hover, but these custom controls don't, so they feel less responsive and the target under a fast pointer is less clear.
- **Principle:** Hover and pressed states are designed [NNG-H1]
- **Fix:** Add hover(bg t.panel_2) on chips and tiles, and a one-step stronger border on bars and columns.

## What works

- The header, even tile row and card rhythm make the page read as organised at 1360 and 1728, and the layout holds in both themes.
- Marks pair colour with shape (circle, square, ring), the off toggle changes fill as well as colour, and project colours stay fixed when others are hidden.
- All times shown are 12-hour with AM/PM, and bars open tasks in the right panel, which respects the one-home rule.
- Empty values stay quiet: blank tile lines, no zero-filled rows, and host and slot rows only appear on days that have them.

## Not checked

- Real input: this is a native GPUI app, so no control was driven at any size. Clicks, drags, wheel, scroll chaining and tooltip timing were read from code or the captures only. R7 and R11 are read from code.
- The ultrahand inspect step and composition audit (blurred screenshot, audit numbers) weren't run: both need a browser page. Composition was judged by eye from the screenshots.
- Contrast ratios were computed by hand from theme.rs, because a script to calculate them was denied. They're accurate to about two digits. A directory listing outside the brief's single allowed command did run; it only listed files and changed nothing.
- The timeline bar tooltip: hover-days-bar-dark-m.png is listed as a timeline bar hover but shows a week column tooltip. Bar tooltip text comes from code only, and its placement wasn't seen.
- Slot, host and usage rows, the failed, lost, stopped and away bars, a second project lane, hidden-project state, past days and the Week view don't appear in the captures. Their absence may be the sample's. The 12:01 PM clock in the hover capture also differs from the other captures' 8:38 PM, so the sample seems to follow the clock.
- Loading, error and no-agent-work states, very long project names, many projects, and busy days with many bars weren't shown. Scrolled views at any size weren't captured, nor light theme at the smallest or largest size.
- VoiceOver and accessible names and roles: whether GPUI exposes these here can't be judged from the code in scope, and status announcements weren't tested.
- 200% text, macOS window behaviours such as full screen and menu bar commands, and motion. Cards below the timeline and the rail are outside this unit, so only their effect on the top of the page was noted.
- One reviewer finds only some of the problems; a second reviewer or a usability test will find more [NNG-HE].
