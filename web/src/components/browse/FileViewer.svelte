<!--
  Read-only file viewer for the repository browser. Fetches the
  file at (commit, path), runs it through the same Shiki pipeline
  the diff renderer uses, and displays one line per row with
  syntax-highlighted spans.

  Binary files come back with `binary: true`; we render a
  placeholder instead of trying to display arbitrary bytes.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import { ApiError, api } from '../../lib/api';
  import type { CommitId } from '../../lib/types';
  import {
    langForPath,
    loadLang,
    tokenizeWholeFile,
    themeState,
    type LineHighlights,
  } from '../../lib/highlight.svelte';
  import { SvelteMap } from 'svelte/reactivity';

  let {
    repo,
    commit,
    path,
    onclose,
  }: {
    repo: string;
    commit: CommitId;
    path: string;
    onclose: () => void;
  } = $props();

  type Content =
    | { kind: 'loading' }
    | { kind: 'binary'; size: number }
    | { kind: 'text'; content: string; size: number }
    | { kind: 'error'; message: string };

  let content: Content = $state({ kind: 'loading' });
  /** 1-based line number → rendered HTML span list. Empty during
   *  initial load; the Shiki worker fills it in. */
  let highlights: LineHighlights = $state(new SvelteMap());

  const lines = $derived.by(() => {
    if (content.kind !== 'text') return [] as string[];
    // Trailing newline produces an empty final entry; drop it so
    // the line-count matches `wc -l + 1` semantics that users
    // expect (one terminal line per actual line of code).
    const parts = content.content.split('\n');
    if (parts.length > 0 && parts[parts.length - 1] === '') parts.pop();
    return parts;
  });

  async function load() {
    content = { kind: 'loading' };
    highlights = new SvelteMap();
    try {
      const body = await api.browseFile(repo, commit, path);
      if (body.binary) {
        content = { kind: 'binary', size: body.size };
        return;
      }
      content = { kind: 'text', content: body.content, size: body.size };
      // Kick off highlighting. Languages we don't recognise render
      // as plain text; the viewer falls back to the unstyled span.
      const lang = langForPath(path);
      if (!lang) return;
      // Take snapshots before async hops so the closure doesn't
      // reactively recapture on every change.
      const text = body.content;
      const target = highlights;
      const h = await loadLang(lang);
      // Re-tokenize on theme change. Tracked via `themeState.value`
      // so a colour-scheme flip updates the colours.
      void themeState.value;
      await tokenizeWholeFile(h, text, lang, target);
    } catch (e) {
      content = {
        kind: 'error',
        message: e instanceof ApiError ? e.message : String(e),
      };
    }
  }

  $effect(() => {
    // Re-tokenize when the theme flips. The closure tracks
    // `themeState.value` so the effect re-runs on dark/light swap.
    void themeState.value;
    void load();
  });

  // Reload on (commit, path) change. Untrack themeState so this
  // effect doesn't double-fire with the theme effect above.
  $effect(() => {
    void repo;
    void commit;
    void path;
    untrack(() => load());
  });
</script>

<section class="file-viewer">
  <header class="viewer-header">
    <span class="path"><code>{path}</code></span>
    <span class="meta">
      {#if content.kind === 'text'}
        {lines.length} line{lines.length === 1 ? '' : 's'} ·
        {formatBytes(content.size)}
      {:else if content.kind === 'binary'}
        binary · {formatBytes(content.size)}
      {/if}
    </span>
    <button type="button" class="close" onclick={onclose}>Close</button>
  </header>
  {#if content.kind === 'loading'}
    <p class="status muted">Loading…</p>
  {:else if content.kind === 'error'}
    <p class="status error"><strong>Couldn't load:</strong> {content.message}</p>
  {:else if content.kind === 'binary'}
    <p class="status muted">
      Binary file ({formatBytes(content.size)}) — not rendered.
    </p>
  {:else}
    <div class="content">
      {#each lines as line, i (i)}
        {@const n = i + 1}
        {@const h = highlights.get(n)}
        <div class="line">
          <span class="line-num">{n}</span>
          <span class="code">
            {#if h}{@html h}{:else}{line}{/if}
          </span>
        </div>
      {/each}
    </div>
  {/if}
</section>

<script lang="ts" module>
  function formatBytes(n: number): string {
    if (n < 1024) return `${n} B`;
    if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
    return `${(n / 1024 / 1024).toFixed(1)} MB`;
  }
</script>

<style>
  .file-viewer {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
    background: var(--bg);
  }

  .viewer-header {
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 6px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-panel);
    font-size: 12px;
  }

  .viewer-header .path {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .viewer-header .path code {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
  }

  .viewer-header .meta {
    color: var(--text-muted);
  }

  .viewer-header .close {
    padding: 2px 10px;
    font-size: 12px;
  }

  .status {
    padding: 16px;
    margin: 0;
  }

  .content {
    overflow: auto;
    flex: 1;
    min-height: 0;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
    line-height: 1.5;
  }

  .line {
    display: flex;
    align-items: baseline;
    gap: 12px;
    padding: 0 12px;
    white-space: pre;
  }

  .line:hover {
    background: var(--bg-panel);
  }

  .line-num {
    color: var(--text-faint);
    text-align: right;
    user-select: none;
    flex: 0 0 auto;
    min-width: 4ch;
    font-variant-numeric: tabular-nums;
  }

  .code {
    flex: 1;
    min-width: 0;
    /* Long lines wrap so they're readable without horizontal scroll
     * — same default as the diff renderer. */
    white-space: pre-wrap;
    word-break: break-word;
  }
</style>
