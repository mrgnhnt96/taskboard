window.ultrahandSetup = function (canvas) {
  const R = canvas.root;
  const q = (s, el) => (el || R).querySelector(s);
  const qa = (s, el) => [...(el || R).querySelectorAll(s)];

  const I = {
    x: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8"/></svg>',
    chev: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M4 6l4 4 4-4"/></svg>',
    more: '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="3.5" cy="8" r="1.1" fill="currentColor" stroke="none"/><circle cx="8" cy="8" r="1.1" fill="currentColor" stroke="none"/><circle cx="12.5" cy="8" r="1.1" fill="currentColor" stroke="none"/></svg>',
    clock: '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6.25"/><path d="M8 4.75V8l2.25 1.5"/></svg>',
    alert: '<svg viewBox="0 0 16 16" aria-hidden="true"><circle cx="8" cy="8" r="6.25"/><path d="M8 4.75v4M8 11.25v.01"/></svg>',
    check: '<svg viewBox="0 0 16 16" aria-hidden="true"><path d="M3.5 8.5l3 3 6-7"/></svg>'
  };

  /* ---------- what each state shows ---------- */
  const PLAN = 'Read the 5:00 PM benchmark run, then compare it with the numbers from before the change.';
  const PLAN_AM = 'Read the overnight benchmark run, then compare it with the numbers from before the change.';
  const S = {
    waiting: { at: '6:30 PM', plan: PLAN, last: 'Nothing more can be checked before the 5:00 PM benchmark run.' },
    tomorrow: { at: 'tomorrow 6:30 AM', plan: PLAN_AM, last: 'Nothing more can be checked before the overnight benchmark run.' },
    overdue: { at: '6:30 PM', plan: PLAN, last: 'Nothing more can be checked before the 5:00 PM benchmark run.' },
    resumed: { at: '6:30 PM', plan: PLAN, last: 'Reading the 5:00 PM benchmark run.' }
  };
  const st = () => S[canvas.state] ? canvas.state : 'waiting';
  const parked = () => st() === 'waiting' || st() === 'tomorrow';

  /* The waiting task's row: Waiting right after Working; when it misses its time it is a Needs you row. */
  function t10() {
    const s = S[st()];
    if (parked()) return { ref: 'T10', title: 'Re-measure and compare', line: `<span class="at">Resumes ${s.at}</span> · ${s.plan}`, chip: ['waiting', 'Waiting'], rank: 2.5 };
    if (st() === 'overdue') return { ref: 'T10', title: 'Re-measure and compare', line: `<span class="at">Didn't resume at ${s.at}</span> · ${s.plan}`, loud: true, chip: ['needs', 'Needs you'], rank: 0 };
    return { ref: 'T10', title: 'Re-measure and compare', line: s.last.replace(/\.$/, '') + ' · 1m', chip: ['working', 'Working'], rank: 2 };
  }
  function g2rows() {
    return [
      { ref: 'T6', title: 'Measure the sign-in path', line: '412 ms median before the change', chip: ['done', 'Done'], rank: 8 },
      { ref: 'T7', title: 'Cache the user lookup', line: 'PR #951 merged', chip: ['done', 'Done'], rank: 8.1 },
      { ref: 'T8', title: 'Batch the session writes', line: 'Moving the writes behind one flush · 1h 5m', chip: ['working', 'Working'], rank: 2 },
      t10(),
      { ref: 'T9', title: 'Drop the extra redirect', line: 'Waits for a free terminal', chip: ['queued', 'Queued'], rank: 6 },
      { ref: 'T11', title: 'Write up the numbers', line: '', chip: ['planned2', 'Planned'], rank: 7 }
    ].sort((a, b) => a.rank - b.rank);
  }
  const g2bar = () => {
    const w = { waiting: 'waiting', tomorrow: 'waiting', overdue: 'needs', resumed: 'working' }[st()];
    return [['done', 2], ['working', 1], [w, 1], ['queued', 1], ['planned', 1]];
  };
  const bar = (parts) => `<span class="sbar" aria-hidden="true">${parts.map(([c, n]) => `<i class="${c}" style="flex:${n}"></i>`).join('')}</span>`;
  const chip = ([c, l]) => `<span class="pill st ${c}">${l}</span>`;

  /* ---------- sidebar ---------- */
  function side(active) {
    const needsN = st() === 'overdue' ? 2 : 1;
    const item = (nav, label, color, n, loud) =>
      `<button class="side-i${active === nav ? ' on' : ''}" ${nav ? `data-nav="${nav}"` : 'data-say="Not part of this design"'}${active === nav ? ' aria-current="page"' : ''}><span class="dotc" style="background:${color}"></span><span class="nm">${label}</span>${n ? `<span class="n${loud ? ' loud' : ''}">${n}</span>` : ''}</button>`;
    const g2sub = st() === 'overdue' ? `<span class="loud">2 of 6 · Didn't resume</span>` : '<span>2 of 6</span>';
    return `<nav class="side" aria-label="Pages">
      ${item('needs', 'Needs you', 'var(--color-warning)', needsN, true)}
      ${item('project', 'Projects', 'var(--color-on-surface-variant)', 2)}
      ${item('', 'Goals', 'var(--color-goal)', 2)}
      ${item('', 'Backlog', 'var(--color-outline)', 2)}
      ${item('', 'Ideas', 'var(--color-goal)', 0)}
      ${item('', 'Days', 'var(--color-info)', 0)}
      <h2 class="over side-h">Active goals</h2>
      <div class="sgoal"><span class="nm"><span class="ref">G1</span>Sign-in with passkeys</span>${bar([['done', 1], ['working', 1], ['needs', 1], ['planned', 1]])}<span class="sub"><span class="loud">1 of 4 · Needs your answer</span></span></div>
      <div class="sgoal"><span class="nm"><span class="ref">G2</span>Faster sign-in</span>${bar(g2bar())}<span class="sub">${g2sub}</span></div>
    </nav>`;
  }

  /* ---------- Project page ---------- */
  function project(openRef) {
    const rows = g2rows().map((r) => `<button class="trow" data-open="${r.ref}" title="${r.line.replace(/<[^>]+>/g, '')}"${openRef === r.ref ? ' aria-current="true"' : ''}><span class="tref">${r.ref}</span><span class="ttitle">${r.title}</span><span class="tline${r.loud ? ' loud' : ''}">${r.line}</span>${chip(r.chip)}</button>`).join('');
    const g2chip = st() === 'overdue' ? ['needs', 'Needs you'] : ['working', 'Working'];
    return `<main class="page" aria-labelledby="ph"><div class="page-in">
      <h1 class="t-headline" id="ph">webapp</h1>
      <div class="grp2">
        <div class="gblock open">
          <button class="ghead" aria-expanded="true" data-act="fold"><span class="chev">${I.chev}</span><span class="ref">G2</span><span class="gname">Faster sign-in</span><span class="gprog">${bar(g2bar())}<span>2 of 6 · wave 2</span></span>${chip(g2chip)}</button>
          <div class="trows">${rows}</div>
        </div>
        <div class="gblock">
          <button class="ghead" aria-expanded="false" data-act="fold"><span class="chev">${I.chev}</span><span class="ref">G1</span><span class="gname">Sign-in with passkeys</span><span class="gprog">${bar([['done', 1], ['working', 1], ['needs', 1], ['planned', 1]])}<span>1 of 4</span></span>${chip(['needs', 'Needs you'])}</button>
          <div class="trows" hidden>
            <button class="trow" data-say="Not part of this design"><span class="tref">T3</span><span class="ttitle">Settings page: manage passkeys</span><span class="tline loud">Question: Should removing the last passkey require a password re-check?</span>${chip(['needs', 'Needs you'])}</button>
            <button class="trow" data-say="Not part of this design"><span class="tref">T2</span><span class="ttitle">Sign in with a passkey</span><span class="tline">Wiring the assertion check into the session middleware</span>${chip(['working', 'Working'])}</button>
            <button class="trow" data-say="Not part of this design"><span class="tref">T4</span><span class="ttitle">Docs: passkey support</span><span class="tline"></span>${chip(['planned2', 'Planned'])}</button>
            <button class="trow" data-say="Not part of this design"><span class="tref">T1</span><span class="ttitle">Add the WebAuthn registration endpoint</span><span class="tline">Registration endpoint and tests are in</span>${chip(['done', 'Done'])}</button>
          </div>
        </div>
      </div>
    </div></main>`;
  }

  /* ---------- Needs you page ---------- */
  function needs() {
    const over = st() === 'overdue';
    const n = over ? 2 : 1;
    const acts = `<div class="nyc-acts"><button class="btn secondary sm" data-say="Opens its terminal in Midna">Open terminal</button><button class="btn secondary sm" data-say="Later menu, as today">Later <span class="chev-d">${I.chev}</span></button><button class="btn secondary sm" data-say="More menu, as today">More <span class="chev-d">${I.chev}</span></button></div>`;
    const card = (goal, ref, title, q, plan, age) => `<article class="nyc" aria-label="${title}">
      <div class="nyc-top"><div class="nyc-t"><span class="nyc-g">${goal}</span><button class="nyc-title" data-open="${ref}">${title}</button></div>${acts}<span class="age">${age}</span></div>
      <p class="nyc-q">${q}</p>${plan ? `<p class="nyc-plan">${plan}</p>` : ''}
    </article>`;
    const late = over ? card('Faster sign-in', 'T10', 'Re-measure and compare', `Didn't resume at ${S.overdue.at}`, S.overdue.plan, '22m') : '';
    return `<main class="page" aria-labelledby="ph"><div class="page-in">
      <div class="ph1"><h1 class="t-headline" id="ph">Needs you</h1><span class="muted">${n} agent${n > 1 ? 's' : ''} waiting</span></div>
      <div class="grp2" role="group" aria-labelledby="gw">
        <h2 class="over" id="gw">Agents waiting<span class="n">${n}</span></h2>
        ${late}
        ${card('Sign-in with passkeys', 'T3', 'Settings page: manage passkeys', 'Should removing the last passkey require a password re-check?', '', '4m')}
      </div>
    </div></main>`;
  }

  /* ---------- Task panel ---------- */
  const stop = (dot, inner) => `<li class="stop"><span class="dot ${dot}">${dot === 'ok' ? I.check : ''}</span>${inner}</li>`;
  const nowcard = (head, cap, text, extra) => `<div class="nowcard"><div class="nh">${head}<span class="cap">${cap}</span><span class="grow"></span><button class="icon-btn" aria-label="More" data-say="More menu, as today">${I.more}</button></div><p>${text}</p>${extra || ''}<p class="then">Then: Pull request, Checks, Review, Merge</p></div>`;
  function panel(ref) {
    if (ref !== 'T10') return '';
    const s = S[st()];
    let box = '', now = '', past = '';
    if (parked()) {
      box = `<section class="box wait" aria-labelledby="bh"><h3 class="bh" id="bh">${I.clock}Waiting · resumes ${s.at}</h3><p class="bm">${s.plan}</p></section>`;
      now = stop('waitd', nowcard(chip(['waiting', 'Waiting']), 'after 52m of work · since 1:48 PM', s.last));
    } else if (st() === 'overdue') {
      box = `<section class="box needs" aria-labelledby="bh"><h3 class="bh" id="bh">${I.alert}Didn't resume at ${s.at}</h3><p class="bm">${s.plan}</p><div class="acts"><button class="btn primary" data-say="Focusing it in Midna">Focus in Midna</button></div></section>`;
      now = stop('you', nowcard(chip(['needs', 'Needs you']), 'after 52m of work · since 6:45 PM', s.last));
    } else {
      now = stop('now', nowcard(chip(['working', 'Working']), 'under a minute · updated just now', s.last, `<button class="btn secondary sm fit" data-say="Focusing it in Midna">Focus in Midna</button>`));
      past = stop('waitd', `<div class="sh"><h3>Waited until 6:30 PM</h3><span class="r">6:31 PM</span></div><p>${PLAN}</p>`);
    }
    const started = stop('ok', `<div class="sh"><h3>Started</h3><span class="muted">Baseline from the sign-in path saved</span><span class="r">12:56 PM</span></div>`);
    return `<aside class="panel" aria-labelledby="ttl">
      <header class="ph">
        <div class="crumb"><a class="goal-link" href="#" data-say="Opens the goal">G2 Faster sign-in</a><span class="sepc" aria-hidden="true">›</span><span class="t-label muted">T10</span><span class="grow"></span><button class="icon-btn" data-act="close" aria-label="Close task panel" data-tip="Close · Esc">${I.x}</button></div>
        <h2 class="t-title-lg ttl" id="ttl" tabindex="-1">Re-measure and compare</h2>
        <div class="facts"><span class="chip">webapp</span><span class="chip">Wave 2 · 4 of 6</span></div>
      </header>
      <div class="ptabs" role="tablist" aria-label="Task">
        <button class="tab" role="tab" aria-selected="true" data-label="Timeline">Timeline</button>
        <button class="tab" role="tab" aria-selected="false" tabindex="-1" data-label="Brief" data-say="Brief tab, as today">Brief</button>
        <button class="tab" role="tab" aria-selected="false" tabindex="-1" data-label="Context" data-say="Context tab, as today">Context</button>
        <button class="tab" role="tab" aria-selected="false" tabindex="-1" data-label="Full log 18" data-say="Full log, as today"><span>Full log<span class="n">18</span></span></button>
      </div>
      <div class="pbody" role="tabpanel" aria-label="Timeline" tabindex="0">${box}<ol class="tl">${now}${past}${started}</ol></div>
    </aside>`;
  }

  /* ---------- render ---------- */
  const open = { project: null, panel: 'T10', needs: null };
  let opener = null;
  function render(id, focusPanel) {
    const app = q(`[data-app="${id}"]`);
    if (!app) return;
    const page = id === 'needs' ? needs() : project(open[id]);
    app.innerHTML = side(id === 'needs' ? 'needs' : 'project') + page + panel(open[id]);
    app.classList.toggle('open', !!open[id]);
    app.classList.toggle('docked', !!open[id]);
    setInert(app);
    if (focusPanel) q('#ttl', app)?.focus();
    canvas.refresh();
  }
  /* under 1280 the panel overlays the page: keep Tab inside it */
  function setInert(app) {
    const overlay = !!open[app.dataset.app] && app.offsetWidth < 1280;
    qa('.side, .page', app).forEach((el) => { el.inert = overlay; });
  }
  const ro = new ResizeObserver((es) => es.forEach((e) => setInert(e.target)));
  qa('.app').forEach((a) => ro.observe(a));
  const renderAll = () => ['project', 'panel', 'needs'].forEach((id) => render(id));

  R.addEventListener('click', (e) => {
    const app = e.target.closest('.app');
    if (!app) return;
    const id = app.dataset.app;
    if (e.target === app && open[id]) { close(id); return; }
    const say = e.target.closest('[data-say]');
    if (say) { e.preventDefault(); canvas.toast(say.dataset.say); return; }
    const o = e.target.closest('[data-open]');
    if (o) { opener = o.dataset.open; open[id] = o.dataset.open; render(id, true); return; }
    const a = e.target.closest('[data-act]');
    if (!a) return;
    if (a.dataset.act === 'close') close(id);
    if (a.dataset.act === 'fold') {
      const blk = a.closest('.gblock');
      const on = !blk.classList.contains('open');
      blk.classList.toggle('open', on);
      a.setAttribute('aria-expanded', on);
      q('.trows', blk).hidden = !on;
      canvas.refresh();
    }
  });
  function close(id) {
    const ref = open[id];
    open[id] = null;
    render(id);
    const back = ref && q(`[data-app="${id}"] [data-open="${ref}"]`);
    if (back) back.focus();
  }
  R.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape') return;
    const app = e.target.closest && e.target.closest('.app');
    if (app && open[app.dataset.app]) { e.preventDefault(); close(app.dataset.app); }
  });

  canvas.onState(renderAll);
  canvas.onNavigate(() => renderAll());
};
