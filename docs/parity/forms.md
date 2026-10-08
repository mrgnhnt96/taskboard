# Parity: forms

The app has no forms. The New task, New goal, Edit goal and Add an issue forms, their pickers and
their drafts were removed on 2026-10-08: tasks, goals and issues are made and changed through Claude
(`tb task new`, `tb goal new`, `tb goal set`, `tb backlog add`). See [PARITY.md](../PARITY.md).

What is left in `crates/taskboard-app/src/ui/modals.rs`:

| Web function | Behaviour | Native | Test |
|---|---|---|---|
| `localeCompare` | ICU root order for names (whitespace, punctuation, digits, letters; accents, then case) | `modals::locale_cmp` | `names_sort_like_the_web` (the `localeSort` cases in `parity/golden/forms.json`) |
| alerts dialog | Lists the alerts; Dismiss all; Esc or Close closes it; closes itself when none are left | `modals::render`, `alerts` | `escape_closes_the_alerts_dialog`, `the_alerts_dialog_closes_itself_when_none_are_left` |
