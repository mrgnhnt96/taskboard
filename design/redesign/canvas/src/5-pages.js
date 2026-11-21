// ── Pages ──
const sk = (n, w = [42, 26]) => Array.from({ length: n }, (_, i) => `<div class="sk" aria-hidden="true"><i style="width:16px;height:16px;border-radius:50%"></i><i style="width:${w[0] - (i % 3) * 6}%"></i><i style="width:${w[1] - (i % 2) * 8}%;margin-left:auto"></i></div>`).join('');
const loadingCol = (title) => `<div class="col" aria-busy="true"><div class="ph"><div class="t"><h1>${title}</h1><p class="sum">Loading…</p></div></div><ul class="rows">${sk(7)}</ul></div>`;
const grpH = (title, n, extra = '') => `<div class="grp-h"><h2>${title}</h2>${n != null ? `<span class="n">${n}</span>` : ''}<span class="grow"></span>${extra}</div>`;
const tLine = (r) => { const t = TASKS[r]; return t ? t.line : ''; };
const goalOf = (r) => { const t = TASKS[r] || ISSUES[r]; return t && t.goal ? `<a data-act="goal" data-arg="${t.goal}" data-uh-ignore-target>${t.goal} ${GOALS[t.goal].name}</a>` : 'No goal'; };

// ── Needs you ──
function pgNeeds(ds) {
  if (ds === 'loading') return loadingCol('Needs you');
  if (ds === 'empty') return `<div class="col"><div class="ph"><div class="t"><h1>Needs you</h1></div></div>
    <div class="empty">${ic('done', 'i c-done')}<b>Nothing needs you</b><span class="var">Nothing is running. You’ll hear when an agent needs a hand.</span>
    <div class="btnrow" style="margin-top:8px"><button class="btn" data-act="go" data-arg="goals">See the goals</button><button class="btn plain" data-act="pop" data-arg="hours">Work hours</button></div></div>
    ${laterBlock()}</div>`;
  const q = TASKS.T3;
  const qBody = `<div class="q">
      <span class="lbl-s">T3 asks · question 1 of 2</span>
      <p class="qt"><q>Should removing the last passkey require a password re-check?</q></p>
      ${optsHtml('T3')}
      ${answerHtml('T3')}
    </div>`;
  const rows = [];
  const grp = (title, items) => { const on = items.filter(Boolean); if (!on.length) return ''; return `<section class="grp" aria-label="${title}">${grpH(title, on.length)}<ul class="rows">${on.join('')}</ul></section>`; };
  const waiting = [
    live('T5') && row({ ref: 'T5', g: 'quiet', title: 'Profile the token refresh path', meta: ['No update for 40 minutes', 'G2 Faster sign-in'], age: '40m', main: ['Focus', 'toast', 'Focused T5’s terminal in Midna'], sec: ['Dismiss', 'gone', 'T5|Dismissed the alert for T5'] })
  ];
  const decide = [
    live('T3') && row({ ref: 'T3', g: 'needs', title: q.title, meta: ['Asks which re-check to use', 'G1 Sign-in with passkeys'], age: '9m', body: S.panel === 'T3' ? null : qBody }),
    live('T6') && row({ ref: 'T6', g: 'review', title: 'Cache session lookups', meta: ['PR #412 waits on your review', 'then Sam’s'], age: '25m', main: ['Open PR', 'toast', 'Opened PR #412 in the browser'], sec: ['I reviewed it', 'gone', 'T6|Marked PR #412 reviewed'] }),
    live('B44') && row({ ref: 'B44', g: 'failed', title: 'Review found an SQL string built from input', meta: ['Holds PR #93', 'G3 Rate limits per key'], age: '1h', main: ['Make a task', 'gone', 'B44|Made T25 from B44'], sec: ['Open', 'open', 'B44'] }),
    live('B41') && row({ ref: 'B41', g: 'needs', title: 'Reset links expire after 15 minutes wasn’t proven', meta: ['Outcome missed', 'holds G9 Password reset emails open'], age: '1d', main: ['Make a task', 'gone', 'B41|Made T24 from B41'], sec: ['Open', 'open', 'B41'] })
  ];
  const todo = [
    live('H1') && row({ ref: 'H1', g: 'human', title: TASKS.H1.title, meta: ['Your to-do', '3 steps', 'G1 Sign-in with passkeys'], age: '2h', main: ['Done', 'gone', 'H1|H1 done'], sec: ['Talk it over', 'toast', 'Opened a conversation about H1 in Midna'] }),
    live('WR') && row({ ref: 'WR', g: 'needs', title: 'Your week in review is ready', showRef: false, meta: ['Nov 14 – 20', '2 fixes suggested'], age: '3h', titleAct: ['go', 'week'], main: ['Open', 'go', 'week'], later: true })
  ];
  const g3Kids = [
    live('T7') && row({ ref: 'T7', g: 'failed', title: 'Per-key token bucket', meta: ['3 of 212 tests fail under load'], age: '34m', main: ['Try again', 'gone', 'T7|T7 queued again'], later: false }),
    live('T11') && row({ ref: 'T11', g: 'nostart', title: 'Retry-After on every 429', meta: ['Midna is at its terminal limit, 8 of 8'], age: '20m', main: ['Start in a new terminal', 'gone', 'T11|T11 starting in a new terminal'], later: false })
  ].filter(Boolean).join('');
  const stopped = [
    live('G3') && (g3Kids ? row({ ref: 'G3', x: 'b-G3', title: 'Rate limits per key', meta: ['Wave 2 waits', `${g3Kids.match(/<li class/g).length} tasks need a hand`], age: '34m', titleAct: ['goal', 'G3'], main: ['Continue anyway', 'toast', 'Wave 2 of G3 can start now|undo'], kids: g3Kids }) : ''),
    live('T10') && row({ ref: 'T10', g: 'lost', title: 'Move sessions to Redis', meta: ['Terminal lost', 'the work looks finished', 'PR #409'], age: '48m', main: ['Check and finish', 'gone', 'T10|T10 marked done'], sec: ['Resume', 'toast', 'Resuming T10 in a new terminal'] })
  ];
  const n = needsCount();
  return `<div class="col">
  <div class="ph"><div class="t"><h1>Needs you</h1><p class="sum">${n ? `${n} things wait on you` : 'All caught up'} · 4 agents working · 2 goals running</p></div>
    <div class="acts"><button class="btn" data-act="toast" data-arg="Opened a conversation about what needs you in Midna">${ic('talk')}Talk it over</button></div></div>
  ${grp('Agents waiting', waiting)}
  ${grp('Decide', decide)}
  ${grp('Your to-dos', todo)}
  ${grp('Stopped', stopped)}
  ${ideasWaiting()}
  ${laterBlock()}
</div>`;
}

function optsHtml(ref) {
  const opts = [['Yes, always', 'Ask for the password before the last passkey goes'], ['Only without a recent sign-in', 'Skip it if they signed in within 10 minutes'], ['No', 'Removing it is enough; they can still use the password']];
  return `<div class="opts" role="radiogroup" aria-label="Answers">${opts.map(([b, s], i) => `<button class="opt${S.sending === ref && S.pick[ref] === i ? ' sending' : ''}" role="radio" aria-checked="${S.pick[ref] === i}" aria-pressed="${S.pick[ref] === i}" data-act="pickopt" data-arg="${ref}|${i}"><span class="r"></span><b>${b}</b><span>${s}</span></button>`).join('')}</div>`;
}
const OPT_LABELS = { T3: ['Yes, always', 'Only without a recent sign-in', 'No'] };
function sendLabel(ref) { const i = S.pick[ref]; const l = i != null && OPT_LABELS[ref] ? OPT_LABELS[ref][i] : null; return l ? `Send “${l}”` : S.drafts[ref] ? 'Send your answer' : 'Send'; }
function answerHtml(ref) {
  return `<div class="answer"><label class="field"><span>Other answer</span><textarea class="in" rows="1" data-answer="${ref}" placeholder="">${esc(S.drafts[ref] || '')}</textarea></label><button class="btn pri${S.sending === ref ? ' sending' : ''}" data-act="send" data-arg="${ref}"><span data-send-for="${ref}">${S.sending === ref ? 'Sending' : sendLabel(ref)}</span> <span class="kbd">⌘↩</span></button></div>`;
}

function ideasWaiting() {
  const w = IDEAS.filter(i => i.wait && live(i.ref));
  if (!w.length) return '';
  return `<section class="grp" aria-label="Ideas waiting">${grpH('Ideas waiting', w.length)}<ul class="rows">${w.map(i => row({ ref: i.ref, g: 'needs', title: i.title, meta: [i.status, i.note, i.project], titleAct: ['idea', i.ref], main: i.ref === 'I4' ? ['Plan', 'toast', 'Planning I4 · a goal appears when it’s ready|undo'] : ['Answer', 'idea', i.ref], age: i.ref === 'I2' ? '4h' : '1d' })).join('')}</ul></section>`;
}

function laterBlock() {
  const sn = Object.keys(S.snoozed);
  const base = [['T15', 'Bump the session library', 'back at 3:00 PM'], ['H2', 'Confirm the SSO domain', 'waiting on Sam · back Mon, Nov 23']];
  const all = base.concat(sn.map(r => [r, (TASKS[r] || ISSUES[r] || { title: r === 'WR' ? 'Your week in review' : GOALS[r] ? GOALS[r].name : r }).title || GOALS[r].name, S.snoozed[r]]));
  const open = S.open.later;
  return `<section class="grp" aria-label="Later"><div class="grp-h"><button class="btn plain" style="padding:0 4px;margin-left:-4px" data-act="expand" data-arg="later" aria-expanded="${open}">${ic(open ? 'down' : 'chev')}<span style="font-size:var(--fs-title);font-weight:650;color:var(--on-surface)">Later</span></button><span class="n">${all.length}</span></div>
    ${open ? `<ul class="rows">${all.map(([r, t, w]) => row({ ref: r, g: 'paused', title: t, meta: [w], main: ['Bring back now', 'unsnooze', r], later: false, more: false })).join('')}</ul>` : ''}</section>`;
}

// ── Goals ──
function goalRow(r) {
  const g = GOALS[r];
  const kids = g.waves ? g.waves.map(w => `<li class="wv-h"><span class="lbl-s">Wave ${w.n} · ${w.name}${w.when ? ' · ' + w.when : ''}</span></li>` + w.tasks.map(t => { const k = TASKS[t]; return row({ ref: t, g: k.status === 'lost' ? 'lost' : k.status === 'nostart' ? 'nostart' : k.status, title: k.title, meta: [k.line], later: false, more: true }); }).join('')).join('') : '';
  return row({ ref: r, x: 'g-' + r, title: g.name, titleAct: ['goal', r], meta: [st(g.state, g.stateWords), `${g.done} of ${g.total} done`, `wave ${g.wave}`, g.project, `updated ${g.updated}`], later: false, kids });
}
function pgGoals(ds) {
  if (ds === 'loading') return loadingCol('Goals');
  if (ds === 'empty') return `<div class="col"><div class="ph"><div class="t"><h1>Goals</h1></div></div><div class="empty">${ic('goal', 'i var')}<b>No goals yet</b><span class="var">A goal comes from planning an idea or a group of backlog issues.</span><div class="btnrow" style="margin-top:8px"><button class="btn" data-act="go" data-arg="ideas">Ideas</button><button class="btn plain" data-act="go" data-arg="backlog">Backlog</button></div></div></div>`;
  const shelf = (title, refs, main) => `<section class="grp" aria-label="${title}">${grpH(title, refs.length)}<ul class="rows">${refs.map(r => { const g = GOALS[r]; const m = main(g); return row({ ref: r, g: g.shelf === 'finished' ? 'done' : g.shelf === 'paused' ? 'paused' : g.shelf === 'aside' ? 'stopped' : 'planned', title: g.name, titleAct: ['goal', r], meta: m.meta, main: m.btn, later: false }); }).join('')}</ul></section>`;
  return `<div class="col">
  <div class="ph"><div class="t"><h1>Goals</h1><p class="sum">4 running · 2 next · 1 paused</p></div></div>
  <section class="grp" aria-label="Active">${grpH('Active', 4)}<ul class="rows">${ACTIVE.map(goalRow).join('')}</ul></section>
  ${shelf('Next', ['G6', 'G8'], g => ({ meta: [g.after, `${g.total} tasks`, g.project], btn: g.ref === 'G6' ? ['Start now', 'toast', 'G6 started|undo'] : null }))}
  ${shelf('Paused', ['G4'], g => ({ meta: [`${g.done} of ${g.total} done`, g.project], btn: ['Resume', 'toast', 'Resumed G4 · 4 tasks start|undo'] }))}
  ${shelf('Set aside', ['G7'], g => ({ meta: [`${g.done} of ${g.total} done`, g.project], btn: ['Bring back', 'toast', 'G7 is in Next|undo'] }))}
  ${shelf('Finished this week', ['G5', 'G10'], g => ({ meta: [g.prs, g.when, g.project] }))}
  <div class="more-line"><button class="btn plain" data-act="expand" data-arg="earlier" aria-expanded="${!!S.open.earlier}">${S.open.earlier ? 'Hide' : 'Show'} 3 earlier</button></div>
  ${S.open.earlier ? `<ul class="rows">${[['G0', 'Project setup', 'Nov 2'], ['G-1', 'CI on every PR', 'Oct 28'], ['G-2', 'Docs site', 'Oct 20']].map(([r, t, d]) => row({ ref: r, g: 'done', title: t, meta: ['finished ' + d], later: false, more: false })).join('')}</ul>` : ''}
</div>`;
}

// ── Goal page ──
function pgGoal(ds) {
  const g = GOALS[S.goal] || GOALS.G2;
  if (ds === 'loading') return loadingCol(g.name);
  if (g.shelf) return `<div class="col"><div class="crumb"><a role="link" tabindex="0" data-act="go" data-arg="goals">Goals</a>›<span>${g.ref}</span></div><div class="ph"><div class="t"><h1>${g.name}</h1><p class="sum">${g.after || ''}</p></div><div class="acts"><button class="btn pri" data-act="toast" data-arg="${g.ref} started|undo">Start now</button></div></div></div>`;
  const tabs = [['waves', 'Waves'], ['notes', 'Notes', 6], ['files', 'Files', 3], ['backlog', 'Backlog', 2], ['how', 'How it runs']];
  const band = g.stop ? `<div class="band${g.stop.kind === 'missed' ? ' n' : ''}" role="status">${ic(g.stop.kind === 'failed' ? 'failed' : 'needs', 'i ' + (g.stop.kind === 'failed' ? 'c-error' : 'c-needs'))}<div class="t"><b>${g.stop.title}</b><div class="var">${g.stop.detail}</div></div><div class="btnrow"><button class="btn pri" data-act="open" data-arg="${g.stop.primary[1].split(':')[1]}">${g.stop.primary[0]}</button>${g.stop.secondary ? `<button class="btn" data-act="toast" data-arg="${g.stop.secondary[1]}|undo">${g.stop.secondary[0]}</button>` : ''}</div></div>` : '';
  const sp = g.split; const tot = g.total;
  const segs = [['done', 'var(--done)'], ['working', 'var(--working)'], ['review', 'var(--review)'], ['needs', 'var(--needs)'], ['failed', 'var(--error)']].filter(([k]) => sp[k]).map(([k, c]) => `<i style="width:${sp[k] / tot * 100}%;background:${c};display:inline-block;height:100%"></i>`).join('');
  const body = S.goalTab === 'waves' ? wavesHtml(g) : S.goalTab === 'notes' ? goalNotes(g) : S.goalTab === 'files' ? goalFiles(g) : S.goalTab === 'backlog' ? goalBacklog(g) : goalHow(g);
  return `<div class="col${S.goalTab === 'waves' ? ' wide' : ''}" style="${S.goalTab === 'waves' ? 'max-width:1180px' : ''}">
  <div class="crumb"><a role="link" tabindex="0" data-act="go" data-arg="goals">Goals</a>›<span>${g.ref} · ${g.project}</span></div>
  <div class="ph"><div class="t"><h1>${g.name}</h1><p class="sum">${g.tldr}</p></div>
    <div class="acts">${st('' + g.state, g.stateWords)}<button class="btn" data-act="toast" data-arg="Paused ${g.ref}. Running tasks finish first.|undo">${ic('pause')}Pause</button><button class="ib" data-act="menu" data-arg="goal-more" aria-haspopup="menu" aria-label="More for ${g.ref}" title="More">${ic('more')}</button></div></div>
  <div style="display:flex;align-items:center;gap:12px;margin:-8px 0 24px;flex-wrap:wrap"><div class="bar" style="width:200px;display:flex" role="img" aria-label="${g.done} of ${tot} tasks done">${segs}</div><span class="lbl-s num">${g.done} of ${tot} done · wave ${g.wave} · ${g.now} · updated ${g.updated}</span></div>
  ${band}
  <div class="tabs" role="tablist" aria-label="Goal" style="border-bottom:1px solid var(--outline);margin-bottom:8px">${tabs.map(([k, l, n]) => `<button role="tab" aria-selected="${S.goalTab === k}" data-act="gtab" data-arg="${k}">${l}${n ? `<span class="n">${n}</span>` : ''}</button>`).join('')}</div>
  <div class="two">${`<div>${body}</div>`}${S.goalTab === 'waves' ? goalAside(g) : ''}</div>
</div>`;
}

function wavesHtml(g) {
  return g.waves.map(w => {
    const head = `<div class="wave-h"><h3>Wave ${w.n} · ${w.name}</h3>${w.phase === 'done' ? st('done', 'Done') : w.phase === 'now' ? st('working', 'Now') : w.phase === 'failed' ? st('failed', 'A task failed') : st('planned', 'Next')}${w.when ? `<span class="lbl-s">${w.when}</span>` : ''}</div>`;
    const rows = w.tasks.map(t => { const k = TASKS[t]; return row({ ref: t, g: k.status, title: k.title, meta: [k.line], end: true, later: false, main: k.pr && k.pr !== 'No PR yet' && k.pr ? null : null }).replace('<div class="end">', `<div class="end"><span class="lbl-s pr-cell">${k.pr || ''}</span>`); }).join('');
    let gate = '';
    if (g.reviewStop === w.n) gate = `<div class="stop on">${ic('review', 'i c-review')}<span class="grow"><b>Stop for your review</b> before wave ${w.n + 1} starts</span><button class="btn" data-act="toast" data-arg="Wave ${w.n + 1} can start now|undo">Continue to wave ${w.n + 1}</button><button class="ib" data-act="menu" data-arg="row:G3" aria-label="Review stop options" title="Review stop options">${ic('more')}</button></div>`;
    else if (w.n < g.waves.length && w.phase !== 'done') gate = `<div class="stop">${ic('play', 'i')}<span class="grow">Wave ${w.n + 1} starts when this wave is done</span>${w.phase === 'failed' ? `<button class="btn" data-act="toast" data-arg="Wave ${w.n + 1} can start now|undo">Let wave ${w.n + 1} start</button>` : ''}</div>`;
    return `<section class="wave" aria-label="Wave ${w.n}">${head}<ul class="rows">${rows}</ul>${gate}</section>`;
  }).join('');
}
function goalAside(g) {
  return `<aside class="g-aside" aria-label="Goal details" style="display:flex;flex-direction:column;gap:16px;margin-top:20px">
  <div class="card"><h3>Plan review</h3><span class="var">2 findings, both folded into wave 2.</span><ul class="checks"><li>${ic('done', 'i c-done')}Split T6 so the cache lands before Redis</li><li>${ic('done', 'i c-done')}Add a load test after the speed-ups</li></ul></div>
  <div class="card"><h3>Devices</h3><span class="var">iPhone 15 · lent to T5 until it finishes</span></div>
  <div class="card"><h3>Feature flags</h3><ul class="checks"><li>${ic('done', 'i c-done')}<span><span class="mono">fast_session_cache</span> · local</span></li><li>${ic('needs', 'i c-needs')}<span><span class="mono">redis_sessions</span> · backend, not made yet</span></li></ul><span class="lbl-s">Wave 3 waits until <span class="mono">redis_sessions</span> exists.</span></div>
</aside>`;
}
function goalNotes() {
  return `<ul class="tl" style="margin-top:16px">${[['11:20 AM', 'Decision', 'Cache lookups before moving to Redis, so the speed-up shows in wave 2 alone.', 'T6'], ['10:48 AM', 'Found', 'Token refresh does two round trips; filed B46.', 'T5'], ['Tue 3:10 PM', 'Done', 'Baseline p95 is 910 ms on staging.', 'T16'], ['Tue 2:40 PM', 'Done', 'Most time goes to three session reads per request.', 'T13'], ['Tue 9:02 AM', 'Planned', 'Three waves: measure, speed up, check.', 'G2'], ['Mon 4:55 PM', 'Idea', 'From I11 Faster sign-in.', '']].map(([t, k, s, r]) => `<li class="ev"><time>${t}</time><span class="dot"></span><div class="w"><b>${k}${r ? ` · <span class="ref">${r}</span>` : ''}</b><span>${s}</span></div></li>`).join('')}</ul>`;
}
function goalFiles() {
  return `<ul class="rows" style="margin-top:16px">${[['file', 'load-test-baseline.md', 'T16 · Tue 3:10 PM'], ['img', 'flame-graph-signin.png', 'T13 · Tue 2:40 PM'], ['link', 'Staging dashboard', 'You · Mon 5:00 PM']].map(([i, n, m]) => `<li class="row"><div class="g">${ic(i)}</div><div class="mid"><div class="ti"><button class="tt" data-act="toast" data-arg="Opened ${n}">${n}</button></div><div class="meta"><span>${m}</span></div></div><div class="end"><button class="ib" data-act="copy" data-arg="${n}" aria-label="Copy link to ${n}" title="Copy link">${ic('copy')}</button></div></li>`).join('')}</ul>`;
}
function goalBacklog() {
  return `<ul class="rows" style="margin-top:16px">${['B37', 'B38'].map(r => { const b = ISSUES[r]; return row({ ref: r, g: 'queued', title: b.title, meta: [b.kind, b.pri, 'added to this goal'], later: false }); }).join('')}</ul>`;
}
function goalHow(g) {
  return `<dl class="kv" style="margin-top:16px"><dt>Project</dt><dd>${g.project}</dd><dt>Runs</dt><dd>Inside work hours, 9:00 AM – 6:00 PM</dd><dt>Tasks at once</dt><dd>Up to 3</dd><dt>Stops for review</dt><dd>After wave 2</dd><dt>Review</dt><dd>Your review, then Sam’s · pre-merge checks</dd><dt>PRs</dt><dd>Merge as they pass</dd><dt>Planned from</dt><dd><a class="ref" data-act="idea" data-arg="I5" data-uh-ignore-target>I11 Faster sign-in</a></dd><dt>Started</dt><dd>Tue, Nov 17, 9:02 AM</dd></dl>
  <p class="lbl-s" style="margin-top:16px">These are set when the goal is planned. Change them with <span class="mono">tb goal set</span>.</p>`;
}

// ── Backlog ──
function pgBacklog(ds) {
  if (ds === 'loading') return loadingCol('Backlog');
  if (ds === 'empty') return `<div class="col"><div class="ph"><div class="t"><h1>Backlog</h1></div></div><div class="empty">${ic('backlog', 'i var')}<b>The backlog is clear</b><span class="var">Agents add issues here when they find bugs, gaps and follow-ups.</span></div></div>`;
  if (S.blTab === 'closed') return backlogClosed();
  const all = Object.values(ISSUES).filter(b => !b.miss && !b.holdsPr && (S.kind === 'all' || b.kind === S.kind));
  const groups = {}; all.forEach(b => { (groups[b.group] = groups[b.group] || []).push(b); });
  const n = S.picked.size;
  const bulk = n ? `<div class="bulk" role="toolbar" aria-label="${n} selected"><b>${n} selected</b><span class="grow"></span>
    <button class="btn pri" data-act="go" data-arg="plan">Plan into waves</button>
    <button class="btn" data-act="menu" data-arg="bulk-move" aria-haspopup="menu">Move into goal <span class="kbd">M</span></button>
    <button class="btn" data-act="menu" data-arg="bulk-pri" aria-haspopup="menu">Priority <span class="kbd">P</span></button>
    <button class="btn plain sec" data-act="bulk" data-arg="Deferred 3 issues">Defer <span class="kbd">D</span></button>
    <button class="btn plain sec" data-act="bulk" data-arg="Closed 3 issues as won’t do">Won’t do <span class="kbd">X</span></button>
    <button class="ib cmp" data-act="menu" data-arg="bulk-more" aria-haspopup="menu" aria-label="More actions for the selection" title="More">${ic('more')}</button>
    <button class="ib" data-act="clear" aria-label="Clear the selection" title="Clear  esc">${ic('close')}</button></div>` : '';
  const grpHtml = Object.entries(groups).map(([name, items]) => {
    const refs = items.map(b => b.ref); const on = refs.filter(r => S.picked.has(r)).length;
    const folded = S.folded.has(name);
    return `<section class="grp bl-grp" aria-label="${name}"><div class="grp-h"><label class="check" title="Select the group"><input type="checkbox" data-act="pickgrp" data-arg="${refs.join(',')}" ${on === refs.length ? 'checked' : ''} ${on && on < refs.length ? 'data-mixed="1"' : ''} aria-label="Select all in ${name}"></label><button class="btn plain fold" data-act="fold" data-arg="${name}" aria-expanded="${!folded}">${ic(folded ? 'chev' : 'down')}<span>${name}</span></button><span class="n">${items.length}</span><span class="grow"></span>${name !== 'Not grouped yet' ? `<button class="btn plain" data-act="plangrp" data-arg="${refs.join(',')}">Plan these</button>` : ''}</div>
      ${folded ? '' : `<ul class="rows">${items.map(b => blRow(b)).join('')}</ul>`}</section>`;
  }).join('');
  const lens = [['similar', 'Similar'], ['area', 'Area'], ['impact', 'Impact'], ['priority', 'Priority']];
  return `<div class="col">
  <div class="ph"><div class="t"><h1>Backlog</h1><p class="sum">23 open · 4 new since yesterday · 2 need you</p></div>
    <div class="acts"><div class="seg" role="tablist" aria-label="Open or closed"><button role="tab" aria-selected="${S.blTab === 'open'}" data-act="bltab" data-arg="open">Open</button><button role="tab" aria-selected="${S.blTab === 'closed'}" data-act="bltab" data-arg="closed">Closed</button></div></div></div>
  <div class="bl-tools"><div class="seg" role="radiogroup" aria-label="Group by">${lens.map(([k, l]) => `<button role="radio" aria-checked="${S.lens === k}" aria-pressed="${S.lens === k}" data-act="lens" data-arg="${k}">${l}</button>`).join('')}</div>
    <button class="btn" data-act="menu" data-arg="kind" aria-haspopup="menu">${ic('filter')}${S.kind === 'all' ? 'All kinds' : S.kind}<span class="lbl-s">${S.kind === 'all' ? '23' : all.length}</span></button>
    <label class="bl-findw">${ic('search')}<input class="in bl-find" type="search" placeholder="Filter" aria-label="Filter the backlog  ⌘F"><span class="kbd">⌘F</span></label></div>
  ${bulk}
  <p class="lbl-s" style="margin:12px 0 20px">${ic('working', 'i c-working')} Claude is grouping 4 new issues · groups update when it’s done</p>
  ${grpHtml}
  <p class="lbl-s" style="margin-top:24px">Showing 7 of 23 · the rest load as you scroll</p>
</div>`;
}
function blRow(b) {
  const on = S.picked.has(b.ref);
  return `<li class="row bl${on ? ' picked' : ''}${S.panel === b.ref ? ' sel' : ''}" data-ref="${b.ref}" data-open="false">
  <div class="g"><label class="check"><input type="checkbox" data-act="pick" data-arg="${b.ref}" ${on ? 'checked' : ''} aria-label="Select ${b.ref}"></label></div>
  <div class="mid"><div class="ti"><button class="tt" data-act="open" data-arg="${b.ref}" data-k="row-${b.ref}">${b.title}</button><span class="ref">${b.ref}</span></div>
    <div class="meta"><span>${b.kind}</span><span class="sep"></span><span>${b.area}</span><span class="sep"></span><span>${b.project}</span>${b.isNew ? '<span class="sep"></span><span class="c-needs" style="font-weight:600">New</span>' : ''}</div></div>
  <div class="end"><span class="lbl-s pri">${b.pri}</span><button class="ib" data-act="menu" data-arg="row:${b.ref}" aria-haspopup="menu" aria-label="More for ${b.ref}" title="More  ⇧F10">${ic('more')}</button></div>
</li>`;
}
function backlogClosed() {
  const rows = [['B30', 'Session cookie path is too broad', 'Fixed by T6 · PR #412'], ['B31', 'Login page flickers on slow networks', 'Won’t do · Nov 18'], ['B33', 'Old feature-flag docs', 'Done in G5 · Nov 17'], ['B29', 'Migrate the queue tests', 'Made WEB-288 · Nov 16']];
  return `<div class="col"><div class="ph"><div class="t"><h1>Backlog</h1><p class="sum">41 closed</p></div><div class="acts"><div class="seg" role="tablist" aria-label="Open or closed"><button role="tab" aria-selected="false" data-act="bltab" data-arg="open">Open</button><button role="tab" aria-selected="true" data-act="bltab" data-arg="closed">Closed</button></div></div></div>
  <ul class="rows">${rows.map(([r, t, m]) => row({ ref: r, g: m.startsWith('Won') ? 'stopped' : 'done', title: t, meta: [m], main: ['Reopen', 'toast', `Reopened ${r}|undo`], later: false, more: false })).join('')}</ul></div>`;
}

// ── Plan into waves ──
function pgPlan(ds) {
  const w = [['Clear sign-out state', 'Both bugs touch the same sign-out handler; fix them together first.', ['B37', 'B45']], ['Prove it', 'The test needs the fixed handler.', ['B38']]];
  return `<div class="col">
  <div class="crumb"><a role="link" tabindex="0" data-act="go" data-arg="backlog">Backlog</a>›<span>Plan into waves</span></div>
  <div class="ph"><div class="t"><h1>Plan into waves</h1><p class="sum">3 issues · drafted 11:40 AM</p></div><div class="acts"><button class="btn plain" data-act="toast" data-arg="Re-planning · the draft updates when it’s done">Re-plan</button><button class="btn pri" data-act="toast" data-arg="Made G11. Start it when you’re ready.|undo">Create the goal</button></div></div>
  <label class="field" style="max-width:480px;margin-bottom:24px"><span>Goal name</span><input class="in" value="Sign-out leaves nothing behind"></label>
  ${w.map(([n, why, items], i) => `<section class="wave" aria-label="Wave ${i + 1}"><div class="wave-h"><h3>Wave ${i + 1} · ${n}</h3></div><p class="var" style="margin-bottom:8px;max-width:68ch">${why}</p><ul class="rows">${items.map(r => { const b = ISSUES[r]; return row({ ref: r, g: 'planned', title: b.title, meta: [b.kind, b.pri], main: ['Remove', 'toast', `Took ${r} out of the plan|undo`], later: false, more: false }); }).join('')}</ul></section>`).join('')}
  <section class="grp" style="margin-top:32px" aria-label="Added since the last plan">${grpH('Added since the last plan', 1)}<ul class="rows">${row({ ref: 'B47', g: 'queued', title: 'Sign-out doesn’t revoke the refresh token', meta: ['Bug', 'P1', 'found 11:52 AM'], main: ['Add to last wave', 'toast', 'Added B47 to wave 2|undo'], later: false, more: false })}</ul></section>
</div>`;
}

// ── Ideas ──
function pgIdeas(ds) {
  if (ds === 'loading') return loadingCol('Ideas');
  if (ds === 'empty') return `<div class="col"><div class="ph"><div class="t"><h1>Ideas</h1></div></div><div class="empty">${ic('idea', 'i var')}<b>No ideas yet</b><span class="var">An idea gets dug into, scored for fit, and planned into a goal.</span><div class="btnrow" style="margin-top:8px"><button class="btn pri" data-act="newidea">New idea <span class="kbd">⌘N</span></button></div></div></div>`;
  const r = (i) => row({ ref: i.ref, g: i.wait ? 'needs' : i.status === 'Digging' ? 'working' : i.status === 'Planned' ? 'done' : 'queued', title: i.title, titleAct: ['idea', i.ref], meta: [i.status, i.note, i.project, i.when].filter(Boolean), later: !!i.wait, main: i.wait ? (i.ref === 'I4' ? ['Plan', 'toast', 'Planning I4 · a goal appears when it’s ready|undo'] : ['Answer', 'idea', i.ref]) : i.status === 'New' ? ['Dig', 'toast', 'Digging into I7|undo'] : null });
  return `<div class="col">
  <div class="ph"><div class="t"><h1>Ideas</h1><p class="sum">7 open · 2 wait on you</p></div><div class="acts"><button class="btn pri" data-act="newidea">${ic('plus')}New idea</button></div></div>
  <section class="grp" aria-label="Waiting on you">${grpH('Waiting on you', 2)}<ul class="rows">${IDEAS.filter(i => i.wait).map(r).join('')}</ul></section>
  <section class="grp" aria-label="Other ideas">${grpH('Other ideas', 5)}<ul class="rows">${IDEAS.filter(i => !i.wait).map(r).join('')}</ul></section>
  <div class="more-line"><button class="btn plain" data-act="expand" data-arg="closedIdeas" aria-expanded="${!!S.open.closedIdeas}">${S.open.closedIdeas ? 'Hide' : 'Show'} 2 closed</button></div>
  ${S.open.closedIdeas ? `<ul class="rows">${CLOSED_IDEAS.map(i => row({ ref: i.ref, g: 'stopped', title: i.title, meta: [i.status, i.project, i.when], main: ['Reopen', 'toast', `Reopened ${i.ref}|undo`], later: false, more: false })).join('')}</ul>` : ''}
</div>`;
}

function pgIdea(ds) {
  if (ds === 'loading') return loadingCol('Offline mode');
  const qs = [['Which data stays offline?', [['The last 7 days of tasks', 'Small, covers most trips'], ['Everything the person has opened', 'Bigger, nothing surprises them'], ['Only what they pin', 'Smallest; they choose']]], ['What happens on a conflict?', [['The newest edit wins', 'Simple; can lose an edit'], ['Ask the person', 'Safe; adds a step']]]];
  const step = S.qStep.I2 || 0; const [qt, opts] = qs[step];
  return `<div class="col">
  <div class="crumb"><a role="link" tabindex="0" data-act="go" data-arg="ideas">Ideas</a>›<span>I2 · webapp</span></div>
  <div class="ph"><div class="t"><h1>Offline mode</h1><p class="sum">Fits · round 2 of digging · updated 8:15 AM</p></div>
    <div class="acts"><button class="btn" data-act="toast" data-arg="Digging into I2 again|undo">Dig <span class="kbd">⌘D</span></button><button class="btn pri" data-act="dlg" data-arg="planq">Plan <span class="kbd">⇧⌘P</span></button><button class="ib" data-act="menu" data-arg="idea-more" aria-haspopup="menu" aria-label="More for I2" title="More">${ic('more')}</button></div></div>
  <section class="attn" aria-label="Questions"><h3>${ic('needs', 'i c-needs')}2 questions before planning</h3>
    <div class="q"><div class="qsteps" role="tablist" aria-label="Questions">${qs.map((q, i) => `<button role="tab" aria-selected="${i === step}" data-act="qstep" data-arg="I2|${i}">Question ${i + 1}</button>`).join('')}</div>
    <p class="qt"><q>${qt}</q></p>
    <div class="opts" role="radiogroup" aria-label="Answers">${opts.map(([b, s], i) => `<button class="opt" role="radio" aria-checked="${S.pick['I2-' + step] === i}" aria-pressed="${S.pick['I2-' + step] === i}" data-act="pickopt" data-arg="I2-${step}|${i}"><span class="r"></span><b>${b}</b><span>${s}</span></button>`).join('')}</div>
    <div class="answer"><label class="field"><span>Other answer</span><textarea class="in" rows="1" data-answer="I2-${step}">${esc(S.drafts['I2-' + step] || '')}</textarea></label><button class="btn pri" data-act="${step === 0 ? 'qstep' : 'toast'}" data-arg="${step === 0 ? 'I2|1' : 'Answers sent · digging again|undo'}">${step === 0 ? 'Next' : 'Send'}</button></div></div>
  </section>
  <div class="prose" style="margin-top:32px">
    <h4>Round 2 · today, 8:15 AM</h4>
    <p>A service worker can cache the shell and the last-used lists. The store already versions every record, so offline edits can queue and replay.</p>
    <div class="score" role="table" aria-label="Fit scorecard">${[['Fits the product', 'Strong', 'done'], ['Effort', '3 goals, about 2 weeks', 'needs'], ['Risk', 'Conflicts on shared tasks', 'needs'], ['Builds on', 'G2 Faster sign-in (session cache)', 'done']].map(([k, v, s]) => `<div role="row"><span role="rowheader">${k}</span><span role="cell">${st(s, v)}</span></div>`).join('')}</div>
    <h4>How it would flow</h4>
    <ol><li>The app caches the shell and recent lists on first load.</li><li>Edits made offline queue on the device.</li><li>On reconnect, the queue replays and conflicts ask the person.</li></ol>
    <h4>Points to weigh</h4>
    <ul><li><b class="c-error">Security</b> · cached data stays on shared computers; sign-out must clear it.</li><li>Search won’t work offline in the first goal.</li></ul>
    <h4>Compared with</h4>
    <p>A read-only offline view is a third of the work, but people asked to edit on trains.</p>
    <h4>Related</h4>
    <p><a class="ref" data-act="idea" data-arg="I3" data-uh-ignore-target>I3 Search across projects</a> · <a class="ref" data-act="open" data-arg="B37" data-uh-ignore-target>B37 Session cookie isn’t cleared on sign-out</a></p>
    <details><summary>Round 1 · yesterday, 4:30 PM</summary><p class="var" style="margin-top:8px">First pass: fits, but the store would need versions. It has them now.</p></details>
  </div>
</div>`;
}

// ── Projects ──
const PJ = {
  webapp: { path: '~/Development/webapp', line: '3 goals · needs you · 1 working', mark: 'needs', rel: ['v2.8.0', 'Oct 17 · 13 unreleased'], waits: null, backlog: 15,
    open: [['G1', 'needs', 'Needs you', { done: 1, working: 1, needs: 1, planned: 2 }], ['G2', 'working', 'Working', { done: 2, review: 1, working: 1, needs: 1, planned: 2 }], ['G6', 'queued', 'Queued', { planned: 6 }], ['G7', 'stopped', 'Set aside', { done: 1, planned: 3 }]],
    loose: [], fin: [['G10', 'Upgrade build tooling', 4, 'Nov 16']] },
  api: { path: '~/Development/api', line: '4 goals · 1 task · needs you', mark: 'needs', rel: null, waits: 'webapp', backlog: 8,
    open: [['G3', 'failed', 'Failed', { done: 1, failed: 1, needs: 1, planned: 2 }], ['G9', 'needs', 'Needs you', { done: 4 }], ['G8', 'queued', 'Waits on G3', { planned: 4 }], ['G4', 'paused', 'Paused', { done: 3, planned: 4 }]],
    loose: ['T8'], fin: [['G5', 'Audit log', 5, 'Nov 18'], ['G11', 'Key rotation', 3, 'Nov 9'], ['G12', 'Health endpoint', 2, 'Nov 2'], ['G13', 'Request ids', 4, 'Oct 28'], ['G14', 'Postgres 16', 6, 'Oct 21'], ['G15', 'Error envelope', 3, 'Oct 14'], ['G16', 'OpenAPI spec', 5, 'Oct 3']] }
};
const PJ_IDLE = [['docs', '2 done'], ['infra', '']];
const PSEG = ['done', 'review', 'working', 'needs', 'failed', 'planned'];
const pbar = (b) => { const t = PSEG.reduce((s, k) => s + (b[k] || 0), 0); return `<span class="pbar" role="img" aria-label="${PSEG.filter(k => b[k]).map(k => `${b[k]} ${k === 'planned' ? 'not started' : k === 'needs' ? 'need you' : k}`).join(', ')}">${PSEG.filter(k => b[k]).map(k => `<i class="p-${k}" style="flex:${b[k]}"></i>`).join('')}</span>`; };
function pgProjects(ds) {
  const head = `<div class="ph"><div class="t"><h1>Projects</h1></div></div>`;
  if (ds === 'loading') return loadingCol('Projects');
  if (ds === 'empty') return `<div class="col">${head}<div class="empty">${ic('project', 'i var')}<b>No projects</b><span class="var">Add one with <span class="mono">tb project add</span>.</span></div></div>`;
  const cur = S.proj || 'api', p = PJ[cur];
  const item = (id, line, mark) => `<li><button class="pj-i" data-act="proj" data-arg="${id}" ${cur === id ? 'aria-current="true"' : ''}><b>${id}</b>${line ? `<span>${mark ? ic(mark, 'i c-' + (mark === 'needs' ? 'needs' : 'working')) : ''}${line}</span>` : ''}</button></li>`;
  const tasksOf = (g) => Object.values(TASKS).filter(t => t.goal === g);
  const goalItem = ([r, gs, words, b]) => { const g = GOALS[r]; const kids = tasksOf(r).map(k => row({ ref: k.ref, g: k.status, title: k.title, meta: [k.line], later: false, more: true })).join('') || `<li class="row"><div class="g"></div><div class="mid"><span class="var">No tasks yet</span></div></li>`;
    return row({ ref: r, x: 'pg-' + r, title: g.name, titleAct: ['goal', r], meta: [st(gs, words), `${pbar(b)} ${g.done || 0} of ${g.total} done${g.wave ? ' · wave ' + g.wave.split(' ')[0] : ''}`], later: false, kids }); };
  const finN = S.open['fin-all'] ? p.fin.length : 5;
  return `<div class="col">${head}
  <div class="pj">
  <nav class="pj-list" aria-label="Projects"><ul>${Object.entries(PJ).map(([id, q]) => item(id, q.line, q.mark)).join('')}</ul>
    <button class="fold" data-act="expand" data-arg="pj-idle" aria-expanded="${!!S.open['pj-idle']}">${ic('down')}Nothing open <span class="n">${PJ_IDLE.length}</span></button>
    ${S.open['pj-idle'] ? `<ul>${PJ_IDLE.map(([id, l]) => item(id, l)).join('')}</ul>` : ''}</nav>
  <div class="pj-main">
    <div class="pj-h"><h2>${cur}</h2><dl class="kv"><dt>Path</dt><dd class="mono">${p.path}</dd>${p.rel ? `<dt>Latest release</dt><dd><button class="lnk" data-act="go" data-arg="releases-webapp">${p.rel[0]}</button> · ${p.rel[1]}</dd>` : ''}${p.waits ? `<dt>Waits for</dt><dd>${p.waits}</dd>` : ''}<dt>Backlog</dt><dd><button class="lnk" data-act="go" data-arg="backlog">${p.backlog} open</button></dd></dl></div>
    <section class="grp" aria-label="Open goals">${grpH('Open goals', p.open.length)}<ul class="rows">${p.open.map(goalItem).join('')}</ul></section>
    ${p.loose.length ? `<section class="grp" aria-label="Tasks">${grpH('Tasks', p.loose.length)}<ul class="rows">${p.loose.map(r => { const k = TASKS[r]; return row({ ref: r, g: k.status, title: k.title, meta: [k.line], later: false, more: true }); }).join('')}</ul></section>` : ''}
    <section class="grp" aria-label="Finished">${grpH('Finished', p.fin.length)}<ul class="rows">${p.fin.slice(0, finN).map(([r, n, c, d]) => row({ ref: r, g: 'done', title: n, titleAct: ['goal', r], meta: [`${c} tasks`, d], later: false, more: false })).join('')}</ul>
    ${p.fin.length > 5 ? `<div class="more-line"><button class="btn plain" data-act="expand" data-arg="fin-all" aria-expanded="${!!S.open['fin-all']}">${S.open['fin-all'] ? 'Show fewer' : `Show all ${p.fin.length}`}</button></div>` : ''}</section>
  </div></div>
</div>`;
}

// ── Days ──
function dayHead(view) {
  return `<div class="ph"><div class="t"><h1>Days</h1></div><div class="acts"><div class="seg" role="tablist" aria-label="View">${[['day', 'Day'], ['week', 'Week in review'], ['recaps', 'Recaps']].map(([k, l]) => `<button role="tab" aria-selected="${view === k}" data-act="dview" data-arg="${k}">${l}${k === 'recaps' ? ' <span class="c-needs">·</span>' : ''}</button>`).join('')}</div></div></div>`;
}
function pgDays(ds) {
  if (ds === 'loading') return loadingCol('Days');
  if (S.dayView === 'week') return pgWeek(ds);
  if (S.dayView === 'recaps') return pgRecaps(ds);
  const lanes = [['T2 Passkey sign-in screen', [[10, 38, 'w'], [38, 42, 'n'], [42, 70, 'w']]], ['T3 Passkey management page', [[5, 30, 'w'], [30, 60, 'n']]], ['T6 Cache session lookups', [[0, 22, 'w'], [22, 52, 'r']], 52], ['T10 Move sessions to Redis', [[12, 32, 'w']], 32], ['T7 Per-key token bucket', [[2, 25, 'w']], 25], ['T5 Profile the token refresh path', [[30, 70, 'w']]]];
  return `<div class="col wide"><div class="${''}">
  ${dayHead('day')}
  <div style="display:flex;align-items:center;gap:16px;flex-wrap:wrap;margin-bottom:24px"><div class="daynav"><button class="ib" data-act="toast" data-arg="Thursday, Nov 19" aria-label="Previous day" title="Previous day  ←">${ic('back')}</button><b>Today, Fri, Nov 20</b><button class="ib" aria-disabled="true" aria-label="Next day" title="Next day  →">${ic('fwd')}</button></div><span class="grow" style="flex:1"></span><button class="btn" data-act="menu" data-arg="day-projects" aria-haspopup="menu">${ic('project')}Both projects</button></div>
  <div class="two"><div>
  <div class="stats">${[['6 h 40 m', 'agent time'], ['4', 'tasks done'], ['3', 'PRs merged'], ['52 m', 'waiting on you'], ['2', 'failed']].map(([b, s]) => `<div class="stat"><b>${b}</b><span>${s}</span></div>`).join('')}</div>
  <div class="week" role="group" aria-label="This week, agent hours per day">${[['Mon', 50], ['Tue', 80], ['Wed', 64], ['Thu', 72], ['Fri', 58], ['Sat', 0], ['Sun', 0]].map(([d, h], i) => `<button ${i === 4 ? 'aria-current="date"' : ''} data-act="noop" aria-label="${d}, ${(h / 10).toFixed(1)} hours"><i style="height:${h}%"></i>${d}</button>`).join('')}</div>
  <section class="grp" style="margin-top:32px" aria-label="Timeline">${grpH('Timeline', null, `<div class="legend"><span><i style="background:var(--working)"></i>Working</span><span><i style="background:var(--needs)"></i>Waiting on you</span><span><i style="background:var(--review)"></i>In review</span><span><i style="background:var(--on-surface);width:2px"></i>Merged</span></div>`)}
    <div class="lanes" role="table" aria-label="Tasks over the day, 9 AM to 7 PM"><span></span><div class="axis" role="row">${['9 AM', '10', '11', '12 PM', '1', '2', '3', '4', '5', '6'].map(h => `<span>${h}</span>`).join('')}</div>
    ${lanes.map(([n, segs, mk]) => `<div class="ln" role="row"><button class="nm" role="rowheader" data-act="open" data-arg="${n.split(' ')[0]}">${n}</button><div class="track" role="cell">${segs.map(([a, b, c]) => `<span class="seg-b ${c}" style="left:${a}%;width:${b - a}%"></span>`).join('')}${mk ? `<span class="mk" style="left:${mk}%" title="Merged"></span>` : ''}<span class="nowl" style="left:46%" title="Now, 1:40 PM"></span></div></div>`).join('')}</div></section>
  <section class="grp" aria-label="How long tasks took">${grpH('How long tasks took', 8)}<ul class="hbars">${[['T6 Cache session lookups', 82, '2 h 10 m'], ['T18 Key usage counters', 60, '1 h 35 m'], ['T1 Passkey registration', 48, '1 h 15 m'], ['T13 Trace the sign-in path', 30, '48 m']].map(([n, w, t]) => `<li><span>${n}</span><span class="bar"><i style="width:${w}%"></i></span><span class="num">${t}</span></li>`).join('')}</ul><div class="more-line"><button class="btn plain" data-act="noop">Show all 8</button></div></section>
  <section class="grp" aria-label="Done on the day">${grpH('Done on the day', 14)}<ul class="rows">${['T1', 'T18', 'T13'].map(r => { const t = TASKS[r]; return row({ ref: r, g: 'done', title: t.title, meta: [t.line, t.project], later: false, more: false }); }).join('')}</ul><div class="more-line"><button class="btn plain" data-act="noop">Show all 14</button></div></section>
  </div>
  <aside class="g-aside" aria-label="Day details" style="display:flex;flex-direction:column;gap:16px">
    <div class="card"><h3>Notable</h3><ul class="checks"><li>${ic('done', 'i c-done')}G9 finished its last wave</li><li>${ic('failed', 'i c-error')}T7 failed under load</li><li>${ic('needs', 'i c-needs')}Midna hit its terminal limit at 1:20 PM</li></ul></div>
    <div class="card"><h3>Waiting on you</h3><span><b class="num">52 m</b> today · median wait 9 m</span></div>
    <div class="card"><h3>Agent time by project</h3><ul class="hbars">${[['webapp', 58, '3 h 52 m'], ['api', 42, '2 h 48 m']].map(([n, w, t]) => `<li style="grid-template-columns:64px 1fr 64px"><span>${n}</span><span class="bar"><i style="width:${w}%"></i></span><span class="num">${t}</span></li>`).join('')}</ul></div>
    <div class="card"><h3>This week</h3><dl class="kv"><dt>Hours saved</dt><dd class="num">31 h</dd><dt>Tasks done</dt><dd class="num">27</dd></dl></div>
    <div class="card"><h3>Planning · 30 days</h3><dl class="kv num"><dt>Added after planning</dt><dd>14%</dd><dt>Failed or retried</dt><dd>9%</dd><dt>Fix rounds a task</dt><dd>1.3</dd><dt>Questions a task</dt><dd>0.6</dd><dt>Time vs estimate</dt><dd>+12%</dd></dl></div>
  </aside></div>
</div></div>`;
}

function pgWeek(ds) {
  if (ds === 'empty') return `<div class="col wide">${dayHead('week')}<div class="empty">${ic('days', 'i var')}<b>No reviews yet</b><span class="var">The first one comes on Saturday, 9:00 AM. Agents have worked 31 h this week.</span></div></div>`;
  const nums = [['Shipped', '27 tasks', '+6', true], ['Agent time', '41 h', '+4 h', null], ['Waiting on you', '3 h 50 m', '−1 h 10 m', true], ['Timeouts', '5', '+3', false], ['Commands rerun', '12', '−4', true]];
  return `<div class="col wide">
  ${dayHead('week')}
  <div style="display:flex;align-items:center;gap:8px;flex-wrap:wrap;margin-bottom:24px"><div class="daynav"><button class="ib" data-act="toast" data-arg="Nov 7 – 13" aria-label="Previous week" title="Previous week">${ic('back')}</button><b>Nov 14 – 20</b><button class="ib" aria-disabled="true" aria-label="Next week" title="Next week">${ic('fwd')}</button></div><button class="btn plain" data-act="menu" data-arg="weeks" aria-haspopup="menu">All weeks</button><span class="lbl-s">compared with Nov 7 – 13</span></div>
  <div class="two"><div>
  <div class="stats">${nums.map(([k, v, d, good]) => `<div class="stat"><b>${v}</b><span>${k} · <span class="${good === true ? 'c-done' : good === false ? 'c-error' : ''}">${d}${good === true ? ' better' : good === false ? ' worse' : ''}</span></span></div>`).join('')}</div>
  <section class="grp" aria-label="Suggested fixes">${grpH('Suggested fixes', 3, `<button class="btn" data-act="go" data-arg="backlog">Plan the open fixes</button>`)}
    <p class="lbl-s" style="margin-bottom:8px">Filed as backlog issues · 2 open · 1 in a goal</p>
    <ul class="rows">${[['B48', 'Cache the cargo build between test runs', 'Follow-up', 'Open in the backlog'], ['B49', 'Raise the integration test timeout to 5 minutes', 'Clean-up', 'Open in the backlog'], ['B46', 'Token refresh does two round trips', 'Bug', 'In G2 Faster sign-in']].map(([r, t, k, w]) => row({ ref: r, g: w.startsWith('In') ? 'working' : 'queued', title: t, meta: [k, w], later: false, more: false })).join('')}</ul>
    <div class="banner bad" style="border-radius:8px;margin-top:12px;padding:12px 16px">${ic('failed', 'i c-error')}<div class="t"><b>One fix wasn’t filed</b><span class="var">The backlog was busy at 9:00 AM.</span></div><button class="btn" data-act="toast" data-arg="Filing again">File again</button></div>
  </section>
  <section class="grp" aria-label="Shipped">${grpH('Shipped', 27)}<ul class="rows">${row({ ref: 'v0.13.1', g: 'done', title: 'webapp v0.13.1', meta: ['release', 'Wed'], later: false, more: false, showRef: false })}${row({ ref: 'G9', g: 'done', title: 'Password reset emails', titleAct: ['goal', 'G9'], meta: ['goal', '4 PRs'], later: false, more: false })}${['T1', 'T18', 'T13'].map(r => row({ ref: r, g: 'done', title: TASKS[r].title, meta: [TASKS[r].pr], later: false, more: false })).join('')}</ul><div class="more-line"><button class="btn plain" data-act="noop">Show all 27</button></div></section>
  <section class="grp" aria-label="Where the time went">${grpH('Where the time went', null)}<ul class="hbars">${[['Writing code', 44, '18 h · 44%', '+2 h'], ['Running tests', 27, '11 h · 27%', '+3 h'], ['Waiting on review', 16, '6 h 30 m · 16%', '−1 h'], ['Reading the code', 13, '5 h 30 m · 13%', '0']].map(([n, w, t, d]) => `<li style="grid-template-columns:160px 1fr 180px"><span>${n}</span><span class="bar"><i style="width:${w}%"></i></span><span class="num">${t} · ${d}</span></li>`).join('')}</ul></section>
  <section class="grp" aria-label="Waiting on you">${grpH('Waiting on you', null)}<dl class="kv"><dt>Questions</dt><dd>1 h 40 m · 11 times</dd><dt>PR reviews</dt><dd>1 h 25 m · 6 times</dd><dt>To-dos</dt><dd>45 m · 2 times</dd><dt>Longest</dt><dd><a class="ref" data-act="open" data-arg="H1" data-uh-ignore-target>H1</a> 2 h · <a class="ref" data-act="open" data-arg="T3" data-uh-ignore-target>T3</a> 55 m</dd></dl></section>
  <section class="grp" aria-label="Slowest tasks">${grpH('Slowest tasks', null)}<ul class="hbars">${[['T6 Cache session lookups', 90, '5 h 10 m'], ['T7 Per-key token bucket', 70, '4 h'], ['T10 Move sessions to Redis', 55, '3 h 10 m']].map(([n, w, t]) => `<li><span>${n}</span><span class="bar"><i style="width:${w}%"></i></span><span class="num">${t}</span></li>`).join('')}</ul></section>
  <section class="grp" aria-label="Commands">${grpH('Commands', null)}
    <h3 class="sub">Slow and repeated</h3><table class="tbl"><thead><tr><th>Command</th><th>Median</th><th>p90</th><th>Runs</th></tr></thead><tbody>${[['cargo test -p api', '3 m 40 s', '6 m 10 s', 48], ['npm run e2e', '2 m 5 s', '4 m', 31]].map(r => `<tr><td class="mono">${r[0]}</td><td class="num">${r[1]}</td><td class="num">${r[2]}</td><td class="num">${r[3]}</td></tr>`).join('')}</tbody></table>
    <h3 class="sub">Timed out</h3><table class="tbl"><thead><tr><th>Command</th><th>Task</th><th>After</th></tr></thead><tbody><tr><td class="mono">cargo test --test load</td><td>T7</td><td class="num">10 m</td></tr><tr><td class="mono">npm run e2e -- --grep passkey</td><td>T2</td><td class="num">5 m</td></tr></tbody></table>
    <h3 class="sub">Rerun</h3><table class="tbl"><thead><tr><th>Command</th><th>Task</th><th>Second try</th></tr></thead><tbody><tr><td class="mono">cargo test -p api rate</td><td>T7</td><td>${st('failed', 'Failed again')}</td></tr><tr><td class="mono">npm test session</td><td>T6</td><td>${st('done', 'Passed')}</td></tr></tbody></table>
  </section>
  </div>
  <aside class="g-aside" aria-label="Agent time by project" style="display:flex;flex-direction:column;gap:16px"><div class="card"><h3>Agent time by project</h3><ul class="hbars">${[['webapp', 61, '25 h'], ['api', 39, '16 h']].map(([n, w, t]) => `<li style="grid-template-columns:64px 1fr 48px"><span>${n}</span><span class="bar"><i style="width:${w}%"></i></span><span class="num">${t}</span></li>`).join('')}</ul></div></aside>
  </div>
</div>`;
}

function pgRecaps() {
  return `<div class="col wide">${dayHead('recaps')}
  <div class="two"><div>
  <div class="hero"><span class="lbl-s">Recap · this week · new</span><b>Sign-in got 2× faster and reset emails shipped.</b></div>
  <div class="prose"><h4>What moved</h4><ul><li>G9 Password reset emails finished: 4 PRs, one outcome still to prove.</li><li>G2 Faster sign-in is two waves in; p95 is down from 910 ms to 450 ms.</li></ul><h4>What got stuck</h4><ul><li>T7 fails under load; G3 waits on it.</li></ul><h4>Next week</h4><ul><li>G6 Offline mode groundwork starts Monday.</li></ul></div>
  </div>
  <aside aria-label="Earlier recaps"><h3 class="sub" style="margin-top:24px">Earlier recaps</h3><ul class="rows">${[['This week', 'Nov 14 – 20', true], ['Last week', 'Nov 7 – 13', false], ['October', 'month', false]].map(([t, m, n]) => `<li class="row" style="grid-template-columns:16px 1fr"><span class="g">${n ? '<span class="c-needs" title="Not seen yet">●</span>' : ''}</span><div class="mid"><div class="ti"><button class="tt" data-act="noop">${t}</button></div><div class="meta"><span>${m}</span></div></div></li>`).join('')}</ul></aside>
  </div></div>`;
}

// ── Releases ──
const REL = {
  webapp: { total: 14, last30: 3, failed: 1,
    next: { name: 'v2.9.0', status: '2 still to finish', after: [['G2', 'Faster sign-in', '2 of 7 done', 'goal'], ['T3', 'Passkey management page', 'Needs you', 'task'], ['T1', 'Passkey registration endpoint', 'Done', 'task']],
      done: [['G1', 'Sign-in with passkeys', [['T1', 'Passkey registration endpoint', '#402']]], [null, 'No goal', [['T24', 'Fix flaky avatar upload test', '#405']]]] },
    past: [['October 2026', [
      ['v2.8.0', 'Passkeys for everyone', 'Oct 17, 3:05 PM', 6, false, 'Sign-in with a passkey on any browser that supports it.\nPassword sign-in stays for older devices.', [['G0', 'Passkeys groundwork', [['T30', 'WebAuthn library', '#380'], ['T31', 'Credential table', '#382']]]]],
      ['v2.7.1', 'Notarizing timed out', 'Oct 9, 11:20 AM', 0, true, '', [], [['T28', 'Session cookie flags', 'went out in v2.7.2'], ['T29', 'Header spacing', 'in Unreleased']]],
      ['v2.7.0', 'Faster dashboard', 'Oct 3, 4:40 PM', 9, false]]],
      ['September 2026', [['v2.6.0', 'Team invites', 'Sep 22, 2:15 PM', 7, false], ['v2.5.2', 'Billing fixes', 'Sep 9, 10:05 AM', 3, false]]]] },
  docs: { total: 3, last30: 1, failed: 0, dropped: ['v1.3.0', 'its last task, T41, was closed without a PR', 'tb release cut docs v1.3.0'],
    unreleased: '2 tasks · 1 goal', past: [['November 2026', [['v1.2.0', 'Search box', 'Nov 12, 9:30 AM', 4, false]]]] }
};
function pgReleases(ds) {
  if (ds === 'loading') return loadingCol('Releases');
  if (ds === 'empty') return `<div class="col"><div class="ph"><div class="t"><h1>Releases</h1></div></div><div class="empty">${ic('release', 'i var')}<b>Releases are off</b><span class="var">Turn them on for a project with <span class="mono">tb project set &lt;name&gt; --releases on</span>.</span></div></div>`;
  return S.relProj ? relProject(S.relProj) : relOverview();
}
const relRow = (r, open) => {
  const [name, head, when, n, failed, notes, work, wasIn] = r; const k = 'rel-' + name; const isOpen = open ?? !!S.open[k];
  const groups = (gs) => gs.map(([g, gn, ts]) => `<li class="wv-h">${g ? `<button class="lnk" data-act="goal" data-arg="${g}">${g} ${gn}</button>` : `<span class="lbl-s">${gn}</span>`}</li>${ts.map(([t, tt, pr]) => row({ ref: t, g: 'done', title: tt, meta: [`<button class="lnk mono" data-act="toast" data-arg="Opened PR ${pr} in the browser">${pr}</button>`], later: false, more: false })).join('')}`).join('');
  const body = isOpen ? `<div class="body">${failed ? `<p class="err">${ic('failed')}Failed ${when} · its work went back to Unreleased</p><h3 class="sub">Was in it</h3><ul class="checks">${wasIn.map(([t, tt, w]) => `<li><span class="mono">${t}</span> ${tt}${w ? ` <span class="var">· ${w}</span>` : ''}</li>`).join('')}</ul>` : `${notes ? `<p class="prose">${notes.split('\n').slice(1).join(' ')}</p>` : ''}${work && work.length ? `<ul class="rows kids">${groups(work)}</ul>` : ''}`}</div>` : '';
  return `<li class="row" data-ref="${name}" data-open="${isOpen}"><div class="g">${notes !== undefined ? `<button class="x" data-act="expand" data-arg="${k}" aria-expanded="${isOpen}" aria-label="${isOpen ? 'Fold' : 'Unfold'} ${name}">${ic('chev')}</button>` : glyph(failed ? 'failed' : 'done')}</div>
  <div class="mid"><div class="ti"><b>${name}</b><span class="var"> · ${head}</span></div><div class="meta">${failed ? st('failed', 'Failed') : `<span>${n} tasks</span>`}<span class="sep"></span><span>${when}</span></div></div><div class="end"></div>${body}</li>`;
};
function relOverview() {
  const card = (p, newest, unrel, earlier) => `<section class="card rel-c" aria-label="${p}"><div class="grp-h"><h2>${p}</h2><span class="grow"></span><button class="lnk" data-act="relproj" data-arg="${p}">${REL[p].total} releases</button></div>
    <button class="rel-new" data-act="relproj" data-arg="${p}"><b>${newest[0]}</b><span>${newest[1]}</span><span class="var">${newest[2]} · ${newest[3]} tasks</span></button>
    ${unrel ? `<button class="rel-un" data-act="relproj" data-arg="${p}">${ic(unrel[2], 'i c-' + unrel[2])}<span><b>Unreleased</b> · ${unrel[0]}</span><span class="var">${unrel[1]}</span></button>` : ''}
    <ul class="rel-earlier">${earlier.map(([n, h, w, f]) => `<li><button class="lnk" data-act="relproj" data-arg="${p}">${n}</button> <span class="var">${h}</span><span class="grow"></span>${f ? st('failed', 'Failed') : `<span class="var">${w}</span>`}</li>`).join('')}</ul></section>`;
  return `<div class="col"><div class="ph"><div class="t"><h1>Releases</h1><p class="sum">5 tasks unreleased · 1 pending cut</p></div></div>
  <div class="rel-grid">
  ${card('webapp', ['v2.8.0', 'Passkeys for everyone', 'Oct 17, 3:05 PM', 6], ['3 tasks', 'v2.9.0 · 2 still to finish', 'working'], [['v2.7.1', 'Notarizing timed out', 'Oct 9', true], ['v2.7.0', 'Faster dashboard', 'Oct 3'], ['v2.6.0', 'Team invites', 'Sep 22']])}
  ${card('docs', ['v1.2.0', 'Search box', 'Nov 12, 9:30 AM', 4], ['2 tasks', 'v1.3.0 dropped', 'needs'], [])}
  <section class="card rel-c" aria-label="api"><div class="grp-h"><h2>api</h2></div><p class="err">${ic('failed')}Can’t load api’s releases · timed out</p><div class="btnrow"><button class="btn" data-act="toast" data-arg="Loaded api’s releases">Try again</button></div></section>
  </div></div>`;
}
function relProject(p) {
  const r = REL[p];
  const std = (s) => s === 'Done' ? st('done', s) : s === 'Needs you' ? st('needs', s) : `<span class="var">${s}</span>`;
  return `<div class="col"><div class="ph"><div class="t"><button class="sw-back" data-act="relproj" data-arg="">${ic('back')}Releases</button><h1>${p}</h1><p class="sum">${r.total} releases · ${r.last30} in the last 30 days${r.failed ? ` · ${r.failed} failed` : ''}</p></div></div>
  ${r.dropped ? `<div class="attn rel-drop" role="note"><div class="lines"><b>${r.dropped[0]} was dropped: ${r.dropped[1]}</b><span>Cut it by hand once it’s ready:</span><pre class="mono">${r.dropped[2]}</pre></div></div>` : ''}
  ${r.next ? `<section class="grp" aria-label="Next release">${grpH('Next release · ' + r.next.name, null, `<span class="var">${r.next.status}</span>`)}
    <h3 class="sub">Cuts after</h3><ul class="rows">${r.next.after.map(([ref, t, s, kind]) => row({ ref, g: s === 'Done' ? 'done' : s === 'Needs you' ? 'needs' : 'working', title: t, titleAct: kind === 'goal' ? ['goal', ref] : ['open', ref], meta: [std(s)], later: false, more: false })).join('')}</ul>
    <h3 class="sub">Already finished</h3><ul class="rows">${r.next.done.map(([g, gn, ts]) => `<li class="wv-h">${g ? `<button class="lnk" data-act="goal" data-arg="${g}">${g} ${gn}</button>` : `<span class="lbl-s">${gn}</span>`}</li>${ts.map(([t, tt, pr]) => row({ ref: t, g: 'done', title: tt, meta: [`<button class="lnk mono" data-act="toast" data-arg="Opened PR ${pr} in the browser">${pr}</button>`], later: false, more: false })).join('')}`).join('')}</ul></section>` : ''}
  ${r.unreleased ? `<section class="grp" aria-label="Unreleased">${grpH('Unreleased', null, `<span class="var">${r.unreleased}</span>`)}<ul class="rows">${row({ ref: 'T42', g: 'done', title: 'Dark mode for code blocks', meta: ['G20 Docs polish', '#61'], later: false, more: false })}${row({ ref: 'T43', g: 'done', title: 'Fix broken anchor links', meta: ['G20 Docs polish', '#62'], later: false, more: false })}</ul></section>` : ''}
  ${r.past.map(([m, rs], mi) => `<section class="grp" aria-label="${m}">${grpH(m, null)}<ul class="rows">${rs.map((x, i) => relRow(x, mi === 0 && i === 0 ? (S.open['rel-' + x[0]] === undefined ? true : S.open['rel-' + x[0]]) : undefined)).join('')}</ul></section>`).join('')}
  ${p === 'webapp' ? `<div class="more-line"><span class="var">${sk(1)}</span></div>` : ''}
</div>`;
}

// ── First run ──
function pgSetup() {
  const steps = [['done', 'Connect to the board', 'Running on this Mac'], ['needs', 'Add a project', 'Run tb project add in the project’s folder', 'copy', 'tb project add'], ['queued', 'Connect GitHub', 'So agents can open PRs', 'settings', 'accounts'], ['queued', 'Set your work hours', 'New work starts only inside them', 'settings', 'hours']];
  return `<div class="col"><div class="ph"><div class="t"><h1>Set up</h1><p class="sum">Three steps and your agents can start.</p></div></div>
  <ul class="rows">${steps.map(([s, t, m, a, arg]) => `<li class="row"><div class="g">${glyph(s)}</div><div class="mid"><div class="ti"><b>${t}</b></div><div class="meta"><span>${m}</span></div></div><div class="end">${a ? `<button class="btn${s === 'needs' ? ' pri' : ''}" data-act="${a}" data-arg="${arg}">${a === 'copy' ? ic('copy') + 'Copy the command' : 'Open'}</button>` : ''}</div></li>`).join('')}</ul></div>`;
}

// ── Settings window ──
const SET = [['accounts', 'Accounts', 'link', 1], ['history', 'History', 'clock'], ['usage', 'Usage', 'gauge'], ['hours', 'Work hours', 'moon'], ['qa', 'QA', 'check'], ['questions', 'Questions', 'talk'], ['reviewers', 'Reviewers', 'review'], ['worktrees', 'Worktrees', 'branch'], ['system', 'System', 'pulse', 1]];
function settingsWin(ds) {
  const sec = S.setSec;
  return `<div class="sw">
  <nav class="sw-side" aria-label="Settings sections"><div class="sw-top"><button class="ib" data-act="back" aria-label="Close Settings" title="Close  ⌘W">${ic('close')}</button><h1>Settings</h1></div><div class="nav">${SET.map(([k, l, i, n]) => `<a role="link" tabindex="0" data-act="setsec" data-arg="${k}" ${sec === k ? 'aria-current="page"' : ''} title="${l}${n ? ', 1 needs you' : ''}">${ic(i)}<span class="lbl">${l}</span>${n ? `<span class="n hot" aria-hidden="true">${n}</span>` : ''}</a>`).join('')}</div></nav>
  <main class="sw-main" aria-label="${(SET.find(s => s[0] === sec) || [, ''])[1]}"><div class="sw-col">${ds === 'loading' ? `<h2>${(SET.find(s => s[0] === sec) || [, ''])[1]}</h2>${sk(5)}` : isDown() ? `<h2>${(SET.find(s => s[0] === sec) || [, ''])[1]}</h2><p class="err">${ic('failed')}The board didn’t answer · connection refused</p>` : setBody(sec)}</div></main>
  <div class="layer">${overlays()}</div><div class="toasts" role="status" aria-live="polite">${toastHtml()}</div>
</div>`;
}
const srow = (t, s, ctl) => `<div class="setrow"><div class="t"><b>${t}</b>${s ? `<span>${s}</span>` : ''}</div>${ctl}</div>`;
const sw_ = (on, label, cli) => `<input type="checkbox" class="switch" role="switch" ${on ? 'checked' : ''} aria-label="${label}" disabled title="Change with ${cli}">`;
const cli = (c) => `<p class="lbl-s">Change with <span class="mono">${c}</span></p>`;
const pick = (label, opts, cur) => `<select class="in" aria-label="${label}">${opts.map(o => `<option${o === cur ? ' selected' : ''}>${o}</option>`).join('')}</select>`;
function setBody(sec) {
  switch (sec) {
    case 'accounts': {
      if (S.flow === 'github') return `<h2>Accounts</h2><div class="card"><h3>Sign in to GitHub</h3><p>Paste this code on GitHub’s page. It’s already copied.</p><button class="code" data-act="copy" data-arg="the code" aria-label="Code W D J B dash M J H T, copy again" title="Copy again">WDJB-MJHT</button><div class="btnrow"><button class="btn pri" data-act="toast" data-arg="Opened github.com/login/device · code copied again">${ic('ext')}Open GitHub</button><button class="btn plain" data-act="flow" data-arg="">Cancel</button></div><span class="lbl-s">${ic('working', 'i c-working')} Waiting for you to approve · the code works for 15 minutes</span></div>`;
      const a = [
        ['GH', 'GitHub', st('done', 'morgan · checked 9:02 AM'), `<button class="btn plain" data-act="toast" data-arg="GitHub still works · checked just now">Check</button>`],
        ['BB', 'Bitbucket', st('needs', 'Missing read:user · names show as ids'), ''],
        ['AZ', 'Azure DevOps', '<span class="var">Not connected</span>', ''],
        ['SL', 'Slack', st('failed', 'Refused Nov 19 · no Slack messages'), ''],
        ['JI', 'Jira', st('done', 'acme.atlassian.net · QA comments on'), `<button class="btn plain" data-act="toast" data-arg="Jira still works · checked just now">Check</button><button class="btn plain" data-act="toast" data-arg="Jira disconnected · tickets and QA comments wait|undo">Disconnect</button>`]];
      return `<h2>Accounts</h2><div>${a.map(([av, n, s, b]) => `<div class="acct"><span class="av" aria-hidden="true">${av}</span><div><b>${n}</b><div class="lbl-s">${s}</div></div><div class="btnrow">${b}</div></div>`).join('')}</div>
      <p class="lbl-s">Tokens live in the login Keychain. GitHub signs in with a code: <button class="ref" data-act="flow" data-arg="github" data-uh-ignore-target>Sign in with a code</button>. Other accounts connect with <span class="mono">tb accounts</span>.</p>`;
    }
    case 'history': return `<h2>History</h2>
      ${srow('Every event', 'Commits, questions, checkpoints · the log and day marks', '<span>1 year</span>')}
      ${srow('Day summaries', 'One per project per day · timeline bars and week totals', '<span>Forever</span>')}
      <dl class="kv"><dt>Space</dt><dd>48,210 events · 31 MB · 412 day summaries · 640 KB</dd><dt>Cleanup</dt><dd>Nightly after 3:00 AM · last ran today 3:04 AM</dd></dl>${cli('tb history')}`;
    case 'usage': return `<h2>Usage</h2>
      <p>${st('needs', 'Today is limited · 18% of the week used of a 20% share · new work starts again at midnight')}</p>
      ${srow('Daily share', 'Today’s part of the weekly budget', '<span>20%</span>')}
      ${srow('Unused share', '', '<span>Carries to the next day</span>')}${cli('tb usage daily')}`;
    case 'hours': return `<h2>Work hours</h2>
      ${srow('Only start agents in work hours', 'On now · new work starts until 6:00 PM', `<input type="checkbox" class="switch" role="switch" checked aria-label="Only start agents in work hours">`)}
      ${srow('Days', '', `<div class="daypick" role="group" aria-label="Work days">${['S', 'M', 'T', 'W', 'T', 'F', 'S'].map((d, i) => `<button aria-pressed="${i > 0 && i < 6}" data-act="noop" aria-label="${['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'][i]}" title="${['Sunday', 'Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday'][i]}">${d}</button>`).join('')}</div>`)}
      ${srow('Hours', '', `<div class="time"><input class="in" value="9:00 AM" aria-label="Start"><span>–</span><input class="in" value="6:00 PM" aria-label="End"></div>`)}
      ${srow('Today until', 'Ends today at 8:00 PM', `<div class="time"><input class="in" value="8:00 PM" aria-label="Today until"><button class="ib" data-act="toast" data-arg="Today ends at the usual 6:00 PM|undo" aria-label="Clear today until" title="Clear">${ic('close')}</button></div>`)}
      ${srow('Keep the Mac awake', 'Holding until 6:00 PM · weekdays', st('done', 'On'))}${cli('tb keep-awake')}`;
    case 'qa': return `<h2>QA</h2>
      ${srow('Read QA comments from Jira', 'Clear directions become follow-up tasks; questions wait on you', st('done', 'On'))}
      <dl class="kv"><dt>Read</dt><dd>14 comments</dd><dt>Waiting on you</dt><dd>1</dd><dt>Checked</dt><dd>2 min ago</dd></dl>${cli('tb qa')}`;
    case 'questions': return `<h2>Questions</h2>
      ${srow('Send back buried questions', 'A question asked mid-reply goes back once, so it reaches you as a question', st('done', 'On'))}${cli('tb settings questions')}`;
    case 'reviewers': return `<h2>Reviewers</h2>
      <h3 class="sub">webapp</h3><table class="tbl fixed"><thead><tr><th style="width:44%">Reviewer</th><th>Median</th><th>Open asks</th><th>Last asked</th></tr></thead><tbody>
      <tr><td><b>Sam Ortiz</b> · pinned<br><span class="lbl-s">sortiz · sam@acme.dev</span></td><td>3h 10m</td><td>1</td><td>Today 11:52 AM</td></tr>
      <tr><td><b>lens-bot</b> · every 2 hours<br><span class="lbl-s">automation ×0.5</span></td><td>14m</td><td>0</td><td>Today 11:40 AM</td></tr>
      <tr><td><b>Jo Park</b> · removed<br><span class="lbl-s">left the team</span></td><td>–</td><td>–</td><td>Oct 30</td></tr></tbody></table>
      <h3 class="sub">api</h3><table class="tbl fixed"><thead><tr><th style="width:44%">Reviewer</th><th>Median</th><th>Open asks</th><th>Last asked</th></tr></thead><tbody>
      <tr><td><b>Ana Lee</b><br><span class="lbl-s">alee</span></td><td>5h 40m</td><td>2</td><td>Thu 4:15 PM</td></tr></tbody></table>${cli('tb reviewers')}`;
    case 'worktrees': return `<h2>Worktrees</h2>
      <h3 class="sub">webapp</h3>
      ${srow('Slots', 'Isolated checkouts kept for agents', `<div class="time">${pick('Minimum slots', ['0', '1', '2', '3', '4'], '2')}<span>to</span>${pick('Maximum slots', ['4', '6', '8', '10', '12'], '6')}</div>`)}
      ${srow('Clear build output after', '', pick('Clear build output after', ['1 day', '3 days', '7 days', 'Never'], '3 days'))}
      ${srow('Remove an idle slot after', '', pick('Remove an idle slot after', ['1 day', '7 days', '30 days', 'Never'], '7 days'))}
      <table class="tbl" style="margin-top:16px"><thead><tr><th>Slot</th><th>Size</th><th>Last used</th><th>Held by</th></tr></thead><tbody>
      <tr><td class="mono">webapp-1</td><td>2.1 GB</td><td>12:51 PM</td><td>T3</td></tr><tr><td class="mono">webapp-2</td><td>2.4 GB</td><td>11:52 AM</td><td>T6</td></tr><tr><td class="mono">webapp-3</td><td>1.8 GB</td><td>Thu 5:20 PM</td><td class="var">Free</td></tr></tbody></table>
      <p class="lbl-s">${ic('queued', 'i')} 1 task waits for a slot · T8</p>`;
    case 'system': return `<h2>System</h2>
      <div class="card"><h3>Update</h3>${st('needs', 'Version 0.14.0 is ready')}<p class="var">Agents keep running in Midna. The board is offline for about 10 seconds and 2 code reviews start again.</p><div class="btnrow"><button class="btn pri" data-act="toast" data-arg="Restarting to update…">Restart to update</button><button class="btn plain" data-act="toast" data-arg="Opened the 0.14.0 release notes">${ic('ext')}Release notes</button></div><span class="lbl-s">Public releases only · this is 0.13.2</span></div>
      <div class="card"><h3>Board</h3>${st('done', 'Running · 0.13.2 · up 3 days · pid 4127')}<div class="btnrow"><button class="btn" data-act="restart-board">Restart the board</button><button class="btn plain" data-act="toast" data-arg="Opened the board log">Open the log</button><button class="btn plain" data-act="toast" data-arg="Saved taskboard-debug.zip to Downloads">Export for debugging</button></div></div>
      <div class="card"><h3>Midna</h3>${st('needs', 'At its limit · 8 of 8 terminals')}<span class="var">Raise the limit in Midna’s own settings.</span></div>
      <div class="card"><h3>Hooks</h3>${st('done', 'Installed in 2 projects')}<div class="btnrow"><button class="btn plain" data-act="toast" data-arg="Hooks reinstalled in 2 projects">Reinstall</button></div></div>`;
  }
  return '';
}
