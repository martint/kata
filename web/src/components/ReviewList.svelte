<script lang="ts">
  import { api } from '../lib/api';
  import type { Bookmark, RepoSummary, ReviewSummary } from '../lib/types';

  interface Props {
    repos: RepoSummary[];
    repo: string;
    summaries: ReviewSummary[] | null;
    loading: boolean;
    createdBy: string;
    onchangerepo: (name: string) => void;
    onopen: (id: string) => void;
  }
  const {
    repos,
    repo,
    summaries,
    loading,
    createdBy,
    onchangerepo,
    onopen,
  }: Props = $props();

  let bookmarks: Bookmark[] = $state([]);
  let bookmarksLoading: boolean = $state(true);
  let bookmarksError: string | null = $state(null);
  let selected: string = $state('');
  let revset: string = $state('');
  let revsetEdited: boolean = $state(false);
  let creating: boolean = $state(false);
  let createError: string | null = $state(null);

  async function loadBookmarks() {
    if (!repo) {
      bookmarks = [];
      bookmarksLoading = false;
      return;
    }
    bookmarksLoading = true;
    bookmarksError = null;
    try {
      bookmarks = await api.listBookmarks(repo);
      const existing = new Set((summaries ?? []).map((s) => s.manifest.review_id));
      const fresh = bookmarks.find((b) => !existing.has(b.name));
      if (!selected || !bookmarks.some((b) => b.name === selected)) {
        selected = (fresh ?? bookmarks[0])?.name ?? '';
      }
    } catch (e) {
      bookmarksError = (e as Error).message;
    } finally {
      bookmarksLoading = false;
    }
  }
  $effect(() => {
    void repo;
    void summaries;
    void loadBookmarks();
  });

  // Recompute the suggested revset whenever the bookmark changes, unless
  // the user has hand-edited it (don't clobber their input).
  $effect(() => {
    if (!revsetEdited && selected) {
      revset = `trunk()..${selected}`;
    }
  });

  /** Branches that don't yet have a review, newest first — the candidates
   *  for "I want to start a review on this." Already sorted server-side
   *  by commit timestamp. */
  const reviewedNames = $derived(
    new Set((summaries ?? []).map((s) => s.manifest.review_id)),
  );
  const RECENT_LIMIT = 10;
  const recentBranches = $derived(
    bookmarks
      .filter((b) => !reviewedNames.has(b.name))
      .slice(0, RECENT_LIMIT),
  );

  /** Compact relative-time formatting: "5m ago", "3h ago", "2d ago". */
  function relative(iso: string): string {
    if (!iso) return '';
    const t = Date.parse(iso);
    if (Number.isNaN(t)) return '';
    const diffMs = Date.now() - t;
    const sec = Math.round(diffMs / 1000);
    if (sec < 60) return 'just now';
    const min = Math.round(sec / 60);
    if (min < 60) return `${min}m ago`;
    const hr = Math.round(min / 60);
    if (hr < 24) return `${hr}h ago`;
    const day = Math.round(hr / 24);
    if (day < 30) return `${day}d ago`;
    const mo = Math.round(day / 30);
    if (mo < 12) return `${mo}mo ago`;
    const yr = Math.round(day / 365);
    return `${yr}y ago`;
  }

  function pickBranch(name: string) {
    selected = name;
    revsetEdited = false; // re-derive revset for the new pick
  }

  async function submit(event: Event) {
    event.preventDefault();
    if (!repo || !selected || !createdBy || !revset.trim()) return;
    creating = true;
    createError = null;
    try {
      await api.createReview(repo, {
        review_id: selected,
        revset: revset.trim(),
        bookmark: selected,
        created_by: createdBy,
      });
      revsetEdited = false;
      onopen(selected);
    } catch (e) {
      createError = (e as Error).message;
    } finally {
      creating = false;
    }
  }
</script>

<style>
  .repo-picker {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 16px;
  }

  .repo-picker select {
    min-width: 200px;
  }

  .revset-field input {
    margin-left: 8px;
    min-width: 220px;
  }

  .hint {
    margin: 6px 0 0;
    font-size: 12px;
  }

  .hint code {
    background: var(--bg-elevated);
    padding: 0 4px;
    border-radius: 3px;
  }

  .recent-section {
    margin-bottom: 12px;
  }

  .recent-section h4 {
    margin: 0 0 6px;
    font-size: 13px;
    color: var(--text-muted);
    font-weight: 500;
  }

  .recent-list {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .recent-list button {
    width: 100%;
    display: flex;
    align-items: baseline;
    gap: 10px;
    padding: 6px 10px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    cursor: pointer;
    text-align: left;
    font: inherit;
  }

  .recent-list button:hover {
    background: var(--bg-panel);
    border-color: var(--link);
  }

  .recent-list .name {
    font-weight: 500;
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .recent-list .when {
    font-size: 12px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }

  .recent-list .picked {
    border-color: var(--link);
    background: var(--link-bg);
  }
</style>

{#if repos.length === 0}
  <p class="muted">No repositories configured.</p>
{:else if repos.length > 1}
  <div class="repo-picker">
    <label for="repo-select"><strong>Repository</strong></label>
    <select
      id="repo-select"
      value={repo}
      onchange={(e) => onchangerepo((e.currentTarget as HTMLSelectElement).value)}
    >
      {#each repos as r (r.name)}
        <option value={r.name}>{r.name}</option>
      {/each}
    </select>
  </div>
{/if}

<section>
  <h2>Reviews</h2>
  {#if loading && summaries === null}
    <p class="muted">Loading…</p>
  {:else if summaries && summaries.length === 0}
    <p class="muted">No reviews yet. Pick a bookmark below.</p>
  {:else if summaries}
    <ul class="review-list">
      {#each summaries as s (s.manifest.review_id)}
        <li>
          <button class="row" onclick={() => onopen(s.manifest.review_id)}>
            <strong>{s.manifest.review_id}</strong>
            <span class="meta">{s.manifest.revset}</span>
            <span style="flex: 1"></span>
            <span class="meta">{s.published_comment_count} comments</span>
          </button>
        </li>
      {/each}
    </ul>
  {/if}
</section>

<section>
  <h3>Start a new review</h3>
  {#if bookmarksError}
    <p class="error">Couldn't load bookmarks: {bookmarksError}</p>
    <button type="button" onclick={loadBookmarks}>Retry</button>
  {:else if bookmarksLoading}
    <p class="muted">Loading bookmarks…</p>
  {:else if bookmarks.length === 0}
    <p class="muted">
      No bookmarks found in this repo. Create one with
      <code>jj bookmark create &lt;name&gt; -r &lt;rev&gt;</code>, then refresh.
    </p>
  {:else}
    {#if recentBranches.length > 0}
      <div class="recent-section">
        <h4>Recently updated branches</h4>
        <ul class="recent-list">
          {#each recentBranches as b (b.name)}
            <li>
              <button
                type="button"
                class={selected === b.name ? 'picked' : ''}
                onclick={() => pickBranch(b.name)}
                title="Click to fill the form below"
              >
                <span class="name">{b.name}</span>
                <span class="when">{relative(b.commit_timestamp)}</span>
              </button>
            </li>
          {/each}
        </ul>
      </div>
    {/if}
    <form class="create-form" onsubmit={submit}>
      {#if repos.length > 1}
        <label>
          Repository
          <select
            value={repo}
            onchange={(e) => onchangerepo((e.currentTarget as HTMLSelectElement).value)}
          >
            {#each repos as r (r.name)}
              <option value={r.name}>{r.name}</option>
            {/each}
          </select>
        </label>
      {/if}
      <label>
        Bookmark
        <select bind:value={selected}>
          {#each bookmarks as b (b.name)}
            <option value={b.name}>{b.name}</option>
          {/each}
        </select>
      </label>
      <label class="revset-field">
        Revset
        <input
          type="text"
          bind:value={revset}
          oninput={() => (revsetEdited = true)}
          placeholder="e.g. trunk()..feature, @-..@"
        />
      </label>
      <button type="submit" class="primary" disabled={creating || !selected || !revset.trim()}>
        {creating ? 'Creating…' : 'Create review'}
      </button>
    </form>
    <p class="hint muted">
      The default <code>trunk()..&lt;bookmark&gt;</code> is empty when the bookmark
      <em>is</em> the trunk. Try <code>@-..@</code> for the latest commit, or any
      other <a href="https://jj-vcs.github.io/jj/latest/revsets/" target="_blank" rel="noreferrer">jj revset</a>.
    </p>
  {/if}
  {#if createError}
    <p class="error">{createError}</p>
  {/if}
</section>
