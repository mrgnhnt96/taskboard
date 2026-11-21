// ── App state, shell, overlays and actions ──
let C = null;
const S = {
  screen: 'needs', depth: 0, page: 'needs', panel: null, lastPanel: 'T3', panelTab: 'story',
  goal: 'G2', goalTab: 'waves', idea: 'I2', proj: 'api', relProj: null, dayView: 'day', setSec: 'accounts', blTab: 'open',
  overlay: null, menu: null, dlg: null, side: true, insW: null,
  open: { T3: true, 'b-G3': true, 'g-G2': true, later: false, earlier: false, closedIdeas: false, H1: false },
  picked: new Set(), lens: 'similar', kind: 'all', folded: new Set(), gone: new Set(), snoozed: {},
  pick: {}, drafts: {}, undo: [], toast: null, fwd: [], flow: null, qStep: {}, errors: null, ideaDraft: null, sending: null
};
const CTX = ['page', 'panel', 'panelTab', 'goal', 'goalTab', 'idea', 'dayView', 'setSec', 'blTab', 'proj', 'relProj', 'overlay', 'dlg', 'flow'];
const hist = [];
const PAGE_NAMES = { needs: 'Needs you', goals: 'Goals', goal: 'Goal', backlog: 'Backlog', plan: 'Plan into waves', ideas: 'Ideas', idea: 'Idea', projects: 'Projects', days: 'Days', releases: 'Releases', settings: 'Settings' };
const PAGE_SCREEN = { needs: 'needs', goals: 'goals', backlog: 'backlog', ideas: 'ideas', days: 'days', releases: 'releases', projects: 'projects', settings: 'settings' };

const live = (ref) => !S.gone.has(ref) && !S.snoozed[ref];
const isDown = () => C && C.state === 'down';
const esc = (v) => String(v).replace(/&/g, '&amp;').replace(/</g, '&lt;');
// Commands that change the board: off while it's down
// Toasts for things that stay on this Mac (open, focus, copy, save a file); every other toast reports a change to the board
const LOCAL_TOAST = /^(Opened|Focused|Copied|Saved |Showing)/;
const mutating = (act, arg) => ['gone', 'snooze', 'unsnooze', 'send', 'bulk', 'dlg-done', 'undo', 'submit-idea'].includes(act) || (act === 'toast' && !LOCAL_TOAST.test(arg || ''));
const OFFLINE = "The board is offline. Try again when it's back.";
// Where focus goes when a row leaves the list: the next row, or the one before at the end
function nextRowSel(ref) {
  const rows = [...C.root.querySelectorAll(`[data-screen="${S.screen}"] .main .row[data-ref]`)].filter(r => r.querySelector('.tt'));
  const i = rows.findIndex(r => r.dataset.ref === ref); if (i < 0) return null;
  const n = rows[i + 1] || rows[i - 1]; return n ? `.main .row[data-ref="${n.dataset.ref}"] .tt` : null;
}
function remove(ref, txt, final) {
  S.focusNext = nextRowSel(ref);
  S.gone.add(ref); if (S.panel === ref) S.panel = null;
  if (!final) S.undo.push({ label: txt, ref });
  if (txt) toast(txt, { undo: !final, pushed: true });
}

function defaults(screen) {
  const sec = C.root.querySelector(`[data-screen="${screen}"]`);
  const d = sec ? sec.dataset : {};
  return {
    page: d.page || screen, panel: d.panel || null, panelTab: d.tab || 'story', goal: d.goal || 'G2',
    goalTab: d.gtab || 'waves', idea: d.idea || 'I2', dayView: d.view || 'day', setSec: d.set || 'accounts',
    blTab: d.bl || 'open', proj: d.proj || 'api', relProj: d.relproj || null, overlay: d.overlay || null, dlg: d.dlg || null, flow: d.flow || null,
    _picked: d.picked || null
  };
}

function go(screen, ctx) {
  S.next = ctx || {};
  S.fwd = [];
  if (screen === S.screen) { applyCtx(Object.assign(defaults(screen), S.next)); S.next = null; render(); return; }
  C.navigate(screen);
}
function applyCtx(ctx) {
  const picked = ctx._picked; delete ctx._picked;
  CTX.forEach(k => { if (k in ctx) S[k] = ctx[k]; });
  if (picked) S.picked = new Set(picked.split(','));
  else if (S.page !== 'backlog') S.picked = new Set();
  S.menu = null;
  if (S.panel) S.lastPanel = S.panel;
}

// ── Render ──
let lastShown = null;
function render() {
  if (!C) return;
  const sec = C.root.querySelector(`[data-screen="${S.screen}"]`);
  if (!sec) return;
  const ds = C.state || 'default';
  const oldMain = sec.querySelector('.main, .sw-main');
  const top = oldMain && lastShown === S.screen ? oldMain.scrollTop : 0;
  const oldInsp = sec.querySelector('.insp-b');
  const itop = oldInsp && lastShown === S.screen ? oldInsp.scrollTop : 0;
  const a = C.root.activeElement;
  const fk = a && a.dataset ? (a.dataset.k || (a.dataset.act ? a.dataset.act + '|' + (a.dataset.arg || '') : null)) : null;
  sec.innerHTML = S.page === 'settings' ? settingsWin(ds) : win(ds);
  const m = sec.querySelector('.main, .sw-main'); if (m) m.scrollTop = top;
  const ib = sec.querySelector('.insp-b'); if (ib) ib.scrollTop = itop;
  lastShown = S.screen;
  if (hist[S.depth]) hist[S.depth].ctx = snapshot();
  if (S.focusNext) {
    const el = sec.querySelector(S.focusNext); S.focusNext = null;
    if (el) { if (!el.matches('a[tabindex], button, input, select, textarea, [tabindex]')) el.tabIndex = -1; el.focus(); }
  } else if (fk) {
    const sel = fk.includes('|') ? `[data-act="${fk.split('|')[0]}"][data-arg="${fk.split('|').slice(1).join('|')}"]` : `[data-k="${fk}"]`;
    let el = null; try { el = sec.querySelector(sel) || (fk.includes('|') ? sec.querySelector(`[data-act="${fk.split('|')[0]}"]:not([data-arg])`) : null); } catch (e) {}
    if (el) el.focus({ preventScroll: true });
  }
  sec.querySelectorAll('.nav, .gl').forEach(g => { const all = [...g.querySelectorAll('a[data-act]')]; const cur = all.find(a => a === C.root.activeElement) || all.find(a => a.getAttribute('aria-current')) || all[0]; all.forEach(a => a.tabIndex = a === cur ? 0 : -1); });
  if (isDown()) { const add = sec.querySelector('form[data-form="idea"] [type=submit]'); if (add) { add.setAttribute('aria-disabled', 'true'); add.classList.add('off'); } }
  if (isDown()) sec.querySelectorAll('[data-act]').forEach(b => { if (mutating(b.dataset.act, b.dataset.arg)) { b.setAttribute('aria-disabled', 'true'); b.classList.add('off'); b.title = "The board is offline"; } });
  placeOverlays(sec);
  C.refresh && C.refresh();
}
function snapshot() { const o = {}; CTX.forEach(k => o[k] = S[k]); return o; }

function win(ds) {
  const panelOn = !!S.panel && ds !== 'loading' && ds !== 'setup' && ds !== 'empty';
  const w = S.insW ? ` style="--insp-w:${S.insW}px"` : '';
  return `<div class="win${S.side ? '' : ' side-off'}${panelOn ? ' panel-on' : ''}"${w}>
  ${toolbar()}
  ${sidebar(ds)}
  <main class="main${ds === 'down' ? ' stale' : ''}" aria-label="${PAGE_NAMES[S.page] || ''}">
    ${ds === 'down' ? downBanner() : ''}
    ${ds === 'setup' ? pgSetup() : page(ds)}
  </main>
  <div class="divider" role="separator" tabindex="0" aria-orientation="vertical" aria-label="Details panel width" aria-valuemin="360" aria-valuemax="640" aria-valuenow="${S.insW || 420}" title="Drag to resize · ← → with focus"></div>
  <aside class="insp" aria-label="Details">${panelOn ? panelHtml(ds) : ''}</aside>
  <div class="layer">${overlays()}</div>
  <div class="toasts" role="status" aria-live="polite">${toastHtml()}</div>
</div>`;
}

function page(ds) {
  switch (S.page) {
    case 'needs': return pgNeeds(ds);
    case 'goals': return pgGoals(ds);
    case 'goal': return pgGoal(ds);
    case 'backlog': return pgBacklog(ds);
    case 'plan': return pgPlan(ds);
    case 'ideas': return pgIdeas(ds);
    case 'idea': return pgIdea(ds);
    case 'projects': return pgProjects(ds);
    case 'days': return pgDays(ds);
    case 'releases': return pgReleases(ds);
  }
  return '';
}

function toolbar() {
  const canBack = S.depth > 0;
  return `<header class="tb" role="toolbar" aria-label="Window">
  <div class="grp">
    <button class="ib" data-act="side" aria-pressed="${S.side}" aria-label="Sidebar" title="Show or hide the sidebar  ⌃⌘S">${ic('side')}</button>
    <button class="ib" data-act="back" aria-label="Back" title="Back  ⌘[" ${canBack ? '' : 'aria-disabled="true"'}>${ic('back')}</button>
    <button class="ib" data-act="fwd" aria-label="Forward" title="Forward  ⌘]" ${S.fwd.length ? '' : 'aria-disabled="true"'}>${ic('fwd')}</button>
  </div>
  <div class="grow"></div>
  <div class="grp">
    <button class="find" data-act="search" aria-label="Search" title="Search everything  ⌘K">${ic('search')}<span>Search</span><span class="kbd">⌘K</span></button>
    <button class="ib" data-act="newidea" aria-label="New idea" title="New idea  ⌘N">${ic('plus')}</button>
    <button class="ib" data-act="panel" aria-pressed="${!!S.panel}" aria-label="Details panel" title="Show or hide the details panel  ⌥⌘I">${ic('insp')}</button>
  </div>
</header>`;
}

function needsCount() {
  return ['T5', 'T3', 'T6', 'B44', 'B41', 'H1', 'WR', 'T7', 'T11', 'T10'].filter(live).length;
}

function sidebar(ds) {
  const ld = ds === 'loading', setup = ds === 'setup', empty = ds === 'empty';
  const n = setup || empty ? '' : ld ? '' : String(needsCount());
  const items = [
    ['needs', 'Needs you', 'inbox', n, true, '⌘1'],
    ['goals', 'Goals', 'goal', setup || empty ? '' : '4', false, '⌘2'],
    ['backlog', 'Backlog', 'backlog', setup || empty ? '' : '23', false, '⌘3'],
    ['ideas', 'Ideas', 'idea', '', false, '⌘4'],
    ['days', 'Days', 'days', setup || empty ? '' : '1 new', true, '⌘5'],
    ['releases', 'Releases', 'release', '', false, '⌘6'],
    ['projects', 'Projects', 'project', setup ? '' : '2', false, '⌘7']
  ];
  const cur = S.page === 'goal' ? null : S.page === 'plan' ? 'backlog' : S.page === 'idea' ? 'ideas' : S.page;
  const nav = items.map(([id, label, icon, cnt, hot, k]) => `<a role="link" tabindex="0" data-act="go" data-arg="${PAGE_SCREEN[id]}" ${cur === id ? 'aria-current="page"' : ''} title="${label}  ${k}" aria-label="${label}${cnt ? ', ' + cnt : ''}">${ic(icon)}<span class="lbl">${label}</span>${cnt ? `<span class="n${hot ? ' hot' : ''}">${cnt}</span><span class="dotn" aria-hidden="true">${cnt.replace(' new', '')}</span>` : ''}</a>`).join('');
  const goals = setup || empty || ld ? '' : `<div><h2>Active goals</h2><div class="gl">${ACTIVE.map(r => {
    const g = GOALS[r];
    return `<a role="link" tabindex="0" data-act="goal" data-arg="${r}" ${S.page === 'goal' && S.goal === r ? 'aria-current="page"' : ''} title="${r} ${g.name} · ${g.stateWords}">${glyph(g.state)}<span class="lbl">${g.name}</span><span class="n">${g.done} of ${g.total}</span></a>`;
  }).join('')}</div></div>`;
  const health = ds === 'down'
    ? `<button class="sf bad" data-act="pop" data-arg="health" aria-label="Health: can't reach the board" title="Health">${ic('red')}<span class="lbl">Board offline</span></button>`
    : empty || setup ? '' : `<button class="sf warn" data-act="pop" data-arg="health" aria-label="Health: Midna is at its terminal limit" title="Health">${ic('pulse')}<span class="lbl">Midna at its limit</span></button>`;
  return `<nav class="side" aria-label="Sidebar">
  <div class="side-scroll">
    <button class="scope" data-act="menu" data-arg="scope" aria-haspopup="menu" aria-label="Projects shown: all projects" title="Projects shown">${ic('project')}<span class="lbl">All projects</span><span class="chev">${ic('down')}</span></button>
    <div class="nav">${nav}</div>
    ${goals}
  </div>
  <div class="side-foot">
    ${health}
    <button class="sf" data-act="pop" data-arg="usage" aria-label="Usage: 5-hour 62%, 7-day 41%" title="Usage">${ic('gauge')}<span class="lbl">5h <span class="meter warm"><i style="width:62%"></i></span> 7d <span class="meter"><i style="width:41%"></i></span></span></button>
    <button class="sf" data-act="pop" data-arg="hours" aria-label="Work hours: until 6:00 PM" title="Work hours">${ic('moon')}<span class="lbl">Work hours until 6:00 PM</span></button>
    <button class="sf" data-act="settings" aria-label="Settings" title="Settings  ⌘,">${ic('gear')}<span class="lbl">Settings</span></button>
  </div>
</nav>`;
}

function downBanner() {
  return `<div class="banner bad" role="alert">${ic('red')}<div class="t"><b>Can't reach the board</b><span class="var">Showing what it sent at 11:42 AM. Actions wait until it's back.</span></div><div class="btnrow"><button class="btn" data-act="restart-board">Restart the board</button></div></div>`;
}

// ── Row component ──
// o: { ref, g (status key) | x (expander key), title, meta:[], age, main:[label, act, arg, pri], sec:[...], later, more, body, kids, urgent }
function row(o) {
  const open = o.x ? !!S.open[o.x] : !!o.body && !!S.open[o.ref];
  const sel = S.panel === o.ref ? ' sel' : '';
  const g = o.x
    ? `<button class="x" data-act="expand" data-arg="${o.x}" aria-expanded="${open}" aria-label="${open ? 'Fold' : 'Unfold'} ${o.title}">${ic('chev')}</button>`
    : o.g ? glyph(o.g) : '';
  const btn = (b, cls) => b ? `<button class="btn ${cls}${b[3] ? ' pri' : ''}" data-act="${b[1]}" data-arg="${b[2] || o.ref}">${b[0]}</button>` : '';
  const titleAct = o.titleAct || ['open', o.ref];
  return `<li class="row${sel}${o.urgent ? ' urgent' : ''}" data-ref="${o.ref}" data-open="${open}">
  <div class="g">${g}</div>
  <div class="mid">
    <div class="ti"><button class="tt" data-act="${titleAct[0]}" data-arg="${titleAct[1]}" data-k="row-${o.ref}">${o.title}</button>${o.showRef === false ? '' : `<span class="ref">${o.ref}</span>`}</div>
    ${o.meta && o.meta.length ? `<div class="meta">${o.meta.map((m, i) => (i ? '<span class="sep" aria-hidden="true"></span>' : '') + `<span>${m}</span>`).join('')}</div>` : ''}
  </div>
  <div class="end">${o.age ? `<span class="age">${o.age}</span>` : ''}${btn(o.main, 'main')}${btn(o.sec, 'sec plain')}${o.later === false ? '' : `<button class="ib" data-act="menu" data-arg="later:${o.ref}" aria-haspopup="menu" aria-label="Later: ${o.title}" title="Later">${ic('clock')}</button>`}${o.more === false ? '' : `<button class="ib" data-act="menu" data-arg="row:${o.ref}" aria-haspopup="menu" aria-label="More for ${o.title}" title="More  ⇧F10">${ic('more')}</button>`}</div>
  ${o.body && open ? `<div class="body">${o.body}</div>` : ''}
  ${o.kids && open ? `<ul class="kids rows">${o.kids}</ul>` : ''}
</li>`;
}

// ── Overlays ──
function overlays() {
  let h = '';
  if (S.overlay === 'search') h += `<div class="scrim" data-act="close-ov"></div>` + palette();
  if (S.overlay && S.overlay.startsWith('pop:')) h += popover(S.overlay.slice(4));
  if (S.menu) h += menuHtml(S.menu);
  if (S.dlg) h += `<div class="scrim" data-act="close-dlg"></div>` + dialog(S.dlg);
  return h;
}

function placeOverlays(sec) {
  const winEl = sec.querySelector('.win, .sw'); if (!winEl) return;
  const wr = winEl.getBoundingClientRect(); const scale = wr.width / winEl.offsetWidth || 1;
  const place = (el, anchorSel, how) => {
    const a = anchorSel && sec.querySelector(anchorSel);
    if (!el) return;
    let x, y;
    if (S.menu && S.menu.at && el.classList.contains('menu')) { x = S.menu.at[0]; y = S.menu.at[1]; }
    else if (a) {
      const r = a.getBoundingClientRect();
      x = (r.left - wr.left) / scale; y = (r.bottom - wr.top) / scale + 4;
      if (how === 'right') { x = (r.right - wr.left) / scale + 8; y = (r.top - wr.top) / scale; }
      if (how === 'end') x = (r.right - wr.left) / scale - el.offsetWidth;
    } else return;
    const W = winEl.offsetWidth, H = winEl.offsetHeight;
    if (x + el.offsetWidth > W - 8) x = Math.max(8, W - 8 - el.offsetWidth);
    if (y + el.offsetHeight > H - 8) {
      const up = a ? (a.getBoundingClientRect().top - wr.top) / scale - el.offsetHeight - 4 : H - 8 - el.offsetHeight;
      y = Math.max(8, how === 'right' ? H - 8 - el.offsetHeight : up);
    }
    el.style.left = x + 'px'; el.style.top = y + 'px';
  };
  const m = sec.querySelector('.layer .menu');
  if (m && S.menu) {
    const anchor = S.menu.anchor ? `[data-act="menu"][data-arg="${S.menu.anchor}"]` : null;
    place(m, anchor, S.menu.anchor && S.menu.anchor.startsWith('row:') || (S.menu.anchor || '').startsWith('later:') || (S.menu.anchor || '').startsWith('task-more') || (S.menu.anchor || '').startsWith('goal-more') ? 'end' : '');
    const first = m.querySelector('[role^=menuitem]:not([aria-disabled="true"])'); if (first) first.focus();
  }
  const p = sec.querySelector('.layer .pop');
  if (p) { const id = S.overlay.slice(4); place(p, `[data-act="pop"][data-arg="${id}"]`, 'right'); const f = p.querySelector('button, a'); if (f && !S.keepFocus) f.focus(); }
  const pal = sec.querySelector('.pal-in input'); if (pal) pal.focus();
  const d = sec.querySelector('.layer .dlg'); if (d && !d.contains(C.root.activeElement)) { const f = d.querySelector('input, textarea, select, .opt, .btn.pri, .btn'); if (f) f.focus(); }
}

const MI = (label, act, arg, k, extra = '') => `<button role="menuitem" class="${extra.includes('danger') ? 'danger' : ''}" data-act="${act}" data-arg="${arg || ''}" ${extra.includes('dis') ? 'aria-disabled="true"' : ''}><span class="lbl">${label}</span>${k ? `<span class="k">${k}</span>` : ''}</button>`;
const SNOOZE = (ref) => [
  MI('In 1 hour', 'snooze', ref + '|back at 1:40 PM'),
  MI('In 3 hours', 'snooze', ref + '|back at 3:40 PM'),
  MI('Tomorrow morning', 'snooze', ref + '|back tomorrow, 9:00 AM'),
  MI('Next week', 'snooze', ref + '|back Mon, Nov 23, 9:00 AM'),
  MI('Until I’m back', 'snooze', ref + '|back when you are'),
  '<hr>', MI('After G2 Faster sign-in finishes', 'snooze', ref + '|back after G2 finishes')
].join('');

function menuHtml(m) {
  const id = m.id; let items = '';
  if (id === 'scope') items = `<h3>Show</h3>
    <button role="menuitemradio" aria-checked="true" data-act="scope" data-arg="all">${ic('check')}<span class="lbl">All projects</span></button>
    <button role="menuitemradio" aria-checked="false" data-act="scope" data-arg="webapp"><span class="i"></span><span class="lbl">webapp</span><span class="k">7</span></button>
    <button role="menuitemradio" aria-checked="false" data-act="scope" data-arg="api"><span class="i"></span><span class="lbl">api</span><span class="k">3</span></button>
    <hr>${MI('Projects', 'go', 'projects', '⌘7')}`;
  else if (id.startsWith('later:')) items = `<h3>Bring it back</h3>${SNOOZE(id.slice(6))}`;
  else if (id.startsWith('row:') || id.startsWith('task-more:')) items = rowMenu(id.split(':')[1], id.startsWith('task-more'));
  else if (id === 'goal-more') items = goalMenu();
  else if (id === 'misses') items = `<h3>Misses holding G9</h3>${MI('B41 Reset links expire after 15 minutes', 'open', 'B41')}<hr>${MI('Make every miss a task', 'toast', 'Made T24 from B41|undo')}`;
  else if (id === 'kind') items = ['All kinds|23', 'Bug|9', 'Test gap|6', 'Follow-up|5', 'Clean-up|3'].map(s => { const [l, n] = s.split('|'); const k = l === 'All kinds' ? 'all' : l; return `<button role="menuitemradio" aria-checked="${S.kind === k}" data-act="kind" data-arg="${k}">${S.kind === k ? ic('check') : '<span class="i"></span>'}<span class="lbl">${l}</span><span class="k">${n}</span></button>`; }).join('');
  else if (id === 'bulk-move') items = `<h3>Move 3 issues into</h3>${MI('G1 Sign-in with passkeys', 'bulk', 'Moved 3 issues into G1')}${MI('G2 Faster sign-in', 'bulk', 'Moved 3 issues into G2')}${MI('G6 Offline mode groundwork', 'bulk', 'Moved 3 issues into G6')}<hr>${MI('Choose a goal…', 'dlg', 'chooser', 'M')}`;
  else if (id === 'bulk-more') items = `${MI('Defer', 'bulk', 'Deferred 3 issues', 'D')}${MI('Won’t do', 'bulk', 'Closed 3 issues as won’t do', 'X')}`;
  else if (id === 'bulk-pri') items = `<h3>Priority</h3>${MI('P1 · now', 'bulk', 'Set 3 issues to P1', '1')}${MI('P2 · soon', 'bulk', 'Set 3 issues to P2', '2')}${MI('P3 · some day', 'bulk', 'Set 3 issues to P3', '3')}`;
  else if (id === 'day-projects') items = `<h3>Projects on the timeline</h3><button role="menuitemcheckbox" aria-checked="true" data-act="noop">${ic('check')}<span class="lbl">webapp</span></button><button role="menuitemcheckbox" aria-checked="true" data-act="noop">${ic('check')}<span class="lbl">api</span></button>`;
  else if (id === 'weeks') items = `<h3>Weeks</h3>${['Nov 14 – 20 · 41 h', 'Nov 7 – 13 · 37 h', 'Oct 31 – Nov 6 · 29 h', 'Oct 24 – 30 · 33 h', 'Oct 17 – 23 · 30 h'].map((w, i) => `<button role="menuitemradio" aria-checked="${i === 0}" data-act="noop">${i === 0 ? ic('check') : '<span class="i"></span>'}<span class="lbl">${w}</span></button>`).join('')}`;
  else if (id === 'start-at') items = `<h3>Start at</h3>${MI('Today, 2:00 PM', 'toast', 'T8 starts today at 2:00 PM|undo')}${MI('Today, 6:30 PM (after hours)', 'toast', 'T8 starts today at 6:30 PM|undo')}${MI('Tomorrow, 9:00 AM', 'toast', 'T8 starts tomorrow at 9:00 AM|undo')}<hr>${MI('Another time…', 'dlg', 'startat')}`;
  else if (id === 'idea-more') items = `${MI('Talk it over', 'toast', 'Opened a conversation about I2 in Midna', '⌘T')}${MI('Dig again', 'toast', 'Digging into I2 again', '⌘D')}<hr>${MI('Shelve', 'toast', 'Shelved I2|undo')}${MI('Won’t do', 'toast', 'Closed I2 as won’t do|undo', '', 'danger')}`;
  else if (id === 'ctx-goal') items = goalMenu();
  return `<div class="menu" role="menu" aria-label="${m.label || 'Menu'}" data-k="menu">${items}</div>`;
}

function rowMenu(ref, inPanel) {
  const t = TASKS[ref], b = ISSUES[ref];
  const open = inPanel ? '' : MI('Open', 'open', ref, '↩');
  if (ref === 'WR') return `${MI('Open the weekly review', 'go', 'week')}<hr>${MI('Dismiss', 'gone', 'WR|Dismissed the weekly review')}`;
  if (ref === 'G3') return `${MI('Open goal', 'goal', 'G3')}${MI('Continue anyway', 'toast', 'Wave 2 can start now|undo')}<hr><h3>Bring it back</h3>${SNOOZE('G3')}`;
  if (b) {
    if (b.miss) return `${open}${MI('Make a task', 'gone', 'B41|Made T24 from B41')}${MI('Make it a Jira ticket', 'gone', 'B41|Made WEB-311 from B41')}${MI('Won’t do', 'gone', 'B41|Closed B41 as won’t do')}<hr>${MI('Finish G9 without it…', 'dlg', 'finishgoal', '', 'danger')}`;
    if (b.holdsPr) return `${open}${MI('Make a task', 'gone', 'B44|Made T25 from B44')}${MI('Open PR #93', 'toast', 'Opened PR #93 in the browser', '', '')}<hr>${MI('Won’t do', 'gone', 'B44|Closed B44 as won’t do')}`;
    return `${open}${MI('Select for a goal', 'pick', ref)}${MI('Make a task', 'toast', 'Made T26 from ' + ref + '|undo')}${MI('Make it a Jira ticket', 'toast', 'Made WEB-312 from ' + ref + '|undo')}<hr>${MI('Defer', 'toast', 'Deferred ' + ref + '|undo', 'D')}${MI('Won’t do', 'toast', 'Closed ' + ref + ' as won’t do|undo', 'X')}`;
  }
  if (!t) return open;
  const term = `${MI('Focus terminal', 'toast', 'Focused ' + ref + '’s terminal in Midna', '⌘↩')}${MI('Detach terminal', 'toast', 'Detached ' + ref + '’s terminal')}${MI('Close terminal…', 'dlg', 'closeterm', '', 'danger')}`;
  const copy = MI('Copy branch name', 'toast', 'Copied ' + ref.toLowerCase() + '-' + t.title.toLowerCase().replace(/[^a-z]+/g, '-').slice(0, 20));
  switch (t.attn) {
    case 'question': return `${open}${MI('Answer in the terminal', 'toast', 'Focused T3’s terminal in Midna')}${MI('Talk it over', 'toast', 'Opened a conversation about T3 in Midna', '⌘T')}<hr>${term}`;
    case 'ownerreview': return `${open}${MI('Open PR #412', 'toast', 'Opened PR #412 in the browser', '⌘O')}${MI('Mark reviewed', 'gone', 'T6|Marked PR #412 reviewed')}<hr>${MI('Merge anyway…', 'dlg', 'merge', '', 'danger')}<hr>${term}`;
    case 'lost': return `${open}${MI('Resume in a new terminal', 'toast', 'Resuming T10 in a new terminal')}${MI('Check and finish', 'gone', 'T10|T10 marked done')}${MI('Reopen the old conversation', 'toast', 'Reopened T10’s conversation in Midna')}${MI('See context', 'open', 'T10')}`;
    case 'stuck': return `${open}${MI('Focus terminal', 'toast', 'Focused T5’s terminal in Midna', '⌘↩')}${MI('Ask for an update', 'toast', 'Asked T5 for an update')}${MI('Dismiss the alert', 'gone', 'T5|Dismissed the alert for T5')}<hr>${MI('Detach terminal', 'toast', 'Detached T5’s terminal')}${MI('Close terminal…', 'dlg', 'closeterm', '', 'danger')}`;
    case 'failed': return `${open}${MI('Try again', 'gone', 'T7|T7 queued again')}${MI('Close terminal…', 'dlg', 'closeterm', '', 'danger')}${copy}`;
    case 'nostart': return `${open}${MI('Start in a new terminal', 'gone', 'T11|T11 starting in a new terminal')}${MI('Queue in Midna', 'gone', 'T11|T11 queued in Midna')}`;
    case 'human': return `${open}${MI('Done', 'gone', 'H1|H1 done')}${MI('Talk it over', 'toast', 'Opened a conversation about H1 in Midna', '⌘T')}<hr>${MI('Waiting on someone…', 'dlg', 'waiting')}${MI('Can’t do', 'gone', 'H1|Marked H1 can’t do')}${MI('Not needed', 'gone', 'H1|Marked H1 not needed')}`;
    case 'start': return `${open}${MI('Start now', 'toast', 'T8 starting')}${MI('Start when the repo is free', 'toast', 'T8 starts when api is free|undo')}<hr>${MI('Start at…', 'menu', 'start-at')}`;
    case 'note': return `${open}${MI('Ask for the note', 'toast', 'Asked T12 to write the tester note. Its conversation is coming back to write it.')}${MI('Open PR #91', 'toast', 'Opened PR #91 in the browser', '⌘O')}`;
  }
  return `${open}${t.status === 'working' ? term : ''}${copy}`;
}

function goalMenu() {
  const g = GOALS[S.goal] || GOALS.G2;
  return `${MI('Open in Claude', 'toast', 'Opened ' + g.ref + ' in a Claude conversation')}${MI('Start now, outside work hours', 'toast', g.ref + ' runs until it finishes tonight|undo')}${MI('Start at…', 'dlg', 'startat')}<hr>${MI('Misses', 'menu', 'misses', '', g.ref === 'G9' ? '' : 'dis')}<hr>${g.state === 'working' ? MI('Pause', 'toast', 'Paused ' + g.ref + '. Running tasks finish first.|undo', '⇧⌘P') : MI('Resume', 'toast', 'Resumed ' + g.ref)}`;
}

function popover(id) {
  if (id === 'health') return `<div class="pop" role="dialog" aria-label="Health" data-k="pop">
    <div class="it"><h3>Board</h3>${isDown() ? st('failed', "Can't reach it since 11:42 AM") : st('done', 'Connected · updated 11:42 AM')}${isDown() ? '<div class="btnrow"><button class="btn pri" data-act="restart-board">Restart the board</button></div>' : ''}</div>
    <div class="it"><h3>Midna</h3>${st('needs', 'At its limit · 8 of 8 terminals')}<span class="lbl-s">T11 waits for a free terminal.</span></div>
    <div class="it"><h3>Accounts</h3>${st('done', 'GitHub connected')}${st('needs', 'Slack needs a new sign-in')}</div>
    <div class="btnrow"><button class="btn" data-act="settings" data-arg="accounts">Accounts</button><button class="btn plain" data-act="settings" data-arg="system">System</button></div>
  </div>`;
  if (id === 'usage') return `<div class="pop" role="dialog" aria-label="Usage" data-k="pop">
    <div class="it"><h3>5-hour window</h3><div class="bar"><i style="width:62%;background:var(--needs)"></i></div><span class="lbl-s num">62% · resets 1:00 PM</span></div>
    <div class="it"><h3>7-day window</h3><div class="bar"><i style="width:41%"></i></div><span class="lbl-s num">41% · resets Tue, 9:00 AM</span></div>
    <div class="it"><h3>Today</h3><span>On pace. webapp 58% · api 42%</span></div>
    <div class="btnrow"><button class="btn" data-act="settings" data-arg="usage">Usage settings</button></div>
  </div>`;
  if (id === 'hours') return `<div class="pop" role="dialog" aria-label="Work hours" data-k="pop">
    <div class="it"><h3>Work hours</h3><span>9:00 AM – 6:00 PM · Mon–Fri</span><span class="lbl-s">New work starts only inside them. The Mac stays awake until 6:00 PM.</span></div>
    <div class="btnrow"><button class="btn" data-act="toast" data-arg="Work hours run until 8:00 PM today|undo">Until 8:00 PM today</button><button class="btn plain" data-act="settings" data-arg="hours">Work hours settings</button></div>
  </div>`;
  return '';
}

function palette() {
  const res = [
    ['Tasks', [['needs', 'T3', 'Passkey management page', 'open', 'T3'], ['working', 'T2', 'Passkey sign-in screen', 'open', 'T2'], ['review', 'T6', 'Cache session lookups', 'open', 'T6']]],
    ['Goals', [['needs', 'G1', 'Sign-in with passkeys', 'goal', 'G1'], ['working', 'G2', 'Faster sign-in', 'goal', 'G2']]],
    ['Backlog', [['queued', 'B38', 'No test for an expired passkey challenge', 'open', 'B38']]],
    ['Ideas', [['done', 'I5', 'Passkey sign-in', 'idea', 'I5']]]
  ];
  let i = 0;
  return `<div class="pal" role="dialog" aria-label="Search" data-k="pal">
  <div class="pal-in">${ic('search')}<input type="search" value="passkey" aria-label="Search tasks, goals, issues and ideas" role="combobox" aria-expanded="true" aria-controls="pal-list" aria-activedescendant="pal-0"><span class="chip">All projects</span></div>
  <div class="pal-r" role="listbox" id="pal-list" aria-label="Results">${res.map(([h, rs]) => `<h3>${h}</h3>${rs.map(([s, ref, t, act, arg]) => `<div role="option" id="pal-${i}" aria-selected="${i++ === 0}" data-act="${act}" data-arg="${arg}">${glyph(s)}<span class="ref">${ref}</span><span>${t}</span><span class="lbl-s">${(TASKS[ref] && TASKS[ref].project) || (GOALS[ref] && GOALS[ref].project) || 'webapp'}</span></div>`).join('')}`).join('')}</div>
  <div class="pal-f"><span><span class="kbd">↑↓</span> move</span><span><span class="kbd">↩</span> open</span><span><span class="kbd">⌘↩</span> open in a new window</span><span><span class="kbd">esc</span> close</span></div>
</div>`;
}

function dialog(id) {
  const foot = (cancel, ok, okAct, okArg, danger) => `<div class="foot"><button class="btn plain" data-act="close-dlg">${cancel}</button><button class="btn ${danger ? 'danger' : 'pri'}" data-act="${okAct}" data-arg="${okArg || ''}">${ok}</button></div>`;
  const d = (label, body) => `<div class="dlg" role="dialog" aria-modal="true" aria-label="${label}" data-k="dlg"><h2>${label}</h2>${body}</div>`;
  switch (id) {
    case 'merge': return d('Merge PR #412 without waiting?', `<p>Merging now skips:</p><ul class="skips"><li>${st('working', 'Checks are still running')}</li><li>${st('needs', 'Your review')}</li><li>${st('queued', 'Sam’s review · 0 of 1 approvals')}</li></ul><p>A conflict with <span class="mono">main</span> still stops it. A merge can’t be undone from here.</p>${foot('Cancel', 'Merge now', 'dlg-done', 'Merged PR #412', true)}`);
    case 'mergeno': return d('GitHub refused the merge', `<p>PR #412 is behind <span class="mono">main</span> by 3 commits and conflicts in <span class="mono">src/session/cache.ts</span>. T6’s agent can rebase it.</p><div class="foot"><button class="btn plain" data-act="close-dlg">Close</button><button class="btn" data-act="toast" data-arg="Focused T6’s terminal in Midna">Focus terminal</button><button class="btn pri" data-act="dlg-done" data-arg="Asked T6 to rebase onto main">Ask it to rebase</button></div>`);
    case 'confirmdone': return d('Mark T2 done now?', `<p>A terminal link asked to mark <b>T2 Passkey sign-in screen</b> done. Yes marks it done without waiting for the agent to hand it in.</p>${foot('Cancel', 'Mark done', 'dlg-done', 'Marked T2 done')}`);
    case 'closeterm': return d('Close this terminal?', `<p>The agent stops where it is. Its handoff stays, so a new terminal can pick it up.</p>${foot('Cancel', 'Close terminal', 'dlg-done', 'Closed the terminal', true)}`);
    case 'finishgoal': return d('Finish G9 without this outcome?', `<p>B41 is the last miss. G9 finishes with “Reset links expire after 15 minutes” unproven, and B41 closes as won’t do.</p>${foot('Cancel', 'Finish G9', 'dlg-done', 'G9 finished', true)}`);
    case 'waiting': return d('Waiting on someone', `<label class="field"><span>Who</span><input class="in" value="Sam"></label>
      <div class="field"><span>Check again</span><div class="seg" role="radiogroup" aria-label="Check again">${['Tomorrow', 'Next week', 'In 2 weeks', 'In a month'].map((x, i) => `<button role="radio" aria-checked="${i === 1}" aria-pressed="${i === 1}" data-act="noop">${x}</button>`).join('')}</div></div>
      ${foot('Cancel', 'Wait', 'dlg-done', 'H1 waits on Sam until Mon, Nov 30|undo')}`);
    case 'note': return d('Note for testers · T13', `<div class="prose dlg-scroll"><p><b>What changed:</b> sign-in lookups now come from a cache instead of the database.</p><p><b>Where to look:</b> sign in and out three times on staging; the second sign-in should feel instant.</p><p><b>Check:</b></p><ul><li>Signing out clears the cached session at once.</li><li>A changed password signs out every other device within a minute.</li><li>Staging’s sign-in chart stays under 400 ms at p95.</li></ul><p><b>Not changed:</b> the sign-in screens and their wording.</p></div><div class="foot"><button class="btn pri" data-act="close-dlg">Close</button></div>`);
    case 'startat': return d('Start at a time', `<div class="time"><input class="in" value="2:00" aria-label="Hour and minute"><div class="seg" role="radiogroup" aria-label="AM or PM"><button role="radio" aria-checked="false" aria-pressed="false" data-act="noop">AM</button><button role="radio" aria-checked="true" aria-pressed="true" data-act="noop">PM</button></div></div><span class="lbl-s">Today, Fri, Nov 20</span>${foot('Cancel', 'Schedule', 'dlg-done', 'Starts today at 2:00 PM|undo')}`);
    case 'chooser': return d('Move into a goal', `<input class="in" type="search" value="" aria-label="Find a goal or project"><ul class="rows">${['G1 Sign-in with passkeys · webapp', 'G2 Faster sign-in · webapp', 'G6 Offline mode groundwork · webapp', 'G8 Session audit trail · api'].map(x => `<li class="row" style="grid-template-columns:0 1fr auto"><span></span><button class="tt" style="border:0;background:none;text-align:left;min-height:24px" data-act="dlg-done" data-arg="Moved 3 issues into ${x.split(' ')[0]}|undo">${x}</button></li>`).join('')}</ul>${foot('Cancel', 'New goal from these…', 'go', 'plan')}`);
    case 'planq': return d('Plan I2 with 2 questions open?', `<p>The plan will guess at “Which data stays offline?” and “What happens on a conflict?”. Answer them first for a better plan.</p>${foot('Answer first', 'Plan anyway', 'dlg-done', 'Planning I2 · a goal appears when it’s ready|undo')}`);
    case 'newidea': return newIdeaDlg();
  }
  return '';
}

function newIdeaDlg() {
  const e = S.errors || {}; const dft = S.ideaDraft || { title: '', project: '', detail: '' };
  return `<form class="dlg" role="dialog" aria-modal="true" aria-label="New idea" data-k="dlg" data-form="idea" novalidate>
  <h2>New idea</h2>
  <label class="field"><span>Idea</span><input class="in" name="title" value="${dft.title}" ${e.title ? 'aria-invalid="true" aria-describedby="e-title"' : ''}>${e.title ? `<em class="err" id="e-title">${ic('needs')}Say what the idea is.</em>` : ''}</label>
  <label class="field"><span>Project</span><select class="in" name="project" ${e.project ? 'aria-invalid="true" aria-describedby="e-proj"' : ''}><option value="">Choose…</option><option ${dft.project === 'webapp' ? 'selected' : ''}>webapp</option><option ${dft.project === 'api' ? 'selected' : ''}>api</option></select>${e.project ? `<em class="err" id="e-proj">${ic('needs')}Choose the project it’s for.</em>` : ''}</label>
  <label class="field"><span>Detail <span class="var">(optional)</span></span><textarea class="in" name="detail" rows="4">${dft.detail}</textarea></label>
  <div class="foot"><button type="button" class="btn plain" data-act="close-dlg">Cancel</button><button type="submit" class="btn pri${S.sending === 'idea' ? ' sending' : ''}">${S.sending === 'idea' ? 'Adding…' : 'Add idea'} <span class="kbd">⌘↩</span></button></div>
</form>`;
}

function toastHtml() {
  if (!S.toast) return '';
  const t = S.toast;
  return `<div class="toast${t.err ? ' err' : ''}"><span>${t.text}</span>${t.undo ? `<button data-act="undo">${t.undoLabel || 'Undo'}</button>` : ''}${t.reopen ? `<button data-act="reopen-draft">Reopen <span class="kbd" style="color:inherit">⌘N</span></button>` : ''}<button data-act="toast-x" aria-label="Dismiss">${ic('close')}</button></div>`;
}
let toastTimer = null;
function toast(text, opts = {}) {
  if (opts.undo && !opts.pushed) S.undo.push({ label: text });
  S.toast = Object.assign({ text }, opts);
  armToast();
}
function armToast() { clearTimeout(toastTimer); toastTimer = setTimeout(() => { S.toast = null; render(); }, 10000); }
function holdToast(e) { if (e.target.closest && e.target.closest('.toast')) clearTimeout(toastTimer); }
function releaseToast(e) { const t = e.target.closest && e.target.closest('.toast'); if (t && S.toast && !(e.relatedTarget && t.contains(e.relatedTarget))) armToast(); }

// ── Actions ──
function setup(canvas) {
  C = canvas;
  canvas.onNavigate(({ screen, depth }) => {
    let ctx;
    if (S.next) { ctx = Object.assign(defaults(screen), S.next); S.next = null; }
    else if (hist[depth] && hist[depth].screen === screen && hist[depth].ctx) ctx = Object.assign({}, hist[depth].ctx);
    else ctx = defaults(screen);
    if (depth < S.depth && hist[S.depth] && S.fwdPush) S.fwd.push({ screen: hist[S.depth].screen, ctx: hist[S.depth].ctx });
    S.fwdPush = false;
    hist.length = depth + 1; hist[depth] = { screen, ctx: null };
    S.screen = screen; S.depth = depth; S.overlay = null; S.dlg = null; S.menu = null;
    applyCtx(ctx);
    const was = C.root.activeElement;
    if (!S.focusNext && !(was && was.closest && was.closest('.side, .sw-side'))) S.focusNext = '.main h1, .sw-col h2';
    render();
  });
  canvas.onState(() => render());
  canvas.onResize(() => { if (S.menu || S.overlay) placeOverlays(C.root.querySelector(`[data-screen="${S.screen}"]`)); });

  const first = canvas.screen || 'needs';
  S.screen = first; hist[0] = { screen: first, ctx: null };
  applyCtx(defaults(first));
  render();

  canvas.root.addEventListener('click', onClick);
  canvas.root.addEventListener('submit', onSubmit);
  canvas.root.addEventListener('keydown', onKey);
  canvas.root.addEventListener('contextmenu', onContext);
  canvas.root.addEventListener('pointerdown', onDividerDown);
  canvas.root.addEventListener('pointerover', holdToast); canvas.root.addEventListener('focusin', holdToast);
  canvas.root.addEventListener('pointerout', releaseToast); canvas.root.addEventListener('focusout', releaseToast);
  canvas.root.addEventListener('focusin', (e) => {
    const ref = e.target.dataset && e.target.dataset.answer; if (!ref || !S.drafts[ref] || S.pick[ref] == null) return;
    e.target.dispatchEvent(new Event('input', { bubbles: true })); // back in the field: the typed text is the answer again
  });
  canvas.root.addEventListener('input', (e) => {
    const ref = e.target.dataset && e.target.dataset.answer; if (!ref) return;
    S.drafts[ref] = e.target.value;
    C.root.querySelectorAll(`[data-answer="${ref}"]`).forEach(f => { if (f !== e.target) f.value = e.target.value; });
    if (e.target.value && S.pick[ref] != null) { delete S.pick[ref]; C.root.querySelectorAll(`[data-act="pickopt"][data-arg^="${ref}|"]`).forEach(o => { o.setAttribute('aria-checked', 'false'); o.setAttribute('aria-pressed', 'false'); }); }
    C.root.querySelectorAll(`[data-send-for="${ref}"]`).forEach(l => { l.textContent = sendLabel(ref); });
  });
  document.addEventListener('keydown', (e) => { if (e.target === document.body && (e.metaKey || e.ctrlKey)) onKey(e); });
}

function onSubmit(e) {
  e.preventDefault();
  const f = e.target;
  if (f.dataset.form === 'idea') submitIdea(f);
}
function submitIdea(f) {
  const title = f.title.value.trim(), project = f.project.value, detail = f.detail.value;
  S.ideaDraft = { title, project, detail };
  if (isDown()) { S.errors = null; toast(OFFLINE + ' Your idea is kept as a draft.', { err: true }); render(); return; }
  const errs = {}; if (!title) errs.title = 1; if (!project) errs.project = 1;
  if (errs.title || errs.project) { S.errors = errs; render(); const bad = C.root.querySelector('[aria-invalid="true"]'); if (bad) bad.focus(); return; }
  S.errors = null; S.sending = 'idea'; render();
  setTimeout(() => { S.sending = null; S.dlg = null; S.ideaDraft = null; toast('Kept I10'); render(); }, 700);
}

function closeAll() { S.menu = null; S.overlay = null; S.dlg = null; }

function onClick(e) {
  const t = e.target.closest('[data-act]');
  if (!t) { if (S.menu && !e.target.closest('.menu')) { S.menu = null; render(); } else if (S.overlay && S.overlay.startsWith('pop:') && !e.target.closest('.pop')) { S.overlay = null; render(); } return; }
  if (t.closest('form') && t.type === 'submit') return;
  const act = t.dataset.act, arg = t.dataset.arg || '';
  if (isDown() && mutating(act, arg)) { e.preventDefault(); S.menu = null; toast(OFFLINE, { err: true }); render(); return; }
  if (t.getAttribute('aria-disabled') === 'true') return;
  e.preventDefault();
  const wasMenu = !!S.menu && act !== 'menu';
  if (wasMenu) S.menu = null;
  run(act, arg, t);
}

function run(act, arg, t) {
  if (isDown() && mutating(act, arg)) { S.menu = null; toast(OFFLINE, { err: true }); render(); return; }
  switch (act) {
    case 'noop': { const g = t.closest('[role=radiogroup], .seg'); if (g) { g.querySelectorAll('[aria-pressed],[aria-checked]').forEach(b => { b.setAttribute('aria-pressed', b === t); if (b.hasAttribute('aria-checked')) b.setAttribute('aria-checked', b === t); }); } if (t.getAttribute('role') === 'menuitemcheckbox') { t.setAttribute('aria-checked', t.getAttribute('aria-checked') !== 'true'); } return; }
    case 'go': closeAll(); return go(arg, arg === 'week' ? { dayView: 'week', page: 'days' } : undefined);
    case 'goal': closeAll(); return go(arg === 'G3' ? 'goal-stopped' : 'goal', { goal: arg, page: 'goal', panel: null });
    case 'idea': closeAll(); return go('idea', { idea: arg, page: 'idea' });
    case 'settings': closeAll(); return go('settings', { page: 'settings', setSec: arg || 'accounts' });
    case 'back': S.fwdPush = true; return C.back();
    case 'fwd': { const f = S.fwd.pop(); if (f) { S.next = Object.assign({}, f.ctx); const keep = S.fwd; C.navigate(f.screen); S.fwd = keep; } return; }
    case 'side': S.side = !S.side; break;
    case 'panel': S.panel = S.panel ? null : S.lastPanel; if (S.panel) S.focusNext = '.insp-h h2'; break;
    case 'open': closeAll(); if (TASKS[arg] || ISSUES[arg]) { S.panelBack = t && t.closest && t.closest('.insp-b') && S.panel && S.panel !== arg ? S.panel : null; S.panel = arg; S.lastPanel = arg; S.panelTab = 'story'; S.focusNext = '.insp [data-k="insp-title"]'; } else if (GOALS[arg]) return run('goal', arg); break;
    case 'panel-back': { const b = S.panelBack; S.panelBack = null; S.panel = b; S.lastPanel = b; S.panelTab = 'story'; S.focusNext = '.insp [data-k="insp-title"]'; break; }
    case 'close-panel': { S.panelBack = null; const was = S.panel; S.panel = null; S.focusNext = `[data-k="row-${was}"]`; break; }
    case 'tab': S.panelTab = arg; S.focusNext = `.tabs [data-arg="${arg}"]`; break;
    case 'gtab': S.goalTab = arg; S.focusNext = `.tabs [data-arg="${arg}"]`; break;
    case 'dview': S.dayView = arg; S.focusNext = `[data-act="dview"][data-arg="${arg}"]`; break;
    case 'bltab': S.blTab = arg; break;
    case 'setsec': S.setSec = arg; S.flow = null; S.focusNext = t && t.closest && t.closest('.sw-side') ? `.sw-side [data-arg="${arg}"]` : `.sw-col h2`; break;
    case 'expand': S.open[arg] = !S.open[arg]; break;
    case 'relproj': S.relProj = arg || null; S.focusNext = 'h1'; break;
    case 'proj': S.proj = arg; S.open['fin-all'] = false; S.focusNext = '.pj-main h2'; break;
    case 'search': S.overlay = 'search'; break;
    case 'close-ov': S.overlay = null; S.focusNext = '.find'; break;
    case 'pop': S.overlay = S.overlay === 'pop:' + arg ? null : 'pop:' + arg; break;
    case 'menu': {
      if (S.menu && S.menu.anchor === arg) { S.menu = null; break; }
      S.menu = { id: arg, anchor: t.closest('.menu') ? (S.menu && S.menu.anchor) : arg, from: arg };
      if (t.closest('.menu')) S.menu.anchor = S.lastAnchor; else S.lastAnchor = arg;
      break;
    }
    case 'dlg': S.menu = null; S.dlg = arg; S.dlgOpener = t && t.closest && t.closest('.menu') ? (S.lastAnchor ? `[data-act="menu"][data-arg="${S.lastAnchor}"]` : null) : t && t.dataset && t.dataset.k ? `[data-k="${t.dataset.k}"]` : `[data-act="dlg"][data-arg="${arg}"]`; break;
    case 'newidea': S.menu = null; S.overlay = null; S.dlg = 'newidea'; S.errors = null; S.dlgOpener = t && t.dataset && t.dataset.act === 'newidea' ? '[data-act="newidea"]' : '.find'; break;
    case 'reopen-draft': S.dlg = 'newidea'; S.toast = null; break;
    case 'close-dlg': {
      if (S.dlg === 'newidea') { const f = C.root.querySelector('form[data-form="idea"]'); if (f && (f.title.value || f.detail.value)) { S.ideaDraft = { title: f.title.value, project: f.project.value, detail: f.detail.value }; toast('Idea draft kept', { reopen: true }); } }
      S.dlg = null; S.errors = null; S.focusNext = S.dlgOpener || null; break;
    }
    case 'dlg-done': {
      S.dlg = null; S.focusNext = S.dlgOpener || null; const [txt, u] = arg.split('|');
      if (txt.startsWith('Merged') || txt.startsWith('G9 finished')) remove(txt.startsWith('Merged') ? 'T6' : 'B41', txt, true); else toast(txt, { undo: !!u });
      break;
    }
    case 'toast': { const [txt, u] = arg.split('|'); toast(txt, { undo: !!u }); break; }
    case 'gone': { const [ref, txt] = arg.split('|'); remove(ref, txt); break; }
    case 'snooze': { const [ref, when] = arg.split('|'); S.focusNext = nextRowSel(ref); S.snoozed[ref] = when; if (S.panel === ref) S.panel = null; toast(`${ref} is in Later, ${when}`, { undo: true }); S.undo[S.undo.length - 1].ref = ref; break; }
    case 'unsnooze': delete S.snoozed[arg]; toast(`${arg} is back in Needs you`); break;
    case 'undo': {
      const u = S.undo.pop(); if (!u) break;
      if (u.ref) { S.gone.delete(u.ref); delete S.snoozed[u.ref]; S.focusNext = `.main .row[data-ref="${u.ref}"] .tt`; }
      const next = S.undo[S.undo.length - 1];
      toast(`Undone: ${u.label}`, next ? { undo: true, pushed: true, undoLabel: `Undo “${next.label}”` } : {});
      break;
    }
    case 'toast-x': S.toast = null; break;
    case 'pickopt': { const [ref, i] = arg.split('|'); S.pick[ref] = +i; break; }
    case 'send': {
      if (S.pick[arg] == null && !S.drafts[arg]) { toast('Pick an answer or type one first.', { err: true }); break; }
      S.sending = arg; render();
      setTimeout(() => { S.sending = null; delete S.drafts[arg]; delete S.pick[arg]; remove(arg, `Answer sent to ${arg}`); render(); }, 600);
      return;
    }
    case 'qstep': { const [ref, n] = arg.split('|'); S.qStep[ref] = +n; break; }
    case 'restart-board': toast('Restarting the board…'); break;
    case 'scope': toast(arg === 'all' ? 'Showing all projects' : `Showing only ${arg}`); break;
    case 'kind': S.kind = arg; break;
    case 'lens': S.lens = arg; break;
    case 'pick': { if (S.picked.has(arg)) S.picked.delete(arg); else S.picked.add(arg); break; }
    case 'plangrp': closeAll(); return go('plan', { page: 'plan', _picked: arg });
    case 'pickgrp': { const refs = arg.split(','); const all = refs.every(r => S.picked.has(r)); refs.forEach(r => all ? S.picked.delete(r) : S.picked.add(r)); break; }
    case 'fold': if (S.folded.has(arg)) S.folded.delete(arg); else S.folded.add(arg); break;
    case 'clear': S.picked = new Set(); break;
    case 'bulk': { const n = S.picked.size; toast(arg.replace('3', n), { undo: true }); S.picked = new Set(); break; }
    case 'flow': S.flow = arg || null; break;
    case 'copy': toast(`Copied ${arg}`); break;
  }
  render();
}

function onContext(e) {
  const r = e.target.closest('.row[data-ref], .wave .row');
  if (!r || !r.dataset.ref) return;
  e.preventDefault();
  const sec = C.root.querySelector(`[data-screen="${S.screen}"]`); const winEl = sec.querySelector('.win');
  const wr = winEl.getBoundingClientRect(); const sc = wr.width / winEl.offsetWidth || 1;
  const ref = r.dataset.ref;
  S.menu = { id: GOALS[ref] && !TASKS[ref] ? 'row:' + ref : 'row:' + ref, at: [(e.clientX - wr.left) / sc, (e.clientY - wr.top) / sc], label: 'Actions for ' + ref };
  S.lastAnchor = null;
  render();
}

function onKey(e) {
  const k = e.key, mod = e.metaKey || e.ctrlKey;
  const inField = e.target && /^(INPUT|TEXTAREA|SELECT)$/.test(e.target.tagName);
  // menus
  const menu = C.root.querySelector('.layer .menu');
  if (menu && (k === 'ArrowDown' || k === 'ArrowUp' || k === 'Home' || k === 'End')) {
    const items = [...menu.querySelectorAll('[role^=menuitem]:not([aria-disabled="true"])')];
    let i = items.indexOf(C.root.activeElement);
    i = k === 'Home' ? 0 : k === 'End' ? items.length - 1 : (i + (k === 'ArrowDown' ? 1 : -1) + items.length) % items.length;
    items[i] && items[i].focus(); e.preventDefault(); return;
  }
  if (k === 'Escape') {
    if (S.menu) { const a = S.menu.anchor; S.menu = null; S.focusNext = a ? `[data-act="menu"][data-arg="${a}"]` : null; render(); e.preventDefault(); return; }
    if (S.dlg) { run('close-dlg', ''); e.preventDefault(); return; }
    if (S.overlay) { const o = S.overlay; S.overlay = null; S.focusNext = o === 'search' ? '.find' : `[data-act="pop"][data-arg="${o.slice(4)}"]`; render(); e.preventDefault(); return; }
    if (S.panel) { run('close-panel', ''); e.preventDefault(); return; }
  }
  if (k === 'Tab' && (S.dlg || S.overlay === 'search')) { // trap focus in modal
    const box = C.root.querySelector('.layer .dlg, .layer .pal'); if (box) {
      const f = [...box.querySelectorAll('button, input, textarea, select, [tabindex="0"]')].filter(x => x.offsetParent);
      if (f.length) { const i = f.indexOf(C.root.activeElement); if (e.shiftKey && i <= 0) { f[f.length - 1].focus(); e.preventDefault(); } else if (!e.shiftKey && i === f.length - 1) { f[0].focus(); e.preventDefault(); } }
    }
  }
  if (S.overlay === 'search' && (k === 'ArrowDown' || k === 'ArrowUp' || k === 'Enter')) {
    const opts = [...C.root.querySelectorAll('.pal-r [role=option]')]; let i = opts.findIndex(o => o.getAttribute('aria-selected') === 'true');
    if (k === 'Enter') { const o = opts[i]; if (o) { S.overlay = null; run(o.dataset.act, o.dataset.arg, o); } e.preventDefault(); return; }
    i = (i + (k === 'ArrowDown' ? 1 : -1) + opts.length) % opts.length;
    opts.forEach((o, j) => o.setAttribute('aria-selected', j === i)); C.root.querySelector('.pal-in input').setAttribute('aria-activedescendant', opts[i].id); opts[i].scrollIntoView({ block: 'nearest' }); e.preventDefault(); return;
  }
  if (mod && k === 'Enter') {
    const f = e.target.closest && e.target.closest('form'); if (f) { e.preventDefault(); f.requestSubmit(); return; }
    const ans = e.target.dataset && e.target.dataset.answer; if (ans) { e.preventDefault(); run('send', ans); return; }
  }
  if (k === 'Enter' && !mod && e.target.dataset && e.target.dataset.answer === undefined && e.target.name === 'title') { e.preventDefault(); e.target.closest('form').requestSubmit(); return; }
  if ((k === 'F10' && e.shiftKey) || k === 'ContextMenu') {
    const r = e.target.closest && e.target.closest('.row[data-ref]');
    if (r) { e.preventDefault(); S.menu = { id: 'row:' + r.dataset.ref, anchor: 'row:' + r.dataset.ref }; S.lastAnchor = S.menu.anchor; render(); }
    return;
  }
  if (mod && S.page === 'settings' && /^[1-9]$/.test(k)) { e.preventDefault(); return run('setsec', SET[+k - 1][0]); }
  if (mod && S.page === 'settings' && k === 'w') { e.preventDefault(); return run('back'); }
  if (S.page === 'settings' && k === 'Escape' && !inField && !S.dlg && !S.menu && !S.overlay) { e.preventDefault(); return run('back'); }
  if (mod && (k === 'z' || k === 'Z') && !e.shiftKey && !inField && (S.undo.length || isDown())) { e.preventDefault(); return run('undo'); }
  if (mod) {
    const map = { '1': 'needs', '2': 'goals', '3': 'backlog', '4': 'ideas', '5': 'days', '6': 'releases', '7': 'projects' };
    if (map[k]) { e.preventDefault(); return go(map[k]); }
    if (k === 'k') { e.preventDefault(); return run('search'); }
    if (k === 'n') { e.preventDefault(); return run('newidea'); }
    if (k === ',') { e.preventDefault(); return run('settings', ''); }
    if (k === '[') { e.preventDefault(); return run('back'); }
    if (k === ']') { e.preventDefault(); return run('fwd'); }
    if (k === 's' && e.ctrlKey && e.metaKey) { e.preventDefault(); return run('side'); }
    if (k === 'i' && e.altKey) { e.preventDefault(); return run('panel'); }
    if ((k === 'c' || k === 'C') && e.shiftKey && S.panel) { e.preventDefault(); return run('copy', S.panel); }
    if (k === 'f' && S.page === 'backlog') { e.preventDefault(); const f = C.root.querySelector('.bl-find'); if (f) f.focus(); return; }
  }
  if (inField) return;
  // links drawn as anchors without an address: Enter and Space activate them like buttons
  if ((k === 'Enter' || k === ' ') && e.target.matches && e.target.matches('a[data-act], [role=link][data-act]')) { e.preventDefault(); e.target.click(); return; }
  // ↑↓ inside the sidebar page list and the Settings section list
  const grp = /^(ArrowDown|ArrowUp|Home|End)$/.test(k) && e.target.closest && e.target.closest('.nav, .gl');
  if (grp) {
    const all = [...grp.querySelectorAll('a[data-act]')]; const i = all.indexOf(e.target);
    const n = k === 'Home' ? all[0] : k === 'End' ? all[all.length - 1] : all[i + (k === 'ArrowDown' ? 1 : -1)];
    if (n) { all.forEach(a => a.tabIndex = -1); n.tabIndex = 0; n.focus(); e.preventDefault(); } return;
  }
  // list navigation: ↑↓ move between rows of the main list, ↩ opens
  if ((k === 'ArrowDown' || k === 'ArrowUp') && e.target.closest && e.target.closest('.main') && e.target.classList.contains('tt')) {
    const all = [...C.root.querySelectorAll(`[data-screen="${S.screen}"] .main .tt`)];
    const i = all.indexOf(e.target); const n = all[i + (k === 'ArrowDown' ? 1 : -1)];
    if (n) { n.focus(); e.preventDefault(); }
    return;
  }
  if (S.page === 'backlog' && S.picked.size && !mod) {
    if (k === 'm' || k === 'M') { S.menu = { id: 'bulk-move', anchor: 'bulk-move' }; S.lastAnchor = 'bulk-move'; render(); return; }
    if (k === 'p' || k === 'P') { S.menu = { id: 'bulk-pri', anchor: 'bulk-pri' }; S.lastAnchor = 'bulk-pri'; render(); return; }
    if ('123'.includes(k)) return run('bulk', `Set 3 issues to P${k}`);
    if (k === 'd' || k === 'D') return run('bulk', 'Deferred 3 issues');
    if (k === 'x' || k === 'X') return run('bulk', 'Closed 3 issues as won’t do');
  }
  if (k === ' ' && S.page === 'backlog' && e.target.closest && e.target.closest('.row[data-ref]') && e.target.classList.contains('tt')) { e.preventDefault(); run('pick', e.target.closest('.row').dataset.ref); }
  // divider keyboard resize
  if (e.target.classList && e.target.classList.contains('divider') && (k === 'ArrowLeft' || k === 'ArrowRight')) {
    S.insW = Math.max(360, Math.min(640, (S.insW || 420) + (k === 'ArrowLeft' ? 20 : -20))); S.focusNext = '.divider'; render(); e.preventDefault();
  }
}

function onDividerDown(e) {
  const d = e.target.closest('.divider'); if (!d) return;
  const winEl = d.closest('.win'); const wr = winEl.getBoundingClientRect(); const sc = wr.width / winEl.offsetWidth || 1;
  const insp = winEl.querySelector('.insp');
  const move = (ev) => { const w = Math.max(360, Math.min(640, (wr.right - ev.clientX) / sc)); S.insW = Math.round(w); winEl.style.setProperty('--insp-w', S.insW + 'px'); d.setAttribute('aria-valuenow', S.insW); };
  const up = () => { window.removeEventListener('pointermove', move); window.removeEventListener('pointerup', up); };
  window.addEventListener('pointermove', move); window.addEventListener('pointerup', up);
  e.preventDefault();
}
