// ── Side panel: one task or one issue ──
const PSTATUS = {
  T3: ['needs', 'Needs you'], T6: ['needs', 'Waiting on your review'], T10: ['lost', 'Terminal lost'], T5: ['quiet', 'May be stuck'],
  T7: ['failed', 'Failed'], T11: ['nostart', "Couldn't start"], H1: ['human', 'Your to-do'], T8: ['queued', 'Queued'],
  T12: ['done', 'Merged'], T13: ['done', 'Merged'], T2: ['working', 'Working'], T4: ['planned', 'Planned'], T1: ['done', 'Merged']
};
const KIND = { H1: 'Human task', T16: 'Research' };
const JIRA = { T3: 'WEB-204 · In progress', T6: 'WEB-198 · In review', T12: 'API-61 · Ready for QA', T13: 'WEB-190 · Done' };
const PRN = { T6: '#412', T10: '#409', T12: '#91', T13: '#398', T1: '#402' };
const branch = (ref) => `${ref.toLowerCase()}-${(TASKS[ref] || { title: '' }).title.toLowerCase().replace(/[^a-z0-9]+/g, '-').replace(/-$/, '').slice(0, 28)}`;
const btn = (label, act, arg, cls = '') => `<button class="btn ${cls}" data-act="${act}" data-arg="${arg}">${label}</button>`;

function panelHtml(ds) {
  const ref = S.panel;
  if (ISSUES[ref]) return issuePanel(ref);
  const t = TASKS[ref]; if (!t) return '';
  const [sk_, sw] = PSTATUS[ref] || [t.status, null];
  const g = t.goal && GOALS[t.goal];
  const wave = g && g.waves ? g.waves.find(w => w.tasks.includes(ref)) : null;
  const goalLink = g ? `<a data-act="goal" data-arg="${g.ref}" title="${g.waves ? 'Next up: ' + nextUp(g) : ''}" data-uh-ignore-target>${g.ref} ${g.name}${wave ? ` · wave ${wave.n} of ${g.waves.length}` : ''} · ${g.done || 0} of ${g.total} done</a>` : '<span>No goal</span>';
  const files = [
    PRN[ref] ? `<button class="file" data-act="toast" data-arg="Opened PR ${PRN[ref]} in the browser" title="Open in the browser">${ic('pr')}PR ${PRN[ref]}</button>` : '',
    t.status !== 'planned' && !t.human ? `<button class="file" data-act="copy" data-arg="${branch(ref)}" title="Copy the branch name">${ic('branch')}<span class="mono">${branch(ref)}</span></button>` : '',
    ['T3', 'T6', 'H1', 'T16'].includes(ref) ? `<button class="file" data-act="pop" data-arg="att" aria-haspopup="dialog" title="Attachments">${ic('file')}${ref === 'T6' ? 3 : 2} attachments</button>` : ''
  ].join('');
  const logN = { T3: 86, T6: 214, T10: 312, T7: 141, T2: 57, H1: 9, T8: 3, T5: 120, T11: 6, T12: 98, T13: 77, T4: 1 }[ref] || 40;
  const tabs = [['story', 'Story'], ['brief', 'Brief'], ['handoff', 'Handoff'], ['log', 'Log', logN]];
  const body = S.panelTab === 'brief' ? briefTab(ref) : S.panelTab === 'handoff' ? handoffTab(ref) : S.panelTab === 'log' ? logTab(ref, logN) : storyTab(ref);
  return `<div class="insp-h">
    <div class="insp-top">${S.panelBack ? `<button class="btn plain bk" data-act="panel-back" title="Back to ${S.panelBack} ${(TASKS[S.panelBack] || {}).title || ''}">${ic('back')}${S.panelBack}</button>` : ''}${st(sk_, sw)}${KIND[ref] ? `<span class="kind">${KIND[ref]}</span>` : ''}<span class="grow"></span>
      <button class="ib" data-act="copy" data-arg="${ref}" aria-label="Copy ${ref}" title="Copy ${ref}  ⇧⌘C">${ic('copy')}</button>
      <button class="ib" data-act="menu" data-arg="task-more:${ref}" aria-haspopup="menu" aria-label="More for ${ref}" title="More">${ic('more')}</button>
      <button class="ib" data-act="close-panel" aria-label="Close ${ref}" title="Close  esc">${ic('close')}</button></div>
    <h2 tabindex="-1" data-k="insp-title">${t.title}</h2>
    <div class="meta"><span class="ref">${ref}</span>${goalLink}<a data-act="go" data-arg="projects" title="~/Development/${t.project}" data-uh-ignore-target>${t.project}</a>${JIRA[ref] ? `<a data-act="toast" data-arg="Opened ${JIRA[ref].split(' ')[0]} in Jira" data-uh-ignore-target>${JIRA[ref]}</a>` : ''}</div>
    ${files ? `<div class="files">${files}</div>` : ''}
    <div class="tabs" role="tablist" aria-label="Views of ${ref}">${tabs.map(([k, l, n]) => `<button role="tab" aria-selected="${S.panelTab === k}" data-act="tab" data-arg="${k}">${l}${n ? `<span class="n">${n}</span>` : ''}</button>`).join('')}</div>
  </div>
  <div class="insp-b" role="tabpanel" aria-label="${tabs.find(x => x[0] === S.panelTab)[1]}">${body}</div>`;
}
const nextUp = (g) => { for (const w of g.waves) for (const r of w.tasks) { const k = TASKS[r]; if (k && k.status === 'planned') return `${r} ${k.title}`; } return 'nothing left'; };

// ── Story: what needs you, then the timeline, then what comes next ──
function storyTab(ref) {
  return `${attnBlock(ref)}${prSection(ref)}<section aria-label="Timeline"><h3 class="sub">Timeline</h3><ol class="tl">${(TL[ref] || TL._).map(e => evHtml(e)).join('')}</ol></section>${nextPath(ref)}`;
}
const attn = (cls, head, body, acts) => `<section class="attn${cls ? ' ' + cls : ''}" aria-label="${head.replace(/<[^>]+>/g, '')}"><h3>${head}</h3>${body}${acts ? `<div class="btnrow">${acts}</div>` : ''}</section>`;

function attnBlock(ref) {
  switch (ref) {
    case 'T3': return attn('', `${ic('needs', 'i c-needs')}T3 asks · 9 min ago`, `<div class="q"><span class="lbl-s">Question 1 of 2</span><p class="qt"><q>Should removing the last passkey require a password re-check?</q></p>${optsHtml('T3')}${answerHtml('T3')}</div>`, `${btn('Answer in the terminal', 'toast', 'Focused T3’s terminal in Midna', 'plain')}`);
    case 'T6': return attn('', `${ic('review', 'i c-review')}PR #412 waits on your review`, `<p>Cache session lookups · checks passed 11:52 AM. Sam reviews after you.</p>`, `${btn('Open PR #412', 'toast', 'Opened PR #412 in the browser', 'pri')}${btn('I reviewed it', 'gone', 'T6|Marked PR #412 reviewed')}`);
    case 'T10': return attn('bad', `${ic('stopped', 'i c-error')}Terminal lost, but the work looks finished`, `<dl class="kv"><dt>Closed</dt><dd>11:05 AM</dd><dt>Last update</dt><dd><q>Opened PR #409, waiting on checks</q> · 10:58 AM</dd><dt>Looks finished</dt><dd><a data-act="toast" data-arg="Opened PR #409 in the browser" data-uh-ignore-target>PR #409</a> opened 10:58 AM · commit <span class="mono">4be19c2</span></dd><dt>Handoff</dt><dd>Saved 10:57 AM · 42 turns · 6 checkpoints</dd></dl>`,
      `${btn('Check and finish', 'gone', 'T10|T10 marked done', 'pri')}${btn('Resume in a new terminal', 'toast', 'Resuming T10 in a new terminal')}${btn('Reopen the old conversation', 'toast', 'Reopened T10’s conversation in Midna', 'plain')}${btn('See what it would get', 'tab', 'handoff', 'plain')}`);
    case 'T5': return attn('', `${ic('quiet', 'i c-needs')}May be stuck · flagged 12:40 PM`, `<p>No update for 40 minutes while a command runs.</p><details class="lines"${S.open.lines ? ' open' : ''}><summary data-act="expand" data-arg="lines">Last lines from the terminal</summary><pre class="mono">$ cargo bench -p auth refresh
   Compiling auth v0.9.2
    Running benches/refresh.rs
refresh/cold   time: [412.8 ms 418.1 ms 423.9 ms]
refresh/warm   …</pre></details>`, `${btn('Focus terminal', 'toast', 'Focused T5’s terminal in Midna', 'pri')}${btn('Dismiss', 'gone', 'T5|Dismissed the alert for T5', 'plain')}`);
    case 'T7': return attn('bad', `${ic('failed', 'i c-error')}Failed 34 min ago`, `<p><q>3 of 212 tests fail under load: the bucket refills twice when two requests land in the same millisecond.</q></p><p class="lbl-s">No PR: the tests failed. No evidence: nothing to show yet.</p>`, `${btn('Try again', 'gone', 'T7|T7 queued again', 'pri')}`);
    case 'T11': return attn('bad', `${ic('failed', 'i c-error')}Couldn't start · 20 min ago`, `<p>Midna refused the start: it is at its limit of 8 terminals.</p>`, `${btn('Start in a new terminal', 'gone', 'T11|T11 starting in a new terminal', 'pri')}${btn('Queue in Midna', 'gone', 'T11|T11 queued in Midna')}`);
    case 'H1': return humanBox();
    case 'T8': return attn('', `${ic('queued', 'i var')}Not starting yet`, `<p>It isn’t in a goal and nothing is scheduled.</p><form class="startat" data-form="startat"><label class="field"><span>Start at</span><input class="in" name="at" value="" aria-describedby="sa-hint"></label><button class="btn" data-act="toast" data-arg="T8 starts today at 5:00 PM|undo">Schedule</button></form><span class="lbl-s" id="sa-hint">Words like 5pm or tomorrow 9am</span>`, `${btn('Start now', 'toast', 'T8 starting in a new terminal', 'pri')}${btn('Start when the repo is free', 'toast', 'T8 starts when api is free|undo')}`);
    case 'T4': return attn('', `${ic('planned', 'i var')}Planned for wave 2 of G1`, `<p>Queue it now to take it out of the plan; G1’s rules then decide when it starts.</p>`, btn('Queue it now', 'toast', 'T4 queued|undo'));
    case 'T12': return attn('', `${ic('done', 'i c-done')}Merged without a note for testers`, `<p>API-61 waits for the note before it moves to QA.</p>`, btn('Ask for the note', 'toast', 'Asked T12 for the note. Its conversation is coming back to write it.', 'pri'));
  }
  return '';
}

function humanBox() {
  const done = S.gone.has('H1');
  if (done) return attn('', `${ic('done', 'i c-done')}Done by you · just now`, `<p><q>RP ID set to app.example.com in production. Restarted the web service.</q></p>`, btn('Reopen', 'undo', ''));
  return `<section class="attn todo" aria-label="Your to-do"><h3>${ic('needs', 'i c-needs')}Only you can do this</h3>
    <ol class="prose steps"><li>Open the production settings for <span class="mono">webapp</span> in the hosting console.</li><li>Add <code>PASSKEY_RP_ID</code> with the value <code>app.example.com</code>.</li><li>Restart the web service and say below when it’s back.</li></ol>
    <div class="btnrow">${btn(`${ic('ext')}console.hosting.example`, 'toast', 'Opened console.hosting.example in the browser', 'plain')}</div>
    <label class="field"><span>Note for the agents</span><textarea class="in" rows="2" data-k="h1-note" aria-describedby="h1-who"></textarea></label><span class="lbl-s" id="h1-who">T3 and T4 read this when they pick up.</span>
    <div class="btnrow">${btn('Done', 'gone', 'H1|H1 done', 'pri')}${btn(`${ic('talk')}Talk it over`, 'toast', 'Opened a conversation about H1 in Midna', 'plain')}</div></section>`;
}

// ── Pull request ──
function prSection(ref) {
  const head = (n, title, read) => `<div class="pr-h"><h3 class="sub">PR ${n} · ${title}</h3><span class="lbl-s">acme/${TASKS[ref].project} · read ${read}</span></div>`;
  const step = (k, name, what, cur) => `<li${cur ? ' class="cur"' : ''}>${glyph(k)}<span>${name}</span><span class="var">${what}</span></li>`;
  if (ref === 'T6') return `<section class="psec" aria-label="Pull request">${head('#412', 'Cache session lookups', '1 min ago')}
    <ol class="pipe">${step('done', 'Author review', '<a data-act="tab" data-arg="story" data-uh-ignore-target>Round 2 passed</a>')}${step('done', 'Checks', '<a data-act="toast" data-arg="Opened build 1182" data-uh-ignore-target>Passed · build 1182</a>')}${step('needs', 'Your review', 'Waiting on you', true)}${step('queued', 'Sam’s review', 'After yours · 0 of 1 approvals')}${step('planned', 'Before merge', 'Changelog line · agent next')}${step('planned', 'Merge', 'Waits on T10’s PR #409')}</ol>
    <dl class="kv"><dt>Reviewers</dt><dd><button class="lnk" data-act="pop" data-arg="rev" aria-haspopup="dialog">Sam Ortiz</button> · not asked yet · swapped 0 times</dd>
    <dt>Stacks on</dt><dd><a data-act="open" data-arg="T10" data-uh-ignore-target>T10</a>’s PR #409, still open · <span title="Tail merge: this PR merges after the one below it, onto main">tail merge</span></dd>
    <dt>Terminal</dt><dd>None on it · comes back if review asks for changes</dd></dl></section>`;
  if (ref === 'T10') return `<section class="psec" aria-label="Pull request">${head('#409', 'Move sessions to Redis', '3 min ago')}
    <ol class="pipe">${step('done', 'Author review', 'Round 1 passed')}${step('done', 'Checks', 'Passed · build 1179')}${step('quiet', 'Your review', 'After the work is finished')}${step('planned', 'Merge', 'Waits on steps')}</ol>
    <dl class="kv"><dt>Stacked on it</dt><dd><a data-act="open" data-arg="T6" data-uh-ignore-target>T6</a>’s PR #412</dd></dl></section>`;
  if (ref === 'T13') return `<section class="psec" aria-label="Pull request">${head('#398', 'Trace the sign-in path', 'Tue 2:41 PM')}
    <p>${st('done', 'Merged Tue 2:40 PM')}</p>
    <div class="note-pv"><span class="lbl-s">Note for testers</span><p><b>What changed:</b> each sign-in step now records how long it took.</p><p><b>Where to look:</b> the staging traces, filtered to sign-in.</p><p><b>Check:</b> a slow step shows up in red.</p></div>
    <div class="btnrow">${btn('Show the whole note', 'dlg', 'note', 'plain')}</div></section>`;
  if (ref === 'T12') return `<section class="psec" aria-label="Pull request">${head('#91', 'Export audit events as CSV', '8 min ago')}<p>${st('done', 'Merged Wed 4:12 PM')}</p></section>`;
  return '';
}

function nextPath(ref) {
  const t = TASKS[ref];
  if (!['T3', 'T2', 'T5', 'T4'].includes(ref)) return '';
  return `<section aria-label="What comes next"><h3 class="sub">What comes next</h3><ol class="path">${['Author review', 'Pull request', 'Checks', 'Your review, then Sam’s', 'Merge'].map(s => `<li>${s}</li>`).join('')}</ol>${ref === 'T2' ? '<p class="lbl-s">Stacks on T1’s PR #402, merged.</p>' : ''}</section>`;
}

// ── Timeline ──
const evHtml = (e) => `<li class="ev"><time>${e[0]}</time><span class="dot${e[3] ? ' now' : ''}"></span><div class="w"><b>${e[1]}</b>${e[2] || ''}${e[4] ? `<details${e[5] ? ' open' : ''}><summary>Details</summary>${e[4]}</details>` : ''}</div></li>`;
const startedDetail = (term, closed) => `<dl class="kv"><dt>Terminals</dt><dd>${term}${closed ? `<br><span class="var">${closed}</span>` : ''}</dd><dt>Uncommitted</dt><dd>2 files</dd></dl>`;
const TL = {
  _: [['9:02 AM', 'Started', '<span class="var">In a Midna terminal</span>']],
  T3: [['12:51 PM', 'Now · waiting on you', '<span class="var">Worked 38 min before asking</span>', true],
    ['12:30 PM', 'Checks it ran', `<ul class="checks"><li>${glyph('done')}Unit tests · 48 passed</li><li>${glyph('done')}Lint · clean</li></ul>`, false, '<p class="lbl-s">Ran on commit <span class="mono">9a1c0e4</span></p>'],
    ['12:13 PM', 'Started', '<span class="var">Terminal webapp-3 · first terminal</span>', false, startedDetail(`${btn(`${ic('term')}webapp-3 · waiting`, 'toast', 'Focused webapp-3 in Midna', 'plain')}`)]],
  T6: [['11:52 AM', 'Done · PR opened', '<p><q>Session lookups read from an in-memory cache with a 30 s lifetime; p95 sign-in drops from 610 ms to 380 ms.</q></p>'],
    ['11:40 AM', 'Code review round 2', `${st('done', 'Passed')}`, false, '<p class="lbl-s">Reviewed by lens · $0.42 · commit <span class="mono">c71e2aa</span></p>'],
    ['11:21 AM', 'Code review round 1', `${st('needs', '2 findings · both fixed')}`, false, `<ul class="checks"><li>${glyph('done')}<span>Fixed · major · cache never evicts on sign-out · <span class="mono">cache.ts:44</span></span></li><li>${glyph('done')}<span>Sent to backlog as B45 · minor · prompt stays open in other tabs</span></li></ul>`],
    ['10:30 AM', 'Started', '<span class="var">Terminal webapp-2 · 1 earlier closed</span>', false, startedDetail('webapp-2 · closed 11:53 AM', 'webapp-1 · closed 10:29 AM')]],
  T10: [['11:05 AM', 'Now · terminal lost', '<span class="var">Waiting on you</span>', true], ['10:58 AM', 'Opened PR #409'], ['9:15 AM', 'Started', '<span class="var">Terminal webapp-1</span>']],
  T5: [['12:40 PM', 'Now · no update for 40 min', '<p><q>Running the refresh benchmark with a warm cache</q></p>', true], ['12:00 PM', 'Started', '<span class="var">Terminal webapp-4</span>']],
  T7: [['12:26 PM', 'Failed', '<span class="var">Its terminal api-2 is still open</span>', true], ['12:20 PM', 'Load test', `${st('failed', '3 of 212 failed')} <span class="stale-f" title="The code moved on since this ran">${ic('warn')}Code moved since</span>`, false, `<ul class="checks"><li>${glyph('failed')}<span>F1 · high · double refill under contention · <span class="mono">bucket.rs:88</span></span></li><li>${glyph('failed')}<span>F2 · medium · Retry-After off by one second · <span class="mono">limit.rs:131</span></span></li></ul><p class="lbl-s">Ran on commit <span class="mono">e02b7d1</span></p>`, true], ['11:10 AM', 'Started', '<span class="var">Terminal api-2</span>']],
  T11: [['12:40 PM', 'Couldn’t start', '<span class="var">Midna at 8 of 8 terminals</span>', true]],
  H1: [['11:00 AM', 'Waiting on you', '<span class="var">T1 finished, so this opened</span>', true]],
  T8: [['Thu 3:15 PM', 'Added', '<span class="var">From backlog issue B36</span>']],
  T2: [['1:19 PM', 'Now · working 42 min', '<p><q>Wiring the browser prompt into the sign-in form</q> · updated 1 min ago</p>' + btn(`${ic('term')}Focus terminal`, 'toast', 'Focused T2’s terminal in Midna', 'plain'), true], ['12:37 PM', 'Started', '<span class="var">Terminal webapp-5</span>']],
  T12: [['Wed 4:12 PM', 'Merged', '<p><q>Audit events export as CSV from the audit log page.</q></p>'], ['Wed 1:30 PM', 'Started']],
  T13: [['Tue 2:40 PM', 'Merged'], ['Tue 9:05 AM', 'Started']],
  T4: [['Nov 18', 'Planned', '<span class="var">Wave 2 of G1</span>']]
};

// ── Brief, handoff, log ──
function briefTab(ref) {
  if (ref !== 'T3') return `<div class="prose"><p>${TASKS[ref].title}.</p><p class="var">Nothing more written yet.</p></div>`;
  return `<div class="prose"><p>Build the page where people see and remove their passkeys.</p>
    <h4>Done when</h4><ul><li>The page lists each passkey with its device name and last use.</li><li>Removing one asks for confirmation and updates the list.</li><li>The last passkey can’t be removed without the re-check decided in the question.</li></ul>
    <h4>Notes</h4><p>Reuse the list component from <code>src/settings/devices.tsx</code>. The API is ready:</p>
    <div class="code-b"><pre>DELETE /api/passkeys/:id</pre><button class="ib" data-act="copy" data-arg="the code block" aria-label="Copy code" title="Copy">${ic('copy')}</button></div></div>`;
}
function handoffTab(ref) {
  const t = TASKS[ref];
  return `<p class="lbl-s">Saved ${ref === 'T10' ? '10:57 AM' : '12:48 PM'} · a new terminal gets all of this</p>
  <dl class="kv"><dt>Branch</dt><dd class="mono">${branch(ref)}</dd><dt>Worktree</dt><dd class="mono">~/.wt/${t.project}/${ref.toLowerCase()}</dd><dt>Last commit</dt><dd><span class="mono">4be19c2</span> · 10:56 AM</dd><dt>Uncommitted</dt><dd>Nothing</dd>
    <dt>Conversation</dt><dd><span class="mono">a91f…3c2e</span> · can reopen in a new terminal</dd><dt>Came from</dt><dd><a data-act="goal" data-arg="${t.goal || 'G2'}" data-uh-ignore-target>Planned in ${t.goal || 'G2'}</a></dd>
    <dt>Devices</dt><dd>None needed</dd><dt>Feature flags</dt><dd><span class="mono">redis_sessions</span> · created</dd></dl>
  <div class="ho"><h3 class="sub">Done so far</h3><ul class="prose"><li>Session store moved behind a trait</li><li>Redis store with a 24 h lifetime</li><li>PR #409 opened</li></ul>
  <h3 class="sub">Next</h3><ul class="prose"><li>Wait for checks, then hand in</li></ul>
  <h3 class="sub">Decisions</h3><ul class="prose"><li>Kept the in-memory store for tests</li></ul>
  <h3 class="sub">Your answers</h3><ul class="prose"><li><q>Use the shared Redis, not a new one</q></li></ul>
  <h3 class="sub">Found, not fixed here</h3><ul class="prose"><li><a data-act="open" data-arg="B39" data-uh-ignore-target>B39 Split the auth middleware</a> · backlog</li></ul>
  <h3 class="sub">Files touched</h3><div class="files"><span class="file mono" title="src/session/store.rs">store.rs</span><span class="file mono" title="src/session/redis.rs">redis.rs</span><span class="file mono" title="tests/session.rs">session.rs</span></div>
  <details><summary>The whole handoff</summary><pre class="mono ho-pre">## T10 Move sessions to Redis
Branch ${branch(ref)} · 6 checkpoints · 42 turns
Done: session store trait, Redis store, PR #409 …</pre></details></div>`;
}
function logTab(ref, n) {
  const kinds = ['Everything', 'Status', 'Turns', 'Checkpoints', 'Questions', 'Found', 'Jira'];
  const rows = [['12:51 PM', 'question', 'T3', 'Should removing the last passkey require a password re-check?'], ['12:48 PM', 'checkpoint', 'T3', 'Done: list and remove. Next: last-passkey rule.'], ['12:40 PM', 'turn', 'T3', 'Edited src/settings/passkeys.tsx'], ['12:30 PM', 'status', 'board', 'Checks passed'], ['12:22 PM', 'commit', 'T3', '9a1c0e4 List passkeys with last use'], ['12:20 PM', 'jira', 'board', 'WEB-204 moved to In progress'], ['12:14 PM', 'midna', 'board', 'Terminal webapp-3 opened'], ['12:13 PM', 'status', 'board', 'Started']];
  return `<div class="logf" role="radiogroup" aria-label="Show">${kinds.map((k, i) => `<button class="btn plain${i ? '' : ' on'}" role="radio" aria-checked="${!i}" data-act="noop">${k}</button>`).join('')}</div>
  <ol class="log" aria-label="${n} entries, newest first">${rows.map(r => `<li><span title="Fri, Nov 20, ${r[0]}">${r[0]}</span><span>${r[1]}</span><span><b>${r[2]}</b> ${r[3]}</span></li>`).join('')}<li class="more-line"><span></span><span></span><span class="var">Loading older entries…</span></li></ol>`;
}

// ── Issue panel ──
function issuePanel(ref) {
  const b = ISSUES[ref];
  const flag = b.miss ? `<span class="kind bad">Holds ${b.goal} open</span>` : b.holdsPr ? `<span class="kind bad">Holds PR ${b.holdsPr}</span>` : '';
  const state = `Open · reported ${b.reported}`;
  return `<div class="insp-h">
    <div class="insp-top">${S.panelBack ? `<button class="btn plain bk" data-act="panel-back" title="Back to ${S.panelBack}">${ic('back')}${S.panelBack}</button>` : ''}<span class="st c-quiet">${ic('backlog', 'i g')}${b.kind}</span>${flag}<span class="grow"></span>
      <button class="ib" data-act="copy" data-arg="${ref}" aria-label="Copy ${ref}" title="Copy ${ref}  ⇧⌘C">${ic('copy')}</button>
      <button class="ib" data-act="menu" data-arg="task-more:${ref}" aria-haspopup="menu" aria-label="More for ${ref}" title="More">${ic('more')}</button>
      <button class="ib" data-act="close-panel" aria-label="Close ${ref}" title="Close  esc">${ic('close')}</button></div>
    <h2 tabindex="-1" data-k="insp-title">${b.title}</h2>
    <div class="meta"><span class="ref">${ref}</span><span>${b.project}</span><span>${state}</span></div>
  </div>
  <div class="insp-b">${b.miss ? missBlock(b) : b.holdsPr ? attn('bad', `${ic('failed', 'i c-error')}Holds PR #93 in ${b.goal}`, '<p>Lens review blocked the merge until this is fixed.</p>', `${btn('Make a task', 'gone', 'B44|Made T25 from B44', 'pri')}${btn('Open PR #93', 'toast', 'Opened PR #93 in the browser')}${btn('Won’t do', 'gone', 'B44|Closed B44 as won’t do', 'plain')}`) : triage(ref)}
    <section aria-label="How it was reported"><h3 class="sub">How it was reported</h3><p>${b.miss ? 'The goal check found it when G9 finished wave 2.' : 'Terminal webapp-2 reported it while working on <a data-act="open" data-arg="T6" data-uh-ignore-target>T6 Cache session lookups</a>.'}</p>
      <blockquote><q>${b.miss ? 'No test or log shows a reset link refused after 15 minutes. The expiry code exists in T22 but nothing proves it.' : 'Signing out leaves the session cookie in place until the browser closes. Found while testing the cache.'}</q></blockquote></section>
    <section aria-label="What was happening"><h3 class="sub">What was happening</h3><dl class="kv"><dt>Task</dt><dd>${b.miss ? 'T23 Reset flow end-to-end test' : 'T6 Cache session lookups'}</dd><dt>Step</dt><dd>${b.miss ? 'Goal check' : 'Running the sign-out test'}</dd><dt>Branch</dt><dd class="mono">${b.miss ? 't23-reset-flow-e2e' : 't6-cache-session-lookups'}</dd><dt>Last commit</dt><dd><span class="mono">c71e2aa</span> · 2 uncommitted</dd></dl>
      <div class="btnrow">${btn('Open its log at that time', 'open', b.miss ? 'T23' : 'T6', 'plain')}</div></section>
    <section aria-label="History"><h3 class="sub">History</h3><ol class="log"><li><span>${b.reported.split(', ').pop()}</span><span>report</span><span>${b.miss ? 'goal check' : 'webapp-2'} · reported it</span></li><li><span>4:20 PM</span><span>seen</span><span>you · opened it</span></li></ol></section>
  </div>`;
}
function triage(ref) {
  return `<section class="tri" aria-label="Decide"><div class="btnrow">${btn('Make a task', 'gone', `${ref}|Made T26 from ${ref}`, 'pri')}${btn('Select for a goal', 'pick', ref)}${btn('Make a Jira ticket', 'gone', `${ref}|Made WEB-312 from ${ref}`)}${btn('Defer', 'gone', `${ref}|Deferred ${ref}`, 'plain')}${btn('Won’t do', 'gone', `${ref}|Closed ${ref} as won’t do`, 'plain')}</div></section>`;
}
function missBlock(b) {
  const ch = [['Make a task', 'G9 stays open until the task is done.', 'gone', 'B41|Made T24 from B41 · open it', 'pri'], ['Send to Jira', 'G9 finishes with this outcome unmet.', 'dlg', 'finishgoal'], ['Won’t do', 'G9 finishes with this outcome unmet.', 'dlg', 'finishgoal'], ['Finish without it', 'B41 stays in the backlog; G9 finishes.', 'dlg', 'finishgoal']];
  return `<section class="attn" aria-label="Outcome missed"><h3>${ic('needs', 'i c-needs')}The last miss holding G9 Password reset emails open</h3>
    <p>G9 is done when <q>Reset links arrive within a minute and expire after 15 minutes.</q></p>
    <ul class="choices">${ch.map(([l, d, a, arg, c]) => `<li><button class="btn ${c || ''}" data-act="${a}" data-arg="${arg}">${l}</button><span class="var">${d}</span></li>`).join('')}</ul></section>`;
}

// ── Panel popovers: attachments and a reviewer's load ──
const _popover = popover;
popover = function (id) {
  if (id === 'att') return `<div class="pop wide" role="dialog" aria-label="Attachments" data-k="pop"><h3>Attachments</h3><ul class="att">${[['img', 'Design', 'Passkey page design', 'G1', '2 d', 'Three states of the passkey list'], ['file', 'Proposal', 'Last-passkey rule', 'T3', '1 h', 'Options for the re-check'], ['file', 'Evidence', 'Load test before', 'T16', 'Tue', 'p95 610 ms on staging']].slice(0, S.panel === 'T6' ? 3 : 2).map(([i, k, t, on, age, sum]) => `<li>${ic(i)}<div><b>${t}</b><span class="lbl-s">${k} · on ${on} · ${age} · ${sum}</span></div><button class="ib" data-act="toast" data-arg="Opened ${t}" aria-label="Open ${t}" title="Open">${ic('ext')}</button><button class="ib" data-act="copy" data-arg="the link to ${t}" aria-label="Copy the link to ${t}" title="Copy link">${ic('link')}</button></li>`).join('')}</ul></div>`;
  if (id === 'rev') return `<div class="pop wide" role="dialog" aria-label="Sam Ortiz’s reviews" data-k="pop"><h3>Sam Ortiz · <span class="mono">sortiz</span></h3><span class="lbl-s">3 open reviews · usually 2 h · last reviewed 10:40 AM</span>
    <ul class="att"><li>${ic('pr')}<div><b>acme/webapp#405 · Passkey copy</b><span class="lbl-s">Waited 3 h 10 min of work time · swaps in 20 min · T2 in G1</span></div></li><li>${ic('pr')}<div><b>acme/api#88 · Key usage counters</b><span class="lbl-s">Changes addressed 9:30 AM · re-review</span></div></li></ul>
    <details><summary>Reviewed this week · 6</summary><ul class="att"><li><div><b>Approved #401</b><span class="lbl-s">in 1 h 20 min · Wed</span></div></li></ul></details></div>`;
  return _popover(id);
};
