'use strict';

const P = {
  goal: null, goalErr: '',
  backlog: null, backlogErr: '', openAll: null,
  blFilter: { project: 'all', goal: 'all', kind: 'all', state: 'open', sort: 'new' },
  kept: {},
  order: [],
  showDropped: false,
  gnav: null,
  sel: new Set(), selAnchor: null,
};
const pct = (n, d) => d ? `${Math.round((n / d) * 100)}%` : '0%';
const firstUpper = s => s ? s[0].toUpperCase() + s.slice(1) : s;

function resetPageState(prev, next) {
  if (prev.page !== next.page || prev.id !== next.id) { P.kept = {}; P.order = []; P.showDropped = false; P.sel.clear(); P.selAnchor = null; }
  if (next.page !== 'goal' || !P.goal || !sameRef(P.goal, next.id, 'G')) { P.goal = null; P.goalErr = ''; }
}
function pageLoads() {
  const r = S.route;
  if (r.page === 'goal' && r.id) return [loadGoal(r.id)];
  if (r.page === 'backlog') return [loadBacklog()];
  return [];
}
function afterLoad() {
  const r = S.route;
  if (r.page === 'goal' && !r.id && S.goalsAll && S.goalsAll.length) {
    const g = S.goalsAll.find(x => !x.archived) || S.goalsAll[0];
    setTimeout(() => nav(routeHash('goal', ref(g, 'G'), r.q), { replace: true }), 0);
  }
}
const currentGoal = () => (P.goal && sameRef(P.goal, S.route.id, 'G') ? P.goal : null);
function keepBacklogRow(r) {
  const rows = S.route.page === 'goal' ? goalBacklogRows() : S.route.page === 'backlog' ? backlogRows() : [];
  const b = rows.find(x => sameRef(x, r, 'B'));
  if (b) P.kept[r] = b;
}
function withKept(list) {
  const rows = list.slice();
  const have = new Set(rows.map(b => ref(b, 'B')));
  P.order.forEach((r, i) => {
    if (!have.has(r) && P.kept[r]) { rows.splice(Math.min(i, rows.length), 0, P.kept[r]); have.add(r); }
  });
  return rows;
}
async function refreshKept(list) {
  const missing = Object.keys(P.kept).filter(k => !list.some(b => sameRef(b, k, 'B')));
  await Promise.all(missing.map(async k => {
    try { const b = await api('/backlog/' + encodeURIComponent(k)); P.kept[k] = (b && b.issue) || b; } catch {}
  }));
  list.forEach(b => { const k = ref(b, 'B'); if (P.kept[k]) P.kept[k] = b; });
}

async function loadGoal(id) {
  try {
    const g = await api('/goals/' + encodeURIComponent(id));
    if (S.route.id !== id) return;
    const goal = g && g.goal ? { ...g.goal, ...g } : g;
    await refreshKept(goal.backlog || []);
    P.goal = goal; P.goalErr = '';
  } catch (e) { if (S.route.id === id) P.goalErr = e.message; }
}
function goalBacklogRows() {
  const g = currentGoal();
  if (!g) return [];
  return withKept(g.backlog || []).filter(b => P.showDropped || b.state !== 'drop' || P.kept[ref(b, 'B')]);
}
function goalState(tasks, g = {}) {
  if (tasks.length && tasks.every(t => t.status === 'done')) {
    const open = tasks.filter(awaitingMerge);
    if (open.some(prStopped)) return { label: 'Waiting on you', cls: 'st-needs' };
    if (open.length) return { label: open.length > 1 ? `${open.length} PRs awaiting merge` : 'Awaiting merge', cls: 'st-working' };
    return { label: 'Done', cls: 'st-done' };
  }
  if (g.deprioritized) return { label: 'Deprioritized', cls: 'st-queued' };
  if (g.paused) return { label: 'Paused', cls: 'st-needs' };
  if (tasks.some(t => t.lost)) return { label: 'Needs a restart', cls: 'st-failed' };
  if (tasks.some(t => t.status === 'needs')) return { label: 'Waiting on you', cls: 'st-needs' };
  if (tasks.some(t => t.status === 'working')) return { label: 'In progress', cls: 'st-working' };
  const queued = tasks.filter(t => t.status === 'queued');
  if (queued.some(t => t.blocked)) return { label: 'Blocked', cls: 'st-blocked' };
  if (queued.length) return { label: 'Queued', cls: 'st-working' };
  return { label: tasks.length ? 'Not started' : 'No tasks yet', cls: 'st-queued' };
}
function goalCounts(g) {
  const tasks = g.tasks || [];
  const prs = g.tasks ? tasks.filter(awaitingMerge).length : (g.prs_open || []).length;
  const c = !g.tasks ? { n: g.total || 0, done: g.done || 0, active: g.active || 0, queued: 0, prs } : {
    n: tasks.length,
    done: tasks.filter(t => t.status === 'done').length,
    active: tasks.filter(t => t.status === 'working' || t.status === 'needs').length,
    queued: tasks.filter(t => t.status === 'queued').length,
    prs,
  };
  return { ...c, finished: !!(c.n && c.done === c.n && !c.prs) };
}

function renderGoalPage(page) {
  const goals = S.goalsAll ? S.goalsAll.filter(x => !x.archived) : null;
  const cur = S.route.id;
  if (!cur && goals && goals.length) {
    const first = goalByRef(S.filter.goal) || goals.filter(x => gnavView(x) === 'active')
      .sort((a, b) => (a.project || '').localeCompare(b.project || ''))[0] || goals[0];
    nav(`#/goals/${ref(first, 'G')}`, { replace: true });
    return;
  }
  const g = currentGoal();
  const nav = `<nav class="gnav" aria-label="Goals">
    <a class="back" href="#/">${ICON.back}Task board</a>
    <div class="gnav-head"><h1>Goals</h1><span class="grow"></span>${btn('New goal', 'new-goal', { cls: 'soft sm', icon: ICON.plus })}</div>
    ${goals == null ? '<p class="muted">Loading…</p>'
      : goals.length ? goalNavList(goals, cur)
      : '<p class="help">No goals yet. A goal groups tasks that share notes, decisions and a backlog.</p>'}
  </nav>`;
  let main;
  if (!cur) main = `<main class="gmain"><p class="muted">${goals && !goals.length ? 'No goals yet.' : 'Loading…'}</p></main>`;
  else if (!g) main = `<main class="gmain"><p class="${P.goalErr ? 'note err' : 'muted'}">${esc(P.goalErr || 'Loading…')}</p></main>`;
  else main = goalMain(g);
  patch(page, `<div class="gpage">${nav}${main}</div>`);
  page.querySelectorAll('input[data-mixed]').forEach(n => { n.indeterminate = true; });
}
const GNAV_VIEWS = [['active', 'Active'], ['deprio', 'Deprioritized'], ['done', 'Finished']];
const gnavView = g => goalDone(g) ? 'done' : g.deprioritized ? 'deprio' : 'active';
function goalNavList(goals, cur) {
  const sets = {
    active: goals.filter(g => gnavView(g) === 'active'),
    deprio: goals.filter(g => gnavView(g) === 'deprio'),
    done: goals.filter(goalDone).sort((a, b) => (Date.parse(b.finished_at) || 0) - (Date.parse(a.finished_at) || 0)),
  };
  const g = goals.find(x => sameRef(x, cur, 'G'));
  if (g && (!P.gnav || P.gnav.for !== ref(g, 'G'))) P.gnav = { view: gnavView(g), for: ref(g, 'G') };
  const view = (P.gnav && P.gnav.view) || 'active';
  const list = sets[view];
  const projects = [...new Set(list.map(x => x.project || ''))].sort((a, b) => a.localeCompare(b));
  const tabs = `<div class="seg sm gn-views" role="group" aria-label="Which goals">${GNAV_VIEWS.map(([k, l]) =>
    `<button type="button" data-act="gnav-view" data-arg="${k}" aria-pressed="${view === k}">${l}<span class="count">${sets[k].length}</span></button>`).join('')}</div>`;
  const body = list.length ? projects.map(p => {
    const gs = list.filter(x => (x.project || '') === p);
    return `<div class="gn-group"><div class="gn-proj"><span>${esc(p || 'No project')}</span><span class="bg-n">${gs.length}</span></div>
      <div class="gn-list">${gs.map(x => goalItem(x, cur)).join('')}</div></div>`;
  }).join('') : `<p class="help">${view === 'deprio' ? 'No deprioritized goals' : view === 'done' ? 'No finished goals yet' : 'No active goals'}</p>`;
  return tabs + body;
}
function goalNavStatus(x, c) {
  if (c.n && c.done === c.n) return c.prs ? { k: 'run', label: c.prs > 1 ? `${c.prs} PRs awaiting merge` : 'Awaiting merge' } : { k: 'done', label: 'Finished' };
  if (x.needs) return { k: 'warn', label: x.needs > 1 ? `${x.needs} need you` : 'Needs you' };
  if (x.paused) return { k: 'warn', label: 'Paused' };
  const running = c.active - (x.needs || 0);
  if (running) return { k: 'run', label: running > 1 ? `${running} running` : 'Running' };
  if (x.queued) return x.blocked ? { k: 'warn', label: 'Blocked' } : { k: 'queued', label: 'Queued' };
  if (!c.n) return { k: 'idle', label: 'No tasks yet' };
  return { k: 'idle', label: c.done ? 'Not running' : 'Not started' };
}
function goalItem(x, cur) {
  const c = goalCounts(x);
  const s = goalNavStatus(x, c);
  const st = goalStatus(x, c);
  const dot = s.k === 'idle' ? '' : '<i aria-hidden="true"></i>';
  return `<a class="gitem" href="#/goals/${esc(ref(x, 'G'))}" aria-current="${sameRef(x, cur, 'G')}" data-peek="${esc(ref(x, 'G'))}">${goalRing(c, st)}
    <span class="gi-txt"><b><span class="gi-ref">${esc(ref(x, 'G'))}</span>${esc(x.name)}</b><span class="gi-st ${s.k}">${dot}${esc(s.label)}<span class="gi-n">· ${c.done} of ${c.n} done</span></span></span></a>`;
}
function goalMain(g) {
  const gr = ref(g, 'G');
  const tasks = g.tasks || [];
  const c = goalCounts(g);
  const openIssues = (g.backlog || []).filter(b => (b.state || 'open') === 'open').length;
  const view = S.route.q.view === 'backlog' ? 'backlog' : 'tasks';
  const gs = goalState(tasks, g);
  const epic = g.epic_key
    ? `Jira epic ${g.epic_url ? `<a class="epic" href="${esc(safeUrl(g.epic_url))}" target="_blank" rel="noopener">${esc(g.epic_key)}</a>` : `<span class="epic">${esc(g.epic_key)}</span>`}${g.epic_status ? ' · ' + esc(g.epic_status) : ''}`
    : jiraOn() ? 'No Jira epic' : '';
  const views = [['tasks', 'Tasks', c.n, false], ['backlog', 'Backlog', openIssues, openIssues > 0]].map(([id, label, n, hot]) =>
    `<button type="button" role="tab" aria-selected="${view === id}" data-act="goal-view" data-arg="${id}">${label}<span class="count${hot ? ' hot' : ''}">${n}</span></button>`).join('');
  const tools = view === 'tasks' ? btn('Add a task', 'add-task', { id: gr, cls: 'soft md', icon: ICON.plus }) : btn('Add an issue', 'add-issue', { id: gr, cls: 'soft md' });
  return `<main class="gmain">
    <header class="ghead">
      <div class="row" style="gap:10px"><span class="pill goal">${ICON.flag}Goal</span><span class="pill ${gs.cls}">${gs.label}</span><span class="pill ref">${esc(gr)}</span>${btn('Edit', 'goal-edit', { id: gr, cls: 'ghost sm', title: 'Edit the goal’s name, TLDR, outcome and project' })}<div class="grow"></div>${goalRunButtons(g)}</div>
      <h2>${esc(g.name)}</h2>
      ${g.tldr ? `<p class="tldr"><b>TLDR</b> ${esc(g.tldr)}</p>` : ''}
      <div class="meta"><span class="chip repo">${esc(g.project || '')}</span>${epic ? `<span>${epic}</span>` : ''}<span>${c.done} of ${c.n} done · ${c.active} active${(p => p ? ' · ' + p : '')(goalPrCount(tasks))} · ${openIssues} open in the backlog</span></div>
      <div class="bar lg"><span class="d" style="width:${pct(c.done, c.n)}"></span><span class="a" style="width:${pct(c.active, c.n)}"></span><span class="q" style="width:${pct(c.queued, c.n)}"></span></div>
    </header>
    <div class="gbody">
      <section class="gsec" aria-label="${view === 'tasks' ? 'Tasks in this goal' : 'The goal’s backlog'}">
        <div class="gbar"><div class="seg sm" role="tablist" aria-label="Tasks or backlog">${views}</div><div class="grow"></div>${tools}</div>
        <div class="gsec-body">${view === 'tasks' ? goalTasks(g) : goalBacklog(g)}</div>
      </section>
      <div class="stack sticky" style="gap:16px">${view === 'tasks' ? attachAside(g) + notesAside(g) : issueAside(false)}</div>
    </div>
  </main>`;
}

function goalTaskMeta(t, i, tasks, g) {
  const prevOpen = () => { for (let j = i - 1; j >= 0; j--) if (tasks[j].status !== 'done') return j + 1; return 0; };
  const key = t.jira && t.jira.key;
  let parts;
  if (t.status === 'done') parts = [t.who, `${t.failed ? 'stopped' : 'finished'} ${ago(t.when)}`, key];
  else if (t.lost) parts = ['Terminal lost', t.who];
  else if (t.status === 'working' || t.status === 'needs') parts = [t.who, runningFor(t), `${t.question ? 'asked' : 'updated'} ${ago(t.when)}`, key];
  else if (t.status === 'queued' && t.starting) parts = [t.priority === 'high' && 'High', 'starting', key];
  else if (t.status === 'queued' && t.waiting) parts = [t.priority === 'high' && 'High', t.waiting, key];
  else if (t.status === 'queued') { const p = g.run_in_order ? prevOpen() : 0; parts = [t.priority === 'high' && 'High', p ? `after task ${p}` : 'starts when a terminal is free', key]; }
  else { const p = g.run_in_order ? prevOpen() : 0; parts = [p && `after task ${p}`]; }
  const list = parts.filter(Boolean);
  if (list[0] && list[0] !== t.who) list[0] = firstUpper(list[0]);
  return list.join(' · ');
}
const PR_STAGE_ICON = (() => {
  const svg = d => `<svg class="pr-stage" viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${d}</svg>`;
  const clock = svg('<circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/>');
  return {
    checks: clock,
    fix: svg('<path d="M14.5 6.5a4 4 0 0 0 5 5L12 19a2.1 2.1 0 0 1-3-3l7.5-7.5a4 4 0 0 0-2-2z"/>'),
    review: svg('<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="2.8"/>'),
    comments: svg('<path d="M4 5h16v11H9l-5 4z"/>'),
    merge: svg('<circle cx="6" cy="5" r="2.3"/><circle cx="6" cy="19" r="2.3"/><circle cx="18" cy="12" r="2.3"/><path d="M6 7.3v9.4M6 7.3c0 3 3 4.7 9.7 4.7"/>'),
    merged: svg('<path d="M5 12.5l4.5 4.5L19 7.5"/>'),
    declined: svg('<path d="M6 6l12 12M18 6 6 18"/>'),
  };
})();
function prMark(t, link) {
  if (!t.pr || t.pr.num == null) return '';
  const st = t.pr.stage;
  const cls = `pr-at open${st ? ' pr-' + esc(st.phase) : ''}`;
  const tip = `PR #${t.pr.num} · ${st ? st.label + ' · ' : ''}checks ${checksOf(t.pr)[0].toLowerCase()}`;
  const inner = `${ICON.prOpen}#${esc(t.pr.num)}${st ? `${PR_STAGE_ICON[st.phase] || ''}<span class="sr">${esc(st.label)}</span>` : ''}`;
  return link && t.pr.url ? `<a class="${cls}" href="${esc(safeUrl(t.pr.url))}" target="_blank" rel="noopener" title="${esc(tip + ' · open the PR')}">${inner}</a>`
    : `<span class="${cls}" title="${esc(tip)}">${inner}</span>`;
}
function goalPrCount(tasks) {
  const prs = tasks.filter(t => t.pr && t.pr.num != null);
  const merged = prs.filter(t => (t.pr.state || '').toUpperCase() === 'MERGED').length;
  return !prs.length ? '' : merged ? `${merged} of ${plural(prs.length, 'PR')} merged` : `${plural(prs.length, 'PR')} open`;
}
function goalTasks(g) {
  const gr = ref(g, 'G');
  const tasks = g.tasks || [];
  const rowHtml = (t, i) => {
    const r = ref(t, 'T');
    const k = stKey(t);
    return `<li class="trow${t.status === 'planned' ? ' planned' : ''}"><span class="n" aria-hidden="true"><span class="num">${i + 1}</span></span>
      <button type="button" class="main" data-act="open-task" data-id="${esc(r)}"><span class="l1"><span class="chip st-${k}">${STATUS[k]}</span>${t.jira && t.jira.key ? `<span class="jira-at" title="${esc(t.jira.key + (t.jira.status ? ' · ' + t.jira.status : ''))}">${ICON.jiraMark}<span class="sr">Jira ${esc(t.jira.key)}</span></span>` : ''}${prMark(t)}<b>${esc(t.title)}</b></span>${(m => m ? `<span class="m">${esc(m)}</span>` : '')(goalTaskMeta(t, i, tasks, g))}</button></li>`;
  };
  const max = Number(g.max_terminals) || 2;
  return `${gateBanner(g)}${tasks.length ? `<ol class="rows trows">${tasks.map(rowHtml).join('')}</ol>` : '<p class="empty-box">No tasks in this goal yet. Add one, or let Claude plan them.</p>'}
    <div class="card-box"><h3 class="h3">How this goal runs</h3>
      <div class="row" style="gap:10px"><label for="max-terms" style="font-size:14px">At most</label>
        <select id="max-terms" class="select sm" data-change="goal-set" data-field="max_terminals" data-id="${esc(gr)}" data-grp="gset:${esc(gr)}">${[1, 2, 3, 4, 5, 6, 8].map(n => opt(n, plural(n, 'terminal'), max)).join('')}</select>
        <span style="font-size:13px;color:var(--muted)">working on this goal at once</span></div>
      <label class="check" style="font-size:14px;color:var(--text)"><input type="checkbox" data-change="goal-set" data-field="run_in_order" data-id="${esc(gr)}" data-grp="gset:${esc(gr)}"${g.run_in_order === 0 || g.run_in_order === false ? '' : ' checked'}>Run the tasks in order, one after another</label>
      <label class="check" style="font-size:14px;color:var(--text)"><input type="checkbox" data-change="goal-set" data-field="auto_close" data-id="${esc(gr)}" data-grp="gset:${esc(gr)}"${g.auto_close === 0 || g.auto_close === false ? '' : ' checked'}>Close each terminal when its task is done</label>
      ${note('gset:' + gr)}
    </div>
`;
}

function noteFrom(n) {
  const at = n.at ? hhmm(n.at) : '';
  if (!n.source) return at;
  return n.source === 'you' ? `you${at ? ', ' + at : ''}` : n.source;
}
const NOTE_KIND = { finding: 'Finding', decision: 'Decision', reference: 'Reference' };
const noteLong = t => t.length > 220 || t.split('\n').length > 4;
function noteItem(n) {
  const t = String(n.text || '').trim();
  const pin = n.pinned ? '<span class="pin">Pinned</span> ' : '';
  if (!noteLong(t)) return `<li><span class="note-t">${pin}${esc(t)}</span> <small>${esc(noteFrom(n))}</small></li>`;
  return `<li><span class="note-t clamp">${pin}${esc(t)}</span>
    <span class="note-more"><button type="button" class="btn link sm" data-act="gnote-open" data-id="${esc(n.id)}" aria-haspopup="dialog">Show all</button> <small>${esc(noteFrom(n))}</small></span></li>`;
}
const INLINE = /(https?:\/\/[^\s<]+[^\s<.,;:)\]'"])|`([^`\n]+)`|((?:[\w.-]+|\.\.\.)\/(?:[\w.-]+\/|\.\.\.\/)*(?:[\w-]+(?:\.[\w-]+)*)?|\b[\w-]+\.(?:dart|kt|kts|java|swift|py|js|mjs|ts|tsx|jsx|json|ya?ml|md|sh|gradle|xml|html|css|toml|lock)\b)/g;
const pathLike = p => /\/$|\.\w+$|[_-]|\.\.\./.test(p);
function inlineText(s) {
  let out = '', at = 0;
  for (const m of String(s).matchAll(INLINE)) {
    const [whole, url, tick, path] = m;
    if (path && !pathLike(path)) continue;
    out += esc(s.slice(at, m.index));
    out += url ? `<a href="${esc(url)}" target="_blank" rel="noopener">${esc(url)}</a>` : `<code>${esc(tick || path)}</code>`;
    at = m.index + whole.length;
  }
  return out + esc(s.slice(at));
}
function noteBody(text) {
  const link = inlineText;
  const out = [];
  let kind = null, buf = [];
  const flush = () => {
    if (!buf.length) return;
    if (kind === 'li') out.push(`<ul>${buf.map(l => `<li>${link(l)}</li>`).join('')}</ul>`);
    else if (kind === 'code') out.push(`<pre>${esc(buf.join('\n'))}</pre>`);
    else out.push(`<p>${buf.map(link).join('<br>')}</p>`);
    buf = [];
  };
  for (const raw of String(text || '').split('\n')) {
    const line = raw.replace(/\s+$/, '');
    const k = !line.trim() ? null : /^\s*[-*•] /.test(line) ? 'li' : /^(\s{2,}|\t)/.test(line) ? 'code' : 'p';
    if (k === 'code' && kind === 'li' && buf.length) { buf[buf.length - 1] += ' ' + line.trim(); continue; }
    if (k !== kind || k === null) flush();
    kind = k;
    if (k === 'li') buf.push(line.replace(/^\s*[-*•] /, ''));
    else if (k === 'code') buf.push(line.replace(/^(\s{2}|\t)/, ''));
    else if (k) buf.push(line.trim());
  }
  flush();
  return out.join('');
}
function noteDialogHtml(m) {
  const n = m.note;
  const meta = [NOTE_KIND[n.kind] || 'Finding', noteFrom(n)].filter(Boolean).join(' · ');
  return `<div class="overlay" data-act="modal-bg"><div class="modal narrow" role="dialog" aria-modal="true" aria-label="Goal note">
    <div class="modal-head"><h2>${n.pinned ? '<span class="pin">Pinned</span> ' : ''}${esc(meta)}</h2>${closeBtn('modal-close')}</div>
    <div class="note-body">${noteBody(n.text)}</div>
  </div></div>`;
}
function attachAside(g) {
  const a = attachList(g.attachments, 'goals', ref(g, 'G'));
  return `<aside class="aside-card" aria-label="Attached to this goal">
    <div class="row" style="flex-wrap:nowrap"><h3 class="h3 grow">Attached</h3></div>
    ${note(a.fk)}
    ${a.items ? `<ul class="atts">${a.items}</ul>` : '<p style="font-size:13px;color:var(--muted)">Designs, proposals and docs for the whole goal. Every task’s handoff lists them.</p>'}
  </aside>`;
}
function notesAside(g) {
  const gr = ref(g, 'G');
  const notes = (g.notes || []).slice().sort((a, b) => (b.pinned ? 1 : 0) - (a.pinned ? 1 : 0) || String(a.at).localeCompare(String(b.at)));
  P.notes = notes;
  const groups = [['finding', 'Findings'], ['decision', 'Decisions'], ['reference', 'References']]
    .map(([k, title]) => [title, notes.filter(n => (['decision', 'reference'].includes(n.kind) ? n.kind : 'finding') === k)])
    .filter(([, l]) => l.length);
  const fk = 'gnote:' + gr;
  const kind = S.drafts['gnotekind:' + gr] || 'finding';
  const form = S.reveal[fk] ? `<div class="stack" style="gap:8px">
      <div class="seg sm" role="group" aria-label="Kind of note">${[['finding', 'Finding'], ['decision', 'Decision'], ['reference', 'Reference']].map(([id, l]) => `<button type="button" data-act="gnote-kind" data-id="${esc(gr)}" data-arg="${id}" aria-pressed="${kind === id}">${l}</button>`).join('')}</div>
      <label class="sr" for="gn-${esc(gr)}">Note</label><textarea class="input" id="gn-${esc(gr)}" data-k="${esc(fk)}" rows="3" placeholder="What every task in this goal should know">${esc(S.drafts[fk] || '')}</textarea>
      <div class="row">${btn('Save note', 'gnote-save', { id: gr, cls: 'soft sm', grp: fk })}${btn('Cancel', 'hide', { arg: fk, cls: 'ghost sm' })}</div></div>` : '';
  const open = foldOpen('gnotes');
  return `<aside class="aside-card" aria-label="Goal notes"><details class="card-fold" data-fold="gnotes"${open ? ' open' : ''}>
    <summary><h3 class="h3 grow">Goal notes</h3>${S.reveal[fk] ? '' : btn('Add a note', 'reveal', { arg: fk, cls: 'plain' })}</summary>
    <div class="card-fold-body">
    <p style="font-size:13px;color:var(--muted)">Every task in this goal gets these, and the open backlog, in its handoff. A new terminal starts with what earlier tasks learned.</p>
    ${form}${note(fk)}
    ${groups.length ? groups.map(([title, l]) => `<div class="notes-g"><b>${title}</b><ul>${l.map(noteItem).join('')}</ul></div>`).join('')
      : '<div class="notes-g"><b>Nothing yet</b><ul><li>Notes from tasks and from you collect here.</li></ul></div>'}
    </div></details>
  </aside>`;
}
function goalIssueState(b, g) {
  if (b.state === 'task') {
    const tasks = g.tasks || [];
    const i = tasks.findIndex(t => t.id === b.task_id);
    if (i >= 0) return `Now task ${i + 1} · ${STATUS[stKey(tasks[i])].toLowerCase()}`;
  }
  const info = issueState(b);
  return info ? info.text : '';
}
const pickableRefs = rows => rows.filter(b => (b.state || 'open') === 'open').map(b => ref(b, 'B'));
function bulkBar(pickable) {
  if (!pickable.length) return '';
  const n = P.sel.size;
  const all = n > 0 && n === pickable.length;
  const box = `<input type="checkbox" data-act="bl-pick-all" aria-label="Select every open issue"${all ? ' checked' : ''}${n && !all ? ' data-mixed="1"' : ''}>`;
  if (!n) return `<div class="bl-head"><label class="bl-all">${box}Select all</label></div>`;
  const busy = [...S.busy].some(k => k.startsWith('bl-bulk:'));
  const b = (label, arg, cls) => btn(label, 'bl-bulk', { arg, cls, grp: 'bulk', disabled: busy });
  return `<div class="bl-head on" role="toolbar" aria-label="Change the selected issues"><label class="bl-all">${box}${esc(n)} selected</label>
    ${b('Make tasks', 'task', 'soft sm')}${jiraOn() ? b('Create tickets', 'ticket', 'sm') : ''}${b('Won’t do', 'drop', 'ghost sm')}
    ${goalButton('bl-bulk-move', '', { target: 'change:bl-bulk-move', placeholder: 'Move to a goal', specials: [['none', 'Not in a goal']], cls: 'sm', grp: 'bulk', disabled: busy })}
    ${note('bulk')}<div class="grow"></div>${btn('Clear', 'bl-pick-clear', { cls: 'link', disabled: busy })}</div>`;
}
function goalBacklog(g) {
  const rows = goalBacklogRows();
  P.order = rows.map(b => ref(b, 'B'));
  const pickable = pickableRefs(rows);
  P.sel.forEach(r => { if (!pickable.includes(r)) P.sel.delete(r); });
  const sel = S.issueRef;
  const closed = g.closed_count != null ? g.closed_count : (g.backlog || []).filter(b => b.state === 'drop').length;
  const items = rows.map(b => {
    const r = ref(b, 'B');
    const on = sameRef(sel, r, 'B');
    const open = (b.state || 'open') === 'open';
    const info = issueState(b);
    const box = open ? `<input type="checkbox" class="bsel" data-act="bl-pick" data-id="${esc(r)}" aria-label="Select ${esc(r)}"${P.sel.has(r) ? ' checked' : ''}>` : '';
    return `<li class="irow${on ? ' sel' : ''}${P.sel.has(r) ? ' picked' : ''}">${box}<button type="button" class="main" data-act="pick-issue" data-id="${esc(r)}" aria-pressed="${on}">
        <span class="l1"><span class="chip k-${esc(b.kind)}">${esc(kindLabel(b.kind))}</span><b>${esc(b.title)}</b></span><span class="from">${esc(issueFrom(b, true))}</span></button>
      ${open ? `<span class="acts">${issueActions(b, 'goal', { small: true })}</span>` : info ? `<span class="state ${info.cls}">${esc(goalIssueState(b, g))}</span>` : ''}${note('issue:' + r)}</li>`;
  }).join('');
  const closedLine = !rows.length && !closed ? 'Nothing in the backlog yet. Issues that terminals find will collect here.'
    : closed ? `${closed} closed as won’t do · ` : '';
  return `<p style="font-size:13px;color:var(--muted)">Issues found while testing, building or reviewing that no task covers yet. Terminals add them instead of fixing them on the side.</p>
    ${rows.length ? `${bulkBar(pickable)}<ul class="rows${pickable.length ? ' picks' : ''}">${items}</ul>` : ''}
    ${closedLine ? `<span class="help">${esc(closedLine)}${closed ? `<button type="button" class="btn link" data-act="show-dropped" style="font-size:12.5px;min-height:0">${P.showDropped ? 'Hide' : 'Show'}</button>` : ''}</span>` : ''}`;
}
function issueAside(showMove) {
  if (!S.issueRef) return '';
  const b = S.issue;
  if (!b) return `<aside class="aside-card" aria-label="Issue"><p class="${S.issueErr ? 'note err' : 'muted'}">${esc(S.issueErr || 'Loading…')}</p></aside>`;
  const info = issueState(b);
  return `<aside class="aside-card" aria-label="Issue">
    <div class="stack"><span class="row" style="gap:8px;flex-wrap:nowrap"><span class="chip k-${esc(b.kind)}">${esc(kindLabel(b.kind))}</span><span class="help grow">${esc(info ? info.text : `Open · reported at ${hhmm(b.created_at)}`)}</span><span class="pill ref">${esc(ref(b, 'B'))}</span></span>
      <h2 class="it">${esc(b.title)}</h2>${b.state === 'task' && b.task_id != null ? `<a href="${esc(hashWith({ task: 'T' + b.task_id, tab: null }))}" style="font-size:12.5px;align-self:flex-start">Open task T${esc(b.task_id)}</a>` : ''}</div>
    ${S.issueErr ? `<p class="note err">${esc(S.issueErr)}</p>` : ''}
    ${issueDetail(b, { move: showMove, compact: true })}
  </aside>`;
}

const BL_STATES = [['open', 'Open'], ['task', 'Planned task'], ['ticket', 'Jira ticket'], ['drop', 'Won’t do'], ['all', 'All']];
const BL_KINDS = [['all', 'All'], ['bug', 'Bug'], ['gap', 'Test gap'], ['follow', 'Follow-up'], ['clean', 'Clean-up']];
const isDefaultBl = f => f.project === 'all' && f.goal === 'all' && f.kind === 'all' && f.state === 'open';
async function loadBacklog() {
  const f = P.blFilter;
  const goal = f.goal === 'all' || f.goal === 'none' ? f.goal : String(idOf(f.goal));
  const q = new URLSearchParams({ project: f.project, goal, kind: f.kind, state: f.state, sort: f.sort });
  try {
    const [r, all] = await Promise.all([
      api('/backlog?' + q),
      isDefaultBl(f) ? null : api('/backlog?' + new URLSearchParams({ project: 'all', goal: 'all', kind: 'all', state: 'open', sort: 'new' })).catch(() => null),
    ]);
    const issues = Array.isArray(r) ? r : (r && r.issues) || [];
    await refreshKept(issues);
    P.backlog = { issues, total: r && r.total != null ? r.total : issues.length };
    P.openAll = isDefaultBl(f) ? P.backlog.total : all ? (all.total ?? (all.issues || []).length) : P.openAll;
    P.backlogErr = '';
  } catch (e) { P.backlogErr = e.message; }
}
function backlogRows() { return P.backlog ? withKept(P.backlog.issues) : []; }
function blStateText(b) {
  if (b.state === 'task') return issueGoalId(b) != null ? 'Planned task in its goal' : 'Queued task, no goal';
  const info = issueState(b);
  return info ? (b.state === 'drop' ? 'Won’t do' : info.text) : '';
}
function renderBacklogPage(page) {
  const f = P.blFilter;
  const rows = backlogRows();
  P.order = rows.map(b => ref(b, 'B'));
  const projects = projectNames();
  if (f.project !== 'all' && !projects.includes(f.project)) projects.push(f.project);
  const goals = allGoals();
  const sel = S.issueRef;
  const open = P.openAll ?? (S.state && S.state.counts ? S.state.counts.open_issues : null);
  const list = !P.backlog
    ? `<p class="${P.backlogErr ? 'note err' : 'muted'}">${esc(P.backlogErr || 'Loading…')}</p>`
    : rows.length ? `<ul class="rows">${rows.map(b => {
        const r = ref(b, 'B');
        const on = sameRef(sel, r, 'B');
        const gname = issueGoalName(b);
        const info = issueState(b);
        const isOpen = (b.state || 'open') === 'open';
        return `<li class="brow${on ? ' sel' : ''}"><button type="button" class="main" data-act="pick-issue" data-id="${esc(r)}" aria-pressed="${on}">
            <span class="l1"><span class="chip k-${esc(b.kind)}">${esc(kindLabel(b.kind))}</span><b>${esc(b.title)}</b></span>
            <span class="l2"><span class="gp${gname ? '' : ' none'}">${esc(gname || 'Not in a goal')}</span><span class="mono">${esc(b.project || '')}</span><span>${esc(issueFrom(b, true))}</span></span></button>
          <span class="acts">${isOpen ? issueActions(b, issueGoalId(b) != null ? 'goal' : 'board', { small: true }) : info ? `<span class="state chip ${info.cls}">${esc(blStateText(b))}</span>` : ''}${note('issue:' + r)}</span></li>`;
      }).join('')}</ul>`
    : '<p class="empty-box">No issues match these filters.</p>';
  const kinds = BL_KINDS.map(([id, l]) => `<button type="button" data-act="bl-kind" data-arg="${id}" aria-pressed="${f.kind === id}">${l}</button>`).join('');
  const sortSel = [['new', 'Newest'], ['old', 'Oldest'], ['kind', 'Type']].map(([id, l]) => opt(id, l, f.sort)).join('');
  patch(page, `<div class="bpage">
    <header class="top"><a class="back" href="#/" style="min-height:40px">${ICON.back}Task board</a><h1>Backlog</h1>${open != null ? `<span class="pill st-needs">${esc(open)} open</span>` : ''}
      <div class="grow"></div><a class="goals-link" href="#/goals">Goals</a>
      <button type="button" class="btn primary" data-act="add-issue">Add an issue</button></header>
    <div class="bfilters" role="group" aria-label="Filters">
      <label for="bl-project">Project</label>${projectButton('bl-project', f.project, { target: 'change:bl-filter', all: true, style: 'min-width:170px' })}
      <label for="bl-goal" style="margin-left:4px">Goal</label>${goalButton('bl-goal', f.goal, { target: 'change:bl-filter', specials: [['all', 'All goals'], ['none', 'Not in a goal']], project: f.project, style: 'min-width:220px;max-width:320px' })}
      <div class="seg sm" role="group" aria-label="Type" style="margin-left:4px">${kinds}</div>
      <label for="bl-state" style="margin-left:4px">State</label><select id="bl-state" class="select" style="min-width:140px" data-change="bl-filter" data-field="state">${BL_STATES.filter(([id]) => id !== 'ticket' || jiraOn() || f.state === 'ticket').map(([id, l]) => opt(id, l, f.state)).join('')}</select>
      <div class="grow"></div>
      <label for="bl-sort">Sort</label><select id="bl-sort" class="select" data-change="bl-filter" data-field="sort">${sortSel}</select>
    </div>
    <div class="bgrid">
      <section class="stack" aria-label="Issues" style="gap:8px">${P.backlog ? `<span style="font-size:13px;color:var(--muted)">Showing ${rows.length} of ${esc(P.backlog.total)} issue${P.backlog.total === 1 ? '' : 's'}</span>` : ''}${list}</section>
      <div class="sticky">${issueAside(true)}</div>
    </div>
  </div>`);
}

function openNewIssue(goalRef) {
  const g = goalRef ? goalByRef(goalRef) || currentGoal() : null;
  const f = P.blFilter;
  const goal = goalRef ? ref(goalRef, 'G') : (S.route.page === 'backlog' && f.goal !== 'all' && f.goal !== 'none' ? f.goal : '');
  const gg = goal && (goalByRef(goal) || g);
  const projects = projectNames();
  const project = (gg && gg.project) || (S.route.page === 'backlog' && f.project !== 'all' ? f.project : S.filter.project !== 'all' ? S.filter.project : projects[0] || '');
  S.modal = { kind: 'issue', title: '', kindV: 'bug', goal, project, said: '', detail: '', err: '', busy: false };
  renderModal();
  setTimeout(() => { const n = $('#ni-title'); if (n) n.focus(); }, 0);
}
function newIssueHtml(m) {
  const projects = projectNames();
  if (m.project && !projects.includes(m.project)) projects.push(m.project);
  return `<div class="overlay" data-act="modal-bg"><form class="modal narrow" aria-label="Add an issue" data-form="issue" novalidate>
    <div class="modal-head"><h2>Add an issue</h2>${closeBtn('modal-close')}</div>
    <div class="field"><label for="ni-title">Title</label><input id="ni-title" class="input" type="text" data-m="title" autocomplete="off" value="${esc(m.title)}"></div>
    <fieldset class="field"><legend>Type</legend><div class="seg">${seg('kindV', Object.entries(KINDS), m.kindV)}</div></fieldset>
    <div class="two">
      <div class="field"><label for="ni-goal">Goal</label>${goalButton('ni-goal', m.goal || '', { target: 'modal:goal', specials: [['', 'Not in a goal']] })}</div>
      <div class="field"><label for="ni-project">Project</label>${projectButton('ni-project', m.project, { target: 'modal:project' })}</div>
    </div>
    <div class="field"><label for="ni-said">What you saw</label><textarea id="ni-said" class="input" rows="3" data-m="said">${esc(m.said)}</textarea></div>
    <div class="field"><label for="ni-detail">More detail <span style="font-weight:400;color:var(--muted)">(optional)</span></label><textarea id="ni-detail" class="input" rows="3" data-m="detail">${esc(m.detail)}</textarea>
      <span class="help">It goes in the backlog. Nothing starts until you make it a task.</span></div>
    <div class="modal-foot">${m.err ? `<span class="ferr note" role="alert">${esc(m.err)}</span>` : ''}<button type="button" class="btn lg" data-act="modal-close">Cancel</button><button type="submit" class="btn primary lg"${m.busy ? ' disabled' : ''}>${m.busy ? 'Adding…' : 'Add issue'}</button></div>
  </form></div>`;
}
async function submitNewIssue() {
  const m = S.modal;
  const title = (m.title || '').trim();
  m.err = !title ? 'Give the issue a title.' : !m.project ? 'Pick a project.' : '';
  if (m.err) return renderModal();
  m.busy = true;
  renderModal();
  try {
    const b = await post('/backlog', { title, kind: m.kindV, goal_id: m.goal ? idOf(m.goal) : null, project: m.project, said: (m.said || '').trim() || undefined, detail: (m.detail || '').trim() || undefined });
    closeModal();
    const r = b && ref(b.issue || b, 'B');
    toast(r ? `Added ${r}` : 'Issue added');
    if (r && S.route.page !== 'board') nav(hashWith({ issue: r }), { replace: true }); else refresh();
  } catch (e) { if (S.modal === m) { m.busy = false; m.err = e.message; renderModal(); } }
}

function openGoalForm(g, o = {}) {
  const projects = projectNames();
  S.modal = g ? {
    kind: 'goal', id: ref(g, 'G'), name: g.name || '', tldr: g.tldr || '', outcome: g.outcome || '', project: g.project || '',
    epicMode: 'keep', epicKey: g.epic_key || '', run_in_order: g.run_in_order !== 0 && g.run_in_order !== false,
    max_terminals: Number(g.max_terminals) || 2, auto_close: g.auto_close !== 0 && g.auto_close !== false, err: '', busy: false,
  } : {
    kind: 'goal', id: null, name: o.name || '', tldr: '', outcome: '',
    project: o.project || (o.back && o.back.project) || (S.filter.project !== 'all' ? S.filter.project : projects[0] || ''),
    epicMode: 'none', epicKey: '', run_in_order: true, max_terminals: 2, auto_close: true, err: '', busy: false,
    back: o.back || null, after: o.after || null, prefill: o.prefill || '',
  };
  if (!g && !o.name) restoreDraft(S.modal);
  renderModal();
  setTimeout(() => { const n = $('#ng-name'); if (n) n.focus(); }, 0);
}
function goalFormHtml(m) {
  const projects = projectNames();
  if (m.project && !projects.includes(m.project)) projects.push(m.project);
  const epicHelp = { create: 'The board creates the epic in your Jira project. Tasks’ tickets go under it.', link: 'Tasks’ tickets go under this epic.', none: 'You can add an epic later.', keep: '' };
  const epic = !jiraOn() ? '' : m.id
    ? `<div class="field"><label for="ng-epic">Jira epic</label><input id="ng-epic" class="input mono" data-m="epicKey" placeholder="PROJ-123, or leave empty" autocomplete="off" value="${esc(m.epicKey)}"><span class="help">Tasks’ tickets go under this epic.</span></div>`
    : `<fieldset class="field" style="gap:8px"><legend>Jira epic</legend><div class="seg">${seg('epicMode', [['create', 'Create an epic'], ['link', 'Link existing'], ['none', 'None']], m.epicMode)}</div>
        ${m.epicMode === 'link' ? `<label class="sr" for="ng-epic">Epic key</label><input id="ng-epic" class="input mono" data-m="epicKey" placeholder="PROJ-123" autocomplete="off" value="${esc(m.epicKey)}">` : ''}
        <span class="help">${epicHelp[m.epicMode]}</span></fieldset>`;
  return `<div class="overlay" data-act="modal-bg"><form class="modal narrow" aria-label="${m.id ? 'Edit goal' : 'New goal'}" data-form="goal" novalidate>
    <div class="modal-head"><h2>${m.id ? 'Edit goal' : 'New goal'}</h2>${closeBtn('modal-close')}</div>${draftNote(m)}${m.prefill ? `<p class="prefill">${esc(m.prefill)}</p>` : ''}
    <div class="field"><label for="ng-name">Name</label><input id="ng-name" class="input" type="text" data-m="name" autocomplete="off" value="${esc(m.name)}"></div>
    <div class="field"><label for="ng-tldr">TLDR</label><textarea id="ng-tldr" class="input" rows="3" data-m="tldr">${esc(m.tldr || '')}</textarea><span class="help">The problem and what's changing, in plain words. It sits at the top of the goal page.</span></div>
    <div class="field"><label for="ng-outcome">Done when</label><textarea id="ng-outcome" class="input" rows="3" data-m="outcome">${esc(m.outcome)}</textarea><span class="help">What’s true once the goal is finished. Every task in it sees this.</span></div>
    <div class="field"><label for="ng-project">Project</label>${projectButton('ng-project', m.project, { target: 'modal:project' })}</div>
    ${epic}
    <div class="card-box" style="padding:12px 14px">
      <div class="row" style="gap:10px"><label for="ng-max" style="font-size:14px">At most</label><select id="ng-max" class="select sm" data-m="max_terminals">${[1, 2, 3, 4, 5].map(n => opt(n, plural(n, 'terminal'), m.max_terminals)).join('')}</select><span style="font-size:13px;color:var(--muted)">working on this goal at once</span></div>
      <label class="check" style="font-size:14px;color:var(--text)"><input type="checkbox" data-m="run_in_order"${m.run_in_order ? ' checked' : ''}>Run the tasks in order, one after another</label>
      <label class="check" style="font-size:14px;color:var(--text)"><input type="checkbox" data-m="auto_close"${m.auto_close ? ' checked' : ''}>Close each terminal when its task is done</label>
    </div>
    <div class="modal-foot">${m.err ? `<span class="ferr note" role="alert">${esc(m.err)}</span>` : ''}<button type="button" class="btn lg" data-act="modal-close">Cancel</button><button type="submit" class="btn primary lg"${m.busy ? ' disabled' : ''}>${m.busy ? 'Saving…' : m.id ? 'Save goal' : 'Add goal'}</button></div>
  </form></div>`;
}
async function submitGoal() {
  const m = S.modal;
  const name = (m.name || '').trim();
  const key = (m.epicKey || '').trim().toUpperCase();
  const needKey = jiraOn() && (m.id ? !!key : m.epicMode === 'link');
  m.err = !name ? 'Give the goal a name.' : !m.project ? 'Pick a project.'
    : needKey && !/^[A-Z][A-Z0-9_]*-\d+$/.test(key) ? 'Enter an epic key like PROJ-123.' : '';
  if (m.err) return renderModal();
  const common = { name, tldr: (m.tldr || '').trim(), outcome: (m.outcome || '').trim(), project: m.project, run_in_order: !!m.run_in_order, max_terminals: Number(m.max_terminals) || 2, auto_close: !!m.auto_close };
  m.busy = true;
  renderModal();
  try {
    if (m.id) {
      await post(`/goals/${encodeURIComponent(m.id)}`, jiraOn() ? { ...common, epic_key: key || null } : common);
      closeModal();
      toast('Goal saved');
      refresh();
    } else {
      const g = await post('/goals', { ...common, epic: !jiraOn() ? { mode: 'none' } : m.epicMode === 'link' ? { mode: 'link', key } : { mode: m.epicMode } });
      clearDraft('goal');
      closeModal();
      const r = g && ref(g.goal || g, 'G');
      toast(r ? `Added ${r}` : 'Goal added');
      if (r && m.after) { await loadGoals(); applyPick(m.after, r); refresh(); }
      else if (r) nav(routeHash('goal', r, {})); else refresh();
    }
  } catch (e) { if (S.modal === m) { m.busy = false; m.err = e.message; renderModal(); } }
}

MODALS.issue = newIssueHtml;
MODALS.goal = goalFormHtml;
MODALS.gnote = noteDialogHtml;
SUBMITS.issue = submitNewIssue;
SUBMITS.goal = submitGoal;

Object.assign(ACTIONS, {
  'goal-view': el => nav(hashWith({ view: el.dataset.arg === 'backlog' ? 'backlog' : null, issue: null })),
  'pick-issue': el => nav(hashWith({ issue: el.dataset.id }), { replace: true }),
  'show-dropped': () => { P.showDropped = !P.showDropped; renderAll(); },
  'add-issue': el => openNewIssue(el.dataset.id || null),
  'gnav-view': el => { P.gnav = { ...(P.gnav || {}), view: el.dataset.arg }; renderAll(); },
  'gnote-open': el => {
    const n = (P.notes || []).find(x => String(x.id) === el.dataset.id);
    if (!n) return;
    S.modal = { kind: 'gnote', note: n };
    renderModal();
    setTimeout(() => { const b = $('#modal-root [data-act="modal-close"]'); if (b) b.focus(); }, 0);
  },
  'gnote-kind': el => { S.drafts['gnotekind:' + el.dataset.id] = el.dataset.arg; renderAll(); },
  'gnote-save': el => {
    const gr = el.dataset.id;
    const k = 'gnote:' + gr;
    const text = (S.drafts[k] || '').trim();
    if (!text) { setNote(k, 'Write the note first.', true); return renderAll(); }
    run(el, () => post(`/goals/${encodeURIComponent(gr)}/notes`, { kind: S.drafts['gnotekind:' + gr] || 'finding', text, source: 'you' }),
      { ok: () => { delete S.drafts[k]; delete S.reveal[k]; return 'Note added'; } });
  },
  'att-remove': el => { S.attMenu = null; run(el, () => post(`/attachments/${encodeURIComponent(el.dataset.id)}/remove`), { ok: () => 'Removed' }); },
  'att-menu': el => { S.attMenu = S.attMenu === el.dataset.id ? null : el.dataset.id; renderAll(); },
  'att-menu-close': () => { setTimeout(() => { S.attMenu = null; renderAll(); }, 0); },
  'att-copy': el => {
    S.attMenu = null; renderAll();
    copyText(el.dataset.url || '').then(ok => ok ? toast('Copied') : toast('Couldn’t copy it', true));
  },
  'att-edit': el => { S.attMenu = null; S.attEdit = el.dataset.id; clearAttEdit(el.dataset.id); renderAll(); },
  'att-ekind': el => { S.drafts[`attedit:${el.dataset.id}:kind`] = el.dataset.arg; renderAll(); },
  'att-ecancel': el => { S.attEdit = null; clearAttEdit(el.dataset.id); renderAll(); },
  'att-esave': el => {
    const id = el.dataset.id, k = 'attedit:' + id;
    const body = {};
    ['title', 'url', 'kind'].forEach(f => { if (S.drafts[`${k}:${f}`] != null) body[f] = S.drafts[`${k}:${f}`]; });
    run(el, () => post(`/attachments/${encodeURIComponent(id)}`, body), { grp: k, toast: true, ok: () => { S.attEdit = null; clearAttEdit(id); return 'Saved'; } });
  },
  'bl-kind': el => { P.blFilter.kind = el.dataset.arg; blFiltersChanged(); },
  'bl-pick': (el, e) => {
    const r = el.dataset.id, on = el.checked;
    const rows = pickableRefs(goalBacklogRows());
    const i = rows.indexOf(P.selAnchor), j = rows.indexOf(r);
    const span = e && e.shiftKey && i >= 0 && j >= 0 ? rows.slice(Math.min(i, j), Math.max(i, j) + 1) : [r];
    span.forEach(x => { if (on) P.sel.add(x); else P.sel.delete(x); });
    P.selAnchor = r;
    renderAll();
  },
  'bl-pick-all': el => {
    const rows = pickableRefs(goalBacklogRows());
    if (el.checked) rows.forEach(r => P.sel.add(r)); else P.sel.clear();
    P.selAnchor = null;
    renderAll();
  },
  'bl-pick-clear': () => { P.sel.clear(); P.selAnchor = null; delete S.notes.bulk; renderAll(); },
  'bl-bulk': el => bulkChange(el, el.dataset.arg, {}),
});
const BULK_DONE = {
  task: n => `Made ${plural(n, 'task')}`,
  ticket: n => `Asked Jira for ${plural(n, 'ticket')}`,
  drop: n => `Closed ${plural(n, 'issue')} as won’t do`,
  move: n => `Moved ${plural(n, 'issue')}`,
};
function bulkChange(el, action, extra) {
  const ids = [...P.sel];
  if (!ids.length) return;
  ids.forEach(keepIssue);
  const body = { ids, action, ...(action === 'task' ? { where: 'goal' } : {}), ...extra };
  return run(el, () => post('/backlog/bulk', body), {
    grp: 'bulk',
    ok: r => {
      P.sel.clear(); P.selAnchor = null;
      toast(BULK_DONE[action]((r && r.count) || ids.length));
      return '';
    },
  });
}
function blFiltersChanged() {
  P.kept = {}; P.order = [];
  if (S.route.q.issue) nav(hashWith({ issue: null }), { replace: true });
  else { renderAll(); refresh(); }
}
Object.assign(CHANGES, {
  'bl-filter': el => { P.blFilter[el.dataset.field] = el.value; blFiltersChanged(); },
  'bl-bulk-move': el => bulkChange({ dataset: { act: 'bl-bulk', arg: 'move' } }, 'move', { goal_id: el.value === 'none' ? null : idOf(el.value) }),
  'goal-set': el => {
    const f = el.dataset.field;
    const v = el.type === 'checkbox' ? el.checked : Number(el.value);
    if (P.goal) P.goal[f] = v;
    run(el, () => post(`/goals/${encodeURIComponent(el.dataset.id)}`, { [f]: v }), { ok: 'Saved' });
  },
});

function gateBanner(g) {
  const gr = ref(g, 'G');
  if (g.deprioritized) {
    return `<div class="box ask"><span class="box-label">Deprioritized</span><p>It’s off the board’s goal list and nothing new starts. Tasks already running carry on.</p>
      <div class="row">${btn('Bring it back', 'goal-deprio', { id: gr, arg: 'off', cls: 'primary', grp: 'ggate:' + gr })}</div>${note('ggate:' + gr)}</div>`;
  }
  if (g.paused) {
    return `<div class="box ask"><span class="box-label">Paused</span><p>Nothing new starts in this goal. Tasks already running carry on.</p>
      <div class="row">${btn('Resume the goal', 'goal-pause', { id: gr, arg: 'off', cls: 'primary', grp: 'ggate:' + gr })}</div>${note('ggate:' + gr)}</div>`;
  }
  return note('ggate:' + gr) ? `<div>${note('ggate:' + gr)}</div>` : '';
}

const PLAY = '<svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor" aria-hidden="true"><path d="M8 5.5v13a1 1 0 0 0 1.5.86l10.2-6.5a1 1 0 0 0 0-1.72L9.5 4.64A1 1 0 0 0 8 5.5z"/></svg>';
const CHAT = '<svg viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 15a2 2 0 0 1-2 2H8l-4 4V5a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2z"/></svg>';
const PAUSE = '<svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor" aria-hidden="true"><rect x="6" y="5" width="4" height="14" rx="1"/><rect x="14" y="5" width="4" height="14" rx="1"/></svg>';
const afterHours = () => { const h = S.state && S.state.work_hours; return !!(h && h.on && !h.open); };
function opensAt() {
  const nxt = new Date(S.state.work_hours.next_open || '');
  if (isNaN(nxt)) return ['later', ''];
  const tmr = new Date(); tmr.setDate(tmr.getDate() + 1);
  const day = nxt.toDateString() === new Date().toDateString() ? 'today' : nxt.toDateString() === tmr.toDateString() ? 'tomorrow'
    : nxt.toLocaleDateString(undefined, { weekday: 'long' });
  return [day, clock12(nxt.toTimeString().slice(0, 5))];
}
// Outside work hours, Start is a split button: the main part queues the planned tasks for when the hours open,
// and its menu's "Start now" lets this goal run outside the hours until they next open.
function startLater(g, more) {
  const gr = ref(g, 'G');
  const [day, at] = opensAt();
  const later = `Start ${day}`;
  const open = S.startMenu === gr;
  const menu = open ? `<div class="ctxmenu att-menu start-menu" role="menu" aria-label="When to start">
      <button type="button" role="menuitem" data-act="goal-run" data-id="${esc(gr)}"><b>${esc(later)}</b><span>Queued now, starts when work hours open${at ? ' at ' + esc(at) : ''}</span></button>
      <button type="button" role="menuitem" data-act="goal-run-now" data-id="${esc(gr)}"><b>Start now</b><span>Runs outside work hours until they open</span></button></div>` : '';
  return `<span class="split att-acts">${btn(more ? `${later}: ${more}` : later, 'goal-run', { id: gr, cls: 'primary md run', icon: PLAY, busyLabel: 'Queuing…',
      title: `Queue the planned tasks now; they start when work hours open${at ? ' (' + day + ' at ' + at + ')' : ''}` })}<button type="button" class="btn primary md split-caret" data-act="start-menu" data-id="${esc(gr)}" aria-haspopup="menu" aria-expanded="${open}" aria-label="More ways to start" title="Start now instead">${ICON.chev}</button>${menu}</span>`;
}
document.addEventListener('mousedown', e => { if (S.startMenu && !e.target.closest('.start-menu, [data-act="start-menu"]')) { S.startMenu = null; renderAll(); } }, true);
document.addEventListener('keydown', e => { if (S.startMenu && e.key === 'Escape') { S.startMenu = null; renderAll(); } });

function goalRunButtons(g) {
  const gr = ref(g, 'G');
  const tasks = g.tasks || [];
  const planned = tasks.filter(t => t.status === 'planned').length;
  const going = tasks.filter(t => ['queued', 'working', 'needs'].includes(t.status)).length;
  const unstarted = tasks.some(t => t.status === 'needs' && t.needs_reason === 'start_failed');
  const out = [];
  if (planned && afterHours()) out.push(startLater(g, going ? `${plural(planned, 'more task')}` : ''));
  else if (planned || (going && (g.paused || unstarted))) {
    const label = !planned ? 'Resume' : going ? `Start ${plural(planned, 'more task')}` : 'Start';
    out.push(btn(label, 'goal-run', { id: gr, cls: 'primary md run', icon: PLAY, busyLabel: 'Starting…',
      title: planned ? `Queue the ${plural(planned, 'planned task')}; the goal’s rules decide what runs when` : unstarted ? 'Queue the tasks that couldn’t start again' : 'Let new tasks start again' }));
  }
  if (going && !g.paused && !g.deprioritized && !unstarted) out.push(btn('Pause', 'goal-pause', { id: gr, arg: 'on', cls: 'md run', icon: PAUSE, title: 'Nothing new starts; running tasks carry on' }));
  const done = tasks.length && tasks.every(t => t.status === 'done');
  out.push(btn('Plan in Claude', 'goal-plan-edit', { id: gr, cls: 'md run', icon: CHAT, busyLabel: 'Opening…',
    title: 'Open a Claude terminal in Midna on this goal’s plan, to change it by talking it through' }));
  if (!g.deprioritized && !done) out.push(btn('Deprioritize', 'goal-deprio-ask', { id: gr, cls: 'danger md run', title: 'Take it off the board’s goal list; nothing new starts' }));
  return out.join('');
}

function gdeprioHtml(m) {
  const g = currentGoal() || {};
  const running = (g.tasks || []).filter(t => ['working', 'needs'].includes(t.status)).length;
  const queued = (g.tasks || []).filter(t => t.status === 'queued').length;
  const after = [running ? `${plural(running, 'task')} already running carr${running === 1 ? 'ies' : 'y'} on.` : '',
    queued ? `${plural(queued, 'queued task')} won’t start.` : ''].filter(Boolean).join(' ');
  return `<div class="overlay" data-act="modal-bg"><div class="modal narrow" role="alertdialog" aria-modal="true" aria-label="Deprioritize ${esc(m.id)}">
    <div class="modal-head"><h2>Deprioritize this goal?</h2>${closeBtn('modal-close')}</div>
    <p style="margin:0"><b>${esc(g.name || m.id)}</b> goes off the board’s goal list and nothing new starts. ${esc(after)}</p>
    <p class="help" style="margin:0">You can bring it back from the Deprioritized list under Goals.</p>
    <div class="modal-foot"><div class="grow"></div><button type="button" class="btn lg" data-act="modal-close">Cancel</button>${btn('Deprioritize', 'goal-deprio-yes', { id: m.id, cls: 'danger solid lg', busyLabel: 'Deprioritizing…' })}</div>
  </div></div>`;
}
MODALS.gdeprio = gdeprioHtml;

const gpost = (gr, suffix, body) => post(`/goals/${encodeURIComponent(gr)}${suffix}`, body || {});
Object.assign(ACTIONS, {
  'goal-run': el => { S.startMenu = null; run(el, () => gpost(el.dataset.id, '/run'), { toast: true, ok: r => !(r && r.queued_now) ? 'Going again'
    : afterHours() ? `${plural(r.queued_now, 'task')} queued. They start ${opensAt().filter(Boolean).join(' at ')}.` : `Started: ${plural(r.queued_now, 'task')} queued` }); },
  'goal-run-now': el => { S.startMenu = null; run(el, () => gpost(el.dataset.id, '/run', { now: true }), { toast: true, ok: r => `Started now: ${plural((r && r.queued_now) || 0, 'task')} queued. The goal runs until work hours open.` }); },
  'start-menu': el => { S.startMenu = S.startMenu === el.dataset.id ? null : el.dataset.id; renderAll(); },
  'goal-plan-edit': el => run(el, () => gpost(el.dataset.id, '/plan', { mode: 'edit' }), { toast: true, ok: 'Opening a Claude terminal in Midna. It starts in about half a minute.' }),
  'goal-deprio-ask': el => {
    S.modal = { kind: 'gdeprio', id: el.dataset.id };
    renderModal();
    setTimeout(() => { const b = $('#modal-root [data-act="modal-close"]'); if (b) b.focus(); }, 0);
  },
  'goal-deprio-yes': el => run(el, () => gpost(el.dataset.id, '', { deprioritized: true }),
    { toast: true, ok: () => { closeModal(); return 'Deprioritized. Running tasks carry on.'; } }),
  'goal-deprio': el => run(el, () => gpost(el.dataset.id, '', { deprioritized: el.dataset.arg === 'on' }), { toast: true, ok: el.dataset.arg === 'on' ? 'Deprioritized. Running tasks carry on.' : 'Back on the board' }),
  'goal-pause': el => run(el, () => gpost(el.dataset.id, '', { paused: el.dataset.arg === 'on' }), { toast: true, ok: el.dataset.arg === 'on' ? 'Paused. Running tasks carry on.' : 'Going again' }),
  'goal-edit': el => { const g = currentGoal(); if (g && sameRef(g, el.dataset.id, 'G')) openGoalForm(g); },
  'add-task': el => openTaskForm({ goal: el.dataset.id, planned: true }),
});

const PICKUPS = [['queue', 'When the repo’s free'], ['new', 'Now, in a new terminal'], ['attach', 'In an idle terminal'], ['manual', 'Only when I press Start']];
const PICKUP_HELP = {
  queue: 'It starts in a new Midna terminal once no other task is working in this project, inside work hours.',
  new: 'It starts in a new Midna terminal right away, even if another task is working in this project.',
  attach: 'The terminal you pick gets the handoff as its next prompt.',
  manual: 'It waits in Queued until you press Start on it.',
};
const defaultProject = () => S.filter.project !== 'all' ? S.filter.project : projectNames()[0] || '';
function openTaskForm(o = {}) {
  const g = o.goal ? goalByRef(o.goal) || (currentGoal() && sameRef(currentGoal(), o.goal, 'G') ? currentGoal() : null) : null;
  S.modal = {
    kind: 'task', id: null, title: '', detail: '', project: (g && g.project) || o.project || defaultProject(),
    goal: g ? ref(g, 'G') : (o.goal ? ref(o.goal, 'G') : (S.filter.goal !== 'all' ? S.filter.goal : '')), planned: !!o.planned,
    priority: 'normal', pickup: 'queue', session: '', jiraMode: 'none', jiraKey: '',
    auto_close: g ? g.auto_close !== false && g.auto_close !== 0 : true, err: '', busy: false,
  };
  if (!o.goal) restoreDraft(S.modal);
  renderModal();
  setTimeout(() => { const n = $('#nt-title'); if (n) n.focus(); }, 0);
}
function idleTerminals(project) {
  return ((S.state && S.state.sessions) || []).filter(s => s.can_take && (!project || s.project === project));
}
function taskFormHtml(m) {
  const terms = idleTerminals(m.project);
  const pickup = m.pickup === 'attach' && !terms.length ? 'queue' : m.pickup;
  const jira = jiraOn() ? `<fieldset class="field" style="gap:8px"><legend>Jira ticket</legend><div class="seg">${seg('jiraMode', [['create', 'Create one'], ['link', 'Link existing'], ['none', 'None']], m.jiraMode)}</div>
      ${m.jiraMode === 'link' ? `<label class="sr" for="nt-jira">Ticket key</label><input id="nt-jira" class="input mono" data-m="jiraKey" placeholder="PROJ-123" autocomplete="off" value="${esc(m.jiraKey)}">` : ''}</fieldset>` : '';
  const when = m.goal && m.planned ? '<span class="help">It joins the goal’s plan and starts when you press Start on the goal.</span>'
    : `<fieldset class="field" style="gap:8px"><legend>Start</legend><div class="seg wrap">${PICKUPS.map(([id, label]) =>
      `<button type="button" data-act="m-set" data-field="pickup" data-arg="${id}" aria-pressed="${pickup === id}"${id === 'attach' && !terms.length ? ' disabled title="No idle terminal in this project"' : ''}>${esc(label)}</button>`).join('')}</div>
      ${pickup === 'attach' ? `<label class="sr" for="nt-session">Terminal</label><select id="nt-session" class="select" data-m="session">${opt('', 'Pick a terminal', m.session)}${terms.map(s => opt(s.id, s.name, m.session)).join('')}</select>` : ''}
      <span class="help">${PICKUP_HELP[pickup]}</span></fieldset>`;
  return `<div class="overlay" data-act="modal-bg"><form class="modal narrow" aria-label="New task" data-form="task" novalidate>
    <div class="modal-head"><h2>New task</h2>${closeBtn('modal-close')}</div>${draftNote(m)}
    <div class="field"><label for="nt-title">Title</label><input id="nt-title" class="input" type="text" data-m="title" autocomplete="off" value="${esc(m.title)}"></div>
    <div class="field"><label for="nt-detail">What to do</label><textarea id="nt-detail" class="input" rows="5" data-m="detail">${esc(m.detail)}</textarea><span class="help">The terminal gets this in its handoff.</span></div>
    <div class="two">
      <div class="field"><label for="nt-project">Project</label>${projectButton('nt-project', m.project, { target: 'modal:project' })}</div>
      <div class="field"><label for="nt-goal">Goal</label>${goalButton('nt-goal', m.goal || '', { target: 'modal:goal', specials: [['', 'Not in a goal']] })}</div>
    </div>
    ${m.goal ? `<label class="check" style="font-size:14px;color:var(--text)"><input type="checkbox" data-m="planned" data-rerender="1"${m.planned ? ' checked' : ''}>Add it to the goal’s plan instead of the queue</label>` : ''}
    <fieldset class="field"><legend>Priority</legend><div class="seg">${seg('priority', [['normal', 'Normal'], ['high', 'High']], m.priority)}</div></fieldset>
    ${when}
    ${jira}
    <label class="check" style="font-size:14px;color:var(--text)"><input type="checkbox" data-m="auto_close"${m.auto_close ? ' checked' : ''}>Close its terminal when it’s done</label>
    <div class="modal-foot">${m.err ? `<span class="ferr note" role="alert">${esc(m.err)}</span>` : ''}<button type="button" class="btn lg" data-act="modal-close">Cancel</button><button type="submit" class="btn primary lg"${m.busy ? ' disabled' : ''}>${m.busy ? 'Adding…' : 'Add task'}</button></div>
  </form></div>`;
}
async function submitTask() {
  const m = S.modal;
  const title = (m.title || '').trim();
  const key = (m.jiraKey || '').trim().toUpperCase();
  const planned = !!(m.goal && m.planned);
  const pickup = m.pickup === 'attach' && !idleTerminals(m.project).length ? 'queue' : m.pickup;
  m.err = !title ? 'Give the task a title.' : !m.project ? 'Pick a project.'
    : !planned && pickup === 'attach' && !m.session ? 'Pick the terminal to hand it to.'
    : jiraOn() && m.jiraMode === 'link' && !/^[A-Z][A-Z0-9_]*-\d+$/.test(key) ? 'Enter a ticket key like PROJ-123.' : '';
  if (m.err) return renderModal();
  const body = {
    title, detail: (m.detail || '').trim(), project: m.project, priority: m.priority === 'high' ? 'high' : 'normal',
    goal_id: m.goal ? idOf(m.goal) : null, auto_close: !!m.auto_close,
    pickup: planned ? { mode: 'queue' } : pickup === 'attach' ? { mode: 'attach', session_id: m.session } : { mode: pickup },
  };
  if (planned) body.status = 'planned';
  if (jiraOn()) body.jira = m.jiraMode === 'link' ? { mode: 'link', key } : { mode: m.jiraMode === 'create' ? 'create' : 'none' };
  m.busy = true;
  renderModal();
  try {
    const t = await post('/tasks', body);
    clearDraft('task');
    closeModal();
    const r = t && ref(t, 'T');
    toast(r ? `Added ${r}` : 'Task added');
    if (r) openTask(r); else refresh();
  } catch (e) { if (S.modal === m) { m.busy = false; m.err = e.message; renderModal(); } }
}
MODALS.task = taskFormHtml;
SUBMITS.task = submitTask;
