// The board page (app.js): the sessions strip (sessionsHtml, sessCard, nameView), the goal bar
// (goalBarHtml), the five columns (columnsHtml, colShell, taskCard, issueCard, issueState,
// issueFrom, doneMenuButton) and which cards drag to Working (startsByHand).
import { web, writeGolden, text as rawText, acts, NOW } from '../web.mjs';

// Visible text with the template's line breaks collapsed (layout, not content).
const text = h => rawText(h).replace(/\s+/g, ' ').trim();

const at = mins => new Date(Date.parse(NOW) - mins * 60000).toISOString();
const attr = (html, cls, name) => {
  const m = String(html).match(new RegExp(`class="[^"]*\\b${cls}\\b[^"]*"[^>]*?\\s${name}="([^"]*)"`));
  return m ? m[1].replace(/&amp;/g, '&').replace(/&quot;/g, '"').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&#39;/g, "'") : null;
};
const classes = (html, base) => [...String(html).matchAll(new RegExp(`class="${base} ([^"]+)"`, 'g'))].map(m => m[1]);

const goals = [
  { id: 1, ref: 'G1', name: 'Sign-in with passkeys', project: 'webapp', total: 4, done: 1, active: 2, needs: 1, open_issues: 1, prs_open: [], archived: false },
  { id: 2, ref: 'G2', name: 'Faster builds', project: 'api', total: 3, done: 3, active: 0, needs: 0, open_issues: 0, prs_open: [41], paused: true },
  { id: 3, ref: 'G3', name: 'Docs refresh', project: 'webapp', total: 2, done: 0, active: 1, needs: 0, open_issues: 2, prs_open: [], deprioritized: true },
];
const g1 = { id: 1, ref: 'G1', name: 'Sign-in with passkeys' };
const tasks = {
  queued: [
    { id: 5, ref: 'T5', title: 'Upgrade the HTTP client', project: 'webapp', status: 'queued', priority: 'high', when: at(5), goal: null, jira: null, pr: null, starting: false },
    { id: 6, ref: 'T6', title: 'Queued in a goal', project: 'webapp', status: 'queued', priority: 'normal', when: at(30), goal: g1, starting: false, blocked: true },
    { id: 7, ref: 'T7', title: 'Starting now', project: 'api', status: 'queued', priority: 'normal', when: at(1), goal: null, starting: true },
  ],
  working: [
    { id: 2, ref: 'T2', title: 'Sign in with a passkey', project: 'webapp', status: 'working', priority: 'normal', when: at(2), goal: g1, who: 'T2 Sign in with a passkey',
      jira: { key: 'PROJ-12', status: 'In Progress', url: 'https://x.atlassian.net/browse/PROJ-12' }, pr: { repo: 'webapp', num: 12, url: 'https://github.com/x/webapp/pull/12', state: 'OPEN', checks: 'pass' } },
    { id: 8, ref: 'T8', title: 'API rate limits', project: 'api', status: 'working', priority: 'normal', updated_at: at(90), goal: null, who: 'api shell', pr: { num: 3, checks: 'pending', state: 'OPEN' } },
  ],
  needs: [
    { id: 3, ref: 'T3', title: 'Settings page: manage passkeys', project: 'webapp', status: 'needs', priority: 'normal', when: at(3), goal: g1, question: 'Should removing the last passkey require a password re-check?', who: 'T3 Settings', needs_reason: 'question' },
    { id: 9, ref: 'T9', title: 'Lost one', project: 'webapp', status: 'needs', priority: 'high', when: at(12), goal: null, lost: true, needs_reason: 'lost', who: null },
  ],
  done: [
    { id: 1, ref: 'T1', title: 'Add the WebAuthn registration endpoint', project: 'webapp', status: 'done', priority: 'normal', when: at(60 * 5), goal: g1, who: 'T1 Add the WebAuthn', pr: { num: 10, checks: 'fail', state: 'MERGED' } },
    { id: 10, ref: 'T10', title: 'Failed migration', project: 'api', status: 'done', failed: true, priority: 'normal', when: at(60 * 20), goal: null, pr: { num: 11, checks: 'weird' } },
    { id: 11, ref: 'T11', title: 'No PR number', project: 'api', status: 'done', priority: 'normal', when: at(60 * 2), goal: null, pr: { num: null, checks: 'pass' } },
  ],
};
const issues = [
  { id: 1, ref: 'B1', kind: 'gap', title: 'No test for an expired challenge', goal: g1, goal_id: 1, project: 'webapp', state: 'open', source: 'terminal', found_by_name: 'T1 Add the WebAuthn', found_by_task: 1, created_at: at(40) },
  { id: 2, ref: 'B2', kind: 'bug', title: 'Made into a task', goal: null, goal_id: null, project: 'webapp', state: 'task', task_id: 12, source: 'you', found_by_name: 'Alex', created_at: at(60 * 30) },
  { id: 3, ref: 'B3', kind: 'follow', title: 'Ticket with key', goal_id: 3, project: 'webapp', state: 'ticket', jira_key: 'PROJ-40', source: 'answer', created_at: at(10) },
  { id: 4, ref: 'B4', kind: 'clean', title: 'Ticket being created', goal: null, project: 'api', state: 'ticket', jira_key: null, created_at: at(2) },
  { id: 5, ref: 'B5', kind: 'odd', title: 'Dropped', goal: null, project: 'api', state: 'drop', source: 'terminal', created_at: at(60 * 24 * 3) },
  { id: 6, ref: 'B6', kind: '', title: 'Nobody reported it', goal: null, project: 'webapp', state: 'open', created_at: at(0) },
];
const sessions = [
  { id: 's-idle', name: 'api shell', project: 'api', project_path: '/tmp/api', status: 'idle', task_ref: null, task_title: null, can_take: true, close: 'close', closing: false, renaming: null, rename_error: null },
  { id: 's-work', name: 'T2 Sign in with a passkey', project: 'webapp', project_path: '/tmp/webapp', status: 'working', task_ref: 'T2', task_title: 'Sign in with a passkey', close: 'force', renaming: null, rename_error: null },
  { id: 's-needs', name: 'T3 Settings', project: 'webapp', project_path: '/tmp/webapp', status: 'needs', task_ref: 'T3', task_title: 'Settings page: manage passkeys', close: 'force', renaming: null, rename_error: null },
  { id: 's-ren', name: 'old name', project: 'webapp', project_path: '/tmp/webapp', status: 'idle', task_ref: null, task_title: 'A title without a ref', renaming: 'new name', rename_error: null },
  { id: 's-err', name: 'stubborn', project: 'webapp', project_path: null, status: 'weird', task_ref: null, task_title: null, renaming: null, rename_error: 'Midna said no' },
  { id: 's-noname', name: '', project: 'webapp', status: 'idle' },
];
const many = Array.from({ length: 8 }, (_, i) => ({ id: `m${i}`, name: `term ${i}`, project: 'webapp', project_path: '/tmp/webapp', status: i % 3 === 0 ? 'needs' : i % 3 === 1 ? 'working' : 'idle' }));

const baseState = (over = {}) => ({
  now: NOW, midna: { up: true, seen_at: at(0) }, jira: { enabled: false }, alerts: [], projects: [{ name: 'api', path: '/tmp/api' }, { name: 'webapp', path: '/tmp/webapp' }],
  sessions, session_projects: ['api', 'webapp'], goals,
  columns: { backlog: issues, backlog_open: 9, ...tasks },
  counts: { needs: 2, open_issues: 9, done_hidden: 0 },
  ...over,
});

const scenarios = [
  { name: 'everything', filter: { project: 'all', goal: 'all', done: '24h' }, state: baseState(), route: { page: 'board', id: null, q: {} } },
  { name: 'project webapp', filter: { project: 'webapp', goal: 'all', done: '24h' }, state: baseState(), route: { page: 'board', id: null, q: { task: 'T2' } } },
  { name: 'goal G1', filter: { project: 'webapp', goal: 'G1', done: '24h' }, state: baseState(), route: { page: 'board', id: null, q: { issue: 'B1' } } },
  { name: 'goal G2 paused', filter: { project: 'api', goal: 'G2', done: '7d' }, state: baseState(), route: { page: 'board', id: null, q: {} } },
  { name: 'goal G3 deprioritized', filter: { project: 'webapp', goal: 'G3', done: 'all' }, state: baseState(), route: { page: 'board', id: null, q: {} } },
  { name: 'done hidden 24h', filter: { project: 'all', goal: 'all', done: '24h' }, state: baseState({ counts: { needs: 0, open_issues: 0, done_hidden: 1 } }), route: { page: 'board', id: null, q: {} } },
  { name: 'done empty with hidden 7d', filter: { project: 'all', goal: 'all', done: '7d' }, state: baseState({ columns: { backlog: [], backlog_open: 0, queued: [], working: [], needs: [], done: [] }, counts: { done_hidden: 14 } }), route: { page: 'board', id: null, q: {} } },
  { name: 'empty board, filter api', filter: { project: 'api', goal: 'all', done: 'all' }, state: baseState({ sessions: [], columns: { backlog: [], queued: [], working: [], needs: [], done: [] }, counts: {} }), route: { page: 'board', id: null, q: {} } },
  { name: 'many sessions', filter: { project: 'all', goal: 'all' }, state: baseState({ sessions: many }), route: { page: 'board', id: null, q: {} } },
  { name: 'loading', filter: { project: 'all', goal: 'all', done: '24h' }, state: null, stateErr: '', route: { page: 'board', id: null, q: {} } },
  { name: 'cannot load', filter: { project: 'all', goal: 'all', done: '24h' }, state: null, stateErr: 'Can’t reach the task board server.', route: { page: 'board', id: null, q: {} } },
];

const cases = [];
for (const sc of scenarios) {
  const w = web();
  w.set('S.filter', sc.filter);
  w.set('S.state', sc.state);
  w.set('S.stateErr', sc.stateErr || '');
  w.set('S.route', sc.route);
  w.set('S.goalsAll', goals);
  const input = { fn: 'board', filter: sc.filter, state: sc.state, down: sc.stateErr || null, route: sc.route, goals };

  // Sessions strip.
  const strip = w.run('sessionsHtml()');
  const cards = (sc.state ? w.run(`(S.state.sessions || []).filter(s => S.filter.project === 'all' || s.project === S.filter.project).sort((a, b) => (SESS_ORDER[a.status] ?? 2) - (SESS_ORDER[b.status] ?? 2)).slice(0, STRIP_MAX).map(s => [s.id, sessCard(s)])`) : [])
    .map(([id, h]) => ({
      id, text: text(h), acts: acts(h),
      name_title: attr(h, 'sess-name', 'title'), project_title: attr(h, 'sess-proj', 'title'),
      renaming: /sess-name renaming/.test(h), needs: /class="sess [^"]*\bneeds\b/.test(h), gone: /class="sess [^"]*\bgone\b/.test(h),
    }));
  // Columns: split into the five sections.
  const colsHtml = w.run('columnsHtml()');
  const cols = colsHtml.split('<section').slice(1).map(h => ({ text: text('<section' + h), acts: acts(h), drop: /data-drop="working"/.test(h) }));
  const shownTasks = sc.state ? w.run(`['queued','working','needs','done'].flatMap(k => filterTasks(S.state.columns[k] || []))`) : [];
  const taskCards = shownTasks.map(t => {
    const h = w.call('taskCard', t);
    return {
      ref: t.ref, text: text(h), acts: acts(h),
      when_title: attr(h, 't-when', 'title'), goal_title: attr(h, 'goal-line', 'title'),
      drag: /draggable="true"/.test(h), drag_title: (h.match(/draggable="true" data-drag="[^"]*" title="([^"]*)"/) || [])[1] || null,
      selected: /aria-pressed="true"/.test(h), needs: /class="tcard needs"/.test(h), chips: classes(h, 'chip'),
    };
  });
  const shownIssues = sc.state ? w.run('filterIssues(S.state.columns.backlog || [])') : [];
  const issueCards = shownIssues.map(b => {
    const h = w.call('issueCard', b);
    return { ref: b.ref, text: text(h), acts: acts(h), when_title: attr(h, 't-when', 'title'), goal_title: attr(h, 'goal-line', 'title'), goal_none: /goal-line none/.test(h), selected: /aria-pressed="true"/.test(h), chips: classes(h, 'chip') };
  });
  const bar = w.run('goalBarHtml()');
  w.run('S.doneMenu = true');
  const doneMenu = w.run('doneMenuButton()');
  cases.push({
    name: sc.name, input,
    expect: {
      strip: { text: text(strip), acts: acts(strip), cards },
      columns: cols, task_cards: taskCards, issue_cards: issueCards,
      goal_bar: bar ? { text: text(bar), acts: acts(bar) } : null,
      done_menu: { text: text(doneMenu), acts: acts(doneMenu), close_disabled: /data-act="close-done" data-grp="done-col" disabled/.test(doneMenu) },
    },
  });
}

// What /state is asked for (loadState's query) and how the filters move (goal-pick, done-window).
const w = web();
w.set('S.goalsAll', goals);
for (const f of [{ project: 'all', goal: 'all' }, { project: 'webapp', goal: 'G1', done: '7d' }, { project: 'api', goal: 'G2', done: 'all' }]) {
  w.set('S.filter', f);
  w.set('S.keepIssues', f.goal === 'G1' ? ['B3', 'B4'] : []);
  const q = w.run(`(() => { const f = S.filter; const q = new URLSearchParams({ project: f.project || 'all', goal: f.goal === 'all' ? 'all' : String(idOf(f.goal)), done: f.done || '24h' }); if (S.keepIssues.length) q.set('keep', S.keepIssues.join(',')); return Object.fromEntries(q); })()`);
  cases.push({ name: `state query ${JSON.stringify(f)}`, input: { fn: 'state_query', filter: f, keep: f.goal === 'G1' ? ['B3', 'B4'] : [] }, expect: q });
}
for (const [f, arg] of [[{ project: 'webapp', goal: 'G1', done: '24h' }, 'all'], [{ project: 'all', goal: 'all', done: '7d' }, 'G2'], [{ project: 'api', goal: 'G2' }, 'G2']]) {
  w.set('S.filter', f);
  w.set('S.keepIssues', ['B1']);
  w.run(`ACTIONS['goal-pick']({ dataset: { arg: ${JSON.stringify(arg)} } })`);
  cases.push({ name: `goal-pick ${arg} from ${JSON.stringify(f)}`, input: { fn: 'goal_pick', filter: f, arg }, expect: { filter: w.run('({ ...S.filter })'), keep: w.run('S.keepIssues.slice()') } });
}
for (const [f, arg] of [[{ project: 'all', goal: 'all', done: '24h' }, '7d'], [{ project: 'webapp', goal: 'G1' }, 'all']]) {
  w.set('S.filter', f);
  w.set('S.keepIssues', ['B1']);
  w.run(`ACTIONS['done-window']({ dataset: { arg: ${JSON.stringify(arg)} } })`);
  cases.push({ name: `done-window ${arg}`, input: { fn: 'done_window', filter: f, arg }, expect: { filter: w.run('({ ...S.filter })'), keep: w.run('S.keepIssues.slice()') } });
}
// keepIssue only keeps on the board page.
for (const page of ['board', 'backlog']) {
  w.set('S.route', { page, id: null, q: {} });
  w.set('S.keepIssues', ['B1']);
  w.run(`keepIssue('B1'); keepIssue('B2')`);
  cases.push({ name: `keepIssue on ${page}`, input: { fn: 'keep_issue', page, keep: ['B1'], add: ['B1', 'B2'] }, expect: w.run('S.keepIssues.slice()') });
}
// nameView for each rename state (no flash running).
for (const s of [{ id: 'a', name: 'plain' }, { id: 'b', name: 'old', renaming: 'new' }, { id: 'c', name: 'kept', rename_error: 'Midna said no' }, { id: 'd', name: '' }]) {
  const v = w.call('nameView', s);
  cases.push({ name: `nameView ${s.id}`, input: { fn: 'name_view', session: s }, expect: { text: v.text, renaming: v.cls.includes('renaming'), title: (v.attrs.match(/title="([^"]*)"/) || [])[1] || null } });
}
writeGolden('board', cases);
