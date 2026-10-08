'use strict';
// A fake API for trying the UI without a server: open /tasks/?mock=1.
// Extra query flags: midna=0 (Midna down), alert=1|2 (one or two alerts), jira=0 (Jira not set up), many=1 (lots of terminals).
(function () {
  const PARAMS = new URLSearchParams(location.search);
  const T0 = Date.now();
  const iso = ms => new Date(ms).toISOString().replace(/\.\d{3}Z$/, 'Z');
  const ago = min => iso(T0 - min * 60000);
  const now = () => iso(Date.now());
  const HOME = '/home/you/code';
  const TB = HOME + '/.local/bin/tb';
  const JIRA_ON = PARAMS.get('jira') !== '0';
  const JIRA_SITE = 'https://example.atlassian.net/browse/';
  const OWNER = 'You';

  const db = {
    alerts: PARAMS.get('alert') ? [{ id: 'a1', at: ago(40), task: 'T3', goal: null, text: 'T3 didn’t start: Midna refused: denied by user.' },
      ...(PARAMS.get('alert') === '2' ? [{ id: 'a2', at: ago(20), task: 'T4', goal: null, text: 'Your answer to T4 hasn’t reached API auth yet.' }] : [])] : [],
    hours: { on: true, start: '06:00', end: '15:00', days: ['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'] },
    todayUntil: null,
    midnaUp: PARAMS.get('midna') !== '0',
    projects: [
      { name: 'api', path: HOME + '/api' }, { name: 'mobile', path: HOME + '/mobile' },
      { name: 'webapp', path: HOME + '/webapp' }, { name: 'infra', path: HOME + '/infra' },
    ],
    sessions: [
      { id: 's-auth', name: 'API auth', project: 'api', project_path: HOME + '/api', status: 'working', last_activity: ago(4) },
      { id: 's-cache', name: 'Cache cleanup', project: 'api', project_path: HOME + '/api', status: 'idle', last_activity: ago(40) },
      { id: 's-nav', name: 'Tab navigation', project: 'mobile', project_path: HOME + '/mobile', status: 'needs', last_activity: ago(12) },
      { id: 's-idle', name: 'Mobile spare', project: 'mobile', project_path: HOME + '/mobile', status: 'idle', last_activity: ago(30) },
      { id: 's-web', name: 'Settings page', project: 'webapp', project_path: HOME + '/webapp', status: 'working', last_activity: ago(0) },
      { id: 's-ci', name: 'CI tweak', project: 'infra', project_path: HOME + '/infra', status: 'needs', last_activity: ago(6) },
      { id: 's-old1', name: 'Old dependency bump', project: 'webapp', project_path: HOME + '/webapp', status: 'gone', gone_at: ago(200), last_activity: ago(210), claude_session_id: 'c-old-1' },
    ],
    goals: [
      { id: 1, name: 'Sign-in that survives restarts', project: 'api', repo_path: HOME + '/api', epic_key: 'PROJ-100', epic_status: 'In Progress',
        tldr: 'Users get signed out whenever the API restarts, because sessions live in memory. Move them to the database.',
        outcome: 'restarting the API leaves every signed-in user signed in, and a test checks that on every build.',
        run_in_order: 1, max_terminals: 2, auto_close: 1, archived: 0, paused: 0, deprioritized: 0, created_at: ago(600) },
      { id: 2, name: 'Smoother tab navigation', project: 'mobile', repo_path: HOME + '/mobile', epic_key: 'PROJ-101', epic_status: 'In Progress',
        tldr: '', outcome: 'going back to a recent tab is instant on both platforms, with memory at its normal level.',
        run_in_order: 1, max_terminals: 2, auto_close: 1, archived: 0, paused: 0, deprioritized: 0, created_at: ago(500) },
      { id: 3, name: 'Faster dashboard', project: 'webapp', repo_path: HOME + '/webapp', epic_key: null, epic_status: null,
        tldr: '', outcome: 'the dashboard shows in under a second on a cold cache.', run_in_order: 1, max_terminals: 1, auto_close: 1, archived: 0, paused: 0, deprioritized: 0, created_at: ago(400) },
      { id: 4, name: 'Settings redesign', project: 'webapp', repo_path: HOME + '/webapp', epic_key: null, epic_status: null,
        tldr: '', outcome: 'the new settings page ships behind no flag, with every old setting still reachable.', run_in_order: 0, max_terminals: 2, auto_close: 1, archived: 0, paused: 0, deprioritized: 0, created_at: ago(300) },
      { id: 5, name: 'Remove the old cache layer', project: 'api', repo_path: HOME + '/api', epic_key: 'PROJ-103', epic_status: 'Done',
        tldr: '', outcome: 'nothing reads from the old cache any more.', run_in_order: 1, max_terminals: 1, auto_close: 1, archived: 0, paused: 0, deprioritized: 0, created_at: ago(900) },
    ],
    notes: [
      { id: 1, goal_id: 1, kind: 'finding', text: 'Sessions are kept in a plain map in `src/auth/sessions.rs`; nothing writes them to disk.', source: 'T10', pinned: 0, at: ago(150) },
      { id: 2, goal_id: 1, kind: 'decision', text: 'Keep the session cookie format as it is, so nobody gets signed out by the change itself.', source: 'T4', pinned: 0, at: ago(60) },
      { id: 3, goal_id: 1, kind: 'decision', text: 'Write the migration first, then switch reads over.', source: 'you', pinned: 1, at: ago(90) },
      { id: 4, goal_id: 1, kind: 'reference', text: 'Session storage design doc: https://example.com/docs/sessions', source: '', pinned: 0, at: ago(200) },
      { id: 5, goal_id: 2, kind: 'decision', text: 'The keep-alive window lives in remote config so it can change without a release.', source: 'T6', pinned: 0, at: ago(70) },
      { id: 6, goal_id: 3, kind: 'finding', text: 'The chart library loads before the first draw and holds it up.', source: 'T9', pinned: 0, at: ago(40) },
    ],
    attachments: [
      { id: 1, task_id: 1, goal_id: null, kind: 'design', title: 'Restart test plan', url: 'https://example.com/docs/restart-test-plan', added_by: 'API auth', at: ago(30) },
      { id: 2, task_id: null, goal_id: 1, kind: 'proposal', title: 'session-storage.md', url: HOME + '/api/docs/session-storage.md', added_by: OWNER, at: ago(90) },
    ],
    tasks: [],
    events: [],
    issues: [],
    ihist: [],
    seq: { task: 20, goal: 10, issue: 20, note: 20, sess: 1, event: 1, att: 10 },
  };
  if (PARAMS.get('many') === '1') {
    const extra = ['docs-site', 'design-system', 'billing', 'search', 'notifications', 'analytics', 'admin', 'payments'];
    extra.forEach(n => db.projects.push({ name: n, path: HOME + '/' + n }));
    const st = ['idle', 'working', 'idle', 'needs', 'working', 'idle'];
    extra.forEach((n, i) => {
      for (let k = 0; k < 1 + (i % 3); k++) {
        db.sessions.push({ id: `s-x${i}-${k}`, name: `${n} ${['fix', 'review', 'tests'][k]}`, project: n,
          project_path: HOME + '/' + n, status: st[(i + k) % st.length], last_activity: ago(i * 17 + k * 5) });
      }
    });
  }

  const ctx = (o = {}) => ({ where: {}, done: [], next: [], decisions: [], answers: [], files: [], turns: 0, checkpoints: 0, ...o });
  const projectPath = name => (db.projects.find(p => p.name === name) || {}).path || null;
  function task(o) {
    return {
      detail: '', priority: 'normal', failed: 0, lost: 0, needs_reason: null, question: null, latest: null, summary: null,
      goal_id: null, position: 0, pickup: 'queue', pickup_session: null, session_id: null, session_name: null, claude_session_id: null,
      auto_close: 1, jira_key: null, jira_status: null, pr: null, starting: false, waiting: null,
      from_issue_id: null, context: ctx(), meta: [], created_at: ago(120), updated_at: ago(60), started_at: null, finished_at: null,
      repo_path: projectPath(o.project), ...o,
    };
  }
  const gh = (repo, num) => `https://github.com/example/${repo}/pull/${num}`;
  db.tasks = [
    task({ id: 15, goal_id: 5, position: 1, status: 'done', title: 'Stop reading from the old cache', project: 'api',
      session_id: 's-cache', session_name: 'Cache cleanup', started_at: ago(400), finished_at: ago(180), updated_at: ago(180),
      pr: { repo: 'api', num: 88, state: 'MERGED', title: 'Stop reading from the old cache', checks: 'pass', review: 'approved', stage: { phase: 'merged', label: 'Merged' } },
      summary: 'Every read now goes through the new cache.', latest: 'Every read now goes through the new cache.', detail: 'Move the last reads off the old cache layer.' }),
    task({ id: 10, goal_id: 1, position: 1, status: 'done', title: 'Find where sessions are stored', project: 'api',
      session_id: 's-cache', session_name: 'Cache cleanup', started_at: ago(200), finished_at: ago(120), updated_at: ago(120),
      summary: 'Sessions live in an in-memory map. Details are in the goal notes.', latest: 'Sessions live in an in-memory map.',
      detail: 'Find every place that reads or writes a session, and note it in the goal.',
      context: ctx({ where: { branch: 'main', worktree: 'api', last_commit: 'Latest main, no changes', uncommitted: 0 }, done: ['Listed every session read', 'Listed every session write'], turns: 9, checkpoints: 2, saved_at: ago(121) }) }),
    task({ id: 4, goal_id: 1, position: 2, status: 'working', title: 'Store sessions in the database', project: 'api',
      session_id: 's-auth', session_name: 'API auth', claude_session_id: 'c-auth-1', started_at: ago(58), updated_at: ago(4),
      jira_key: 'PROJ-121', jira_status: 'In Progress',
      pr: { repo: 'api', num: 101, state: 'OPEN', title: 'PROJ-121: Store sessions in the database', checks: 'pending', review: 'none', stage: { phase: 'checks', label: 'Watching checks' } },
      latest: 'Migration written. Switching the reads over.',
      detail: 'Add a sessions table and a migration.\nSwitch session reads and writes to it.\nRun the restart test before opening a PR.',
      meta: [['Database', 'Postgres 16'], ['Feature', 'Sign-in']],
      context: ctx({ where: { branch: 'feature/PROJ-121-db-sessions', worktree: 'api · db-sessions', last_commit: 'Add the sessions table', uncommitted: 2, conversation: 'c-auth-1' },
        done: ['Added the sessions table and migration', 'Writes go to the database'],
        next: ['Switch reads to the database', 'Run the restart test', 'Open the PR'],
        decisions: ['Keep the cookie format as it is'],
        files: ['src/auth/sessions.rs', 'migrations/0042_sessions.sql', 'src/auth/middleware.rs', 'tests/restart.rs'],
        turns: 5, checkpoints: 1, saved_at: ago(5) }) }),
    task({ id: 1, goal_id: 1, position: 3, status: 'queued', priority: 'high', title: 'Add a restart test', project: 'api', created_at: ago(25), updated_at: ago(25),
      waiting: 'Waits for T4 to finish', latest: 'Sign in, restart the API, check the user is still signed in.',
      detail: 'Sign in a test user.\nRestart the API.\nFail if the user has to sign in again.' }),
    task({ id: 11, goal_id: 1, position: 4, status: 'planned', title: 'Expire old sessions', project: 'api',
      detail: 'Delete sessions older than 30 days once a night.' }),
    task({ id: 12, goal_id: 1, position: 5, status: 'planned', title: 'Run the restart test on every build', project: 'api',
      detail: 'Add the restart test to CI.' }),
    task({ id: 6, goal_id: 2, position: 1, status: 'needs', needs_reason: 'question', title: 'Pick the keep-alive window size', project: 'mobile',
      session_id: 's-nav', session_name: 'Tab navigation', claude_session_id: 'c-nav-1', started_at: ago(80), updated_at: ago(12), auto_close: 0,
      jira_key: 'PROJ-122', jira_status: 'In Review',
      pr: { repo: 'mobile', num: 102, state: 'OPEN', title: 'Keep recent tabs alive', checks: 'pass', review: 'changes', stage: { phase: 'comments', label: 'Answering comments' } },
      question: 'Keep the last 3 tabs alive, or 5? 5 makes going back smoother but holds more memory on older phones.',
      latest: 'Opened PR #102; the checks passed.',
      detail: 'Set the keep-alive window from the design doc and wire it into the tab navigator.\nAsk me if the window size is unclear.',
      meta: [['Platforms', 'Android, iOS']],
      context: ctx({ where: { branch: 'feature/PROJ-122-keep-alive', worktree: 'mobile', last_commit: 'Keep recent tabs alive', uncommitted: 0, conversation: 'c-nav-1' },
        done: ['Wired the keep-alive window into the tab navigator', 'Opened PR #102; the checks passed'],
        next: ['Set the window size once you answer', 'Reply to the review comments'],
        decisions: ['The window lives in remote config'],
        answers: ['Android first, then iOS'], files: ['src/nav/TabNavigator.tsx', 'src/nav/routeCache.ts'],
        turns: 7, checkpoints: 2, saved_at: ago(13) }) }),
    task({ id: 8, goal_id: 2, position: 2, status: 'done', failed: 1, title: 'Update the screenshot tests', project: 'mobile',
      session_id: null, session_name: 'Mobile spare', started_at: ago(100), finished_at: ago(60), updated_at: ago(60), jira_key: 'PROJ-123', jira_status: 'In Progress',
      summary: 'Stopped: the simulator would not boot.', latest: 'Stopped: the simulator would not boot.',
      detail: 'Record the screenshot tests again after the tab bar change.' }),
    task({ id: 16, goal_id: 2, position: 3, status: 'done', title: 'Keep scroll position per tab', project: 'mobile',
      session_id: null, session_name: 'Tab navigation', started_at: ago(300), finished_at: ago(240), updated_at: ago(30),
      pr: { repo: 'mobile', num: 99, state: 'OPEN', title: 'Keep scroll position per tab', checks: 'fail', review: 'pending',
        stage: { phase: 'fix', label: 'Fixing checks', session: 's-nav', stopped: { asked: true, message: 'The lint check fails on a file this PR doesn’t touch. Fix it here, or leave it?' } } },
      summary: 'Each tab keeps its scroll position.', latest: 'Opened PR #99.', detail: 'Remember the scroll position for each tab.' }),
    task({ id: 2, goal_id: 2, position: 4, status: 'queued', title: 'Fix the flaky tab switch test', project: 'mobile', created_at: ago(60), updated_at: ago(60),
      jira_key: 'PROJ-124', jira_status: 'To Do', latest: 'Fails about once in five runs.',
      detail: 'It fails about once in five runs. Start with the wait for the tab to load.' }),
    task({ id: 9, goal_id: 3, position: 1, status: 'needs', lost: 1, needs_reason: 'lost', title: 'Load the chart library after first draw', project: 'webapp',
      session_id: null, session_name: 'Dashboard perf', claude_session_id: 'c-dash-1', started_at: ago(90), updated_at: ago(20),
      latest: 'The terminal closed before the task was done. Its context was saved.',
      detail: 'Cut the time to first draw on the dashboard.\nMeasure before and after with a cold cache.',
      context: ctx({ where: { branch: 'perf/dashboard-first-draw', worktree: 'webapp', last_commit: 'Load the chart library lazily', uncommitted: 1, conversation: 'c-dash-1' },
        done: ['Measured first draw with a cold cache', 'Loaded the chart library lazily'],
        next: ['Measure again', 'Open the PR'], files: ['src/dashboard/Dashboard.tsx', 'src/dashboard/charts.ts'],
        turns: 6, checkpoints: 1, saved_at: ago(22) }) }),
    task({ id: 5, goal_id: 4, position: 1, status: 'working', title: 'Build the new settings page', project: 'webapp',
      session_id: 's-web', session_name: 'Settings page', claude_session_id: 'c-web-1', started_at: ago(45), updated_at: ago(0),
      latest: 'Layout done. Wiring up the account section.',
      detail: 'Build the new settings page from the design. Keep every old setting reachable.',
      context: ctx({ where: { branch: 'feature/settings', worktree: 'webapp', last_commit: 'Add the settings layout', uncommitted: 5 }, done: ['Layout'], next: ['Account section'], files: ['src/settings/Settings.tsx'], turns: 11, checkpoints: 1, saved_at: ago(1) }) }),
    task({ id: 3, goal_id: 4, position: 2, status: 'queued', title: 'Link to the new settings from the menu', project: 'webapp', created_at: ago(5), updated_at: ago(5), starting: true,
      latest: 'Starting in Midna…', detail: 'Point the Settings menu item at the new page.' }),
    task({ id: 7, status: 'done', title: 'Bump the HTTP client', project: 'api', session_id: 's-cache', session_name: 'Cache cleanup',
      started_at: ago(70), finished_at: ago(40), updated_at: ago(40), summary: 'Bumped it and the tests pass.', latest: 'Bumped it and the tests pass.',
      detail: 'Bump the HTTP client to the latest minor version.' }),
    task({ id: 13, status: 'needs', needs_reason: 'start_failed', title: 'Raise the CI cache size', project: 'infra', created_at: ago(15), updated_at: ago(14),
      latest: 'Midna refused: consent request expired before it was approved.', detail: 'Raise the CI cache size to 10 GB.' }),
    task({ id: 14, status: 'needs', needs_reason: 'attention', title: 'Split the slow CI job', project: 'infra',
      session_id: 's-ci', session_name: 'CI tweak', started_at: ago(20), updated_at: ago(6),
      latest: 'Waiting for permission to run the test suite.', detail: 'Split the integration job in two so each runs under 10 minutes.' }),
    task({ id: 17, status: 'queued', title: 'Write the README for the API', project: 'api', created_at: ago(8), updated_at: ago(8),
      waiting: 'Waits for work hours (tomorrow 6am)', detail: 'Explain how to run the API locally.' }),
  ];
  const ev = (task_id, min, who, kind, text) => db.events.push({ id: db.seq.event++, task_id, at: ago(min), who, kind, text });
  [
    [10, 205, OWNER, 'status', 'Added to the goal'], [10, 200, 'Cache cleanup', 'status', 'Picked up in Cache cleanup'],
    [10, 150, 'Cache cleanup', 'checkpoint', 'Checkpoint: listed every session read and write'], [10, 120, 'Cache cleanup', 'status', 'Marked done with a summary'],
    [4, 62, OWNER, 'status', 'Added to the queue, linked PROJ-121'], [4, 58, 'Midna', 'midna', 'Started in a new Midna terminal'],
    [4, 58, 'API auth', 'status', 'Picked up in API auth'], [4, 57, 'Task board', 'jira', 'Moved PROJ-121 to In Progress'],
    [4, 30, 'API auth', 'turn', 'Added the sessions table · edited 2 files'], [4, 25, 'API auth', 'commit', 'Add the sessions table'],
    [4, 20, 'API auth', 'found', 'Found an issue: The sign-out route doesn’t delete the session. Added to the goal’s backlog with a snapshot.'],
    [4, 18, 'API auth', 'status', 'Opened PR #101'], [4, 5, 'API auth', 'checkpoint', 'Checkpoint: writes go to the database'],
    [4, 4, 'API auth', 'note', 'Migration written. Switching the reads over.'],
    [1, 25, OWNER, 'status', 'Added to the queue'],
    [6, 85, OWNER, 'status', 'Added to the queue, linked PROJ-122'], [6, 80, 'Tab navigation', 'status', 'Picked up in Tab navigation'],
    [6, 50, 'Tab navigation', 'question', 'Android first, or both at once?'], [6, 48, OWNER, 'answer', 'Android first, then iOS'],
    [6, 30, 'Tab navigation', 'turn', 'Wired the keep-alive window into the tab navigator · edited 2 files'],
    [6, 20, 'Tab navigation', 'status', 'Opened PR #102'],
    [6, 12, 'Tab navigation', 'question', 'Keep the last 3 tabs alive, or 5? 5 makes going back smoother but holds more memory on older phones.'],
    [8, 100, 'Mobile spare', 'status', 'Picked up in Mobile spare'], [8, 60, 'Mobile spare', 'status', 'Stopped with an error: the simulator would not boot'],
    [16, 300, 'Tab navigation', 'status', 'Picked up in Tab navigation'], [16, 240, 'Tab navigation', 'status', 'Opened PR #99'],
    [9, 90, 'Dashboard perf', 'status', 'Picked up in Dashboard perf'], [9, 40, 'Dashboard perf', 'found', 'Found an issue: The chart cache never shrinks.'],
    [9, 22, 'Dashboard perf', 'checkpoint', 'Checkpoint: loaded the chart library lazily'],
    [9, 20, 'Midna', 'status', 'Terminal ended (exit) before the task was done'],
    [5, 45, 'Settings page', 'status', 'Picked up in Settings page'], [5, 1, 'Settings page', 'turn', 'Added the layout · edited 2 files'],
    [7, 70, 'Cache cleanup', 'status', 'Picked up in Cache cleanup'], [7, 40, 'Cache cleanup', 'status', 'Marked done with a summary'],
    [13, 15, OWNER, 'status', 'Added to the queue'], [13, 14, 'Midna', 'midna', 'Midna refused: consent request expired before it was approved'],
    [14, 20, 'CI tweak', 'status', 'Picked up in CI tweak'], [14, 6, 'CI tweak', 'question', 'Waiting for permission to run the test suite.'],
  ].forEach(a => ev(...a));

  function issue(o) { return { detail: '', said: '', how: '', source: 'terminal', found_by_task: null, found_by_session: null, found_by_name: null, state: 'open', task_id: null, jira_key: null, snapshot: {}, created_at: ago(60), updated_at: ago(60), ...o }; }
  db.issues = [
    issue({ id: 1, goal_id: 1, project: 'api', kind: 'bug', title: 'The sign-out route doesn’t delete the session', found_by_task: 4, found_by_name: 'API auth', created_at: ago(20),
      how: 'The API auth terminal reported it while working on T4.', said: '“Signing out clears the cookie but leaves the session row. It isn’t on this task’s list, so I’ve left it.”',
      snapshot: { task: 'T4 · Store sessions in the database', step: 'Switching reads to the database', branch: 'feature/PROJ-121-db-sessions', last_commit: 'Add the sessions table', uncommitted: 2, last_turn: 'Signed in and out 5 times, counting session rows', output: 'Session rows: 4 before, 5 after signing out' } }),
    issue({ id: 2, goal_id: 1, project: 'api', kind: 'gap', title: 'No test covers two sign-ins at once', found_by_task: 10, found_by_name: 'Cache cleanup', created_at: ago(110),
      how: 'The Cache cleanup terminal reported it while working on T10.', said: '“Two sign-ins for the same user race on the session write. Nothing tests that.”',
      snapshot: { task: 'T10 · Find where sessions are stored', step: 'Listing session writes', branch: 'main', uncommitted: 0 } }),
    issue({ id: 3, goal_id: 1, project: 'api', kind: 'clean', title: 'Two session helpers are no longer used', source: 'you', created_at: ago(35),
      how: 'You added it on the Backlog page.', said: '“These two helpers aren’t called anywhere after the change.”' }),
    issue({ id: 4, goal_id: 1, project: 'api', kind: 'follow', state: 'ticket', jira_key: 'PROJ-105', title: 'Sessions should record the device they came from', found_by_task: 10, found_by_name: 'Cache cleanup', created_at: ago(116),
      how: 'The Cache cleanup terminal reported it while working on T10.', said: '“Knowing the device would let users sign out other devices.”' }),
    issue({ id: 5, goal_id: 2, project: 'mobile', kind: 'bug', title: 'Going back twice quickly shows a blank screen', found_by_task: 6, found_by_name: 'Tab navigation', created_at: ago(30),
      how: 'The Tab navigation terminal reported it while testing for T6.', said: '“It happens on main too, so it isn’t from this change.”',
      snapshot: { task: 'T6 · Pick the keep-alive window size', step: 'Testing going back on iOS', branch: 'feature/PROJ-122-keep-alive', uncommitted: 0 } }),
    issue({ id: 6, goal_id: 3, project: 'webapp', kind: 'follow', title: 'The chart cache never shrinks', found_by_task: 9, found_by_name: 'Dashboard perf', created_at: ago(40),
      how: 'The Dashboard perf terminal reported it while working on T9.', said: '“The chart cache only grows for the whole session.”' }),
    issue({ id: 7, goal_id: null, project: 'infra', kind: 'bug', title: 'The CI cache key ignores the lockfile', found_by_task: 14, found_by_name: 'CI tweak', created_at: ago(45),
      how: 'The CI tweak terminal reported it while working on T14.', said: '“A changed lockfile still restores the old cache.”', snapshot: { task: 'T14 · Split the slow CI job', branch: 'main' } }),
    issue({ id: 8, goal_id: null, project: 'mobile', kind: 'follow', title: 'The simulator won’t boot after the toolchain update', source: 'you', created_at: ago(150),
      how: 'You added it on the Backlog page.', said: '“The screenshot task stopped because the simulator wouldn’t boot.”' }),
    issue({ id: 9, goal_id: 1, project: 'api', kind: 'clean', state: 'drop', title: 'Rename the session test file', source: 'you', created_at: ago(300), how: 'You added it.' }),
  ];
  const ih = (issue_id, min, who, kind, text) => db.ihist.push({ issue_id, at: ago(min), who, kind, text });
  [
    [1, 20, 'API auth', 'report', 'Reported while working on T4'], [1, 6, OWNER, 'note', 'Added a note: “Check this again after the reads move.”'],
    [2, 110, 'Cache cleanup', 'report', 'Reported while working on T10'], [2, 18, 'API auth', 'seen', 'Seen again while testing. Merged here with its own snapshot.'],
    [3, 35, OWNER, 'note', 'Added from the Backlog page'],
    [4, 116, 'Cache cleanup', 'report', 'Reported while working on T10'], [4, 88, OWNER, 'ticket', 'Asked Jira for a ticket'], [4, 87, 'Task board', 'ticket', 'Created PROJ-105 under the epic PROJ-100'],
    [5, 30, 'Tab navigation', 'report', 'Reported while testing'],
    [6, 40, 'Dashboard perf', 'report', 'Reported while working on T9'], [6, 20, 'Midna', 'lost', 'The terminal that reported it closed. The issue and its snapshot are kept.'],
    [7, 45, 'CI tweak', 'report', 'Reported while working on T14'],
    [8, 150, OWNER, 'note', 'Added from the Backlog page'],
    [9, 300, OWNER, 'note', 'Added from the Backlog page'], [9, 200, OWNER, 'drop', 'Closed as won’t do'],
  ].forEach(a => ih(...a));

  const idOf = r => r == null || r === '' || r === 'null' ? null : Number(String(r).replace(/^[a-z]/i, ''));
  const goalOf = t => db.goals.find(g => g.id === t.goal_id) || null;
  const sessOf = id => db.sessions.find(s => s.id === id) || null;
  const findTask = r => db.tasks.find(t => t.id === idOf(r));
  const findIssue = r => db.issues.find(b => b.id === idOf(r));
  const findGoal = r => db.goals.find(g => g.id === idOf(r));
  const log = (t, who, kind, text) => { db.events.push({ id: db.seq.event++, task_id: t.id, at: now(), who, kind, text }); t.updated_at = now(); };
  const ilog = (b, who, kind, text) => { db.ihist.push({ issue_id: b.id, at: now(), who, kind, text }); b.updated_at = now(); };
  const goalTasks = g => db.tasks.filter(t => t.goal_id === g.id).sort((a, b) => a.position - b.position);
  const openIssues = gid => db.issues.filter(b => b.goal_id === gid && b.state === 'open').length;
  const byNew = (a, b) => String(b.created_at).localeCompare(String(a.created_at));
  const jiraUrl = key => key ? JIRA_SITE + key : null;
  const prOut = p => p ? { ...p, url: gh(p.repo, p.num) } : null;
  const attOut = a => ({ id: a.id, kind: a.kind, title: a.title, url: a.url, added_by: a.added_by, at: a.at,
    task: a.task_id ? 'T' + a.task_id : null, goal: a.goal_id ? 'G' + a.goal_id : null });

  function card(t) {
    const g = goalOf(t);
    const when = t.status === 'queued' || t.status === 'planned' ? t.created_at : t.status === 'done' ? (t.finished_at || t.updated_at) : t.updated_at;
    const blocked = t.status === 'queued' && !!t.waiting && /^Waits for T/.test(t.waiting);
    return {
      id: t.id, ref: 'T' + t.id, title: t.title, project: t.project, status: t.status, priority: t.priority, failed: !!t.failed, lost: !!t.lost,
      needs_reason: t.needs_reason, question: t.status === 'needs' ? t.question : null, latest: t.latest, summary: t.summary,
      when, started_at: t.started_at, finished_at: t.finished_at, who: t.session_name, session_id: t.session_id,
      goal: g ? { id: g.id, ref: 'G' + g.id, name: g.name } : null,
      jira: t.jira_key || t.jira_status ? { key: t.jira_key, status: t.jira_status, url: jiraUrl(t.jira_key) } : null,
      pr: prOut(t.pr), position: t.position, starting: !!t.starting && t.status === 'queued',
      waiting: t.status === 'queued' ? t.waiting : null, blocked,
    };
  }
  function issueCard(b) {
    const g = db.goals.find(x => x.id === b.goal_id);
    return { id: b.id, ref: 'B' + b.id, kind: b.kind, title: b.title, goal: g ? { id: g.id, ref: 'G' + g.id, name: g.name } : null, goal_id: b.goal_id, project: b.project,
      found_by_name: b.found_by_name, source: b.source, created_at: b.created_at, updated_at: b.updated_at, state: b.state,
      jira_key: b.jira_key, task_id: b.task_id, task_ref: b.task_id ? 'T' + b.task_id : null };
  }
  function issueFull(b) {
    const ft = b.found_by_task && findTask(b.found_by_task);
    return { ...issueCard(b), detail: b.detail, said: b.said, how: b.how, snapshot: b.snapshot,
      found_by_task: ft ? { id: ft.id, ref: 'T' + ft.id, title: ft.title } : null,
      history: db.ihist.filter(h => h.issue_id === b.id).map(({ at, who, kind, text }) => ({ at, who, kind, text })) };
  }
  function handoff(t) {
    const g = goalOf(t);
    const c = t.context || {};
    const w = c.where || {};
    const out = [`[task-board:T${t.id}] You are picking up “${t.title}” in ${t.project}${t.jira_key ? ` (${t.jira_key})` : ''}`];
    if (g) {
      const notes = db.notes.filter(n => n.goal_id === g.id).sort((a, b) => b.pinned - a.pinned);
      out.push(`It is part of the goal “${g.name}”: done when ${g.outcome}` + (notes.length ? '\nGoal notes:\n' + notes.map(n => `- ${n.kind === 'decision' ? 'Decision' : n.kind === 'reference' ? 'Reference' : 'Finding'}: ${n.text}`).join('\n') : ''));
    }
    if (w.branch) out.push(`Branch: ${w.branch}` + (w.last_commit ? `\nLast commit: ${w.last_commit}` : ''));
    if (c.done && c.done.length) out.push(`Done: ${c.done.join('; ')}.`);
    if (c.next && c.next.length) out.push(`Next: ${c.next.join('; ')}.`);
    if (c.decisions && c.decisions.length) out.push(`Keep: ${c.decisions.join('; ')}.`);
    if (c.answers && c.answers.length) out.push(`The owner’s answers: ${c.answers.join('; ')}.`);
    if (t.detail) out.push(`What to do:\n${t.detail}`);
    out.push(`How to report:\n${TB} note "<progress>"\n${TB} checkpoint --done "…" --next "…" --decision "…"\n${TB} found "<issue outside this task>"\n${TB} question "<question for the owner>"\n${TB} done "<summary>"\n${TB} fail "<reason>"`);
    out.push('Check the branch and uncommitted files before changing anything, then report a checkpoint.');
    return out.join('\n\n');
  }
  function terminalsOf(t) {
    const out = [];
    const s = sessOf(t.session_id);
    if (s) out.push({ id: s.id, name: s.name, status: s.status, why: 'Worked on the task', at: t.started_at });
    if (t.pr && t.pr.stage && t.pr.stage.session && t.pr.stage.session !== t.session_id) {
      const v = sessOf(t.pr.stage.session);
      if (v) out.push({ id: v.id, name: v.name, status: v.status, why: 'Worked on the PR', at: ago(10) });
    }
    if (!out.length && t.session_name && t.started_at) out.push({ id: null, name: t.session_name, status: 'gone', why: 'Worked on the task', at: t.started_at });
    return out;
  }
  function taskFull(t) {
    const g = goalOf(t);
    const s = sessOf(t.session_id);
    let goal = null;
    if (g) {
      const list = goalTasks(g);
      const i = list.findIndex(x => x.id === t.id);
      const next = list[i + 1];
      goal = { id: g.id, ref: 'G' + g.id, name: g.name, position: i + 1, total: list.length, next_title: next ? next.title : null, open_issues: openIssues(g.id) };
    }
    const blocker = t.waiting && /^Waits for (T\d+)/.exec(t.waiting);
    const bt = blocker && findTask(blocker[1]);
    return {
      ...card(t), goal, detail: t.detail, repo_path: t.repo_path, pickup: t.pickup, pickup_session: t.pickup_session,
      session: s ? { id: s.id, name: s.name, status: s.status } : null,
      terminals: terminalsOf(t), claude_session_id: t.claude_session_id, auto_close: !!t.auto_close,
      attachments: db.attachments.filter(a => a.task_id === t.id).map(attOut),
      goal_attachments: g ? db.attachments.filter(a => a.goal_id === g.id).map(attOut) : [],
      blocked_by: bt ? [{ ...card(bt) }] : [],
      meta: t.meta, context: t.context, created_at: t.created_at, updated_at: t.updated_at,
      log: db.events.filter(e => e.task_id === t.id).sort((a, b) => b.at.localeCompare(a.at) || b.id - a.id).map(({ at, who, kind, text }) => ({ at, who, kind, text })),
      handoff: handoff(t),
      found: db.issues.filter(b => b.found_by_task === t.id).map(b => ({ id: b.id, ref: 'B' + b.id, title: b.title })),
    };
  }
  function goalSummary(g) {
    const list = goalTasks(g);
    const prsOpen = list.filter(t => t.status === 'done' && !t.failed && t.pr && t.pr.state === 'OPEN').map(t => t.pr.num);
    const done = list.filter(t => t.status === 'done').length;
    const finished = list.length && done === list.length && !prsOpen.length;
    const peekKey = t => t.status === 'done' ? (t.failed ? 'failed' : null) : t.status === 'planned' ? null
      : t.status === 'queued' ? (t.starting ? 'starting' : card(t).blocked ? 'blocked' : 'queued') : t.status;
    return {
      id: g.id, ref: 'G' + g.id, name: g.name, project: g.project, repo_path: g.repo_path, tldr: g.tldr || '', outcome: g.outcome,
      epic_key: g.epic_key, epic_status: g.epic_status, epic_url: jiraUrl(g.epic_key),
      run_in_order: !!g.run_in_order, max_terminals: g.max_terminals, auto_close: !!g.auto_close, archived: !!g.archived,
      paused: !!g.paused, deprioritized: !!g.deprioritized, hours_until: g.hours_until || null, created_at: g.created_at, updated_at: g.created_at,
      total: list.length, done, active: list.filter(t => t.status === 'working' || t.status === 'needs').length, needs: list.filter(t => t.status === 'needs').length,
      queued: list.filter(t => t.status === 'queued').length, starting: list.filter(t => t.status === 'queued' && t.starting).length,
      blocked: list.filter(t => card(t).blocked).length, planned: list.filter(t => t.status === 'planned').length,
      failed: list.filter(t => t.status === 'done' && t.failed).length, prs_open: prsOpen,
      finished_at: finished ? list.map(t => t.finished_at).sort().pop() : null, open_issues: openIssues(g.id),
      closed_count: db.issues.filter(b => b.goal_id === g.id && b.state === 'drop').length,
      peek: list.map(t => ({ ref: 'T' + t.id, title: t.title, status: peekKey(t), why: card(t).blocked ? t.waiting : null })).filter(x => x.status),
    };
  }
  function goalFull(g) {
    return { ...goalSummary(g), tasks: goalTasks(g).map((t, i) => ({ ...card(t), n: i + 1 })),
      notes: db.notes.filter(n => n.goal_id === g.id).map(({ id, kind, text, source, pinned, at }) => ({ id, kind, text, source, pinned: !!pinned, at })),
      backlog: db.issues.filter(b => b.goal_id === g.id && b.state !== 'drop').sort(byNew).map(issueFull),
      attachments: db.attachments.filter(a => a.goal_id === g.id).map(attOut) };
  }
  function sessionOut(s) {
    const t = db.tasks.find(x => x.session_id === s.id && x.status !== 'done');
    return { id: s.id, name: s.name, project: s.project, project_path: s.project_path, status: s.status, task_ref: t ? 'T' + t.id : null, task_title: t ? t.title : null,
      task_id: t ? t.id : null, last_activity: s.last_activity, seen_at: s.last_activity, renaming: s.renaming || null, rename_error: null,
      can_take: s.status === 'idle' && !t, closing: !!s.closing, branch: s.branch || (t ? 'feature/' + (t.jira_key || 'T' + t.id).toLowerCase() : 'main'), dirty: s.dirty ?? (t ? 2 : 0),
      close: s.status === 'gone' ? null : s.status === 'idle' ? 'close' : 'force' };
  }
  function sessionDetail(s) {
    const o = sessionOut(s);
    const t = db.tasks.find(x => x.session_id === s.id && x.status !== 'done');
    const last = !t && db.tasks.find(x => x.session_name === s.name);
    const evs = t ? db.events.filter(e => e.task_id === t.id).slice(-12).reverse() : [];
    const kindOf = k => ({ commit: 'commit', checkpoint: 'checkpoint', found: 'found', question: 'ask', status: 'take', midna: 'start' }[k] || 'reply');
    const timeline = evs.map(e => ({ at: e.at, kind: kindOf(e.kind), text: e.text }));
    if (!timeline.length) timeline.push({ at: s.last_activity, kind: 'reply', text: 'Finished the last turn.' }, { at: ago(90), kind: 'prompt', text: 'Look over the open PRs and tidy anything small.' });
    return { ...o, agent: 'claude', status: s.status, status_at: s.last_activity, gone_at: s.gone_at || null, claude_session_id: s.claude_session_id || null,
      task: t ? card(t) : null, last_task: last ? { ref: 'T' + last.id, title: last.title, status: last.status } : null,
      prompt: { text: t ? t.detail || t.title : 'Look over the open PRs and tidy anything small.', at: ago(t ? 8 : 90) },
      reply: { text: t ? (t.latest || 'Working on it.') : 'Nothing left to tidy. **Stopping here.**', at: s.last_activity },
      waiting: s.status === 'needs' ? { text: 'Waiting for permission: Claude needs your permission to use Bash', at: ago(6) } : null,
      diff: s.status === 'gone' ? null : t ? { files: 4, added: 126, removed: 31, new: 1 } : { files: 0, added: 0, removed: 0, new: 0 },
      stats: { turns: 3 + (s.id.length % 7), commits: t ? 1 : 0, files: t ? 4 : 0 }, timeline };
  }
  function state(q) {
    const project = q.get('project') || 'all';
    const goal = q.get('goal') || 'all';
    const done = q.get('done') || '24h';
    const keep = (q.get('keep') || '').split(',').filter(Boolean).map(idOf);
    const inP = p => project === 'all' || p === project;
    const inG = gid => goal === 'all' || gid === idOf(goal);
    const tasks = db.tasks.filter(t => t.status !== 'planned' && inP(t.project) && inG(t.goal_id));
    const col = s => tasks.filter(t => t.status === s).sort((a, b) => String(card(b).when).localeCompare(String(card(a).when))).map(card);
    const windowMin = { '24h': 1440, '7d': 7 * 1440, all: Infinity }[done] || 1440;
    const allDone = col('done');
    const shownDone = allDone.filter(c => (Date.now() - Date.parse(c.when)) / 60000 <= windowMin);
    const issues = db.issues.filter(b => inP(b.project) && inG(b.goal_id));
    const open = issues.filter(b => b.state === 'open').sort(byNew);
    const shown = new Set(open.slice(0, 4).map(b => b.id));
    keep.forEach(id => { if (issues.some(b => b.id === id)) shown.add(id); });
    const live = db.sessions.filter(s => s.status !== 'gone');
    return {
      now: now(),
      alerts: db.alerts,
      jira: { enabled: JIRA_ON },
      work_hours: hoursOut(),
      usage: { seen_at: now(), windows: [
        { key: 'five_hour', label: '5h', pct: 82, resets_at: iso(Date.now() + 95 * 60e3) },
        { key: 'seven_day', label: '7d', pct: 37, resets_at: iso(Date.now() + 4 * 86400e3) }] },
      midna: db.midnaUp ? { up: true, seen_at: now() } : { up: false, seen_at: ago(45) },
      projects: db.projects,
      sessions: live.filter(s => inP(s.project)).map(sessionOut),
      session_projects: [...new Set(live.map(s => s.project))].sort(),
      goals: db.goals.filter(g => !g.archived && inP(g.project)).map(goalSummary),
      columns: {
        backlog: issues.filter(b => shown.has(b.id)).sort(byNew).map(issueCard), backlog_open: open.length,
        queued: col('queued'), working: col('working'), needs: col('needs'), done: shownDone,
      },
      counts: { queued: col('queued').length, working: col('working').length, needs: col('needs').length, open_issues: open.length, done_hidden: allDone.length - shownDone.length },
    };
  }
  function backlog(q) {
    const project = q.get('project') || 'all', goal = q.get('goal') || 'all', kind = q.get('kind') || 'all', st = q.get('state') || 'open', sort = q.get('sort') || 'new';
    const ORDER = { bug: 0, gap: 1, follow: 2, clean: 3 };
    const list = db.issues.filter(b => (project === 'all' || b.project === project)
      && (goal === 'all' || (goal === 'none' ? b.goal_id == null : b.goal_id === idOf(goal)))
      && (kind === 'all' || b.kind === kind) && (st === 'all' || b.state === st));
    list.sort((a, b) => sort === 'old' ? -byNew(a, b) : sort === 'kind' ? (ORDER[a.kind] - ORDER[b.kind]) || byNew(a, b) : byNew(a, b));
    return { issues: list.map(b => ({ ...issueCard(b), said: b.said, how: b.how, detail: b.detail })), total: list.length };
  }

  function simulateStart(t, how) {
    if (!db.midnaUp) { log(t, 'Task board', 'midna', 'Waiting for Midna'); return; }
    t.starting = true;
    setTimeout(() => {
      t.starting = false;
      if (t.status !== 'queued' && !(t.status === 'needs' && (t.lost || t.needs_reason === 'start_failed'))) return;
      let s = how.session_id && sessOf(how.session_id);
      if (!s) {
        s = { id: 's-new' + db.seq.sess++, name: t.title.slice(0, 24), project: t.project, project_path: t.repo_path, status: 'working', last_activity: now() };
        db.sessions.push(s);
      }
      s.status = 'working';
      Object.assign(t, { status: 'working', session_id: s.id, session_name: s.name, lost: 0, needs_reason: null, started_at: now(), latest: 'Reading the handoff.', waiting: null });
      log(t, 'Midna', 'midna', how.session_id ? `Handed to ${s.name} through Midna` : 'Started in a new Midna terminal');
      log(t, s.name, 'status', `Picked up in ${s.name}`);
    }, 2500);
  }
  function closeSession(id) {
    const s = sessOf(id);
    if (!s) return;
    s.status = 'gone';
    s.gone_at = now();
    db.tasks.forEach(t => { if (t.session_id === id) t.session_id = null; });
  }

  class HttpError extends Error { constructor(status, msg) { super(msg); this.status = status; } }
  const need = (x, what) => { if (!x) throw new HttpError(404, `There’s no ${what} with that ref.`); return x; };

  const GET = [
    [/^\/state$/, (m, q) => state(q)],
    [/^\/tasks\/([^/]+)$/, m => taskFull(need(findTask(m[1]), 'task'))],
    [/^\/tasks\/([^/]+)\/handoff$/, m => ({ text: handoff(need(findTask(m[1]), 'task')) })],
    [/^\/goals$/, () => ({ goals: db.goals.filter(g => !g.archived).map(goalSummary) })],
    [/^\/goals\/([^/]+)$/, m => goalFull(need(findGoal(m[1]), 'goal'))],
    [/^\/backlog$/, (m, q) => backlog(q)],
    [/^\/sessions$/, () => ({ sessions: db.sessions.filter(s => s.status !== 'gone').map(sessionOut) })],
    [/^\/sessions\/closed$/, () => {
      const c = db.sessions.filter(s => s.status === 'gone').map(s => ({ id: s.id, name: s.name, project: s.project, project_path: s.project_path,
        closed_at: s.gone_at || ago(120), last_activity: s.last_activity, claude_session_id: s.claude_session_id || null,
        task_ref: null, task_title: null, task_status: null, can_reopen: !!s.claude_session_id }));
      return { sessions: c, total: c.length };
    }],
    [/^\/sessions\/([^/]+)$/, m => sessionDetail(need(sessOf(decodeURIComponent(m[1])), 'terminal'))],
    [/^\/backlog\/([^/]+)$/, m => issueFull(need(findIssue(m[1]), 'issue'))],
  ];
  const POST = [
    [/^\/tasks$/, (m, b) => {
      if (!b.title) throw new HttpError(400, 'The title can’t be empty.');
      if (!b.project) throw new HttpError(400, 'Pick a project.');
      const g = b.goal_id != null ? need(findGoal(b.goal_id), 'goal') : null;
      const mode = (b.pickup && b.pickup.mode) || 'queue';
      if (mode === 'attach' && !(b.pickup && b.pickup.session_id)) throw new HttpError(400, 'Pick a Midna terminal to attach.');
      const t = task({ id: db.seq.task++, title: b.title, detail: b.detail || '', project: b.project, priority: b.priority || 'normal',
        status: b.status === 'planned' ? 'planned' : 'queued', goal_id: g ? g.id : null, position: g ? goalTasks(g).length + 1 : 0,
        pickup: mode, pickup_session: mode === 'attach' ? b.pickup.session_id : null,
        auto_close: b.auto_close === false ? 0 : 1, created_at: now(), updated_at: now(), latest: null });
      db.tasks.push(t);
      log(t, OWNER, 'status', t.status === 'planned' ? 'Added as planned' : 'Added to the queue');
      if (b.jira && b.jira.mode === 'link') { t.jira_key = b.jira.key; t.jira_status = 'To Do'; log(t, OWNER, 'jira', `Linked ${b.jira.key}`); }
      if (b.jira && b.jira.mode === 'create') { t.jira_status = 'Ticket asked for'; log(t, OWNER, 'jira', 'Asked Jira for a ticket'); setTimeout(() => { t.jira_key = 'PROJ-' + (200 + t.id); t.jira_status = 'To Do'; log(t, 'Task board', 'jira', `Created ${t.jira_key}`); }, 3000); }
      if (t.status === 'queued' && (mode === 'new' || mode === 'attach')) { t.latest = 'Starting in Midna…'; simulateStart(t, { session_id: mode === 'attach' ? t.pickup_session : null }); }
      return taskFull(t);
    }],
    [/^\/tasks\/([^/]+)$/, (m, b) => {
      const t = need(findTask(m[1]), 'task');
      if ('status' in b && b.status !== t.status) {
        if (!['planned', 'queued'].includes(b.status) || !['planned', 'queued'].includes(t.status)) throw new HttpError(409, 'Only planned and queued can be switched here.');
        t.status = b.status; log(t, OWNER, 'status', b.status === 'queued' ? 'Queued' : 'Moved back to planned');
      }
      if ('goal_id' in b) { const g = b.goal_id == null ? null : need(findGoal(b.goal_id), 'goal'); t.goal_id = g ? g.id : null; t.position = g ? goalTasks(g).length + 1 : 0; log(t, OWNER, 'status', g ? `Moved to the goal “${g.name}”` : 'Taken out of its goal'); }
      ['title', 'detail', 'priority', 'meta', 'position'].forEach(k => { if (k in b) t[k] = b[k]; });
      if ('auto_close' in b) t.auto_close = b.auto_close ? 1 : 0;
      t.updated_at = now();
      return taskFull(t);
    }],
    [/^\/tasks\/([^/]+)\/start$/, (m, b) => {
      const t = need(findTask(m[1]), 'task');
      if (t.status === 'done') throw new HttpError(409, 'This task is done. Queue it again first.');
      if (b.mode === 'attach' && !b.session_id) throw new HttpError(400, 'Pick a Midna terminal to attach.');
      if (t.status === 'planned') t.status = 'queued';
      if (t.needs_reason === 'start_failed') { t.status = 'queued'; t.needs_reason = null; }
      t.pickup = b.mode; t.latest = 'Starting in Midna…';
      log(t, OWNER, 'status', b.mode === 'new' ? 'Asked for a new Midna terminal' : b.mode === 'queue' ? 'Queued in Midna' : `Asked ${(sessOf(b.session_id) || {}).name || 'a terminal'} to take it`);
      simulateStart(t, b);
      return taskFull(t);
    }],
    [/^\/tasks\/([^/]+)\/answer$/, (m, b) => {
      const t = need(findTask(m[1]), 'task');
      if (!b.text) throw new HttpError(400, 'The answer can’t be empty.');
      log(t, OWNER, 'answer', b.when === 'morning' ? `Answered: ${b.text} (sends when work hours open)` : `Answered: ${b.text}`);
      t.context.answers = (t.context.answers || []).concat(b.text);
      if (t.pr && t.pr.stage && t.pr.stage.stopped) t.pr.stage.stopped = null;
      if (db.midnaUp && b.when !== 'morning') setTimeout(() => { if (t.status === 'needs') { t.status = 'working'; t.question = null; t.needs_reason = null; t.latest = 'Got your answer. Carrying on.'; const s = sessOf(t.session_id); if (s) s.status = 'working'; log(t, t.session_name || 'Terminal', 'status', 'Took the answer and carried on'); } }, 2500);
      return taskFull(t);
    }],
    [/^\/tasks\/([^/]+)\/detach$/, m => {
      const t = need(findTask(m[1]), 'task');
      if (t.status === 'done') throw new HttpError(409, 'This task is done.');
      const s = sessOf(t.session_id);
      if (s && s.status !== 'gone') s.status = 'idle';
      Object.assign(t, { session_id: null, status: 'queued', lost: 0, needs_reason: null, question: null, latest: 'Detached; back in the queue.' });
      log(t, OWNER, 'status', s ? `Detached from ${s.name}; back in the queue` : 'Back in the queue');
      return taskFull(t);
    }],
    [/^\/tasks\/([^/]+)\/close-terminal$/, (m, b) => {
      const t = need(findTask(m[1]), 'task');
      const s = sessOf(t.session_id);
      if (!s || s.status === 'gone') throw new HttpError(409, 'This task has no open Midna terminal.');
      log(t, 'Midna', 'midna', `${b.force ? 'Force closed' : 'Closed'} ${s.name}`);
      if (db.midnaUp) setTimeout(() => closeSession(s.id), 1500);
      return taskFull(t);
    }],
    [/^\/sessions\/close$/, (m, b) => {
      const closing = [], skipped = [];
      (b.ids || []).forEach(id => { const s = sessOf(id); if (s && s.status === 'idle') { closing.push(id); s.closing = true; setTimeout(() => { closeSession(id); s.closing = false; }, 2000); } else skipped.push(id); });
      return { ok: true, closing, skipped };
    }],
    [/^\/sessions\/([^/]+)\/close$/, (m, b) => {
      const s = need(sessOf(decodeURIComponent(m[1])), 'terminal');
      if (s.status !== 'idle' && !b.force) throw new HttpError(409, 'It’s busy. Use Force close to stop what it’s doing.');
      s.closing = true;
      if (db.midnaUp) setTimeout(() => { closeSession(s.id); s.closing = false; }, 2500);
      return { ok: true, session: s.id };
    }],
    [/^\/sessions\/([^/]+)\/focus$/, () => ({ ok: true })],
    [/^\/sessions\/([^/]+)\/reopen$/, m => { const s = need(sessOf(decodeURIComponent(m[1])), 'terminal'); setTimeout(() => { s.status = 'idle'; s.gone_at = null; }, 1500); return { ok: true, session: s.id }; }],
    [/^\/sessions\/([^/]+)\/rename$/, (m, b) => {
      const sess = need(sessOf(decodeURIComponent(m[1])), 'terminal');
      const name = String(b.name || '').trim();
      if (!name) throw new HttpError(400, 'The name can’t be empty.');
      sess.renaming = name;
      if (db.midnaUp) setTimeout(() => { sess.name = name; delete sess.renaming; }, 1500);
      return { ok: true, session: sess.id, name };
    }],
    [/^\/tasks\/([^/]+)\/focus$/, m => { const t = need(findTask(m[1]), 'task'); log(t, 'Midna', 'midna', 'Focused the terminal'); return { ok: true }; }],
    [/^\/tasks\/([^/]+)\/resume$/, (m, b) => {
      const t = need(findTask(m[1]), 'task');
      if (b.mode === 'reopen' && !t.claude_session_id) throw new HttpError(400, 'No conversation was saved for this task.');
      t.lost = 0; t.status = 'queued'; t.needs_reason = null; t.latest = 'Starting in Midna…';
      log(t, OWNER, 'handoff', b.mode === 'reopen' ? 'Reopening the old conversation in a new terminal' : 'Resumed in a new terminal with the handoff');
      simulateStart(t, {});
      return taskFull(t);
    }],
    [/^\/tasks\/([^/]+)\/requeue$/, m => {
      const t = need(findTask(m[1]), 'task');
      if (t.status === 'queued' || t.status === 'planned') throw new HttpError(409, 'It’s already waiting to start.');
      Object.assign(t, { status: 'queued', failed: 0, lost: 0, summary: null, finished_at: null, latest: null, session_id: null });
      log(t, OWNER, 'status', 'Queued again');
      return taskFull(t);
    }],
    [/^\/tasks\/([^/]+)\/done$/, (m, b) => {
      const t = need(findTask(m[1]), 'task');
      if (t.status === 'done') throw new HttpError(409, 'It’s already done.');
      Object.assign(t, { status: 'done', failed: 0, lost: 0, question: null, needs_reason: null, summary: b.summary || 'Marked done by you', latest: b.summary || 'Marked done by you', finished_at: now() });
      log(t, OWNER, 'status', 'Marked done');
      return taskFull(t);
    }],
    [/^\/tasks\/([^/]+)\/fail$/, (m, b) => {
      const t = need(findTask(m[1]), 'task');
      if (t.status === 'done') throw new HttpError(409, 'It’s already done.');
      Object.assign(t, { status: 'done', failed: 1, lost: 0, question: null, needs_reason: null, summary: b.reason || 'Stopped by you', latest: b.reason || 'Stopped by you', finished_at: now() });
      log(t, OWNER, 'status', 'Marked failed');
      return taskFull(t);
    }],
    [/^\/tasks\/([^/]+)\/meta$/, (m, b) => { const t = need(findTask(m[1]), 'task'); t.meta = b.meta || []; t.updated_at = now(); return taskFull(t); }],
    [/^\/done\/close-terminals$/, () => {
      let n = 0;
      db.tasks.filter(t => t.status === 'done' && t.session_id && (sessOf(t.session_id) || {}).status !== 'gone').forEach(t => { n++; log(t, 'Midna', 'midna', 'Closed its terminal'); if (db.midnaUp) closeSession(t.session_id); });
      return { ok: true, jobs: n };
    }],
    [/^\/goals$/, (m, b) => {
      if (!b.name) throw new HttpError(400, 'The name can’t be empty.');
      if (!b.project) throw new HttpError(400, 'The project can’t be empty.');
      const mode = (b.epic && b.epic.mode) || 'none';
      const g = { id: db.seq.goal++, name: b.name, tldr: b.tldr || '', outcome: b.outcome || '', project: b.project, repo_path: projectPath(b.project),
        epic_key: mode === 'link' ? b.epic.key : null, epic_status: mode === 'create' ? 'Asked for' : mode === 'link' ? 'To Do' : null,
        run_in_order: b.run_in_order === false ? 0 : 1, max_terminals: b.max_terminals || 2, auto_close: b.auto_close === false ? 0 : 1, archived: 0, paused: 0, deprioritized: 0, created_at: now() };
      db.goals.push(g);
      return goalFull(g);
    }],
    [/^\/goals\/([^/]+)$/, (m, b) => {
      const g = need(findGoal(m[1]), 'goal');
      ['name', 'tldr', 'outcome', 'project', 'epic_key', 'max_terminals'].forEach(k => { if (k in b) g[k] = b[k]; });
      ['run_in_order', 'auto_close', 'archived', 'paused', 'deprioritized'].forEach(k => { if (k in b) g[k] = b[k] ? 1 : 0; });
      return goalFull(g);
    }],
    [/^\/goals\/([^/]+)\/run$/, (m, b) => {
      const g = need(findGoal(m[1]), 'goal');
      let n = 0;
      goalTasks(g).forEach(t => {
        if (t.status === 'planned') { t.status = 'queued'; log(t, OWNER, 'status', 'Queued with the rest of the goal'); n++; }
        else if (t.status === 'needs' && t.needs_reason === 'start_failed') { t.status = 'queued'; t.needs_reason = null; }
      });
      g.paused = 0; g.deprioritized = 0;
      g.hours_until = b.now ? hoursOut().next_open : null;
      return { ...goalFull(g), queued_now: n };
    }],
    [/^\/goals\/([^/]+)\/plan$/, m => { need(findGoal(m[1]), 'goal'); return { ok: true, job: 'J' + db.seq.event++ }; }],
    [/^\/goals\/([^/]+)\/notes$/, (m, b) => {
      const g = need(findGoal(m[1]), 'goal');
      if (!b.text) throw new HttpError(400, 'The note can’t be empty.');
      db.notes.push({ id: db.seq.note++, goal_id: g.id, kind: b.kind || 'finding', text: b.text, source: b.source || 'you', pinned: 0, at: now() });
      return goalFull(g);
    }],
    [/^\/attachments\/([^/]+)\/remove$/, m => {
      const i = db.attachments.findIndex(a => a.id === Number(m[1]));
      if (i < 0) throw new HttpError(404, 'That attachment is already gone.');
      db.attachments.splice(i, 1);
      return { ok: true };
    }],
    [/^\/attachments\/([^/]+)$/, (m, b) => {
      const a = need(db.attachments.find(x => x.id === Number(m[1])), 'attachment');
      ['title', 'url', 'kind'].forEach(k => { if (k in b) a[k] = b[k]; });
      return attOut(a);
    }],
    [/^\/hours$/, (m, b) => {
      ['on', 'start', 'end', 'days'].forEach(k => { if (k in b) db.hours[k] = b[k]; });
      if ('today_until' in b) db.todayUntil = b.today_until === 'off' ? null : b.today_until;
      return hoursOut();
    }],
    [/^\/alerts\/([^/]+)\/dismiss$/, m => { db.alerts = db.alerts.filter(a => a.id !== m[1]); return { alerts: db.alerts }; }],
    [/^\/backlog$/, (m, b) => {
      if (!b.title) throw new HttpError(400, 'The title can’t be empty.');
      const x = issue({ id: db.seq.issue++, goal_id: b.goal_id ?? null, project: b.project, kind: b.kind || 'bug', title: b.title, said: b.said ? `“${b.said}”` : '', detail: b.detail || '',
        source: 'you', found_by_name: OWNER, how: 'You added it on the Backlog page.', created_at: now(), updated_at: now(), snapshot: {} });
      db.issues.push(x);
      ilog(x, OWNER, 'note', 'Added from the Backlog page');
      return issueFull(x);
    }],
    [/^\/backlog\/bulk$/, (m, b) => {
      const suffix = { task: 'promote', ticket: 'ticket', drop: 'drop', move: 'move' }[b.action];
      if (!suffix) throw new HttpError(400, 'Say what to do: task, ticket, drop or move.');
      const xs = (b.ids || []).map(r => need(findIssue(r), 'issue'));
      if (!xs.length) throw new HttpError(400, 'Pick at least one issue.');
      const shut = b.action !== 'move' && xs.find(x => x.state !== 'open');
      if (shut) throw new HttpError(409, `B${shut.id} isn’t open any more, so nothing changed.`);
      const [, fn] = POST.find(([re]) => re.test(`/backlog/B0/${suffix}`));
      const outs = xs.map(x => fn([null, 'B' + x.id], b));
      return { ok: true, action: b.action, count: xs.length, issues: xs.map(issueFull), tasks: outs.map(o => o.task).filter(Boolean) };
    }],
    [/^\/backlog\/([^/]+)\/promote$/, (m, b) => {
      const x = need(findIssue(m[1]), 'issue');
      if (x.state === 'task' && x.task_id) throw new HttpError(409, `It’s already T${x.task_id}.`);
      const g = x.goal_id != null ? findGoal(x.goal_id) : null;
      const planned = b.where === 'goal' && !!g;
      const t = task({ id: db.seq.task++, title: x.title, project: x.project, status: planned ? 'planned' : 'queued', goal_id: g ? g.id : null,
        position: g ? goalTasks(g).length + 1 : 0, from_issue_id: x.id, created_at: now(), updated_at: now(),
        latest: `From the backlog: ${x.found_by_name ? 'reported by ' + x.found_by_name : 'added by you'}.`,
        detail: `${x.title}.\nSee how it was found before changing anything.` });
      db.tasks.push(t);
      log(t, OWNER, 'status', `Added from the backlog (B${x.id})${planned ? ' as planned' : ''}`);
      x.state = 'task'; x.task_id = t.id;
      ilog(x, OWNER, 'task', planned ? `Made into T${t.id}, a planned task in its goal.` : `Made into T${t.id}, a queued task.`);
      return { issue: issueFull(x), task: taskFull(t) };
    }],
    [/^\/backlog\/([^/]+)\/ticket$/, m => {
      const x = need(findIssue(m[1]), 'issue');
      if (!JIRA_ON) throw new HttpError(409, 'Jira isn’t set up.');
      x.state = 'ticket';
      ilog(x, OWNER, 'ticket', 'Asked Jira for a ticket');
      setTimeout(() => { x.jira_key = 'PROJ-' + (300 + x.id); ilog(x, 'Task board', 'ticket', `Created ${x.jira_key}`); }, 3000);
      return issueFull(x);
    }],
    [/^\/backlog\/([^/]+)\/drop$/, m => { const x = need(findIssue(m[1]), 'issue'); x.state = 'drop'; ilog(x, OWNER, 'drop', 'Closed as won’t do'); return issueFull(x); }],
    [/^\/backlog\/([^/]+)\/move$/, (m, b) => {
      const x = need(findIssue(m[1]), 'issue');
      const from = db.goals.find(g => g.id === x.goal_id);
      const to = b.goal_id == null ? null : need(findGoal(b.goal_id), 'goal');
      x.goal_id = to ? to.id : null;
      if (to) x.project = to.project;
      ilog(x, OWNER, 'move', `Moved from ${from ? from.name : 'no goal'} to ${to ? to.name : 'no goal'}`);
      return issueFull(x);
    }],
    [/^\/backlog\/([^/]+)\/note$/, (m, b) => { const x = need(findIssue(m[1]), 'issue'); if (!b.text) throw new HttpError(400, 'The note can’t be empty.'); ilog(x, OWNER, 'note', `Added a note: “${b.text}”`); return issueFull(x); }],
  ];

  const realFetch = window.fetch.bind(window);
  const json = (status, body) => new Response(JSON.stringify(body), { status, headers: { 'Content-Type': 'application/json' } });
  const header = (h, name) => !h ? null : typeof h.get === 'function' ? h.get(name) : (h[name] ?? h[name.toLowerCase()] ?? null);
  window.fetch = async (input, init = {}) => {
    const url = new URL(typeof input === 'string' ? input : input.url, location.href);
    if (!url.pathname.startsWith('/tasks/api/')) return realFetch(input, init);
    const path = url.pathname.slice('/tasks/api'.length);
    const method = (init.method || 'GET').toUpperCase();
    await new Promise(r => setTimeout(r, 60 + Math.random() * 90));
    try {
      if (method === 'POST') {
        if (header(init.headers, 'X-Task-Board') !== '1') return json(403, { error: 'This request is missing the X-Task-Board header.' });
        let body = {};
        try { body = init.body ? JSON.parse(init.body) : {}; } catch { return json(400, { error: 'The body isn’t JSON.' }); }
        for (const [re, fn] of POST) { const m = path.match(re); if (m) return json(200, fn(m, body)); }
      } else if (method === 'GET') {
        for (const [re, fn] of GET) { const m = path.match(re); if (m) return json(200, fn(m, url.searchParams)); }
      } else return json(405, { error: 'That isn’t allowed here.' });
      return json(404, { error: 'There’s nothing at that address.' });
    } catch (e) {
      return json(e.status || 500, { error: e.message || 'The mock server broke.' });
    }
  };
  window.__mockDb = db;
  const p2 = n => String(n).padStart(2, '0');
  const localIso = d => `${d.getFullYear()}-${p2(d.getMonth() + 1)}-${p2(d.getDate())}T${p2(d.getHours())}:${p2(d.getMinutes())}`;
  const clock = hm => { const [h, mm] = hm.split(':').map(Number); return `${h % 12 || 12}${mm ? ':' + String(mm).padStart(2, '0') : ''}${h < 12 ? 'am' : 'pm'}`; };
  function hoursOut() {
    const h = db.hours, d = new Date(), m = d.getHours() * 60 + d.getMinutes(), mins = t => +t.slice(0, 2) * 60 + +t.slice(3);
    const day = ['sun', 'mon', 'tue', 'wed', 'thu', 'fri', 'sat'][d.getDay()];
    const until = h.on ? db.todayUntil : null;
    const open = !h.on || (until ? m < mins(until) : (h.days.includes(day) && m >= mins(h.start) && m < mins(h.end)));
    const nx = new Date(d); nx.setHours(+h.start.slice(0, 2), +h.start.slice(3), 0, 0); if (nx <= d) nx.setDate(nx.getDate() + 1);
    const span = `${clock(h.start)}–${clock(h.end)}`;
    return { ...h, open, span, today_until: until, next_open: open ? null : localIso(nx),
      line: !h.on ? 'No work hours: agents start any time' : open ? `Work hours ${span}: agents start until ${clock(until || h.end)}${until ? ' today' : ''}` : `Outside work hours: agents start again ${clock(h.start)}` };
  }
})();
