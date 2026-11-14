window.ultrahandSetup = function (canvas) {
  const R = canvas.root;
  const $ = (s, el) => (el || R).querySelector(s);
  const $$ = (s, el) => [...(el || R).querySelectorAll(s)];
  const esc = s => String(s).replace(/[&<>"]/g, c => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  const IC = {
    search: '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="7" cy="7" r="4.5"/><path d="M10.5 10.5 14 14"/></svg>',
    x: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8"/></svg>',
    check: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3.5 8.5l3 3 6-7"/></svg>',
    alert: '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6"/><path d="M8 5v3.5M8 11h.01"/></svg>',
    gear: '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="2.2"/><path d="M8 1.8v2M8 12.2v2M1.8 8h2M12.2 8h2M3.6 3.6 5 5M11 11l1.4 1.4M3.6 12.4 5 11M11 5l1.4-1.4"/></svg>',
  };

  /* ---------------- Sample board ---------------- */
  const GNAME = { G1: 'Sign-in with passkeys', G2: 'Faster sign-in' };
  const TASKS = [
    { r: 'T3', t: 'Settings page: manage passkeys', s: 'needs', sl: 'Needs you', gr: 'G1' },
    { r: 'T8', t: 'Batch the session writes', s: 'working', sl: 'Working', gr: 'G2' },
    { r: 'T2', t: 'Sign in with a passkey', s: 'working', sl: 'Working', gr: 'G1' },
    { r: 'T10', t: 'Re-measure and compare', s: 'planned', sl: 'Planned', gr: 'G2' },
    { r: 'T9', t: 'Drop the extra redirect', s: 'planned', sl: 'Planned', gr: 'G2' },
    { r: 'T5', t: 'Upgrade the HTTP client', s: 'planned', sl: 'Planned', proj: 'webapp' },
    { r: 'T4', t: 'Docs: passkey support', s: 'planned', sl: 'Planned', gr: 'G1' },
    { r: 'T1', t: 'Add the WebAuthn registration endpoint', s: 'done', sl: 'Done', gr: 'G1', kw: 'passkey', why: 'Matched in its description', at: '11:52 AM' },
  ];
  const GOALS = [
    { r: 'G2', t: 'Faster sign-in', s: 'working', sl: 'Working', side: '2 of 7 done' },
    { r: 'G1', t: 'Sign-in with passkeys', s: 'needs', sl: 'Needs you', side: '1 of 4 done' },
  ];
  const IDEAS = [
    { r: 'I3', t: 'Let the CLI sign in with a passkey', s: 'results', sl: 'Ready to plan', proj: 'webapp' },
    { r: 'I2', t: 'Dark app icon', s: 'neutral', sl: 'New', proj: 'webapp' },
    { r: 'I1', t: 'Weekly digest email', s: 'neutral', sl: 'New', proj: 'api' },
  ];
  const BACKLOG = [
    { r: 'B4', t: 'Split cargo test --workspace by crate', s: 'neutral', sl: 'Follow-up', gr: 'G1', kw: 'passkey', why: 'Matched in details' },
    { r: 'B1', t: 'No test for an expired challenge', s: 'neutral', sl: 'Test gap', gr: 'G1', kw: 'passkey', why: 'Matched in details' },
  ];
  const TERMS = [
    { r: 'T3', t: 'webapp · Settings page: manage passkeys', s: 'needs', sl: 'Needs you', open: 'T3' },
    { r: 'T2', t: 'webapp · Sign in with a passkey', s: 'working', sl: 'Working', open: 'T2' },
    { r: '', t: 'api · api shell', dim: true },
  ];
  const GROUPS = [['tasks', 'Tasks', TASKS], ['goals', 'Goals', GOALS], ['ideas', 'Ideas', IDEAS], ['backlog', 'Backlog', BACKLOG], ['terms', 'Terminals', TERMS]];
  const PAGES = [['needs', 'Needs you', '⌘1', '1'], ['projects', 'Projects', '⌘2', '2'], ['goals', 'Goals', '⌘3', '2'], ['backlog', 'Backlog', '⌘4', '2'], ['ideas', 'Ideas', '⌘5', ''], ['days', 'Days', '⌘6', ''], ['releases', 'Releases', '⌘7', '']];
  const PNAME = Object.fromEntries(PAGES.map(p => [p[0], p[1]]));

  /* ---------------- Menus ---------------- */
  const OLD_MENUS = [
    ['Task board', [['Settings…', '⌘,'], '-', ['Hide Task board', '⌘H'], ['Hide Others', '⌥⌘H'], ['Show All'], '-', ['Quit Task board', '⌘Q']]],
    ['File', [['Close Window', '⌘W'], '-', ['Export for Debugging…']]],
    ['View', [['Needs you', '⌘1'], ['Projects', '⌘2'], ['Goals', '⌘3'], ['Backlog', '⌘5'], ['Days', '⌘6'], '-', ['Search…', '⌘K'], '-', ['Back', '⌘←'], ['Forward', '⌘→'], '-', ['Refresh', '⌘R']]],
  ];
  function menusFor(A) {
    const task = A && A.panel; const page = A ? A.page : 'needs';
    return [
      ['app', 'Task board', [{ l: 'About Task board' }, '-', { l: 'Check for Updates…' }, { l: 'Settings…', k: '⌘,', act: 'settings' }, '-', { l: 'Install Hooks', disabled: true }, { l: 'Restart Daemon' }, '-', { l: 'Hide Task board', k: '⌘H' }, { l: 'Hide Others', k: '⌥⌘H' }, { l: 'Show All' }, '-', { l: 'Quit Task board', k: '⌘Q' }]],
      ['file', 'File', [{ l: 'New Idea…', k: '⌘N', act: 'new-idea' }, '-', { l: 'Close Window', k: '⌘W' }]],
      ['edit', 'Edit', [{ l: A && A.undoLabel ? A.undoLabel : 'Undo', k: '⌘Z', disabled: !(A && A.undoLabel), act: 'undo' }, { l: 'Redo', k: '⇧⌘Z', disabled: true }, '-', { l: 'Cut', k: '⌘X', disabled: true }, { l: 'Copy', k: '⌘C' }, { l: 'Paste', k: '⌘V', disabled: true }, { l: 'Select All', k: '⌘A' }, '-', { l: 'Copy Reference', k: '⇧⌘C', disabled: !task, act: 'copy-ref' }]],
      ['view', 'View', [...PAGES.map(p => ({ l: p[1], k: p[2], checked: page === p[0], act: 'go:' + p[0] })), '-', { l: 'Search…', k: '⌘K', act: 'search' }, '-', { l: 'Back', k: '⌘[', disabled: !(A && A.back.length), act: 'back' }, { l: 'Forward', k: '⌘]', disabled: !(A && A.fwd.length), act: 'fwd' }, '-', { l: 'Refresh', k: '⌘R', act: 'refresh' }]],
      ['task', 'Task', [{ l: 'Focus in Midna', disabled: !task, act: 'midna' }, { l: 'Open Pull Request', k: '⌘↩', disabled: !task }, '-', { l: 'Start', disabled: !task }, { l: 'Stop', disabled: !task }, { l: 'Done', disabled: !task }]],
      ['idea', 'Idea', [{ l: 'Dig', k: '⌘D', disabled: true }, { l: 'Plan', k: '⇧⌘P', disabled: true }, { l: 'Talk It Over', k: '⌘T', disabled: true }, '-', { l: 'Close Idea', disabled: true }]],
      ['window', 'Window', [{ l: 'Minimize', k: '⌘M' }, { l: 'Zoom' }, '-', { l: 'Bring All to Front' }, '-', { l: (PNAME[page] || 'Sign-in with passkeys') + ' — Task board', checked: true }]],
      ['help', 'Help', [{ l: 'Open Log' }, { l: 'Export for Debugging…' }]],
    ];
  }
  const miHTML = (it, i) => it === '-' ? '<div class="sep" role="separator"></div>' :
    `<button class="mi${it.disabled ? ' dis' : ''}" role="menuitem" tabindex="-1" data-i="${i}" ${it.disabled ? 'aria-disabled="true"' : ''} ${it.act ? `data-act="${it.act}"` : ''}><span class="mi-c">${it.checked ? IC.check : ''}</span><span>${esc(it.l)}</span><span class="mi-k">${it.k || ''}</span></button>`;
  const menuHTML = items => `<div class="menu" role="menu">${items.map(miHTML).join('')}</div>`;

  // Static menu comparison in the doc
  $('#menus-old').innerHTML = OLD_MENUS.map(([n, items]) => `<div class="mcol"><span class="mname">${n}</span><div class="o-menu">${items.map(it => it === '-' ? '<div class="o-sep"></div>' : `<div class="o-mi">${esc(it[0])}<span class="o-k">${it[1] || ''}</span></div>`).join('')}</div></div>`).join('');
  $('#menus-new').innerHTML = menusFor(null).map(([id, n, items]) => `<div class="mcol"><span class="mname">${n}</span><div class="menu" aria-label="${n} menu">${items.map(it => it === '-' ? '<div class="sep"></div>' : `<div class="mi${it.disabled ? ' dis' : ''}"><span class="mi-c">${it.checked ? IC.check : ''}</span><span>${esc(it.l)}</span><span class="mi-k">${it.k || ''}</span></div>`).join('')}</div></div>`).join('');

  /* ---------------- Key table ---------------- */
  const KEYS = [
    ['⌘1 … ⌘7', 'Needs you, Projects, Goals, Backlog, Ideas, Days, Releases', '⌘1–⌘3, Backlog ⌘5, Days ⌘6'],
    ['⌘K', 'Search', 'Same'], ['⌘N', 'New Idea…', 'None'], ['⌘R', 'Refresh', 'Same'], ['⌘,', 'Settings', 'Same'], ['⌘W', 'Close Window', 'Same'],
    ['⌘C', 'Copy the selected text', 'Same'], ['⇧⌘C', 'Copy Reference of the open task (greyed with none open)', 'Same, not in a menu'],
    ['⌘[ or ⌘←', 'Back', '⌘← only'], ['⌘] or ⌘→', 'Forward', '⌘→ only'],
    ['⌘D · ⇧⌘P · ⌘T', 'Dig, Plan, Talk It Over on an idea page', 'None'], ['⌘↩', 'Open the pull request of the open or selected task', 'None'],
    ['↑ ↓ · ↩ · Esc', 'Move in a list or search, open, close the top layer', 'Same'],
    ['← → · T · [ ]', 'On Days with focus: day before and after, Today, week before and after', 'None'],
    ['⌘Z', 'Undo the last closing signal (option below)', 'None'],
  ];
  $('#keyrows').innerHTML = KEYS.map(k => `<tr><th scope="row"><span class="k">${k[0]}</span></th><td>${k[1]}</td><td class="muted">${k[2]}</td></tr>`).join('');

  /* ---------------- State → what the window shows ---------------- */
  function viewFor(st) {
    const v = { page: 'needs', panel: null, pal: null, menu: null, toasts: [], undoLabel: null, dialog: false };
    switch (st) {
      case 'menu': v.menu = 'view'; break;
      case 'search-empty': v.pal = { q: '' }; break;
      case 'search-typed': v.pal = { q: 'pass' }; break;
      case 'search-all': v.pal = { q: '', all: 'tasks', sel: 'tasks:T9' }; break;
      case 'search-goal': v.page = 'goal:G1'; v.pal = { q: 'pass', scope: 'G1' }; break;
      case 'search-ref': v.pal = { q: 'G1' }; break;
      case 'search-none': v.pal = { q: 'zzzz' }; break;
      case 'search-wait': v.pal = { q: 'pass', wait: true }; break;
      case 'search-down': v.pal = { q: 'pass', down: true }; break;
      case 'link': v.panel = 'T3'; v.toasts = [{ k: 'done', t: 'Idea draft kept', a: 'Reopen', key: '⌘N', act: 'new-idea' }]; break;
      case 'link-bad': v.toasts = [{ k: 'error', t: 'Couldn’t open T99: it’s not on this board.' }]; break;
      case 'midna': v.panel = 'T3'; v.toasts = [{ k: 'error', t: 'Midna isn’t running, so it can’t show T3’s terminal.' }]; break;
      case 'releases-off': v.page = 'releases'; break;
      case 'undo': v.menu = 'edit'; v.undoLabel = 'Undo Mark Done'; v.toasts = [{ k: 'undo', t: 'Marked T6 done', a: 'Undo', key: '⌘Z', act: 'undo' }]; break;
      case 'no-undo': v.menu = 'edit'; v.toasts = [{ k: 'done', t: 'Marked T6 done' }]; break;
    }
    v.back = []; v.fwd = [];
    return v;
  }

  /* ---------------- Search ---------------- */
  const hl = (text, q) => {
    if (!q) return esc(text);
    const i = text.toLowerCase().indexOf(q.toLowerCase());
    if (i < 0) return esc(text);
    return esc(text.slice(0, i)) + '<mark>' + esc(text.slice(i, i + q.length)) + '</mark>' + esc(text.slice(i + q.length));
  };
  function results(p) {
    const q = p.q.trim().toLowerCase();
    let groups = GROUPS.map(([id, name, items]) => {
      let list = items.filter(it => {
        if (p.scope && it.gr !== p.scope) return false;
        if (!q) return true;
        return (it.r + ' ' + it.t + ' ' + (it.kw || '') + ' ' + (it.gr || '')).toLowerCase().includes(q);
      }).map(it => {
        const exact = !!q && it.r.toLowerCase() === q, inTitle = q && it.t.toLowerCase().includes(q);
        return { ...it, score: exact ? 0 : inTitle ? 1 : 2, viaText: q && !exact && !inTitle && it.why && !(it.gr || '').toLowerCase().includes(q) };
      });
      if (q) list.sort((a, b) => a.score - b.score);
      return { id, name, list, exact: list.some(i => i.score === 0) };
    });
    if (p.scope) groups = groups.filter(g => g.id === 'tasks' || g.id === 'backlog');
    groups = groups.filter(g => g.list.length);
    groups.sort((a, b) => (b.exact - a.exact));
    return groups;
  }
  function palListHTML(p) {
    if (p.wait) return `<div class="pmsg" role="status"><span class="t-label muted">Searching…</span></div>`;
    if (p.down) return `<div class="pmsg" role="alert"><span class="t-title">Can’t reach the board</span><p class="muted">It stopped answering at 9:42 AM.</p><button class="btn secondary sm" data-act="retry">Retry</button></div>`;
    const groups = results(p);
    if (!groups.length) return `<div class="pmsg" role="status"><p>Nothing matches “${esc(p.q)}” in ${p.scope ? 'this goal' : 'tasks, goals, ideas, backlog or terminals'}.</p>${p.scope ? `<button class="btn secondary sm" data-act="unscope">Search everything</button>` : ''}</div>`;
    let sel = p.sel; const keys = [];
    const out = groups.map(g => {
      const show = p.all === g.id ? g.list : g.list.slice(0, 5);
      const head = `<div class="pg t-label-sm" role="presentation">${g.name} <span class="pg-n">· ${g.list.length}</span>${g.id === 'terms' ? ' <span class="warn">· Midna isn’t running</span>' : ''}</div>`;
      const rows = show.map(it => {
        const key = g.id + ':' + (it.r || it.t); if (!it.dim) keys.push(key);
        const go = it.open ? 'T:' + it.open : g.id === 'tasks' ? 'T:' + it.r : g.id === 'goals' ? 'G:' + it.r : g.id === 'ideas' ? 'P:ideas' : 'P:backlog';
        const gn = it.gr ? GNAME[it.gr] : it.proj || it.side || '';
        return `<button class="pr${it.dim ? ' dim' : ''}" role="option" tabindex="-1" data-key="${esc(key)}" ${it.dim ? 'aria-disabled="true"' : `data-open="${go}"`}><span class="ref">${it.r}</span><span class="tl"><span>${hl(it.t, p.q.trim())}</span>${it.viaText ? `<span class="why">${it.why}</span>` : ''}</span>${it.sl ? `<span class="pill ${it.s}">${it.sl}</span>` : '<span></span>'}<span class="gn">${esc(g.id === 'goals' ? it.side : gn)}</span></button>`;
      }).join('');
      const more = g.list.length > 5 && p.all !== g.id ? `<button class="pr more" tabindex="-1" data-act="all:${g.id}" data-key="more:${g.id}"><span></span><span>Show all ${g.list.length} ${g.name.toLowerCase()}</span></button>` : '';
      if (more) keys.push('more:' + g.id);
      return head + rows + more;
    }).join('');
    const every = p.scope ? `<button class="pr more" tabindex="-1" data-act="unscope" data-key="every"><span>${IC.search}</span><span>Search everything for “${esc(p.q.trim())}”</span></button>` : '';
    if (p.scope) keys.push('every');
    if (!sel || !keys.includes(sel)) sel = keys[0];
    p.sel = sel; p.keys = keys;
    return (out + every).replace(`data-key="${esc(sel)}"`, `data-key="${esc(sel)}" aria-selected="true"`);
  }
  const palHTML = p => `<div class="pscrim" data-scrim="pal"><div class="pal" role="dialog" aria-label="Search"><div class="pal-in">${p.wait ? '<span class="spin" aria-hidden="true"></span>' : IC.search}${p.scope ? `<span class="scope"><span>In ${GNAME[p.scope]}</span><button data-act="unscope" aria-label="Search everything" data-tip="Search everything">${IC.x}</button></span>` : ''}<input type="text" aria-label="Search" placeholder="Search" value="${esc(p.q)}" spellcheck="false" autocomplete="off"></div><div class="pal-list" role="listbox" aria-label="Results">${palListHTML(p)}</div></div></div>`;

  /* ---------------- Window parts ---------------- */
  function sideHTML(v) {
    const cur = v.page.startsWith('goal:') ? 'goals' : v.page;
    return `<nav class="side" aria-label="Pages">
      <button class="sfind" data-act="search" data-tip="Search" data-tk="⌘K">${IC.search}<span class="grow">Search</span><span class="kbd">⌘K</span></button>
      <div class="nav">${PAGES.map(p => `<button class="nv" data-act="go:${p[0]}" ${cur === p[0] ? 'aria-current="page"' : ''} data-tip="${p[1]}" data-tk="${p[2]}">${p[1]}<span class="n${p[0] === 'needs' ? ' hot' : ''}">${p[3]}</span></button>`).join('')}</div>
      <div><p class="side-h eyebrow">Active goals</p>${GOALS.map(g => `<button class="gl" data-act="goal:${g.r}" style="width:100%"><span class="gl-t"><span class="gl-r">${g.r}</span><span>${g.t}</span></span><span class="gl-m">${g.side}</span></button>`).join('')}</div>
      <div class="side-f">
        <span class="pill needs" data-tip="Starting, messaging and showing terminals wait until it’s back.">Midna isn’t running</span>
        <span class="foot-t">No work hours</span>
        <div class="row2"><span class="chip">Sample board</span><button class="icon-btn" data-act="settings" aria-label="Settings" data-tip="Settings" data-tk="⌘,">${IC.gear}</button></div>
      </div>
    </nav>`;
  }
  const trow = (it, cur) => `<button class="trow" data-open="T:${it.r}" ${cur === it.r ? 'aria-current="true"' : ''}><span class="pill ${it.s}">${it.sl}</span><span class="ref">${it.r}</span><span class="tt">${esc(it.t)}</span><span class="when">${it.at || ''}</span></button>`;
  function pageHTML(v) {
    const pg = v.page;
    if (pg === 'needs') return `<div class="main-in">
      <div class="ph"><h1 class="t-headline">Needs you</h1></div>
      <div class="stats"><div class="stat"><b>5h 36m</b><span>Agent time today</span></div><div class="stat"><b>3</b><span>Done today</span></div><div class="stat"><b>0</b><span>PRs opened</span></div><div class="stat warn"><b>36m</b><span>Waiting on you</span></div><div class="stat"><b>55h</b><span>Saved this week</span></div></div>
      <div class="sec"><p class="eyebrow">Answer · 1</p>
        <div class="qcard"><div class="qtop"><span class="pill needs">Question</span><span class="ref">T3</span><button class="link grow" data-open="T:T3" style="text-align:left">Settings page: manage passkeys</button><span class="gl-m">Sign-in with passkeys · 9:30 AM</span></div>
        <p class="qq">Should removing the last passkey be allowed when the account has no password?</p>
        <div class="qa"><button class="btn secondary sm" data-act="answer">Allow it</button><button class="btn secondary sm" data-act="answer">Block it</button><button class="btn secondary sm" data-open="T:T3">Talk it over</button></div></div>
      </div></div>`;
    if (pg === 'releases') return `<div class="main-in"><div class="ph"><h1 class="t-headline">Releases</h1></div><div class="empty"><span class="t-title">Releases are off</span><p class="muted">No project has releases on. Turn them on with <span class="code">tb project set &lt;name&gt; --releases on</span></p></div></div>`;
    if (pg.startsWith('goal:')) {
      const g = pg.slice(5), gl = GOALS.find(x => x.r === g), ts = TASKS.filter(t => t.gr === g);
      const n = ts.length, dn = ts.filter(t => t.s === 'done').length, wk = ts.filter(t => t.s === 'working' || t.s === 'needs').length;
      return `<div class="main-in"><div class="sec"><div class="ph"><span class="gl-r">${g}</span><span class="chip">webapp</span></div><div class="ph"><h1 class="t-headline">${gl.t}</h1><span class="pill ${gl.s}">${gl.sl}</span></div>
        <div class="prog" aria-label="${dn} of ${n} done"><i style="width:${dn / n * 100}%;background:var(--color-success)"></i><i style="width:${wk / n * 100}%;background:var(--color-primary)"></i></div><span class="gl-m">${dn} of ${n} done</span></div>
        <div class="list">${ts.map(t => trow(t, v.panel)).join('')}</div></div>`;
    }
    const lists = { projects: [['webapp', '2 goals · 6 tasks'], ['api', '1 task']], goals: GOALS.map(g => [g.r + '  ' + g.t, g.side, 'goal:' + g.r]), backlog: BACKLOG.map(b => [b.r + '  ' + b.t, b.sl]), ideas: IDEAS.map(i => [i.r + '  ' + i.t, i.sl]), days: TASKS.filter(t => t.at).map(t => [t.r + '  ' + t.t, 'Done ' + t.at]) };
    return `<div class="main-in"><div class="ph"><h1 class="t-headline">${PNAME[pg]}</h1>${pg === 'days' ? '<span class="gl-m">Today, Friday, November 14</span>' : ''}</div><div class="list">${(lists[pg] || []).map(r => `<button class="row" ${r[2] ? `data-act="${r[2]}"` : ''}><span class="t-body">${esc(r[0])}</span><span class="gl-m">${esc(r[1])}</span></button>`).join('')}</div></div>`;
  }
  function panelHTML(r) {
    const t = TASKS.find(x => x.r === r) || TASKS[0];
    const q = t.r === 'T3';
    return `<aside class="panel" aria-label="${t.r} ${esc(t.t)}">
      <div class="pn-h"><div class="grow"><span class="gl-r" style="color:var(--color-on-surface-variant)">${t.r}</span><h2 class="t-title">${esc(t.t)}</h2></div><button class="icon-btn" data-act="close-panel" aria-label="Close" data-tip="Close" data-tk="Esc">${IC.x}</button></div>
      <div class="pn-meta"><span class="pill ${t.s}">${t.sl}</span><span class="chip">webapp</span>${t.gr ? `<button class="link" data-act="goal:${t.gr}">${GNAME[t.gr]}</button>` : ''}</div>
      ${q ? `<div class="qcard"><p class="qq">Should removing the last passkey be allowed when the account has no password?</p><div class="qa"><button class="btn secondary sm" data-act="answer">Allow it</button><button class="btn secondary sm" data-act="answer">Block it</button></div></div>` : ''}
      <div class="pn-a"><button class="btn secondary sm" data-act="midna">Focus in Midna</button><button class="btn secondary sm" data-act="talk">Talk it over</button></div>
    </aside>`;
  }
  const dialogHTML = d => `<div class="dscrim" data-scrim="dlg"><div class="dialog" role="dialog" aria-modal="true" aria-label="New idea">
      <div class="dh"><h2 class="t-title">New idea</h2><button class="icon-btn" data-act="close-dialog" aria-label="Close" data-tip="Close" data-tk="Esc">${IC.x}</button></div>
      <label class="field live"><input data-f="title" placeholder="Title" value="${esc(d.title || '')}"></label>
      <label class="field live area"><textarea data-f="detail" placeholder="Details">${esc(d.detail || '')}</textarea></label>
      <div class="actions"><button class="btn secondary sm">Choose a project</button><button class="btn primary" data-act="add-idea">Add idea</button></div>
    </div></div>`;
  const toastHTML = (t, i) => `<div class="toast${t.k === 'error' ? ' error' : ''}" role="${t.k === 'error' ? 'alert' : 'status'}" data-ti="${i}"><span class="ti">${t.k === 'error' ? IC.alert : IC.check}</span><span class="tt">${esc(t.t)}</span>${t.a ? `<button class="ta" data-act="${t.act || ''}" data-toast="${i}">${t.a}</button>${t.key ? `<span class="tk">${t.key}</span>` : ''}` : ''}<button class="tx" data-act="close-toast" data-toast="${i}" aria-label="Close">${IC.x}</button></div>`;
  function winHTML(v, bar) {
    return `<div class="win" ${bar ? 'data-bar' : ''}>
      <div class="mbar" role="menubar" aria-label="Menu bar">${menusFor(v).map(([id, n]) => `<button class="mb${id === 'app' ? ' app' : ''}" role="menuitem" aria-haspopup="menu" aria-expanded="${v.menu === id}" data-mb="${id}">${n}</button>`).join('')}</div>
      <div class="wbody">${sideHTML(v)}<main class="main">${pageHTML(v)}</main>${v.panel ? panelHTML(v.panel) : ''}${v.pal ? palHTML(v.pal) : ''}${v.dialog ? dialogHTML(v.dialog) : ''}<div class="wtoasts">${v.toasts.map(toastHTML).join('')}</div></div>
      <div class="wlayer"></div></div>`;
  }
  function placeMenu(win, v) {
    const layer = $('.wlayer', win); layer.innerHTML = '';
    if (!v.menu) return null;
    const mb = $(`[data-mb="${v.menu}"]`, win); if (!mb) return null;
    const entry = menusFor(v).find(m => m[0] === v.menu);
    layer.insertAdjacentHTML('beforeend', menuHTML(entry[2]));
    const m = layer.lastElementChild; m.classList.add('wmenu');
    m.style.left = Math.min(mb.offsetLeft, win.clientWidth - m.offsetWidth - 8) + 'px';
    m.dataset.menu = v.menu;
    return m;
  }

  /* ---------------- Minis in the doc ---------------- */
  const SIZES = { l: [2560, 1440, 'Largest'], s: [900, 560, 'Smallest'] };
  const CAP = { l: [1728, 1084], s: [900, 560] };
  const ro = new ResizeObserver(es => es.forEach(e => fit(e.target)));
  function fit(box) {
    const w = +box.dataset.w, h = +box.dataset.h, win = box.firstElementChild;
    win.style.transform = `scale(${box.clientWidth / w})`;
  }
  function mini(st, size, bar) {
    const [w, h, lab] = SIZES[size];
    const box = document.createElement('div');
    box.className = 'mini'; box.tabIndex = 0; box.setAttribute('role', 'button');
    box.dataset.w = w; box.dataset.h = h; box.dataset.st = st; box.dataset.size = size;
    box.style.aspectRatio = w + ' / ' + h;
    const label = stateLabel(st) + ', ' + lab.toLowerCase() + ' window, ' + w + ' × ' + h;
    box.setAttribute('aria-label', 'Open in the app: ' + label);
    const v = viewFor(st);
    box.innerHTML = winHTML(v, bar);
    const win = box.firstElementChild; win.style.width = w + 'px'; win.style.height = h + 'px';
    win.setAttribute('inert', '');
    requestAnimationFrame(() => { placeMenu(win, v); fit(box); });
    ro.observe(box);
    return box;
  }
  const STATE_LABELS = {}; (canvas.states || []).forEach?.(s => STATE_LABELS[s.id] = s.label);
  function stateLabel(st) { return STATE_LABELS[st] || { 'default': 'Needs you', menu: 'View menu open', 'search-empty': 'Search, nothing typed', 'search-typed': 'Search, typed', 'search-all': 'Search, all 8 tasks', 'search-goal': 'Search inside a goal', 'search-ref': 'Search, typed a goal ref', 'search-none': 'Search, nothing found', 'search-wait': 'Search, still looking', 'search-down': 'Search, board down', link: 'Link over a dialog', 'link-bad': 'Link to a missing task', midna: 'Midna isn’t running', 'releases-off': 'Releases are off', undo: 'With Undo', 'no-undo': 'Without Undo' }[st] || st; }
  const fig = (inner, cap) => `<figure class="fig">${inner}<figcaption class="fig-c">${cap}</figcaption></figure>`;

  $$('.cmpx').forEach(c => {
    const cap = c.dataset.cap, st = c.dataset.st, what = c.dataset.what;
    let html = '<div class="cmp"><div class="cmp-h"><span></span><span>Today</span><span>With fixes</span></div>';
    ['l', 's'].forEach(sz => {
      const [cw, ch] = CAP[sz], [w, h, lab] = SIZES[sz];
      const old = `<img class="shot lt" loading="lazy" src="shots/${cap}--light-${sz}.webp" width="${cw}" height="${ch}" alt="Today: ${esc(what)}, light, ${cw} × ${ch}"><img class="shot dk" loading="lazy" src="shots/${cap}--dark-${sz}.webp" width="${cw}" height="${ch}" alt="Today: ${esc(what)}, dark, ${cw} × ${ch}">` +
        (c.dataset.oldToast ? `<div class="today-c" style="margin-top:var(--space-inset-xs)"><div class="o-toast">${esc(c.dataset.oldToast)}</div></div>` : '');
      html += `<div class="cmp-r" data-row="${sz}"><span class="cmp-l">${lab} window</span>${fig(old, 'Today · ' + cw + ' × ' + ch + (c.dataset.oldToast ? ' · toast after Open terminal' : ''))}${fig('<div class="slot"></div>', 'With fixes · ' + w + ' × ' + h)}</div>`;
    });
    c.innerHTML = html + '</div>';
    $$('.cmp-r', c).forEach(r => $('.slot', r).replaceWith(mini(st, r.dataset.row, false)));
  });
  $$('[data-minis]').forEach(c => {
    const bar = c.hasAttribute('data-bar');
    c.dataset.minis.split(' ').forEach(st => ['l', 's'].forEach(sz => {
      const f = document.createElement('figure'); f.className = 'fig';
      f.appendChild(mini(st, sz, bar));
      f.insertAdjacentHTML('beforeend', `<figcaption class="fig-c">${stateLabel(st)} · ${SIZES[sz][0]} × ${SIZES[sz][1]}</figcaption>`);
      c.appendChild(f);
    }));
  });
  R.addEventListener('click', e => { const m = e.target.closest('.mini'); if (m) openMini(m); });
  R.addEventListener('keydown', e => { const m = e.target.closest && e.target.closest('.mini'); if (m && (e.key === 'Enter' || e.key === ' ')) { e.preventDefault(); openMini(m); } });
  function openMini(m) {
    canvas.usePreset(m.dataset.size === 'l' ? 'largest' : 'smallest');
    canvas.setState(m.dataset.st);
    canvas.navigate('app');
  }

  /* ---------------- Contents list ---------------- */
  // The canvas may be zoomed: rects are in screen px, layout in CSS px.
  const zk = el => (el.getBoundingClientRect().width / el.offsetWidth) || 1;
  const docEl = $('[data-screen="doc"]');
  function goTo(id) { const t = $('#' + id); if (!t) return; docEl.scrollTop += (t.getBoundingClientRect().top - docEl.getBoundingClientRect().top) / zk(docEl) - (docEl.clientWidth >= 840 ? 24 : 72); const h = $('h2', t) || t; h.setAttribute('tabindex', '-1'); h.focus({ preventScroll: true }); }
  $$('.toc-i').forEach(b => b.addEventListener('click', () => goTo(b.dataset.go)));
  function markCurrent() {
    const top = docEl.getBoundingClientRect().top + 80; let cur = null;
    $$('.toc-i').forEach(b => { const t = $('#' + b.dataset.go); if (t && t.getBoundingClientRect().top <= top) cur = b; });
    if (!cur) cur = $('.toc-i');
    if (docEl.scrollTop + docEl.clientHeight >= docEl.scrollHeight - 4) cur = $$('.toc-i').pop();
    $$('.toc-i').forEach(b => b.setAttribute('aria-current', b === cur ? 'true' : 'false'));
  }
  docEl.addEventListener('scroll', markCurrent, { passive: true }); markCurrent();
  // Narrow: contents as a menu
  const tocBtn = $('[data-menu-toc]');
  tocBtn.addEventListener('click', () => {
    const lay = $('#doclayer');
    if (lay.firstChild) { lay.innerHTML = ''; return; }
    lay.innerHTML = menuHTML($$('.toc-i').map(b => ({ l: b.textContent, act: 'toc:' + b.dataset.go })));
    const m = lay.firstElementChild; const r = tocBtn.getBoundingClientRect(), d = docEl.getBoundingClientRect();
    const k = zk(docEl); m.style.top = ((r.bottom - d.top) / k + docEl.scrollTop + 4) + 'px'; m.style.left = Math.max(8, (r.right - d.left) / k - m.offsetWidth) + 'px';
    menuKeys(m, it => { lay.innerHTML = ''; goTo(it.dataset.act.slice(4)); tocBtn.focus(); }, () => { lay.innerHTML = ''; tocBtn.focus(); });
    canvas.refresh();
  });

  // Checklist ticks: a per-viewer convenience only.
  const boxes = $$('[data-checks] input');
  const LS = 'menus-keys-checks';
  try { const saved = JSON.parse(localStorage.getItem(LS) || '{}'); boxes.forEach(b => b.checked = !!saved[b.id]); } catch (e) {}
  function pass() { const n = boxes.filter(b => b.checked).length; $('#passf').style.width = (n / boxes.length * 100) + '%'; $('#passn').textContent = `${n} of ${boxes.length} checks pass`; }
  boxes.forEach(b => b.addEventListener('change', () => { pass(); try { localStorage.setItem(LS, JSON.stringify(Object.fromEntries(boxes.map(x => [x.id, x.checked])))); } catch (e) {} }));
  pass();

  /* ---------------- Menu keyboard (shared) ---------------- */
  function menuKeys(m, onPick, onClose, onSide) {
    const items = () => $$('.mi:not(.dis)', m);
    let i = -1;
    const set = n => { const it = items(); if (!it.length) return; i = (n + it.length) % it.length; it.forEach((x, j) => x.classList.toggle('is-active', j === i)); it[i].focus({ preventScroll: true }); };
    m.addEventListener('pointermove', e => { const it = e.target.closest('.mi'); if (it && !it.classList.contains('dis')) set(items().indexOf(it)); });
    m.addEventListener('click', e => { const it = e.target.closest('.mi'); if (it && !it.classList.contains('dis')) onPick(it); });
    m.addEventListener('keydown', e => {
      if (e.key === 'ArrowDown') { e.preventDefault(); set(i + 1); }
      else if (e.key === 'ArrowUp') { e.preventDefault(); set(i - 1); }
      else if (e.key === 'Home') { e.preventDefault(); set(0); }
      else if (e.key === 'End') { e.preventDefault(); set(items().length - 1); }
      else if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); if (i >= 0) onPick(items()[i]); }
      else if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); onClose(); }
      else if (e.key === 'Tab') { e.preventDefault(); onClose(); }
      else if (onSide && (e.key === 'ArrowLeft' || e.key === 'ArrowRight')) { e.preventDefault(); onSide(e.key === 'ArrowLeft' ? -1 : 1); }
      else if (e.key.length === 1) { const it = items(); const j = it.findIndex((x, k) => k > i && x.textContent.trim().toLowerCase().startsWith(e.key.toLowerCase())); const j2 = j >= 0 ? j : it.findIndex(x => x.textContent.trim().toLowerCase().startsWith(e.key.toLowerCase())); if (j2 >= 0) set(j2); }
    });
    m.tabIndex = -1; m.focus({ preventScroll: true });
    return set;
  }

  /* ---------------- The live app ---------------- */
  const host = $('#appwin');
  let A = viewFor('default');
  let menuFromKey = false;
  function render(keepFocus) {
    const focusPal = A.pal && keepFocus !== false;
    host.innerHTML = winHTML(A, true);
    const win = host.firstElementChild;
    const m = placeMenu(win, A);
    if (m) {
      const set = menuKeys(m, it => { const act = it.dataset.act; A.menu = null; render(); if (act) run(act); }, () => { const id = A.menu; A.menu = null; render(); $(`[data-mb="${id}"]`)?.focus(); }, d => { const ids = menusFor(A).map(x => x[0]); A.menu = ids[(ids.indexOf(A.menu) + d + ids.length) % ids.length]; menuFromKey = true; render(); });
      if (menuFromKey) set(0); menuFromKey = false;
    }
    if (focusPal) { const inp = $('.pal-in input', win); if (inp) { inp.focus({ preventScroll: true }); inp.setSelectionRange(inp.value.length, inp.value.length); } }
    if (A.dialog) $('.dialog input', win)?.focus();
    canvas.refresh();
  }
  function refreshList() { const l = $('.pal-list', host); if (l) { l.innerHTML = palListHTML(A.pal); } const sel = $('.pr[aria-selected="true"]', host); if (sel && l) { const a = sel.offsetTop - 40, b = sel.offsetTop + sel.offsetHeight - l.clientHeight; if (l.scrollTop > a) l.scrollTop = a; else if (l.scrollTop < b) l.scrollTop = b; } }
  function go(page) {
    if (A.page !== page) { if (page !== 'releases') A.back.push(A.page); A.fwd = []; A.page = page; }
    A.pal = null; A.menu = null; render();
  }
  let timers = [];
  function toast(k, t, opt) {
    opt = opt || {};
    A.toasts = A.toasts.filter(x => x.k !== 'undo' || k !== 'undo').slice(-2);
    const item = { k, t, a: opt.a, key: opt.key, act: opt.act, id: Math.random() };
    A.toasts.push(item); render(false);
    if (k !== 'error') { const ms = k === 'undo' ? 8000 : 4000; const tm = setTimeout(() => { if ($('.wtoasts:hover', host) || $('.wtoasts:focus-within', host)) return; A.toasts = A.toasts.filter(x => x !== item); render(false); }, ms); timers.push(tm); }
  }
  function run(act) {
    if (!act) return;
    if (act.startsWith('go:')) return go(act.slice(3));
    if (act.startsWith('goal:')) return go('goal:' + act.slice(5));
    if (act.startsWith('all:')) { A.pal.all = act.slice(4); refreshList(); return; }
    switch (act) {
      case 'search': A.menu = null; A.pal = { q: '', scope: A.page.startsWith('goal:') ? A.page.slice(5) : null }; return render();
      case 'unscope': A.pal.scope = null; return render();
      case 'retry': A.pal.down = false; A.pal.wait = true; render(); setTimeout(() => { if (A.pal) { A.pal.wait = false; render(); } }, 900); return;
      case 'new-idea': A.menu = null; A.pal = null; A.dialog = A.draft || {}; A.toasts = A.toasts.filter(t => t.act !== 'new-idea'); return render();
      case 'close-dialog': A.draft = A.dialog; A.dialog = false; return render();
      case 'add-idea': A.dialog = false; A.draft = null; render(); return toast('done', 'Idea added');
      case 'close-panel': A.panel = null; return render();
      case 'back': if (A.back.length) { A.fwd.push(A.page); A.page = A.back.pop(); render(); } return;
      case 'fwd': if (A.fwd.length) { A.back.push(A.page); A.page = A.fwd.pop(); render(); } return;
      case 'refresh': return toast('done', 'Up to date');
      case 'settings': return toast('done', 'Settings opens in its own window');
      case 'copy-ref': if (A.panel) toast('done', 'Copied ' + A.panel); return;
      case 'midna': return toast('error', `Midna isn’t running, so it can’t show ${A.panel || 'T3'}’s terminal.`);
      case 'undo': if (A.undoLabel) { A.undoLabel = null; A.toasts = []; render(); toast('done', 'T6 is open again'); } return;
      case 'answer': return toast('done', 'Answered T3');
      case 'talk': return toast('done', 'Opening a talk about T3');
    }
  }
  function open(spec) {
    const [k, v] = spec.split(':');
    A.pal = null;
    if (k === 'T') { A.panel = v; render(); $('.panel .icon-btn', host)?.focus(); return; }
    if (k === 'G') return go('goal:' + v);
    if (k === 'P') return go(v);
  }
  host.addEventListener('click', e => {
    const mb = e.target.closest('[data-mb]');
    if (mb) { A.menu = A.menu === mb.dataset.mb ? null : mb.dataset.mb; return render(false); }
    if (A.menu && !e.target.closest('.wmenu')) { A.menu = null; render(false); }
    const tx = e.target.closest('[data-act="close-toast"]');
    if (tx) { A.toasts.splice(+tx.dataset.toast, 1); return render(false); }
    const a = e.target.closest('[data-act]');
    if (a && !a.closest('.wmenu')) { if (a.dataset.toast != null) A.toasts.splice(+a.dataset.toast, 1); return run(a.dataset.act); }
    const o = e.target.closest('[data-open]');
    if (o) return open(o.dataset.open);
    const sc = e.target.closest('[data-scrim]');
    if (sc && e.target === sc) { if (sc.dataset.scrim === 'pal') A.pal = null; else { A.draft = A.dialog; A.dialog = false; } render(); }
  });
  host.addEventListener('pointerover', e => {
    const mb = e.target.closest('[data-mb]');
    if (mb && A.menu && A.menu !== mb.dataset.mb) { A.menu = mb.dataset.mb; render(false); }
  });
  host.addEventListener('pointermove', e => {
    const pr = e.target.closest('.pr'); if (!pr || !A.pal || pr.getAttribute('aria-disabled')) return;
    if (A.pal.sel !== pr.dataset.key) { A.pal.sel = pr.dataset.key; $$('.pr[aria-selected]', host).forEach(x => x.removeAttribute('aria-selected')); pr.setAttribute('aria-selected', 'true'); }
  });
  host.addEventListener('input', e => {
    if (e.target.matches('.pal-in input')) { A.pal.q = e.target.value; A.pal.sel = null; A.pal.all = null; refreshList(); }
    if (e.target.dataset.f && A.dialog) A.dialog[e.target.dataset.f] = e.target.value;
  });
  host.addEventListener('contextmenu', e => {
    const row = e.target.closest('.trow, .pr[data-open^="T:"]'); if (!row) return;
    e.preventDefault(); ctxMenu(row, e.clientX, e.clientY);
  });
  host.addEventListener('keydown', e => {
    if (e.shiftKey && e.key === 'F10') { const row = e.target.closest('.trow'); if (row) { e.preventDefault(); const r = row.getBoundingClientRect(); ctxMenu(row, r.left + 24, r.bottom); } }
  });
  function ctxMenu(row, x, y) {
    const lay = $('.wlayer', host); const ref = row.dataset.open.slice(2);
    lay.innerHTML = menuHTML([{ l: 'Open', act: 'ctx-open' }, { l: 'Open Pull Request', k: '⌘↩', disabled: true }, '-', { l: 'Copy Reference', k: '⇧⌘C', act: 'ctx-copy' }, { l: 'Focus in Midna', act: 'midna' }]);
    const m = lay.firstElementChild; m.classList.add('wmenu'); const wr = host.getBoundingClientRect(), k = zk(host);
    m.style.top = Math.min((y - wr.top) / k, host.offsetHeight - m.offsetHeight - 8) + 'px'; m.style.left = Math.min((x - wr.left) / k, host.offsetWidth - m.offsetWidth - 8) + 'px';
    menuKeys(m, it => { lay.innerHTML = ''; const a = it.dataset.act; if (a === 'ctx-open') open(row.dataset.open); else if (a === 'ctx-copy') toast('done', 'Copied ' + ref); else run(a); }, () => { lay.innerHTML = ''; row.focus(); });
    canvas.refresh();
  }

  // Keys for the app window
  const PAGE_KEYS = { '1': 'needs', '2': 'projects', '3': 'goals', '4': 'backlog', '5': 'ideas', '6': 'days', '7': 'releases' };
  document.addEventListener('keydown', e => {
    if (canvas.screen !== 'app') return;
    const cmd = e.metaKey || e.ctrlKey, k = e.key;
    const inField = e.composedPath()[0]?.matches?.('input, textarea');
    if (k === 'Escape') {
      if ($('.wlayer .menu', host)) { $('.wlayer', host).innerHTML = ''; if (A.menu) { A.menu = null; render(); } }
      else if (A.pal) { A.pal = null; render(); }
      else if (A.dialog) { A.draft = A.dialog; A.dialog = false; render(); }
      else if (A.panel) { A.panel = null; render(); }
      else return;
      e.preventDefault(); return;
    }
    if (A.pal && !cmd) {
      const keys = A.pal.keys || [];
      if (k === 'ArrowDown' || k === 'ArrowUp') { e.preventDefault(); const i = keys.indexOf(A.pal.sel); A.pal.sel = keys[(i + (k === 'ArrowDown' ? 1 : -1) + keys.length) % keys.length]; refreshList(); return; }
      if (k === 'Enter') { e.preventDefault(); const s = $('.pr[aria-selected="true"]', host); if (s) s.click(); return; }
    }
    if (!cmd) return;
    if (A.dialog && /^[1-7\[\]]$/.test(k)) return;
    let act = null;
    if (PAGE_KEYS[k] && !e.shiftKey) act = 'go:' + PAGE_KEYS[k];
    else if (k === 'k' || k === 'K') act = 'search';
    else if (k === 'n' || k === 'N') act = 'new-idea';
    else if (k === 'r' || k === 'R') act = 'refresh';
    else if (k === ',') act = 'settings';
    else if (k === '[' || (k === 'ArrowLeft' && !inField)) act = 'back';
    else if (k === ']' || (k === 'ArrowRight' && !inField)) act = 'fwd';
    else if ((k === 'c' || k === 'C') && e.shiftKey) act = 'copy-ref';
    else if ((k === 'z' || k === 'Z') && !e.shiftKey && !inField) act = 'undo';
    if (act) { e.preventDefault(); run(act); }
  }, true);

  // Tips: 500ms on hover, at once on keyboard focus
  let tipT = null;
  const hideTip = () => { clearTimeout(tipT); $$('.wtip', host).forEach(t => t.remove()); };
  function showTip(el) {
    hideTip(); const win = host.firstElementChild; if (!win) return;
    const t = document.createElement('div'); t.className = 'tip wtip'; t.setAttribute('role', 'tooltip');
    t.innerHTML = esc(el.dataset.tip) + (el.dataset.tk ? ` <span class="kbd">${el.dataset.tk}</span>` : '');
    win.appendChild(t);
    const r = el.getBoundingClientRect(), w = win.getBoundingClientRect(), k = zk(win);
    let x = (r.left - w.left + r.width / 2) / k - t.offsetWidth / 2; x = Math.max(8, Math.min(x, win.offsetWidth - t.offsetWidth - 8));
    let y = (r.bottom - w.top) / k + 4; if (y + t.offsetHeight > win.offsetHeight - 8) y = (r.top - w.top) / k - t.offsetHeight - 4;
    t.style.left = x + 'px'; t.style.top = y + 'px';
  }
  host.addEventListener('pointerover', e => { const el = e.target.closest('[data-tip]'); if (!el) return; clearTimeout(tipT); tipT = setTimeout(() => showTip(el), 500); });
  host.addEventListener('pointerout', e => { if (e.target.closest('[data-tip]')) hideTip(); });
  host.addEventListener('focusin', e => { const el = e.target.closest('[data-tip]'); if (el && el.matches(':focus-visible')) showTip(el); else hideTip(); });
  host.addEventListener('keydown', e => { if (e.key === 'Escape' && $('.wtip', host)) { hideTip(); } }, true);

  canvas.onState(st => {
    timers.forEach(clearTimeout); timers = [];
    A = viewFor(st);
    if (canvas.screen === 'app') render(false); else host.innerHTML = '';
  });
  canvas.onNavigate(({ screen }) => { if (screen === 'app') render(false); });
};
