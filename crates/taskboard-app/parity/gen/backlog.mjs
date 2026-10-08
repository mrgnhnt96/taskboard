// The Backlog page and the backlog issue panel/aside (pages.js renderBacklogPage, blStateText,
// withKept, issueAside; app.js issueState, issueFrom, issueHow, snapRows, issueDetail,
// issueActions, issuePanel). Every issue state, kind and source; Jira on and off; selection,
// kept rows, loading, empty lists, inline notes, busy buttons and the revealed note form.
import { web, writeGolden, text as rawText, acts, NOW } from '../web.mjs';

// What the reader sees: the browser collapses source whitespace (newlines included) to one space.
// Screen-reader-only labels (`.sr`) aren't visible either.
const text = html => rawText(String(html ?? '').replace(/<(label|span) class="sr"[^>]*>[\s\S]*?<\/\1>/g, '')).replace(/\s+/g, ' ').trim();

const at = mins => new Date(Date.parse(NOW) - mins * 60000).toISOString();
const goals = [
  { id: 1, ref: 'G1', name: 'Sign-in with passkeys', project: 'webapp', epic_key: 'PROJ-9', archived: false },
  { id: 2, ref: 'G2', name: 'Billing revamp', project: 'api', epic_key: null, archived: false },
  { id: 3, ref: 'G3', name: 'Old archived goal', project: 'api', archived: true },
];
const projects = [{ name: 'webapp', path: '/Users/alex/code/webapp' }, { name: 'api', path: '/Users/alex/code/api' }];

const issues = {
  B1: { id: 1, ref: 'B1', kind: 'gap', title: 'No test for an expired challenge', goal: { id: 1, ref: 'G1', name: 'Sign-in with passkeys' }, goal_id: 1,
    project: 'webapp', state: 'open', task_id: null, jira_key: null, source: 'terminal', found_by_name: 'T1 Add the WebAuthn',
    found_by_task: { id: 1, ref: 'T1', title: 'Add the WebAuthn registration endpoint' }, created_at: at(55), updated_at: at(55),
    said: '“The register endpoint has no test for an expired challenge.”', how: 'The T1 terminal reported it while working on T1.',
    detail: 'Seen while writing the happy path.\nThe expiry is 5 minutes.',
    snapshot: { cwd: '/tmp/webapp', branch: 'feature/passkeys', task: { ref: 'T1', title: 'Add the WebAuthn registration endpoint' }, uncommitted: 0, empty: '', nothing: null },
    history: [
      { at: at(50), who: 'T1 Add the WebAuthn', kind: 'seen', text: 'Seen again' },
      { at: at(55), who: null, kind: 'report', text: 'Reported' },
      { at: at(10), who: 'Alex', kind: 'note', text: 'Check the clock skew too.' },
    ] },
  B2: { id: 2, ref: 'B2', kind: 'bug', title: 'Login button flickers', goal: null, goal_id: null, project: 'webapp', state: 'open',
    task_id: null, jira_key: null, source: 'you', found_by_name: 'Alex', found_by_task: null, created_at: at(60 * 26), updated_at: at(60 * 26),
    said: null, how: null, detail: 'On Safari only.', snapshot: '{"branch":"main","uncommitted":3}', history: [] },
  B3: { id: 3, ref: 'B3', kind: 'follow', title: 'Retry webhook deliveries', goal: null, goal_id: 2, project: 'api', state: 'task',
    task_id: 7, jira_key: null, source: 'answer', found_by_name: null, found_by_task: 5, created_at: at(120), updated_at: at(20),
    said: null, how: null, detail: null, snapshot: {}, history: [{ at: at(20), who: 'you', kind: 'task', text: 'Made into T7' }] },
  B4: { id: 4, ref: 'B4', kind: 'clean', title: 'Remove the old auth flag', goal: null, goal_id: null, project: 'api', state: 'ticket',
    task_id: null, jira_key: 'PROJ-12', source: 'terminal', found_by_name: 'api shell', found_by_task: null, created_at: at(300), updated_at: at(30),
    snapshot: [['step', 'cleanup'], ['file', 'src/flags.rs'], 'junk'], history: [] },
  B5: { id: 5, ref: 'B5', kind: 'bug', title: 'Crash on empty cart', goal: { id: 2, ref: 'G2', name: 'Billing revamp' }, goal_id: 2, project: 'api',
    state: 'ticket', task_id: null, jira_key: null, source: 'terminal', found_by_name: null, found_by_task: { id: 9, ref: 'T9', title: 'Cart' },
    created_at: at(5), updated_at: at(5), snapshot: null, history: [] },
  B6: { id: 6, ref: 'B6', kind: 'gap', title: 'Flaky e2e on CI', goal: { id: 1, ref: 'G1', name: 'Sign-in with passkeys' }, goal_id: 1, project: 'webapp',
    state: 'drop', task_id: null, jira_key: null, source: 'terminal', found_by_name: null, found_by_task: 4, created_at: at(60 * 24 * 3), updated_at: at(60),
    snapshot: { last_commit: 'abc123 fix', worktree: '/tmp/wt', output: 'FAIL 1 test', commit: 'def456' },
    history: [{ at: at(60), who: 'you', kind: 'drop', text: 'Closed as won’t do' }, { at: at(61), kind: 'lost', text: 'Terminal lost' }] },
  B7: { id: 7, ref: 'B7', kind: 'follow', title: 'Doc the API', goal: null, goal_id: null, project: 'api', state: 'task', task_id: null,
    source: 'terminal', found_by_name: 'docs', found_by_task: null, created_at: at(15), updated_at: at(15), snapshot: { from: 'T2 review', step: 'docs' }, history: [] },
  B8: { id: 8, ref: 'B8', kind: 'perf', title: 'Slow search', goal_id: null, project: 'webapp', state: 'open', source: 'terminal',
    found_by_name: null, found_by_task: null, created_at: at(1), updated_at: at(1), snapshot: { last_turn: 'Profiled the query', custom_key: 'x', obj: { a: 1 } }, history: [] },
  B9: { id: 9, ref: 'B9', kind: '', title: 'Odd snapshot', goal_id: 2, project: 'api', state: 'open', source: null, found_by_name: null,
    found_by_task: null, created_at: at(2), updated_at: at(2), snapshot: 'not json at all', history: [{ at: at(1), kind: 'move', text: 'Moved to G2' }, { at: at(0), kind: 'ticket', text: 'x' }, { at: at(0.5), kind: 'source', who: 'T3', text: 'y' }] },
};
const all = Object.values(issues);

const w = web();
w.run('var __html = ""; patch = (el, html) => { __html = html; };');
const base = jira => {
  w.set('S.state', { projects, jira: { enabled: jira }, counts: { open_issues: 11, needs: 0 }, alerts: [], sessions: [] });
  w.set('S.goalsAll', goals);
  w.set('S.route', { page: 'backlog', id: null, q: {} });
  w.run('S.busy = new Set(); S.notes = {}; S.reveal = {}; S.drafts = {}; S.issue = null; S.issueRef = null; S.issueErr = ""');
  w.run('P.kept = {}; P.order = []; P.backlog = null; P.backlogErr = ""; P.openAll = null; Object.assign(P.blFilter, { project: "all", goal: "all", kind: "all", state: "open", sort: "new" })');
};
const extras = o => {
  if (o.notes) w.set('S.notes', o.notes);
  if (o.busy) w.run(`S.busy = new Set(${JSON.stringify(o.busy)})`);
  if (o.reveal) w.set('S.reveal', Object.fromEntries(o.reveal.map(k => [k, true])));
  if (o.drafts) w.set('S.drafts', o.drafts);
};

const cases = [];
// ---- pure helpers
for (const b of all) {
  const r = b.ref;
  cases.push({ name: `issueState ${r}`, input: { fn: 'issueState', issue: b }, expect: w.call('issueState', b) && (({ short, text }) => ({ short, text }))(w.call('issueState', b)) });
  cases.push({ name: `issueFrom long ${r}`, input: { fn: 'issueFrom', issue: b, long: true }, expect: w.call('issueFrom', b, true) });
  cases.push({ name: `issueFrom short ${r}`, input: { fn: 'issueFrom', issue: b, long: false }, expect: w.call('issueFrom', b, false) });
  cases.push({ name: `issueHow ${r}`, input: { fn: 'issueHow', issue: b }, expect: w.call('issueHow', b) });
  cases.push({ name: `snapRows ${r}`, input: { fn: 'snapRows', snapshot: b.snapshot ?? null }, expect: w.call('snapRows', b.snapshot ?? null) });
  base(false);
  cases.push({ name: `blStateText ${r}`, input: { fn: 'blStateText', issue: b, goals }, expect: w.call('blStateText', b) });
  cases.push({ name: `kindLabel ${b.kind}`, input: { fn: 'kindLabel', kind: b.kind }, expect: w.call('kindLabel', b.kind) });
}

// ---- the Backlog page
const page = (name, o) => {
  base(!!o.jira);
  w.run(`Object.assign(P.blFilter, ${JSON.stringify(o.filter || {})})`);
  if (o.backlog !== null) w.set('P.backlog', { issues: o.issues, total: o.total ?? o.issues.length });
  if (o.openAll != null) w.set('P.openAll', o.openAll);
  if (o.backlogErr) w.set('P.backlogErr', o.backlogErr);
  if (o.kept) { w.set('P.kept', o.kept); w.set('P.order', o.order); }
  if (o.counts === null) w.run('S.state.counts = null');
  w.set('S.issueRef', o.selected ?? null);
  w.set('S.issue', o.selected ? issues[o.selected] : null);
  extras(o);
  w.run('renderBacklogPage({})');
  const html = w.run('__html');
  const rows = [...html.matchAll(/<li class="brow[\s\S]*?<\/li>/g)].map(m => ({ text: text(m[0]), acts: acts(m[0]) }));
  const header = html.match(/<header[\s\S]*?<\/header>/)[0];
  const filters = html.match(/<div class="bfilters"[\s\S]*?<\/div>\s*<div class="bgrid">/)[0];
  const states = [...filters.matchAll(/<select id="bl-state"[\s\S]*?<\/select>/g)][0][0];
  const list = html.match(/<section class="stack" aria-label="Issues"[\s\S]*?<\/section>/)[0];
  const aside = (html.match(/<aside[\s\S]*?<\/aside>/) || [''])[0];
  cases.push({
    name: `page ${name}`,
    input: { fn: 'page', jira: !!o.jira, goals, projects, counts: o.counts === null ? null : { open_issues: 11 }, filter: { project: 'all', goal: 'all', kind: 'all', state: 'open', sort: 'new', ...(o.filter || {}) },
      backlog: o.backlog === null ? null : { issues: o.issues, total: o.total ?? o.issues.length }, openAll: o.openAll ?? null, backlogErr: o.backlogErr ?? null,
      kept: o.kept || {}, order: o.order || [], selected: o.selected ?? null, issue: o.selected ? issues[o.selected] : null,
      notes: o.notes || {}, busy: o.busy || [], reveal: o.reveal || [], drafts: o.drafts || {} },
    expect: {
      header: text(header), header_acts: acts(header),
      filters: text(filters.replace(/<select[\s\S]*?<\/select>/g, '')), filter_values: [...filters.matchAll(/class="picker-val">([^<]*)</g)].map(m => m[1]),
      state_options: [...states.matchAll(/<option value="([^"]*)"/g)].map(m => m[1]),
      list_head: text(list.replace(/<ul class="rows">[\s\S]*<\/ul>/, '')),
      rows, aside: text(aside), aside_acts: acts(aside),
    },
  });
};
const open = all.filter(b => b.state === 'open');
page('default jira off', { issues: open, total: open.length, selected: null });
page('default jira on, B1 selected', { jira: true, issues: open, total: open.length, selected: 'B1' });
page('every state', { issues: all, total: 12, filter: { state: 'all', sort: 'old' }, openAll: 4, selected: 'B3' });
page('every state jira on, filtered', { jira: true, issues: all, total: all.length, filter: { project: 'api', goal: 'G2', kind: 'bug', state: 'ticket', sort: 'kind' }, selected: 'B4' });
page('ticket state kept without jira', { issues: [issues.B4], total: 1, filter: { state: 'ticket' }, selected: null });
page('empty', { issues: [], total: 0, filter: { kind: 'clean' } });
page('empty default', { issues: [], total: 0 });
page('loading', { backlog: null, issues: [] });
page('loading no counts', { backlog: null, issues: [], counts: null });
page('list failed', { backlog: null, issues: [], backlogErr: 'Can’t reach the task board server.' });
page('one issue', { issues: [issues.B2], total: 1 });
page('kept rows go back in place', { issues: [issues.B1, issues.B8], total: 2, kept: { B6: issues.B6, B2: issues.B2 }, order: ['B6', 'B1', 'B2', 'B8'] });
page('project not in list', { issues: open, filter: { project: 'mobile', goal: 'none' } });
page('notes and busy', { jira: true, issues: open, notes: { 'issue:B1': { text: 'Closed as won’t do', err: false }, 'issue:B2': { text: 'Jira said no.', err: true } }, busy: ['promote:goal:B1', 'drop::B2', 'ticket::B8'], selected: 'B2' });
page('aside with note open', { issues: open, selected: 'B1', reveal: ['inote:B1'], drafts: { 'inote:B1': 'half a note' }, notes: { 'move:B1': { text: 'Moved', err: false }, 'inote:B1': { text: 'Write the note first.', err: true } } });
page('aside selected not in list', { issues: open, selected: 'B6' });

// ---- the issue panel (board page) and the aside (backlog page)
const panel = (name, b, o = {}) => {
  base(!!o.jira);
  w.set('S.route', { page: 'board', id: null, q: { issue: b.ref } });
  w.set('S.issueRef', b.ref);
  w.set('S.issue', b);
  extras(o);
  const html = w.call('issuePanel');
  cases.push({ name: `panel ${name}`, input: { fn: 'panel', issue: b, jira: !!o.jira, goals, notes: o.notes || {}, busy: o.busy || [], reveal: o.reveal || [], drafts: o.drafts || {} }, expect: { text: text(html), acts: acts(html) } });
};
for (const b of all) { panel(`${b.ref} jira off`, b); panel(`${b.ref} jira on`, b, { jira: true }); }
panel('B1 note revealed with draft and notes', issues.B1, { reveal: ['inote:B1'], drafts: { 'inote:B1': 'draft text' }, notes: { 'issue:B1': { text: 'Asked Jira for a ticket', err: false }, 'move:B1': { text: 'No such goal.', err: true }, 'inote:B1': { text: 'Note added', err: false } } });
panel('B2 busy', issues.B2, { jira: true, busy: ['promote:board:B2', 'issue-note::B2'], reveal: ['inote:B2'] });
panel('B6 dropped with note', issues.B6, { notes: { 'issue:B6': { text: 'Closed as won’t do', err: false } } });

writeGolden('backlog', cases);
