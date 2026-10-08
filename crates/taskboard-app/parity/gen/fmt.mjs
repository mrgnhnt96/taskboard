// Shared text helpers (app.js): ago, span, whenLine, clock12, plural, ref.
import { web, writeGolden, NOW } from '../web.mjs';
const w = web();
const at = mins => new Date(Date.parse(NOW) - mins * 60000).toISOString();
const cases = [];
for (const m of [0, 0.4, 1, 59, 60, 61, 90, 60 * 23, 60 * 24, 60 * 24 * 3]) cases.push({ name: `ago ${m}m`, input: { fn: 'ago', iso: at(m) }, expect: w.call('ago', at(m)) });
for (const m of [0, 1, 59, 60, 61, 125, 60 * 24, 60 * 24 + 90, 60 * 49]) cases.push({ name: `span ${m}`, input: { fn: 'span', m }, expect: w.call('span', m) });
for (const h of ['00:00', '09:00', '09:30', '12:00', '12:05', '15:00', '23:30']) cases.push({ name: `clock12 ${h}`, input: { fn: 'clock12', hhmm: h }, expect: w.call('clock12', h) });
const tasks = [
  { status: 'queued', when: at(5) }, { status: 'planned', when: at(120) }, { status: 'working', when: at(1) },
  { status: 'needs', lost: true, when: at(3) }, { status: 'needs', question: 'q?', when: at(3) }, { status: 'needs', when: at(3) },
  { status: 'done', failed: true, when: at(60 * 30) }, { status: 'done', when: at(60 * 30) }, { status: 'done', updated_at: at(2) },
  { status: 'done' },
];
tasks.forEach((t, i) => cases.push({ name: `whenLine ${i} ${t.status}`, input: { fn: 'whenLine', task: t }, expect: w.call('whenLine', t) }));
for (const [x, p] of [[{ id: 4 }, 'T'], [{ ref: 'G2', id: 2 }, 'G'], [7, 'B'], ['t9', 'T'], ['', 'T'], [null, 'T']]) cases.push({ name: `ref ${JSON.stringify(x)}`, input: { fn: 'ref', x, p }, expect: w.call('ref', x, p) });
writeGolden('fmt', cases);
