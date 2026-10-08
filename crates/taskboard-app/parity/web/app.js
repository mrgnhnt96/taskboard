'use strict';

const API = '/tasks/api';
const POLL_MS = 3000;

const $ = (sel, root = document) => root.querySelector(sel);
const esc = s => s == null ? '' : String(s).replace(/[<>&"']/g, c => ({ '<': '&lt;', '>': '&gt;', '&': '&amp;', '"': '&quot;', "'": '&#39;' }[c]));
function mdLite(text, max = 1200) {
  let src = String(text || '').replace(/\r\n?/g, '\n').trim();
  if (src.length > max) { const cut = src.lastIndexOf('\n', max); src = src.slice(0, cut > max * 0.6 ? cut : max).trimEnd() + ' …'; }
  const inline = t => esc(t)
    .replace(/`([^`\n]+)`/g, '<code>$1</code>')
    .replace(/\*\*([^*\n]+)\*\*/g, '<b>$1</b>')
    .replace(/(^|[^\w*])\*([^*\n]+)\*(?!\w)/g, '$1<i>$2</i>')
    .replace(/\[([^\]\n]+)\]\((https?:\/\/[^\s)]+)\)/g, '<a href="$2" target="_blank" rel="noopener">$1</a>')
    .replace(/(^|[\s(])(https?:\/\/[^\s<)]+[^\s<).,;:!?])/g, '$1<a href="$2" target="_blank" rel="noopener">$2</a>');
  const out = [];
  let para = [], list = null, code = null;
  const flush = () => {
    if (para.length) { out.push(`<p>${para.map(inline).join('<br>')}</p>`); para = []; }
    if (list) { out.push(`<${list.tag}>${list.items.map(i => `<li>${inline(i)}</li>`).join('')}</${list.tag}>`); list = null; }
  };
  for (const line of src.split('\n')) {
    if (code) { if (/^\s*```/.test(line)) { out.push(`<pre><code>${esc(code.join('\n'))}</code></pre>`); code = null; } else code.push(line); continue; }
    if (/^\s*```/.test(line)) { flush(); code = []; continue; }
    const h = line.match(/^#{1,6}\s+(.*)$/);
    const li = line.match(/^\s*(?:([-*+])|(\d+)[.)])\s+(.*)$/);
    if (!line.trim()) flush();
    else if (h) { flush(); out.push(`<p class="md-h">${inline(h[1])}</p>`); }
    else if (li) {
      const tag = li[1] ? 'ul' : 'ol';
      if (para.length || (list && list.tag !== tag)) flush();
      (list = list || { tag, items: [] }).items.push(li[3]);
    } else if (list && /^\s+\S/.test(line)) list.items[list.items.length - 1] += ' ' + line.trim();
    else { if (list) flush(); para.push(line); }
  }
  if (code) out.push(`<pre><code>${esc(code.join('\n'))}</code></pre>`);
  flush();
  return `<div class="md">${out.join('')}</div>`;
}
const cap = s => s ? String(s)[0].toUpperCase() + String(s).slice(1) : '';
const plural = (n, one, many = one + 's') => `${n} ${n === 1 ? one : many}`;
const safeUrl = u => /^https?:\/\//i.test(String(u || '')) ? String(u) : '#';
const opt = (v, label, cur) => `<option value="${esc(v)}"${String(v) === String(cur ?? '') ? ' selected' : ''}>${esc(label)}</option>`;

function ref(x, p) {
  if (x == null || x === '') return '';
  if (typeof x === 'object') return x.ref || (x.id != null ? p + x.id : '');
  const s = String(x);
  return /^[a-z]\d+$/i.test(s) ? s.toUpperCase() : p + s;
}
const isGoalRef = r => /^G\d+$/.test(r || '');
const idOf = r => r == null || r === '' ? null : Number(String(r).replace(/^[a-z]/i, ''));
const sameRef = (a, b, p) => !!a && !!b && ref(a, p) === ref(b, p);

let serverClockOffset = 0;
function ago(iso) {
  const t = Date.parse(iso);
  if (isNaN(t)) return '';
  const m = Math.round((Date.now() + serverClockOffset - t) / 60000);
  if (m < 1) return 'just now';
  if (m < 60) return `${m}m ago`;
  if (m < 60 * 24) return `${Math.round(m / 60)}h ago`;
  return `${Math.round(m / 1440)}d ago`;
}
function hhmm(iso) {
  const d = new Date(iso);
  if (!iso || isNaN(d)) return '';
  if (d.toDateString() === new Date().toDateString())
    return d.toLocaleTimeString([], { hour: 'numeric', minute: '2-digit', hour12: true });
  return d.toLocaleDateString([], { day: 'numeric', month: 'short' });
}
function span(m) {
  if (m < 1) return 'under a minute';
  if (m < 60) return `${m}m`;
  if (m < 60 * 24) return `${Math.floor(m / 60)}h${m % 60 ? ` ${m % 60}m` : ''}`;
  return `${Math.floor(m / 1440)}d ${Math.round(m % 1440 / 60)}h`;
}
function tookLine(t) {
  const m = Math.round((Date.parse(t.finished_at) - Date.parse(t.started_at)) / 60000);
  if (t.status !== 'done' || isNaN(m) || m < 0) return '';
  return `Took ${span(m)}`;
}
function runningFor(t) {
  const m = Math.round((Date.now() + serverClockOffset - Date.parse(t.started_at)) / 60000);
  return isNaN(m) || m < 0 ? '' : `running ${span(m)}`;
}
const fullTime = iso => { const d = new Date(iso); return !iso || isNaN(d) ? '' : d.toLocaleString(); };

const ICON = {
  check: '<svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M5 12.5l4.5 4.5L19 7.5"/></svg>',
  board: '<svg viewBox="0 0 24 24" width="22" height="22" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><rect x="3" y="4" width="5" height="16" rx="1.5"/><rect x="10" y="4" width="5" height="11" rx="1.5"/><rect x="17" y="4" width="4" height="7" rx="1.5"/></svg>',
  chev: '<svg class="chev" viewBox="0 0 24 24" width="14" height="14" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M6 9l6 6 6-6"/></svg>',
  search: '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><circle cx="11" cy="11" r="6.5"/><path d="M16 16l4.5 4.5"/></svg>',
  plus: '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round"><path d="M12 5v14M5 12h14"/></svg>',
  x: '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round"><path d="M6 6l12 12M18 6L6 18"/></svg>',
  flag: '<svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M5 21V4"/><path d="M5 4h11l-2 4 2 4H5"/></svg>',
  jiraMark: '<svg class="jira-mark" viewBox="0 0 24 24" width="14" height="14" fill="currentColor" aria-hidden="true"><path d="M11.571 11.513H0a5.218 5.218 0 0 0 5.232 5.215h2.13v2.057A5.215 5.215 0 0 0 12.575 24V12.518a1.005 1.005 0 0 0-1.005-1.005zm5.723-5.756H5.736a5.215 5.215 0 0 0 5.215 5.214h2.129v2.058a5.218 5.218 0 0 0 5.215 5.214V6.758a1.001 1.001 0 0 0-1.001-1.001zM23.013 0H11.455a5.215 5.215 0 0 0 5.215 5.215h2.129v2.057A5.215 5.215 0 0 0 24 12.483V1.005A1.001 1.001 0 0 0 23.013 0z"/></svg>',
  jira: '<svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><path d="M4 6h16v12H4z"/><path d="M8 10h8M8 14h5"/></svg>',
  pr: '<svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round"><circle cx="6" cy="6" r="2.5"/><circle cx="6" cy="18" r="2.5"/><circle cx="18" cy="18" r="2.5"/><path d="M6 8.5v7"/><path d="M18 15.5V9a3 3 0 0 0-3-3h-4"/></svg>',
  prPlanned: '<svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="6" cy="6" r="2.5" stroke-dasharray="2.2 2"/><circle cx="6" cy="18" r="2.5" stroke-dasharray="2.2 2"/><circle cx="18" cy="18" r="2.5" stroke-dasharray="2.2 2"/><path d="M6 9v6M18 15.5V9a3 3 0 0 0-3-3h-4" stroke-dasharray="2 2.4"/></svg>',
  prOpen: '<svg viewBox="0 0 24 24" width="13" height="13" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="6" cy="6" r="2.6" fill="currentColor"/><circle cx="6" cy="18" r="2.6" fill="currentColor"/><circle cx="18" cy="18" r="2.6" fill="currentColor"/><path d="M6 8.5v7"/><path d="M18 15.5V9a3 3 0 0 0-3-3h-4"/><path d="M13 3.5 10.5 6 13 8.5"/></svg>',
  fwd: '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M9 6l6 6-6 6"/></svg>',
  back: '<svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><path d="M15 6l-6 6 6 6"/></svg>',
  more: '<svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><circle cx="5" cy="12" r="1.8"/><circle cx="12" cy="12" r="1.8"/><circle cx="19" cy="12" r="1.8"/></svg>',
  ext: '<svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M14 4h6v6"/><path d="M20 4l-9 9"/><path d="M18 14v5a1 1 0 0 1-1 1H5a1 1 0 0 1-1-1V7a1 1 0 0 1 1-1h5"/></svg>',
  comment: '<svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M20 12a8 8 0 0 1-11.6 7.1L4 20l1-4.1A8 8 0 1 1 20 12z"/></svg>',
  bell: '<svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M6 8a6 6 0 0 1 12 0c0 7 3 9 3 9H3s3-2 3-9M10.3 21a1.9 1.9 0 0 0 3.4 0"/></svg>',
};

const STATUS = { planned: 'Planned', queued: 'Queued', blocked: 'Blocked', working: 'Working', needs: 'Needs you', done: 'Done', failed: 'Failed' };
const stKey = t => (t.status === 'done' && t.failed) ? 'failed' : (t.status === 'queued' && t.blocked) ? 'blocked' : (STATUS[t.status] ? t.status : 'queued');
const KINDS = { bug: 'Bug', gap: 'Test gap', follow: 'Follow-up', clean: 'Clean-up' };
const kindLabel = k => KINDS[k] || cap(k) || 'Issue';
const SESS = { idle: 'Idle', working: 'Working', needs: 'Needs you', gone: 'Gone' };
const SESS_DOT = { idle: 'var(--faint)', working: 'var(--accent)', needs: 'var(--warn)', gone: 'var(--border-2)' };
const COLS = [
  ['backlog', 'Backlog', 'var(--backlog-dot)'], ['queued', 'Queued', 'var(--faint)'], ['working', 'Working', 'var(--accent)'],
  ['needs', 'Needs you', 'var(--warn)'], ['done', 'Done', 'var(--up)'],
];
const LOG_FILTERS = [['all', 'All'], ['status', 'Status'], ['turn', 'Turns'], ['checkpoint', 'Checkpoints'], ['question', 'Questions'], ['found', 'Found'], ['jira', 'Jira']];
const LOG_GROUP = { status: 'status', midna: 'status', handoff: 'status', turn: 'turn', commit: 'turn', checkpoint: 'checkpoint', question: 'question', answer: 'question', found: 'found', jira: 'jira' };
const LOG_LABEL = { status: 'Status', note: 'Note', turn: 'Turn', checkpoint: 'Checkpoint', question: 'Question', answer: 'Answer', jira: 'Jira', found: 'Found', commit: 'Commit', midna: 'Midna', handoff: 'Handoff' };
const LOG_DOT = { status: 'var(--accent)', note: 'var(--faint)', turn: 'var(--faint)', checkpoint: 'var(--accent)', question: 'var(--warn)', answer: 'var(--warn)', jira: 'var(--accent-fg)', found: 'var(--warn)', commit: 'var(--up)', midna: 'var(--faint)', handoff: 'var(--goal)' };
const HIST_DOT = { report: 'var(--warn)', seen: 'var(--seen)', note: 'var(--faint)', source: 'var(--faint)', ticket: 'var(--accent)', task: 'var(--goal)', move: 'var(--goal)', drop: 'var(--faint)', lost: 'var(--down)' };

// pr.checks is one of pass | fail | pending | none (no checks run on this PR); anything else reads as Unknown.
const CHECKS = { pass: ['Passed', 'b-pass'], fail: ['Failed', 'b-fail'], pending: ['Running', 'b-run'], none: ['No checks', 'b-unk'] };
const checksOf = pr => CHECKS[String((pr && pr.checks) || '').toLowerCase()] || ['Unknown', 'b-unk'];
// pr.review is one of approved | changes | pending (asked, nobody has answered) | none (nobody asked yet).
const REVIEW = { approved: 'Approved', changes: 'Changes asked', pending: 'Waiting', none: 'Not asked' };
const prOpen = p => (p.state || 'OPEN').toUpperCase() === 'OPEN';
const jiraOn = () => !!(S.state && S.state.jira && S.state.jira.enabled);
const uncommitted = n => typeof n === 'number' ? (n === 0 ? 'Nothing' : plural(n, 'file')) : String(n);

function whenLine(t) {
  const a = ago(t.when || t.updated_at);
  if (!a) return '';
  const verb = t.status === 'queued' || t.status === 'planned' ? 'Added'
    : t.status === 'working' ? 'Updated'
    : t.status === 'needs' ? (t.lost ? 'Lost' : t.question ? 'Asked' : 'Updated')
    : t.failed ? 'Stopped' : 'Finished';
  return `${verb} ${a}`;
}

async function api(path, body) {
  const opts = { cache: 'no-store', headers: { Accept: 'application/json' } };
  if (body !== undefined) {
    opts.method = 'POST';
    opts.headers['Content-Type'] = 'application/json';
    opts.headers['X-Task-Board'] = '1';
    opts.body = JSON.stringify(body || {});
  }
  let r;
  try { r = await fetch(API + path, opts); }
  catch { throw new Error('Can’t reach the task board server.'); }
  let data = null;
  try { data = await r.json(); } catch {}
  if (!r.ok) throw new Error((data && data.error) || `The board answered ${r.status}.`);
  return data;
}
const post = (path, body = {}) => api(path, body);

const S = {
  route: { page: 'board', id: null, q: {} },
  state: null, stateErr: '',
  filter: { project: 'all', goal: 'all' },
  goalsAll: null,
  task: null, taskRef: null, taskErr: '', handoffs: {},
  issue: null, issueRef: null, issueErr: '',
  keepIssues: [],
  drafts: {},
  busy: new Set(), notes: {}, reveal: {}, metaEdit: {},
  logFilter: 'all', rename: null, renameWatch: {}, renameFlash: {}, picker: null, hoursMenu: null,
  modal: null,
};

function loadFilter() {
  try {
    const f = JSON.parse(localStorage.getItem('taskboard.filter') || '{}');
    if (typeof f.project === 'string') S.filter.project = f.project;
    if (f.goal === 'all' || isGoalRef(f.goal)) S.filter.goal = f.goal;
    if (['24h', '7d', 'all'].includes(f.done)) S.filter.done = f.done;
  } catch {}
}
function saveFilter() {
  try { localStorage.setItem('taskboard.filter', JSON.stringify(S.filter)); } catch {}
}

const allGoals = () => (S.goalsAll || (S.state && S.state.goals) || []).filter(g => !g.archived);
const goalByRef = r => allGoals().find(g => sameRef(g, r, 'G'));
function projectNames() {
  const set = new Set();
  ((S.state && S.state.projects) || []).forEach(p => { const n = typeof p === 'string' ? p : p && p.name; if (n) set.add(n); });
  allGoals().forEach(g => g.project && set.add(g.project));
  return [...set].sort((a, b) => a.localeCompare(b));
}
const issueGoalId = b => b.goal_id != null ? b.goal_id : (b.goal && b.goal.id != null ? b.goal.id : null);
function issueGoalName(b) {
  if (b.goal && b.goal.name) return b.goal.name;
  const id = issueGoalId(b);
  const g = id != null && allGoals().find(x => x.id === id);
  return g ? g.name : '';
}
function sentNote() {
  return S.state && S.state.midna && S.state.midna.up === false ? 'Saved. It runs once Midna is back.' : 'Sent to Midna';
}
const sessionLive = t => !!(t.session && t.session.status !== 'gone');

function parseRoute() {
  const h = location.hash.replace(/^#/, '') || '/';
  const i = h.indexOf('?');
  const path = i < 0 ? h : h.slice(0, i);
  const q = Object.fromEntries(new URLSearchParams(i < 0 ? '' : h.slice(i + 1)));
  const parts = path.split('/').filter(Boolean);
  if (parts[0] === 'goals') { const id = parts[1] ? ref(parts[1], 'G') : null; return { page: 'goal', id: isGoalRef(id) ? id : null, q }; }
  if (parts[0] === 'backlog') return { page: 'backlog', id: null, q };
  if (parts[0] === 'sessions') return { page: 'sessions', id: null, q };
  return { page: 'board', id: null, q };
}
function routeHash(page, id, q) {
  const path = page === 'goal' ? `/goals${id ? '/' + id : ''}` : page === 'backlog' ? '/backlog' : page === 'sessions' ? '/sessions' : '/';
  const clean = {};
  for (const [k, v] of Object.entries(q || {})) if (v != null && v !== '') clean[k] = v;
  const qs = new URLSearchParams(clean).toString();
  return '#' + path + (qs ? '?' + qs : '');
}
const hashWith = changes => routeHash(S.route.page, S.route.id, { ...S.route.q, ...changes });
let lastHash = null;
function nav(hash, { replace = false } = {}) {
  const url = location.pathname + location.search + hash;
  if (replace) history.replaceState(null, '', url); else history.pushState(null, '', url);
  onRoute();
}
function onRoute() {
  if (location.hash === lastHash) return;
  lastHash = location.hash;
  const prev = S.route;
  S.route = parseRoute();
  const tr = S.route.q.task ? ref(S.route.q.task, 'T') : null;
  if (tr !== S.taskRef) { S.taskRef = tr; S.task = null; S.taskErr = ''; }
  if (prev.page !== S.route.page || prev.id !== S.route.id) {
    if (typeof resetPageState === 'function') resetPageState(prev, S.route);
  }
  const ir = currentIssueRef();
  if (ir !== S.issueRef) { S.issueRef = ir; S.issue = null; S.issueErr = ''; }
  renderAll();
  refresh();
}
function openTask(r, tab, fromPanel = false) {
  const cur = S.route.q.task, next = ref(r, 'T');
  const trail = fromPanel && cur && cur !== next ? [...taskTrail(), cur].join(',') : null;
  nav(hashWith({ task: next, issue: S.route.page === 'board' ? null : S.route.q.issue, tab: tab || null, from: trail }));
}

function taskTrail() { return (S.route.q.from || '').split(',').filter(Boolean); }

function currentIssueRef() {
  const r = S.route;
  if (r.q.issue) return ref(r.q.issue, 'B');
  if (r.page === 'goal' && r.q.view === 'backlog' && typeof goalBacklogRows === 'function') {
    const first = goalBacklogRows()[0];
    return first ? ref(first, 'B') : null;
  }
  if (r.page === 'backlog' && typeof backlogRows === 'function') {
    const first = backlogRows()[0];
    return first ? ref(first, 'B') : null;
  }
  return null;
}

async function loadState() {
  const f = S.filter;
  const q = new URLSearchParams({ project: f.project || 'all', goal: f.goal === 'all' ? 'all' : String(idOf(f.goal)), done: f.done || '24h' });
  if (S.keepIssues.length) q.set('keep', S.keepIssues.join(','));
  try {
    const st = await api('/state?' + q);
    S.state = st || {};
    S.stateErr = '';
    const now = Date.parse(st && st.now);
    if (!isNaN(now)) serverClockOffset = now - Date.now();
  } catch (e) { S.stateErr = e.message; }
}
async function loadGoals() {
  try { const r = await api('/goals'); S.goalsAll = Array.isArray(r) ? r : (r && r.goals) || []; }
  catch {}
}
async function loadTask() {
  const r = S.taskRef;
  if (!r) return;
  try {
    const t = await api('/tasks/' + encodeURIComponent(r));
    if (S.taskRef !== r) return;
    S.task = t; S.taskErr = '';
    if (handoffText(t) == null && S.route.q.tab === 'context') loadHandoff(r);
  } catch (e) { if (S.taskRef === r) S.taskErr = e.message; }
}
async function loadHandoff(r) {
  try { const h = await api(`/tasks/${encodeURIComponent(r)}/handoff`); S.handoffs[r] = h && (h.text ?? h.handoff ?? ''); }
  catch (e) { S.handoffs[r] = `Couldn’t load the handoff: ${e.message}`; }
}
async function loadIssue() {
  const r = S.issueRef;
  if (!r) return;
  try {
    const b = await api('/backlog/' + encodeURIComponent(r));
    if (S.issueRef === r) { S.issue = b && (b.issue || b); S.issueErr = ''; }
  } catch (e) { if (S.issueRef === r) S.issueErr = e.message; }
}

let refreshing = null, refreshAgain = false;
async function refresh() {
  if (refreshing) { refreshAgain = true; return refreshing; }
  refreshing = (async () => {
    const jobs = [loadState(), loadGoals(), loadTask()];
    if (typeof pageLoads === 'function') jobs.push(...pageLoads());
    if (typeof sessionsLoads === 'function') jobs.push(...sessionsLoads());
    await Promise.all(jobs);
    const ir = currentIssueRef();
    if (ir !== S.issueRef) { S.issueRef = ir; S.issue = null; S.issueErr = ''; }
    await loadIssue();
    if (typeof afterLoad === 'function') afterLoad();
    if (typeof afterSessionsLoad === 'function') afterSessionsLoad();
    renderAll();
  })();
  try { await refreshing; } finally {
    refreshing = null;
    if (refreshAgain) { refreshAgain = false; refresh(); }
  }
}
let pollTimer = null;
async function tick() {
  clearTimeout(pollTimer);
  if (!document.hidden) { try { await refresh(); } catch {} }
  pollTimer = setTimeout(tick, POLL_MS);
}

function focusKey(a) {
  if (a.id) return '#' + CSS.escape(a.id);
  for (const attr of ['data-k', 'data-m', 'data-meta']) if (a.hasAttribute(attr)) return `[${attr}="${CSS.escape(a.getAttribute(attr))}"]`;
  if (a.dataset.act) return `[data-act="${CSS.escape(a.dataset.act)}"]` + (a.dataset.arg ? `[data-arg="${CSS.escape(a.dataset.arg)}"]` : '') + (a.dataset.id ? `[data-id="${CSS.escape(a.dataset.id)}"]` : '');
  return null;
}
let patching = false;
function patch(el, html) {
  if (!el || el._html === html) return;
  const scrolls = {};
  el.querySelectorAll('[data-scroll]').forEach(n => { scrolls[n.dataset.scroll] = n.scrollTop; });
  const a = document.activeElement;
  let focus = null;
  if (a && a !== document.body && el.contains(a)) {
    focus = { key: focusKey(a) };
    try { focus.s = a.selectionStart; focus.e = a.selectionEnd; } catch {}
  }
  patching = true;
  try { el.innerHTML = html; } finally { patching = false; }
  el._html = html;
  el.querySelectorAll('[data-scroll]').forEach(n => { if (n.dataset.scroll in scrolls) n.scrollTop = scrolls[n.dataset.scroll]; });
  if (focus && focus.key) {
    const n = el.querySelector(focus.key);
    if (n) {
      n.focus({ preventScroll: true });
      try { if (focus.s != null) n.setSelectionRange(focus.s, focus.e); } catch {}
    }
  }
}

const busyKey = el => [el.dataset.act || el.dataset.change || '', el.dataset.arg || '', el.dataset.id || ''].join(':');
function btn(label, act, o = {}) {
  const key = [act, o.arg ?? '', o.id ?? ''].join(':');
  const busy = S.busy.has(key);
  const attrs = [
    `data-act="${esc(act)}"`,
    o.arg != null && o.arg !== '' ? `data-arg="${esc(o.arg)}"` : '',
    o.id != null && o.id !== '' ? `data-id="${esc(o.id)}"` : '',
    o.grp ? `data-grp="${esc(o.grp)}"` : '',
    o.title ? `title="${esc(o.title)}"` : '',
    (o.disabled || busy) ? 'disabled' : '',
  ].filter(Boolean).join(' ');
  return `<button type="button" class="btn ${o.cls || ''}" ${attrs}>${o.icon || ''}${esc(busy ? (o.busyLabel || 'Sending…') : label)}</button>`;
}
function note(grp) {
  const n = S.notes[grp];
  return n ? `<span class="note ${n.err ? 'err' : 'ok'}" role="status">${esc(n.text)}</span>` : '';
}
function setNote(grp, text, err) {
  const n = { text, err: !!err };
  S.notes[grp] = n;
  setTimeout(() => { if (S.notes[grp] === n) { delete S.notes[grp]; renderAll(); } }, err ? 15000 : 5000);
}
async function copyText(text) {
  try { await navigator.clipboard.writeText(text); return true; } catch {}
  const box = document.createElement('textarea');
  box.value = text;
  box.setAttribute('readonly', '');
  box.style.cssText = 'position:fixed;top:0;left:0;opacity:0;pointer-events:none';
  document.body.appendChild(box);
  box.select();
  let ok = false;
  try { ok = document.execCommand('copy'); } catch {}
  box.remove();
  return ok;
}
let toastTimer = null;
function toast(msg, err) {
  const root = $('#toast-root');
  root.innerHTML = `<div class="toast${err ? ' err' : ''}">${esc(msg)}</div>`;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { root.innerHTML = ''; }, err ? 6000 : 3000);
}

async function run(el, fn, o = {}) {
  const key = busyKey(el);
  const grp = o.grp || (el && el.dataset.grp);
  if (S.busy.has(key)) return;
  S.busy.add(key);
  if (grp) delete S.notes[grp];
  renderAll();
  try {
    const r = await fn();
    S.busy.delete(key);
    const ok = typeof o.ok === 'function' ? o.ok(r) : (o.ok ?? sentNote());
    if (grp && ok) setNote(grp, ok);
    else if (ok && o.toast) toast(ok);
    await refresh();
    return r;
  } catch (err) {
    S.busy.delete(key);
    if (grp) setNote(grp, err.message, true); else toast(err.message, true);
    renderAll();
  }
}

function renderAll() {
  renderBanner();
  patch($('#statusbar'), statusBarHtml());
  renderMenus();
  renderPage();
  renderPanel();
  const needs = S.state && S.state.counts ? S.state.counts.needs : 0;
  const base = S.route.page === 'goal' ? 'Goals' : S.route.page === 'backlog' ? 'Backlog' : S.route.page === 'sessions' ? 'Sessions' : 'Task board';
  document.title = needs ? `(${needs}) ${base}` : base;
}

function renderBanner() {
  let html = '';
  if (S.stateErr) {
    html = `<div class="banner down" role="status"><span class="dot"></span><span>${esc(S.stateErr)} Trying again every few seconds.</span></div>`;
  }
  const alerts = currentAlerts();
  if (alerts.length > 1) {
    html += `<div class="banner down alert" role="alert"><span class="dot"></span>
    <span class="banner-text grow">${plural(alerts.length, 'task needs', 'tasks need')} your attention. <small>${esc(ago(alerts[alerts.length - 1].at))}</small></span>
    ${btn('Show all', 'alerts-open', { cls: 'soft sm' })}</div>`;
  } else {
    html += alerts.map(a => `<div class="banner down alert" role="alert"><span class="dot"></span>
    <span class="banner-text grow">${esc(a.text)} <small>${esc(ago(a.at))}</small></span>
    ${alertOpenBtn(a)}
    ${alertStays(a) ? '' : btn('Dismiss', 'alert-dismiss', { id: a.id, cls: 'ghost sm' })}</div>`).join('');
  }
  patch($('#banner'), html);
  if (S.modal && S.modal.kind === 'alerts') { if (currentAlerts().length) renderModal(); else closeModal(); }
}
const currentAlerts = () => (S.state && S.state.alerts) || [];
const alertStays = a => !!a.review;
const alertOpenBtn = a => a.task || a.goal ? btn('Open ' + (a.task || a.goal), 'alert-open', { id: a.id, cls: 'soft sm' }) : '';
function alertsDialogHtml() {
  const alerts = currentAlerts();
  return `<div class="overlay" data-act="modal-bg"><div class="modal narrow" role="dialog" aria-modal="true" aria-label="Needs your attention">
    <div class="modal-head"><h2>${plural(alerts.length, 'task needs', 'tasks need')} your attention</h2>${closeBtn('modal-close')}</div>
    <ul class="alert-list">${alerts.map(a => `<li><span class="dot"></span>
      <span class="grow">${esc(a.text)} <small>${esc(ago(a.at))}</small></span>
      <span class="alert-acts">${alertOpenBtn(a)}${alertStays(a) ? '' : btn('Dismiss', 'alert-dismiss', { id: a.id, cls: 'ghost sm' })}</span></li>`).join('')}</ul>
    ${alerts.some(a => !alertStays(a)) ? `<div class="modal-foot">${btn('Dismiss all', 'alerts-dismiss-all', { cls: 'ghost sm' })}</div>` : ''}
  </div></div>`;
}

function renderPage() {
  const page = $('#page');
  const kind = S.route.page;
  if (page.dataset.kind !== kind) { page.dataset.kind = kind; page.innerHTML = ''; page._html = null; page.scrollTop = 0; }
  if (kind === 'board') renderBoard(page);
  else if (kind === 'goal' && typeof renderGoalPage === 'function') renderGoalPage(page);
  else if (kind === 'backlog' && typeof renderBacklogPage === 'function') renderBacklogPage(page);
  else if (kind === 'sessions' && typeof renderSessionsPage === 'function') renderSessionsPage(page);
}

function renderBoard(page) {
  if (!page.querySelector('.board')) {
    page.innerHTML = '<div class="boardpage"><nav class="bgoals" id="b-goals" aria-label="Goals"></nav><div class="board"><section class="sessions-wrap" id="b-sessions" aria-label="Sessions"></section><div id="b-goalbar"></div><main class="cols" id="b-cols"></main></div><div class="rail-grip" role="separator" aria-orientation="vertical" aria-label="Resize goals" tabindex="0" title="Drag to resize · double-click to reset"></div></div>';
    page._html = null;
    bindRailGrip(page.querySelector('.boardpage'));
  }
  patch($('#b-goals'), goalsRailHtml());
  patch($('#b-sessions'), sessionsHtml());
  patch($('#b-goalbar'), goalBarHtml());
  patch($('#b-cols'), columnsHtml());
}

function statusBarHtml() {
  const m = S.state && S.state.midna;
  return `${m ? connPill(m) : ''}${hoursPill()}${usagePill()}`;
}

const RAIL_W = { min: 220, max: 600, dflt: 300 };
function railWidth() { try { const w = Number(localStorage.getItem('tb.rail.w')); return w ? Math.min(RAIL_W.max, Math.max(RAIL_W.min, w)) : RAIL_W.dflt; } catch { return RAIL_W.dflt; } }
function setRailWidth(el, w, save) {
  w = Math.round(Math.min(RAIL_W.max, Math.max(RAIL_W.min, w)));
  el.style.setProperty('--rail-w', w + 'px');
  if (save) try { localStorage.setItem('tb.rail.w', String(w)); } catch {}
  return w;
}
function bindRailGrip(el) {
  const grip = el.querySelector('.rail-grip');
  setRailWidth(el, railWidth());
  grip.addEventListener('pointerdown', e => {
    if (e.button !== 0) return;
    e.preventDefault();
    grip.setPointerCapture(e.pointerId);
    grip.classList.add('drag'); document.body.classList.add('rail-dragging');
    const left = el.getBoundingClientRect().left;
    const move = ev => setRailWidth(el, ev.clientX - left);
    const up = ev => {
      setRailWidth(el, ev.clientX - left, true);
      grip.classList.remove('drag'); document.body.classList.remove('rail-dragging');
      grip.removeEventListener('pointermove', move); grip.removeEventListener('pointerup', up); grip.removeEventListener('pointercancel', up);
    };
    grip.addEventListener('pointermove', move); grip.addEventListener('pointerup', up); grip.addEventListener('pointercancel', up);
  });
  grip.addEventListener('dblclick', () => { try { localStorage.removeItem('tb.rail.w'); } catch {} setRailWidth(el, RAIL_W.dflt); });
  grip.addEventListener('keydown', e => {
    const step = e.key === 'ArrowLeft' ? -16 : e.key === 'ArrowRight' ? 16 : 0;
    if (step) { e.preventDefault(); setRailWidth(el, railWidth() + step, true); }
  });
}

function railShut() { try { return new Set(JSON.parse(localStorage.getItem('tb.rail.shut') || '[]')); } catch { return new Set(); } }
function setRailShut(set) { try { localStorage.setItem('tb.rail.shut', JSON.stringify([...set])); } catch {} }
const goalDone = g => goalCounts(g).finished;
const DAY_MS = 24 * 3600e3;
function recentSince(now) {
  const d = new Date(now), day = d.getDay();
  const weekendOrMonday = day === 6 || day === 0 || day === 1;
  if (!weekendOrMonday) return now - DAY_MS;
  const daysSinceFriday = (day + 2) % 7;
  const fridayStart = new Date(d.getFullYear(), d.getMonth(), d.getDate() - daysSinceFriday);
  return Math.min(fridayStart.getTime(), now - DAY_MS);
}
function recentlyDone(g) {
  const at = Date.parse(g.finished_at);
  if (isNaN(at)) return false;
  const now = Date.now() + serverClockOffset;
  return at >= recentSince(now);
}
function railRecentOpen() { try { return localStorage.getItem('tb.rail.recent') === 'open'; } catch { return false; } }
function goalRowHtml(g) {
  const r = ref(g, 'G');
  const c = goalCounts(g);
  const sel = sameRef(r, S.filter.goal, 'G');
  const st = goalStatus(g, c);
  return `<div class="bg-item${sel ? ' on' : ''}${c.finished || g.deprioritized ? ' finished' : ''}">
    <button type="button" class="bg-pick" data-act="goal-pick" data-arg="${esc(r)}" aria-pressed="${sel}" data-peek="${esc(r)}">${goalRing(c, st)}<span class="bg-name"><span class="bg-ref">${esc(r)}</span> ${esc(g.name)}</span>${st ? `<span class="sr">${esc(st.label)}</span>` : ''}<span class="bg-n">${c.done}/${c.n}</span></button>
    ${sel ? `<a class="bg-open" href="#/goals/${esc(r)}" aria-label="Open ${esc(g.name)}" title="Open the goal">${ICON.fwd}</a>` : ''}</div>`;
}
const GOAL_GLYPH = {
  working: '<path d="M7.7 6.4v5.2l4.1-2.6z"/>',
  starting: '<path d="M7.7 6.4v5.2l4.1-2.6z"/>',
  needs: '<path d="M9 5.8v3.7M9 12.2v.01"/>',
  paused: '<path d="M7.6 6.6v4.8M10.4 6.6v4.8"/>',
  held: '<path d="M10.6 6.1a3.1 3.1 0 1 0 2 4.9 2.5 2.5 0 0 1-2-4.9z"/>',
  blocked: '<rect x="6.6" y="8.6" width="4.8" height="3.4" rx=".7"/><path d="M7.6 8.6V7.7a1.4 1.4 0 0 1 2.8 0v.9"/>',
  queued: '<path d="M12.1 9a3.1 3.1 0 1 1-6.2 0 3.1 3.1 0 0 1 6.2 0zM9 7.4V9l1.1.8"/>',
};
function agentsHeldWhy() {
  const h = S.state && S.state.work_hours;
  const five = ((S.state && S.state.usage && S.state.usage.windows) || []).find(w => w.key === 'five_hour');
  if (h && h.on && !h.open) return 'After hours';
  if (five && five.pct >= 100) return 'Out of usage';
  return '';
}
const agentsHeld = () => !!agentsHeldWhy();
function goalStatus(g, c) {
  if (c.finished) return null;
  if (c.n && c.done === c.n && c.prs) return { key: 'queued', label: c.prs > 1 ? `${c.prs} PRs awaiting merge` : 'Awaiting merge' };
  const queued = g.queued ?? c.queued ?? 0;
  if (g.needs) return { key: 'needs', label: `${plural(g.needs, 'task')} need${g.needs === 1 ? 's' : ''} you` };
  if (c.active) return { key: 'working', label: 'Running' };
  if (g.starting) return { key: 'starting', label: 'Starting' };
  if (g.paused) return { key: 'paused', label: 'Paused' };
  if (!queued) return null;
  if (g.blocked && g.blocked >= queued) return { key: 'blocked', label: 'Blocked' };
  if (agentsHeld()) return { key: 'held', label: 'Queued until agents can start' };
  return { key: 'queued', label: 'Queued' };
}
function goalRing(c, st) {
  const r = 7, circ = 2 * Math.PI * r;
  const d = c.n ? c.done / c.n : 0, a = c.n ? (c.done + c.active) / c.n : 0;
  const tick = c.finished ? '<path class="tick" d="M5.5 9.2l2.3 2.3 4.5-4.8"/>'
    : st && GOAL_GLYPH[st.key] ? `<g class="glyph g-${st.key}">${GOAL_GLYPH[st.key]}</g>` : '';
  return `<svg class="ring" width="22" height="22" viewBox="0 0 18 18" aria-hidden="true"><circle class="r0" cx="9" cy="9" r="${r}"/>
    <circle class="ra" cx="9" cy="9" r="${r}" stroke-dasharray="${(a * circ).toFixed(1)} ${circ.toFixed(1)}"/><circle class="rd" cx="9" cy="9" r="${r}" stroke-dasharray="${(d * circ).toFixed(1)} ${circ.toFixed(1)}"/>${tick}</svg>`;
}
const PEEK_GROUPS = [['needs', 'Needs you'], ['working', 'Running'], ['failed', 'Failed'], ['starting', 'Starting'], ['blocked', 'Blocked']];
const PEEK_CHIP = { needs: 'needs', working: 'working', failed: 'failed', starting: 'working', blocked: 'blocked' };
function goalPeekHtml(g) {
  const c = goalCounts(g);
  const st = goalStatus(g, c);
  const peek = g.peek || [];
  const groups = PEEK_GROUPS.map(([k, label]) => {
    const ts = peek.filter(t => t.status === k);
    if (!ts.length) return '';
    return `<div class="gp-grp"><div class="gp-h"><span class="chip st-${PEEK_CHIP[k]}">${label}</span><span class="bg-n">${ts.length}</span></div>
      ${ts.map(t => `<button type="button" class="gp-task" data-act="open-task" data-id="${esc(t.ref)}"><span class="gp-ref">${esc(t.ref)}</span><span class="gp-t"><b>${esc(t.title)}</b>${t.why ? `<span class="gp-why">${esc(t.why)}</span>` : ''}</span></button>`).join('')}</div>`;
  }).join('');
  const planned = g.planned ? ` · ${g.planned} planned` : '';
  return `<div class="gp-head"><b><span class="gi-ref">${esc(ref(g, 'G'))}</span>${esc(g.name)}</b><span class="gp-sub">${st && agentsHeld() ? `<span class="gp-held">${esc(agentsHeldWhy())}</span> · ` : ''}${st ? esc(st.key === 'held' ? 'Queued' : st.label) + ' · ' : ''}${c.done} of ${c.n} done${planned}</span></div>${groups}`;
}
const PEEK = { el: null, for: null, show: 0, hide: 0 };
function peekEl() {
  if (!PEEK.el) {
    PEEK.el = document.createElement('div');
    PEEK.el.className = 'gpeek';
    PEEK.el.setAttribute('role', 'tooltip');
    PEEK.el.addEventListener('mouseenter', () => clearTimeout(PEEK.hide));
    PEEK.el.addEventListener('mouseleave', () => peekHideSoon());
    PEEK.el.addEventListener('click', () => peekHide());
    document.body.appendChild(PEEK.el);
  }
  return PEEK.el;
}
function peekShow(anchor) {
  const g = goalByRef(anchor.dataset.peek);
  if (!g) return;
  const el = peekEl();
  el.innerHTML = goalPeekHtml(g);
  el.hidden = false;
  PEEK.for = anchor;
  const r = anchor.getBoundingClientRect(), w = el.offsetWidth, h = el.offsetHeight;
  const left = r.right + 8 + w <= innerWidth - 8 ? r.right + 8 : Math.max(8, r.left - 8 - w);
  el.style.left = left + 'px';
  el.style.top = Math.max(8, Math.min(r.top, innerHeight - 8 - h)) + 'px';
}
function peekHide() {
  clearTimeout(PEEK.show); clearTimeout(PEEK.hide);
  if (PEEK.el) PEEK.el.hidden = true;
  PEEK.for = null;
}
function peekHideSoon() { clearTimeout(PEEK.show); clearTimeout(PEEK.hide); PEEK.hide = setTimeout(peekHide, 150); }
document.addEventListener('mouseover', e => {
  const a = e.target.closest && e.target.closest('[data-peek]');
  if (!a) return;
  clearTimeout(PEEK.hide);
  if (PEEK.for === a) return;
  clearTimeout(PEEK.show);
  PEEK.show = setTimeout(() => peekShow(a), PEEK.for ? 0 : 300);
});
document.addEventListener('mouseout', e => {
  const a = e.target.closest && e.target.closest('[data-peek]');
  if (a && !(e.relatedTarget && a.contains(e.relatedTarget))) peekHideSoon();
});
document.addEventListener('focusin', e => { const a = e.target.closest && e.target.closest('[data-peek]'); if (a && a.matches(':focus-visible')) peekShow(a); });
document.addEventListener('focusout', e => { if (e.target.closest && e.target.closest('[data-peek]')) peekHideSoon(); });
document.addEventListener('scroll', peekHide, true);
document.addEventListener('mousedown', e => { if (PEEK.for && !e.target.closest('.gpeek')) peekHide(); }, true);
function railClear() {
  S.filter.goal = 'all'; S.filter.project = 'all';
  S.keepIssues = []; saveFilter(); renderAll(); refresh();
}
function goalsRailHtml() {
  const f = S.filter;
  const every = allGoals();
  const goals = every.filter(g => !goalDone(g) && !g.deprioritized);
  const recent = every.filter(goalDone).filter(recentlyDone).sort((a, b) => (Date.parse(b.finished_at) || 0) - (Date.parse(a.finished_at) || 0));
  const st = S.state || {};
  const seen = new Set(st.session_projects || []);
  goals.forEach(g => g.project && seen.add(g.project));
  if (f.project !== 'all') seen.add(f.project);
  const shut = railShut();
  const projects = [...seen].sort((a, b) => a.localeCompare(b));
  const groups = projects.map(p => {
    const gs = goals.filter(g => g.project === p);
    const on = f.project === p, closed = shut.has(p) && !gs.some(g => sameRef(g, f.goal, 'G'));
    const needs = gs.reduce((n, g) => n + (g.needs || 0), 0);
    const head = `<div class="bg-proj${on ? ' on' : ''}">
      <button type="button" class="bg-fold" data-act="rail-fold" data-arg="${esc(p)}" aria-expanded="${!closed}" aria-label="${closed ? 'Show' : 'Hide'} ${esc(p)} goals">${closed ? ICON.fwd : ICON.chev}</button>
      <button type="button" class="bg-pname" data-act="rail-project" data-arg="${esc(p)}" aria-pressed="${on}" title="${on ? 'Show every project' : 'Show only ' + esc(p)}"><span>${esc(p)}</span>${closed && needs ? '<span class="bg-dot" aria-label="Needs you"></span>' : ''}<span class="bg-n">${gs.length}</span></button></div>`;
    if (closed) return head;
    return head + gs.map(goalRowHtml).join('');
  }).join('');
  const none = !projects.length ? `<p class="bg-none">${S.goalsAll || S.state ? 'No goals yet' : 'Loading…'}</p>` : '';
  return `<div class="bg-brand"><a class="logo" href="#/" aria-label="Task board" title="Task board">${ICON.board}</a><h1>Task board</h1><span class="grow"></span>${btn('New task', 'new-task', { cls: 'primary sm', icon: ICON.plus })}</div>
    <div class="bg-head"><h2 class="h3"><a class="bg-title" href="#/goals" title="See every goal: active, deprioritized and finished">Goals${ICON.fwd}</a></h2><span class="count">${goals.length}</span><span class="grow"></span><button type="button" class="icon-btn sm" data-act="new-goal" aria-label="New goal" title="New goal">${ICON.plus}</button></div>
    <div class="bg-list">${groups}</div>${none}${recent.length ? recentHtml(recent) : ''}`;
}
function recentHtml(list) {
  const open = railRecentOpen() || list.some(g => sameRef(g, S.filter.goal, 'G'));
  return `<div class="bg-recent"><button type="button" class="bg-proj bg-rhead" data-act="rail-recent" aria-expanded="${open}">
      <span class="bg-fold" aria-hidden="true">${open ? ICON.chev : ICON.fwd}</span><span class="bg-pname"><span>Recently completed</span><span class="bg-n">${list.length}</span></span></button>
    ${open ? `<div class="bg-list">${list.map(goalRowHtml).join('')}</div>` : ''}</div>`;
}
function goalBarHtml() {
  const g = S.filter.goal !== 'all' && goalByRef(S.filter.goal);
  if (!g) return '';
  const c = goalCounts(g);
  const bits = [`${c.done} of ${c.n} done`, g.needs ? `${g.needs} need${g.needs === 1 ? 's' : ''} you` : '', c.active - (g.needs || 0) > 0 ? `${c.active - (g.needs || 0)} working` : '',
    g.open_issues ? `${g.open_issues} in the backlog` : '', g.deprioritized ? 'Deprioritized' : g.paused ? 'Paused' : ''].filter(Boolean).join(' · ');
  const r = ref(g, 'G');
  return `<div class="goalbar"><span class="gb-flag">${ICON.flag}</span><b>${esc(g.name)}</b><span class="gb-sum">${esc(bits)}</span><span class="grow"></span>
    <a class="btn goal-btn" href="#/goals/${esc(r)}">Open goal${ICON.fwd}</a>
    <button type="button" class="icon-btn" data-act="goal-pick" data-arg="all" aria-label="Show every goal" title="Show every goal">${ICON.x}</button></div>`;
}

const inFilter = (project, goalRef) => (S.filter.project === 'all' || project === S.filter.project)
  && (S.filter.goal === 'all' || sameRef(goalRef, S.filter.goal, 'G'));
const filterTasks = list => list.filter(t => inFilter(t.project, t.goal));
const filterIssues = list => list.filter(b => inFilter(b.project, issueGoalId(b) != null ? { id: issueGoalId(b) } : null));

function sessionsHtml() {
  const st = S.state;
  if (!st) return `<h2 class="h3">Sessions</h2><p class="none-yet">${S.stateErr ? 'Can’t load the sessions.' : 'Loading…'}</p>`;
  const order = { needs: 0, working: 1, idle: 2, gone: 3 };
  const list = (st.sessions || [])
    .filter(s => S.filter.project === 'all' || s.project === S.filter.project)
    .sort((a, b) => (order[a.status] ?? 2) - (order[b.status] ?? 2));
  const shown = list.slice(0, STRIP_MAX);
  const body = list.length ? `<div class="sessions">${shown.map(sessCard).join('')}</div>`
    : `<p class="none-yet">No Midna terminals${S.filter.project !== 'all' ? ' in ' + esc(S.filter.project) : ''} yet. They show up here once Midna lists them.</p>`;
  const more = list.length > shown.length ? `<span class="sess-more">Showing ${shown.length} of ${list.length}</span>` : '';
  return `<div class="sess-top"><h2 class="h3"><a class="sess-title" href="#/sessions" title="See every session">Sessions</a></h2>${more}</div>${body}`;
}
const STRIP_MAX = 6;
const SESS_ORDER = { needs: 0, working: 1, idle: 2, gone: 3 };

function sessCard(s) {
  const status = SESS[s.status] ? s.status : 'idle';
  const sub = s.task_title || '';
  const rk = 'rename:' + s.id;
  const editing = S.rename === s.id;
  const nv = nameView(s);
  const nameHtml = editing
    ? `<label class="sr" for="rename-${esc(s.id)}">New name in Midna</label><input class="sess-name-input" id="rename-${esc(s.id)}" data-k="${esc(rk)}" data-rename="${esc(s.id)}" maxlength="80" value="${esc(S.drafts[rk] ?? s.name ?? '')}">`
    : `<span class="sess-name${nv.cls}" data-rename-id="${esc(s.id)}"${nv.attrs}>${esc(nv.text)}</span>`;
  const tr = s.task_ref || '';
  const click = ` data-act="open-session" data-id="${esc(s.id)}" role="button" tabindex="0" title="See what ${esc(s.name || 'it')} has done"`;
  return `<div class="sess s-${status} clickable${status === 'needs' ? ' needs' : ''}${status === 'gone' ? ' gone' : ''}"${click}>
    <div class="sess-head"><span class="dot"></span>${nameHtml}<span class="sess-state">${esc(SESS[status])}</span></div>
    <span class="sess-proj" title="${esc(s.project_path || '')}">${esc(s.project || '')}</span>
    ${tr ? `<span class="sess-sub sess-task" data-act="open-task" data-id="${esc(tr)}" role="button" tabindex="0" title="Open ${esc(tr)}">${esc(sub)}</span>` : sub ? `<span class="sess-sub" title="${esc(sub)}">${esc(sub)}</span>` : ''}${note(rk)}</div>`;
}

function columnsHtml() {
  const st = S.state;
  if (!st) return COLS.map(([k, name, color]) => colShell(k, name, color, '', `<p class="empty">${S.stateErr ? 'Can’t load the board.' : 'Loading…'}</p>`)).join('');
  const c = st.columns || {};
  return COLS.map(([k, name, color]) => {
    if (k === 'backlog') {
      const issues = filterIssues(c.backlog || []);
      const shownOpen = issues.filter(b => (b.state || 'open') === 'open').length;
      const open = c.backlog_open ?? shownOpen;
      const more = open - shownOpen;
      const body = issues.map(issueCard).join('')
        + (more > 0 ? `<a class="more" href="#/backlog">${more} more on the Backlog page</a>` : '')
        + (issues.length ? '' : '<p class="empty">Nothing here</p>');
      return colShell(k, name, color, open, body, '<a class="end" href="#/backlog">See all</a>');
    }
    const tasks = filterTasks(c[k] || []);
    if (k === 'done') {
      const hidden = (st.counts && st.counts.done_hidden) || 0;
      const win = DONE_WINDOWS.find(([id]) => id === (S.filter.done || '24h'));
      const empty = `<p class="empty">${hidden ? `Nothing in the ${esc(win[2])}` : 'Nothing here'}</p>`;
      const more = hidden ? `<button type="button" class="more btn link" data-act="done-window" data-arg="${S.filter.done === '24h' || !S.filter.done ? '7d' : 'all'}">${plural(hidden, 'older task')} hidden · show more</button>` : '';
      return colShell(k, name, color, tasks.length, (tasks.map(taskCard).join('') || empty) + more, doneMenuButton(), note('done-col'));
    }
    return colShell(k, name, color, tasks.length, tasks.map(taskCard).join('') || '<p class="empty">Nothing here</p>');
  }).join('');
}
const DONE_WINDOWS = [['24h', 'Last 24 hours', 'last 24 hours'], ['7d', 'Last 7 days', 'last 7 days'], ['all', 'Everything', 'whole history']];
function doneMenuButton() {
  const open = S.doneMenu;
  const cur = S.filter.done || '24h';
  const menu = open ? `<div class="ctxmenu att-menu" role="menu" aria-label="Done column">
      <span class="menu-h">Show</span>${DONE_WINDOWS.map(([id, l]) => `<button type="button" role="menuitemradio" aria-checked="${cur === id}" data-act="done-window" data-arg="${id}">${cur === id ? '✓ ' : ''}${esc(l)}</button>`).join('')}
      <hr><button type="button" role="menuitem" data-act="close-done" data-grp="done-col"${(S.state && (S.state.columns.done || []).length) ? '' : ' disabled'}>Close their terminals</button></div>` : '';
  return `<span class="end att-acts"><button type="button" class="icon-btn sm" data-act="done-menu" aria-haspopup="menu" aria-expanded="${!!open}" aria-label="Done column options" title="Options">${ICON.more}</button>${menu}</span>`;
}
document.addEventListener('dragstart', e => {
  const card = e.target.closest && e.target.closest('[data-drag]');
  if (!card) return;
  S.dragging = card.dataset.drag;
  e.dataTransfer.effectAllowed = 'move';
  e.dataTransfer.setData('text/plain', S.dragging);
  dropTargets(true);
});
document.addEventListener('dragend', () => { S.dragging = null; dropTargets(false); });
function dropTargets(on) {
  document.querySelectorAll('[data-drop="working"]').forEach(c => { c.classList.toggle('drop-ready', on); if (!on) c.classList.remove('drop-over'); });
}
document.addEventListener('dragover', e => {
  const col = S.dragging && e.target.closest && e.target.closest('[data-drop="working"]');
  if (!col) return;
  e.preventDefault();
  e.dataTransfer.dropEffect = 'move';
  col.classList.add('drop-over');
});
document.addEventListener('dragleave', e => {
  const col = e.target.closest && e.target.closest('[data-drop="working"]');
  if (col && !col.contains(e.relatedTarget)) col.classList.remove('drop-over');
});
document.addEventListener('drop', e => {
  const col = S.dragging && e.target.closest && e.target.closest('[data-drop="working"]');
  if (!col) return;
  e.preventDefault();
  const r = S.dragging;
  S.dragging = null;
  dropTargets(false);
  run({ dataset: { id: r } }, () => post(`/tasks/${encodeURIComponent(r)}/start`, { mode: 'new' }), { ok: `Starting ${r}`, toast: true });
});
document.addEventListener('mousedown', e => { if (S.doneMenu && !e.target.closest('.att-menu, [data-act="done-menu"]')) { S.doneMenu = false; renderAll(); } }, true);
document.addEventListener('keydown', e => { if (S.doneMenu && e.key === 'Escape') { S.doneMenu = false; renderAll(); } });

function colShell(k, name, color, n, body, extra = '', sub = '') {
  const drop = k === 'working' ? ' data-drop="working"' : '';
  return `<section class="col${drop && S.dragging ? ' drop-ready' : ''}" aria-label="${esc(name)}"${drop}>
    <div class="col-head"><span class="sq" style="background:${color}"></span><h2>${esc(name)}</h2>${n === '' ? '' : `<span class="n">${esc(n)}</span>`}${extra}</div>
    ${sub ? `<div class="col-note">${sub}</div>` : ''}
    <div class="col-body" data-scroll="col-${k}">${body}</div></section>`;
}

const startsByHand = t => (t.status === 'queued' || t.status === 'planned') && !t.goal && !t.starting;
function taskCard(t) {
  const r = ref(t, 'T');
  const sel = sameRef(S.route.q.task, r, 'T');
  const pr = t.pr && t.pr.num != null ? t.pr : null;
  const [bl, bc] = pr ? checksOf(pr) : [];
  const drag = startsByHand(t) ? ` draggable="true" data-drag="${esc(r)}" title="Drag to Working to start it"` : '';
  return `<button type="button" class="tcard${t.status === 'needs' ? ' needs' : ''}" data-act="open-task" data-id="${esc(r)}" aria-pressed="${sel}"${drag}>
    <span class="t-top">${t.goal ? `<span class="goal-line" title="${esc(t.goal.name)}">${ICON.flag}<span>${esc(ref(t.goal, 'G'))}</span></span><span class="t-ref" aria-hidden="true">·</span>` : ''}<span class="t-ref">${esc(r)}</span><span class="grow"></span><span class="t-when" title="${esc(whenLine(t))}">${esc(ago(t.when || t.updated_at))}</span></span>
    <span class="t-title"><b>${esc(t.title)}</b>${t.priority === 'high' ? '<span class="chip high">High</span>' : ''}</span>
    <span class="chips"><span class="chip repo">${esc(t.project)}</span>${t.jira && t.jira.key ? `<span class="chip jira">${ICON.jira}${esc(t.jira.key)}</span>` : ''}${pr ? `<span class="chip ${bc}">${ICON.pr}PR #${esc(pr.num)} · ${esc(bl)}</span>` : ''}${t.failed ? '<span class="chip bad">Failed</span>' : ''}${t.lost ? '<span class="chip bad">Terminal lost</span>' : ''}</span>
    ${t.who ? `<span class="foot"><b>${esc(t.who)}</b></span>` : ''}
  </button>`;
}

function issueState(b) {
  const tr = b.task_id != null ? 'T' + b.task_id : '';
  if (b.state === 'task') return { short: 'Now a task', text: tr ? `Made into task ${tr}` : 'Made into a task', cls: 'st-task' };
  if (b.state === 'ticket') return { short: b.jira_key || 'Ticket asked for', text: b.jira_key ? `Jira ${b.jira_key}` : 'Jira ticket being created', cls: 'st-ticket' };
  if (b.state === 'drop') return { short: 'Won’t do', text: 'Closed as won’t do', cls: 'st-drop' };
  return null;
}
function foundTask(b) {
  const f = b.found_by_task;
  if (!f) return null;
  return typeof f === 'object' ? { ...f, ref: ref(f, 'T') } : { id: f, ref: ref(f, 'T') };
}
function issueFrom(b, long) {
  const ft = foundTask(b);
  let who;
  if (b.source === 'you' || (!b.source && !b.found_by_name && !ft)) who = 'Added by you';
  else if (b.source === 'answer') who = 'From your answer';
  else if (long && ft) who = `Found by ${ft.ref} · ${b.found_by_name || 'a terminal'}`;
  else who = b.found_by_name || 'Found by a terminal';
  return [who, hhmm(b.created_at)].filter(Boolean).join(' · ');
}
function issueCard(b) {
  const r = ref(b, 'B');
  const sel = S.route.page === 'board' && sameRef(S.route.q.issue, r, 'B');
  const info = b.state && b.state !== 'open' ? issueState(b) : null;
  const gid = issueGoalId(b);
  return `<button type="button" class="icard" data-act="open-issue" data-id="${esc(r)}" aria-pressed="${sel}">
    <span class="chips"><span class="chip k-${esc(b.kind)}">${esc(kindLabel(b.kind))}</span>${info ? `<span class="chip ${info.cls}">${esc(info.short)}</span>` : ''}<span class="grow"></span><span class="t-when" title="${esc(cap(issueFrom(b)))}">${esc(ago(b.created_at))}</span></span>
    <span class="t-title"><b>${esc(b.title)}</b></span>
    <span class="goal-line${gid != null ? '' : ' none'}" title="${esc(issueGoalName(b))}">${ICON.flag}<span>${esc(gid != null ? ref(gid, 'G') : 'Not in a goal')}</span></span>
  </button>`;
}

function renderPanel() {
  const r = S.route;
  let html = '';
  if (r.q.task) html = taskPanel();
  else if (r.page === 'board' && r.q.issue) html = issuePanel();
  patch($('#panel-root'), html);
}
const backBtn = () => {
  const prev = taskTrail().at(-1);
  return prev ? `<button type="button" class="icon-btn" data-act="task-back" aria-label="Back to ${esc(prev)}" title="Back to ${esc(prev)}">${ICON.back}</button>` : '';
};
const closeBtn = act => `<button type="button" class="icon-btn" data-act="${act}" aria-label="Close">${ICON.x}</button>`;
const lrow = (k, v) => `<div class="lrow"><span class="k">${esc(k)}</span><div class="v">${v}</div></div>`;

function taskPanel() {
  const t = S.task;
  if (!t) {
    return `<aside class="panel" aria-label="Task detail"><div class="panel-top">${backBtn()}<span class="pill ref">${esc(S.taskRef)}</span><div class="grow"></div>${closeBtn('close-panel')}</div>
      <p class="${S.taskErr ? 'note err' : 'muted'}">${esc(S.taskErr || 'Loading…')}</p></aside>`;
  }
  const r = ref(t, 'T');
  const k = stKey(t);
  const alert = currentAlerts().find(a => a.task === r);
  const tab = ['context', 'log'].includes(S.route.q.tab) ? S.route.q.tab : 'overview';
  const body = tab === 'context' ? contextTab(t) : tab === 'log' ? logTab(t) : overviewTab(t);
  const tabs = [['overview', 'Overview'], ['context', 'Context'], ['log', 'Log']].map(([id, label]) =>
    `<button type="button" role="tab" class="tab" aria-selected="${tab === id}" data-act="tab" data-arg="${id}">${label}</button>`).join('');
  return `<aside class="panel" aria-label="Task detail" data-scroll="task-${esc(r)}-${tab}">
    <div class="panel-top">${backBtn()}${statusPill(t, k)}${alert ? `<span class="pill st-needs" title="${esc(alert.text)}">Needs you</span>` : ''}${t.priority === 'high' ? '<span class="pill high">High priority</span>' : ''}<span class="pill ref">${esc(r)}</span><div class="grow"></div>${closeBtn('close-panel')}</div>
    ${S.taskErr ? `<p class="note err">${esc(S.taskErr)}</p>` : ''}
    <div class="stack" style="gap:4px"><h2 class="title">${esc(t.title)}</h2>
      <div class="subline"><span class="pill sm repo">${esc(t.project)}</span><span>${esc(whenLine(t))}</span>${tookLine(t) ? `<span>${esc(tookLine(t))}</span>` : ''}${(t.status === 'working' || t.status === 'needs') && runningFor(t) ? `<span>${esc(cap(runningFor(t)))}</span>` : ''}</div></div>
    ${alert ? `<div class="box ask" role="alert"><span class="box-label">Needs you</span><p>${esc(alert.text)}</p></div>` : ''}
    <div class="tabs" role="tablist" aria-label="Task sections">${tabs}</div>
    <div class="tabbody" role="tabpanel">${body}</div>
  </aside>`;
}

const awaitingMerge = t => t.status === 'done' && !t.failed && t.pr && t.pr.num != null && prOpen(t.pr);
const prStopped = t => awaitingMerge(t) && t.pr.stage && t.pr.stage.stopped;
const prStatus = t => prStopped(t) ? ['Needs you', 'st-needs', t.pr.stage.stopped.asked ? `Its terminal asked you about PR #${t.pr.num}` : `Its terminal stopped on PR #${t.pr.num} before it was finished`]
  : t.pr.stage ? [t.pr.stage.label, PR_STAGE_CLS[t.pr.stage.phase] || 'st-working', `The work is done; PR #${t.pr.num} isn’t merged yet`]
  : ['Awaiting merge', 'st-working', `The work is done; PR #${t.pr.num} isn’t merged yet`];
function hoursOpenAt() {
  const h = S.state && S.state.work_hours;
  if (!h || !h.on || h.open || !h.next_open) return '';
  const nxt = new Date(h.next_open);
  return (nxt.toDateString() === new Date().toDateString() ? '' : nxt.toLocaleDateString(undefined, { weekday: 'short' }) + ' ')
    + clock12(nxt.toTimeString().slice(0, 5));
}
function answerBtns(r) {
  const at = hoursOpenAt();
  return btn('Send answer', 'answer', { id: r, cls: 'primary', grp: 'ask:' + r })
    + (at ? btn(`Send at ${at}`, 'answer', { id: r, arg: 'morning', grp: 'ask:' + r, title: 'Hold the answer until work hours start; the task doesn’t start before then' }) : '');
}
function statusPill(t, k) {
  if (awaitingMerge(t)) { const [label, cls, tip] = prStatus(t); return `<span class="pill ${cls}" title="${esc(tip)}">${esc(label)}</span>`; }
  if (k === 'queued' && t.starting) return '<span class="pill st-queued">Starting</span>';
  return `<span class="pill st-${k}">${STATUS[k]}</span>`;
}

function savedLine(t) {
  const c = t.context || {};
  return c.saved_at ? `Saved at ${hhmm(c.saved_at)} · ${plural(c.turns || 0, 'turn')} · ${plural(c.checkpoints || 0, 'checkpoint')}`
    : '';
}

function overviewTab(t) {
  const r = ref(t, 'T');
  const who = t.who || (t.session && t.session.name) || 'The terminal';
  const out = [];
  const act = 'act:' + r;

  if (t.lost) {
    out.push(`<div class="box lost"><span class="box-label">Terminal lost</span>
      <p>${esc(t.latest || 'The terminal closed before the task was done.')}</p>
      ${savedLine(t) ? `<p class="sub2">${esc(savedLine(t))}</p>` : ''}
      <div class="row">${btn('Resume in a new terminal', 'resume', { id: r, arg: 'fresh', cls: 'primary', grp: 'lost:' + r })}${btn('Reopen the old conversation', 'resume', { id: r, arg: 'reopen', grp: 'lost:' + r, disabled: !t.claude_session_id, title: t.claude_session_id ? '' : 'No conversation was saved for this task' })}</div>
      ${note('lost:' + r)}
      <button type="button" class="btn link" data-act="tab" data-arg="context">See what the new terminal gets</button></div>`);
  } else if (t.status === 'needs' && t.needs_reason === 'start_failed') {
    out.push(`<div class="box lost"><span class="box-label">Couldn’t start in Midna</span>
      <p>${esc(t.latest || 'Midna didn’t start a terminal for this task.')}</p>
      <div class="row">${btn('New Midna terminal', 'start', { id: r, arg: 'new', cls: 'primary', grp: act })}${btn('Queue in Midna', 'start', { id: r, arg: 'queue', grp: act })}</div>${note(act)}</div>`);
  } else if (t.status === 'needs') {
    const k = 'answer:' + r;
    out.push(`<div class="box ask"><span class="box-label">${esc(who)} ${t.question ? 'is asking' : 'needs you'}</span>
      <p>${esc(t.question || t.latest || 'It’s waiting for you in Midna.')}</p>
      <textarea class="input" id="ans-${esc(r)}" data-k="${esc(k)}" rows="2" aria-label="Your answer">${esc(S.drafts[k] || '')}</textarea>
      <div class="row">${answerBtns(r)}${!t.question && sessionLive(t) ? btn('Focus in Midna', 'focus', { id: r, grp: 'ask:' + r }) : ''}${note('ask:' + r)}</div></div>`);
  }

  if (startsByHand(t)) {
    out.push(`<div class="stack"><div class="row" style="flex-wrap:nowrap">${btn('Start', 'start', { id: r, arg: 'new', cls: 'primary lg wide', grp: act })}${btn('Start when the repo’s free', 'start', { id: r, arg: 'queue', cls: 'soft lg wide', grp: act })}</div>
      ${note(act)}${t.waiting ? `<p class="help" style="color:var(--warn-fg)">Not starting yet: ${esc(t.waiting)}. Start runs it now anyway.</p>` : ''}</div>`);
  } else if (t.status === 'planned') {
    out.push(`<div class="stack"><div class="row">${btn('Queue it now', 'queue-planned', { id: r, cls: 'soft', grp: act, title: 'Take it out of the plan and queue it; the goal’s rules decide when it starts' })}</div>${note(act)}</div>`);
  }

  if (prStopped(t)) {
    const st = t.pr.stage;
    const k = 'answer:' + r;
    const term = (t.terminals || []).find(x => x.id === st.session);
    out.push(`<div class="box ask"><span class="box-label">${esc((term && term.name) || 'Its terminal')} ${st.stopped.asked ? 'asks you about the PR' : 'stopped on the PR'}</span>
      <p>${esc(st.stopped.message)}</p>
      <textarea class="input" id="ans-${esc(r)}" data-k="${esc(k)}" rows="2" aria-label="Your answer">${esc(S.drafts[k] || '')}</textarea>
      <div class="row">${answerBtns(r)}${btn('Focus in Midna', 'focus', { id: r, grp: 'ask:' + r })}${note('ask:' + r)}</div></div>`);
  }

  if (t.status === 'done') {
    out.push(`<div class="box ${t.failed ? 'failed' : 'done'}"><details class="box-fold" data-fold="summary"${foldOpen('summary', true) ? ' open' : ''}><summary class="box-label">Summary from ${esc(who)}</summary>
      <p>${esc(t.summary || t.latest || 'No summary was given.')}</p></details>
      <div class="row">${btn(t.failed ? 'Try again' : 'Queue again', 'requeue', { id: r, grp: act })}${sessionLive(t) ? btn('Close its terminal', 'close-term', { id: r, cls: 'danger', grp: act }) : ''}</div>${note(act)}</div>`);
  } else if (t.status !== 'planned') {
    out.push(manageBox(t));
  }

  out.push(`<details class="linked-d" data-linked${linkedOpen() ? ' open' : ''}><summary>Details</summary><section class="linked" aria-label="Linked">${blockedRow(t)}${goalRow(t)}${terminalRow(t)}${jiraRow(t)}${prRow(t)}${attachRow(t)}</section></details>`);
  out.push(`<details class="linked-d" data-fold="what"${foldOpen('what', true) ? ' open' : ''}><summary>What to do</summary>${t.detail ? `<div class="note-body what">${noteBody(t.detail)}</div>` : '<p class="what">Nothing written yet.</p>'}</details>`);
  return out.join('');
}

// Your manual overrides for a task that isn't done: focus, detach, close the terminal, mark done or failed.
function manageBox(t) {
  const r = ref(t, 'T');
  const grp = 'manage:' + r;
  const live = sessionLive(t);
  const buttons = [
    live && t.status === 'working' ? btn('Focus in Midna', 'focus', { id: r, cls: 'sm', grp }) : '',
    t.session_id || live ? btn('Detach', 'detach', { id: r, cls: 'sm', grp, title: 'Take it off its terminal and put it back in the queue' }) : '',
    live ? btn('Close its terminal', 'close-term', { id: r, arg: t.session && t.session.status === 'idle' ? '' : 'force', cls: 'sm danger', grp, title: 'Close the terminal; a busy one is stopped' }) : '',
  ].filter(Boolean).join('');
  return `<div class="stack manage" style="gap:8px"><div class="row" style="gap:6px">${buttons}</div>${note(grp)}</div>`;
}

function linkedOpen() { try { return localStorage.getItem('tb.linked') !== 'closed'; } catch { return true; } }
function foldOpen(k, shut = false) { try { const v = localStorage.getItem('tb.fold.' + k); return v ? v === 'open' : !shut; } catch { return !shut; } }
document.addEventListener('toggle', e => {
  const d = e.target;
  if (!d.matches) return;
  const key = d.matches('details[data-linked]') ? 'tb.linked' : d.matches('details[data-fold]') ? 'tb.fold.' + d.dataset.fold : null;
  if (!key) return;
  try { localStorage.setItem(key, d.open ? 'open' : 'closed'); } catch {}
}, true);

function blockedRow(t) {
  const bs = t.blocked_by || [];
  if (!bs.length) return '';
  return lrow('Blocked by', bs.map(b => `<div class="line"><button type="button" class="btn link inline" data-act="open-task" data-id="${esc(b.ref)}">${esc(b.ref)}</button>${statusPill(b, stKey(b))}</div>
    <span>${esc(b.title)}</span>`).join('')).replace('class="lrow"', 'class="lrow blocked"');
}

function goalRow(t) {
  const g = t.goal;
  if (g && (g.id != null || g.ref)) {
    const gr = ref(g, 'G');
    if (S.route.page === 'goal' && sameRef(S.route.id, gr, 'G')) return '';
    const pos = Number.isInteger(g.position) && g.total && g.position <= g.total ? `Task ${g.position} of ${g.total}` : '';
    const n = g.open_issues || 0;
    return lrow('Goal', `<div class="line"><a class="goal-name" href="#/goals/${esc(gr)}">${esc(g.name)}</a>${pos ? `<span class="small">${pos}</span>` : ''}</div>
      <span class="small">${g.next_title ? `Next in this goal: ${esc(g.next_title)}` : 'Last task in this goal'}</span>
      <a class="small" style="color:var(--accent);align-self:flex-start" href="#/goals/${esc(gr)}?view=backlog">${n ? `${n} open in the goal’s backlog` : 'Nothing in the goal’s backlog'}</a>`);
  }
  return lrow('Goal', '<span class="small">Not in a goal.</span>');
}

function termHref(id) { return routeHash('sessions', null, { s: id, back: location.hash }); }
function waitsOnYou(x, cur) {
  const stage = (cur.pr || {}).stage;
  return !!(stage && stage.stopped && stage.session === x.id);
}
const WHY_SHORT = { 'Worked on the task': 'Task', 'Worked on the PR': 'PR' };
function termItem(x, cur) {
  const raw = SESS[x.status] ? x.status : 'gone';
  const waiting = raw !== 'gone' && waitsOnYou(x, cur);
  const st = waiting ? 'needs' : raw;
  const live = st !== 'gone';
  const why = waiting ? (cur.pr.stage.stopped.asked ? 'Asked you a question' : 'Stopped before it was finished') : WHY_SHORT[x.why] || x.why;
  const name = x.name || x.id;
  const state = live ? SESS[st] : cur.lost && x.id === (cur.session || {}).id ? 'Gone' : 'Closed';
  const head = x.id ? `<a class="term-link" href="${esc(termHref(x.id))}" title="Open this terminal">${esc(name)}</a>` : `<span>${esc(name)}</span>`;
  const focus = live && x.id ? `<button type="button" class="icon-btn xs" data-act="term-focus" data-id="${esc(x.id)}" data-grp="term:${esc(ref(cur, 'T'))}" title="Focus in Midna" aria-label="Focus ${esc(name)} in Midna">${ICON.ext}</button>` : '';
  return `<li class="tl-item${live ? ' live' : ''}"><span class="tl-dot"${live ? ` style="background:${SESS_DOT[st]}"` : ''}></span>
    <div class="tl-body"><div class="tl-name">${head}${focus}</div><div class="tl-sub"><span class="pill sm st-${live ? st : 'closed'}">${esc(state)}</span>${why ? `<span>${esc(why)}</span>` : ''}</div></div>
    ${x.at ? `<span class="tl-at">${esc(hhmm(x.at))}</span>` : ''}</li>`;
}
function terminalRow(t) {
  const grp = 'term:' + ref(t, 'T');
  const s = t.session;
  let all = t.terminals || [];
  if (!all.length && sessionLive(t)) all = [{ id: s.id, name: s.name || t.who || s.id, status: s.status, why: 'Attached', at: t.started_at }];
  if (!all.length && t.who && t.status !== 'queued' && t.status !== 'planned') all = [{ name: t.who, status: 'gone' }];
  if (!all.length) return lrow('Terminal', t.status === 'done' ? '<span class="small">No terminal.</span>' : `<span class="small">No terminal yet.</span>${note(grp)}`);
  const cur = (s || {}).id;
  const rank = x => x.id && x.id === cur ? 0 : x.status && x.status !== 'gone' ? 1 : 2;
  all = all.map((x, i) => [x, i]).sort((a, b) => rank(a[0]) - rank(b[0]) || a[1] - b[1]).map(([x]) => x);
  const shown = all.length > 2 ? all.slice(0, 1) : all;
  const rest = all.slice(shown.length);
  const more = rest.length ? `<details class="tl-more" data-fold="terms"${foldOpen('terms', true) ? ' open' : ''}><summary>${plural(rest.length, 'earlier terminal')}</summary><ol class="tl">${rest.map(x => termItem(x, t)).join('')}</ol></details>` : '';
  return lrow(all.length > 1 ? 'Terminals' : 'Terminal', `<ol class="tl">${shown.map(x => termItem(x, t)).join('')}</ol>${more}${note(grp)}`);
}

// Jira is optional: the row only shows when the server says it's set up (state.jira.enabled), or the task already has a ticket.
function jiraRow(t) {
  const j = t.jira;
  if (!jiraOn() && !(j && j.key)) return '';
  let v;
  if (j && j.key) {
    const key = j.url ? `<a class="jkey" href="${esc(safeUrl(j.url))}" target="_blank" rel="noopener" title="Open ${esc(j.key)} in Jira">${esc(j.key)}</a>` : `<span class="jkey">${esc(j.key)}</span>`;
    v = `<div class="line">${key}${j.status ? `<span class="pill sm jira">${esc(j.status)}</span>` : ''}</div>`;
  } else if (j) {
    v = `<span class="small">${esc(j.status || 'Ticket asked for')}</span>`;
  } else {
    v = '<span class="small">No ticket.</span>';
  }
  return lrow('Jira', v);
}

const PR_STAGE_CLS = { checks: 'st-queued', fix: 'st-failed', review: 'st-review', rereview: 'st-review', comments: 'st-needs',
  merge: 'st-working', merged: 'st-done', declined: 'neutral' };
const PR_AGENT_PHASES = new Set(['fix', 'comments', 'merge']);
const STEP_ICON = {
  done: ICON.check,
  fail: '<svg viewBox="0 0 24 24" width="11" height="11" fill="none" stroke="currentColor" stroke-width="3" stroke-linecap="round" aria-hidden="true"><path d="M6 6l12 12M18 6L6 18"/></svg>',
  ask: ICON.comment,
  wait: '<svg viewBox="0 0 24 24" width="12" height="12" fill="none" stroke="currentColor" stroke-width="2.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="12" cy="12" r="8.5"/><path d="M12 7.5V12l3 2"/></svg>',
};
function checksStep(p) {
  const s = { name: 'Checks' };
  const c = String(p.checks || '').toLowerCase();
  const phase = p.stage && p.stage.phase;
  if (c === 'none') return { ...s, st: 'done', sub: 'None' };
  if (c === 'pass') return { ...s, st: 'done', sub: 'Passed' };
  if (c === 'fail' && phase === 'fix') return { ...s, st: 'fail', sub: p.stage.stopped ? 'Fix stopped' : p.stage.session ? 'Being fixed' : 'Fix starting' };
  if (c === 'fail') return { ...s, st: 'fail', sub: 'Failed' };
  if (c === 'pending') return { ...s, st: 'now', sub: 'Running' };
  return { ...s, st: prOpen(p) ? 'wait' : 'todo', sub: 'Waiting' };
}
function reviewStep(p) {
  const s = { name: 'Review' };
  const r = String(p.review || '').toLowerCase();
  if (p.stage && p.stage.phase === 'rereview') return { ...s, st: 'wait', sub: 'Awaiting re-review' };
  if (r === 'changes') return { ...s, st: 'ask', sub: REVIEW.changes };
  if (p.stage && p.stage.phase === 'comments') return { ...s, st: 'ask', sub: 'New comments' };
  if (r === 'approved') return { ...s, st: 'done', sub: REVIEW.approved };
  if (r === 'pending') return { ...s, st: 'wait', sub: REVIEW.pending };
  return { ...s, st: 'todo', sub: REVIEW.none };
}
function mergeStep(p) {
  const phase = p.stage && p.stage.phase;
  const state = (p.state || 'OPEN').toUpperCase();
  if (state === 'MERGED') return { name: 'Merge', st: 'done', sub: 'Merged' };
  if (state !== 'OPEN') return { name: 'Merge', st: 'fail', sub: 'Declined' };
  if (phase === 'merge') return { name: 'Merge', st: 'now', sub: 'Merging' };
  return { name: 'Merge', st: 'todo' };
}
function stepHtml(x) {
  const inner = `<span class="pr-step-dot">${STEP_ICON[x.st] || ''}</span><span class="pr-step-name">${esc(x.name)}</span>${x.sub ? `<span class="pr-step-sub">${esc(x.sub)}</span>` : ''}`;
  return `<span class="pr-step s-${x.st}">${inner}</span>`;
}
function prBar(t, p) {
  const stage = p.stage;
  if (!prOpen(p) || !stage || !PR_AGENT_PHASES.has(stage.phase)) return '';
  const body = `<span class="dot"></span><span class="grow">${esc(stage.stopped ? `Needs you · ${stage.label}` : stage.label)}</span>`;
  return stage.session
    ? `<a class="pr-bar${stage.stopped ? ' warn' : ''}" href="${esc(termHref(stage.session))}" title="${stage.stopped ? 'Its terminal stopped before it was finished' : 'Open the terminal doing this'}">${body}<span class="go">Terminal ›</span></a>`
    : `<div class="pr-bar" title="Waiting for its terminal to open">${body}</div>`;
}
function prRow(t) {
  const p = t.pr;
  if (!p || p.num == null) return lrow('Pull request', '<span class="small">No PR yet.</span>');
  const key = t.jira && t.jira.key;
  const title = key && (p.title || '').startsWith(key) ? p.title.slice(key.length).replace(/^[\s:–-]+/, '') : p.title;
  const num = `<b>#${esc(p.num)}</b> ${esc(title || p.repo || '')}`;
  const steps = [checksStep(p), reviewStep(p), mergeStep(p)];
  return lrow('Pull request', `<div class="pr-head">${p.url ? `<a class="pr-link" href="${esc(safeUrl(p.url))}" target="_blank" rel="noopener" title="Open the pull request">${num}</a>` : `<span class="pr-link">${num}</span>`}
      <span class="small pr-repo">${title ? esc(p.repo) : ''}</span></div>
    <div class="pr-steps" aria-label="Where the PR is">${steps.map(stepHtml).join('<span class="pr-step-line" aria-hidden="true"></span>')}</div>
    ${prBar(t, p)}`);
}
const attHref = a => /^https?:\/\//i.test(a.url || '') ? safeUrl(a.url) : /^[~/]/.test(a.url || '') ? `/tasks/files/${encodeURIComponent(a.id)}` : '';
const ATT_KIND = { design: 'Design', proposal: 'Proposal', doc: 'Doc', evidence: 'Evidence', results: 'Results', other: 'Link' };
const ATT_SVG = p => `<svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${p}</svg>`;
const ATT_ICON = {
  design: ATT_SVG('<rect x="2.5" y="2.5" width="11" height="11" rx="2"/><path d="M2.5 6.5h11M6.5 6.5v7"/>'),
  proposal: ATT_SVG('<path d="M8 2.5a4 4 0 0 0-2.4 7.2c.5.4.9 1 .9 1.6v.2h3v-.2c0-.6.4-1.2.9-1.6A4 4 0 0 0 8 2.5z"/><path d="M6.5 13.5h3"/>'),
  doc: ATT_SVG('<path d="M4 2.5h5.5L12 5v8.5H4z"/><path d="M6.5 8h3M6.5 10.5h3"/>'),
  evidence: ATT_SVG('<path d="M2.5 13.5h11"/><path d="M4.5 11V8"/><path d="M8 11V4.5"/><path d="M11.5 11V6.5"/>'),
  results: ATT_SVG('<path d="M2.5 13.5h11"/><path d="M3.5 10.5l3-3 2.5 2 4-4.5"/><path d="M10 5h3v3"/>'),
  other: ATT_SVG('<path d="M6.8 9.2a2.5 2.5 0 0 0 3.6 0l2-2a2.5 2.5 0 0 0-3.6-3.6l-.6.6"/><path d="M9.2 6.8a2.5 2.5 0 0 0-3.6 0l-2 2a2.5 2.5 0 0 0 3.6 3.6l.6-.6"/>'),
};
function attSrc(a, owner, r) {
  const s = a.task || a.goal;
  if (!s || (owner === 'goals' && s === r)) return '';
  return a.task
    ? `<button type="button" class="att-src t" data-act="open-task" data-id="${esc(s)}" title="Open ${esc(s)}">${esc(s)}</button>`
    : `<a class="att-src g" href="#/goals/${esc(s)}" title="Open ${esc(s)}">${esc(s)}</a>`;
}
function attachList(list, owner, r) {
  const fk = `att:${owner}:${r}`;
  const items = (list || []).map(a => {
    const web = /^https?:\/\//i.test(a.url || '');
    const href = attHref(a);
    const kind = ATT_KIND[a.kind] ? a.kind : 'other';
    const name = href ? `<a class="att-name" href="${esc(href)}" target="_blank" rel="noopener" title="${esc(web ? a.title : `${a.title}\n${a.url}`)}">${esc(a.title)}</a>`
      : `<span class="att-name mono" title="${esc(a.url)}">${esc(a.title)}</span>`;
    const src = attSrc(a, owner, r);
    const meta = [esc(ATT_KIND[kind]), src, a.at ? esc(ago(a.at)) : ''].filter(Boolean).join(' · ');
    const open = S.attMenu === String(a.id);
    return `<li class="att${open ? ' menu-open' : ''}"><span class="att-k ${kind}" aria-hidden="true">${ATT_ICON[kind]}</span><span class="att-t">${name}<span class="att-m">${meta}</span></span>
      <span class="att-acts"><button type="button" class="icon-btn xs" data-act="att-menu" data-id="${esc(a.id)}" aria-haspopup="menu" aria-expanded="${open}" aria-label="Actions for ${esc(a.title)}">${ICON.more}</button>${attMenuHtml(a, web, fk, owner !== 'goals')}</span></li>`;
  }).map((html, k) => owner !== 'goals' && S.attEdit === String((list || [])[k].id) ? attEditHtml(list[k]) : html).join('');
  return { items, fk, n: (list || []).length };
}
function attMenuHtml(a, web, fk, edit) {
  if (S.attMenu !== String(a.id)) return '';
  const item = (act, label, extra = '') => `<button type="button" role="menuitem" data-act="${act}" data-id="${esc(a.id)}" data-grp="${esc(fk)}"${extra}>${esc(label)}</button>`;
  return `<div class="ctxmenu att-menu" role="menu" aria-label="Actions for ${esc(a.title)}">
    ${attHref(a) ? `<a role="menuitem" href="${esc(attHref(a))}" target="_blank" rel="noopener" data-act="att-menu-close">Open</a>` : ''}
    ${item('att-copy', web ? 'Copy link' : 'Copy path', ` data-url="${esc(a.url)}"`)}${edit ? `${item('att-edit', 'Edit')}<hr>${item('att-remove', 'Remove')}` : ''}</div>`;
}
function attEditHtml(a) {
  const k = 'attedit:' + a.id;
  const kind = S.drafts[k + ':kind'] ?? a.kind ?? 'other';
  return `<li class="att att-editing"><div class="stack att-form" style="gap:8px;width:100%">
    <div class="seg sm" role="group" aria-label="Kind">${Object.entries(ATT_KIND).map(([id, l]) => `<button type="button" data-act="att-ekind" data-arg="${id}" data-id="${esc(a.id)}" aria-pressed="${kind === id}">${l}</button>`).join('')}</div>
    <label class="sr" for="${esc(k)}-t">Title</label><input class="input sm" id="${esc(k)}-t" data-k="${esc(k + ':title')}" placeholder="Title" value="${esc(S.drafts[k + ':title'] ?? a.title ?? '')}">
    <label class="sr" for="${esc(k)}-u">Link or file path</label><input class="input sm mono" id="${esc(k)}-u" data-k="${esc(k + ':url')}" placeholder="https://… or /path/to/file" value="${esc(S.drafts[k + ':url'] ?? a.url ?? '')}">
    <div class="row">${btn('Save', 'att-esave', { id: a.id, cls: 'soft sm', grp: k })}${btn('Cancel', 'att-ecancel', { id: a.id, cls: 'ghost sm' })}</div>${note(k)}</div></li>`;
}
const clearAttEdit = id => ['kind', 'title', 'url'].forEach(f => delete S.drafts[`attedit:${id}:${f}`]);
document.addEventListener('mousedown', e => { if (S.attMenu && !e.target.closest('.att-menu, [data-act="att-menu"]')) { S.attMenu = null; renderAll(); } }, true);
document.addEventListener('keydown', e => { if (S.attMenu && e.key === 'Escape') { S.attMenu = null; renderAll(); } });

function attachRow(t) {
  const r = ref(t, 'T');
  const a = attachList([...(t.attachments || []), ...(t.goal_attachments || [])], 'tasks', r);
  return `<div class="lsec"><div class="lsec-h">Attached${a.n ? `<span class="count">${a.n}</span>` : ''}</div>
    ${a.items ? `<ul class="atts">${a.items}</ul>` : '<span class="small">Nothing yet. Designs, proposals, docs and results go here.</span>'}
    ${note(a.fk)}</div>`;
}

function handoffText(t) {
  const r = ref(t, 'T');
  if (typeof t.handoff === 'string') return t.handoff;
  if (t.handoff && typeof t.handoff.text === 'string') return t.handoff.text;
  return S.handoffs[r] ?? null;
}
function metaRows(t) {
  const r = ref(t, 'T');
  if (S.metaEdit[r]) return S.metaEdit[r];
  const m = Array.isArray(t.meta) ? t.meta : [];
  return m.map(p => Array.isArray(p) ? [String(p[0] ?? ''), String(p[1] ?? '')] : [String((p && (p.name ?? p.k)) ?? ''), String((p && (p.value ?? p.v)) ?? '')]);
}

function originRow(t) {
  const o = t.origin;
  if (!o || !o.from) return '';
  const from = o.url ? `<a href="${esc(o.url)}" target="_blank" rel="noopener">${esc(o.from)}</a>` : esc(o.from);
  return `<div><span class="k">From</span><span class="v">${from}${o.by ? ` · ${esc(o.by)}` : ''}</span></div>`;
}

function contextTab(t) {
  const r = ref(t, 'T');
  const c = t.context || {};
  const w = c.where || {};
  const where = [
    ['Branch', w.branch || (t.started_at || t.who ? 'Not reported yet' : 'Not started')],
    ['Worktree', w.worktree || t.repo_path || t.project],
    w.last_commit && ['Last commit', w.last_commit],
    w.uncommitted != null && ['Not committed', uncommitted(w.uncommitted)],
    (w.conversation || t.claude_session_id) && ['Conversation', 'Can be reopened in a new terminal'],
  ].filter(Boolean);
  const listDefs = [
    ['Done so far', c.done, '✓', 'var(--up)'],
    ['Next', c.next, '→', 'var(--accent)'],
    ['Decisions', c.decisions, '•', 'var(--muted)'],
    ['Your answers', c.answers, '•', 'var(--warn)'],
  ];
  const lists = listDefs.filter(([, items]) => Array.isArray(items) && items.length).map(([title, items, g, color]) =>
    `<div class="stack"><h3 class="h3">${title}</h3><ul class="list">${items.map(x => `<li><span class="g" aria-hidden="true" style="color:${color}">${g}</span><span>${esc(x)}</span></li>`).join('')}</ul></div>`);
  if ((t.found || []).length) {
    lists.push(`<div class="stack"><h3 class="h3">Found, not fixed here</h3><ul class="list">${t.found.map(f =>
      `<li><span class="g" aria-hidden="true" style="color:var(--warn)">!</span><span><a href="#/?issue=${esc(ref(f, 'B'))}">${esc(f.title)}</a> <span class="muted">(in the ${t.goal ? 'goal’s ' : ''}backlog)</span></span></li>`).join('')}</ul></div>`);
  }
  const files = Array.isArray(c.files) ? c.files : [];
  const meta = metaRows(t);
  const text = handoffText(t);
  const handoff = `<div class="handoff">${esc(text ?? 'Loading…')}</div>`;
  return `
    ${savedLine(t) ? `<span class="muted" style="font-size:13px">${esc(savedLine(t))}</span>` : ''}
    <section class="kv" aria-label="Where it is">${where.map(([k, v]) => `<div><span class="k">${esc(k)}</span><span class="v">${esc(v)}</span></div>`).join('')}${originRow(t)}</section>
    ${lists.join('')}
    <div class="stack" style="gap:8px"><h3 class="h3">Files touched</h3>${files.length ? `<div class="files">${files.map(f => `<span title="${esc(f)}">${esc(String(f).split('/').pop())}</span>`).join('')}</div>` : '<span class="help">None yet.</span>'}</div>
    ${meta.length ? `<div class="stack" style="gap:8px"><h3 class="h3">Details</h3>
      ${meta.map(([k, v], i) => `<div class="meta-row"><input type="text" class="input k" aria-label="Field name" placeholder="Name" data-meta="${esc(r)}:${i}:0" data-change="meta" data-id="${esc(r)}" data-grp="meta:${esc(r)}" value="${esc(k)}"><input type="text" class="input" aria-label="Field value" placeholder="Value" data-meta="${esc(r)}:${i}:1" data-change="meta" data-id="${esc(r)}" data-grp="meta:${esc(r)}" value="${esc(v)}"><button type="button" class="icon-btn sm" data-act="meta-del" data-id="${esc(r)}" data-arg="${i}" aria-label="Remove this field">${ICON.x}</button></div>`).join('')}
      ${note('meta:' + r)}</div>` : ''}
    <div class="box info"><h3 class="h3">What a new terminal gets</h3>${handoff}</div>`;
}

function logTab(t) {
  const f = S.logFilter;
  const all = Array.isArray(t.log) ? t.log : [];
  const list = all.filter(e => f === 'all' || LOG_GROUP[e.kind] === f);
  const chips = LOG_FILTERS.map(([id, label]) => `<button type="button" class="fchip" data-act="log-filter" data-arg="${id}" aria-pressed="${f === id}">${label}</button>`).join('');
  const items = list.map(e => `<li><span class="time" title="${esc(fullTime(e.at))}">${esc(hhmm(e.at))}</span><span class="dot" style="background:${LOG_DOT[e.kind] || 'var(--faint)'}"></span>
    <span class="stack" style="gap:0"><span class="who"><b>${esc(e.who || 'Task board')}</b><span class="kind">${esc(LOG_LABEL[e.kind] || cap(e.kind))}</span></span><span class="txt">${esc(e.text)}</span></span></li>`).join('');
  return `<div class="fchips" role="group" aria-label="Filter the log">${chips}</div>
    ${list.length ? `<ol class="log">${items}</ol>` : `<p class="empty">${all.length ? 'Nothing of this kind yet.' : 'Nothing logged yet.'}</p>`}
    <p class="help">The log is saved as it happens, so it survives a closed terminal or a restart.</p>`;
}

const SNAP_ORDER = ['from', 'task', 'step', 'branch', 'worktree', 'last_commit', 'commit', 'uncommitted', 'file', 'last_turn', 'output'];
const SNAP_LABEL = { from: 'From', task: 'Task', step: 'Step', branch: 'Branch', worktree: 'Worktree', last_commit: 'Last commit', commit: 'Last commit', uncommitted: 'Not committed', file: 'File', last_turn: 'Last turn', output: 'Output', cwd: 'Folder' };
function snapRows(snap) {
  if (!snap) return [];
  if (typeof snap === 'string') { try { snap = JSON.parse(snap); } catch { return [['Snapshot', snap]]; } }
  let pairs = Array.isArray(snap) ? snap.filter(Array.isArray) : Object.entries(snap);
  if (!Array.isArray(snap)) pairs.sort((a, b) => { const i = SNAP_ORDER.indexOf(a[0]), j = SNAP_ORDER.indexOf(b[0]); return (i < 0 ? 99 : i) - (j < 0 ? 99 : j); });
  return pairs.filter(([, v]) => v != null && v !== '').map(([k, v]) => {
    let val = v;
    if (k === 'uncommitted') val = uncommitted(v);
    else if (v && typeof v === 'object') val = v.ref || v.title ? [v.ref, v.title].filter(Boolean).join(' · ') : JSON.stringify(v);
    return [SNAP_LABEL[k] || cap(String(k).replace(/_/g, ' ')), String(val)];
  });
}
function issueHow(b) {
  if (b.how) return b.how;
  const ft = foundTask(b);
  if (b.source === 'you') return 'You added it.';
  if (b.source === 'answer') return 'It came from one of your answers.';
  return `${b.found_by_name ? `The ${b.found_by_name} terminal` : 'A terminal'} reported it${ft ? ` while working on ${ft.ref}` : ''}.`;
}
function issueDetail(b, o = {}) {
  const r = ref(b, 'B');
  const snap = snapRows(b.snapshot);
  const ft = foundTask(b);
  const hist = (Array.isArray(b.history) ? b.history : []).slice().sort((x, y) => String(x.at).localeCompare(String(y.at)));
  return `<div class="issue-d">
    <div class="stack"><h3 class="h3">How it was reported</h3><span style="font-size:${o.compact ? 13 : 14}px">${esc(issueHow(b))}</span>${b.said ? `<p class="quote">${esc(b.said)}</p>` : ''}${b.detail ? `<p class="what" style="font-size:13px">${esc(b.detail)}</p>` : ''}</div>
    <div class="stack"><h3 class="h3">What was happening</h3>
      ${snap.length ? `<div class="kv${o.compact ? ' compact' : ''}">${snap.map(([k, v]) => `<div><span class="k">${esc(k)}</span><span class="v">${esc(v)}</span></div>`).join('')}</div>` : '<span class="help">No snapshot was saved with this issue.</span>'}
      ${ft ? `<a href="#/?task=${esc(ft.ref)}&tab=log" style="font-size:13px;align-self:flex-start">Open ${esc(ft.ref)}’s log${b.created_at ? ` at ${esc(hhmm(b.created_at))}` : ''}</a>` : ''}</div>
    <div class="stack" style="gap:8px"><h3 class="h3">History</h3>
      ${hist.length ? `<ol class="hist">${hist.map(h => `<li><span class="time" title="${esc(fullTime(h.at))}">${esc(hhmm(h.at))}</span><span class="dot" style="background:${HIST_DOT[h.kind] || 'var(--faint)'}"></span><span class="txt">${h.who ? `<b>${esc(h.who)}</b> · ` : ''}${esc(h.text)}</span></li>`).join('')}</ol>` : '<span class="help">Nothing yet.</span>'}
</div>
  </div>`;
}
function issueActions(b, where, o = {}) {
  const r = ref(b, 'B');
  const grp = 'issue:' + r;
  return `${btn('Make it a task', 'promote', { id: r, arg: where, cls: o.small ? 'soft sm' : 'primary', grp })}${jiraOn() ? btn('Create ticket', 'ticket', { id: r, cls: o.small ? 'sm' : '', grp }) : ''}${btn('Won’t do', 'drop', { id: r, cls: 'ghost' + (o.small ? ' sm' : ''), grp })}`;
}

function issuePanel() {
  const b = S.issue;
  const r = S.issueRef;
  if (!b) {
    return `<aside class="panel" aria-label="Backlog issue"><div class="panel-top"><span class="pill neutral">Backlog</span><div class="grow"></div>${closeBtn('close-panel')}</div>
      <p class="${S.issueErr ? 'note err' : 'muted'}">${esc(S.issueErr || 'Loading…')}</p></aside>`;
  }
  const info = issueState(b);
  const state = info ? info.text : `Open · reported at ${hhmm(b.created_at)}`;
  return `<aside class="panel" aria-label="Backlog issue" data-scroll="issue-${esc(r)}">
    <div class="panel-top"><span class="pill neutral">Backlog</span><span class="pill k-${esc(b.kind)}">${esc(kindLabel(b.kind))}</span><span class="pill ref">${esc(ref(b, 'B'))}</span><div class="grow"></div>${closeBtn('close-panel')}</div>
    ${S.issueErr ? `<p class="note err">${esc(S.issueErr)}</p>` : ''}
    <div class="stack" style="gap:4px"><h2 class="title">${esc(b.title)}</h2><div class="subline"><span class="mono">${esc(b.project)}</span><span>${esc(state)}${b.state === 'task' && b.task_id != null ? ` · <a href="#/?task=T${esc(b.task_id)}">Open it</a>` : ''}</span></div></div>
    ${(b.state || 'open') === 'open' ? `<div class="box info"><span style="font-size:13px">Not part of any task yet. Make it a task to queue it${jiraOn() ? ', or send it to Jira' : ''}.</span><div class="row">${issueActions(b, 'board')}</div>${note('issue:' + ref(b, 'B'))}</div>` : note('issue:' + ref(b, 'B'))}
    ${issueDetail(b)}
    <a href="#/backlog?issue=${esc(ref(b, 'B'))}" style="font-size:13px;align-self:flex-start">See every backlog issue</a>
  </aside>`;
}

const anySession = id => ((S.state && S.state.sessions) || []).find(x => x.id === id)
  || (typeof ssSession === 'function' ? ssSession(id) : null)
  || (typeof SS !== 'undefined' && SS.detail && SS.detail.id === id ? SS.detail : null);
function startRename(id) {
  const sess = anySession(id);
  if (!sess) return;
  S.rename = id;
  S.drafts['rename:' + id] = sess.renaming || sess.name || '';
  renderAll();
  const n = document.getElementById('rename-' + id);
  if (n) { n.focus(); n.select(); }
}
function endRename(id, save) {
  if (S.rename !== id) return;
  const k = 'rename:' + id;
  const name = (S.drafts[k] || '').trim();
  const sess = anySession(id);
  S.rename = null;
  delete S.drafts[k];
  if (!save || !name || !sess || name === (sess.renaming || sess.name)) { renderAll(); return; }
  sessionCopies(id).forEach(x => { x.renaming = name; });
  S.renameWatch[id] = name;
  renderAll();
  post(`/sessions/${encodeURIComponent(id)}/rename`, { name })
    .then(() => refresh())
    .catch(e => { sessionCopies(id).forEach(x => { x.renaming = null; x.rename_error = e.message; }); renderAll(); });
}
function sessionCopies(id) {
  return [((S.state && S.state.sessions) || []).find(x => x.id === id),
    typeof ssSession === 'function' ? ssSession(id) : null,
    typeof SS !== 'undefined' && SS.detail && SS.detail.id === id ? SS.detail : null].filter(Boolean);
}
const FLASH_MS = { ok: 1200, bad: 1800 };
function renameFlash(s) {
  const to = S.renameWatch[s.id];
  if (to != null && !s.renaming) {
    delete S.renameWatch[s.id];
    const ok = !s.rename_error && s.name === to;
    S.renameFlash[s.id] = { ok, to, why: s.rename_error || 'Midna kept the old name', at: Date.now() };
    setTimeout(() => { delete S.renameFlash[s.id]; renderAll(); }, ok ? FLASH_MS.ok : FLASH_MS.bad);
  }
  const f = S.renameFlash[s.id];
  return f && { ...f, ago: Date.now() - f.at };
}
function nameView(s, hint = ' · double-click to rename') {
  if (s.renaming) return { text: s.renaming, cls: ' renaming', attrs: ' title="Renaming in Midna…" aria-busy="true"' };
  const fl = renameFlash(s);
  if (fl && !fl.ok) return { text: fl.to, cls: ' rename-bad', attrs: ` style="animation-delay:-${fl.ago}ms" title="Rename failed: ${esc(fl.why)}" role="status"` };
  const title = s.rename_error && !fl ? `Last rename failed: ${s.rename_error}` : s.name;
  return { text: s.name || s.id, cls: fl ? ' rename-ok' : '', attrs: `${fl ? ` style="animation-delay:-${fl.ago}ms"` : ''} title="${esc(title + hint)}"` };
}

function connPill(m) {
  const heard = m.seen_at ? `Last heard from ${ago(m.seen_at)}` : 'Not heard from yet';
  const [cls, label, why] = m.up ? ['up', 'Connected to Midna', heard]
    : ['down', 'Midna isn’t running', `${heard}. Starting, messaging and closing terminals wait until it’s back.`];
  return `<span class="conn ${cls}" id="conn-pill" role="status" title="${esc(why)}"><span class="dot"></span>${esc(label)}</span>`;
}
function renderMenus() {
  patch($('#menu-root'), hoursMenuHtml());
}

const HOUR_DAYS = [['sun', 'S'], ['mon', 'M'], ['tue', 'T'], ['wed', 'W'], ['thu', 'T'], ['fri', 'F'], ['sat', 'S']];
function clock12(hhmm) {
  const [h, m] = String(hhmm || '0:0').split(':').map(Number);
  return `${h % 12 || 12}${m ? ':' + String(m).padStart(2, '0') : ''}${h < 12 ? 'am' : 'pm'}`;
}
function hoursPill() {
  const h = S.state && S.state.work_hours;
  if (!h) return '';
  const nxt = h.next_open ? new Date(h.next_open) : null;
  const when = nxt ? (nxt.toDateString() === new Date().toDateString() ? '' : nxt.toLocaleDateString(undefined, { weekday: 'short' }) + ' ')
    + clock12(nxt.toTimeString().slice(0, 5)) : '';
  const [cls, label] = !h.on ? ['hours', 'No work hours'] : h.open && h.today_until ? ['hours', `Work hours until ${clock12(h.today_until)} today`]
    : h.open ? ['hours', `Work hours ${clock12(h.start)}–${clock12(h.end)}`]
    : ['hours closed', `Agents off until ${when}`];
  return `<button type="button" class="conn ${cls}" id="hours-pill" data-act="hours-menu" aria-haspopup="dialog" aria-expanded="${!!S.hoursMenu}" title="${esc(h.line)}"><span class="dot"></span>${esc(label)}${ICON.chev}</button>`;
}
const USAGE_NAMES = { five_hour: '5-hour', seven_day: '7-day' };
function usageLevel(pct) { return pct >= 90 ? 'hot' : pct >= 75 ? 'warm' : 'ok'; }
function resetWhen(iso) {
  const d = new Date(iso);
  const day = d.toDateString() === new Date().toDateString() ? '' : d.toLocaleDateString(undefined, { weekday: 'short' }) + ' ';
  return day + clock12(d.toTimeString().slice(0, 5));
}
function usagePill() {
  const u = S.state && S.state.usage;
  if (!u || !u.windows.length) return '';
  const stale = Date.now() - new Date(u.seen_at).getTime() > 3600e3;
  const title = u.windows.map(w => `${USAGE_NAMES[w.key] || w.label} limit: ${w.pct}% used${w.resets_at ? `, resets ${resetWhen(w.resets_at)}` : ''}`)
    .concat(stale ? [`Last read ${ago(u.seen_at)}`] : []).join('\n');
  return `<span class="conn usage${stale ? ' stale' : ''}" role="status" aria-label="Claude usage" title="${esc(title)}">${u.windows.map(w =>
    `<span class="u-win ${usageLevel(w.pct)}"><span class="u-k">${esc(w.label)}</span><span class="u-bar"><i style="width:${Math.min(100, Math.max(0, w.pct))}%"></i></span><span class="u-v">${w.pct}%</span></span>`).join('')}</span>`;
}
function hourOptions(cur) {
  const times = [];
  for (let m = 0; m < 24 * 60; m += 30) times.push(`${String(Math.floor(m / 60)).padStart(2, '0')}:${String(m % 60).padStart(2, '0')}`);
  if (cur && !times.includes(cur)) { times.push(cur); times.sort(); }
  return times.map(t => `<option value="${t}"${t === cur ? ' selected' : ''}>${clock12(t)}</option>`).join('');
}
function hoursMenuHtml() {
  const m = S.hoursMenu;
  const h = S.state && S.state.work_hours;
  if (!m || !h) return '';
  const x = Math.min(m.x, window.innerWidth - 300);
  const off = m.on ? '' : ' disabled';
  return `<div class="ctxmenu hours-menu" role="dialog" aria-label="Work hours" style="left:${Math.max(8, x)}px;bottom:${window.innerHeight - m.y}px">
    <label class="hours-row"><input type="checkbox" id="hours-on" data-hours="on"${m.on ? ' checked' : ''}> Only start agents during work hours</label>
    <div class="hours-row"><label for="hours-start">From</label><select class="select" id="hours-start" data-hours="start"${off}>${hourOptions(m.start)}</select>
      <label for="hours-end">to</label><select class="select" id="hours-end" data-hours="end"${off}>${hourOptions(m.end)}</select></div>
    <div class="seg sm hours-days" role="group" aria-label="Days">${HOUR_DAYS.map(([d, l]) =>
      `<button type="button" data-act="hours-day" data-arg="${d}" aria-pressed="${m.days.includes(d)}" title="${cap(d)}"${off}>${l}</button>`).join('')}</div>
    <div class="hours-row"><label for="hours-today">Today until</label><select class="select" id="hours-today" data-hours="today"${off}>
      <option value=""${m.today ? '' : ' selected'}>Usual end</option>${hourOptions(m.today)}</select></div>
    ${m.err ? `<div class="note err" role="alert">${esc(m.err)}</div>` : ''}</div>`;
}
function openHoursMenu(el) {
  const h = S.state.work_hours;
  const r = el.getBoundingClientRect();
  S.hoursMenu = { x: r.left, y: r.top - 6, on: h.on, start: h.start, end: h.end, days: h.days.slice(), today: h.today_until || '' };
  renderAll();
  const first = document.querySelector('.hours-menu input');
  if (first) first.focus();
}
function closeHoursMenu() { if (S.hoursMenu) { S.hoursMenu = null; renderAll(); const p = $('#hours-pill'); if (p) p.focus(); } }
let hoursSaving = Promise.resolve();
function saveHours() {
  const m = S.hoursMenu;
  if (!m) return;
  const body = { on: m.on, start: m.start, end: m.end, days: m.days };
  if (m.today !== ((S.state.work_hours || {}).today_until || '')) body.today_until = m.today || 'off';
  hoursSaving = hoursSaving.then(async () => {
    try {
      const h = await post('/hours', body);
      if (S.state) S.state.work_hours = h;
      if (S.hoursMenu) S.hoursMenu.err = null;
    } catch (e) { if (S.hoursMenu) S.hoursMenu.err = e.message; }
    renderAll();
  });
}
document.addEventListener('input', e => {
  const k = e.target.dataset && e.target.dataset.hours;
  if (!k || !S.hoursMenu) return;
  S.hoursMenu[k] = k === 'on' ? e.target.checked : e.target.value;
  renderAll();
  saveHours();
});
document.addEventListener('mousedown', e => { if (S.hoursMenu && !e.target.closest('.hours-menu, #hours-pill')) closeHoursMenu(); }, true);
document.addEventListener('mousedown', e => { if (e.target.closest('[data-act="meta-del"]')) e.preventDefault(); });

const PICKERS = {
  project: { title: 'Choose a project', search: 'Search projects', none: 'No project matches' },
  goal: { title: 'Choose a goal', search: 'Search goals by name, project or epic', none: 'No goal matches' },
};
function pickerButton(kind, id, value, o = {}) {
  const specials = o.specials || [];
  const sp = specials.find(([v]) => v === value);
  const g = kind === 'goal' && value ? goalByRef(value) : null;
  const label = sp ? sp[1] : kind === 'goal' ? (g ? g.name : o.placeholder || 'Choose a goal') : (value || o.placeholder || 'Choose a project');
  const empty = !sp && !(kind === 'goal' ? g : value);
  const attrs = [
    `id="${esc(id)}"`, 'data-act="open-picker"', `data-kind="${kind}"`, `data-arg="${esc(o.target)}"`, `data-value="${esc(value || '')}"`,
    specials.length ? `data-specials="${esc(JSON.stringify(specials))}"` : '',
    o.project ? `data-proj="${esc(o.project)}"` : '', o.id ? `data-id="${esc(o.id)}"` : '', o.grp ? `data-grp="${esc(o.grp)}"` : '',
    o.style ? `style="${esc(o.style)}"` : '', o.disabled ? 'disabled' : '', 'aria-haspopup="dialog"',
    `title="${esc(label)} · click to search"`,
  ].filter(Boolean).join(' ');
  return `<button type="button" class="picker-btn${o.cls ? ' ' + o.cls : ''}${empty ? ' empty' : ''}" ${attrs}>${ICON.search}<span class="picker-val">${esc(label)}</span></button>`;
}
const projectButton = (id, value, o = {}) => pickerButton('project', id, value,
  { ...o, specials: o.all ? [['all', 'All projects']] : o.specials });
const goalButton = (id, value, o = {}) => pickerButton('goal', id, value, o);

function projectPaths() {
  const out = {};
  ((S.state && S.state.projects) || []).forEach(p => { if (p && p.name && p.path) out[p.name] = p.path; });
  return out;
}
function openPicker(el) {
  const d = el.dataset;
  let specials = [];
  try { specials = d.specials ? JSON.parse(d.specials) : []; } catch {}
  S.picker = { kind: d.kind || 'project', target: d.arg, value: d.value, specials, proj: d.proj || '', id: d.id, grp: d.grp, q: '', active: 0, back: el.id };
  const cur = pickerItems().findIndex(it => it.v === S.picker.value);
  S.picker.active = Math.max(0, cur);
  const P = PICKERS[S.picker.kind];
  $('#picker-root').innerHTML = `<div class="overlay picker-bg" data-act="picker-bg"><div class="modal picker" role="dialog" aria-modal="true" aria-label="${esc(P.title)}">
    <div class="picker-search">${ICON.search}<input id="picker-q" class="picker-q" type="search" placeholder="${esc(P.search)}" autocomplete="off" spellcheck="false" aria-controls="picker-list" aria-label="${esc(P.search)}"></div>
    <ul class="picker-list" id="picker-list" role="listbox"></ul>
    <div class="picker-foot"><span><kbd>↑</kbd><kbd>↓</kbd> to move</span><span><kbd>Enter</kbd> to choose</span><span><kbd>Esc</kbd> to close</span></div></div></div>`;
  renderPickerList(true);
  const q = $('#picker-q');
  q.addEventListener('input', () => { S.picker.q = q.value; S.picker.active = 0; renderPickerList(true); });
  q.focus();
}
function pickerBase(p) {
  if (p.kind === 'goal') {
    const goals = allGoals().filter(g => !p.proj || p.proj === 'all' || g.project === p.proj);
    return goals.map(g => ({ v: ref(g, 'G'), name: g.name, sub: [g.project, g.epic_key].filter(Boolean).join(' · ') }))
      .sort((a, b) => a.name.localeCompare(b.name));
  }
  const names = projectNames();
  if (p.value && !p.specials.some(([v]) => v === p.value) && !names.includes(p.value)) names.push(p.value);
  const paths = projectPaths();
  return names.map(n => ({ v: n, name: n, sub: paths[n] ? paths[n].replace(/^\/Users\/[^/]+/, '~') : '' }));
}
function pickerItems() {
  const p = S.picker;
  const q = (p.q || '').trim().toLowerCase();
  let items = pickerBase(p);
  if (q) {
    const rank = it => { const l = it.name.toLowerCase(); return l.startsWith(q) ? 0 : l.split(/[^a-z0-9]+/).some(w => w.startsWith(q)) ? 1 : l.includes(q) ? 2 : 3; };
    items = items.filter(it => it.name.toLowerCase().includes(q) || (p.kind !== 'project' && it.sub.toLowerCase().includes(q)))
      .sort((a, b) => rank(a) - rank(b) || a.name.localeCompare(b.name));
  }
  const out = q ? items : [...p.specials.map(([v, name]) => ({ v, name, special: true })), ...items];
  return out;
}
function renderPickerList(scroll) {
  const p = S.picker;
  const list = $('#picker-list');
  if (!p || !list) return;
  const items = pickerItems();
  p.active = Math.min(p.active, Math.max(0, items.length - 1));
  list.innerHTML = items.length ? items.map((it, i) => `<li role="option" id="pk-${i}" class="picker-item${i === p.active ? ' active' : ''}${it.special ? ' special' : ''}${it.create ? ' create' : ''}" aria-selected="${i === p.active}" data-act="picker-pick" data-arg="${esc(it.v)}">
      ${it.create ? ICON.plus : ''}<span class="picker-name">${esc(it.name)}</span>${it.sub ? `<span class="picker-path">${esc(it.sub)}</span>` : ''}${it.v === p.value ? '<span class="picker-cur">Current</span>' : ''}</li>`).join('')
    : `<li class="picker-none">${p.q.trim() || !PICKERS[p.kind].empty ? `${esc(PICKERS[p.kind].none)} “${esc(p.q.trim())}”.` : esc(PICKERS[p.kind].empty)}</li>`;
  $('#picker-q').setAttribute('aria-activedescendant', items.length ? 'pk-' + p.active : '');
  if (scroll) { const a = list.querySelector('.active'); if (a) a.scrollIntoView({ block: 'nearest' }); }
}
function closePicker() {
  const back = S.picker && S.picker.back;
  S.picker = null;
  $('#picker-root').innerHTML = '';
  const b = back && document.getElementById(back);
  if (b) b.focus();
}
function pickItem(v) {
  const p = S.picker;
  if (!p) return;
  closePicker();
  if (v === p.value) return;
  applyPick(p, v);
}
function applyPick(p, v) {
  const [kind, name] = p.target.split(':');
  const el = { value: v, tagName: 'SELECT', type: 'select-one', checked: false,
    dataset: { field: p.kind, m: name, id: p.id, grp: p.grp, change: name } };
  if (kind === 'modal' && S.modal) setModalField(el, true);
  else if (kind === 'change' && CHANGES[name]) CHANGES[name](el);
}
function pickerKey(e) {
  const p = S.picker;
  if (e.key === 'Escape') { e.preventDefault(); closePicker(); return; }
  const n = pickerItems().length;
  if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
    e.preventDefault();
    if (n) { p.active = (p.active + (e.key === 'ArrowDown' ? 1 : n - 1)) % n; renderPickerList(true); }
  } else if (e.key === 'Enter') {
    e.preventDefault();
    const it = pickerItems()[p.active];
    if (it) pickItem(it.v);
  } else if (e.key === 'Tab') {
    e.preventDefault();
  }
}

const DRAFT_FIELDS = {
  goal: ['name', 'tldr', 'outcome', 'project', 'epicMode', 'epicKey', 'run_in_order', 'max_terminals', 'auto_close'],
  task: ['title', 'detail', 'project', 'goal', 'priority', 'pickup', 'session', 'jiraMode', 'jiraKey', 'auto_close'],
};
const draftKind = m => m && (m.kind === 'goal' || m.kind === 'task') && !m.id ? m.kind : null;
const hasWords = d => !!d && ['title', 'detail', 'name', 'tldr', 'outcome'].some(k => String(d[k] || '').trim());
function saveDraft() {
  const m = S.modal, k = draftKind(m);
  if (!k || m.busy) return;
  const d = {};
  for (const f of DRAFT_FIELDS[k]) d[f] = m[f];
  try { if (hasWords(d)) localStorage.setItem('tb.draft.' + k, JSON.stringify(d)); else localStorage.removeItem('tb.draft.' + k); } catch {}
}
function loadDraft(k) {
  try { const d = JSON.parse(localStorage.getItem('tb.draft.' + k) || 'null'); return hasWords(d) ? d : null; } catch { return null; }
}
function clearDraft(k) { try { localStorage.removeItem('tb.draft.' + k); } catch {} }
function restoreDraft(m) {
  const k = draftKind(m), d = k && loadDraft(k);
  if (!d) return m;
  Object.assign(m, d, { restored: true, prefill: '' });
  return m;
}
const draftNote = m => m.restored ? `<p class="prefill draft-note">Picked up the ${m.kind} you hadn’t added yet. <button type="button" class="btn link" data-act="draft-discard">Discard it and start over</button></p>` : '';

const seg = (field, opts, cur) => opts.map(([id, label]) =>
  `<button type="button" data-act="m-set" data-field="${field}" data-arg="${id}" aria-pressed="${cur === id}">${esc(label)}</button>`).join('');
const MODALS = { alerts: alertsDialogHtml };
const SUBMITS = {};
function renderModal() {
  const m = S.modal;
  saveDraft();
  patch($('#modal-root'), m && MODALS[m.kind] ? MODALS[m.kind](m) : '');
}
function closeModal() { S.modal = (S.modal && S.modal.back) || null; renderModal(); }
function setModalField(el, final) {
  const m = S.modal;
  const f = el.dataset.m;
  if (el.type === 'checkbox') m[f] = el.checked;
  else if (el.type === 'radio') { if (el.checked) m[f] = el.value; }
  else m[f] = el.value;
  if (!final) return;
  if (el.tagName === 'SELECT' || el.type === 'radio' || el.dataset.rerender) renderModal();
}

const tp = (el, suffix = '') => `/tasks/${encodeURIComponent(el.dataset.id)}${suffix}`;
const ip = (el, suffix = '') => `/backlog/${encodeURIComponent(el.dataset.id)}${suffix}`;
function keepIssue(r) { if (S.route.page === 'board' && !S.keepIssues.includes(r)) S.keepIssues.push(r); if (typeof keepBacklogRow === 'function') keepBacklogRow(r); }

const ACTIONS = {
  'goal-pick': el => {
    const v = el.dataset.arg;
    S.filter.goal = v !== 'all' && sameRef(v, S.filter.goal, 'G') ? 'all' : v;
    const g = S.filter.goal !== 'all' && goalByRef(S.filter.goal);
    if (g && g.project) S.filter.project = g.project;
    if (S.filter.goal === 'all') S.filter.project = 'all';
    S.keepIssues = []; saveFilter(); renderAll(); refresh();
  },
  'rail-project': el => {
    const p = el.dataset.arg;
    if (S.filter.goal !== 'all') { S.filter.goal = 'all'; S.filter.project = p; }
    else S.filter.project = S.filter.project === p ? 'all' : p;
    S.keepIssues = []; saveFilter(); renderAll(); refresh();
  },
  'rail-fold': el => {
    const set = railShut(), p = el.dataset.arg;
    if (set.has(p)) set.delete(p); else set.add(p);
    setRailShut(set); renderAll();
  },
  'rail-recent': () => { try { localStorage.setItem('tb.rail.recent', railRecentOpen() ? 'closed' : 'open'); } catch {} renderAll(); },
  'open-task': el => openTask(el.dataset.id, null, !!el.closest('.panel')),
  'task-back': () => { const tr = taskTrail(); nav(hashWith({ task: tr.pop(), tab: null, from: tr.join(',') || null })); },
  'alert-dismiss': el => run(el, () => post(`/alerts/${encodeURIComponent(el.dataset.id)}/dismiss`, {}), { ok: '' }),
  'alerts-open': () => {
    S.modal = { kind: 'alerts' };
    renderModal();
    setTimeout(() => { const b = $('#modal-root [data-act="modal-close"]'); if (b) b.focus(); }, 0);
  },
  'alert-open': el => {
    const a = currentAlerts().find(x => String(x.id) === el.dataset.id);
    if (!a) return;
    if (S.modal && S.modal.kind === 'alerts') closeModal();
    if (a.task) openTask(a.task); else location.hash = `#/goals/${a.goal}`;
  },
  'alerts-dismiss-all': el => run(el, () => Promise.all(currentAlerts().filter(a => !alertStays(a)).map(a => post(`/alerts/${encodeURIComponent(a.id)}/dismiss`, {}))), { ok: '' }),
  'open-session': el => nav(routeHash('sessions', null, { s: el.dataset.id })),
  'close-modal': () => { if (!(S.modal && S.modal.busy)) closeModal(); },
  'open-issue': el => nav(hashWith({ issue: el.dataset.id, task: null, tab: null })),
  'close-panel': () => nav(hashWith(S.route.page === 'board' ? { task: null, issue: null, tab: null, from: null } : { task: null, tab: null, from: null })),
  'tab': el => nav(hashWith({ tab: el.dataset.arg === 'overview' ? null : el.dataset.arg }), { replace: true }),
  'log-filter': el => { S.logFilter = el.dataset.arg; renderAll(); },
  'reveal': el => {
    const key = el.dataset.arg;
    S.reveal[key] = true;
    renderAll();
    const [kind, ...rest] = key.split(':');
    const field = { done: 'summary', fail: 'reason', inote: 'inote' }[kind];
    const n = field && document.querySelector(`[data-k="${CSS.escape(field + ':' + rest.join(':'))}"]`);
    if (n) n.focus();
  },
  'hide': el => { delete S.reveal[el.dataset.arg]; renderAll(); },
  'close-done': el => { S.doneMenu = false; run(el, () => post('/done/close-terminals', {})); },
  'done-menu': () => { S.doneMenu = !S.doneMenu; renderAll(); },
  'done-window': el => { S.doneMenu = false; S.filter.done = el.dataset.arg; saveFilter(); renderAll(); refresh(); },
  'start': el => {
    const body = { mode: el.dataset.arg };
    run(el, () => post(tp(el, '/start'), body));
  },
  'queue-planned': el => run(el, () => post(tp(el), { status: 'queued' }), { ok: 'Queued', toast: true }),
  'answer': el => {
    const k = 'answer:' + el.dataset.id;
    const text = (S.drafts[k] || '').trim();
    if (!text) { setNote(el.dataset.grp, 'Write an answer first.', true); return renderAll(); }
    run(el, () => post(tp(el, '/answer'), { text, when: el.dataset.arg === 'morning' ? 'morning' : 'now' }), { ok: () => { delete S.drafts[k]; return el.dataset.arg === 'morning' ? `Saved. It sends at ${hoursOpenAt() || 'the start of work hours'}.` : sentNote(); } });
  },
  'requeue': el => run(el, () => post(tp(el, '/requeue'), {}), { ok: 'Back in the queue' }),
  'detach': el => run(el, () => post(tp(el, '/detach'), {}), { ok: 'Detached. It’s back in the queue.' }),
  'mark-done': el => {
    const r = el.dataset.id, k = 'summary:' + r;
    const summary = (S.drafts[k] || '').trim();
    run(el, () => post(tp(el, '/done'), summary ? { summary } : {}), { ok: () => { delete S.drafts[k]; delete S.reveal['done:' + r]; return 'Marked done'; } });
  },
  'mark-fail': el => {
    const r = el.dataset.id, k = 'reason:' + r;
    const reason = (S.drafts[k] || '').trim();
    run(el, () => post(tp(el, '/fail'), reason ? { reason } : {}), { ok: () => { delete S.drafts[k]; delete S.reveal['fail:' + r]; return 'Marked failed'; } });
  },
  'close-term': el => run(el, () => post(tp(el, '/close-terminal'), { force: el.dataset.arg === 'force' })),
  'focus': el => run(el, () => post(tp(el, '/focus'), {})),
  'term-focus': el => run(el, () => post(`/sessions/${encodeURIComponent(el.dataset.id)}/focus`)),
  'resume': el => run(el, () => post(tp(el, '/resume'), { mode: el.dataset.arg })),
  'meta-del': el => {
    const r = el.dataset.id, i = Number(el.dataset.arg);
    if (!S.task) return;
    S.metaEdit[r] = metaRows(S.task).map(p => p.slice());
    S.metaEdit[r].splice(i, 1);
    if (i >= (Array.isArray(S.task.meta) ? S.task.meta.length : 0)) { delete S.notes['meta:' + r]; return renderAll(); }
    saveMeta(el, r, 'Removed');
  },
  'promote': el => {
    const r = el.dataset.id;
    keepIssue(r);
    run(el, () => post(ip(el, '/promote'), { where: el.dataset.arg }), {
      ok: res => {
        const t = res && (res.task || (res.ref && /^T/.test(res.ref) ? res : null));
        const tr = t ? ref(t, 'T') : (res && res.task_id != null ? 'T' + res.task_id : '');
        if (S.route.page === 'board' && tr) { setTimeout(() => openTask(tr), 0); return ''; }
        return S.route.page === 'board' ? 'Made into a task' : '';
      },
    });
  },
  'ticket': el => { keepIssue(el.dataset.id); run(el, () => post(ip(el, '/ticket'), {}), { ok: 'Asked Jira for a ticket' }); },
  'drop': el => { keepIssue(el.dataset.id); run(el, () => post(ip(el, '/drop'), {}), { ok: 'Closed as won’t do' }); },
  'issue-note': el => {
    const k = 'inote:' + el.dataset.id;
    const text = (S.drafts[k] || '').trim();
    if (!text) { setNote(k, 'Write the note first.', true); return renderAll(); }
    run(el, () => post(ip(el, '/note'), { text }), { ok: () => { delete S.drafts[k]; delete S.reveal[k]; return 'Note added'; } });
  },
  'modal-close': () => closeModal(),
  'new-task': () => { if (typeof openTaskForm === 'function') openTaskForm({}); },
  'new-goal': () => { if (typeof openGoalForm === 'function') openGoalForm(null); },
  'open-picker': el => openPicker(el),
  'hours-menu': el => { if (S.hoursMenu) closeHoursMenu(); else openHoursMenu(el); },
  'hours-day': el => { const m = S.hoursMenu, d = el.dataset.arg; m.days = m.days.includes(d) ? m.days.filter(x => x !== d) : [...m.days, d]; renderAll(); saveHours(); },
  'picker-bg': (el, e) => { if (e.target === el) closePicker(); },
  'picker-pick': el => pickItem(el.dataset.arg),
  'modal-bg': () => {},
  'draft-discard': () => {
    const m = S.modal, k = draftKind(m);
    if (!k) return;
    clearDraft(k);
    closeModal();
    if (k === 'task' && typeof openTaskForm === 'function') openTaskForm({});
    else if (typeof openGoalForm === 'function') openGoalForm(null);
  },
  'm-set': el => {
    const m = S.modal;
    if (!m) return;
    m[el.dataset.field] = el.dataset.arg;
    m.err = '';
    renderModal();
  },
};

function saveMeta(el, r, msg = 'Saved') {
  const rows = (S.metaEdit[r] || []).map(([k, v]) => [String(k).trim(), String(v).trim()]).filter(([k, v]) => k || v);
  run(el, () => post(`/tasks/${encodeURIComponent(r)}/meta`, { meta: rows }), {
    grp: 'meta:' + r,
    ok: () => {
      const a = document.activeElement;
      if (!(a && a.dataset && a.dataset.meta && a.dataset.meta.startsWith(r + ':'))) delete S.metaEdit[r];
      return msg;
    },
  });
}

const CHANGES = {
  'filter-project': el => {
    S.filter.project = el.value;
    if (S.filter.goal !== 'all') { const g = goalByRef(S.filter.goal); if (!g || (el.value !== 'all' && g.project !== el.value)) S.filter.goal = 'all'; }
    S.keepIssues = [];
    saveFilter(); renderAll(); refresh();
  },
  'filter-goal': el => { S.filter.goal = el.value; S.keepIssues = []; saveFilter(); renderAll(); refresh(); },
  'meta': el => saveMeta(el, el.dataset.id),
  'issue-move': el => {
    keepIssue(el.dataset.id);
    run(el, () => post(ip(el, '/move'), { goal_id: el.value === 'none' ? null : idOf(el.value) }), { ok: 'Moved' });
  },
};

let nameClick = null;
const DOUBLE_CLICK_WAIT_MS = 260;
const READ_ONLY_MODALS = ['gnote', 'alerts'];
document.addEventListener('dblclick', e => {
  const n = e.target.closest('[data-rename-id]');
  if (!n) return;
  e.preventDefault();
  clearTimeout(nameClick); nameClick = null;
  startRename(n.dataset.renameId);
});
document.addEventListener('focusout', e => { const id = e.target.dataset && e.target.dataset.rename; if (id && !patching) endRename(id, true); });
document.addEventListener('click', e => {
  if (e.target.closest('[data-rename-id]') && e.detail === 1 && e.target.closest('[data-act]')) {
    const el = e.target.closest('[data-act]');
    clearTimeout(nameClick);
    nameClick = setTimeout(() => { nameClick = null; const fn = ACTIONS[el.dataset.act]; if (fn) fn(el, e); }, DOUBLE_CLICK_WAIT_MS);
    return;
  }
  if (e.target.closest('[data-rename-id]') && e.detail > 1) return;
  const el = e.target.closest('[data-act]');
  if (!el || el.disabled) return;
  const buttonInFoldHeading = el.tagName !== 'SUMMARY' && el.closest('summary');
  if (buttonInFoldHeading) e.preventDefault();
  const field = e.target.closest('select, input, textarea, label');
  const typingInsideClickableCard = field && field !== el && el.contains(field);
  if (typingInsideClickableCard) return;
  const fn = ACTIONS[el.dataset.act];
  if (fn) fn(el, e);
});
document.addEventListener('input', e => {
  const el = e.target;
  if (el.dataset.k) S.drafts[el.dataset.k] = el.value;
  if (el.dataset.m && S.modal && el.type !== 'radio' && el.type !== 'checkbox' && el.tagName !== 'SELECT') { S.modal[el.dataset.m] = el.value; saveDraft(); }
  if (el.dataset.meta && S.task) {
    const [r, i, j] = el.dataset.meta.split(':');
    if (!S.metaEdit[r]) S.metaEdit[r] = metaRows(S.task).map(p => p.slice());
    if (S.metaEdit[r][i]) S.metaEdit[r][i][j] = el.value;
  }
});
document.addEventListener('change', e => {
  const el = e.target;
  if (el.dataset.k) S.drafts[el.dataset.k] = el.value;
  if (el.dataset.m && S.modal) setModalField(el, true);
  const fn = el.dataset.change && CHANGES[el.dataset.change];
  if (fn) fn(el, e);
});
document.addEventListener('submit', e => {
  const f = e.target.closest('form[data-form]');
  if (!f) return;
  e.preventDefault();
  const fn = S.modal && !S.modal.busy && SUBMITS[f.dataset.form];
  if (fn) fn();
});
document.addEventListener('keydown', e => {
  const t = e.target;
  if (S.picker) { pickerKey(e); return; }
  if (S.hoursMenu && e.key === 'Escape') { e.preventDefault(); closeHoursMenu(); return; }
  if (t.dataset && t.dataset.rename) {
    if (e.key === 'Enter') { e.preventDefault(); endRename(t.dataset.rename, true); }
    else if (e.key === 'Escape') { e.preventDefault(); endRename(t.dataset.rename, false); }
    return;
  }
  if (e.key === 'Escape') {
    if (S.modal) { if (READ_ONLY_MODALS.includes(S.modal.kind)) { e.preventDefault(); closeModal(); } return; }
    if (S.route.q.task || (S.route.page === 'board' && S.route.q.issue)) { ACTIONS['close-panel'](); return; }
    if (S.route.page === 'board' && !S.doneMenu && !S.attMenu && !(t.closest && t.closest('input, textarea, select'))
        && (S.filter.goal !== 'all' || S.filter.project !== 'all')) { e.preventDefault(); railClear(); }
    return;
  }
  if ((e.key === 'Enter' || e.key === ' ') && t.matches && t.matches('[role="button"][data-act]')) { e.preventDefault(); t.click(); return; }
  if (e.key === 'Enter' && t.dataset && t.dataset.enter) {
    e.preventDefault();
    const b = t.parentElement.querySelector(`[data-act="${CSS.escape(t.dataset.enter)}"]`);
    if (b) b.click();
    return;
  }
  if (e.key === 'Enter' && (e.metaKey || e.ctrlKey) && t.dataset && t.dataset.k && t.dataset.k.startsWith('answer:')) {
    const b = t.closest('.box') && t.closest('.box').querySelector('[data-act="answer"]');
    if (b) b.click();
    return;
  }
});

function boot() {
  loadFilter();
  lastHash = location.hash;
  S.route = parseRoute();
  S.taskRef = S.route.q.task ? ref(S.route.q.task, 'T') : null;
  S.issueRef = currentIssueRef();
  window.addEventListener('hashchange', onRoute);
  window.addEventListener('popstate', onRoute);
  document.addEventListener('visibilitychange', () => { if (!document.hidden) refresh(); });
  renderAll();
  tick();
}
window.addEventListener('DOMContentLoaded', boot);
