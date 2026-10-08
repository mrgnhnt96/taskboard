// The chrome around the pages (app.js): the goals rail and the goal page's goal list (now the
// sidebar), goal status / ring / peek card, banner + alerts dialog, status-bar pills, the work
// hours menu (and what it posts), the page title, and mdLite.
import { web, writeGolden, text, acts, NOW } from '../web.mjs';

const at = mins => new Date(Date.parse(NOW) - mins * 60000).toISOString();
const cases = [];
const add = (name, input, expect) => cases.push({ name, input, expect });

// ---------------------------------------------------------------- fixtures

const goal = (id, o = {}) => ({
  id, ref: `G${id}`, name: `Goal ${id}`, project: 'webapp', tldr: '', outcome: '', epic_key: null, epic_status: null, epic_url: null,
  run_in_order: true, max_terminals: 2, auto_close: true, archived: false, paused: false, deprioritized: false,
  total: 0, done: 0, active: 0, needs: 0, queued: 0, starting: 0, blocked: 0, planned: 0, open_issues: 0, prs_open: [], finished_at: null, peek: [],
  ...o,
});
const GOALS = [
  goal(1, { name: 'Sign-in with passkeys', total: 4, done: 1, active: 2, needs: 1, planned: 1,
    peek: [{ ref: 'T3', title: 'Settings page', status: 'needs', why: null }, { ref: 'T2', title: 'Sign in', status: 'working', why: null },
      { ref: 'T9', title: 'Old thing', status: 'failed', why: null }, { ref: 'T7', title: 'Docs', status: 'blocked', why: 'Waits for T3 to finish' },
      { ref: 'T8', title: 'Starter', status: 'starting', why: null }] }),
  goal(2, { name: 'Running one', project: 'api', total: 3, done: 1, active: 1 }),
  goal(3, { name: 'Starting one', project: 'api', total: 2, queued: 1, starting: 1 }),
  goal(4, { name: 'Paused one', total: 2, done: 1, queued: 1, paused: true }),
  goal(5, { name: 'Blocked one', total: 3, queued: 2, blocked: 2 }),
  goal(6, { name: 'Queued one', total: 3, queued: 1, blocked: 0, planned: 2 }),
  goal(7, { name: 'Planned only', total: 2, planned: 2 }),
  goal(8, { name: 'Empty goal', project: 'docs' }),
  goal(9, { name: 'Awaiting merge', total: 2, done: 2, prs_open: [41] }),
  goal(10, { name: 'Two PRs', total: 2, done: 2, prs_open: [41, 42] }),
  goal(11, { name: 'Finished today', total: 3, done: 3, finished_at: at(60 * 3) }),
  goal(12, { name: 'Finished last week', total: 1, done: 1, finished_at: at(60 * 24 * 6) }),
  goal(13, { name: 'Deprioritized one', total: 2, done: 0, queued: 1, deprioritized: true }),
  goal(14, { name: 'Archived', archived: true, total: 1 }),
  goal(15, { name: 'Several needs', project: 'api', total: 4, done: 0, active: 3, needs: 2 }),
  goal(16, { name: 'Not running', total: 3, done: 1 }),
];
const HOURS_OPEN = { on: true, start: '09:00', end: '17:00', days: ['mon', 'tue', 'wed', 'thu', 'fri'], open: true, line: 'Work hours 9am–5pm: agents start until 5pm', next_open: null, today_until: null };
const HOURS_UNTIL = { ...HOURS_OPEN, today_until: '16:00' };
const HOURS_CLOSED_TODAY = { ...HOURS_OPEN, open: false, next_open: '2026-10-07T18:30', line: 'Closed' };
const HOURS_CLOSED_THU = { ...HOURS_OPEN, open: false, next_open: '2026-10-08T06:00', line: 'Agents off until Thu 6am' };
const HOURS_OFF = { ...HOURS_OPEN, on: false, open: true, line: 'No work hours: agents start any time' };
const USAGE = { seen_at: at(10), windows: [{ key: 'five_hour', label: '5h', pct: 35, resets_at: '2026-10-07T18:00:00Z' }, { key: 'seven_day', label: '7d', pct: 78, resets_at: '2026-10-10T09:00:00Z' }] };
const USAGE_HOT = { seen_at: at(90), windows: [{ key: 'five_hour', label: '5h', pct: 100, resets_at: null }, { key: 'seven_day', label: '7d', pct: 90, resets_at: '2026-10-07T20:00:00Z' }] };
const state = o => ({
  now: NOW, midna: { up: true, seen_at: at(0.2) }, jira: { enabled: false }, alerts: [], work_hours: HOURS_OPEN, usage: USAGE,
  projects: [{ name: 'api', path: '/tmp/api' }, { name: 'webapp', path: '/tmp/webapp' }],
  sessions: [], session_projects: ['webapp', 'zeta'], goals: [], columns: {}, counts: { needs: 0, open_issues: 0, done_hidden: 0 },
  ...o,
});

// ---------------------------------------------------------------- goal status, counts, ring

for (const [hours, usage, why] of [[HOURS_OPEN, USAGE, 'open'], [HOURS_CLOSED_THU, USAGE, 'after hours'], [HOURS_OPEN, USAGE_HOT, 'out of usage']]) {
  const w = web();
  w.set('S.state', state({ work_hours: hours, usage }));
  add(`agentsHeldWhy ${why}`, { fn: 'agentsHeldWhy', state: state({ work_hours: hours, usage }) }, w.call('agentsHeldWhy'));
  for (const g of GOALS) {
    const c = w.call('goalCounts', g);
    add(`goalStatus ${g.ref} (${why})`, { fn: 'goalStatus', goal: g, state: state({ work_hours: hours, usage }) }, w.run(`goalStatus(${JSON.stringify(g)}, goalCounts(${JSON.stringify(g)}))`));
    if (why === 'open') {
      add(`goalCounts ${g.ref}`, { fn: 'goalCounts', goal: g }, c);
      add(`goalNavStatus ${g.ref}`, { fn: 'goalNavStatus', goal: g }, w.run(`goalNavStatus(${JSON.stringify(g)}, goalCounts(${JSON.stringify(g)}))`));
      // The ring: arcs (done, done+active fractions as dasharray lengths) and the glyph / tick.
      const svg = w.run(`goalRing(goalCounts(${JSON.stringify(g)}), goalStatus(${JSON.stringify(g)}, goalCounts(${JSON.stringify(g)})))`);
      const dash = [...svg.matchAll(/stroke-dasharray="([\d.]+) /g)].map(m => Number(m[1]));
      const glyph = (svg.match(/class="glyph g-([a-z]+)"/) || [])[1] || (svg.includes('class="tick"') ? 'tick' : null);
      add(`goalRing ${g.ref}`, { fn: 'goalRing', goal: g }, { active: dash[0], done: dash[1], glyph });
    }
  }
}

// ---------------------------------------------------------------- recently completed window

for (const iso of ['2026-10-07T15:00:00Z', '2026-10-10T12:00:00Z', '2026-10-11T08:00:00Z', '2026-10-12T09:30:00Z', '2026-10-09T23:00:00Z', '2026-10-13T00:30:00Z']) {
  const w = web();
  add(`recentSince ${iso}`, { fn: 'recentSince', now: iso }, new Date(w.call('recentSince', Date.parse(iso))).toISOString());
}

// ---------------------------------------------------------------- the rail

function parseRail(html) {
  const out = { brand: text((html.match(/<div class="bg-brand">([\s\S]*?)<\/div>/) || [])[1]), head: null, groups: [], none: null, recent: null };
  const head = html.match(/<div class="bg-head">([\s\S]*?)<\/div>/);
  out.head = head ? { title: text(head[1].match(/<a class="bg-title"[^>]*>([\s\S]*?)<\/a>/)[1]), count: Number(text(head[1].match(/<span class="count">([\s\S]*?)<\/span>/)[1])) } : null;
  const none = html.match(/<p class="bg-none">([\s\S]*?)<\/p>/);
  out.none = none ? text(none[1]) : null;
  const rows = section => [...section.matchAll(/<div class="bg-item([^"]*)">\s*<button[^>]*data-arg="([^"]+)"[^>]*>([\s\S]*?)<\/button>([\s\S]*?)<\/div>/g)].map(m => {
    const inner = m[3];
    const glyph = (inner.match(/class="glyph g-([a-z]+)"/) || [])[1] || (inner.includes('class="tick"') ? 'tick' : null);
    return {
      ref: m[2], on: m[1].includes(' on'), dim: m[1].includes('finished'),
      name: text(inner.match(/<span class="bg-name">([\s\S]*?)<\/span><\/span>|<span class="bg-name">([\s\S]*?)<\/span>(?=<span class="sr">|<span class="bg-n">)/)[0]),
      status: (inner.match(/<span class="sr">([\s\S]*?)<\/span>/) || [])[1] || null,
      count: text((inner.match(/<span class="bg-n">([\s\S]*?)<\/span>/) || [])[1]),
      glyph, open: m[4].includes('bg-open'),
    };
  });
  const recentAt = html.indexOf('<div class="bg-recent">');
  const listHtml = recentAt < 0 ? html : html.slice(0, recentAt);
  const parts = listHtml.split('<div class="bg-proj').slice(1);
  for (const p of parts) {
    const fold = p.match(/data-act="rail-fold" data-arg="([^"]*)" aria-expanded="(true|false)"/);
    const pn = p.match(/data-act="rail-project" data-arg="[^"]*" aria-pressed="(true|false)"[^>]*><span>([\s\S]*?)<\/span>([\s\S]*?)<span class="bg-n">(\d+)<\/span>/);
    out.groups.push({ project: fold[1], open: fold[2] === 'true', on: pn[1] === 'true', dot: pn[3].includes('bg-dot'), count: Number(pn[4]), rows: rows('<div class="bg-proj' + p) });
  }
  if (recentAt >= 0) {
    const r = html.slice(recentAt);
    out.recent = { open: /aria-expanded="true"/.test(r), count: Number(r.match(/<span class="bg-n">(\d+)<\/span>/)[1]), rows: rows(r) };
  }
  return out;
}

const RAILS = [
  ['default', state({}), GOALS, { project: 'all', goal: 'all', done: '24h' }, {}],
  ['goal picked', state({}), GOALS, { project: 'webapp', goal: 'G4', done: '24h' }, {}],
  ['project filter + shut', state({}), GOALS, { project: 'api', goal: 'all', done: '24h' }, { 'tb.rail.shut': JSON.stringify(['webapp', 'api']) }],
  ['shut but selected', state({}), GOALS, { project: 'webapp', goal: 'G1', done: '24h' }, { 'tb.rail.shut': JSON.stringify(['webapp']) }],
  ['recent open', state({}), GOALS, { project: 'all', goal: 'all', done: '24h' }, { 'tb.rail.recent': 'open' }],
  ['recent selected', state({}), GOALS, { project: 'webapp', goal: 'G11', done: '24h' }, {}],
  ['filter project not seen', state({ session_projects: [] }), [], { project: 'mobile', goal: 'all', done: '24h' }, {}],
  ['no goals yet', state({ session_projects: [] }), [], { project: 'all', goal: 'all', done: '24h' }, {}],
  ['loading', null, null, { project: 'all', goal: 'all', done: '24h' }, {}],
];
for (const [name, st, goals, filter, ls] of RAILS) {
  const w = web();
  w.set('S.state', st);
  w.set('S.goalsAll', goals);
  w.set('S.filter', filter);
  for (const [k, v] of Object.entries(ls)) w.run(`localStorage.setItem(${JSON.stringify(k)}, ${JSON.stringify(v)})`);
  add(`rail ${name}`, { fn: 'rail', state: st, goals, filter, prefs: ls }, parseRail(w.call('goalsRailHtml')));
}

// The rail's actions (goal-pick / rail-project / rail-fold / rail-recent): the filter / prefs after.
for (const [act, arg, filter, ls] of [
  ['goal-pick', 'G4', { project: 'all', goal: 'all', done: '24h' }, {}],
  ['goal-pick', 'G4', { project: 'webapp', goal: 'G4', done: '7d' }, {}],
  ['goal-pick', 'G2', { project: 'webapp', goal: 'G4', done: '24h' }, {}],
  ['rail-project', 'api', { project: 'all', goal: 'all', done: '24h' }, {}],
  ['rail-project', 'api', { project: 'api', goal: 'all', done: '24h' }, {}],
  ['rail-project', 'api', { project: 'webapp', goal: 'G4', done: '24h' }, {}],
  ['rail-fold', 'webapp', { project: 'all', goal: 'all', done: '24h' }, {}],
  ['rail-fold', 'webapp', { project: 'all', goal: 'all', done: '24h' }, { 'tb.rail.shut': JSON.stringify(['webapp', 'api']) }],
  ['rail-recent', null, { project: 'all', goal: 'all', done: '24h' }, {}],
  ['rail-recent', null, { project: 'all', goal: 'all', done: '24h' }, { 'tb.rail.recent': 'open' }],
]) {
  const w = web();
  w.set('S.state', state({}));
  w.set('S.goalsAll', GOALS);
  w.set('S.filter', filter);
  for (const [k, v] of Object.entries(ls)) w.run(`localStorage.setItem(${JSON.stringify(k)}, ${JSON.stringify(v)})`);
  w.run('renderAll = () => {}; refresh = () => {}; S.keepIssues = ["B1"]');
  w.run(`ACTIONS[${JSON.stringify(act)}]({ dataset: { arg: ${JSON.stringify(arg)} } })`);
  add(`action ${act} ${arg} from ${JSON.stringify(filter)} ${JSON.stringify(ls)}`, { fn: 'railAction', act, arg, filter, prefs: ls },
    { filter: w.run('S.filter'), keep: w.run('S.keepIssues'), shut: w.run("localStorage.getItem('tb.rail.shut')"), recent: w.run("localStorage.getItem('tb.rail.recent')"),
      saved: w.run("localStorage.getItem('taskboard.filter')") });
}

// railClear (Esc on the board with a filter set).
{
  const w = web();
  w.set('S.filter', { project: 'api', goal: 'G2', done: '7d' });
  w.run('renderAll = () => {}; refresh = () => {}; S.keepIssues = ["B1"]; railClear()');
  add('railClear', { fn: 'railClear', filter: { project: 'api', goal: 'G2', done: '7d' } }, { filter: w.run('S.filter'), keep: w.run('S.keepIssues') });
}

// Rail width (tb.rail.w): clamp and default.
for (const v of [null, '0', '150', '220', '333', '599.6', '900', 'abc']) {
  const w = web();
  if (v !== null) w.run(`localStorage.setItem('tb.rail.w', ${JSON.stringify(v)})`);
  add(`railWidth ${v}`, { fn: 'railWidth', saved: v }, w.call('railWidth'));
}

// ---------------------------------------------------------------- peek card

function parsePeek(html) {
  const head = html.match(/<div class="gp-head">([\s\S]*?)<\/div>/)[1];
  return {
    title: text(head.match(/<b>([\s\S]*?)<\/b>/)[1]),
    sub: text(head.match(/<span class="gp-sub">([\s\S]*)<\/span>/)[1]),
    groups: [...html.matchAll(/<div class="gp-grp">([\s\S]*?)<\/div>\s*((?:<button[\s\S]*?<\/button>)*)<\/div>/g)].map(m => ({
      label: text(m[1].match(/<span class="chip[^"]*">([\s\S]*?)<\/span>/)[1]),
      count: Number(text(m[1].match(/<span class="bg-n">([\s\S]*?)<\/span>/)[1])),
      tasks: [...m[2].matchAll(/data-id="([^"]+)"><span class="gp-ref">[\s\S]*?<\/span><span class="gp-t"><b>([\s\S]*?)<\/b>(?:<span class="gp-why">([\s\S]*?)<\/span>)?<\/span>/g)].map(t => ({ ref: t[1], title: text(t[2]), why: t[3] ? text(t[3]) : null })),
    })),
  };
}
for (const [hours, usage, why] of [[HOURS_OPEN, USAGE, 'open'], [HOURS_CLOSED_THU, USAGE, 'after hours']]) {
  const w = web();
  w.set('S.state', state({ work_hours: hours, usage }));
  for (const g of GOALS.filter(g => ['G1', 'G2', 'G6', 'G8', 'G9', 'G11'].includes(g.ref))) {
    add(`peek ${g.ref} (${why})`, { fn: 'peek', goal: g, state: state({ work_hours: hours, usage }) }, parsePeek(w.call('goalPeekHtml', g)));
  }
}

// ---------------------------------------------------------------- goal list (goal page nav)

function parseNav(html) {
  const tabs = [...html.matchAll(/data-act="gnav-view" data-arg="([a-z]+)" aria-pressed="(true|false)">([^<]+)<span class="count">(\d+)<\/span>/g)]
    .map(m => ({ view: m[1], on: m[2] === 'true', label: m[3], count: Number(m[4]) }));
  const groups = [...html.matchAll(/<div class="gn-proj"><span>([\s\S]*?)<\/span><span class="bg-n">(\d+)<\/span><\/div>\s*<div class="gn-list">([\s\S]*?)<\/div><\/div>/g)].map(m => ({
    project: text(m[1]), count: Number(m[2]),
    items: [...m[3].matchAll(/<a class="gitem" href="#\/goals\/([^"]+)" aria-current="(true|false)"[\s\S]*?<span class="gi-st ([a-z]+)">(<i[^>]*><\/i>)?([\s\S]*?)<span class="gi-n">([\s\S]*?)<\/span>/g)]
      .map(i => ({ ref: i[1], current: i[2] === 'true', kind: i[3], dot: !!i[4], label: text(i[5]), n: text(i[6]) })),
  }));
  const help = html.match(/<p class="help">([\s\S]*?)<\/p>/);
  return { tabs, groups, empty: help ? text(help[1]) : null };
}
for (const [cur, view] of [['G1', null], ['G13', null], ['G11', null], ['G1', 'done'], ['G1', 'deprio'], [null, null]]) {
  const w = web();
  w.set('S.state', state({}));
  if (view) w.run(`P.gnav = { view: ${JSON.stringify(view)}, for: ${JSON.stringify(cur)} }`);
  const goals = GOALS.filter(g => !g.archived);
  add(`goalNavList cur=${cur} view=${view}`, { fn: 'goalNav', goals, cur, view }, parseNav(w.call('goalNavList', goals, cur)));
}
for (const [cur, view] of [['G1', 'deprio']]) {
  const w = web();
  w.set('S.state', state({}));
  w.run(`P.gnav = { view: ${JSON.stringify(view)}, for: 'G1' }`);
  const goals = [GOALS[0]];
  add(`goalNavList empty deprio`, { fn: 'goalNav', goals, cur, view }, parseNav(w.call('goalNavList', goals, cur)));
  w.run(`P.gnav = { view: 'done', for: 'G1' }`);
  add(`goalNavList empty done`, { fn: 'goalNav', goals, cur, view: 'done' }, parseNav(w.call('goalNavList', goals, cur)));
}
// Which goal `#/goals` opens with no id: renderGoalPage's own expression (pages.js:103-104). In the
// web this branch then threw (it calls the page-local `nav` before its `const` line), so the landing
// never worked there; the app does what the code meant.
for (const fg of ['all', 'G13', 'G99']) {
  const w = web();
  w.set('S.goalsAll', GOALS);
  w.set('S.filter', { project: 'all', goal: fg, done: '24h' });
  const goals = 'S.goalsAll.filter(x => !x.archived)';
  const first = w.run(`(() => { const goals = ${goals}; return goalByRef(S.filter.goal) || goals.filter(x => gnavView(x) === 'active')
      .sort((a, b) => (a.project || '').localeCompare(b.project || ''))[0] || goals[0]; })()`);
  add(`goals page opens (filter ${fg})`, { fn: 'goalsLanding', goals: GOALS, filterGoal: fg }, first ? first.ref : null);
}

// ---------------------------------------------------------------- status bar

for (const [name, m] of [['up', { up: true, seen_at: at(0.2) }], ['up old', { up: true, seen_at: at(5) }], ['down', { up: false, seen_at: at(3) }], ['never', { up: false, seen_at: null }]]) {
  const w = web();
  const html = w.call('connPill', m);
  add(`connPill ${name}`, { fn: 'connPill', midna: m }, { cls: html.match(/class="conn ([a-z]+)"/)[1], label: text(html), title: html.match(/title="([^"]*)"/)[1].replace(/&#39;/g, "'").replace(/&quot;/g, '"').replace(/&amp;/g, '&') });
}
for (const [name, h] of [['open', HOURS_OPEN], ['today until', HOURS_UNTIL], ['closed today', HOURS_CLOSED_TODAY], ['closed thu', HOURS_CLOSED_THU], ['off', HOURS_OFF], ['closed no next', { ...HOURS_OPEN, open: false, next_open: null }]]) {
  const w = web();
  w.set('S.state', state({ work_hours: h }));
  const html = w.call('hoursPill');
  add(`hoursPill ${name}`, { fn: 'hoursPill', hours: h }, { closed: html.includes('conn hours closed'), label: text(html), title: html.match(/title="([^"]*)"/)[1] });
}
for (const [name, u] of [['normal', USAGE], ['hot stale', USAGE_HOT], ['none', null], ['empty', { seen_at: at(1), windows: [] }],
  ['warm edges', { seen_at: at(1), windows: [{ key: 'five_hour', label: '5h', pct: 74, resets_at: null }, { key: 'seven_day', label: '7d', pct: 75, resets_at: null }, { key: 'other', label: 'x', pct: 89, resets_at: null }] }]]) {
  const w = web();
  w.set('S.state', state({ usage: u }));
  const html = w.call('usagePill');
  add(`usagePill ${name}`, { fn: 'usagePill', usage: u }, html ? {
    stale: html.includes('usage stale'),
    title: html.match(/title="([^"]*)"/)[1].replace(/&#39;/g, "'"),
    windows: [...html.matchAll(/<span class="u-win ([a-z]+)"><span class="u-k">([^<]*)<\/span><span class="u-bar"><i style="width:([\d.]+)%"><\/i><\/span><span class="u-v">([^<]*)<\/span>/g)]
      .map(m => ({ level: m[1], label: m[2], width: Number(m[3]), value: m[4] })),
  } : null);
}

// ---------------------------------------------------------------- banner + alerts dialog

const ALERTS = [
  { id: 'a1', at: at(12), text: 'T4 couldn’t start in Midna.', task: 'T4', goal: null },
  { id: 'a2', at: at(3), text: 'G2 is stuck.', task: null, goal: 'G2' },
  { id: 'a3', at: at(1), text: 'Something needs you.', task: null, goal: null },
];
function parseBanner(html) {
  return [...html.matchAll(/<div class="banner ([a-z ]+)"[^>]*>([\s\S]*?)<\/div>/g)].map(m => ({
    kind: m[1].includes('alert') ? 'alert' : 'down',
    text: text(m[2].replace(/<button[\s\S]*?<\/button>/g, '').replace(/<small>[\s\S]*?<\/small>/g, '')),
    ago: text((m[2].match(/<small>([\s\S]*?)<\/small>/) || [])[1]) || null,
    buttons: [...m[2].matchAll(/<button[^>]*data-act="([^"]+)"[^>]*>([\s\S]*?)<\/button>/g)].map(b => `${b[1]}:${text(b[2])}`),
  }));
}
for (const [name, err, alerts] of [['none', '', []], ['down', 'Can’t reach the task board server.', []], ['one task alert', '', [ALERTS[0]]], ['one goal alert', '', [ALERTS[1]]],
  ['one plain alert', '', [ALERTS[2]]], ['three alerts', '', ALERTS], ['down and alert', 'The board answered 500.', [ALERTS[0]]]]) {
  const w = web();
  w.set('S.state', state({ alerts }));
  w.set('S.stateErr', err);
  w.run('var __b = ""; patch = (el, html) => { __b = html }');
  w.call('renderBanner');
  add(`banner ${name}`, { fn: 'banner', down: err || null, alerts }, parseBanner(w.run('__b')));
}
{
  const w = web();
  w.set('S.state', state({ alerts: ALERTS }));
  const html = w.call('alertsDialogHtml');
  add('alertsDialog', { fn: 'alertsDialog', alerts: ALERTS }, {
    title: text(html.match(/<h2>([\s\S]*?)<\/h2>/)[1]),
    rows: [...html.matchAll(/<li>([\s\S]*?)<\/li>/g)].map(m => ({
      text: text(m[1].replace(/<button[\s\S]*?<\/button>/g, '').replace(/<small>[\s\S]*?<\/small>/g, '')),
      ago: text(m[1].match(/<small>([\s\S]*?)<\/small>/)[1]),
      buttons: [...m[1].matchAll(/<button[^>]*data-act="([^"]+)"[^>]*>([\s\S]*?)<\/button>/g)].map(b => `${b[1]}:${text(b[2])}`),
    })),
    foot: acts(html.match(/<div class="modal-foot">([\s\S]*?)<\/div>/)[1]),
  });
}

// ---------------------------------------------------------------- work hours menu

function parseHoursMenu(html) {
  const sel = id => {
    const m = html.match(new RegExp(`<select class="select" id="${id}"[^>]*?( disabled)?>([\\s\\S]*?)</select>`));
    return { disabled: !!m[1], options: [...m[2].matchAll(/<option value="([^"]*)"( selected)?>([^<]*)<\/option>/g)].map(o => [o[1], o[3]]), selected: (m[2].match(/<option value="([^"]*)" selected>/) || [])[1] ?? null };
  };
  return {
    on: /id="hours-on" data-hours="on" checked/.test(html),
    check: text(html.match(/<label class="hours-row">([\s\S]*?)<\/label>/)[1]),
    start: sel('hours-start'), end: sel('hours-end'), today: sel('hours-today'),
    days: [...html.matchAll(/data-act="hours-day" data-arg="([a-z]+)" aria-pressed="(true|false)" title="([^"]+)"( disabled)?>([^<]+)<\/button>/g)].map(m => ({ day: m[1], on: m[2] === 'true', title: m[3], disabled: !!m[4], label: m[5] })),
    err: (html.match(/<div class="note err"[^>]*>([\s\S]*?)<\/div>/) || [])[1] || null,
    labels: [...html.matchAll(/<label for="[^"]+">([^<]+)<\/label>/g)].map(m => m[1]),
  };
}
for (const [name, m] of [
  ['open', { x: 10, y: 800, on: true, start: '09:00', end: '17:00', days: ['mon', 'tue', 'wed', 'thu', 'fri'], today: '' }],
  ['off', { x: 10, y: 800, on: false, start: '09:00', end: '17:00', days: ['mon'], today: '' }],
  ['odd times + today + err', { x: 10, y: 800, on: true, start: '08:15', end: '17:45', days: ['sun', 'sat'], today: '16:00', err: '4pm has already passed today.' }],
]) {
  const w = web();
  w.set('S.state', state({}));
  w.set('S.hoursMenu', m);
  add(`hoursMenu ${name}`, { fn: 'hoursMenu', menu: m }, parseHoursMenu(w.call('hoursMenuHtml')));
}
// What a change posts (saveHours): today_until only when it changed ('' → 'off').
for (const [name, m, cur] of [
  ['no today change', { on: true, start: '09:00', end: '17:00', days: ['mon'], today: '' }, null],
  ['set today', { on: true, start: '09:00', end: '17:00', days: ['mon'], today: '16:00' }, null],
  ['clear today', { on: true, start: '09:00', end: '17:00', days: ['mon'], today: '' }, '16:00'],
  ['same today', { on: false, start: '08:00', end: '18:30', days: [], today: '16:00' }, '16:00'],
]) {
  const w = web();
  w.set('S.state', state({ work_hours: { ...HOURS_OPEN, today_until: cur } }));
  w.set('S.hoursMenu', m);
  w.run('var __p = []; api = (p, b) => { __p.push([p, b]); return new Promise(() => {}) }; renderAll = () => {}');
  w.call('saveHours');
  await new Promise(r => setTimeout(r, 0));
  await new Promise(r => setTimeout(r, 0));
  add(`saveHours ${name}`, { fn: 'saveHours', menu: m, today_until: cur }, w.run('__p'));
}
// Opening the menu copies work_hours into it.
{
  const w = web();
  w.set('S.state', state({ work_hours: HOURS_UNTIL }));
  w.run('renderAll = () => {}; var __el = { getBoundingClientRect: () => ({ left: 5, top: 700 }) }; openHoursMenu(__el)');
  const m = w.run('S.hoursMenu');
  add('openHoursMenu', { fn: 'openHoursMenu', hours: HOURS_UNTIL }, { on: m.on, start: m.start, end: m.end, days: m.days, today: m.today });
}

// ---------------------------------------------------------------- page title

for (const [page, needs] of [['board', 0], ['board', 3], ['goal', 0], ['goal', 2], ['backlog', 1], ['sessions', 0]]) {
  const w = web();
  w.set('S.state', state({ counts: { needs, open_issues: 0, done_hidden: 0 } }));
  w.run(`S.route = { page: ${JSON.stringify(page)}, id: null, q: {} }; renderBanner = () => {}; patch = () => {}; renderMenus = () => {}; renderPage = () => {}; renderPanel = () => {}; document = { title: '', querySelector: () => null, querySelectorAll: () => [] }`);
  w.call('renderAll');
  add(`title ${page} ${needs}`, { fn: 'title', page, needs }, w.run('document.title'));
}

// ---------------------------------------------------------------- mdLite

const MD = [
  'Plain text',
  'Line one\nline two\n\nNew para',
  '# Heading\n## Sub *it*\n####### not a heading\n#nospace',
  '- a\n- b\n  continued\n* c\n+ d\n\n1. one\n2) two\n3. three',
  '- list\nthen para',
  '1. num\n- bullet switch',
  'para\n- list after para',
  '```\ncode <b>\n  indented\n```\nafter',
  '```\nunclosed fence',
  'Use `cargo test` and **bold** and *italic* and snake_case_word and 2*3*4',
  '**bold `code` inside** and `**not bold?**`',
  'A [link](https://example.com/a?b=1) and https://x.io/path. and (https://y.io/q), end https://z.io',
  'Not a link: [x](ftp://nope) and http://ok.io/a.b;',
  '*start* and mid*dle*word and *a*b',
  '<script>alert(1)</script> & "quotes" \'single\'',
  'x\r\ny\r\n\r\nz',
  '   leading spaces kept\n\ttab line',
  '- item with `code` and [l](https://l.io)\n- **b**',
  '#\n# \n#  spaced heading',
  'url at start https://a.io/x) trailing',
  'nested **bold *italic* bold**',
  '***triple***',
  '```js\nconst a = 1;\n```',
  'trailing spaces   \n  \nafter blank-ish',
];
for (const [i, s] of MD.entries()) {
  const w = web();
  add(`mdLite ${i}`, { fn: 'mdLite', text: s, max: 1200 }, w.call('mdLite', s));
}
const LONG = Array.from({ length: 40 }, (_, i) => `Line ${i} with some words to make it long enough`).join('\n');
for (const max of [100, 300, 1200]) {
  const w = web();
  add(`mdLite long max ${max}`, { fn: 'mdLite', text: LONG, max }, w.call('mdLite', LONG, max));
}
{
  const w = web();
  const noNl = 'x'.repeat(250);
  add('mdLite cut without newline', { fn: 'mdLite', text: noNl, max: 100 }, w.call('mdLite', noNl, 100));
}

writeGolden('chrome', cases);
