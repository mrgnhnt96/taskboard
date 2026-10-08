'use strict';
// The goal page's wave rail, from the Python board's waves.js (2026-10-08), minus devices (the port has
// none) and the "Stop after this wave for my review" checkbox (the app doesn't edit: `tb goal wave --stop`).

const goalWaves = g => (g && g.waves) || [];
const waveName = w => `Wave ${w.wave}${w.name ? ' · ' + w.name : ''}`;
const WAVE_NODE = {
  done: '<svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true"><circle cx="10" cy="10" r="9" fill="currentColor"/><path d="M6 10.3l2.6 2.6L14 7.6" fill="none" stroke="var(--card)" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>',
  running: '<svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true"><circle cx="10" cy="10" r="8" fill="var(--accent-soft)" stroke="currentColor" stroke-width="2"/><circle cx="10" cy="10" r="3.5" fill="currentColor"/></svg>',
  stopped: '<svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true"><circle cx="10" cy="10" r="8" fill="var(--warn-soft)" stroke="currentColor" stroke-width="2"/><rect x="7" y="6.5" width="2" height="7" rx=".6" fill="currentColor"/><rect x="11" y="6.5" width="2" height="7" rx=".6" fill="currentColor"/></svg>',
  failed: '<svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true"><circle cx="10" cy="10" r="9" fill="currentColor"/><path d="M10 5.5v5.5" stroke="var(--card)" stroke-width="2" stroke-linecap="round"/><circle cx="10" cy="14.2" r="1.2" fill="var(--card)"/></svg>',
  waiting: '<svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true"><circle cx="10" cy="10" r="7" fill="var(--card)" stroke="currentColor" stroke-width="2"/></svg>',
  finish: '<svg viewBox="0 0 20 20" width="20" height="20" aria-hidden="true"><circle cx="10" cy="10" r="8" fill="var(--card)" stroke="currentColor" stroke-width="2"/><path d="M7.5 14.5v-9M7.5 6h5.5l-1.4 2 1.4 2H7.5" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round"/></svg>',
};
S.waveOpen = S.waveOpen || {};
const waveKey = (g, n) => `${ref(g, 'G')}:${n}`;
const waveOpen = (g, w) => S.waveOpen[waveKey(g, w.wave)] ?? ['running', 'stopped', 'failed'].includes(w.state);
const STALE_UPDATE_MIN = 10;
const minsAgo = iso => { const t = Date.parse(iso); return isNaN(t) ? null : (Date.now() + serverClockOffset - t) / 60000; };
const refLinks = s => esc(s);

function waveCount(w, tasks) {
  const pill = (label, cls, n = 1) => `<span class="pill sm ${cls}">${esc(label)}${n > 1 ? ` ×${n}` : ''}</span>`;
  if (w.state === 'stopped') return pill('Waiting for your review', 'st-needs');
  if (w.state === 'failed') return pill('A task failed', 'st-failed');
  if (w.held_by === 'stopped' && w.state !== 'done') return pill('Stopped for your review', 'st-needs');
  const counts = new Map();
  const add = (label, cls) => { const c = counts.get(label) || { n: 0, cls }; c.n++; counts.set(label, c); };
  const open = tasks.filter(awaitingMerge);
  const done = tasks.filter(t => t.status === 'done' && !t.failed && !awaitingMerge(t)).length;
  const starting = w.starting || 0, running = (w.active || 0) - starting;
  for (let i = 0; i < running; i++) add('Running', 'st-working');
  for (let i = 0; i < starting; i++) add('Starting', 'st-queued');
  tasks.filter(t => t.status === 'queued' && t.blocked).forEach(() => add('Blocked', 'st-blocked'));
  open.forEach(t => add(...prStatus(t)));
  for (let i = 0; i < done; i++) add('Done', 'st-done');
  if (!counts.size) return w.state === 'done' ? pill('Done', 'st-done') : '';
  return [...counts].map(([label, c]) => pill(label, c.cls, c.n)).join('');
}
const wchip = (text, cls = '', html = esc(text)) => `<span class="wchip${cls ? ' ' + cls : ''}">${html}</span>`;
const isLive = t => t.status === 'working' || t.status === 'needs';
function waveTaskLine(t) {
  const r = ref(t, 'T');
  const jira = t.jira && t.jira.key && t.jira.url
    ? `<a class="jira-at" href="${esc(safeUrl(t.jira.url))}" target="_blank" rel="noopener" title="${esc(t.jira.key + (t.jira.status ? ' · ' + t.jira.status : ''))} in Jira">${ICON.jiraMark}<span class="sr">Open ${esc(t.jira.key)} in Jira</span></a>` : '';
  const when = isLive(t) ? runningFor(t).replace(/^running /, '') : '';
  return `<span class="wt-l1"><span class="n">${esc(r)}</span>${jira}${prMark(t, true)}<button type="button" class="wt-title" data-act="open-task" data-id="${esc(r)}">${esc(t.title)}</button><span class="grow"></span>${when ? `<span class="wt-when live">${esc(when)}</span>` : ''}</span>`;
}
function attentionFacts(t) {
  const k = stKey(t);
  const facts = [];
  if (k === 'needs') facts.push(wchip(t.question ? 'Asked you something' : 'Needs you', 'warn'));
  if (k === 'failed') facts.push(wchip('Failed', 'bad'));
  if (t.lost) facts.push(wchip('Terminal lost', 'warn'));
  if (t.who && t.session_id && t.status !== 'planned' && t.status !== 'queued') {
    facts.push(`<a class="wchip term" href="${esc(routeHash('sessions', null, { s: t.session_id }))}" title="Open this terminal">${esc(t.who)}</a>`);
  }
  return facts;
}
function waveTask(t, w) {
  const live = isLive(t);
  const facts = attentionFacts(t);
  if (live && t.when) { const m = minsAgo(t.when); facts.push(wchip(`${t.question ? 'asked' : 'updated'} ${ago(t.when)}`, m != null && m > STALE_UPDATE_MIN ? 'warn' : 'quiet')); }
  if (t.status === 'queued' && !t.starting && !(w && w.hold && t.waiting === w.hold)) facts.push(wchip(t.waiting || 'Starts when a terminal is free', t.blocked || /jira/i.test(t.waiting || '') ? 'warn' : 'quiet', `<span>${refLinks(t.waiting || 'Starts when a terminal is free')}</span>`));
  if (t.status === 'queued' && t.starting) facts.push(wchip('Starting', 'quiet'));
  if (t.priority === 'high' && t.status !== 'done') facts.push(wchip('High priority', 'warn'));
  if ((t.also || []).length) facts.push(wchip(`Also for ${t.also.map(x => x.ref).join(', ')}`, 'quiet'));
  return `<li class="wt${t.status === 'planned' ? ' planned' : ''}">
    ${waveTaskLine(t)}
    ${facts.length ? `<span class="wt-facts">${facts.join('')}</span>` : ''}</li>`;
}
const landed = t => t.status === 'done' && !t.failed && !awaitingMerge(t);
const BRANCH_IN = (line, dashed) => `<svg class="wv-branch" viewBox="0 0 64 64" width="64" height="64" aria-hidden="true"><path d="M49 36C49 54 10 46 10 64" fill="none" stroke="${line}" stroke-width="2"${dashed ? ' stroke-dasharray="4 3"' : ''} stroke-linecap="round"/></svg>`;
function waveRef(t, railDone) {
  const home = t.goal;
  const badge = home
    ? `<a class="g-badge" href="${esc(routeHash('goal', home.ref, {}))}" title="Open ${esc(home.ref + ' · ' + home.name)}">${esc(home.ref)}</a>`
    : '<span class="g-badge" title="From another goal">G?</span>';
  const facts = attentionFacts(t);
  return `<li class="wv wv-ref${railDone ? ' ws-done' : ''}">${BRANCH_IN(landed(t) ? 'var(--up)' : 'var(--goal-line)', !landed(t))}<span class="wv-node"></span>${badge}
    <div class="wv-main"><div class="wt${t.status === 'planned' ? ' planned' : ''}">${waveTaskLine(t)}
      ${facts.length ? `<span class="wt-facts">${facts.join('')}</span>` : ''}</div></div></li>`;
}
function waveFinish(g, refs, wavesDone) {
  const waiting = refs.filter(t => !landed(t));
  const whose = [...new Set(waiting.map(t => `${t.goal ? t.goal.ref : 'another goal'}’s`))];
  const names = whose.length > 1 ? `${whose.slice(0, -1).join(', ')} and ${whose[whose.length - 1]}` : whose[0];
  const done = !!g.finished_at;
  const text = done ? 'Goal finished' : waiting.length
    ? `Goal finishes when ${names} ${waiting.length === 1 ? 'task lands' : 'tasks land'}`
    : wavesDone && (g.prs_open || []).length ? `Goal finishes when ${(g.prs_open || []).length === 1 ? 'its PR merges' : 'its PRs merge'}` : 'Goal finishes after its last wave';
  return `<li class="wv wv-finish${done ? ' ws-done' : ''}"><span class="wv-node">${done ? WAVE_NODE.done : WAVE_NODE.finish}</span>
    <div class="wv-main"><div class="wv-head"><span class="wv-name">${esc(text)}</span></div></div></li>`;
}
function waveGate(g, w, next) {
  const gr = ref(g, 'G');
  const on = next ? `wave ${next.wave}` : 'the rest of the goal';
  if (w.state === 'stopped') {
    return `<div class="wgate ask"><span><b>Stop point.</b> ${esc(waveName(w))} is done. Look it over, then let ${esc(on)} start.</span>${btn(`Continue to ${on}`, 'wave-continue', { id: gr, arg: w.wave, cls: 'primary sm', grp: 'wgate:' + gr })}</div>`;
  }
  if (w.state === 'failed' || (w.failed && w.failed.length && !w.passed)) {
    return `<div class="wgate bad"><span><b>${esc(w.failed.join(', '))} failed.</b> ${esc(on[0].toUpperCase() + on.slice(1))} waits. Try the task again from its ⋯ menu, or go on without it.</span>${btn('Continue anyway', 'wave-continue', { id: gr, arg: w.wave, cls: 'sm', grp: 'wgate:' + gr })}</div>`;
  }
  if (w.stop_after && w.released_at) {
    return `<div class="wgate ok"><span>You let the goal go on at ${esc(hhmm(w.released_at))}.</span></div>`;
  }
  return '';
}
function waveItem(g, w, next, rows, tasks) {
  const gr = ref(g, 'G');
  const fk = `wname:${gr}:${w.wave}`;
  const open = waveOpen(g, w);
  const count = waveCount(w, tasks);
  const node = WAVE_NODE[w.state] || WAVE_NODE.waiting;
  const stopFlag = w.stop_after && !w.released_at && w.state !== 'stopped' ? `<span class="wv-stop" title="The goal waits for you after this wave">${ICON.flag}Review stop</span>` : '';
  const body = open ? `<div class="wv-box"><ol class="wv-tasks">${rows}</ol>
      ${note(fk)}</div>` : '';
  return `<li class="wv ws-${esc(w.state || 'waiting')}" data-wave="${w.wave}"><span class="wv-node">${node}</span>
    <div class="wv-main"><button type="button" class="wv-head" data-act="wave-toggle" data-id="${esc(gr)}" data-arg="${w.wave}" aria-expanded="${open}">
      <span class="wv-n">Wave ${w.wave}</span>${w.name ? `<span class="wv-name">${esc(w.name)}</span>` : ''}<span class="wv-count">${count}</span><span class="grow"></span>${stopFlag}${ICON.fwd}</button>
      ${waveGate(g, w, next)}${body}</div></li>`;
}
function waveRail(g) {
  const tasks = g.tasks || [];
  const ws = goalWaves(g);
  const rowsOf = (keep, w) => tasks.map(t => keep(t) ? waveTask(t, w) : '').join('');
  const items = ws.map((w, k) => waveItem(g, w, ws[k + 1], rowsOf(t => t.wave === w.wave, w), tasks.filter(t => t.wave === w.wave)));
  if (tasks.some(t => t.wave == null)) {
    const open = S.waveOpen[waveKey(g, 'none')] ?? true;
    items.push(`<li class="wv ws-loose"><span class="wv-node">${WAVE_NODE.waiting}</span>
      <div class="wv-main"><button type="button" class="wv-head" data-act="wave-toggle" data-id="${esc(ref(g, 'G'))}" data-arg="none" aria-expanded="${open}">
        <span class="wv-name">Post</span><span class="grow"></span>${ICON.fwd}</button>
        ${open ? `<div class="wv-box"><ol class="wv-tasks">${rowsOf(t => t.wave == null)}</ol></div>` : ''}</div></li>`);
  }
  const refs = g.shared || [];
  if (refs.length) {
    let railDone = ws.every(w => w.state === 'done') && tasks.every(t => t.wave != null || landed(t));
    const wavesDone = railDone;
    refs.forEach(t => { railDone = railDone && landed(t); items.push(waveRef(t, railDone)); });
    items.push(waveFinish(g, refs, wavesDone));
  }
  return `<ol class="wrail">${items.join('')}</ol>`;
}

Object.assign(ACTIONS, {
  'wave-toggle': el => {
    const g = currentGoal();
    const n = el.dataset.arg === 'none' ? 'none' : Number(el.dataset.arg);
    const w = goalWaves(g).find(x => x.wave === n);
    const key = `${el.dataset.id}:${n}`;
    S.waveOpen[key] = !(S.waveOpen[key] ?? (w ? waveOpen(g, w) : true));
    renderAll();
  },
  'wave-continue': el => run(el, () => post(`/goals/${encodeURIComponent(el.dataset.id)}/waves/${el.dataset.arg}/continue`, {}), { ok: 'The goal goes on' }),
});
