// ── Sample data (names from the inventory's sample board) ──
const ICONS = {
  needs: '<circle cx="8" cy="8" r="6.25"/><path d="M8 4.75v3.6M8 11v.25"/>',
  failed: '<circle cx="8" cy="8" r="6.25"/><path d="M5.9 5.9l4.2 4.2M10.1 5.9l-4.2 4.2"/>',
  working: '<circle cx="8" cy="8" r="6.25"/><path d="M8 1.75a6.25 6.25 0 0 1 0 12.5z" fill="currentColor"/>',
  review: '<circle cx="8" cy="8" r="6.25"/><circle cx="8" cy="8" r="2.4" fill="currentColor" stroke="none"/>',
  done: '<circle cx="8" cy="8" r="6.25"/><path d="M5.3 8.2l1.8 1.8 3.6-3.8"/>',
  stopped: '<circle cx="8" cy="8" r="6.25"/><rect x="5.6" y="5.6" width="4.8" height="4.8" rx=".6" fill="currentColor" stroke="none"/>',
  queued: '<circle cx="8" cy="8" r="6.25" stroke-dasharray="2.6 2.3"/>',
  planned: '<circle cx="8" cy="8" r="6.25" stroke-dasharray="1 2.6"/>',
  paused: '<circle cx="8" cy="8" r="6.25"/><path d="M6.4 5.5v5M9.6 5.5v5"/>',
  quiet: '<circle cx="8" cy="8" r="6.25"/><path d="M5.2 8h5.6"/>',
  side: '<rect x="2" y="3" width="12" height="10" rx="2"/><path d="M6 3v10"/>',
  insp: '<rect x="2" y="3" width="12" height="10" rx="2"/><path d="M10 3v10"/>',
  back: '<path d="M10 3.5L5.5 8l4.5 4.5"/>',
  fwd: '<path d="M6 3.5L10.5 8 6 12.5"/>',
  chev: '<path d="M6 3.5L10.5 8 6 12.5"/>',
  down: '<path d="M4 6l4 4 4-4"/>',
  search: '<circle cx="7" cy="7" r="4.25"/><path d="M10.2 10.2l3.3 3.3"/>',
  plus: '<path d="M8 3v10M3 8h10"/>',
  more: '<circle cx="3.5" cy="8" r=".9" fill="currentColor"/><circle cx="8" cy="8" r=".9" fill="currentColor"/><circle cx="12.5" cy="8" r=".9" fill="currentColor"/>',
  clock: '<circle cx="8" cy="8" r="6"/><path d="M8 4.6V8l2.4 1.5"/>',
  close: '<path d="M4.5 4.5l7 7M11.5 4.5l-7 7"/>',
  copy: '<rect x="5.5" y="5.5" width="7.5" height="7.5" rx="1.5"/><path d="M3 10.5V4a1 1 0 0 1 1-1h6.5"/>',
  term: '<rect x="2" y="3" width="12" height="10" rx="2"/><path d="M5 7l2 1.5L5 10M8.5 10.5h3"/>',
  ext: '<path d="M9.5 2.5h4v4M13.5 2.5L8 8M11.5 9.5v4h-9v-9h4"/>',
  inbox: '<path d="M2.5 9.5l1.8-5.5h7.4l1.8 5.5V13h-11z"/><path d="M2.5 9.5H6l.8 1.5h2.4l.8-1.5h3.5"/>',
  goal: '<circle cx="8" cy="8" r="6"/><circle cx="8" cy="8" r="3"/><circle cx="8" cy="8" r=".7" fill="currentColor"/>',
  backlog: '<path d="M2.5 5.5L8 2.5l5.5 3L8 8.5z"/><path d="M2.5 8.5L8 11.5l5.5-3M2.5 11L8 14l5.5-3"/>',
  idea: '<path d="M6 12.3h4M6.6 14h2.8M8 2a4 4 0 0 0-2.5 7.1c.6.5 1 1.2 1 1.9h3c0-.7.4-1.4 1-1.9A4 4 0 0 0 8 2z"/>',
  project: '<path d="M2 4.5a1 1 0 0 1 1-1h3l1.5 1.5H13a1 1 0 0 1 1 1V12a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1z"/>',
  days: '<rect x="2.5" y="3.5" width="11" height="10" rx="1.5"/><path d="M2.5 6.5h11M5.5 2v3M10.5 2v3"/>',
  release: '<path d="M2.5 2.5h5l6 6-5 5-6-6z"/><circle cx="5.5" cy="5.5" r=".9" fill="currentColor"/>',
  gear: '<circle cx="8" cy="8" r="2.2"/><path d="M8 1.8v1.7M8 12.5v1.7M1.8 8h1.7M12.5 8h1.7M3.6 3.6l1.2 1.2M11.2 11.2l1.2 1.2M3.6 12.4l1.2-1.2M11.2 4.8l1.2-1.2"/>',
  pulse: '<path d="M1.5 8.5h3l1.5-3.5 3 7 1.5-3.5h4"/>',
  gauge: '<path d="M2.5 11.5a5.5 5.5 0 1 1 11 0"/><path d="M8 11.5l2.6-3.4"/>',
  moon: '<path d="M13 9.6A5.5 5.5 0 1 1 6.4 3a4.4 4.4 0 0 0 6.6 6.6z"/>',
  talk: '<path d="M3 3.5h10a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H7.2L4 14v-2.5H3a1 1 0 0 1-1-1v-6a1 1 0 0 1 1-1z"/>',
  check: '<path d="M3.5 8.5l3 3 6-6.5"/>',
  warn: '<path d="M8 2.2L14.2 13H1.8z"/><path d="M8 6.5v3M8 11.2v.2"/>',
  file: '<path d="M4 1.8h5l3 3v9.4H4z"/><path d="M9 1.8v3h3"/>',
  img: '<rect x="2" y="3" width="12" height="10" rx="1.5"/><circle cx="6" cy="6.5" r="1.2"/><path d="M2.5 12l3.5-3.5 2.5 2.5 2-2 3 3"/>',
  branch: '<circle cx="4.5" cy="3.5" r="1.5"/><circle cx="4.5" cy="12.5" r="1.5"/><circle cx="11.5" cy="5.5" r="1.5"/><path d="M4.5 5v6M11.5 7c0 3-7 2-7 4"/>',
  pr: '<circle cx="4.5" cy="3.5" r="1.5"/><circle cx="4.5" cy="12.5" r="1.5"/><circle cx="11.5" cy="12.5" r="1.5"/><path d="M4.5 5v6M11.5 11V6.5a2 2 0 0 0-2-2H7.5M8.8 3.2L7.5 4.5l1.3 1.3"/>',
  play: '<path d="M5 3.5v9l7-4.5z"/>',
  pause: '<path d="M5.5 3.5v9M10.5 3.5v9"/>',
  undo: '<path d="M5.5 3.5L2.5 6.5l3 3"/><path d="M2.5 6.5h7a4 4 0 0 1 0 8H7"/>',
  link: '<path d="M6.8 9.2a2.6 2.6 0 0 0 3.7 0l2.2-2.2a2.6 2.6 0 0 0-3.7-3.7l-.9.9M9.2 6.8a2.6 2.6 0 0 0-3.7 0L3.3 9a2.6 2.6 0 0 0 3.7 3.7l.9-.9"/>',
  filter: '<path d="M2.5 3.5h11l-4.3 5v4l-2.4 1.2V8.5z"/>',
  red: '<circle cx="8" cy="8" r="6.25"/><path d="M8 4.75v3.6M8 11v.25"/>'
};
const ic = (n, cls = 'i', label) => `<svg class="${cls}" viewBox="0 0 16 16" ${label ? `role="img" aria-label="${label}"` : 'aria-hidden="true"'}>${ICONS[n] || ''}</svg>`;

// status vocabulary: glyph + words, colour only as a second cue
const ST = {
  needs: ['needs', 'c-needs', 'Needs you'],
  failed: ['failed', 'c-error', 'Failed'],
  working: ['working', 'c-working', 'Working'],
  review: ['review', 'c-review', 'In review'],
  done: ['done', 'c-done', 'Done'],
  stopped: ['stopped', 'c-quiet', 'Stopped'],
  queued: ['queued', 'c-quiet', 'Queued'],
  planned: ['planned', 'c-quiet', 'Planned'],
  paused: ['paused', 'c-quiet', 'Paused'],
  quiet: ['quiet', 'c-needs', 'Gone quiet'],
  lost: ['stopped', 'c-error', 'Terminal lost'],
  nostart: ['failed', 'c-error', "Couldn't start"],
  human: ['needs', 'c-needs', 'Your to-do'],
  merge: ['review', 'c-review', 'Ready to merge']
};
const st = (k, words) => { const s = ST[k] || ST.queued; return `<span class="st ${s[1]}">${ic(s[0], 'i g')}${words || s[2]}</span>`; };
const glyph = (k) => { const s = ST[k] || ST.queued; return `<span class="${s[1]}" title="${s[2]}">${ic(s[0], 'i', s[2])}</span>`; };

const PROJECTS = [
  { id: 'webapp', path: '~/Development/webapp', needs: 7, goals: 4, backlog: 15, ideas: 6 },
  { id: 'api', path: '~/Development/api', needs: 3, goals: 3, backlog: 8, ideas: 3 }
];

const GOALS = {
  G1: { ref: 'G1', name: 'Sign-in with passkeys', project: 'webapp', state: 'needs', stateWords: 'Waiting on you', done: 1, total: 5, wave: '1 of 2', updated: '9 min ago',
    tldr: 'People can register a passkey and use it to sign in; passwords stay as a fallback.',
    split: { done: 1, working: 0, needs: 2, review: 0, planned: 2 },
    now: '1 needs you · 1 to-do',
    waves: [
      { n: 1, name: 'Server and screens', phase: 'now', tasks: ['T1', 'T2', 'T3'] },
      { n: 2, name: 'Polish', phase: 'next', tasks: ['H1', 'T4'] }
    ] },
  G2: { ref: 'G2', name: 'Faster sign-in', project: 'webapp', state: 'working', stateWords: 'Running', done: 2, total: 7, wave: '2 of 3', updated: '2 min ago',
    tldr: 'Sign-in takes under 400 ms at p95, measured on the staging load test.',
    split: { done: 2, working: 1, review: 1, needs: 1, planned: 2 },
    now: '1 working · 1 in review · 1 terminal lost',
    waves: [
      { n: 1, name: 'Measure', phase: 'done', when: 'done Tue 3:10 PM', tasks: ['T13', 'T16'] },
      { n: 2, name: 'Speed up', phase: 'now', when: 'now · stops for your review after', tasks: ['T6', 'T10', 'T5'] },
      { n: 3, name: 'Check', phase: 'next', tasks: ['T14', 'T17'] }
    ], reviewStop: 2 },
  G3: { ref: 'G3', name: 'Rate limits per key', project: 'api', state: 'failed', stateWords: 'Stopped · a task failed', done: 1, total: 5, wave: '1 of 2', updated: '34 min ago',
    tldr: 'Each API key gets its own request budget, with Retry-After on every 429.',
    split: { done: 1, failed: 1, stopped: 1, planned: 2 },
    now: '1 failed · 1 couldn’t start',
    stop: { kind: 'failed', title: 'T7 failed in wave 1, so wave 2 waits', detail: 'Try it again from its panel, or let wave 2 start without it.', primary: ['Open T7', 'open:T7'], secondary: ['Continue anyway', 'Wave 2 can start now.'] },
    waves: [
      { n: 1, name: 'Budget per key', phase: 'failed', when: 'a task failed', tasks: ['T18', 'T7', 'T11'] },
      { n: 2, name: 'Headers and docs', phase: 'next', when: 'waits on wave 1', tasks: ['T9', 'T19'] }
    ] },
  G9: { ref: 'G9', name: 'Password reset emails', project: 'api', state: 'needs', stateWords: 'Stopped · outcome missed', done: 4, total: 4, wave: '2 of 2', updated: '1 h ago',
    tldr: 'Reset links arrive within a minute and expire after 15 minutes.',
    split: { done: 4 }, now: '1 missed',
    stop: { kind: 'missed', title: 'One outcome was missed and holds the goal open', detail: '“Reset links expire after 15 minutes” wasn’t proven.', primary: ['See the miss', 'open:B41'] },
    waves: [{ n: 1, name: 'Send', phase: 'done', when: 'done Wed 11:20 AM', tasks: ['T20', 'T21'] }, { n: 2, name: 'Expire', phase: 'done', when: 'done Thu 2:05 PM', tasks: ['T22', 'T23'] }] },
  G6: { ref: 'G6', name: 'Offline mode groundwork', project: 'webapp', shelf: 'next', total: 6, after: 'Starts tomorrow, 9:00 AM' },
  G8: { ref: 'G8', name: 'Session audit trail', project: 'api', shelf: 'next', total: 4, after: 'Starts after G3 Rate limits per key' },
  G4: { ref: 'G4', name: 'Billing export', project: 'api', shelf: 'paused', done: 3, total: 7, resume: 'Resume · 4 tasks start' },
  G7: { ref: 'G7', name: 'Legacy cookie cleanup', project: 'webapp', shelf: 'aside', done: 1, total: 4 },
  G5: { ref: 'G5', name: 'Audit log', project: 'api', shelf: 'finished', prs: '5 PRs merged', when: 'Wed 4:20 PM' },
  G10: { ref: 'G10', name: 'Upgrade build tooling', project: 'webapp', shelf: 'finished', prs: '3 PRs merged · 1 failed, accepted', when: 'Mon 10:05 AM' }
};
const ACTIVE = ['G3', 'G9', 'G1', 'G2'];

const T = (o) => o;
const TASKS = {
  T1: T({ ref: 'T1', title: 'Passkey registration endpoint', goal: 'G1', project: 'webapp', status: 'done', line: 'Merged #402 · 10:12 AM', pr: '#402 merged' }),
  T2: T({ ref: 'T2', title: 'Passkey sign-in screen', goal: 'G1', project: 'webapp', status: 'working', line: 'Working 42 min · wiring the browser prompt', pr: 'No PR yet' }),
  T3: T({ ref: 'T3', title: 'Passkey management page', goal: 'G1', project: 'webapp', status: 'needs', line: 'Asked 9 min ago', age: '9m', attn: 'question', pr: 'No PR yet' }),
  T4: T({ ref: 'T4', title: 'Passkey help article', goal: 'G1', project: 'webapp', status: 'planned', line: 'Planned · wave 2', pr: '' }),
  H1: T({ ref: 'H1', title: 'Add the passkey RP ID to the production config', goal: 'G1', project: 'webapp', status: 'human', line: 'Waiting on you · 2 h', age: '2h', attn: 'human', human: true }),
  T5: T({ ref: 'T5', title: 'Profile the token refresh path', goal: 'G2', project: 'webapp', status: 'quiet', line: 'No update for 40 minutes', age: '', attn: 'stuck', pr: 'No PR yet' }),
  T6: T({ ref: 'T6', title: 'Cache session lookups', goal: 'G2', project: 'webapp', status: 'review', line: 'PR #412 waits on your review', age: '25m', attn: 'ownerreview', pr: '#412 · your review', hasPr: true }),
  T10: T({ ref: 'T10', title: 'Move sessions to Redis', goal: 'G2', project: 'webapp', status: 'lost', line: 'Terminal lost · the work looks finished', age: '48m', attn: 'lost', pr: '#409 open' }),
  T13: T({ ref: 'T13', title: 'Trace the sign-in path', goal: 'G2', project: 'webapp', status: 'done', line: 'Merged #398 · Tue 2:40 PM', pr: '#398 merged' }),
  T16: T({ ref: 'T16', title: 'Baseline load test', goal: 'G2', project: 'webapp', status: 'done', line: 'Done Tue 3:10 PM · no PR (research)', pr: 'No PR · research' }),
  T14: T({ ref: 'T14', title: 'Load test sign-in again', goal: 'G2', project: 'webapp', status: 'planned', line: 'Planned · wave 3', pr: '' }),
  T17: T({ ref: 'T17', title: 'Write up the results', goal: 'G2', project: 'webapp', status: 'planned', line: 'Planned · wave 3', pr: '' }),
  T7: T({ ref: 'T7', title: 'Per-key token bucket', goal: 'G3', project: 'api', status: 'failed', line: 'Failed 34 min ago · tests fail under load', age: '34m', attn: 'failed', pr: 'No PR · failed' }),
  T11: T({ ref: 'T11', title: 'Retry-After on every 429', goal: 'G3', project: 'api', status: 'nostart', line: "Couldn't start · Midna is at its terminal limit", age: '20m', attn: 'nostart', pr: '' }),
  T18: T({ ref: 'T18', title: 'Key usage counters', goal: 'G3', project: 'api', status: 'done', line: 'Merged #88 · 8:40 AM', pr: '#88 merged' }),
  T9: T({ ref: 'T9', title: 'Rate-limit response headers', goal: 'G3', project: 'api', status: 'planned', line: 'Waits on wave 1', pr: '' }),
  T19: T({ ref: 'T19', title: 'Document rate limits', goal: 'G3', project: 'api', status: 'planned', line: 'Waits on wave 1', pr: '' }),
  T8: T({ ref: 'T8', title: 'Upgrade the HTTP client', goal: null, project: 'api', status: 'queued', line: 'Queued · not started', attn: 'start', pr: '' }),
  T12: T({ ref: 'T12', title: 'Export audit events as CSV', goal: 'G5', project: 'api', status: 'done', line: 'Merged #91 without a tester note', attn: 'note', pr: '#91 merged', hasPr: true }),
  T20: T({ ref: 'T20', title: 'Reset email template', goal: 'G9', project: 'api', status: 'done', line: 'Merged #80', pr: '#80 merged' }),
  T21: T({ ref: 'T21', title: 'Send reset email from the queue', goal: 'G9', project: 'api', status: 'done', line: 'Merged #81', pr: '#81 merged' }),
  T22: T({ ref: 'T22', title: 'Reset token expiry', goal: 'G9', project: 'api', status: 'done', line: 'Merged #84', pr: '#84 merged' }),
  T23: T({ ref: 'T23', title: 'Reset flow end-to-end test', goal: 'G9', project: 'api', status: 'done', line: 'Merged #85', pr: '#85 merged' })
};

const ISSUES = {
  B41: { ref: 'B41', kind: 'Outcome missed', title: 'Reset links expire after 15 minutes wasn’t proven', project: 'api', goal: 'G9', miss: true, area: 'Auth', impact: 3, pri: 'P1', isNew: true, reported: 'Thu, Nov 19, 4:12 PM' },
  B44: { ref: 'B44', kind: 'Bug', title: 'Review found an SQL string built from input', project: 'api', goal: 'G3', holdsPr: '#93', area: 'Storage', impact: 3, pri: 'P1', isNew: true, reported: 'Today, 10:02 AM' },
  B37: { ref: 'B37', kind: 'Bug', title: 'Session cookie isn’t cleared on sign-out', project: 'webapp', area: 'Auth', impact: 3, pri: 'P1', isNew: true, group: 'Sign-out leaves state behind', reported: 'Today, 9:40 AM' },
  B45: { ref: 'B45', kind: 'Bug', title: 'Sign-out leaves the passkey prompt open in other tabs', project: 'webapp', area: 'Auth', impact: 2, pri: 'P2', isNew: true, group: 'Sign-out leaves state behind', reported: 'Today, 10:50 AM' },
  B38: { ref: 'B38', kind: 'Test gap', title: 'No test for an expired passkey challenge', project: 'webapp', area: 'Auth', impact: 2, pri: 'P2', group: 'Sign-out leaves state behind', reported: 'Thu, Nov 19, 2:15 PM' },
  B39: { ref: 'B39', kind: 'Follow-up', title: 'Split the auth middleware into two layers', project: 'api', area: 'Auth', impact: 2, pri: 'P2', group: 'Auth middleware', reported: 'Wed, Nov 18, 5:01 PM' },
  B43: { ref: 'B43', kind: 'Test gap', title: 'Rate-limit test is flaky on slow runners', project: 'api', area: 'CI', impact: 2, pri: 'P2', group: 'Auth middleware', reported: 'Wed, Nov 18, 11:30 AM' },
  B40: { ref: 'B40', kind: 'Clean-up', title: 'Remove the old OAuth callback route', project: 'webapp', area: 'Auth', impact: 1, pri: 'P3', group: 'Not grouped yet', reported: 'Mon, Nov 16, 9:12 AM' },
  B42: { ref: 'B42', kind: 'Clean-up', title: 'README links to a page that no longer exists', project: 'api', area: 'Docs', impact: 1, pri: 'P3', group: 'Not grouped yet', reported: 'Mon, Nov 16, 8:55 AM' }
};

const IDEAS = [
  { ref: 'I2', title: 'Offline mode', project: 'webapp', status: 'Fits', note: '2 questions', when: 'Today, 8:15 AM', wait: true },
  { ref: 'I4', title: 'Keyboard shortcuts sheet', project: 'webapp', status: 'Fits', note: 'Ready to plan', when: 'Yesterday, 4:40 PM', wait: true },
  { ref: 'I1', title: 'Dark theme', project: 'webapp', status: 'Digging', note: '', when: 'Today, 11:02 AM' },
  { ref: 'I3', title: 'Search across projects', project: 'api', status: 'Partly fits', note: '', when: 'Nov 17, 3:20 PM' },
  { ref: 'I7', title: 'Webhook retries with backoff', project: 'api', status: 'New', note: '', when: 'Nov 16, 9:05 AM' },
  { ref: 'I5', title: 'Passkey sign-in', project: 'webapp', status: 'Planned', note: 'Became G1', when: 'Nov 12, 10:30 AM' },
  { ref: 'I6', title: 'Rate limits per key', project: 'api', status: 'Planned', note: 'Became G3', when: 'Nov 13, 1:45 PM' }
];
const CLOSED_IDEAS = [
  { ref: 'I8', title: 'Magic-link sign-in', project: 'webapp', status: 'Won’t do', when: 'Nov 10, 2:00 PM' },
  { ref: 'I9', title: 'GraphQL gateway', project: 'api', status: 'Shelved', when: 'Nov 3, 9:30 AM' }
];
