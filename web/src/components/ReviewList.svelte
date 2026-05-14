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
  let summary: string = $state('');
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

  /** Show the "this revset can be empty" hint only when the user is on
   *  the default `trunk()..<bookmark>` shape and hasn't overridden it.
   *  An author who's already typed a custom revset doesn't need a
   *  warning about a default they're not using. */
  const showDefaultRevsetHint = $derived(
    !!selected && revset.trim() === `trunk()..${selected}`,
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
      const trimmedSummary = summary.trim();
      await api.createReview(repo, {
        review_id: selected,
        revset: revset.trim(),
        bookmark: selected,
        created_by: createdBy,
        summary: trimmedSummary.length > 0 ? trimmedSummary : undefined,
      });
      revsetEdited = false;
      summary = '';
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

  /* The home screen is three distinct sections — reviews, new-review
   * form, and branches-without-a-review — separated by clear gaps and
   * (for the create form) a card so the form reads as its own block
   * rather than as part of a flat list. */
  .home-section + .home-section {
    margin-top: 28px;
  }

  .home-section > h3 {
    margin: 0 0 10px;
  }

  /* Create-review form: stack so the summary textarea has real width,
   * instead of competing with the bookmark/revset selectors for inline
   * space. Row 1 = bookmark + revset, row 2 = summary, row 3 = submit.
   * Every input/select/textarea is forced to `width: 100%` of its label
   * — browser defaults (cols=20 on textarea, intrinsic size on select)
   * otherwise leave fields narrower than the card. */
  .create-card {
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 16px;
    background: var(--bg-panel);
  }

  .create-form {
    display: flex;
    flex-direction: column;
    gap: 12px;
  }

  .create-row {
    display: flex;
    flex-wrap: wrap;
    gap: 16px;
  }

  /* Both labels in the row grow equally — bookmark and revset get
   * matching widths instead of bookmark hugging its content while
   * revset hogs the rest. */
  .create-row > label {
    flex: 1 1 200px;
    min-width: 0;
  }

  /* Labels everywhere in the form share the same vertical-stack shape:
   * caption text on top, full-width control below. */
  .create-form label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 13px;
  }

  .create-form select,
  .create-form input[type='text'],
  .create-form textarea {
    width: 100%;
    box-sizing: border-box;
    font: inherit;
  }

  .create-form textarea {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    padding: 6px 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    resize: vertical;
    min-height: 120px;
  }

  /* Hint sits in the actions row left of the button (flex: 1 to push
   * the button to the far right) so a narrow note doesn't take an
   * extra line of its own. If the viewport is narrow enough that the
   * hint wraps to several lines, `flex-wrap` makes the button drop
   * below it instead of squashing into a 40-char column. */
  .create-actions {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
  }

  .create-actions .hint {
    flex: 1 1 240px;
    margin: 0;
    font-size: 12px;
  }

  .create-actions button {
    flex: 0 0 auto;
    margin-left: auto;
  }

  .hint code {
    background: var(--bg-elevated);
    padding: 0 4px;
    border-radius: 3px;
  }

  /* Branches list: lives in its own section *below* the form. Visually
   * quieter than the form (no card, smaller text) — the form is the
   * primary CTA on this screen; the list is a "or pick one of these"
   * shortcut, not its own affordance. */
  .recent-section h3 .recent-hint {
    font-weight: 400;
    color: var(--text-muted);
    font-size: 13px;
    margin-left: 6px;
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

<section class="home-section">
  <h2>Reviews</h2>
  {#if loading && summaries === null}
    <p class="muted">Loading…</p>
  {:else if summaries && summaries.length === 0}
    <p class="muted">No reviews yet. Start one below.</p>
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

<section class="home-section">
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
    <div class="create-card">
      <form class="create-form" onsubmit={submit}>
        <div class="create-row">
          <label class="bookmark-field">
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
        </div>
        <label class="summary-field">
          Summary <span class="muted">(optional, markdown)</span>
          <textarea
            bind:value={summary}
            rows="4"
            placeholder="A short description of the change. Shown at the top of the review."
          ></textarea>
        </label>
        <div class="create-actions">
          {#if showDefaultRevsetHint}
            <p class="hint muted">
              <code>trunk()..&lt;bookmark&gt;</code> is empty when the bookmark
              <em>is</em> the trunk. Edit the revset above — see
              <a href="https://jj-vcs.github.io/jj/latest/revsets/" target="_blank" rel="noreferrer">jj revsets</a>
              for the shapes you can use.
            </p>
          {/if}
          <button
            type="submit"
            class="primary"
            disabled={creating || !selected || !revset.trim()}
          >
            {creating ? 'Creating…' : 'Create review'}
          </button>
        </div>
      </form>
    </div>
  {/if}
  {#if createError}
    <p class="error">{createError}</p>
  {/if}
</section>

{#if !bookmarksError && !bookmarksLoading && recentBranches.length > 0}
  <section class="home-section recent-section">
    <h3>
      Branches without a review
      <span class="recent-hint">— click to fill the form above</span>
    </h3>
    <ul class="recent-list">
      {#each recentBranches as b (b.name)}
        <li>
          <button
            type="button"
            class={selected === b.name ? 'picked' : ''}
            onclick={() => pickBranch(b.name)}
          >
            <span class="name">{b.name}</span>
            <span class="when">{relative(b.commit_timestamp)}</span>
          </button>
        </li>
      {/each}
    </ul>
  </section>
{/if}
