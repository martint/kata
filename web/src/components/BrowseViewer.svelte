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
  import type { CommitId, LogPage, LogRow } from '../lib/types';
  import FileViewer from './browse/FileViewer.svelte';
  import GraphLog from './browse/GraphLog.svelte';

  let {
    repo,
    initialCommit = null,
    initialPath = null,
    initialRevset = null,
    onstate,
  }: {
    repo: string;
    initialCommit?: CommitId | null;
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

  let selected: CommitId | null = $state(initialCommit);
  let detail: LogRow | null = $state(null);
  /** When set, the detail pane shows the file viewer instead of
   *  the commit detail. Clicking a file path in the commit detail
   *  populates this; Close clears it. */
  let viewingPath: string | null = $state(initialPath);

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

  // Tell the shell about state changes so the URL can update.
  $effect(() => {
    onstate?.({
      commit: selected ?? undefined,
      path: viewingPath ?? undefined,
      revset: revset.trim() || undefined,
    });
  });

  function selectCommit(id: CommitId) {
    selected = id;
    viewingPath = null;
  }

  function openFile(path: string) {
    viewingPath = path;
  }

  function closeFile() {
    viewingPath = null;
  }

  function submitRevset(e: Event) {
    e.preventDefault();
    revset = revsetInput.trim();
  }

  function clearSelection() {
    selected = null;
  }

  function shortId(id: string): string {
    return id.length > 12 ? id.slice(0, 12) : id;
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
        rows={page.rows}
        selectedCommitId={selected}
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
      />
    {:else if detail}
      {@const c = detail.commit}
      <div class="detail-body">
        <header class="detail-header">
          <h3>{c.description_first_line || '(no description)'}</h3>
          <p class="detail-meta">
            <code class="commit-id">{shortId(c.commit_id)}</code>
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
          <pre class="description">{trimDescription(c.description)}</pre>
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

<style>
  .browse-shell {
    display: grid;
    grid-template-columns: 1fr 360px;
    gap: 0;
    height: calc(100vh - var(--app-header-h));
    overflow: hidden;
  }

  .log-pane {
    display: flex;
    flex-direction: column;
    min-width: 0;
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

  .description {
    margin: 12px 0;
    padding: 8px;
    background: var(--bg);
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    font-size: 12px;
    white-space: pre-wrap;
    overflow-x: auto;
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

  .commit-id {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    background: var(--bg);
    padding: 1px 5px;
    border-radius: 3px;
    font-size: 11px;
  }

  .detail-actions {
    margin-top: 16px;
    display: flex;
    justify-content: flex-end;
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
