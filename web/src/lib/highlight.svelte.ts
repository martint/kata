//! Syntax highlighting via Shiki. Languages load on demand the first time a
//! file with that extension is rendered.

import { SvelteMap } from 'svelte/reactivity';
import {
  createHighlighter,
  createJavaScriptRegexEngine,
  type BundledLanguage,
  type BundledTheme,
  type Highlighter,
} from 'shiki';

function preferredTheme(): BundledTheme {
  if (
    typeof window !== 'undefined' &&
    window.matchMedia?.('(prefers-color-scheme: dark)').matches
  ) {
    return 'github-dark';
  }
  return 'github-light';
}

/** Current theme, observable. Re-reads on OS color-scheme change so callers
 *  can re-tokenize without a page reload. */
export const themeState = $state({ value: preferredTheme() });

if (typeof window !== 'undefined' && window.matchMedia) {
  const mq = window.matchMedia('(prefers-color-scheme: dark)');
  mq.addEventListener?.('change', () => {
    themeState.value = preferredTheme();
  });
}

/** Back-compat: a constant that resolves to the current theme at call time.
 *  Use `themeState.value` directly in reactive contexts. */
export function currentTheme(): BundledTheme {
  return themeState.value;
}

let highlighterPromise: Promise<Highlighter> | null = null;
const loadedThemes = new Set<BundledTheme>();

export async function getHighlighter(): Promise<Highlighter> {
  if (!highlighterPromise) {
    // The JS regex engine is ~10x smaller than the Oniguruma WASM and
    // supports the languages we care about for diff display.
    highlighterPromise = createHighlighter({
      themes: [themeState.value],
      langs: [],
      engine: createJavaScriptRegexEngine(),
    });
    loadedThemes.add(themeState.value);
  }
  const h = await highlighterPromise;
  if (!loadedThemes.has(themeState.value)) {
    await h.loadTheme(themeState.value);
    loadedThemes.add(themeState.value);
  }
  return h;
}

const loadedLangs = new Set<string>();
const loadingLangs = new Map<string, Promise<void>>();

export async function loadLang(lang: BundledLanguage): Promise<Highlighter> {
  const h = await getHighlighter();
  if (loadedLangs.has(lang)) return h;
  if (loadingLangs.has(lang)) {
    await loadingLangs.get(lang)!;
    return h;
  }
  const p = h.loadLanguage(lang).then(() => {
    loadedLangs.add(lang);
  });
  loadingLangs.set(lang, p);
  try {
    await p;
  } finally {
    loadingLangs.delete(lang);
  }
  return h;
}

/** Map line number (1-based) → rendered `<span>` HTML for that line. */
export type LineHighlights = SvelteMap<number, string>;

/** Single-slot semaphore so only one whole-file tokenize runs at a time
 *  across the app. `codeToTokensBase` is synchronous and can pin the main
 *  thread for 50–200ms on a big file — running several in parallel just
 *  stacks that latency, and any user click that lands during a burst gets
 *  delayed proportionally. Serializing them keeps the *next* click handler
 *  always one yield away. */
let tokenizeChain: Promise<void> = Promise.resolve();
function acquireTokenizeSlot(): Promise<() => void> {
  let release!: () => void;
  const ticket = new Promise<void>((resolve) => (release = resolve));
  const wait = tokenizeChain;
  tokenizeChain = tokenizeChain.then(() => ticket);
  return wait.then(() => release);
}

/** Hold tokenization while the user has a comment composer open. Mounting
 *  the composer pushes the file-diff list down, which IntersectionObserver
 *  reads as "new files just entered the viewport buffer" — each one then
 *  queues a tokenize, and the synchronous `codeToTokensBase` call (~200-500ms
 *  per big file) blocks the user's keystrokes for 1-2s in aggregate.
 *  Pausing while composing lets the user type instantly; tokenize catches
 *  up the moment the composer closes. */
let composerOpenCount = 0;
let composerWaiters: Array<() => void> = [];

export function setTokenizationPaused(paused: boolean): void {
  if (paused) {
    composerOpenCount++;
  } else {
    composerOpenCount = Math.max(0, composerOpenCount - 1);
    if (composerOpenCount === 0) {
      const w = composerWaiters;
      composerWaiters = [];
      for (const fn of w) fn();
    }
  }
}

function whenTokenizationUnpaused(): Promise<void> {
  if (composerOpenCount === 0) return Promise.resolve();
  return new Promise((resolve) => composerWaiters.push(resolve));
}

/** Wait for the browser to be idle. Falls back to setTimeout for older
 *  browsers. We use this — not setTimeout — for the yields inside the
 *  tokenize loop: a setTimeout(0) macrotask is dispatched *before* the
 *  next paint, so it snipes the frame and we keep blocking ~1.6s on a
 *  big review. requestIdleCallback runs the work only when the browser
 *  has no input/paint to deliver. The timeout cap keeps long bursts of
 *  scrolling from starving tokenization indefinitely. */
function whenIdle(timeout = 300): Promise<void> {
  return new Promise((resolve) => {
    const w = window as unknown as {
      requestIdleCallback?: (
        cb: () => void,
        opts?: { timeout: number },
      ) => number;
    };
    if (typeof w.requestIdleCallback === 'function') {
      w.requestIdleCallback(() => resolve(), { timeout });
    } else {
      setTimeout(resolve, 0);
    }
  });
}

/** Tokenize a whole file in one pass — preserving the multi-line grammar
 *  state Shiki/TextMate needs to handle block comments, template literals,
 *  heredocs, etc. correctly. Populates `out` keyed by 1-based line number.
 *  Yields between lines so a large file doesn't lock the main thread. */
export async function tokenizeWholeFile(
  h: Highlighter,
  text: string,
  lang: BundledLanguage,
  out: LineHighlights,
  options: { isCancelled?: () => boolean; yieldEvery?: number } = {},
): Promise<void> {
  const isCancelled = options.isCancelled ?? (() => false);
  const yieldEvery = options.yieldEvery ?? 200;
  const release = await acquireTokenizeSlot();
  try {
    if (isCancelled()) return;
    // Don't hold the slot through a composer session — the user's
    // keystrokes would be queued behind this sync call (`codeToTokensBase`
    // is uninterruptible and runs 200-500ms on a big file).
    await whenTokenizationUnpaused();
    if (isCancelled()) return;
    // Wait for an idle frame before the expensive sync tokenize. A
    // setTimeout(0) here would dispatch the macrotask *before* the next
    // paint, blocking the frame we're trying to land — requestIdleCallback
    // defers until input/paint work is delivered.
    await whenIdle();
    if (isCancelled()) return;
    let lines: ReturnType<Highlighter['codeToTokensBase']>;
    try {
      lines = h.codeToTokensBase(text, { lang, theme: themeState.value });
    } catch {
      return;
    }
    for (let i = 0; i < lines.length; i++) {
      if (isCancelled()) return;
      let html = '';
      for (const t of lines[i]) {
        const style = t.color ? `color: ${t.color}` : '';
        html += `<span style="${style}">${escapeHtml(t.content)}</span>`;
      }
      out.set(i + 1, html);
      if ((i + 1) % yieldEvery === 0) {
        await whenIdle();
      }
    }
  } finally {
    release();
  }
}

const BUNDLED: Record<string, BundledLanguage> = {
  rs: 'rust',
  ts: 'typescript',
  tsx: 'tsx',
  js: 'javascript',
  jsx: 'jsx',
  mjs: 'javascript',
  cjs: 'javascript',
  py: 'python',
  rb: 'ruby',
  go: 'go',
  java: 'java',
  kt: 'kotlin',
  swift: 'swift',
  c: 'c',
  h: 'c',
  cc: 'cpp',
  cpp: 'cpp',
  hpp: 'cpp',
  cs: 'csharp',
  php: 'php',
  sh: 'shellscript',
  bash: 'shellscript',
  zsh: 'shellscript',
  ps1: 'powershell',
  svelte: 'svelte',
  vue: 'vue',
  html: 'html',
  htm: 'html',
  css: 'css',
  scss: 'scss',
  json: 'json',
  jsonc: 'jsonc',
  md: 'markdown',
  yaml: 'yaml',
  yml: 'yaml',
  toml: 'toml',
  xml: 'xml',
  sql: 'sql',
  dockerfile: 'dockerfile',
  lua: 'lua',
};

export function langForPath(path: string): BundledLanguage | null {
  const lower = path.toLowerCase();
  const base = lower.split('/').pop() ?? '';
  if (base === 'dockerfile') return 'dockerfile';
  const ext = base.includes('.') ? base.slice(base.lastIndexOf('.') + 1) : '';
  return BUNDLED[ext] ?? null;
}

export function escapeHtml(s: string): string {
  return s
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
    .replace(/'/g, '&#39;');
}
