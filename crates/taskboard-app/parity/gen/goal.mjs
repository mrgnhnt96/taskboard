// The goal page (pages.js): goalState, the header (goalMain), goalRunButtons + startLater, goalTasks
// (rows, goalTaskMeta, prMark, "How this goal runs"), gateBanner, gdeprioHtml, notesAside / noteItem /
// noteBody / noteDialogHtml, attachAside, goalBacklog / bulkBar / goalIssueState, the goal picker the
// bulk move uses (pickerItems), and every goal action's request + feedback (run/post stubbed).
import { web, text, acts, writeGolden, NOW } from '../web.mjs';

const w = web();
const at = mins => new Date(Date.parse(NOW) - mins * 60000).toISOString();
const decode = s => String(s ?? '').replace(/&quot;/g, '"').replace(/&#39;/g, "'").replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&amp;/g, '&');
// Visible text, without screen-reader-only spans.
const vis = html => text(String(html ?? '').replace(/<span class="sr">[\s\S]*?<\/span>/g, ''));
// What the browser shows for normal white-space: every run of whitespace is one space.
const shown = html => vis(html).replace(/\s+/g, ' ');
const attr = (html, re) => { const m = String(html).match(re); return m ? decode(m[1]) : null; };

const HOURS_OPEN = { on: true, open: true, start: '09:00', end: '17:00', days: ['mon', 'tue', 'wed', 'thu', 'fri'], line: '', next_open: null, today_until: null };
const HOURS_OFF = { on: false, open: true, start: '09:00', end: '17:00', days: [], line: '', next_open: null, today_until: null };
const closed = next_open => ({ ...HOURS_OPEN, open: false, next_open });
const state = (o = {}) => ({ now: NOW, jira: { enabled: !!o.jira }, work_hours: o.hours || HOURS_OPEN, alerts: [], projects: [], sessions: [], columns: {}, counts: {} });
const setState = o => w.set('S.state', state(o));

const T = (id, o) => ({ id, ref: 'T' + id, title: `Task ${id}`, project: 'webapp', status: 'queued', priority: 'normal', failed: false, lost: false,
  needs_reason: null, question: null, latest: null, summary: null, when: at(10), started_at: null, who: null, goal: { id: 1, ref: 'G1', name: 'Passkeys' },
  jira: null, pr: null, starting: false, waiting: null, blocked: false, ...o });
const RICH = [
  T(1, { status: 'done', who: 'T1 Add the endpoint', when: at(30), jira: { key: 'PROJ-1', status: 'Done', url: null },
    pr: { repo: 'webapp', num: 12, url: 'https://github.com/a/webapp/pull/12', title: 'x', state: 'MERGED', checks: 'pass', review: 'approved', stage: { phase: 'merged', label: 'Merged' } } }),
  T(2, { status: 'working', who: 'T2 Sign in', started_at: at(65), when: at(2),
    pr: { repo: 'webapp', num: 13, url: 'https://github.com/a/webapp/pull/13', title: 'y', state: 'OPEN', checks: 'pending', review: 'none', stage: { phase: 'checks', label: 'Watching checks' } } }),
  T(3, { status: 'needs', who: 'T3 Settings', question: 'Should it?', started_at: at(200), when: at(5), jira: { key: 'PROJ-3', status: null, url: null } }),
  T(4, { status: 'queued', priority: 'high', starting: true }),
  T(5, { status: 'queued', waiting: 'Waits for T4 to finish' }),
  T(6, { status: 'queued' }),
  T(7, { status: 'planned' }),
  T(8, { status: 'needs', lost: true, who: 'T8 Old terminal' }),
  T(9, { status: 'done', failed: true, who: null, when: at(60 * 26) }),
  T(10, { status: 'queued', blocked: true, priority: 'high' }),
  T(11, { status: 'planned', pr: { repo: 'webapp', num: 20, url: '', title: null, state: 'OPEN', checks: null, review: null, stage: null } }),
  T(12, { status: 'done', when: undefined }),
];
const G = o => ({ id: 1, ref: 'G1', name: 'Sign-in with passkeys', project: 'webapp', tldr: 'Add WebAuthn passkeys.', outcome: 'users can sign in with a passkey',
  epic_key: null, epic_status: null, epic_url: null, run_in_order: true, max_terminals: 2, auto_close: true, archived: false, paused: false, deprioritized: false,
  tasks: [], notes: [], backlog: [], closed_count: 0, attachments: [], ...o });

const QA = (id, o) => ({ id, ref: 'Q' + id, jira_key: 'PROJ-7', comment_id: String(100 + id), url: `https://acme.atlassian.net/browse/PROJ-7?focusedCommentId=${100 + id}`,
  author: 'Sam QA', verdict: 'flag', pr: 1, title: 'Keep the old login?', ask: 'Should the old login stay for one release? See `login.ts`.',
  text: 'Tested on iOS.\n\n- the old login still shows\n- should it?', task: null, task_status: null, task_title: null, source_task: 'T1',
  handled_at: null, handled_by: null, created_at: at(40), waiting: false, ...o });
const openPr = (o = {}) => ({ repo: 'webapp', num: 30, url: 'https://x/30', title: null, state: 'OPEN', checks: 'pass', review: 'approved', stage: null, ...o });
const GOALS = {
  'no tasks': G({}),
  'planned only': G({ tasks: [T(1, { status: 'planned' }), T(2, { status: 'planned' })] }),
  'all done': G({ tasks: [T(1, { status: 'done' }), T(2, { status: 'done', failed: true })] }),
  'done, 1 open PR': G({ tasks: [T(1, { status: 'done', pr: openPr() }), T(2, { status: 'done' })] }),
  'done, 2 open PRs': G({ tasks: [T(1, { status: 'done', pr: openPr() }), T(2, { status: 'done', pr: openPr({ num: 31, state: undefined }) })] }),
  'done, PR stopped': G({ tasks: [T(1, { status: 'done', pr: openPr({ stage: { phase: 'fix', label: 'Fixing checks', stopped: { asked: true, message: 'm' } } }) })] }),
  'done, failed with open PR': G({ tasks: [T(1, { status: 'done', failed: true, pr: openPr() })] }),
  'deprioritized': G({ deprioritized: true, tasks: [T(1, { status: 'working' }), T(2, { status: 'planned' })] }),
  'paused': G({ paused: true, tasks: [T(1, { status: 'queued' }), T(2, { status: 'planned' })] }),
  'lost': G({ tasks: [T(1, { status: 'needs', lost: true }), T(2, { status: 'working' })] }),
  'needs': G({ tasks: [T(1, { status: 'needs' }), T(2, { status: 'working' })] }),
  'working': G({ tasks: [T(1, { status: 'working' }), T(2, { status: 'planned' })] }),
  'blocked': G({ tasks: [T(1, { status: 'queued', blocked: true }), T(2, { status: 'queued' })] }),
  'queued': G({ tasks: [T(1, { status: 'queued' })] }),
  'start failed': G({ tasks: [T(1, { status: 'needs', needs_reason: 'start_failed' }), T(2, { status: 'queued' })] }),
  'going, paused, no planned': G({ paused: true, tasks: [T(1, { status: 'working' })] }),
  'rich': G({ tasks: RICH, epic_key: 'PROJ-9', epic_status: 'In Progress', epic_url: 'https://acme.atlassian.net/browse/PROJ-9' }),
  'rich, not in order': G({ tasks: RICH, run_in_order: false, epic_key: 'PROJ-9' }),
  'empty run_in_order 0': G({ run_in_order: 0, auto_close: 0, max_terminals: 7, tasks: [T(1, { status: 'planned' })] }),
  'max missing': G({ max_terminals: null, auto_close: undefined, run_in_order: undefined }),
  // Shared tasks (`tb task set T<n> --also G<n>`): a home goal runs them, other goals count them.
  // Waits, locks and running alone (`tb task set T<n> --waits-for/--lock/--alone`).
  'plan facts': G({ run_in_order: false, tasks: [
    T(1, { status: 'working', who: 'T1 Measure', started_at: at(20), when: at(2), locks: ['local-core'], alone: 'goal' }),
    T(2, { status: 'queued', waits_for_state: [{ ref: 'T1', done: false }, { ref: 'T9', done: true }], locks: ['local-core', 'emulator-5554'] }),
    T(3, { status: 'queued', waiting: 'Waits for local-core (T1 has it)', waits_for_state: [{ ref: 'T1', done: false }], locks: ['local-core'] }),
    T(4, { status: 'planned', waits_for_state: [{ ref: 'T2', done: false }], alone: 'board' }),
    T(5, { status: 'done', when: at(40), locks: ['local-core'], alone: 'goal' })] }),
  'shared': G({ tasks: [T(1, { status: 'done', who: 'T1 Add the endpoint', when: at(30), also: [{ id: 2, ref: 'G2', name: 'Sign-up' }, { id: 4, ref: 'G4', name: 'Audit' }] }),
    T(2, { status: 'planned', also: [{ id: 2, ref: 'G2', name: 'Sign-up' }] })],
    shared: [T(20, { status: 'working', who: 'T20 Auth client', goal: { id: 3, ref: 'G3', name: 'Auth client' }, pr: openPr({ num: 41, stage: { phase: 'rereview', label: 'Awaiting re-review' } }) }),
      T(21, { status: 'done', goal: { id: 3, ref: 'G3', name: 'Auth client' } }), T(22, { status: 'planned', goal: null })] }),
  'shared only': G({ shared: [T(23, { status: 'queued', goal: { id: 3, ref: 'G3', name: 'Auth client' } })] }),
  // QA comments on the goal's tickets (Settings ▸ QA on): waiting ones count on the tab, and make it hot.
  'qa waiting': G({ tasks: [T(1, { status: 'done' })], qa: [QA(3, { verdict: 'flag', waiting: true }), QA(2, { verdict: 'task', task: 'T9', task_status: 'working', task_title: 'QA on PROJ-7: Fix the overlap' }), QA(1, { verdict: 'flag', handled_at: at(5), handled_by: 'T4 Login' })] }),
  'qa handled': G({ tasks: [T(1, { status: 'done' })], qa: [QA(4, { verdict: null, ask: null, title: null, text: null }), QA(5, { verdict: 'task', task: 'T9', task_status: 'done', task_title: null })] }),
  'shared, all done': G({ tasks: [T(1, { status: 'done' })], shared: [T(24, { status: 'working', goal: { id: 3, ref: 'G3', name: 'Auth client' } })] }),
};
const HOURS = {
  open: HOURS_OPEN,
  off: HOURS_OFF,
  'closed, tomorrow': closed('2026-10-08T06:00'),
  'closed, today': closed('2026-10-07T18:30'),
  'closed, saturday': closed('2026-10-10T06:00'),
  'closed, unknown': closed(null),
};

const cases = [];
const add = (fn, name, input, expect) => cases.push({ name: `${fn} ${name}`, input: { fn, ...input }, expect });

// ---------------------------------------------------------------- goalState
for (const [n, g] of Object.entries(GOALS)) add('goalState', n, { goal: g }, w.call('goalState', w.call('countedTasks', g), g).label);

// ---------------------------------------------------------------- header (goalMain's <header>, minus the run buttons)
w.set('S.route', { page: 'goal', id: 'G1', q: {} });
for (const jira of [false, true]) {
  for (const [n, g] of Object.entries(GOALS)) {
    setState({ jira });
    w.set('P.goal', g);
    const html = w.call('goalMain', g);
    const head = html.match(/<header class="ghead">([\s\S]*?)<\/header>/)[1];
    const pills = [...head.matchAll(/<span class="pill [^"]*">([\s\S]*?)<\/span>/g)].map(m => vis(m[1]));
    add('header', `${n}${jira ? ' (jira)' : ''}`, { goal: g, jira }, {
      pills,
      name: vis(head.match(/<h2>([\s\S]*?)<\/h2>/)[1]),
      tldr: (m => m ? vis(m[1]) : null)(head.match(/<p class="tldr">([\s\S]*?)<\/p>/)),
      meta: vis(head.match(/<div class="meta">([\s\S]*?)<\/div>/)[1]),
      epic_link: attr(head, /<a class="epic" href="([^"]*)"/),
      tabs: [...html.matchAll(/data-act="goal-view" data-arg="(\w+)">([\s\S]*?)<\/button>/g)].map(m => vis(m[2])),
      tool: vis(html.match(/<div class="gbar">[\s\S]*?<div class="grow"><\/div>([\s\S]*?)<\/div>\s*<div class="gsec-body">/)[1]),
    });
  }
}
// The Backlog view's tool button.
{
  const g = GOALS.rich;
  setState({});
  w.set('S.route', { page: 'goal', id: 'G1', q: { view: 'backlog' } });
  const html = w.call('goalMain', g);
  add('backlogTool', 'rich', { goal: g }, vis(html.match(/<div class="gbar">[\s\S]*?<div class="grow"><\/div>([\s\S]*?)<\/div>\s*<div class="gsec-body">/)[1]));
  w.set('S.route', { page: 'goal', id: 'G1', q: {} });
}

// The QA tab: its own list, waiting ones first ("Waiting on you" / "Handled").
w.set('S.route', { page: 'goal', id: 'G1', q: { view: 'qa' } });
for (const n of ['qa waiting', 'qa handled']) {
  const g = GOALS[n];
  setState({});
  const html = w.call('goalMain', g);
  const body = html.match(/<div class="gsec-body">([\s\S]*?)<\/div>\s*<\/section>/)[1];
  add('qaTab', n, { goal: g }, {
    tabs: [...html.matchAll(/data-act="goal-view" data-arg="(\w+)">([\s\S]*?)<\/button>/g)].map(m => vis(m[2])),
    hot: /data-arg="qa">QA<span class="count hot">/.test(html),
    selected: attr(html, /aria-selected="true" data-act="goal-view" data-arg="(\w+)"/),
    groups: [...body.matchAll(/<h3 class="h3[^"]*">([^<]*)<\/h3><ol class="rows qrows">([\s\S]*?)<\/ol>/g)].map(m => ({
      title: m[1],
      items: [...m[2].matchAll(/<li class="qrow( wait)?">([\s\S]*?)<\/li>/g)].map(r => ({
        wait: !!r[1],
        chip: vis(r[2].match(/<span class="chip [^"]*">([\s\S]*?)<\/span>/)[1]),
        chip_cls: attr(r[2], /<span class="chip ([^"]*)">/),
        ref: vis(r[2].match(/<span class="pill ref">([\s\S]*?)<\/span>/)[1]),
        ask: vis(r[2].match(/<p class="q-ask">([\s\S]*?)<\/p>/)[1]),
        meta: vis(r[2].match(/<div class="m">([\s\S]*?)<\/div>/)[1]),
        link: attr(r[2], /<div class="m">[^<]*<a href="([^"]*)"/),
        opens: [...r[2].matchAll(/data-act="open-task" data-id="([^"]*)"/g)].map(m => m[1]),
        comment: /<details class="q-text">/.test(r[2]),
      })),
    })),
  });
}
w.set('S.route', { page: 'goal', id: 'G1', q: {} });

// ---------------------------------------------------------------- the wave rail (waves.js)
const W = (wave, o) => ({ wave, name: '', stop_after: false, released_at: null, passed: false, state: 'waiting', blocked: false, held_by: null, hold: null,
  tasks: [], total: 0, done: 0, failed: [], active: 0, starting: 0, planned: 0, ...o });
const g3 = { id: 3, ref: 'G3', name: 'Auth client' };
const WAVE_GOALS = {
  'waves running': G({
    tasks: [
      T(1, { status: 'done', wave: 1, who: 'T1 Model', when: at(90) }),
      T(2, { status: 'done', wave: 1, who: 'T2 Form', when: at(80), pr: openPr({ num: 31, stage: { phase: 'review', label: 'Awaiting review' } }) }),
      T(3, { status: 'working', wave: 2, who: 'T3 Wire', session_id: 's3', started_at: at(65), when: at(15), jira: { key: 'PROJ-3', status: 'In Progress', url: 'https://acme.atlassian.net/browse/PROJ-3' } }),
      T(4, { status: 'queued', wave: 2, starting: true }),
      T(5, { status: 'needs', wave: 2, who: 'T5 Copy', session_id: 's5', question: 'Which wording?', started_at: at(30), when: at(3) }),
      T(6, { status: 'queued', wave: 3, waiting: 'Waits for wave 2' }),
      T(7, { status: 'planned', wave: 3, priority: 'high', also: [{ id: 2, ref: 'G2', name: 'Sign-up' }] }),
      T(8, { status: 'queued', waiting: 'Waits for wave 2', blocked: true }),
    ],
    waves: [W(1, { name: 'Basics', state: 'done', stop_after: true, released_at: at(40), passed: true, total: 2, done: 2 }),
      W(2, { state: 'running', total: 3, active: 3, starting: 1 }),
      W(3, { name: 'Polish', state: 'waiting', blocked: true, held_by: 'running', hold: 'Waits for wave 2', stop_after: true, total: 2, planned: 1 })],
  }),
  'waves plan facts': G({
    tasks: [
      T(1, { status: 'working', wave: 1, who: 'T1 Measure', session_id: 's1', started_at: at(20), when: at(2), locks: ['local-core'], alone: 'board' }),
      T(2, { status: 'queued', wave: 1, waiting: 'Waits for local-core (T1 has it)', waits_for_state: [{ ref: 'T1', done: false }], locks: ['local-core'] }),
      T(3, { status: 'queued', wave: 1, waits_for_state: [{ ref: 'T1', done: false }, { ref: 'T7', done: true }] }),
      T(4, { status: 'planned', wave: 1, alone: 'goal', waits_for_state: [{ ref: 'T3', done: false }] }),
      T(5, { status: 'done', wave: 1, locks: ['local-core'] }),
    ],
    waves: [W(1, { state: 'running', total: 5, done: 1, active: 1, planned: 1 })],
  }),
  'waves stopped': G({
    tasks: [T(1, { status: 'done', wave: 1 }), T(2, { status: 'queued', wave: 2, waiting: 'Stopped for your review after wave 1 (Basics)' })],
    waves: [W(1, { name: 'Basics', state: 'stopped', stop_after: true, total: 1, done: 1 }),
      W(2, { state: 'waiting', blocked: true, held_by: 'stopped', hold: 'Stopped for your review after wave 1 (Basics)', total: 1 })],
  }),
  'waves failed': G({
    tasks: [T(1, { status: 'done', wave: 1, failed: true, who: 'T1 Model', when: at(20) }), T(2, { status: 'done', wave: 1 })],
    waves: [W(1, { state: 'failed', total: 2, done: 2, failed: ['T1'] })],
  }),
  'waves with shared': G({
    tasks: [T(1, { status: 'done', wave: 1 })],
    waves: [W(1, { state: 'done', passed: true, total: 1, done: 1 })],
    shared: [T(20, { status: 'done', goal: g3 }), T(21, { status: 'working', who: 'T21 Client', session_id: 's21', started_at: at(10), when: at(2), goal: g3 }), T(22, { status: 'queued', goal: null })],
  }),
  'waves shared landed, PR open': G({
    tasks: [T(1, { status: 'done', wave: 1, pr: openPr() })], prs_open: [30],
    waves: [W(1, { state: 'done', passed: true, total: 1, done: 1 })],
    shared: [T(20, { status: 'done', goal: g3 })],
  }),
  'waves finished': G({
    tasks: [T(1, { status: 'done', wave: 1 })], finished_at: at(5),
    waves: [W(1, { state: 'done', passed: true, total: 1, done: 1 })],
    shared: [T(20, { status: 'done', goal: g3 })],
  }),
};
const wtask = li => ({
  ref: vis(li.match(/<span class="n">([^<]*)<\/span>/)[1]),
  title: vis(li.match(/class="wt-title"[^>]*>([\s\S]*?)<\/button>/)[1]),
  planned: /class="wt planned"/.test(li),
  jira: attr(li, /class="jira-at" href="([^"]*)"/),
  pr: (x => x ? vis(x[1]) : null)(li.match(/class="pr-at[^"]*"[^>]*>([\s\S]*?)<\/(?:a|span)>/)),
  when: (x => x ? vis(x[1]) : null)(li.match(/<span class="wt-when[^"]*">([^<]*)<\/span>/)),
  facts: [...li.matchAll(/<(?:span|a) class="wchip ?([^"]*)"[^>]*>([\s\S]*?)<\/(?:span|a)>(?=<|\s*$)/g)].map(m => ({ cls: m[1], text: vis(m[2]) })),
  term: attr(li, /class="wchip term" href="([^"]*)"/),
});
const railItems = html => {
  const parts = html.split(/(?=<li class="wv[ "])/).slice(1);
  return parts.map(p => {
    const cls = p.match(/<li class="wv ?([^"]*)"/)[1];
    if (cls.includes('wv-ref')) return { kind: 'ref', done: cls.includes('ws-done'), badge: vis(p.match(/class="g-badge"[^>]*>([^<]*)</)[1]), badge_link: attr(p, /<a class="g-badge" href="([^"]*)"/),
      dashed: /stroke-dasharray/.test(p), line: attr(p, /stroke="(var\(--[a-z-]+\))"/), task: wtask(p) };
    if (cls.includes('wv-finish')) return { kind: 'finish', done: cls.includes('ws-done'), text: vis(p.match(/<span class="wv-name">([^<]*)<\/span>/)[1]) };
    const head = p.match(/<button type="button" class="wv-head"[\s\S]*?<\/button>/)[0];
    const gate = p.match(/<div class="wgate (\w+)"><span>([\s\S]*?)<\/span>([\s\S]*?)<\/div>/);
    return {
      kind: cls === 'ws-loose' ? 'post' : 'wave', state: (cls.match(/ws-(\w+)/) || [])[1] || null,
      open: /aria-expanded="true"/.test(head),
      n: (x => x ? vis(x[1]) : null)(head.match(/<span class="wv-n">([^<]*)<\/span>/)),
      name: (x => x ? vis(x[1]) : null)(head.match(/<span class="wv-name">([^<]*)<\/span>/)),
      count: [...head.matchAll(/<span class="pill sm ([^"]*)">([^<]*)<\/span>/g)].map(m => ({ cls: m[1], text: vis(m[2]) })),
      stop: /class="wv-stop"/.test(head),
      gate: gate ? { cls: gate[1], text: vis(gate[2]), button: (x => x ? { act: x[1], label: vis(x[2]) } : null)(gate[3].match(/data-act="([^"]*)"[^>]*>([\s\S]*?)<\/button>/)) } : null,
      tasks: [...p.matchAll(/<li class="wt[^"]*">([\s\S]*?)<\/li>/g)].map(m => wtask(m[0])),
    };
  });
};
for (const [n, g] of Object.entries(WAVE_GOALS)) {
  for (const opened of [false, true]) {
    setState({});
    w.run('S.waveOpen = {}');
    if (opened) for (const x of [...(g.waves || []).map(v => v.wave), 'none']) w.run(`S.waveOpen['G1:${x}'] = true`);
    const html = w.call('goalTasks', g);
    add('waveRail', `${n}${opened ? ' (all open)' : ''}`, { goal: g, open: opened }, {
      rail: /<ol class="wrail">/.test(html), shared_list: /From other goals/.test(html), items: railItems(html) });
  }
}
w.run('S.waveOpen = {}');
for (const [n, g] of Object.entries(WAVE_GOALS)) add('goalState', n, { goal: g }, w.call('goalState', w.call('countedTasks', g), g).label);

// ---------------------------------------------------------------- run buttons + start menu
const buttons = html => [...html.matchAll(/<button type="button" class="btn ([^"]*)"([^>]*)>([\s\S]*?)<\/button>/g)]
  .filter(m => !/split-caret/.test(m[1]))
  .map(m => ({ act: attr(m[2], /data-act="([^"]*)"/), label: vis(m[3]), title: attr(m[2], /title="([^"]*)"/) }));
for (const [hn, hours] of Object.entries(HOURS)) {
  for (const [n, g] of Object.entries(GOALS)) {
    if (hn !== 'open' && !['planned only', 'working', 'rich', 'deprioritized', 'paused'].includes(n)) continue;
    setState({ hours });
    w.run('S.startMenu = null');
    const html = w.call('goalRunButtons', g);
    add('runButtons', `${n} / ${hn}`, { goal: g, hours }, { split: /split-caret/.test(html), buttons: buttons(html) });
  }
}
for (const [hn, hours] of Object.entries(HOURS)) {
  if (hn === 'open' || hn === 'off') continue;
  setState({ hours });
  w.run("S.startMenu = 'G1'");
  const html = w.call('startLater', GOALS['planned only'], '');
  const menu = html.match(/<div class="ctxmenu[\s\S]*?<\/div>/)[0];
  add('startMenu', hn, { hours }, [...menu.matchAll(/data-act="([^"]*)"[^>]*><b>([\s\S]*?)<\/b><span>([\s\S]*?)<\/span>/g)].map(m => ({ act: m[1], title: vis(m[2]), sub: vis(m[3]) })));
  add('opensAt', hn, { hours }, w.run('opensAt()'));
}
w.run('S.startMenu = null');

// ---------------------------------------------------------------- task rows + how it runs
for (const n of ['rich', 'rich, not in order', 'no tasks', 'planned only', 'empty run_in_order 0', 'max missing', 'shared', 'shared only', 'plan facts']) {
  const g = GOALS[n];
  setState({});
  const html = w.call('goalTasks', g);
  const [own, others = ''] = html.split('<h3 class="h3 shared-h">From other goals</h3>');
  const shared = [...others.matchAll(/<li class="trow[^"]*">([\s\S]*?)<\/li>/g)].map(m => {
    const li = m[1];
    return {
      chip: vis(li.match(/<span class="chip st-[^"]*">([\s\S]*?)<\/span>/)[1]),
      title: vis(li.match(/<b>([\s\S]*?)<\/b>/)[1]),
      meta: vis(li.match(/<span class="m">([\s\S]*?)<\/span>/)[1]),
      tip: attr(li, /class="main"[^>]*title="([^"]*)"/),
      pr_text: (x => x ? vis(x[1]) : null)(li.match(/class="pr-at[^"]*"[^>]*>([\s\S]*?)<b>/)),
      planned: /class="trow planned"/.test(m[0]),
    };
  });
  const rows = [...own.matchAll(/<li class="trow[^"]*">([\s\S]*?)<\/li>/g)].map(m => {
    const li = m[1];
    return {
      n: vis(li.match(/<span class="num">(\d+)<\/span>/)[1]),
      chip: vis(li.match(/<span class="chip st-[^"]*">([\s\S]*?)<\/span>/)[1]),
      title: vis(li.match(/<b>([\s\S]*?)<\/b>/)[1]),
      meta: (x => x ? vis(x[1]) : '')(li.match(/<span class="m">([\s\S]*?)<\/span>/)),
      jira_tip: attr(li, /class="jira-at" title="([^"]*)"/),
      pr_text: (x => x ? vis(x[1]) : null)(li.match(/class="pr-at[^"]*"[^>]*>([\s\S]*?)<b>/)),
      pr_tip: attr(li, /class="pr-at[^"]*"[^>]*title="([^"]*)"/),
      planned: /class="trow planned"/.test(m[0]),
    };
  });
  const runs = html.match(/<div class="card-box">([\s\S]*)<\/div>/)[1];
  add('taskRows', n, { goal: g }, {
    empty: (x => x ? vis(x[1]) : null)(html.match(/<p class="empty-box">([\s\S]*?)<\/p>/)),
    rows,
    ...(shared.length ? { shared } : {}),
    max_options: [...runs.matchAll(/<option value="(\d+)"[^>]*>([^<]*)<\/option>/g)].map(m => vis(m[2])),
    max_selected: (x => x ? vis(x[1]) : null)(runs.match(/<option value="\d+" selected>([^<]*)<\/option>/)),
    run_in_order: /data-field="run_in_order"[^>]*checked/.test(runs),
    auto_close: /data-field="auto_close"[^>]*checked/.test(runs),
  });
}

// ---------------------------------------------------------------- gate banner + deprioritize dialog
for (const n of ['deprioritized', 'paused', 'working', 'going, paused, no planned']) {
  const g = GOALS[n];
  const html = w.call('gateBanner', g);
  add('gate', n, { goal: g }, { label: (x => x ? vis(x[1]) : null)(html.match(/<span class="box-label">([\s\S]*?)<\/span>/)), text: (x => x ? vis(x[1]) : null)(html.match(/<p>([\s\S]*?)<\/p>/)),
    buttons: [...html.matchAll(/<button[^>]*data-act="([^"]*)"[^>]*>([\s\S]*?)<\/button>/g)].map(m => ({ act: m[1], label: vis(m[2]) })) });
}
for (const n of ['rich', 'working', 'queued', 'no tasks', 'planned only']) {
  const g = GOALS[n];
  w.set('P.goal', g);
  const html = w.call('gdeprioHtml', { kind: 'gdeprio', id: 'G1' });
  add('deprioDialog', n, { goal: g }, { title: vis(html.match(/<h2>([\s\S]*?)<\/h2>/)[1]), body: [...html.matchAll(/<p[^>]*>([\s\S]*?)<\/p>/g)].map(m => vis(m[1])),
    buttons: [...html.matchAll(/<div class="modal-foot">([\s\S]*?)<\/div>\s*<\/div>/g)].flatMap(m => [...m[1].matchAll(/<button[^>]*>([\s\S]*?)<\/button>/g)].map(b => vis(b[1]))) });
}

// ---------------------------------------------------------------- notes
const long = 'The session cookie is set in middleware/auth.ts and refreshed on every request, which is why the passkey flow has to call refreshSession() after the assertion succeeds; otherwise the old cookie wins and the user looks signed out on the next page load.';
const NOTES = {
  none: [],
  mixed: [
    { id: 1, kind: 'decision', text: 'Keep passwords working.', source: 'you', pinned: true, at: null },
    { id: 2, kind: 'finding', text: '  Cookie in middleware/auth.ts  ', source: 'T1', pinned: false, at: null },
    { id: 3, kind: 'reference', text: 'https://webauthn.guide', source: null, pinned: false, at: null },
    { id: 4, kind: 'misc', text: 'Odd kind becomes a finding', source: 'you', pinned: false, at: null },
    { id: 5, kind: 'finding', text: long, source: 'T2', pinned: true, at: null },
    { id: 6, kind: 'finding', text: 'one\ntwo\nthree\nfour\nfive', source: null, pinned: false, at: null },
    { id: 7, kind: 'decision', text: 'Second decision', source: 'free text', pinned: false, at: null },
  ],
  ordering: [
    { id: 1, kind: 'finding', text: 'b', at: '2026-10-07T10:00:00Z', source: 'T1' },
    { id: 2, kind: 'finding', text: 'a', at: '2026-10-06T10:00:00Z', source: 'T1' },
    { id: 3, kind: 'finding', text: 'pinned late', at: '2026-10-07T12:00:00Z', source: 'T1', pinned: true },
    { id: 4, kind: 'finding', text: 'no time', source: 'T1' },
  ],
};
for (const [n, notes] of Object.entries(NOTES)) {
  const g = G({ notes });
  w.run("S.reveal = {}; S.drafts = {}; S.notes = {}");
  const html = w.call('notesAside', g);
  const groups = [...html.matchAll(/<div class="notes-g"><b>([\s\S]*?)<\/b><ul>([\s\S]*?)<\/ul><\/div>/g)].map(m => ({
    title: vis(m[1]),
    items: [...m[2].matchAll(/<li>([\s\S]*?)<\/li>/g)].map(li => ({
      text: shown((li[1].match(/<span class="note-t[^"]*">([\s\S]*?)<\/span>(?:\s*<small>|\s*$|\s*<span class="note-more">)/) || [, li[1]])[1]),
      from: (x => x ? vis(x[1]) : '')(li[1].match(/<small>([\s\S]*?)<\/small>/)),
      long: /note-t clamp/.test(li[1]),
      pinned: /class="pin"/.test(li[1]),
    })),
  }));
  add('notes', n, { notes }, { help: vis(html.match(/<p style="font-size:13px;color:var\(--muted\)">([\s\S]*?)<\/p>/)[1]), groups, add_button: acts(html).includes('reveal') });
}
// The note list with times (hhmm): kept apart because it depends on the shared clock format.
{
  const notes = [{ id: 1, kind: 'finding', text: 'x', source: 'you', at: '2026-10-07T14:05:00Z' }, { id: 2, kind: 'finding', text: 'y', at: '2026-10-03T09:00:00Z' }];
  add('noteFromTimes', 'you today / none earlier', { notes }, notes.map(n => w.call('noteFrom', n)));
}
// noteBody -> blocks; inline links/code.
const blocks = html => [...String(html).matchAll(/<(p|ul|pre)>([\s\S]*?)<\/\1>/g)].map(([, tag, body]) => {
  const inline = s => [...s.matchAll(/<a href="([^"]*)"[^>]*>[\s\S]*?<\/a>|<code>([\s\S]*?)<\/code>|([^<]+)/g)]
    .map(m => m[1] != null ? ['link', decode(m[1])] : m[2] != null ? ['code', decode(m[2])] : ['text', decode(m[3])]);
  if (tag === 'pre') return { pre: decode(body) };
  if (tag === 'ul') return { ul: [...body.matchAll(/<li>([\s\S]*?)<\/li>/g)].map(li => inline(li[1])) };
  return { p: body.split('<br>').map(inline) };
});
const BODIES = {
  plain: 'Just one line.',
  paragraphs: 'First line\nsecond line\n\nNew paragraph.',
  bullets: '- one\n* two\n• three\n    continued here\nafter',
  code: 'Run this:\n  cargo test\n  cargo build\nDone.',
  inline: 'See https://example.com/a?b=1. and `cargo fmt` in src/app.rs or app.dart, not and/or nor a/b.',
  trailing: 'Ends with url https://x.io/path), ok',
  ellipsis: 'path .../lib/x and ~/foo_bar',
  empty: '',
};
for (const [n, t] of Object.entries(BODIES)) add('noteBody', n, { text: t }, blocks(w.call('noteBody', t)));
for (const n of [NOTES.mixed[0], NOTES.mixed[1], NOTES.mixed[4], { id: 9, kind: 'reference', text: 'r', source: null }]) {
  const html = w.call('noteDialogHtml', { kind: 'gnote', note: n });
  add('noteDialog', `note ${n.id}`, { note: n }, vis(html.match(/<h2>([\s\S]*?)<\/h2>/)[1]));
}

// ---------------------------------------------------------------- attachments
const ATTS = [
  { id: 1, kind: 'design', title: 'Figma flows', url: 'https://figma.com/x', at: at(90), task: null, goal: 'G1' },
  { id: 2, kind: 'results', title: 'Bench numbers', url: '~/bench/results.md', at: at(60 * 30), task: 'T4', goal: null },
  { id: 3, kind: 'weird', title: 'Spec', url: 'notes.txt', at: null, task: null, goal: 'G2' },
  { id: 4, kind: 'doc', title: 'Doc', url: '/abs/doc.pdf', at: at(1), task: null, goal: null },
];
for (const [n, list] of Object.entries({ none: [], some: ATTS })) {
  const g = G({ attachments: list });
  w.run('S.attMenu = null; S.attEdit = null');
  const html = w.call('attachAside', g);
  add('attachments', n, { goal: g }, {
    empty: (x => x ? vis(x[1]) : null)(html.match(/<p style="font-size:13px;color:var\(--muted\)">([\s\S]*?)<\/p>/)),
    items: [...html.matchAll(/<li class="att[^"]*">([\s\S]*?)<\/li>/g)].map(m => ({
      name: vis((m[1].match(/class="att-name[^"]*"[^>]*>([\s\S]*?)<\/(?:a|span)>/) || [])[1]),
      linked: /<a class="att-name"/.test(m[1]),
      meta: vis((m[1].match(/<span class="att-m">([\s\S]*?)<\/span><\/span>/) || [])[1]),
      tip: attr(m[1], /class="att-name[^"]*"[^>]*title="([^"]*)"/),
    })),
  });
}
for (const a of ATTS) {
  w.run(`S.attMenu = '${a.id}'`);
  const html = w.call('attachAside', G({ attachments: [a] }));
  const menu = html.match(/<div class="ctxmenu att-menu"[\s\S]*?<\/div>/)[0];
  add('attMenu', `att ${a.id}`, { att: a }, [...menu.matchAll(/<(?:a|button)[^>]*role="menuitem"[^>]*>([\s\S]*?)<\/(?:a|button)>/g)].map(m => vis(m[1])));
}
w.run('S.attMenu = null');

// ---------------------------------------------------------------- backlog
const B = (id, o) => ({ id, ref: 'B' + id, kind: 'bug', title: `Issue ${id}`, goal: { id: 1, ref: 'G1', name: 'Passkeys' }, goal_id: 1, project: 'webapp', state: 'open',
  task_id: null, jira_key: null, source: 'terminal', found_by_name: 'T1 Add', found_by_task: 1, created_at: null, updated_at: null, ...o });
const BACKLOG = [
  B(1, {}),
  B(2, { kind: 'gap', source: 'you', found_by_name: null, found_by_task: null }),
  B(3, { kind: 'follow', source: 'answer' }),
  B(4, { kind: 'clean', state: 'task', task_id: 2 }),
  B(5, { kind: 'odd', state: 'task', task_id: 99 }),
  B(6, { kind: '', state: 'task', task_id: null, source: '', found_by_name: null, found_by_task: null }),
  B(7, { state: 'ticket', jira_key: 'PROJ-7', found_by_task: { id: 4, ref: 'T4', title: 'x' } }),
  B(8, { state: 'ticket', jira_key: null, found_by_name: null }),
  B(9, { state: 'drop' }),
];
for (const [n, g, jira, sel, showDropped] of [
  ['empty', G({}), false, [], false],
  ['empty with closed', G({ closed_count: 3 }), false, [], false],
  ['rows', G({ tasks: RICH, backlog: BACKLOG, closed_count: 2 }), false, [], false],
  ['rows, jira', G({ tasks: RICH, backlog: BACKLOG, closed_count: 0 }), true, [], false],
  ['rows, show dropped', G({ tasks: RICH, backlog: BACKLOG, closed_count: 2 }), false, [], true],
  ['no closed_count', G({ tasks: RICH, backlog: BACKLOG, closed_count: undefined }), false, [], false],
  ['some selected', G({ tasks: RICH, backlog: BACKLOG }), true, ['B1'], false],
  ['all selected', G({ tasks: RICH, backlog: BACKLOG }), false, ['B1', 'B2', 'B3'], false],
  ['only closed', G({ tasks: RICH, backlog: [B(4, { state: 'task', task_id: 2 })] }), false, [], false],
]) {
  setState({ jira });
  w.set('P.goal', g);
  w.run(`P.sel = new Set(${JSON.stringify(sel)}); P.showDropped = ${showDropped}; P.kept = {}; P.order = []; S.issueRef = null; S.busy = new Set(); S.notes = {}`);
  const html = w.call('goalBacklog', g);
  const head = html.match(/<div class="bl-head[^"]*"[^>]*>([\s\S]*?)<\/div>\s*<ul/);
  add('backlog', n, { goal: g, jira, sel, showDropped }, {
    help: vis(html.match(/<p[^>]*>([\s\S]*?)<\/p>/)[1]),
    bar: head ? { label: vis(head[1].match(/<label class="bl-all">([\s\S]*?)<\/label>/)[1]),
      buttons: [...head[1].matchAll(/<button[^>]*>([\s\S]*?)<\/button>/g)].map(b => vis(b[1])) } : null,
    rows: [...html.matchAll(/<li class="irow[^"]*">([\s\S]*?)<\/li>/g)].map(m => ({
      checkbox: /class="bsel"/.test(m[1]),
      kind: vis(m[1].match(/<span class="chip k-[^"]*">([\s\S]*?)<\/span>/)[1]),
      title: vis(m[1].match(/<b>([\s\S]*?)<\/b>/)[1]),
      from: vis(m[1].match(/<span class="from">([\s\S]*?)<\/span>/)[1]),
      actions: (x => x ? [...x[1].matchAll(/<button[^>]*>([\s\S]*?)<\/button>/g)].map(b => vis(b[1])) : [])(m[1].match(/<span class="acts">([\s\S]*?)<\/span>/)),
      state: (x => x ? vis(x[1]) : null)(m[1].match(/<span class="state [^"]*">([\s\S]*?)<\/span>/)),
    })),
    closed_line: (x => x ? vis(x[1]) : null)(html.match(/<span class="help">([\s\S]*?)<\/span>\s*$/)),
  });
}
// issueFrom with times (hhmm) kept apart, like noteFrom.
add('issueFromTimes', 'today / earlier', {}, [w.call('issueFrom', B(1, { created_at: '2026-10-07T14:05:00Z' }), true), w.call('issueFrom', B(2, { source: 'you', created_at: '2026-10-03T09:00:00Z' }), true)]);

// ---------------------------------------------------------------- actions: request + feedback, from the web's own handlers
// post() is a const over api(); stub api (a function declaration) and the page's side effects.
w.run(`var __posts = [], __runs = [];
  api = (path, body) => { if (body !== undefined) __posts.push([path, body]); return Promise.resolve(null); };
  run = (el, fn, o) => { __runs.push({ el, o: o || {} }); fn(); };
  toast = () => {}; setNote = () => {}; renderAll = () => {}; renderModal = () => {}; closeModal = () => {}; refresh = () => {}; nav = () => {};`);
const fire = (act, el, setup, okArg) => {
  if (setup) w.run(setup);
  w.run('__posts = []; __runs = []');
  w.set('__el', el);
  w.run(`ACTIONS[${JSON.stringify(act)}](__el, {})`);
  const r = w.run('__runs[0]');
  const o = r ? r.o : {};
  const grp = (o.grp || (r && r.el && r.el.dataset && r.el.dataset.grp)) || null;
  const ok = typeof o.ok === 'function' ? w.run(`(__runs[0].o.ok)(${JSON.stringify(okArg ?? null)})`) : (o.ok ?? null);
  return { posts: w.run('__posts'), feedback: ok ? (grp ? { note: grp, text: ok } : o.toast ? { toast: ok } : { note: null, text: ok }) : null };
};
const ACTS = [];
const act = (name, a, el, setup, okArg) => ACTS.push({ name, expect: fire(a, el, setup, okArg) });
setState({ hours: HOURS_OPEN });
w.set('P.goal', G({ tasks: RICH }));
act('run', 'goal-run', { dataset: { id: 'G1' } }, null, { queued_now: 2 });
act('run, nothing queued', 'goal-run', { dataset: { id: 'G1' } }, null, { queued_now: 0 });
act('run, 1 queued', 'goal-run', { dataset: { id: 'G1' } }, null, { queued_now: 1 });
setState({ hours: closed('2026-10-08T06:00') });
act('run after hours', 'goal-run', { dataset: { id: 'G1' } }, null, { queued_now: 3 });
act('run now', 'goal-run-now', { dataset: { id: 'G1' } }, null, { queued_now: 2 });
act('run now, none', 'goal-run-now', { dataset: { id: 'G1' } }, null, {});
setState({ hours: HOURS_OPEN });
act('plan', 'goal-plan-edit', { dataset: { id: 'G1' } });
act('pause', 'goal-pause', { dataset: { id: 'G1', arg: 'on' } });
act('resume (gate)', 'goal-pause', { dataset: { id: 'G1', arg: 'off', grp: 'ggate:G1' } });
act('deprioritize (dialog)', 'goal-deprio-yes', { dataset: { id: 'G1' } });
act('bring back (gate)', 'goal-deprio', { dataset: { id: 'G1', arg: 'off', grp: 'ggate:G1' } });
w.run("S.route = {page: 'goal', id: 'G1', q: {}}; S.keepIssues = []");
act('promote (goal row)', 'promote', { dataset: { id: 'B1', arg: 'goal', grp: 'issue:B1' } }, null, { task: { id: 7, ref: 'T7' } });
act('ticket (goal row)', 'ticket', { dataset: { id: 'B1', grp: 'issue:B1' } });
act('drop (goal row)', 'drop', { dataset: { id: 'B1', grp: 'issue:B1' } });
for (const [n, a, extra, setup] of [
  ['bulk make tasks', 'task', null, "P.sel = new Set(['B1','B2'])"],
  ['bulk tickets', 'ticket', null, "P.sel = new Set(['B2'])"],
  ['bulk drop', 'drop', null, "P.sel = new Set(['B3','B1'])"],
]) {
  w.run(setup);
  w.run('__posts = []; __runs = []');
  w.run(`bulkChange({ dataset: { act: 'bl-bulk', arg: '${a}' } }, '${a}', {})`);
  const r = w.run('__runs[0]');
  ACTS.push({ name: n, expect: { posts: w.run('__posts'), feedback: null, bulk_toast: [w.run(`BULK_DONE['${a}'](2)`), w.run(`BULK_DONE['${a}'](1)`)] } });
}
for (const [n, field, el] of [
  ['set max terminals', 'max_terminals', { type: 'select-one', value: '3', checked: false, dataset: { field: 'max_terminals', id: 'G1', grp: 'gset:G1' } }],
  ['set run in order off', 'run_in_order', { type: 'checkbox', value: 'on', checked: false, dataset: { field: 'run_in_order', id: 'G1', grp: 'gset:G1' } }],
  ['set auto close on', 'auto_close', { type: 'checkbox', value: 'on', checked: true, dataset: { field: 'auto_close', id: 'G1', grp: 'gset:G1' } }],
]) {
  w.run('__posts = []; __runs = []');
  w.set('__el', el);
  w.run("CHANGES['goal-set'](__el)");
  const r = w.run('__runs[0]');
  ACTS.push({ name: n, expect: { posts: w.run('__posts'), feedback: { note: r.el.dataset.grp, text: r.o.ok } } });
}
for (const c of ACTS) add('action', c.name, {}, c.expect);

writeGolden('goal', cases);
