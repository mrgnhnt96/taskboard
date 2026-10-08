// The Sessions page (sessions.js + the rename helpers in app.js): what the list, a row, a closed
// row, the header, the row menu, the detail pane and the bulk-close confirmation show, and which
// actions each offers, for every state. Checked by `ui::sessions` tests.
import { web, text, acts, writeGolden, NOW } from '../web.mjs';

const T = Date.parse(NOW);
const at = mins => new Date(T - mins * 60000).toISOString();
// Screen-reader-only labels aren't visible text. Line breaks in the text only come from how the
// HTML templates were laid out, so all whitespace is collapsed (the app compares the same way).
const vis = html => text(String(html ?? '').replace(/<label class="sr"[^>]*>[\s\S]*?<\/label>/g, '')).replace(/\s+/g, ' ').trim();
const view = html => ({ text: vis(html), acts: acts(html) });

// ------------------------------------------------------------------ fixtures (docs/API.md shapes)

const row = o => ({
  id: 's', name: 'Terminal', project: null, project_path: null, status: 'idle', task_ref: null, task_title: null,
  last_activity: at(1), seen_at: at(1), can_take: false, branch: null, closing: false, close: 'close', renaming: null, rename_error: null, ...o,
});
const LIVE = [
  row({ id: 's-a', name: 'T2 Sign in with a passkey', project: 'webapp', project_path: '/Users/sam/dev/webapp', status: 'working', task_ref: 'T2', task_title: 'Sign in with a passkey', last_activity: at(3), branch: 'feature/passkeys', close: 'force' }),
  row({ id: 's-b', name: 'T3 Settings page', project: 'webapp', status: 'needs', task_ref: 'T3', task_title: 'Settings page: manage passkeys', last_activity: at(1), close: 'force' }),
  row({ id: 's-c', name: 'api shell', project: 'api', status: 'idle', branch: 'main', last_activity: at(90) }),
  row({ id: 's-d', name: 'api tests', project: 'api', status: 'idle', last_activity: new Date(T - 30000).toISOString() }),
  row({ id: 's-e', name: 'scratch', project: null, status: 'idle', last_activity: at(200), seen_at: at(200) }),
  row({ id: 's-f', name: 'Zeta build', project: 'Zeta', status: 'working', last_activity: at(5), closing: true, close: null }),
  row({ id: 's-g', name: 'Old name', project: 'beta', status: 'idle', renaming: 'New name', task_ref: 'T9', task_title: 'Docs', last_activity: at(10) }),
  row({ id: 's-h', name: 'beta shell', project: 'beta', status: 'idle', rename_error: 'Midna said no', last_activity: at(60 * 26), seen_at: at(60 * 26) }),
  row({ id: 's-i', name: 'odd one', project: 'api', status: 'weird', last_activity: null, seen_at: at(400), close: 'close' }),
];
const CLOSED = [
  { id: 'closed-1', name: 'old one', project: 'webapp', project_path: '/Users/sam/dev/webapp', closed_at: at(120), last_activity: at(125), task_ref: 'T1', task_title: 'Add endpoint', can_reopen: true },
  { id: 'closed-2', name: 'older', project: null, project_path: null, closed_at: at(60 * 72), last_activity: null, task_ref: null, task_title: null, can_reopen: false },
];
const byId = id => LIVE.find(x => x.id === id) || CLOSED.find(x => x.id === id);

const task = o => ({ id: 2, ref: 'T2', title: 'Sign in with a passkey', status: 'working', failed: false, blocked: false, goal: null, ...o });
const detail = o => ({
  id: 's-a', name: 'T2 Sign in with a passkey', project: 'webapp', project_path: '/Users/sam/dev/webapp', status: 'working', status_at: at(12),
  last_activity: at(3), gone_at: null, branch: 'feature/passkeys', dirty: null, diff: { files: 3, added: 40, removed: 7, new: 1 },
  close: 'force', closing: false, renaming: null, rename_error: null, task: null, last_task: null, prompt: null, reply: null, waiting: null,
  stats: { turns: 4, commits: 1, files: 3 }, timeline: [], ...o,
});
const TIMELINE = ['prompt', 'reply', 'turn', 'commit', 'checkpoint', 'found', 'ask', 'wait', 'compact', 'start', 'end', 'take', 'done', 'fail', 'rename', 'close', 'closed', 'close_failed', 'mystery']
  .map((kind, n) => ({ at: at(n * 7), kind, text: `${kind} event`, ...(kind === 'turn' ? { files: 2 } : kind === 'commit' ? { files: 1 } : {}) }));
TIMELINE.push({ at: at(60 * 30), kind: 'reply', text: 'x'.repeat(300) });
TIMELINE.push({ at: at(2), kind: 'prompt', text: 'many   spaces\nand lines' });

// ------------------------------------------------------------------ page state

function page(w, o = {}) {
  w.run(`S.route = { page: 'sessions', id: null, q: {} }; S.rename = null; S.drafts = {}; S.notes = {}; S.busy = new Set();
    S.renameWatch = {}; S.renameFlash = {}; S.state = { midna: { up: true } };
    SS.list = null; SS.listErr = ''; SS.closed = []; SS.closedTotal = 0; SS.detail = null; SS.detailErr = ''; SS.detailFor = null;
    SS.picked = new Set(); SS.confirm = null; SS.bulk = false; SS.q = ''; SS.selecting = false; SS.menu = null; SS.closedOpen = false;
    SS.hideSel = null; SS.collapsed = new Set(); SS.hold = null;`);
  w.set('__o', o);
  w.run(`
    if (__o.list !== undefined) SS.list = __o.list;
    if (__o.closed) { SS.closed = __o.closed; SS.closedTotal = __o.closedTotal || __o.closed.length; }
    if (__o.sel) S.route.q.s = __o.sel;
    if (__o.f) S.route.q.f = __o.f;
    if (__o.back) S.route.q.back = __o.back;
    if (__o.q) SS.q = __o.q;
    if (__o.collapsed) SS.collapsed = new Set(__o.collapsed);
    if (__o.hideSel) SS.hideSel = __o.hideSel;
    if (__o.selecting) SS.selecting = true;
    if (__o.picked) SS.picked = new Set(__o.picked);
    if (__o.closedOpen) SS.closedOpen = true;
    if (__o.detail) { SS.detail = __o.detail; SS.detailFor = __o.detail.id; }
    if (__o.confirm) SS.confirm = __o.confirm;
    if (__o.rename) S.rename = __o.rename;
    if (__o.notes) S.notes = __o.notes;
    if (__o.busy) S.busy = new Set(__o.busy);
    if (__o.hold) SS.hold = { id: __o.hold, t0: Date.now() - 300, timer: 0 };
    if (__o.midnaDown) S.state = { midna: { up: false } };
    if (__o.watch) S.renameWatch = __o.watch;
    if (__o.bulk) SS.bulk = true;
    if (__o.listErr) SS.listErr = __o.listErr;
    if (__o.detailErr) { SS.detailErr = __o.detailErr; SS.detailFor = __o.sel; }`);
}

const w = web();
const cases = [];
const add = (name, input, expect) => cases.push({ name, input, expect });

// Small helpers.
for (const ms of [0, 29000, 30000, 59000, 60000 * 12, 60000 * 59.6, 3600000, 3600000 * 2 + 60000 * 5, 3600000 * 5 + 60000 * 59, 3600000 * 6 + 60000 * 5, 3600000 * 23, 3600000 * 24, 3600000 * 36, 3600000 * 60])
  add(`longAgo ${ms}`, { fn: 'longAgo', ms }, w.call('longAgo', ms));
page(w, { list: LIVE });
for (const s of LIVE) for (const f of ['all', 'needs', 'working', 'idle', 'stale'])
  add(`forFilter ${s.id} ${f}`, { fn: 'forFilter', session: s, f }, w.call('forFilter', s, f));
for (const [q, s] of [['', LIVE[0]], ['PASS', LIVE[0]], ['t3', LIVE[1]], ['main', LIVE[2]], ['nope', LIVE[2]], ['  api  ', LIVE[3]], ['webapp', LIVE[4]]]) {
  w.run(`SS.q = ${JSON.stringify(q)}`);
  add(`ssMatch ${JSON.stringify(q)} ${s.id}`, { fn: 'ssMatch', session: s, q }, w.call('ssMatch', s));
}
w.run(`SS.q = ''`);
add('groupedSessions order', { fn: 'grouped', list: LIVE }, w.run(`groupedSessions(${JSON.stringify(LIVE)}).map(([p, ss]) => [p, ss.map(s => s.id)])`));
add('groupedSessions locale order', { fn: 'grouped', list: [row({ id: 'x1', project: 'beta' }), row({ id: 'x2', project: 'Alpha' }), row({ id: 'x3', project: 'alpha' }), row({ id: 'x4', project: 'Beta' })] },
  w.run(`groupedSessions(${JSON.stringify([row({ id: 'x1', project: 'beta' }), row({ id: 'x2', project: 'Alpha' }), row({ id: 'x3', project: 'alpha' }), row({ id: 'x4', project: 'Beta' })])}).map(([p, ss]) => [p, ss.map(s => s.id)])`));

for (const d of [
  detail({}), detail({ status: 'working', status_at: null, prompt: { text: 'go', at: at(4) } }), detail({ status: 'working', status_at: null }),
  detail({ status: 'needs', status_at: null, waiting: { text: 'ok?', at: at(9) } }), detail({ status: 'idle', last_activity: at(65) }),
  detail({ status: 'idle', last_activity: null, seen_at: at(2) }), detail({ status: 'gone', gone_at: at(30) }), detail({ status: 'gone', gone_at: null, last_activity: at(90) }),
  detail({ status: 'idle', gone_at: at(5) }),
]) add(`statusLine ${d.status} ${d.status_at || d.gone_at || ''}`, { fn: 'statusLine', detail: d }, w.call('statusLine', d));
for (const d of [detail({}), detail({ diff: { files: 0, added: 0, removed: 0, new: 0 } }), detail({ diff: { files: 1, added: 2, removed: 0, new: 0 } }), detail({ diff: null, dirty: 2 }), detail({ diff: null, dirty: 1 }), detail({ diff: null, dirty: 0 }), detail({ diff: null, dirty: null })])
  add(`diffView ${JSON.stringify(d.diff)} ${d.dirty}`, { fn: 'diffView', detail: d }, { text: vis(w.call('diffView', d)), title: (String(w.call('diffView', d)).match(/title="([^"]*)"/) || [, null])[1] });
for (const d of [detail({ close: 'force', prompt: { text: 'Refactor   the\nauth module ' + 'y'.repeat(150), at: at(2) }, task: task({}) }), detail({ close: 'force' }), detail({ close: 'close', status: 'idle', task: task({}) }), detail({ close: 'close', status: 'idle' })])
  add(`closeCopy ${d.close} ${!!d.prompt} ${!!d.task}`, { fn: 'closeCopy', detail: d }, w.call('closeCopy', d));

// nameView in its states (the Sessions page passes '' on rows, ' · double-click to rename' on a live detail title).
for (const [label, s, hint, watch] of [
  ['plain row', LIVE[2], '', null], ['plain detail', LIVE[2], ' · double-click to rename', null], ['renaming', LIVE[6], '', null],
  ['rename error', LIVE[7], ' · double-click to rename', null], ['no name', row({ id: 's-z', name: '' }), '', null],
  ['flash ok', row({ id: 's-c', name: 'api shell' }), '', { 's-c': 'api shell' }],
  ['flash bad error', row({ id: 's-c', name: 'api shell', rename_error: 'busy' }), '', { 's-c': 'new api' }],
  ['flash bad kept', row({ id: 's-c', name: 'api shell' }), ' · double-click to rename', { 's-c': 'new api' }],
  ['watch while renaming', row({ id: 's-c', name: 'api shell', renaming: 'new api' }), '', { 's-c': 'new api' }],
]) {
  page(w, { watch: watch || {} });
  const nv = w.call('nameView', s, hint);
  add(`nameView ${label}`, { fn: 'nameView', session: s, hint, watch }, { text: nv.text, cls: nv.cls.trim(), title: (nv.attrs.match(/title="([^"]*)"/) || [, null])[1].replace(/&amp;/g, '&') });
}

// Rows.
for (const s of LIVE) for (const [label, o] of [['', {}], [' selected', { sel: s.id }], [' selecting', { selecting: true }], [' picked', { selecting: true, picked: [s.id] }]]) {
  page(w, { list: LIVE, ...o });
  add(`ssRow ${s.id}${label}`, { fn: 'ssRow', session: s, sel: o.sel || null, selecting: !!o.selecting, picked: o.picked || [] }, view(w.call('ssRow', s)));
}
for (const s of CLOSED) for (const [label, o] of [['', {}], [' selected', { sel: s.id }], [' selecting', { selecting: true }]]) {
  page(w, { list: LIVE, closed: CLOSED, ...o });
  add(`ssClosedRow ${s.id}${label}`, { fn: 'ssClosedRow', session: s, sel: o.sel || null, selecting: !!o.selecting }, view(w.call('ssClosedRow', s)));
}

// Header.
for (const [label, o] of [['loading', { list: null }], ['loaded', { list: LIVE }], ['one', { list: [LIVE[0]] }], ['filter stale', { list: LIVE, f: 'stale' }],
  ['back to task', { list: LIVE, back: '#/?task=T12' }], ['back elsewhere', { list: LIVE, back: '#/goals/G1' }], ['bad back', { list: LIVE, back: 'http://x' }]]) {
  page(w, o);
  const h = w.call('ssHeader');
  add(`ssHeader ${label}`, { fn: 'ssHeader', ...o }, {
    back: vis((h.match(/<a class="back"[\s\S]*?<\/a>/) || [''])[0]),
    text: vis(h.replace(/<a class="back"[\s\S]*?<\/a>/, '')),
    pressed: (h.match(/data-arg="(\w+)" aria-pressed="true"/) || [, null])[1],
  });
}

// Menu items.
for (const s of [...LIVE, ...CLOSED]) for (const [label, o] of [['', {}], [' picked', { selecting: true, picked: [s.id] }]]) {
  page(w, { list: LIVE, closed: CLOSED, ...o });
  add(`ssMenuItems ${s.id}${label}`, { fn: 'ssMenuItems', session: s, selecting: !!o.selecting, picked: o.picked || [] }, w.call('ssMenuItems', s));
}

// The list.
const lists = [
  ['loading', { list: null }],
  ['empty', { list: [] }],
  ['empty filtered', { list: [LIVE[2]], f: 'needs' }],
  ['default', { list: LIVE, closed: CLOSED, closedTotal: 7 }],
  ['default with selection', { list: LIVE, closed: CLOSED, sel: 's-c' }],
  ['filter needs', { list: LIVE, closed: CLOSED, f: 'needs' }],
  ['filter stale', { list: LIVE, closed: CLOSED, f: 'stale' }],
  ['filter idle', { list: LIVE, closed: CLOSED, f: 'idle' }],
  ['search', { list: LIVE, closed: CLOSED, q: 'Pass' }],
  ['search closed', { list: LIVE, closed: CLOSED, closedTotal: 7, q: 'old', closedOpen: true }],
  ['search none', { list: LIVE, closed: CLOSED, q: 'zzz' }],
  ['search spaces', { list: LIVE, closed: CLOSED, q: '  Zeta ' }],
  ['collapsed api', { list: LIVE, closed: CLOSED, collapsed: ['api'] }],
  ['collapsed api holds selection', { list: LIVE, closed: CLOSED, collapsed: ['api'], sel: 's-c' }],
  ['collapsed api hidden selection', { list: LIVE, closed: CLOSED, collapsed: ['api'], sel: 's-c', hideSel: 's-c' }],
  ['collapsed webapp needs', { list: LIVE, collapsed: ['webapp'] }],
  ['collapsed but searching', { list: LIVE, collapsed: ['webapp'], q: 'T3' }],
  ['closed open', { list: LIVE, closed: CLOSED, closedTotal: 2, closedOpen: true }],
  ['closed holds selection', { list: LIVE, closed: CLOSED, sel: 'closed-2' }],
  ['closed hidden selection', { list: LIVE, closed: CLOSED, sel: 'closed-2', hideSel: 'closed-2' }],
  ['selecting none picked', { list: LIVE, closed: CLOSED, selecting: true }],
  ['selecting picked', { list: LIVE, closed: CLOSED, selecting: true, picked: ['s-e', 's-c'] }],
  ['selecting one picked', { list: LIVE, selecting: true, picked: ['s-d'] }],
  ['one stale', { list: LIVE.filter(s => s.id !== 's-e' && s.id !== 's-h') }],
  ['stale not all filter', { list: LIVE, f: 'idle' }],
  ['list failed', { list: null, listErr: 'Can’t reach the task board server.' }],
];
for (const [label, o] of lists) {
  page(w, o);
  add(`ssList ${label}`, { fn: 'ssList', ...o }, view(w.call('ssList')));
}

// The detail pane.
const live = (o = {}) => ({ list: LIVE, closed: CLOSED, ...o });
const details = [
  ['nothing selected', live()],
  ['loading', live({ sel: 's-a' })],
  ['working full', live({ sel: 's-a', detail: detail({ task: task({ goal: { id: 1, ref: 'G1', name: 'Sign-in with passkeys' } }), prompt: { text: 'Add the   login\nbutton', at: at(4) }, reply: { text: 'Done. The button is in.', at: at(3) }, timeline: TIMELINE }) })],
  ['working prompt only', live({ sel: 's-a', detail: detail({ prompt: { text: 'go', at: at(60 * 20) } }) })],
  ['needs waiting', live({ sel: 's-b', detail: detail({ id: 's-b', name: 'T3 Settings page', status: 'needs', status_at: at(2), task: task({ ref: 'T3', title: 'Settings page', status: 'needs' }), waiting: { text: 'Should I keep the old flow?', at: at(2) }, reply: { text: 'ignored', at: at(5) } }) })],
  ['idle no task last task dirty', live({ sel: 's-c', detail: detail({ id: 's-c', name: 'api shell', project: 'api', project_path: '/Users/alex/code/api', status: 'idle', status_at: at(90), last_activity: at(90), close: 'close', branch: 'main', diff: null, dirty: 2, last_task: { ref: 'T7', title: 'Old work' }, reply: { text: 'All done', at: at(91) } }) })],
  ['idle no changes no last', live({ sel: 's-d', detail: detail({ id: 's-d', name: 'api tests', project: 'api', project_path: null, status: 'idle', last_activity: null, close: 'close', branch: null, diff: { files: 0, added: 0, removed: 0, new: 0 }, stats: { turns: 0, commits: 0, files: 0 } }) })],
  ['no path or project', live({ sel: 's-e', detail: detail({ id: 's-e', name: 'scratch', project: null, project_path: null, status: 'idle', close: 'close', diff: null, dirty: null }) })],
  ['gone can reopen', live({ sel: 'closed-1', detail: detail({ id: 'closed-1', name: 'old one', status: 'gone', gone_at: at(120), close: null, diff: null, last_task: { ref: 'T1', title: 'Add endpoint' } }) })],
  ['gone cannot reopen', live({ sel: 'closed-2', detail: detail({ id: 'closed-2', name: 'older', status: 'gone', gone_at: at(60 * 72), close: null, diff: null }) })],
  ['gone not listed', live({ sel: 'ghost', detail: detail({ id: 'ghost', name: 'ghost', status: 'gone', gone_at: at(10), close: null, diff: null, task: task({ status: 'done' }) }) })],
  ['closing', live({ sel: 's-f', detail: detail({ id: 's-f', name: 'Zeta build', closing: true, close: null }) })],
  ['confirm force busy prompt', live({ sel: 's-a', confirm: 's-a', detail: detail({ prompt: { text: 'Refactor the auth module', at: at(2) }, task: task({}) }) })],
  ['confirm close no task', live({ sel: 's-c', confirm: 's-c', detail: detail({ id: 's-c', name: 'api shell', status: 'idle', close: 'close' }) })],
  ['confirm while closing', live({ sel: 's-a', confirm: 's-a', detail: detail({ closing: true }) })],
  ['renaming input', live({ sel: 's-a', rename: 's-a', detail: detail({}) })],
  ['renaming pending', live({ sel: 's-g', detail: detail({ id: 's-g', name: 'Old name', renaming: 'New name', status: 'idle', close: 'close' }) })],
  ['rename error', live({ sel: 's-h', detail: detail({ id: 's-h', name: 'beta shell', rename_error: 'Midna said no', status: 'idle', close: 'close' }) })],
  ['note ok', live({ sel: 's-a', notes: { 'ss:s-a': { text: 'Sent to Midna', err: false } }, detail: detail({}) })],
  ['note err', live({ sel: 's-a', notes: { 'ss:s-a': { text: 'Midna isn’t answering.', err: true } }, detail: detail({}) })],
  ['holding', live({ sel: 's-a', hold: 's-a', detail: detail({}) })],
  ['hold busy', live({ sel: 's-a', busy: ['ss-hold:force:s-a'], detail: detail({}) })],
  ['focus busy', live({ sel: 's-c', busy: ['ss-focus::s-c'], detail: detail({ id: 's-c', name: 'api shell', status: 'idle', close: 'close' }) })],
  ['reopen busy', live({ sel: 'closed-1', busy: ['ss-reopen::closed-1'], detail: detail({ id: 'closed-1', name: 'old one', status: 'gone', gone_at: at(120), close: null, diff: null }) })],
  ['close busy', live({ sel: 's-c', confirm: 's-c', busy: ['ss-close::s-c'], detail: detail({ id: 's-c', name: 'api shell', status: 'idle', close: 'close' }) })],
  ['close no close', live({ sel: 's-a', detail: detail({ close: null }) })],
  ['unknown status', live({ sel: 's-i', detail: detail({ id: 's-i', name: 'odd one', status: 'weird', status_at: null, last_activity: at(70), close: 'close', reply: { text: 'r', at: at(70) } }) })],
];
for (const [st, extra] of [['done', {}], ['done', { failed: true }], ['queued', { blocked: true }], ['queued', {}], ['queued', { starting: true }], ['planned', {}], ['needs', {}], ['mystery', {}]])
  details.push([`task pill ${st} ${JSON.stringify(extra)}`, live({ sel: 's-a', detail: detail({ task: task({ status: st, ...extra }) }) })]);
details.push(['detail failed', live({ sel: 's-a', detailErr: 'There’s nothing at that address.' })]);
for (const [label, o] of details) {
  page(w, o);
  add(`ssDetail ${label}`, { fn: 'ssDetail', ...o }, view(w.call('ssDetail')));
}

// Bulk close confirmation.
for (const [label, o] of [['one', { list: LIVE, selecting: true, picked: ['s-c'], bulk: true }], ['two in pick order', { list: LIVE, selecting: true, picked: ['s-e', 's-c', 's-d'], bulk: true }], ['hidden', { list: LIVE, selecting: true, picked: ['s-c'] }]]) {
  page(w, o);
  add(`ssBulkConfirm ${label}`, { fn: 'ssBulkConfirm', ...o }, view(w.call('ssBulkConfirm')));
}

// What a click on "Show in Midna" says when it works.
for (const [label, o] of [['midna up', {}], ['midna down', { midnaDown: true }]]) {
  page(w, o);
  add(`sentNote ${label}`, { fn: 'sentNote', ...o }, w.call('sentNote'));
}

writeGolden('sessions', cases);
