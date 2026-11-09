# Page brief

- Type and state: Desktop app, live (beta). Every release so far is a prerelease.
- Audience: developers on a Mac who already run Claude Code in the Midna terminal and want to queue
  work for it. They know Claude Code; many know Midna, some don't.
- The page's one job: get them to download the beta DMG and finish setup.
- Primary CTA: "Download for Mac", to the newest release's DMG (betas included), falling back to
  https://github.com/mrgnhnt96/taskboard/releases without JavaScript.
- Secondary action: "Source on GitHub" in the footer.
- Search phrases: "Claude Code task board", "Claude Code task queue Mac", "run Claude Code sessions
  in parallel", "Midna task board", "Claude Code kanban".
- Objections:
  - What does it actually do, and what does it look like? (one line on what it does, the screenshot)
  - Does it run on my Mac? (Apple Silicon, macOS 13 or later, beta)
  - Do I need Midna? (the lede names Midna and links it)
  - Is setup more than dragging an app? (four install steps, including Install hooks)
- Analytics and cookies: none. The page's script calls the GitHub releases API from the visitor's
  browser to find the newest DMG; it sets no cookies.

## Claims

| Claim on the page | Evidence |
| --- | --- |
| A native macOS task board for Claude Code sessions running in Midna | `README only` (README.md, first line) |
| Queue tasks; it starts a Claude terminal in Midna for each, follows its progress, and hands its saved context to the next terminal if one closes early | README.md "How it works" and intro; the runner in `crates/taskboardd` starts terminals with the handoff as the prompt |
| Apple Silicon | `.github/workflows/release.yml` builds `Taskboard-<version>-macos-arm64` only (checked 2026-11-09) |
| macOS 13 or later | `packaging/build-app.sh` sets `LSMinimumSystemVersion` 13.0 and `MACOSX_DEPLOYMENT_TARGET=13.0` (checked 2026-11-09) |
| Beta | `gh release list -R mrgnhnt96/taskboard`: every release is a prerelease, newest v0.1.0-beta.82 on 2026-11-04 (checked 2026-11-09) |
| Allow it in Login Items so its daemon keeps the board running | README.md Install; the app registers the daemon as a login item on first launch and shows a banner until allowed |
| Click Install hooks at the foot of the sidebar to connect Claude Code | `crates/taskboard-app/src/ui/sidebar.rs` renders `hooks_item` at the foot of the sidebar; `app.rs` `hooks_item` shows "Install hooks" |
| Screenshot: the board with example data | Captured 2026-11-09 from the real app (`taskboard-app`, feature `snapshot`) with `TASKBOARD_BACKEND=fake`, its built-in seeded sample board. The page captions it as example data |

## Waiting on a person

- Morgan: after deploy, verify the site in Google Search Console and Bing Webmaster Tools, submit
  https://taskboard.mrgnhnt.com/sitemap.xml, and run the Rich Results Test on the home page.
- Morgan: say whether Taskboard is free (and under what licence, if any). The repository has no
  LICENSE file, so the page and its structured data state no price and no "open source"; the
  audit warns that `SoftwareApplication` has no `offers` until then.
- Morgan: say whether to be named as the publisher. If so, the home page's JSON-LD gets a `Person`
  (name, url, `sameAs` GitHub profile); until then it has no `Organization` or `Person`, because
  no company is behind the app, and the audit warns about it.
- Morgan: decide whether the page needs a privacy line. The app checks GitHub releases for updates,
  and the daemon talks to the PR hosts, CI and Jira you configure; it has no analytics.
