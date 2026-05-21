<!--
  Repository browser: two-pane layout with the graph log on the
  left and the selected commit's detail on the right. The reviewer
  uses it to look at code outside the scope of any particular
  review — recent activity, named branches, ancestry of a path,
  the working copy.

  All views are read-only. Mutations (rebase, squash, abandon,
  push/fetch, edit description) deliberately don't live here — the
  reviewer's tool isn't a jj client.
-->
<script lang="ts">
  import { ApiError, api } from '../lib/api';
  import { copyText } from '../lib/clipboard';
  import { renderMarkdown } from '../lib/markdown';
  import type { CommitId, LogPage, LogRow } from '../lib/types';
  import FileViewer from './browse/FileViewer.svelte';
  import GraphLog from './browse/GraphLog.svelte';
  import RevId from './RevId.svelte';

  let {
    repo,
    initialCommit = null,
    initialChange = null,
    initialPath = null,
    initialRevset = null,
    onstate,
  }: {
    repo: string;
    initialCommit?: CommitId | null;
    /** Pre-selected change_id from the URL (`?change=…`). The
     *  browser resolves it to the current commit_id on load
     *  and then canonicalises the URL to `?commit=…`. */
    initialChange?: string | null;
    initialPath?: string | null;
    initialRevset?: string | null;
    /** Called whenever the browser's URL-relevant state changes
     *  (selected commit, file path, or revset). The shell threads
     *  this into history.replaceState so the URL stays in sync
     *  without spamming history with every click. */
    onstate?: (state: {
      commit?: string;
      path?: string;
      revset?: string;
    }) => void;
  } = $props();

  let page: LogPage | null = $state(null);
  let loading: boolean = $state(true);
  let error: string | null = $state(null);

  /** The revset currently visualised. `null` means "ask the
   *  server for its default", which is the recipe in IDEAS.md. */
  let revset: string = $state(initialRevset ?? '');
  /** The text in the search box — separated from `revset` so the
   *  page only reloads on Enter, not on every keystroke. */
  let revsetInput: string = $state(initialRevset ?? '');

  /** Anchor of the current selection — the commit shown in the
   *  detail pane. Set by a plain click on a row, or by resolving
   *  the URL's `?commit=` / `?change=` parameters at mount. */
  let selected: CommitId | null = $state(initialCommit);
  /** Extent of a *range* selection. `null` for single-row
   *  selection; set when the user shift-clicks. The range is the
   *  visual span between `selected` and `extent` in `page.rows`
   *  order — newest at top, oldest at bottom. */
  let extent: CommitId | null = $state(null);
  let detail: LogRow | null = $state(null);
  /** When set, the detail pane shows the file viewer instead of
   *  the commit detail. Clicking a file path in the commit detail
   *  populates this; Close clears it. */
  let viewingPath: string | null = $state(initialPath);

  /** Set of commit-ids in the current range. Single-row selection
   *  yields a one-element set; range yields the visual span.
   *  Computed from row indices into `page.rows` so the highlight
   *  follows the log's topological order (newest first). */
  const rangeIds = $derived.by(() => {
    if (!page || !selected) return new Set<string>();
    if (!extent) return new Set([selected]);
    const anchorIdx = page.rows.findIndex(
      (r) => r.commit.commit_id === selected,
    );
    const extentIdx = page.rows.findIndex(
      (r) => r.commit.commit_id === extent,
    );
    if (anchorIdx < 0 || extentIdx < 0) return new Set([selected]);
    const lo = Math.min(anchorIdx, extentIdx);
    const hi = Math.max(anchorIdx, extentIdx);
    return new Set(page.rows.slice(lo, hi + 1).map((r) => r.commit.commit_id));
  });
  const rangeSize = $derived(rangeIds.size);

  /** When the user has shift-extended a range, the topologically-
   *  oldest commit in it (the bottom row, highest row index) and
   *  the newest (the top row, lowest row index) define the revset
   *  the "Create review from N revisions" button hands off. */
  const rangeBounds = $derived.by(() => {
    if (!page || rangeSize < 2 || !selected || !extent) return null;
    const anchorIdx = page.rows.findIndex(
      (r) => r.commit.commit_id === selected,
    );
    const extentIdx = page.rows.findIndex(
      (r) => r.commit.commit_id === extent,
    );
    if (anchorIdx < 0 || extentIdx < 0) return null;
    const loIdx = Math.min(anchorIdx, extentIdx);
    const hiIdx = Math.max(anchorIdx, extentIdx);
    return {
      newest: page.rows[loIdx].commit.commit_id,
      oldest: page.rows[hiIdx].commit.commit_id,
    };
  });

  async function loadPage() {
    loading = true;
    error = null;
    try {
      page = await api.browseLog(repo, revset.trim() || undefined);
    } catch (e) {
      page = null;
      error = e instanceof ApiError ? e.message : String(e);
    } finally {
      loading = false;
    }
  }

  async function loadDetail(commitId: CommitId | null) {
    if (!commitId) {
      detail = null;
      return;
    }
    // Optimistic: surface whatever the log already has so the
    // pane updates immediately. The endpoint fetch then fills in
    // anything the log row didn't carry.
    detail = page?.rows.find((r) => r.commit.commit_id === commitId) ?? null;
    try {
      detail = await api.browseCommit(repo, commitId);
    } catch (e) {
      // Detail-fetch failures shouldn't take down the whole pane —
      // the optimistic data above is good enough. Log and move on.
      console.warn('browseCommit failed', e);
    }
  }

  // Initial load + repo/revset reload.
  $effect(() => {
    void repo;
    void revset;
    void loadPage();
  });

  // Drive detail loading off the selection.
  $effect(() => {
    void loadDetail(selected);
  });

  // Resolve `?change=<id>` to a concrete commit_id once at mount.
  // The URL then canonicalises to `?commit=<id>` via the onstate
  // effect below, so a refresh stays pinned to the same revision
  // even after the change moves.
  $effect(() => {
    if (!initialChange || selected) return;
    void (async () => {
      try {
        const row = await api.browseChange(repo, initialChange);
        if (row) selected = row.commit.commit_id;
      } catch (e) {
        console.warn('browseChange failed', e);
      }
    })();
  });

  // Default the selection to the working-copy commit (`@`) once the
  // first page lands. Without this the detail pane opens on an empty
  // "pick a commit" placeholder; `@` is the most useful starting
  // point and matches what a `jj`-literate user expects "browse" to
  // open on. Skipped when the URL already pins a commit or carries a
  // `?change=` still being resolved above.
  $effect(() => {
    if (selected || initialChange || !page) return;
    const wc = page.rows.find((r) => r.is_working_copy);
    if (wc) selected = wc.commit.commit_id;
  });

  // Tell the shell about state changes so the URL can update.
  $effect(() => {
    onstate?.({
      commit: selected ?? undefined,
      path: viewingPath ?? undefined,
      revset: revset.trim() || undefined,
    });
  });

  function selectCommit(id: CommitId, opts: { extendRange?: boolean } = {}) {
    if (opts.extendRange && selected) {
      // Shift-click extends a range from the existing anchor to
      // this row. Anchor stays put so subsequent shift-clicks
      // can grow or shrink the range from the same starting
      // point — same idiom as text-selection.
      extent = id;
    } else {
      selected = id;
      extent = null;
    }
    viewingPath = null;
  }

  function openFile(path: string) {
    viewingPath = path;
  }

  function closeFile() {
    viewingPath = null;
  }

  /** Leave the browser for the review list. The browse view is its
   *  own screen with no review chrome, so the only way back is the
   *  Kata wordmark — too easy to miss. Intercept the click and drive
   *  the SPA router (pushState + popstate) so we don't trigger a
   *  full document reload. */
  function goReviews(e: MouseEvent) {
    // Honour modifier-clicks (open in new tab, etc.) — let the
    // browser handle those as a normal link.
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey) return;
    e.preventDefault();
    history.pushState({}, '', '/');
    dispatchEvent(new PopStateEvent('popstate'));
  }

  /** Switch the log to show only commits that touched `path`.
   *  Constructs the revset client-side because the server-side
   *  `file-history` endpoint exists for that exact shape — using
   *  the regular log endpoint with this revset gives us the same
   *  data without an extra endpoint round-trip. */
  function showFileHistory(path: string) {
    const escaped = path.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
    const expr = `files("${escaped}")`;
    revsetInput = expr;
    revset = expr;
    // Keep the file viewer open so the user can read the file
    // while scanning its history below.
  }

  /** Navigate to the new-review form with the revset pre-filled.
   *  Single-row selection produces `<commit>-..<commit>` (one
   *  commit); a range produces `<oldest>-..<newest>` covering
   *  every commit in the visual span. The reviewer can adjust
   *  before submitting. */
  function createReviewFromSelection() {
    let expr: string;
    if (rangeBounds) {
      expr = `${rangeBounds.oldest}-..${rangeBounds.newest}`;
    } else if (selected) {
      expr = `${selected}-..${selected}`;
    } else {
      return;
    }
    const params = new URLSearchParams({ prefill_revset: expr });
    location.href = `/?${params.toString()}`;
  }

  function submitRevset(e: Event) {
    e.preventDefault();
    revset = revsetInput.trim();
  }

  function clearSelection() {
    selected = null;
    extent = null;
  }

  function shortId(id: string): string {
    return id.length > 12 ? id.slice(0, 12) : id;
  }

  /** Brief "copied" feedback for the range-banner revset. Reset on a
   *  timer so the affordance reads as momentary, not sticky. */
  let rangeCopied = $state(false);
  async function copyRangeRevset() {
    if (!rangeBounds) return;
    const expr = `${shortId(rangeBounds.oldest)}-..${shortId(rangeBounds.newest)}`;
    if (await copyText(expr)) {
      rangeCopied = true;
      setTimeout(() => (rangeCopied = false), 1500);
    }
  }

  /** Drop the first line (subject) from the full description; that
   *  line already renders as the H3. Trim trailing whitespace so the
   *  pre block doesn't trail a blank row. */
  function trimDescription(desc: string): string {
    const idx = desc.indexOf('\n');
    if (idx < 0) return '';
    return desc.slice(idx + 1).trimEnd();
  }
</script>

<div class="browse-root">
  <!-- Topbar: the browse view has no review chrome, so this strip
       carries the only labelled way back to the review list plus
       the name of the repo being browsed. -->
  <div class="browse-topbar">
    <a class="back-link" href="/" onclick={goReviews}>← Reviews</a>
    <span class="topbar-sep" aria-hidden="true">·</span>
    <span class="topbar-repo">Browsing <strong>{repo}</strong></span>
  </div>
  <div class="browse-shell">
  <div class="log-pane">
    <form class="search-bar" onsubmit={submitRevset}>
      <input
        type="text"
        bind:value={revsetInput}
        placeholder="bookmarks() | @ | latest(@-.. | ..@, 50)"
        spellcheck="false"
        autocomplete="off"
      />
      <button type="submit">Go</button>
      {#if revset.length > 0}
        <button
          type="button"
          class="clear"
          onclick={() => {
            revsetInput = '';
            revset = '';
          }}
          title="Back to the default revset"
        >Clear</button>
      {/if}
      <a
        class="revset-help"
        href="https://jj-vcs.github.io/jj/latest/revsets/"
        target="_blank"
        rel="noreferrer"
        title="jj revset syntax reference"
        aria-label="jj revset syntax reference"
      >?</a>
    </form>

    {#if loading && !page}
      <p class="muted status">Loading…</p>
    {:else if error}
      <p class="error status">
        <strong>Couldn't load:</strong> {error}
      </p>
    {:else if page && page.rows.length === 0}
      <p class="muted status">No commits in this revset.</p>
    {:else if page}
      <GraphLog
        {repo}
        rows={page.rows}
        selectedCommitId={selected}
        {rangeIds}
        onselect={selectCommit}
      />
      {#if page.has_more}
        <p class="muted status more">
          Showing the first {page.rows.length} commits — narrow the
          revset to see further.
        </p>
      {/if}
    {/if}
  </div>

  <aside class="detail-pane" class:viewing-file={viewingPath != null}>
    {#if viewingPath != null && selected}
      <FileViewer
        {repo}
        commit={selected}
        path={viewingPath}
        onclose={closeFile}
        onhistory={showFileHistory}
      />
    {:else if detail}
      {@const c = detail.commit}
      <div class="detail-body">
        {#if rangeSize > 1 && rangeBounds}
          <!-- Range header: the user shift-extended a selection.
               Detail below still shows the anchor's commit so
               clicking around inside the range still has a
               meaningful read-out, but the banner up here makes
               it obvious that "Create review" will use the range
               and shows the resulting revset. -->
          <div class="range-banner" role="status">
            <strong>{rangeSize} revisions selected.</strong>
            Revset:
            <button
              type="button"
              class="range-expr"
              title="Copy revset to clipboard"
              onclick={copyRangeRevset}
            >
              <code
                >{shortId(rangeBounds.oldest)}-..{shortId(rangeBounds.newest)}</code
              >
              <span class="copy-hint">{rangeCopied ? '✓ copied' : 'copy'}</span>
            </button>
          </div>
        {/if}
        <header class="detail-header">
          <h3>{c.description_first_line || '(no description)'}</h3>
          <p class="detail-meta">
            <RevId id={c.change_id} kind="change" {repo} inline />
            <RevId id={c.commit_id} kind="commit" {repo} inline />
            · <span class="muted">{c.author_email}</span>
            · <span class="muted">{c.author_timestamp}</span>
          </p>
          {#if (detail.bookmarks ?? []).length > 0}
            <p class="refs">
              {#each detail.bookmarks ?? [] as bm (bm)}
                <span class="ref">{bm}</span>
              {/each}
              {#if detail.is_working_copy}<span class="ref wc">@</span>{/if}
            </p>
          {:else if detail.is_working_copy}
            <p class="refs"><span class="ref wc">@</span></p>
          {/if}
        </header>

        {#if c.description.includes('\n')}
          {@const body = trimDescription(c.description)}
          {#if body.length > 0}
            <!-- Commit descriptions are conventionally markdown
                 in this codebase (and most projects we work on);
                 render them as such so links, lists, and code
                 blocks come through. -->
            <div class="description markdown">{@html renderMarkdown(body)}</div>
          {/if}
        {/if}

        {#if c.changed_files.length > 0}
          <section class="files">
            <h4>{c.changed_files.length} file{c.changed_files.length === 1 ? '' : 's'} changed</h4>
            <ul>
              {#each c.changed_files as path (path)}
                <li>
                  <button
                    type="button"
                    class="file-link"
                    onclick={() => openFile(path)}
                    title="View this file at {shortId(c.commit_id)}"
                  ><code>{path}</code></button>
                </li>
              {/each}
            </ul>
          </section>
        {/if}

        {#if (c.conflict_paths ?? []).length > 0}
          <section class="conflicts">
            <h4>Conflicts</h4>
            <ul>
              {#each c.conflict_paths ?? [] as path (path)}
                <li>
                  <button
                    type="button"
                    class="file-link conflict"
                    onclick={() => openFile(path)}
                  ><code>{path}</code></button>
                </li>
              {/each}
            </ul>
          </section>
        {/if}

        <footer class="detail-actions">
          <button
            type="button"
            class="primary"
            onclick={createReviewFromSelection}
            title="Open the new-review form with the selected revset pre-filled"
          >
            {rangeSize > 1
              ? `Create review from ${rangeSize} revisions`
              : 'Create review'}
          </button>
          <button
            type="button"
            class="close"
            onclick={clearSelection}
          >Close</button>
        </footer>
      </div>
    {:else}
      <p class="muted detail-empty">
        Pick a commit from the log to see its detail.
      </p>
    {/if}
  </aside>
  </div>
</div>

<style>
  /* Outer column: topbar strip on top, two-pane shell filling the
   * rest. Owns the viewport height below the sticky app header so
   * the shell's inner scroll regions stay bounded. */
  .browse-root {
    display: flex;
    flex-direction: column;
    height: calc(100vh - var(--app-header-h));
  }

  .browse-topbar {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 0 0 auto;
    padding: 6px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-panel);
    font-size: 13px;
  }

  .browse-topbar .back-link {
    color: var(--link);
    text-decoration: none;
    padding: 2px 6px;
    border-radius: 4px;
  }
  .browse-topbar .back-link:hover {
    background: var(--link-bg);
    text-decoration: underline;
  }
  .browse-topbar .topbar-sep {
    color: var(--text-faint);
  }
  .browse-topbar .topbar-repo {
    color: var(--text-muted);
  }

  /* Two-pane shell. The grid columns are flexible: detail pane
   * keeps a sensible default width but adapts on narrow screens
   * via minmax(). */
  .browse-shell {
    display: grid;
    grid-template-columns: minmax(0, 1fr) minmax(320px, 28%);
    gap: 0;
    /* Fills the space the topbar leaves; `min-height: 0` keeps the
     * grid from growing past the flex parent so the panes' inner
     * scroll regions stay bounded. */
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .log-pane {
    display: flex;
    flex-direction: column;
    min-width: 0;
    /* Critical: a flex-column with overflowing children needs
     * `min-height: 0` so the children can shrink past their
     * natural size and trigger the inner scroll. */
    min-height: 0;
    border-right: 1px solid var(--border);
  }

  .search-bar {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    border-bottom: 1px solid var(--border);
    background: var(--bg-panel);
  }

  .search-bar input {
    flex: 1;
    min-width: 0;
    padding: 4px 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg);
    color: var(--text);
    font: inherit;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
  }

  /* "?" link to the jj revset reference — the default revset
   * expression in the input is dense syntax a newcomer won't
   * recognise. A circular badge so it reads as help, not a value. */
  .search-bar .revset-help {
    flex: 0 0 auto;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    height: 20px;
    border: 1px solid var(--border);
    border-radius: 50%;
    color: var(--text-muted);
    text-decoration: none;
    font-size: 12px;
  }
  .search-bar .revset-help:hover {
    background: var(--link-bg);
    color: var(--link);
    border-color: var(--link);
  }

  .search-bar button {
    padding: 4px 12px;
    font-size: 12px;
  }

  .status {
    padding: 12px;
    margin: 0;
  }

  .status.more {
    border-top: 1px solid var(--border-muted);
    text-align: center;
    font-size: 12px;
  }

  .detail-pane {
    overflow-y: auto;
    background: var(--bg-panel);
  }

  /* When showing a file the pane drops its padding so the file
   * viewer's own header sits flush against the divider. */
  .detail-pane:not(.viewing-file) .detail-body {
    padding: 16px;
  }

  /* The file viewer manages its own scroll, so the outer pane
   * shouldn't scroll too. */
  .detail-pane.viewing-file {
    overflow: hidden;
    padding: 0;
    display: flex;
    flex-direction: column;
  }

  .detail-empty {
    text-align: center;
    padding: 32px 16px;
  }

  .detail-header {
    margin-bottom: 16px;
  }

  .detail-header h3 {
    margin: 0 0 4px 0;
    font-size: 14px;
  }

  .detail-meta {
    margin: 0;
    font-size: 12px;
  }

  .refs {
    margin: 8px 0 0 0;
    display: flex;
    flex-wrap: wrap;
    gap: 4px;
  }

  .ref {
    font-size: 11px;
    padding: 1px 6px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg);
    color: var(--link);
  }

  .ref.wc {
    background: var(--link-bg);
    font-weight: 600;
  }

  /* Banner shown above the commit detail when the user has
   * shift-extended a range. Reuses the existing `--link-bg` so
   * the strip visually picks up the anchor row's highlight. */
  .range-banner {
    margin-bottom: 12px;
    padding: 8px 12px;
    background: var(--link-bg);
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    font-size: 12px;
  }

  /* The revset is a button so it can be copied to the clipboard —
   * a reviewer typically wants to paste it into the new-review
   * form or a `jj log`. Styled to still read as an inline code
   * chip, with a quiet "copy" hint that flips to a checkmark. */
  .range-banner .range-expr {
    display: inline-flex;
    align-items: baseline;
    gap: 6px;
    padding: 1px 5px;
    background: var(--bg);
    border: 1px solid var(--border-muted);
    border-radius: 3px;
    cursor: pointer;
    font: inherit;
    color: inherit;
  }
  .range-banner .range-expr:hover {
    border-color: var(--border);
  }
  .range-banner .range-expr code {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
  }
  .range-banner .copy-hint {
    font-size: 10px;
    color: var(--text-muted);
  }

  /* Markdown-rendered description body. The container styles
   * the outer card; the global `.markdown` rules (in app.css)
   * handle the inner heading / list / code styling so this
   * pane matches review-summary rendering elsewhere. */
  .description {
    margin: 12px 0;
    padding: 8px 12px;
    background: var(--bg);
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    font-size: 12px;
  }

  .files,
  .conflicts {
    margin-top: 16px;
  }

  .files h4,
  .conflicts h4 {
    margin: 0 0 6px 0;
    font-size: 12px;
    color: var(--text-muted);
    font-weight: 600;
  }

  .files ul,
  .conflicts ul {
    margin: 0;
    padding: 0;
    list-style: none;
  }

  .files li,
  .conflicts li {
    padding: 2px 0;
    font-size: 12px;
  }

  .file-link {
    background: transparent;
    border: none;
    padding: 0;
    margin: 0;
    cursor: pointer;
    color: var(--link);
    font: inherit;
    font-size: 12px;
    text-align: left;
  }

  .file-link:hover {
    text-decoration: underline;
  }

  .file-link.conflict {
    color: var(--warn-text);
  }

  .file-link code {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
  }

  .detail-actions {
    margin-top: 16px;
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }

  .detail-actions .primary {
    background: var(--link);
    color: var(--on-accent);
    border: 1px solid var(--link);
  }

  .detail-actions .primary:hover {
    filter: brightness(1.05);
  }

  .clear {
    background: transparent;
    border: 1px solid var(--border);
  }

  /* Mobile: stack the two panes. The detail pane drops below
   * the log and the user scrolls between them. */
  @media (max-width: 720px) {
    .browse-shell {
      grid-template-columns: 1fr;
      grid-template-rows: 1fr auto;
    }
    .detail-pane {
      border-top: 1px solid var(--border);
    }
  }
</style>
