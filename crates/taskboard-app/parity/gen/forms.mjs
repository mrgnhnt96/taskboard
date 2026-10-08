// Forms (pages.js taskFormHtml/goalFormHtml/newIssueHtml + submits, app.js pickers and drafts).
//
// Every case runs the web board's own code: the open* functions (what a form starts with), the
// submit* functions (validation message, or the exact POST path and body, captured by stubbing
// post()), pickerItems (picker lists and search), saveDraft (what is kept between launches) and
// the form HTML (what the reader sees: labels, help, pressed choices, labels on picker buttons).
import { web, writeGolden, text } from '../web.mjs';

const PROJECTS = [
  { name: 'webapp', path: '/Users/alex/code/webapp' },
  { name: 'api', path: '/Users/alex/code/api' },
  { name: 'Zeta', path: null },
  { name: 'my_tool', path: '/opt/tools/my_tool' },
];
const SESSIONS = [
  { id: 's1', name: 'T2 Sign in', project: 'webapp', status: 'working', can_take: false },
  { id: 's2', name: 'webapp shell', project: 'webapp', status: 'idle', can_take: true },
  { id: 's3', name: 'api shell', project: 'api', status: 'idle', can_take: true },
  { id: 's4', name: 'webapp spare', project: 'webapp', status: 'idle', can_take: true },
];
const GOALS = [
  { id: 1, ref: 'G1', name: 'Sign-in with passkeys', project: 'webapp', epic_key: 'WEB-12', auto_close: false, run_in_order: true, max_terminals: 3, tldr: 'Add passkeys.', outcome: 'Users sign in with a passkey.', archived: false },
  { id: 2, ref: 'G2', name: 'api cleanup', project: 'api', epic_key: null, auto_close: true, run_in_order: false, max_terminals: 1, tldr: '', outcome: '', archived: false },
  { id: 3, ref: 'G3', name: 'Old mobile work', project: 'mobile', archived: true },
  { id: 4, ref: 'G4', name: 'billing v2', project: 'billing', epic_key: 'BILL-7', archived: false, run_in_order: 0, auto_close: 0, max_terminals: 8 },
  { id: 5, ref: 'G5', name: 'Webhooks', project: 'api', epic_key: null, archived: false },
];
const state = (o = {}) => ({ projects: PROJECTS, sessions: SESSIONS, jira: { enabled: !!o.jira }, ...o.state });

/** Everything a case's page is set up with, written into the golden so the Rust side needs no fixtures. */
function resolve(o = {}) {
  return {
    state: state(o),
    goals: o.goals ?? GOALS,
    filter: { project: 'all', goal: 'all', done: '24h', ...o.filter },
    page: o.page || 'board',
    blFilter: { project: 'all', goal: 'all', kind: 'all', state: 'open', sort: 'new', ...o.blFilter },
    store: o.store || {},
  };
}
/** A fresh page with the board, filters, route and saved drafts set up. */
function page(o = {}) {
  const b = resolve(o);
  const w = web();
  w.set('S.state', b.state);
  w.set('S.goalsAll', b.goals);
  w.set('S.filter', b.filter);
  w.set('S.route', { page: b.page, id: null, q: {} });
  w.set('P.blFilter', b.blFilter);
  for (const [k, v] of Object.entries(b.store)) w.run(`localStorage.setItem(${JSON.stringify(k)}, ${JSON.stringify(JSON.stringify(v))})`);
  return w;
}
const modal = (w, keys) => {
  const m = JSON.parse(w.run('JSON.stringify(S.modal)'));
  const out = {};
  for (const k of keys) out[k] = m[k] === undefined ? null : m[k];
  return out;
};
const TASK_KEYS = ['title', 'detail', 'project', 'goal', 'planned', 'priority', 'pickup', 'session', 'jiraMode', 'jiraKey', 'auto_close', 'restored'];
const GOAL_KEYS = ['id', 'name', 'tldr', 'outcome', 'project', 'epicMode', 'epicKey', 'run_in_order', 'max_terminals', 'auto_close', 'restored'];
const ISSUE_KEYS = ['title', 'kindV', 'goal', 'project', 'said', 'detail'];

/** Run a submit: the validation message, or the POST it sends (captured at fetch, never answered). */
function submit(w, fn, m) {
  w.set('S.modal', m);
  // The wire request: api() JSON-encodes the body for fetch, so this is exactly what's sent.
  w.run(`globalThis.__cap = null; globalThis.fetch = (url, o) => { globalThis.__cap = { path: url.slice(API.length), body: JSON.parse(o.body) }; return new Promise(() => {}); }`);
  w.run(`${fn}()`);
  const cap = JSON.parse(w.run('JSON.stringify(globalThis.__cap)'));
  const err = w.run('S.modal && S.modal.err') || '';
  return cap ? { err: '', path: cap.path.replace(/^\//, ''), body: cap.body } : { err, path: null, body: null };
}

/** What a form shows, read off its HTML. */
function view(html) {
  const all = (re) => [...html.matchAll(re)].map(x => text(x[1]));
  return {
    title: text((html.match(/<h2>([\s\S]*?)<\/h2>/) || [])[1]),
    labels: all(/<label for="[^"]*">([\s\S]*?)<\/label>/g),
    legends: all(/<legend>([\s\S]*?)<\/legend>/g),
    checks: [...html.matchAll(/<label class="check"[^>]*><input type="checkbox" data-m="(\w+)"[^>]*?( checked)?>([\s\S]*?)<\/label>/g)].map(x => ({ field: x[1], on: !!x[2], label: text(x[3]) })),
    helps: all(/<span class="help">([\s\S]*?)<\/span>/g),
    pickers: all(/<span class="picker-val">([\s\S]*?)<\/span>/g),
    pressed: [...html.matchAll(/data-act="m-set" data-field="(\w+)" data-arg="(\w+)" aria-pressed="true"/g)].map(x => `${x[1]}=${x[2]}`),
    disabled: [...html.matchAll(/data-field="(\w+)" data-arg="(\w+)" aria-pressed="\w+" disabled/g)].map(x => `${x[1]}=${x[2]}`),
    options: [...html.matchAll(/<select id="nt-session"[^>]*>([\s\S]*?)<\/select>/g)].flatMap(x => [...x[1].matchAll(/<option value="([^"]*)"[^>]*>([\s\S]*?)<\/option>/g)].map(o => [o[1], text(o[2])])),
    inputs: [...html.matchAll(/<input id="(ng-epic|nt-jira)"[^>]*placeholder="([^"]*)"/g)].map(x => `${x[1]}:${x[2]}`),
    draft: (html.match(/<p class="prefill draft-note">([\s\S]*?)<button/) || [null, null])[1] && text(html.match(/<p class="prefill draft-note">([\s\S]*?)<button/)[1]),
    err: (html.match(/<span class="ferr note" role="alert">([\s\S]*?)<\/span>/) || [null, null])[1],
    submit: text((html.match(/<button type="submit"[^>]*>([\s\S]*?)<\/button>/) || [])[1]),
  };
}

const cases = [];
const add = (name, input, expect) => {
  const { setup, ...rest } = input;
  cases.push({ name, input: { ...rest, board: resolve(setup || {}) }, expect });
};

// ---------------------------------------------------------------- what a form starts with
const opens = [
  ['task: plain', {}, {}],
  ['task: project filter', {}, { filter: { project: 'api' } }],
  ['task: goal filter', {}, { filter: { project: 'webapp', goal: 'G1' } }],
  ['task: add to goal (planned)', { goal: 'G1', planned: true }, {}],
  ['task: add to goal with auto_close 0', { goal: 'G4', planned: true }, {}],
  ['task: unknown goal ref', { goal: 'G99' }, {}],
  ['task: draft restored', {}, { store: { 'tb.draft.task': { title: 'Draft title', detail: 'x', project: 'api', goal: 'G2', priority: 'high', pickup: 'manual', session: '', jiraMode: 'none', jiraKey: '', auto_close: false } } }],
  ['task: draft without words is ignored', {}, { store: { 'tb.draft.task': { title: '  ', detail: '', project: 'api' } } }],
  ['task: draft not used when adding to a goal', { goal: 'G1', planned: true }, { store: { 'tb.draft.task': { title: 'Draft title', project: 'api' } } }],
  ['task: no projects anywhere', {}, { goals: [], state: { projects: [] } }],
];
for (const [name, o, setup] of opens) {
  const w = page(setup);
  w.run(`openTaskForm(${JSON.stringify(o)})`);
  add(name, { fn: 'openTask', o, setup }, modal(w, TASK_KEYS));
}
const gopens = [
  ['goal: new', null, {}],
  ['goal: new with project filter', null, { filter: { project: 'api' } }],
  ['goal: edit', 'G1', {}],
  ['goal: edit with 0/0/8 settings', 'G4', {}],
  ['goal: edit with missing settings', 'G5', {}],
  ['goal: new restores draft', null, { store: { 'tb.draft.goal': { name: 'Draft goal', tldr: '', outcome: '', project: 'webapp', epicMode: 'none', epicKey: '', run_in_order: false, max_terminals: '4', auto_close: true } } }],
  ['goal: edit ignores draft', 'G2', { store: { 'tb.draft.goal': { name: 'Draft goal', project: 'webapp' } } }],
];
for (const [name, g, setup] of gopens) {
  const w = page(setup);
  w.run(g ? `openGoalForm(goalByRef(${JSON.stringify(g)}))` : 'openGoalForm(null)');
  add(name, { fn: 'openGoal', goal: g, setup }, modal(w, GOAL_KEYS));
}
const iopens = [
  ['issue: board', null, {}],
  ['issue: board with project filter', null, { filter: { project: 'api' } }],
  ['issue: from a goal', 'G1', {}],
  ['issue: backlog page filtered to a goal', null, { page: 'backlog', blFilter: { goal: 'G2', project: 'all' } }],
  ['issue: backlog page filtered to "none"', null, { page: 'backlog', blFilter: { goal: 'none', project: 'webapp' } }],
  ['issue: backlog page project filter', null, { page: 'backlog', blFilter: { project: 'my_tool' }, filter: { project: 'api' } }],
  ['issue: board ignores backlog filter', null, { page: 'board', blFilter: { goal: 'G2', project: 'api' } }],
];
for (const [name, g, setup] of iopens) {
  const w = page(setup);
  w.run(`openNewIssue(${JSON.stringify(g)})`);
  add(name, { fn: 'openIssue', goal: g, setup }, modal(w, ISSUE_KEYS));
}

// ---------------------------------------------------------------- submits
const T = { kind: 'task', id: null, title: 'Fix login', detail: '  Steps here  ', project: 'webapp', goal: '', planned: false, priority: 'normal', pickup: 'queue', session: '', jiraMode: 'none', jiraKey: '', auto_close: true, err: '', busy: false };
const tsubs = [
  ['task: plain queue', {}, {}],
  ['task: no title', { title: '   ' }, {}],
  ['task: no project', { project: '' }, {}],
  ['task: high, new terminal, no auto close', { priority: 'high', pickup: 'new', auto_close: false }, {}],
  ['task: manual', { pickup: 'manual' }, {}],
  ['task: attach without terminal', { pickup: 'attach' }, {}],
  ['task: attach with terminal', { pickup: 'attach', session: 's2' }, {}],
  ['task: attach but no idle terminal in project falls back to queue', { pickup: 'attach', project: 'Zeta' }, {}],
  ['task: in a goal, queued', { goal: 'G1' }, {}],
  ['task: in a goal, planned', { goal: 'G1', planned: true, pickup: 'attach' }, {}],
  ['task: planned without goal is not planned', { planned: true }, {}],
  ['task: jira off ignores jira fields', { jiraMode: 'link', jiraKey: 'bad' }, {}],
  ['task: jira none', { jiraMode: 'none' }, { jira: true }],
  ['task: jira create', { jiraMode: 'create' }, { jira: true }],
  ['task: jira link good key (lowercase, spaces)', { jiraMode: 'link', jiraKey: ' web-123 ' }, { jira: true }],
  ['task: jira link bad key', { jiraMode: 'link', jiraKey: 'WEB123' }, { jira: true }],
  ['task: jira link key with dash in project', { jiraMode: 'link', jiraKey: 'A-B-1' }, { jira: true }],
  ['task: jira link key with underscore and digits', { jiraMode: 'link', jiraKey: 'A_2B-77' }, { jira: true }],
  ['task: jira link key starting with digit', { jiraMode: 'link', jiraKey: '2A-7' }, { jira: true }],
];
for (const [name, m, setup] of tsubs) add(name, { fn: 'submitTask', modal: { ...T, ...m }, setup }, submit(page(setup), 'submitTask', { ...T, ...m }));
const G = { kind: 'goal', id: null, name: 'Passkeys', tldr: '  Why  ', outcome: ' Done when ', project: 'webapp', epicMode: 'none', epicKey: '', run_in_order: true, max_terminals: 2, auto_close: true, err: '', busy: false };
const gsubs = [
  ['goal: new plain', {}, {}],
  ['goal: no name', { name: ' ' }, {}],
  ['goal: no project', { project: '' }, {}],
  ['goal: new, settings off, max as text', { run_in_order: false, auto_close: false, max_terminals: '4' }, {}],
  ['goal: new, max not a number', { max_terminals: 'x' }, {}],
  ['goal: new jira none', {}, { jira: true }],
  ['goal: new jira create', { epicMode: 'create' }, { jira: true }],
  ['goal: new jira link', { epicMode: 'link', epicKey: 'web-9' }, { jira: true }],
  ['goal: new jira link bad', { epicMode: 'link', epicKey: 'nope' }, { jira: true }],
  ['goal: new jira off ignores link', { epicMode: 'link', epicKey: 'nope' }, {}],
  ['goal: edit', { id: 'G1', epicMode: 'keep', epicKey: 'WEB-12' }, {}],
  ['goal: edit with jira, key kept', { id: 'G1', epicMode: 'keep', epicKey: 'web-12' }, { jira: true }],
  ['goal: edit with jira, key cleared', { id: 'G1', epicMode: 'keep', epicKey: '  ' }, { jira: true }],
  ['goal: edit with jira, bad key', { id: 'G1', epicMode: 'keep', epicKey: 'bad' }, { jira: true }],
];
for (const [name, m, setup] of gsubs) add(name, { fn: 'submitGoal', modal: { ...G, ...m }, setup }, submit(page(setup), 'submitGoal', { ...G, ...m }));
const I = { kind: 'issue', title: 'Flaky test', kindV: 'bug', goal: '', project: 'webapp', said: '', detail: '', err: '', busy: false };
const isubs = [
  ['issue: plain', {}],
  ['issue: no title', { title: '' }],
  ['issue: no project', { project: '' }],
  ['issue: gap in goal with said and detail', { kindV: 'gap', goal: 'G1', said: ' It flakes ', detail: ' On CI ' }],
  ['issue: follow-up, blank said', { kindV: 'follow', said: '   ' }],
  ['issue: clean-up', { kindV: 'clean' }],
];
for (const [name, m] of isubs) add(name, { fn: 'submitIssue', modal: { ...I, ...m } }, submit(page({}), 'submitNewIssue', { ...I, ...m }));

// ---------------------------------------------------------------- pickers
const pickers = [
  ['project picker', { kind: 'project', value: 'webapp', specials: [], proj: '', q: '' }, {}],
  ['project picker, current not known', { kind: 'project', value: 'legacy', specials: [], proj: '', q: '' }, {}],
  ['project picker with All projects', { kind: 'project', value: 'all', specials: [['all', 'All projects']], proj: '', q: '' }, {}],
  ['project search "ap"', { kind: 'project', value: 'webapp', specials: [['all', 'All projects']], proj: '', q: 'ap' }, {}],
  ['project search "tool" (word start)', { kind: 'project', value: '', specials: [], proj: '', q: 'tool' }, {}],
  ['project search ignores paths', { kind: 'project', value: '', specials: [], proj: '', q: 'code' }, {}],
  ['project search no match', { kind: 'project', value: '', specials: [], proj: '', q: 'zzz' }, {}],
  ['goal picker', { kind: 'goal', value: 'G2', specials: [['', 'Not in a goal']], proj: '', q: '' }, {}],
  ['goal picker for a project', { kind: 'goal', value: '', specials: [['', 'Not in a goal']], proj: 'api', q: '' }, {}],
  ['goal search by epic', { kind: 'goal', value: '', specials: [['', 'Not in a goal']], proj: '', q: 'bill-' }, {}],
  ['goal search by project', { kind: 'goal', value: '', specials: [], proj: '', q: 'api' }, {}],
  ['goal search ranks', { kind: 'goal', value: '', specials: [], proj: '', q: 'w' }, {}],
  ['goal picker, no goals', { kind: 'goal', value: '', specials: [['', 'Not in a goal']], proj: '', q: '' }, { goals: [] }],
];
for (const [name, p, setup] of pickers) {
  const w = page(setup);
  w.set('S.picker', { ...p, target: 'modal:x', active: 0 });
  const items = JSON.parse(w.run('JSON.stringify(pickerItems())'));
  const active = Math.max(0, items.findIndex(it => it.v === p.value));
  const P = JSON.parse(w.run('JSON.stringify(PICKERS)'))[p.kind];
  const empty = items.length ? null : `${P.none} “${p.q.trim()}”.`;
  add(name, { fn: 'picker', picker: p, setup }, {
    items: items.map(it => ({ v: it.v, name: it.name, sub: it.sub || '', special: !!it.special, current: it.v === p.value })),
    active, empty, search: P.search,
  });
}
// The picker button's label.
const buttons = [
  ['project button', 'project', 'api', {}], ['project button empty', 'project', '', {}],
  ['goal button', 'goal', 'G1', { specials: [['', 'Not in a goal']] }], ['goal button none', 'goal', '', { specials: [['', 'Not in a goal']] }],
  ['goal button unknown', 'goal', 'G99', { specials: [['', 'Not in a goal']] }], ['goal button archived', 'goal', 'G3', { specials: [['', 'Not in a goal']] }],
];
for (const [name, kind, value, o] of buttons) {
  const w = page({});
  const html = w.call('pickerButton', kind, 'x', value, { ...o, target: 'modal:x' });
  add(name, { fn: 'pickerButton', kind, value }, text((html.match(/<span class="picker-val">([\s\S]*?)<\/span>/) || [])[1]));
}

// ---------------------------------------------------------------- what is saved as a draft
const drafts = [
  ['task draft', { ...T, title: 'Half typed', goal: 'G1', planned: true }],
  ['task draft without words removes it', { ...T, title: ' ', detail: '' }],
  ['task draft while busy is not written', { ...T, title: 'Busy one', busy: true }],
  ['goal draft', { ...G, name: '', tldr: 'Just a tldr', max_terminals: '3' }],
  ['edit goal is never a draft', { ...G, id: 'G1' }],
  ['issue form is never a draft', { ...I }],
];
for (const [name, m] of drafts) {
  const setup = { store: { 'tb.draft.task': { title: 'old task' }, 'tb.draft.goal': { name: 'old goal' } } };
  const w = page(setup);
  w.set('S.modal', m);
  w.run('saveDraft()');
  const read = k => { const v = w.run(`localStorage.getItem('tb.draft.${k}')`); return v == null ? null : JSON.parse(v); };
  add(name, { fn: 'saveDraft', modal: m, setup }, { task: read('task'), goal: read('goal') });
}

// ---------------------------------------------------------------- what the forms show
const tviews = [
  ['task view: plain', {}, {}],
  ['task view: goal chosen', { goal: 'G1' }, {}],
  ['task view: planned', { goal: 'G1', planned: true }, {}],
  ['task view: attach in webapp', { pickup: 'attach', session: 's4' }, {}],
  ['task view: no idle terminal (attach shown as queue)', { pickup: 'attach', project: 'Zeta' }, {}],
  ['task view: new terminal', { pickup: 'new' }, {}],
  ['task view: manual', { pickup: 'manual' }, {}],
  ['task view: jira on', { priority: 'high' }, { jira: true }],
  ['task view: jira link', { jiraMode: 'link' }, { jira: true }],
  ['task view: restored draft with error, busy', { restored: true, err: 'Give the task a title.', busy: true }, {}],
  ['task view: unknown goal', { goal: 'G99' }, {}],
  ['task view: no project', { project: '' }, {}],
];
for (const [name, m, setup] of tviews) add(name, { fn: 'taskView', modal: { ...T, ...m }, setup }, view(page(setup).call('taskFormHtml', { ...T, ...m })));
const gviews = [
  ['goal view: new', {}, {}],
  ['goal view: new jira', {}, { jira: true }],
  ['goal view: new jira create', { epicMode: 'create' }, { jira: true }],
  ['goal view: new jira link', { epicMode: 'link' }, { jira: true }],
  ['goal view: edit', { id: 'G1', epicMode: 'keep' }, {}],
  ['goal view: edit jira', { id: 'G1', epicMode: 'keep', epicKey: 'WEB-12' }, { jira: true }],
  ['goal view: restored, busy edit', { restored: true, busy: true }, {}],
  ['goal view: busy edit', { id: 'G1', busy: true }, {}],
  ['goal view: settings off', { run_in_order: false, auto_close: false, max_terminals: 1 }, {}],
];
for (const [name, m, setup] of gviews) add(name, { fn: 'goalView', modal: { ...G, ...m }, setup }, view(page(setup).call('goalFormHtml', { ...G, ...m })));
const iviews = [
  ['issue view: plain', {}],
  ['issue view: goal and gap, error', { goal: 'G2', kindV: 'gap', err: 'Pick a project.' }],
  ['issue view: busy', { busy: true }],
];
for (const [name, m] of iviews) add(name, { fn: 'issueView', modal: { ...I, ...m } }, view(page({}).call('newIssueHtml', { ...I, ...m })));

// ---------------------------------------------------------------- localeCompare (pickers and project lists sort with it)
const sorts = [
  ['webapp', 'api', 'Zeta', 'my_tool', 'billing', 'API', 'app10', 'app2', 'my-tool', 'my tool', 'Ápi', 'a', 'B', 'b', 'A'],
  ['Sign-in with passkeys', 'api cleanup', 'billing v2', 'Webhooks', 'sign in', 'Sign in', 'sign-in', '#tags', '_private', '2fa', '(draft)'],
];
sorts.forEach((list, n) => add(`localeCompare ${n}`, { fn: 'localeSort', list }, w0().run(`${JSON.stringify(list)}.sort((a, b) => a.localeCompare(b))`)));
function w0() { return web(); }

writeGolden('forms', cases);
