'use strict';

const SS = {
  list: null, listErr: '', closed: [], closedTotal: 0,
  detail: null, detailErr: '', detailFor: null,
  picked: new Set(), confirm: null, bulk: false, q: '',
  selecting: false,
  menu: null,
  focusRename: null,
  closedOpen: false,
  hideSel: null,
  collapsed: (() => { try { return new Set(JSON.parse(localStorage.getItem('tb.sessCollapsed') || '[]')); } catch { return new Set(); } })(),
};
const STALE_MS = 60 * 60 * 1000;
const SS_FILTERS = [['all', 'All'], ['needs', 'Needs you'], ['working', 'Working'], ['idle', 'Idle'], ['stale', 'Idle over 1 hour']];
const TIMELINE_KINDS = {
  prompt: ['var(--accent)', 'Prompt'], reply: ['var(--faint)', 'Reply'], turn: ['var(--accent)', 'Turn'],
  commit: ['var(--up)', 'Commit'], checkpoint: ['var(--goal)', 'Checkpoint'], found: ['var(--seen)', 'Issue'],
  ask: ['var(--warn)', 'Question'], wait: ['var(--warn)', 'Waiting'], compact: ['var(--faint)', 'Compacted'],
  start: ['var(--faint)', 'Started'], end: ['var(--faint)', 'Ended'], take: ['var(--accent)', 'Task'],
  done: ['var(--up)', 'Done'], fail: ['var(--down)', 'Failed'], rename: ['var(--faint)', 'Renamed'],
  close: ['var(--down)', 'Close'], closed: ['var(--down)', 'Closed'], close_failed: ['var(--down)', 'Close'],
};

const ssSel = () => S.route.q.s || null;
const ssFilter = () => S.route.q.f || 'all';
const idleMs = s => { const t = Date.parse(s.last_activity || s.seen_at); return isNaN(t) ? 0 : Date.now() + serverClockOffset - t; };
const isStale = s => (s.status || 'idle') === 'idle' && idleMs(s) > STALE_MS;
const bulkable = s => s.close === 'close' && !s.task_ref && !s.closing;
function ssSession(id) {
  return (SS.list || []).find(s => s.id === id) || SS.closed.find(s => s.id === id) || null;
}
function forFilter(s, f) {
  if (f === 'all') return true;
  if (f === 'stale') return isStale(s);
  return (s.status || 'idle') === f;
}
function ssMatch(s) {
  const q = SS.q.trim().toLowerCase();
  return !q || [s.name, s.project, s.task_title, s.task_ref, s.branch].some(v => v && String(v).toLowerCase().includes(q));
}
function longAgo(ms) {
  const m = Math.round(ms / 60000);
  if (m < 1) return 'under a minute';
  if (m < 60) return `${m} min`;
  const h = Math.floor(m / 60), r = m % 60;
  if (h < 24) return r && h < 6 ? `${h} h ${r} min` : `${h} h`;
  return plural(Math.round(h / 24), 'day');
}

function sessionsLoads() {
  if (S.route.page !== 'sessions') return [];
  const loads = [loadSessionList()];
  loads.push(loadClosedSessions());
  const sel = ssSel();
  if (sel) loads.push(loadSessionDetail(sel));
  return loads;
}
async function loadSessionList() {
  try { const r = await api('/sessions?project=all'); SS.list = (r && r.sessions) || []; SS.listErr = ''; }
  catch (e) { SS.listErr = e.message; }
  for (const id of [...SS.picked]) { const s = ssSession(id); if (!s || !bulkable(s)) SS.picked.delete(id); }
}
async function loadClosedSessions() {
  try { const r = await api('/sessions/closed?project=all'); SS.closed = (r && r.sessions) || []; SS.closedTotal = (r && r.total) || SS.closed.length; }
  catch {}
}
async function loadSessionDetail(id) {
  try { SS.detail = await api('/sessions/' + encodeURIComponent(id)); SS.detailErr = ''; SS.detailFor = id; }
  catch (e) { if (SS.detailFor !== id) SS.detail = null; SS.detailErr = e.message; SS.detailFor = id; }
}
function afterSessionsLoad() {
  if (S.route.page !== 'sessions' || ssSel() || !SS.list) return;
  const groups = groupedSessions(visibleSessions());
  const first = groups.find(([p]) => !SS.collapsed.has(p)) || groups[0];
  if (!first) return;
  const id = first[1][0].id;
  if (SS.collapsed.has(first[0])) SS.hideSel = id;
  setTimeout(() => nav(hashWith({ s: id }), { replace: true }), 0);
}

function visibleSessions() { return (SS.list || []).filter(s => forFilter(s, ssFilter()) && ssMatch(s)); }
function groupedSessions(list) {
  const groups = new Map();
  list.forEach(s => { const k = s.project || 'No project'; if (!groups.has(k)) groups.set(k, []); groups.get(k).push(s); });
  const rank = s => SESS_ORDER[s.status] ?? 2;
  for (const ss of groups.values()) ss.sort((a, b) => rank(a) - rank(b) || idleMs(a) - idleMs(b));
  return [...groups.entries()].sort((a, b) => Math.min(...a[1].map(rank)) - Math.min(...b[1].map(rank)) || a[0].localeCompare(b[0]));
}

function renderSessionsPage(page) {
  patch(page, `<div class="spage">${ssHeader()}<div class="spage-grid">${ssList()}${ssDetail()}</div></div>${ssBulkConfirm()}${ssMenu()}`);
  const fr = SS.focusRename && document.getElementById('rename-' + SS.focusRename);
  if (fr) { SS.focusRename = null; fr.focus(); fr.select(); }
}

function ssMenuItems(s) {
  if (!s) return [];
  if (s.status === 'gone' || s.closed_at) return s.can_reopen ? [['reopen', 'Reopen its conversation']] : [];
  const items = [['open', 'Open'], ['focus', 'Show in Midna'], ['rename', 'Rename…']];
  if (bulkable(s)) items.push(['select', SS.selecting && SS.picked.has(s.id) ? 'Deselect' : 'Select']);
  if (!s.closing && (s.close === 'close' || s.close === 'force')) items.push(['close', s.close === 'force' ? 'Force close…' : 'Close terminal…', 'danger']);
  return items;
}
function ssMenu() {
  const m = SS.menu;
  const items = m ? ssMenuItems(ssSession(m.id)) : [];
  if (!items.length) return '';
  const x = Math.min(m.x, window.innerWidth - 220), y = Math.min(m.y, window.innerHeight - (items.length * 34 + 16));
  return `<div class="ctxmenu" role="menu" style="left:${Math.max(8, x)}px;top:${Math.max(8, y)}px">${items.map(([k, label, cls]) =>
    `<button type="button" role="menuitem" class="${cls || ''}" data-act="ss-menu" data-arg="${k}" data-id="${esc(m.id)}">${esc(label)}</button>`).join('')}</div>`;
}
function closeSsMenu() { if (SS.menu) { SS.menu = null; renderAll(); } }
document.addEventListener('contextmenu', e => {
  const row = S.route.page === 'sessions' && e.target.closest('[data-row]');
  if (!row || !ssMenuItems(ssSession(row.dataset.row)).length) return;
  e.preventDefault();
  SS.menu = { id: row.dataset.row, x: e.clientX, y: e.clientY };
  renderAll();
  const first = document.querySelector('.ctxmenu button');
  if (first) first.focus();
});
document.addEventListener('mousedown', e => { if (SS.menu && !e.target.closest('.ctxmenu')) closeSsMenu(); }, true);
window.addEventListener('resize', closeSsMenu);
document.addEventListener('scroll', closeSsMenu, true);

function ssHeader() {
  const all = SS.list || [];
  const f = ssFilter();
  const seg = SS_FILTERS.map(([k, label]) => {
    const n = all.filter(s => forFilter(s, k)).length;
    return `<button type="button" data-act="ss-filter" data-arg="${k}" aria-pressed="${f === k}">${esc(label)} <span class="n">${n}</span></button>`;
  }).join('');
  const back = /^#\//.test(S.route.q.back || '') ? S.route.q.back : '';
  const from = back && new URLSearchParams(back.split('?')[1] || '').get('task');
  return `<header class="top spage-top">
    <a class="back" href="${esc(back || '#/')}">${ICON.back}${from ? 'Back to ' + esc(ref(from, 'T')) : 'Task board'}</a>
    <h1>Sessions</h1><span class="muted small">${SS.list ? plural(all.length, 'Claude terminal') + ' in Midna' : ''}</span>
    <div class="grow"></div>
    <div class="seg sm" role="group" aria-label="Show">${seg}</div>
  </header>`;
}

function ssRow(s) {
  const status = SESS[s.status] ? s.status : 'idle';
  const on = ssSel() === s.id;
  const box = !SS.selecting ? '' : bulkable(s)
    ? `<input type="checkbox" id="pick-${esc(s.id)}" data-change="ss-pick" data-id="${esc(s.id)}"${SS.picked.has(s.id) ? ' checked' : ''}><label class="sr" for="pick-${esc(s.id)}">Select ${esc(s.name)}</label>`
    : '<span class="spacer"></span>';
  const sub = s.task_ref ? `${s.task_ref} · ${s.task_title || ''}` : ['No task', s.branch].filter(Boolean).join(' · ');
  const state = s.closing ? 'Closing…' : SESS[status];
  const when = status === 'idle' ? (idleMs(s) > 60000 ? 'for ' + longAgo(idleMs(s)) : 'just now') : ago(s.last_activity);
  return `<div class="srow s-${status}${on ? ' on' : ''}${s.closing ? ' closing' : ''}" data-row="${esc(s.id)}">${box}
    <button type="button" class="srow-main" data-act="ss-open" data-id="${esc(s.id)}" aria-pressed="${on}">
      <span class="dot"></span>
      <span class="srow-text">${(nv => `<span class="srow-name${nv.cls}"${nv.attrs}>${esc(nv.text)}</span>`)(nameView(s, ''))}<span class="srow-sub">${esc(sub)}</span></span>
      <span class="srow-end"><span class="srow-state">${esc(state)}</span><span class="srow-when">${esc(when)}</span></span>
    </button></div>`;
}
function ssClosedRow(s) {
  const on = ssSel() === s.id;
  const sub = s.task_ref ? `${s.task_ref} · ${s.task_title || ''}` : 'No task';
  return `<div class="srow s-gone${on ? ' on' : ''}" data-row="${esc(s.id)}">${SS.selecting ? '<span class="spacer"></span>' : ''}
    <button type="button" class="srow-main" data-act="ss-open" data-id="${esc(s.id)}" aria-pressed="${on}">
      <span class="dot"></span>
      <span class="srow-text"><span class="srow-name">${esc(s.name)}</span><span class="srow-sub">${esc([s.project, sub].filter(Boolean).join(' · '))}</span></span>
      <span class="srow-end"><span class="srow-state">Closed</span><span class="srow-when">${esc(ago(s.closed_at))}</span></span>
    </button></div>`;
}

const holdsSel = rows => { const sel = ssSel(); return !!sel && sel !== SS.hideSel && rows.some(x => x.id === sel); };
function groupHead(open, act, id, label, count) {
  return `<button type="button" class="sgroup-h sgroup-toggle" data-act="${act}" data-id="${esc(id)}" aria-expanded="${open}">${ICON.chev}<span>${esc(label)}</span><span class="n">${count}</span></button>`;
}

function ssList() {
  let body;
  if (!SS.list) body = `<p class="none-yet pad">${esc(SS.listErr || 'Loading…')}</p>`;
  else {
    const groups = groupedSessions(visibleSessions());
    body = groups.length ? groups.map(([proj, ss]) => {
      const open = !SS.collapsed.has(proj) || !!SS.q.trim() || holdsSel(ss);
      const needs = ss.filter(x => x.status === 'needs').length;
      const extra = !open && needs ? ` · <b>${needs} need${needs === 1 ? 's' : ''} you</b>` : '';
      return `<div class="sgroup${open ? ' open' : ''}">${groupHead(open, 'ss-group', proj, proj, plural(ss.length, 'terminal') + extra)}${open ? ss.map(ssRow).join('') : ''}</div>`;
    }).join('')
      : `<p class="none-yet pad">${SS.q.trim() ? `No terminal matches “${esc(SS.q.trim())}”.` : ssFilter() === 'all' ? 'No Claude terminals in Midna right now.' : 'None right now.'}</p>`;
    if (SS.closed.length && ssFilter() === 'all') {
      const closed = SS.closed.filter(ssMatch);
      const open = SS.closedOpen || holdsSel(closed);
      if (closed.length) body += `<div class="sgroup closed${open ? ' open' : ''}">${groupHead(open, 'ss-closed', '', 'Closed', plural(SS.q.trim() ? closed.length : SS.closedTotal, 'terminal'))}${open ? closed.slice(0, 50).map(ssClosedRow).join('') : ''}</div>`;
    }
  }
  const n = SS.picked.size;
  const stale = (SS.list || []).filter(s => bulkable(s) && isStale(s)).length;
  return `<section class="slist" aria-label="Terminals by project">
    <div class="slist-search">${ICON.search}<label class="sr" for="ss-q">Search terminals</label><input id="ss-q" class="input sm" type="search" placeholder="Search by terminal, project, task or branch" data-k="ss-q" value="${esc(SS.q)}" autocomplete="off">${!SS.selecting && stale > 1 && ssFilter() === 'all' ? btn(`Select ${stale} idle`, 'ss-stale', { cls: 'soft sm', title: `Select the ${stale} terminals that have been idle for over an hour, to close them together` }) : ''}</div>
    ${SS.selecting ? `<div class="sbulk${n ? ' on' : ''}"><span class="sbulk-line">${n} selected</span>
      ${stale > 1 && ssFilter() === 'all' && (SS.list || []).some(s => bulkable(s) && isStale(s) && !SS.picked.has(s.id)) ? btn(`Add the ${stale} idle over 1 h`, 'ss-stale', { cls: 'plain sm' }) : ''}
      ${btn('Done', 'ss-done', { cls: 'ghost sm' })}${n ? btn(n === 1 ? 'Close 1 terminal' : `Close ${n} terminals`, 'ss-bulk', { cls: 'danger sm' }) : ''}</div>`
    : ''}
    <div class="slist-body" data-scroll="slist">${body}</div>
  </section>`;
}

function statusLine(d) {
  const st = d.status;
  if (d.gone_at || st === 'gone') return ago(d.gone_at || d.last_activity);
  const since = at => { const t = Date.parse(at); return isNaN(t) ? '' : 'for ' + longAgo(Date.now() + serverClockOffset - t); };
  if (st === 'working') return since(d.status_at || (d.prompt && d.prompt.at));
  if (st === 'needs') return since(d.status_at || (d.waiting && d.waiting.at));
  return 'for ' + longAgo(idleMs(d));
}
function diffView(d) {
  const g = d.diff;
  if (!g) return d.dirty != null ? `<span>${d.dirty ? plural(d.dirty, 'file') : 'No changes'}</span>` : '';
  if (!g.files) return '<span>No changes</span>';
  return `<span class="sdiff" title="Not committed yet${g.new ? ` · ${g.new} new` : ''}">${esc(plural(g.files, 'file'))} <b class="add">+${g.added}</b> <b class="del">−${g.removed}</b></span>`;
}
function closeCopy(d) {
  const force = d.close === 'force';
  const title = force ? `Force close ${d.name}?` : `Close ${d.name}?`;
  const busy = d.prompt && d.prompt.text ? `It’s in the middle of a turn: “${one(d.prompt.text, 120)}”. Closing it stops that. ` : 'It’s in the middle of something. Closing it stops that. ';
  const task = d.task ? 'Its task goes back to Queued with everything saved so far, so another terminal can pick it up. ' : '';
  const keep = 'The board keeps the conversation, so you can reopen it later. ';
  return { title, text: (force ? busy : '') + task + keep + 'Midna may ask you to confirm on the Mac.', label: force ? 'Force close' : 'Close terminal' };
}
const one = (t, n) => { const s = String(t || '').replace(/\s+/g, ' ').trim(); return s.length > n ? s.slice(0, n - 1) + '…' : s; };

function ssDetail() {
  const sel = ssSel();
  if (!sel) return `<section class="sdetail empty"><p class="none-yet">Pick a terminal to see what it’s done.</p></section>`;
  const d = SS.detailFor === sel ? SS.detail : null;
  if (!d) return `<section class="sdetail empty"><p class="${SS.detailErr ? 'note err' : 'none-yet'}">${esc(SS.detailErr || 'Loading…')}</p></section>`;
  const closedRow = SS.closed.find(s => s.id === d.id);
  const gone = d.status === 'gone';
  const status = SESS[d.status] ? d.status : 'idle';
  const rk = 'rename:' + d.id;
  const title = S.rename === d.id
    ? `<label class="sr" for="rename-${esc(d.id)}">New name in Midna</label><input class="sess-name-input big" id="rename-${esc(d.id)}" data-k="${esc(rk)}" data-rename="${esc(d.id)}" maxlength="80" value="${esc(S.drafts[rk] ?? d.name)}">`
    : (nv => `<h2 class="${nv.cls.trim()}"${gone ? '' : ` data-rename-id="${esc(d.id)}"`}${nv.attrs}>${esc(nv.text)}</h2>`)(nameView(d, gone ? '' : ' · double-click to rename'));
  const path = d.project_path ? d.project_path.replace(/^\/Users\/[^/]+/, '~') : d.project || '';
  const meta = [path && `<span class="mono">${esc(path)}</span>`, d.branch && `<span class="mono">${esc(d.branch)}</span>`,
    !gone && diffView(d)].filter(Boolean).join('');

  let actions = '', closeNote = '';
  if (gone) {
    if (closedRow && closedRow.can_reopen) actions = btn('Reopen its conversation', 'ss-reopen', { id: d.id, cls: 'soft', grp: 'ss:' + d.id });
    closeNote = closedRow && !closedRow.can_reopen ? 'The board doesn’t know enough about this conversation to reopen it.' : 'Reopening starts a new Midna terminal with this conversation.';
  } else if (d.closing) {
    closeNote = '';
  } else if (SS.confirm !== d.id) {
    actions = btn('Show in Midna', 'ss-focus', { id: d.id, cls: 'soft', grp: 'ss:' + d.id });
    if (d.close === 'close') actions += btn('Close terminal', 'ss-ask', { id: d.id, cls: 'danger' });
    if (d.close === 'force') actions += holdBtn(d);
  }
  const c = closeCopy(d);
  const confirm = SS.confirm === d.id && !d.closing ? `<div class="sconfirm" role="alertdialog" aria-label="${esc(c.title)}"><b>${esc(c.title)}</b><p>${esc(c.text)}</p>
      <div class="row">${btn(c.label, 'ss-close', { id: d.id, arg: d.close === 'force' ? 'force' : '', cls: 'danger solid', grp: 'ss:' + d.id })}${btn('Keep it open', 'ss-cancel', { cls: 'ghost' })}</div></div>` : '';
  const closing = d.closing ? `<p class="sclosing" role="status">Asked Midna to close ${esc(d.name)}. It moves to Closed once it’s gone.</p>` : '';

  const task = ssTaskBox(d, gone);
  const stats = [[d.stats.turns, 'Turns'], [d.stats.commits, 'Commits'], [d.stats.files, 'Files edited'], [d.last_activity ? ago(d.last_activity) : '—', 'Last activity']]
    .map(([v, k]) => `<div class="stat"><b>${esc(String(v))}</b><span>${esc(k)}</span></div>`).join('');
  const nowBox = d.waiting ? { when: `Waiting since ${hhmm(d.waiting.at)}`, text: d.waiting.text } : d.reply ? { when: `${status === 'working' ? 'Latest' : 'Last'} reply · ${hhmm(d.reply.at)}`, text: d.reply.text } : null;
  const boxes = (d.prompt || nowBox) ? `<div class="snow">
      <div class="sbox"><span class="when">${d.prompt ? `Last prompt · ${esc(hhmm(d.prompt.at))}` : 'Last prompt'}</span><p>${esc(d.prompt ? one(d.prompt.text, 500) : 'Nothing yet.')}</p></div>
      <div class="sbox${d.waiting ? ' needs' : ''}"><span class="when">${esc(nowBox ? nowBox.when : 'Last reply')}</span>${nowBox ? mdLite(nowBox.text) : '<p>Nothing yet.</p>'}</div></div>` : '';
  const tl = d.timeline.length ? `<ol class="stl">${d.timeline.map(e => {
    const [dot, label] = TIMELINE_KINDS[e.kind] || ['var(--faint)', ''];
    const files = e.files ? ` · ${plural(e.files, 'file')} edited` : '';
    return `<li><span class="t mono" title="${esc(fullTime(e.at))}">${esc(hhmm(e.at))}</span><span class="d" style="background:${dot}"></span><span class="x">${esc(one(e.text, 260))}${esc(files)}</span><span class="kl">${esc(label)}</span></li>`;
  }).join('')}</ol>` : '<p class="none-yet">Nothing recorded yet. Its turns show up here once it runs with the task board plugin.</p>';

  return `<section class="sdetail" aria-label="Session details" data-scroll="sdetail">
    <div class="shead"><div class="shead-main">
      <div class="row" style="gap:8px"><span class="pill st-${status}">${esc(d.closing ? 'Closing' : gone ? 'Closed' : SESS[status])}</span>${d.closing ? '' : `<span class="small muted">${esc(statusLine(d))}</span>`}</div>
      ${title}<div class="smeta">${meta}</div></div>
      ${actions ? `<div class="sactions">${actions}</div>` : ''}</div>
    ${closeNote ? `<p class="small muted snote">${esc(closeNote)}</p>` : ''}${note('ss:' + d.id)}${note(rk)}
    ${confirm}${closing}${task}
    <div class="sstats">${stats}</div>
    ${boxes}
    <div class="stl-wrap"><h3 class="h3">Turns and events</h3>${tl}</div>
  </section>`;
}

const HOLD_MS = 1200;
function holdBtn(d) {
  const h = SS.hold && SS.hold.id === d.id ? SS.hold : null;
  const busy = S.busy.has(`ss-hold:force:${d.id}`);
  const style = h ? ` style="animation-delay:-${Math.min(HOLD_MS, Date.now() - h.t0)}ms"` : '';
  return `<button type="button" class="btn danger solid hold${h ? ' holding' : ''}" data-hold="${esc(d.id)}" data-act="ss-hold" data-arg="force" data-id="${esc(d.id)}" data-grp="ss:${esc(d.id)}"
    title="Press and hold to stop what it’s doing and close it"${busy ? ' disabled' : ''}><span class="hold-fill"${style}></span><span>${busy ? 'Sending…' : h ? 'Keep holding…' : 'Force close'}</span></button>`;
}
function holdStart(el) {
  if (SS.hold || el.disabled) return;
  const id = el.dataset.hold;
  SS.hold = { id, t0: Date.now(), timer: setTimeout(() => {
    SS.hold = null;
    const b = document.querySelector(`[data-hold="${CSS.escape(id)}"]`) || el;
    run(b, () => post(`/sessions/${encodeURIComponent(id)}/close`, { force: true }), { ok: () => '' });
  }, HOLD_MS) };
  renderAll();
}
function holdEnd() {
  if (!SS.hold) return;
  clearTimeout(SS.hold.timer); SS.hold = null; renderAll();
}
document.addEventListener('pointerdown', e => { const el = e.target.closest('[data-hold]'); if (el && e.button === 0) { e.preventDefault(); holdStart(el); } });
['pointerup', 'pointercancel', 'blur'].forEach(k => (k === 'blur' ? window : document).addEventListener(k, holdEnd));
document.addEventListener('pointerout', e => { if (SS.hold && e.target.closest('[data-hold]') && !(e.relatedTarget && e.relatedTarget.closest && e.relatedTarget.closest('[data-hold]'))) holdEnd(); });
document.addEventListener('keydown', e => { const el = e.target.closest && e.target.closest('[data-hold]'); if (el && (e.key === ' ' || e.key === 'Enter')) { e.preventDefault(); if (!e.repeat) holdStart(el); } });
document.addEventListener('keyup', e => { if (e.key === ' ' || e.key === 'Enter') holdEnd(); });

function ssTaskBox(d, gone) {
  const row = (k, v, href, go) => `<a class="slink" href="${href}"><span class="k">${k}</span><span class="v">${v}</span><span class="go">${go}</span></a>`;
  if (d.task) {
    const t = d.task, g = t.goal;
    const st = STATUS[stKey(t)] || '';
    return `<div class="slinks">${row('Task', `<b>${esc(t.ref)}</b> ${esc(t.title)}${st ? ` <span class="pill sm st-${stKey(t)}">${esc(st)}</span>` : ''}`, `#/?task=${esc(t.ref)}`, 'Open task')}
      ${g ? row('Goal', `<b>${esc(g.ref)}</b> ${esc(g.name)}`, `#/goals/${esc(g.ref)}`, 'Open goal') : ''}</div>`;
  }
  const last = d.last_task ? row('Last task', `<b>${esc(d.last_task.ref)}</b> ${esc(d.last_task.title)}`, `#/?task=${esc(d.last_task.ref)}`, 'Open task') : '';
  const none = `<div class="slink none"><span class="k">Task</span><span class="v muted">No task</span></div>`;
  if (gone) return last ? `<div class="slinks">${last}</div>` : '';
  return `<div class="slinks">${none}${last}</div>`;
}

function ssBulkConfirm() {
  if (!SS.bulk) return '';
  const picked = [...SS.picked].map(ssSession).filter(Boolean);
  const n = picked.length;
  return `<div class="overlay" data-act="ss-bulk-bg"><div class="modal narrow" role="alertdialog" aria-modal="true" aria-label="Close these terminals?">
    <div class="modal-head"><h2>${n === 1 ? 'Close 1 idle terminal?' : `Close ${n} idle terminals?`}</h2></div>
    <p>They’re idle with no task. The board keeps each one’s conversation, so you can reopen it later. Midna may ask you to confirm on the Mac.</p>
    <ul class="sbulk-list">${picked.map(s => `<li>${esc(s.name)} · ${esc(s.project || '')} · idle for ${esc(longAgo(idleMs(s)))}</li>`).join('')}</ul>
    <div class="modal-foot">${note('ss-bulk')}${btn('Keep them open', 'ss-bulk-cancel', { cls: 'ghost' })}${btn(n === 1 ? 'Close 1 terminal' : `Close ${n} terminals`, 'ss-bulk-go', { cls: 'danger solid', grp: 'ss-bulk' })}</div>
  </div></div>`;
}

Object.assign(ACTIONS, {
  'ss-filter': el => { SS.confirm = null; nav(hashWith({ f: el.dataset.arg === 'all' ? null : el.dataset.arg }), { replace: true }); },
  'ss-open': el => {
    const s = ssSession(el.dataset.id);
    if (SS.selecting && s && bulkable(s)) { if (!SS.picked.delete(s.id)) SS.picked.add(s.id); renderAll(); return; }
    SS.confirm = null; nav(hashWith({ s: el.dataset.id }), { replace: true });
  },
  'ss-select': () => { SS.selecting = true; renderAll(); },
  'ss-done': () => { SS.selecting = false; SS.picked.clear(); renderAll(); },
  'ss-menu': el => {
    const id = el.dataset.id, s = ssSession(id);
    SS.menu = null;
    const open = () => { if (ssSel() !== id) nav(hashWith({ s: id }), { replace: true }); };
    switch (el.dataset.arg) {
      case 'open': SS.confirm = null; open(); break;
      case 'focus': post(`/sessions/${encodeURIComponent(id)}/focus`).then(() => toast(`Showing ${s ? s.name : 'it'} in Midna`)).catch(e => toast(e.message, true)); break;
      case 'rename': open(); S.rename = id; S.drafts['rename:' + id] = (s && (s.renaming || s.name)) || ''; SS.focusRename = id; break;
      case 'select': SS.selecting = true; if (!SS.picked.delete(id)) SS.picked.add(id); break;
      case 'close': open(); SS.confirm = id; break;
      case 'reopen': post(`/sessions/${encodeURIComponent(id)}/reopen`).then(() => toast('Asked Midna to reopen it in a new terminal.')).catch(e => toast(e.message, true)); break;
    }
    renderAll();
  },
  'ss-hold': () => {},
  'ss-ask': el => { SS.confirm = el.dataset.id; renderAll(); },
  'ss-cancel': () => { SS.confirm = null; renderAll(); },
  'ss-close': el => {
    const id = el.dataset.id;
    run(el, () => post(`/sessions/${encodeURIComponent(id)}/close`, { force: el.dataset.arg === 'force' }), { ok: () => { SS.confirm = null; return ''; } });
  },
  'ss-focus': el => run(el, () => post(`/sessions/${encodeURIComponent(el.dataset.id)}/focus`), { ok: () => sentNote() }),
  'ss-reopen': el => run(el, () => post(`/sessions/${encodeURIComponent(el.dataset.id)}/reopen`), { ok: () => 'Asked Midna to reopen it in a new terminal.' }),
  'ss-closed': el => { SS.closedOpen = el.getAttribute('aria-expanded') !== 'true'; if (!SS.closedOpen) SS.hideSel = ssSel(); renderAll(); },
  'ss-group': el => {
    const proj = el.dataset.id;
    if (el.getAttribute('aria-expanded') === 'true') { SS.collapsed.add(proj); SS.hideSel = ssSel(); } else SS.collapsed.delete(proj);
    try { localStorage.setItem('tb.sessCollapsed', JSON.stringify([...SS.collapsed])); } catch {}
    renderAll();
  },
  'ss-stale': () => { SS.selecting = true; (SS.list || []).filter(s => bulkable(s) && isStale(s)).forEach(s => SS.picked.add(s.id)); renderAll(); },
  'ss-bulk': () => { SS.bulk = true; renderAll(); },
  'ss-bulk-cancel': () => { SS.bulk = false; renderAll(); },
  'ss-bulk-bg': (el, e) => { if (e.target === el) { SS.bulk = false; renderAll(); } },
  'ss-bulk-go': el => {
    const ids = [...SS.picked];
    run(el, () => post('/sessions/close', { ids }), { ok: r => {
      SS.bulk = false; SS.picked.clear();
      const n = (r && r.closing || []).length, skipped = (r && r.skipped || []).length;
      toast(`Asked Midna to close ${plural(n, 'terminal')}${skipped ? `; ${skipped} got busy or picked up a task, so they stay open` : ''}. Confirm on the Mac.`);
      return '';
    } });
  },
});
Object.assign(CHANGES, {
  'ss-pick': el => { if (el.checked) SS.picked.add(el.dataset.id); else SS.picked.delete(el.dataset.id); renderAll(); },
});
document.addEventListener('input', e => { if (e.target.id === 'ss-q') { SS.q = e.target.value; renderAll(); } });
document.addEventListener('keydown', e => {
  if (e.key !== 'Escape' || S.route.page !== 'sessions') return;
  if (SS.menu) { e.stopPropagation(); closeSsMenu(); }
  else if (SS.bulk || SS.confirm) { SS.bulk = false; SS.confirm = null; renderAll(); }
  else if (SS.selecting && !e.target.closest('input, textarea')) { SS.selecting = false; SS.picked.clear(); renderAll(); }
}, true);
