// The task side panel (app.js: taskPanel, overviewTab, manageBox, contextTab, logTab, the PR steps,
// attachments, terminals…) rendered by the web board for every state, plus its pure helpers.
//
// Each panel case records what the reader sees and can do:
//   text   – visible text in order (screen-reader-only labels dropped, whitespace collapsed)
//   acts   – every control with a data-act, in order: "act", "act:arg", trailing "!" = disabled
//   titles – every tooltip (title attribute), in order
//   values – the contents of every input / textarea, in order
import { web, writeGolden, NOW, text } from '../web.mjs';

const w = web();
const at = mins => new Date(Date.parse(NOW) - mins * 60000).toISOString();
const unesc = s => String(s).replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&');

// What the reader can see: a closed <details> shows only its <summary> (innermost first).
function visible(html) {
  let h = String(html), prev;
  const inner = /<details\b([^>]*)>((?:(?!<details\b)[\s\S])*?)<\/details>/;
  do {
    prev = h;
    h = h.replace(inner, (_, attrs, body) => {
      if (/\bopen\b/.test(attrs)) return body;
      const sum = body.match(/<summary\b[^>]*>[\s\S]*?<\/summary>/);
      return sum ? sum[0] : '';
    });
  } while (h !== prev);
  return h;
}
function seen(html) {
  const h = String(html).replace(/<label class="sr"[^>]*>[\s\S]*?<\/label>/g, '').replace(/<textarea[\s\S]*?<\/textarea>/g, ' ');
  return text(h).replace(/\s+/g, ' ').trim();
}
function actList(html) {
  return [...String(html).matchAll(/<(?:button|a)\b[^>]*\bdata-act="([^"]*)"[^>]*>/g)].map(m => {
    const tag = m[0];
    const arg = (tag.match(/\bdata-arg="([^"]*)"/) || [])[1];
    const dis = /\sdisabled(\s|>|=)/.test(tag);
    return unesc(m[1]) + (arg != null && arg !== '' ? ':' + unesc(arg) : '') + (dis ? '!' : '');
  });
}
const titles = html => [...String(html).matchAll(/\btitle="([^"]*)"/g)].map(m => unesc(m[1]));
function values(html) {
  const out = [];
  for (const m of String(html).matchAll(/<input\b[^>]*>|<textarea\b[^>]*>([\s\S]*?)<\/textarea>/g)) {
    if (m[0].startsWith('<textarea')) out.push(unesc(m[1]));
    else out.push(unesc((m[0].match(/\bvalue="([^"]*)"/) || [])[1] ?? ''));
  }
  return out;
}

const STATE = {
  now: NOW,
  midna: { up: true, seen_at: at(0) },
  jira: { enabled: false },
  alerts: [],
  work_hours: { on: false, start: '09:00', end: '17:00', days: ['mon', 'tue', 'wed', 'thu', 'fri'], open: true, line: 'No work hours', next_open: null, today_until: null },
  usage: null,
  projects: [{ name: 'webapp', path: '/tmp/webapp' }],
  sessions: [], session_projects: [], goals: [],
  columns: { backlog: [], backlog_open: 0, queued: [], working: [], needs: [], done: [] },
  counts: { needs: 0, open_issues: 0, done_hidden: 0 },
};

const GOAL = { id: 1, ref: 'G1', name: 'Sign-in with passkeys', position: 2, total: 4, next_title: 'Docs: passkey support', open_issues: 1 };
function task(over = {}) {
  return {
    id: 7, ref: 'T7', title: 'Sign in with a passkey', project: 'webapp', status: 'working', priority: 'normal',
    failed: false, lost: false, needs_reason: null, question: null, latest: null, summary: null,
    when: at(4), updated_at: at(4), started_at: at(125), finished_at: null, who: null, session_id: null,
    goal: null, jira: null, pr: null, position: null, starting: false, waiting: null, blocked: false,
    detail: '', repo_path: '/tmp/webapp', created_at: at(300), session: null, terminals: [],
    claude_session_id: null, blocked_by: [], meta: [], context: {}, log: [], handoff: 'You are working on T7.',
    found: [], attachments: [], goal_attachments: [],
    ...over,
  };
}
const live = (status = 'working', id = 's1', name = 'T7 Sign in with a passkey') => ({ session_id: id, who: name, session: { id, name, status } });
const pr = (over = {}) => ({ repo: 'webapp', num: 42, url: 'https://github.com/acme/webapp/pull/42', title: 'Passkey sign-in', state: 'OPEN', checks: 'pending', review: 'pending', stage: null, ...over });

function reset(st, route) {
  w.set('S.state', st);
  w.set('S.route', route);
  w.run(`S.drafts = {}; S.busy = new Set(); S.notes = {}; S.reveal = {}; S.metaEdit = {}; S.handoffs = {}; S.logFilter = 'all'; S.attMenu = null; S.attEdit = null; S.taskErr = '';`);
}

const cases = [];
function panel(name, t, o = {}) {
  const tab = o.tab || null;
  const q = { ...(t ? { task: t.ref } : { task: o.ref }), ...(tab ? { tab } : {}), ...(o.from ? { from: o.from } : {}) };
  reset(o.state || STATE, { page: o.page || 'board', id: o.pageId || null, q });
  w.set('S.task', t);
  w.set('S.taskRef', t ? t.ref : o.ref);
  if (o.setup) w.run(o.setup);
  if (o.err) w.set('S.taskErr', o.err);
  const html = visible(w.run('taskPanel()'));
  cases.push({ name, input: { fn: 'panel', task: t, ref: o.ref || null, err: o.err || null, tab: tab || 'overview', from: o.from || null, page: o.page || 'board', page_id: o.pageId || null, state: o.state || STATE, ui: o.ui || {} },
    expect: { text: seen(html), acts: actList(html), titles: titles(html), values: values(html) } });
}
const ui = o => ({ ui: o, setup: [
  o.drafts ? `Object.assign(S.drafts, ${JSON.stringify(o.drafts)});` : '',
  o.busy ? `${JSON.stringify(o.busy)}.forEach(k => S.busy.add(k));` : '',
  o.notes ? `Object.assign(S.notes, ${JSON.stringify(o.notes)});` : '',
  o.reveal ? `${JSON.stringify(o.reveal)}.forEach(k => S.reveal[k] = true);` : '',
  o.log_filter ? `S.logFilter = ${JSON.stringify(o.log_filter)};` : '',
  o.att_menu ? `S.attMenu = ${JSON.stringify(o.att_menu)};` : '',
  o.att_edit ? `S.attEdit = ${JSON.stringify(o.att_edit)};` : '',
  o.handoffs ? `Object.assign(S.handoffs, ${JSON.stringify(o.handoffs)});` : '',
  o.meta_edit ? `Object.assign(S.metaEdit, ${JSON.stringify(o.meta_edit)});` : '',
  o.folds ? Object.entries(o.folds).map(([k, v]) => `localStorage.setItem(${JSON.stringify(k)}, ${JSON.stringify(v)});`).join('') : '',
].join('') });
const clearFolds = `['tb.linked','tb.fold.summary','tb.fold.what','tb.fold.terms'].forEach(k => localStorage.removeItem(k));`;
function withUi(o) { const u = ui(o); return { ...u, setup: clearFolds + u.setup }; }

// ---- loading
panel('loading', null, { ref: 'T7' });
panel('loading with trail', null, { ref: 'T7', from: 'T2,T5' });
panel('fetch failed', null, { ref: 'T99', err: 'There’s nothing at that address.' });
panel('fetch failed after loading', task({}), { err: 'The board answered 500.' });

// ---- needs
const asking = task({ status: 'needs', needs_reason: 'question', question: 'Should removing the last passkey require a password re-check?', ...live('needs'), goal: GOAL,
  terminals: [{ id: 's1', name: 'T7 Sign in with a passkey', status: 'needs', why: 'Worked on the task', at: at(120) }] });
panel('needs: question', asking, withUi({}));
panel('needs: question with draft and note', asking, withUi({ drafts: { 'answer:T7': 'Yes, ask for it.' }, notes: { 'ask:T7': { text: 'Write an answer first.', err: true } } }));
panel('needs: question, sending', asking, withUi({ busy: ['answer::T7'] }));
const hoursClosed = { ...STATE, work_hours: { ...STATE.work_hours, on: true, open: false, next_open: '2026-10-08T06:00', line: 'Agents off until Thu 6am' } };
panel('needs: question, hours closed', asking, { ...withUi({}), state: hoursClosed });
const hoursToday = { ...STATE, work_hours: { ...STATE.work_hours, on: true, open: false, next_open: '2026-10-07T18:00' } };
panel('needs: question, hours open later today', asking, { ...withUi({}), state: hoursToday });
panel('needs: no question, live idle terminal', task({ status: 'needs', needs_reason: 'attention', latest: 'Waiting at its prompt.', ...live('idle') }), withUi({}));
panel('needs: no question, no terminal', task({ status: 'needs', needs_reason: 'attention' }), withUi({}));
panel('needs: alert and high priority', asking, { ...withUi({}), state: { ...STATE, alerts: [{ id: 'a1', at: at(3), text: 'Your answer to T7 didn’t arrive.', task: 'T7', goal: null }] } });
panel('needs: priority high', task({ ...asking, priority: 'high' }), withUi({}));

// ---- lost / start failed
const lost = task({ status: 'needs', needs_reason: 'lost', lost: true, latest: 'The terminal closed at 2:10.', session_id: 's9', who: 'T7 old', session: { id: 's9', name: 'T7 old', status: 'gone' },
  claude_session_id: 'c-1', context: { saved_at: at(30), turns: 12, checkpoints: 1 },
  terminals: [{ id: 's9', name: 'T7 old', status: 'gone', why: 'Worked on the task', at: at(90) }] });
panel('lost: can reopen', lost, withUi({}));
panel('lost: no conversation', task({ ...lost, claude_session_id: null, latest: null, context: {} }), withUi({}));
panel('lost: note after resume', lost, withUi({ notes: { 'lost:T7': { text: 'Sent to Midna', err: false } } }));
panel('start failed', task({ status: 'needs', needs_reason: 'start_failed', latest: null }), withUi({}));
panel('start failed with latest', task({ status: 'needs', needs_reason: 'start_failed', latest: 'Midna answered: no such project.' }), withUi({}));

// ---- queued / planned
panel('queued by hand, waiting', task({ status: 'queued', started_at: null, when: at(5), waiting: 'Waits for work hours (tomorrow 6am)' }), withUi({}));
panel('queued by hand, busy start', task({ status: 'queued', started_at: null, when: at(5) }), withUi({ busy: ['start:new:T7'], notes: { 'act:T7': { text: 'Sent to Midna', err: false } } }));
panel('queued starting', task({ status: 'queued', started_at: null, starting: true }), withUi({}));
panel('queued blocked', task({ status: 'queued', started_at: null, blocked: true, goal: GOAL,
  blocked_by: [{ id: 4, ref: 'T4', title: 'Add the registration endpoint', status: 'working', failed: false, blocked: false, starting: false, pr: null }, { id: 5, ref: 'T5', title: 'Migrations', status: 'queued', blocked: true }] }), withUi({}));
panel('planned in goal', task({ status: 'planned', started_at: null, goal: GOAL }), withUi({}));
panel('planned not in goal', task({ status: 'planned', started_at: null }), withUi({}));
panel('planned, goal page of its goal', task({ status: 'planned', started_at: null, goal: GOAL }), { ...withUi({}), page: 'goal', pageId: 'G1' });
panel('goal on another goal page', task({ status: 'planned', started_at: null, goal: GOAL }), { ...withUi({}), page: 'goal', pageId: 'G2' });
panel('goal last task, no backlog', task({ goal: { id: 1, ref: 'G1', name: 'G', position: 4, total: 4, next_title: null, open_issues: 0 } }), withUi({}));
panel('goal position out of range', task({ goal: { id: 1, ref: 'G1', name: 'G', position: 5, total: 4 } }), withUi({}));

// ---- working + manage
const working = task({ ...live('working'), latest: 'Wiring the assertion check.',
  terminals: [
    { id: 's1', name: 'T7 Sign in with a passkey', status: 'working', why: 'Worked on the task', at: at(60) },
    { id: 's0', name: 'T7 first try', status: 'gone', why: 'Worked on the task', at: at(200) },
    { id: null, name: 'PR fixer', status: 'gone', why: 'Worked on the PR', at: null },
  ] });
panel('working: manage', working, withUi({}));
panel('working: earlier terminals open', working, withUi({ folds: { 'tb.fold.terms': 'open' } }));
panel('working: details closed', working, withUi({ folds: { 'tb.linked': 'closed' } }));
panel('working: what to do open', task({ ...working, detail: 'Add the login ceremony.\n\n- POST /auth/passkeys/login\n- button on `sign-in.tsx`\n  continued\n\n    code line\nSee https://example.com/spec.' }), withUi({ folds: { 'tb.fold.what': 'open' } }));
panel('working: mark done form', working, withUi({ reveal: ['done:T7'], drafts: { 'summary:T7': 'All done' } }));
panel('working: mark failed form', working, withUi({ reveal: ['fail:T7'] }));
panel('working: idle terminal', task({ ...live('idle'), status: 'working' }), withUi({}));
panel('working: session gone but id kept', task({ session_id: 's3', who: 'T7 x', session: { id: 's3', name: 'T7 x', status: 'gone' } }), withUi({}));
panel('working: no terminals list, live session', task({ ...live('working'), started_at: at(10) }), withUi({}));
panel('working: two terminals', task({ ...live('working'), terminals: [
  { id: 's0', name: 'Older', status: 'idle', why: 'Looked at it', at: at(50) }, { id: 's1', name: 'Current', status: 'working', why: 'Worked on the task', at: at(5) }] }), withUi({}));
panel('working: manage note', working, withUi({ notes: { 'manage:T7': { text: 'Detached. It’s back in the queue.', err: false } } }));

// ---- jira
const jiraState = { ...STATE, jira: { enabled: true } };
panel('jira: ticket with url', task({ jira: { key: 'PROJ-12', status: 'In Progress', url: 'https://acme.atlassian.net/browse/PROJ-12' } }), { ...withUi({}), state: jiraState });
panel('jira: ticket without url, jira off', task({ jira: { key: 'PROJ-12', status: null, url: null } }), withUi({}));
panel('jira: being created', task({ jira: { key: null, status: 'Ticket asked for', url: null } }), { ...withUi({}), state: jiraState });
panel('jira: on, no ticket', task({}), { ...withUi({}), state: jiraState });

// ---- PRs
const done = (over = {}) => task({ status: 'done', when: at(30), finished_at: at(30), started_at: at(150), summary: 'Login ceremony is in.', who: 'T7 Sign in with a passkey', ...over });
panel('done: summary folded', done(), withUi({}));
panel('done: summary open, live idle terminal', done({ ...live('idle'), who: 'T7 Sign in with a passkey' }), withUi({ folds: { 'tb.fold.summary': 'open' } }));
panel('done: failed', done({ failed: true, summary: null, latest: 'Couldn’t reproduce.' }), withUi({ folds: { 'tb.fold.summary': 'open' } }));
panel('done: no summary', done({ summary: null, who: null }), withUi({ folds: { 'tb.fold.summary': 'open' } }));
panel('done: requeue note', done(), withUi({ notes: { 'act:T7': { text: 'Back in the queue', err: false } } }));
for (const [label, p] of [
  ['checks pending, review none', pr({ checks: 'pending', review: 'none' })],
  ['checks none, review approved', pr({ checks: 'none', review: 'approved' })],
  ['checks pass, review changes', pr({ checks: 'pass', review: 'changes' })],
  ['checks null, review null', pr({ checks: null, review: null })],
  ['stage checks', pr({ stage: { phase: 'checks', label: 'Watching checks', session: null, stopped: null } })],
  ['stage fix starting', pr({ checks: 'fail', stage: { phase: 'fix', label: 'Fixing checks', session: null, stopped: null } })],
  ['stage fix with terminal', pr({ checks: 'fail', stage: { phase: 'fix', label: 'Fixing checks', session: 's5', stopped: null } })],
  ['stage fix stopped', pr({ checks: 'fail', stage: { phase: 'fix', label: 'Fixing checks', session: 's5', stopped: { asked: false, message: 'Tests still fail on CI only.' } } })],
  ['stage comments asked', pr({ checks: 'pass', review: 'pending', stage: { phase: 'comments', label: 'Answering comments', session: 's5', stopped: { asked: true, message: 'Should I rename the helper as the reviewer asks?' } } })],
  ['stage review', pr({ checks: 'pass', stage: { phase: 'review', label: 'Awaiting reviews', session: null, stopped: null } })],
  ['stage merge', pr({ checks: 'pass', review: 'approved', stage: { phase: 'merge', label: 'Merging', session: 's5', stopped: null } })],
  ['merged', pr({ state: 'MERGED', checks: 'pass', review: 'approved', stage: { phase: 'merged', label: 'Merged', session: null, stopped: null } })],
  ['declined', pr({ state: 'DECLINED', checks: 'fail', review: 'none' })],
  ['no url, title starts with jira key', pr({ url: null, title: 'PROJ-12: Passkey sign-in' })],
  ['no title', pr({ title: null })],
]) {
  const terminals = [{ id: 's5', name: 'PR visitor', status: 'idle', why: 'Worked on the PR', at: at(20) }, { id: 's1', name: 'T7 Sign in with a passkey', status: 'gone', why: 'Worked on the task', at: at(100) }];
  panel(`done PR: ${label}`, done({ pr: p, terminals, jira: { key: 'PROJ-12', status: 'In Review', url: null } }), withUi({}));
}
panel('working with PR open', task({ ...live('working'), pr: pr({ checks: 'fail', review: 'pending' }) }), withUi({}));
panel('done PR stopped, draft', done({ pr: pr({ stage: { phase: 'fix', label: 'Fixing checks', session: 'sX', stopped: { asked: false, message: 'Stuck.' } } }) }), withUi({ drafts: { 'answer:T7': 'Try rebasing' } }));

// ---- attachments
const atts = {
  attachments: [
    { id: 11, kind: 'design', title: 'Login mock', url: 'https://figma.com/file/abc', at: at(40), task: 'T7', goal: null },
    { id: 12, kind: 'results', title: 'bench.txt', url: '~/out/bench.txt', at: at(10), task: 'T7', goal: null },
    { id: 13, kind: 'weird', title: 'Spec', url: 'notes/spec.md', at: null, task: null, goal: null },
  ],
  goal_attachments: [{ id: 21, kind: 'proposal', title: 'Plan', url: 'https://docs.example.com/plan', at: at(600), task: null, goal: 'G1' }],
};
panel('attachments', task({ ...atts, goal: GOAL }), withUi({}));
panel('attachments: menu open (web)', task({ ...atts, goal: GOAL }), withUi({ att_menu: '11' }));
panel('attachments: menu open (path)', task({ ...atts, goal: GOAL }), withUi({ att_menu: '12' }));
panel('attachments: menu open (no link)', task({ ...atts, goal: GOAL }), withUi({ att_menu: '13' }));
panel('attachments: editing', task({ ...atts, goal: GOAL }), withUi({ att_edit: '11' }));
panel('attachments: editing with drafts', task({ ...atts, goal: GOAL }), withUi({ att_edit: '12', drafts: { 'attedit:12:kind': 'doc', 'attedit:12:title': 'Bench' } }));
panel('attachments: note', task({ ...atts }), withUi({ notes: { 'att:tasks:T7': { text: 'Removed', err: false } } }));

// ---- context tab
const ctx = {
  saved_at: at(20), turns: 3, checkpoints: 2,
  where: { branch: 'feature/passkeys', worktree: '/tmp/webapp-wt', last_commit: 'a1b2c3 Add login', uncommitted: 2, conversation: 'c-1' },
  done: ['Endpoint', 'Tests'], next: ['Button'], decisions: ['Keep passwords'], answers: ['Yes, re-check'],
  files: ['src/auth/login.ts', 'README.md'],
};
panel('context: full', task({ context: ctx, found: [{ id: 3, ref: 'B3', title: 'No test for expired challenge' }], goal: GOAL,
  meta: [['Figma', 'https://figma.com/x'], ['Owner', 'Sam']] }), { ...withUi({}), tab: 'context' });
panel('context: not in goal, found', task({ context: { where: { uncommitted: 0 } }, found: [{ id: 3, ref: 'B3', title: 'Gap' }] }), { ...withUi({}), tab: 'context' });
panel('context: not started, no handoff yet', task({ status: 'queued', started_at: null, who: null, handoff: undefined, repo_path: null }), { ...withUi({}), tab: 'context' });
panel('context: handoff loaded', task({ handoff: undefined }), { ...withUi({ handoffs: { T7: 'Fetched handoff text.' } }), tab: 'context' });
panel('context: handoff object', task({ handoff: { text: 'Object handoff.' } }), { ...withUi({}), tab: 'context' });
panel('context: handoff failed', task({ handoff: undefined }), { ...withUi({ handoffs: { T7: 'Couldn’t load the handoff: The board answered 500.' } }), tab: 'context' });
panel('context: uncommitted string, started by who', task({ started_at: null, who: 'T7 x', context: { where: { uncommitted: 'unknown' } } }), { ...withUi({}), tab: 'context' });
panel('context: meta objects', task({ meta: [{ name: 'A', value: 'b' }, { k: 'C', v: 'd' }, ['E'], [null, 5]] }), { ...withUi({}), tab: 'context' });
panel('context: meta being edited', task({ meta: [['Figma', 'x']] }), { ...withUi({ meta_edit: { T7: [['Figma', 'x2'], ['New', '']] }, notes: { 'meta:T7': { text: 'Saved', err: false } } }), tab: 'context' });

// ---- log tab
const log = [
  { at: at(1), who: 'T7 Sign in', kind: 'turn', text: 'Ran the tests.' },
  { at: at(3), who: null, kind: 'status', text: 'Working' },
  { at: at(60 * 26), who: 'Alex', kind: 'answer', text: 'Yes.' },
  { at: at(70), who: 'T7 Sign in', kind: 'question', text: 'Which?' },
  { at: at(80), who: 'T7 Sign in', kind: 'commit', text: 'a1b2 Add' },
  { at: at(90), who: 'Task board', kind: 'checkpoint', text: 'Saved' },
  { at: at(95), who: 'Task board', kind: 'found', text: 'B3' },
  { at: at(96), who: 'Jira', kind: 'jira', text: 'PROJ-12 In Progress' },
  { at: at(97), who: 'Midna', kind: 'midna', text: 'Opened' },
  { at: at(98), who: 'Task board', kind: 'handoff', text: 'Handed off' },
  { at: at(99), who: 'You', kind: 'note', text: 'A note' },
  { at: at(100), who: 'x', kind: 'custom_kind', text: 'Odd' },
];
for (const f of ['all', 'status', 'turn', 'checkpoint', 'question', 'found', 'jira']) panel(`log: ${f}`, task({ log }), { ...withUi({ log_filter: f }), tab: 'log' });
panel('log: empty', task({ log: [] }), { ...withUi({}), tab: 'log' });
panel('log: none of kind', task({ log: [log[0]] }), { ...withUi({ log_filter: 'jira' }), tab: 'log' });

// ---- trail
panel('trail back button', working, { ...withUi({}), from: 'T2,T5' });

// ---- pure helpers
for (const [label, st] of [['off', STATE], ['closed tomorrow', hoursClosed], ['closed today', hoursToday], ['open', { ...STATE, work_hours: { ...STATE.work_hours, on: true, open: true } }],
  ['closed no next', { ...STATE, work_hours: { ...STATE.work_hours, on: true, open: false, next_open: null } }]]) {
  w.set('S.state', st);
  cases.push({ name: `hoursOpenAt ${label}`, input: { fn: 'hoursOpenAt', state: st }, expect: w.run('hoursOpenAt()') });
}
for (const [label, st] of [['up', STATE], ['down', { ...STATE, midna: { up: false, seen_at: at(5) } }], ['unknown', { ...STATE, midna: null }]]) {
  w.set('S.state', st);
  cases.push({ name: `sentNote ${label}`, input: { fn: 'sentNote', state: st }, expect: w.run('sentNote()') });
}
writeGolden('task', cases);
