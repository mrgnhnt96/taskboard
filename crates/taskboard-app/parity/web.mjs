// Loads the frozen web board (parity/web/*.js, the UI Taskboard.app replaced) into a sandbox so its
// own functions can produce the expected values for the app's parity tests.
//
// Time is frozen at NOW and the zone is UTC (regen.sh runs node with TZ=UTC), matching
// `crate::parity` on the Rust side. The DOM is a do-nothing stub: only pure helpers are called.
import fs from 'node:fs';
import vm from 'node:vm';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const NOW = '2026-10-07T15:00:00.000Z'; // a Wednesday
const here = path.dirname(fileURLToPath(import.meta.url));

export function web() {
  const noop = new Proxy(function () {}, {
    get: (t, k) => (k === Symbol.toPrimitive ? () => '' : noop),
    apply: () => noop,
    construct: () => noop,
  });
  const fixed = Date.parse(NOW);
  class FixedDate extends Date {
    constructor(...a) { if (a.length === 0) super(fixed); else super(...a); }
    static now() { return fixed; }
  }
  const store = {};
  const ctx = {
    console, Math, JSON, Intl, URLSearchParams, Set, Map, Promise, Number, String, Array, Object, RegExp, isNaN,
    Date: FixedDate,
    setTimeout: () => 0, clearTimeout: () => {}, setInterval: () => 0, clearInterval: () => {},
    document: noop, window: noop, location: { hash: '', search: '' }, history: noop, navigator: noop,
    localStorage: { getItem: k => store[k] ?? null, setItem: (k, v) => { store[k] = String(v); }, removeItem: k => { delete store[k]; } },
    fetch: () => new Promise(() => {}), requestAnimationFrame: () => 0,
    matchMedia: () => ({ matches: false, addEventListener() {} }),
    addEventListener: () => {}, getComputedStyle: () => noop, innerWidth: 1400, innerHeight: 900,
  };
  ctx.globalThis = ctx;
  ctx.self = ctx;
  vm.createContext(ctx);
  for (const f of ['app.js', 'pages.js', 'waves.js', 'sessions.js']) {
    vm.runInContext(fs.readFileSync(path.join(here, 'web', f), 'utf8'), ctx, { filename: f });
  }
  // `run("expr")` evaluates in the page's scope (its `let`/`const` globals such as S are only
  // reachable this way); `set(name, value)` assigns a page global first.
  return {
    run: src => vm.runInContext(src, ctx),
    call: (fn, ...args) => {
      ctx.__args = JSON.parse(JSON.stringify(args));
      return vm.runInContext(`${fn}(...__args)`, ctx);
    },
    set: (name, value) => {
      ctx.__v = JSON.parse(JSON.stringify(value));
      vm.runInContext(`${name} = __v`, ctx);
    },
  };
}

/** The visible text of an HTML fragment, whitespace collapsed (what the reader sees). */
export function text(html) {
  return String(html ?? '')
    .replace(/<(script|style)[\s\S]*?<\/\1>/g, '')
    .replace(/<br\s*\/?>/g, '\n')
    .replace(/<[^>]+>/g, ' ')
    .replace(/&amp;/g, '&').replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&#39;/g, "'")
    .replace(/[ \t]+/g, ' ')
    .replace(/ *\n */g, '\n')
    .trim();
}

/** Values of every `data-act` in an HTML fragment, in order (which buttons are offered). */
export function acts(html) {
  return [...String(html ?? '').matchAll(/data-act="([^"]+)"/g)].map(m => m[1]);
}

/** Write golden/<name>.json: {"cases": [{"name", "input", "expect"}...]} */
export function writeGolden(name, cases) {
  const out = path.join(here, 'golden', `${name}.json`);
  fs.writeFileSync(out, JSON.stringify({ now: NOW, cases }, null, 2) + '\n');
  console.log(`wrote ${path.relative(process.cwd(), out)} (${cases.length} cases)`);
}
