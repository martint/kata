<script lang="ts">
  import { api } from '../lib/api';
  import { renderMarkdown } from '../lib/markdown';
  import type { Bookmark, RepoSummary, ReviewSummary } from '../lib/types';
  import ActionsMenu from './ActionsMenu.svelte';

  interface Props {
    repos: RepoSummary[];
    repo: string;
    summaries: ReviewSummary[] | null;
    loading: boolean;
    createdBy: string;
    /** Pre-fills the new-review form's revset. Set by the browse
     *  pane's "Create review from this commit" handoff via the
     *  `?prefill_revset=` URL parameter. */
    prefillRevset?: string;
    onchangerepo: (name: string) => void;
    onopen: (number: number) => void;
  }
  const {
    repos,
    repo,
    summaries,
    loading,
    createdBy,
    prefillRevset,
    onchangerepo,
    onopen,
  }: Props = $props();

  let bookmarks: Bookmark[] = $state([]);
  let bookmarksLoading: boolean = $state(true);
  let bookmarksError: string | null = $state(null);
  let selected: string = $state('');
  let revset: string = $state(prefillRevset ?? '');
  // When the URL handed us a revset, treat it as a user-edit so the
  // bookmark-driven auto-derive below doesn't clobber it.
  let revsetEdited: boolean = $state(prefillRevset !== undefined);
  let summary: string = $state('');
  let summaryMode = $state<'edit' | 'preview'>('edit');
  let creating: boolean = $state(false);
  let createError: string | null = $state(null);

  /** Hide archived reviews from the main list by default — archived
   *  is the "out of the way" state. The user can flip the toggle to
   *  see them and they still appear (dimmed) until flipped back. */
  let showArchived: boolean = $state(false);

  /** When `true`, the list is in batch-selection mode: rows render a
   *  checkbox, the header is replaced by a batch-action toolbar, and
   *  row clicks toggle the selection instead of opening the review.
   *  Exit via the Cancel button, the Escape key, or completing a
   *  batch action. */
  let selectMode: boolean = $state(false);
  /** Selected review IDs in batch mode. Keyed by `review_id` (not
   *  number) so the set survives reordering after a row is removed. */
  let selectedIds: Set<string> = $state(new Set());
  /** True while a batch action is in flight. Disables the toolbar
   *  buttons to prevent double-fires. */
  let bulkBusy: boolean = $state(false);

  /** Reviews split into "live" and "archived" buckets. The live list
   *  is what we render by default; the archived list only appears
   *  when [[showArchived]] is true. */
  const liveSummaries = $derived(
    (summaries ?? []).filter((s) => !s.manifest.archived_at),
  );
  const archivedSummaries = $derived(
    (summaries ?? []).filter((s) => !!s.manifest.archived_at),
  );

  /** Rendered HTML for the preview tab. Only computed when in preview
   *  mode — same idea as the in-review summary editor: don't pay the
   *  markdown-render cost on every keystroke while in Write. */
  const summaryPreview = $derived(
    summaryMode === 'preview' ? renderMarkdown(summary) : '',
  );

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
      const existing = new Set(
        (summaries ?? [])
          .map((s) => s.manifest.bookmark)
          .filter((b): b is string => !!b),
      );
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
    new Set(
      (summaries ?? [])
        .map((s) => s.manifest.bookmark)
        .filter((b): b is string => !!b),
    ),
  );
  const RECENT_LIMIT = 10;
  const recentBranches = $derived(
    bookmarks
      .filter((b) => !reviewedNames.has(b.name))
      .slice(0, RECENT_LIMIT),
  );

  /** Async revset probe. The form submits the current revset to the
   *  jj backend (debounced) and gets back the commit count, so we can
   *  warn before a user creates an empty or malformed review.
   *
   *  Tri-state: `null` while idle / debouncing; `{ count }` once a
   *  request has come back; `{ error }` when jj rejects the revset
   *  (syntax issue, unknown symbol, etc). */
  type RevsetStatus = { kind: 'ok'; count: number } | { kind: 'error'; message: string };
  let revsetStatus: RevsetStatus | null = $state(null);
  /** Bumped by every debounce timer fire so a slow request that
   *  resolves after a newer one started can be dropped silently. */
  let revsetCheckSeq = 0;
  $effect(() => {
    const expr = revset.trim();
    if (!repo || !expr) {
      revsetStatus = null;
      return;
    }
    const mySeq = ++revsetCheckSeq;
    const handle = setTimeout(async () => {
      try {
        const result = await api.previewRevset(repo, expr);
        if (mySeq === revsetCheckSeq) {
          revsetStatus = { kind: 'ok', count: result.count };
        }
      } catch (e) {
        if (mySeq === revsetCheckSeq) {
          revsetStatus = { kind: 'error', message: (e as Error).message };
        }
      }
    }, 300);
    return () => clearTimeout(handle);
  });

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

  /** Per-row archive toggle. Mutates the manifest in place so the
   *  archived-pill flips without waiting for the SSE round-trip
   *  (the broadcast lands ~immediately, but we don't want a stutter
   *  if the user clicks rapidly). The parent's SSE handler will
   *  reload anyway and reconcile any drift. */
  async function toggleArchive(s: ReviewSummary) {
    const wasArchived = !!s.manifest.archived_at;
    try {
      const next = wasArchived
        ? await api.unarchiveReview(repo, s.manifest.number)
        : await api.archiveReview(repo, s.manifest.number);
      s.manifest = next;
    } catch (e) {
      alert(`Couldn't ${wasArchived ? 'unarchive' : 'archive'} #${s.manifest.number}: ${(e as Error).message}`);
    }
  }

  /** Per-row delete, guarded by a confirm dialog. Server response
   *  is 204; the SSE `review-deleted` event tells every tab to
   *  reload, so we don't need to splice the row ourselves. */
  async function deleteReview(s: ReviewSummary) {
    const ok = confirm(
      `Permanently delete review #${s.manifest.number} "${s.manifest.name}"?\n\nThis removes all sessions, comments, responses, and annotations. Cannot be undone.`,
    );
    if (!ok) return;
    try {
      await api.deleteReview(repo, s.manifest.number);
    } catch (e) {
      alert(`Couldn't delete #${s.manifest.number}: ${(e as Error).message}`);
    }
  }

  /** True when this viewer is allowed to mutate the review's state.
   *  Mirrors the server-side check; hides actions the server would
   *  reject anyway so non-creators don't see a useless menu. */
  function canManage(s: ReviewSummary): boolean {
    return !!createdBy && createdBy === s.manifest.created_by;
  }

  function enterSelectMode() {
    selectMode = true;
    selectedIds = new Set();
  }

  function exitSelectMode() {
    selectMode = false;
    selectedIds = new Set();
  }

  /** Toggle membership of one review in the batch selection. Always
   *  replaces the set so reactivity fires; mutating in place would
   *  be invisible to Svelte 5's deep tracking on `Set`. */
  function toggleSelect(id: string) {
    const next = new Set(selectedIds);
    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    selectedIds = next;
  }

  /** Set of reviews that are both visible and the viewer is allowed
   *  to manage — i.e. what the toolbar's actions can target. */
  const manageableVisible = $derived(() => {
    const live = liveSummaries.filter(canManage);
    const archived = showArchived ? archivedSummaries.filter(canManage) : [];
    return [...live, ...archived];
  });

  /** Visible-and-manageable reviews that the viewer has selected. */
  const selectedSummaries = $derived(() =>
    manageableVisible().filter((s) => selectedIds.has(s.manifest.review_id)),
  );

  async function bulkArchive() {
    if (bulkBusy) return;
    bulkBusy = true;
    try {
      const targets = selectedSummaries().filter((s) => !s.manifest.archived_at);
      for (const s of targets) {
        try {
          const next = await api.archiveReview(repo, s.manifest.number);
          s.manifest = next;
        } catch (e) {
          alert(`Couldn't archive #${s.manifest.number}: ${(e as Error).message}`);
        }
      }
    } finally {
      bulkBusy = false;
      exitSelectMode();
    }
  }

  async function bulkDelete() {
    if (bulkBusy) return;
    const targets = selectedSummaries();
    if (targets.length === 0) return;
    const ok = confirm(
      `Permanently delete ${targets.length} review${targets.length === 1 ? '' : 's'}?\n\nThis removes all sessions, comments, responses, and annotations. Cannot be undone.`,
    );
    if (!ok) return;
    bulkBusy = true;
    try {
      for (const s of targets) {
        try {
          await api.deleteReview(repo, s.manifest.number);
        } catch (e) {
          alert(`Couldn't delete #${s.manifest.number}: ${(e as Error).message}`);
        }
      }
    } finally {
      bulkBusy = false;
      exitSelectMode();
    }
  }

  function onKey(e: KeyboardEvent) {
    if (selectMode && e.key === 'Escape') {
      exitSelectMode();
    }
  }

  /** Build the menu items for a single row. Closing the menu is
   *  handled by ActionsMenu itself. Non-creators see the same menu
   *  with both items disabled and a "(creator only)" suffix — the
   *  server enforces creator-only on both endpoints, and hiding the
   *  menu entirely leaves non-creators wondering why some rows have
   *  it and others don't. */
  function rowMenuItems(s: ReviewSummary) {
    const archived = !!s.manifest.archived_at;
    const manageable = canManage(s);
    const suffix = manageable ? '' : ' (creator only)';
    return [
      {
        label: `${archived ? 'Unarchive' : 'Archive'}${suffix}`,
        onclick: () => void toggleArchive(s),
        disabled: !manageable,
      },
      {
        label: `Delete…${suffix}`,
        onclick: () => void deleteReview(s),
        danger: true,
        disabled: !manageable,
      },
    ];
  }

  async function submit(event: Event) {
    event.preventDefault();
    if (!repo || !selected || !createdBy || !revset.trim()) return;
    creating = true;
    createError = null;
    try {
      const trimmedSummary = summary.trim();
      const created = await api.createReview(repo, {
        name: selected,
        revset: revset.trim(),
        bookmark: selected,
        created_by: createdBy,
        summary: trimmedSummary.length > 0 ? trimmedSummary : undefined,
      });
      revsetEdited = false;
      summary = '';
      summaryMode = 'edit';
      onopen(created.number);
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

  /* Plain-text repo label for the single-repo case, in lieu of the
   * `<select>` dropdown. Matches the picker's visual weight so the
   * Browse link reads as anchored to "this repo" not floating. */
  .repo-picker .repo-label {
    color: var(--text);
    font-size: 14px;
  }

  /* "Browse" link tucked into the repo picker so it lives next to
   * the selection it operates on. Styled as a subtle secondary
   * link rather than a button so it doesn't compete with the
   * primary "New review" / list actions below. */
  .repo-picker .browse-link {
    margin-left: 4px;
    font-size: 13px;
    color: var(--link);
    text-decoration: none;
    padding: 4px 8px;
    border-radius: 4px;
  }

  .repo-picker .browse-link:hover {
    background: var(--link-bg);
    text-decoration: underline;
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

  /* Header row of the Reviews section: title on the left, archived
   * toggle right-aligned. The toggle only renders when there's at
   * least one archived review (handled in the template). */
  .review-list-header {
    display: flex;
    align-items: baseline;
    gap: 12px;
    margin-bottom: 10px;
  }

  .review-list-header h2 {
    margin: 0;
  }

  .archived-toggle {
    font-size: 13px;
    color: var(--text-muted);
    display: inline-flex;
    align-items: center;
    gap: 6px;
    cursor: pointer;
  }

  .select-toggle,
  .select-action {
    padding: 4px 10px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg);
    color: inherit;
    font: inherit;
    font-size: 13px;
    cursor: pointer;
  }

  .select-toggle:hover:not(:disabled),
  .select-action:hover:not(:disabled) {
    background: var(--bg-elevated);
  }

  .select-action:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .select-action.danger {
    color: var(--error-text);
  }

  .select-status {
    font-size: 14px;
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

  /* `.summary-field` replaces the previous label wrapper now that it
   * contains its own header row (caption + Write/Preview tabs) above
   * the textarea / preview pane. */
  .summary-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 13px;
  }

  .summary-field-header {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .summary-field-header label {
    /* Override `.create-form label`'s flex-column / font-size — here
     * the caption is just inline text, not a stacked control. */
    display: inline;
    font-size: inherit;
  }

  /* Same Write/Preview pill shape as the in-review summary editor and
   * the comment composer. */
  .summary-field .tabs {
    display: flex;
    border: 1px solid var(--border);
    border-radius: 4px;
    overflow: hidden;
  }

  .summary-field .tab {
    background: transparent;
    border: none;
    padding: 2px 10px;
    font-size: 12px;
    cursor: pointer;
    color: var(--text-muted);
  }

  .summary-field .tab.active {
    background: var(--link);
    color: var(--on-accent);
  }

  /* Preview pane mirrors the textarea's footprint so toggling Write
   * ↔ Preview doesn't jank the form height. */
  .summary-field .preview {
    min-height: 120px;
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg);
    box-sizing: border-box;
  }

  .summary-field .preview :global(p:first-child) {
    margin-top: 0;
  }

  .summary-field .preview :global(p:last-child) {
    margin-bottom: 0;
  }

  .summary-field .preview :global(pre) {
    background: var(--bg-panel);
    padding: 8px;
    border-radius: 4px;
    overflow-x: auto;
  }

  .summary-field .preview :global(code) {
    background: var(--bg-panel);
    padding: 1px 4px;
    border-radius: 3px;
    font-family: ui-monospace, monospace;
    font-size: 12px;
  }

  .summary-field .preview :global(pre code) {
    background: transparent;
    padding: 0;
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

  /* Warning tone — revset resolves to zero commits. Not an error
   * (jj accepted the expression), just a flag that submitting now
   * would produce a degenerate review. The submit button is also
   * disabled in this state. */
  .create-actions .revset-warning {
    color: var(--warn-text);
  }

  /* Error tone — jj couldn't parse / resolve the revset at all. */
  .create-actions .revset-error {
    color: var(--error-text);
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
{:else}
  <!-- Repo bar. The `<select>` only renders when there's a choice to
       make (>1 repo configured); when there's just the one repo it
       renders as plain text. The Browse link always renders so the
       single-repo case (the default install) doesn't lose its only
       path to the repository browser. -->
  <div class="repo-picker">
    {#if repos.length > 1}
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
    {:else}
      <strong class="repo-label">{repo}</strong>
    {/if}
    <a
      class="browse-link"
      href="/r/{encodeURIComponent(repo)}/browse"
      title="Browse the repository's log + files"
    >Browse</a>
  </div>
{/if}

<svelte:window onkeydown={onKey} />

<section class="home-section">
  <div class="review-list-header">
    {#if selectMode}
      <strong class="select-status">
        {selectedIds.size} selected
      </strong>
      <span style="flex: 1"></span>
      <button
        type="button"
        class="select-action"
        onclick={bulkArchive}
        disabled={bulkBusy || selectedIds.size === 0}
      >Archive</button>
      <button
        type="button"
        class="select-action danger"
        onclick={bulkDelete}
        disabled={bulkBusy || selectedIds.size === 0}
      >Delete…</button>
      <button
        type="button"
        class="select-action"
        onclick={exitSelectMode}
        disabled={bulkBusy}
      >Cancel</button>
    {:else}
      <h2>Reviews</h2>
      <span style="flex: 1"></span>
      {#if archivedSummaries.length > 0}
        <label class="archived-toggle">
          <input type="checkbox" bind:checked={showArchived} />
          Show archived ({archivedSummaries.length})
        </label>
      {/if}
      {#if (summaries ?? []).some(canManage)}
        <button
          type="button"
          class="select-toggle"
          onclick={enterSelectMode}
        >Select</button>
      {/if}
    {/if}
  </div>
  {#if loading && summaries === null}
    <p class="muted">Loading…</p>
  {:else if summaries && liveSummaries.length === 0 && archivedSummaries.length === 0}
    <p class="muted">No reviews yet. Start one below.</p>
  {:else if summaries && liveSummaries.length === 0 && !showArchived}
    <p class="muted">No active reviews. Toggle "Show archived" to see archived ones.</p>
  {:else if summaries}
    <ul class="review-list" data-tour="review-list" class:select-mode={selectMode}>
      {#each liveSummaries as s (s.manifest.review_id)}
        {@const id = s.manifest.review_id}
        {@const selectable = canManage(s)}
        <li class="row-shell" class:disabled={selectMode && !selectable}>
          {#if selectMode && selectable}
            <label class="row-check">
              <input
                type="checkbox"
                checked={selectedIds.has(id)}
                onchange={() => toggleSelect(id)}
                aria-label="Select review #{s.manifest.number}"
              />
            </label>
          {/if}
          <button
            class="row"
            onclick={() => (selectMode && selectable ? toggleSelect(id) : onopen(s.manifest.number))}
          >
            <span class="review-number">#{s.manifest.number}</span>
            <strong>{s.manifest.name}</strong>
            <span class="meta">{s.manifest.revset}</span>
            <span style="flex: 1"></span>
            <span class="meta">{s.published_comment_count} comments</span>
          </button>
          {#if !selectMode}
            <span class="row-actions">
              <ActionsMenu items={rowMenuItems(s)} label="Review actions" />
            </span>
          {/if}
        </li>
      {/each}
      {#if showArchived}
        {#each archivedSummaries as s (s.manifest.review_id)}
          {@const id = s.manifest.review_id}
          {@const selectable = canManage(s)}
          <li class="row-shell" class:disabled={selectMode && !selectable}>
            {#if selectMode && selectable}
              <label class="row-check">
                <input
                  type="checkbox"
                  checked={selectedIds.has(id)}
                  onchange={() => toggleSelect(id)}
                  aria-label="Select review #{s.manifest.number}"
                />
              </label>
            {/if}
            <button
              class="row archived"
              onclick={() => (selectMode && selectable ? toggleSelect(id) : onopen(s.manifest.number))}
            >
              <span class="review-number">#{s.manifest.number}</span>
              <strong>{s.manifest.name}</strong>
              <span class="meta">{s.manifest.revset}</span>
              <span class="archived-tag">archived</span>
              <span style="flex: 1"></span>
              <span class="meta">{s.published_comment_count} comments</span>
            </button>
            {#if !selectMode}
              <span class="row-actions">
                <ActionsMenu items={rowMenuItems(s)} label="Review actions" />
              </span>
            {/if}
          </li>
        {/each}
      {/if}
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
        <div class="summary-field">
          <div class="summary-field-header">
            <label for="new-review-summary">
              Summary <span class="muted">(optional, markdown)</span>
            </label>
            <span style="flex: 1"></span>
            <div class="tabs" role="tablist">
              <button
                type="button"
                class="tab {summaryMode === 'edit' ? 'active' : ''}"
                role="tab"
                aria-selected={summaryMode === 'edit'}
                onclick={() => (summaryMode = 'edit')}>Write</button
              >
              <button
                type="button"
                class="tab {summaryMode === 'preview' ? 'active' : ''}"
                role="tab"
                aria-selected={summaryMode === 'preview'}
                onclick={() => (summaryMode = 'preview')}>Preview</button
              >
            </div>
          </div>
          {#if summaryMode === 'edit'}
            <textarea
              id="new-review-summary"
              bind:value={summary}
              rows="4"
              placeholder="A short description of the change. Shown at the top of the review."
            ></textarea>
          {:else}
            <div class="preview markdown">
              {#if summary.trim().length > 0}
                {@html summaryPreview}
              {:else}
                <em class="muted">Nothing to preview.</em>
              {/if}
            </div>
          {/if}
        </div>
        <div class="create-actions">
          {#if revsetStatus?.kind === 'error'}
            <p class="hint revset-error">
              Revset error: <code>{revsetStatus.message}</code>
            </p>
          {:else if revsetStatus?.kind === 'ok' && revsetStatus.count === 0}
            <p class="hint revset-warning">
              This revset resolves to no commits — there'd be nothing to
              review. See
              <a href="https://jj-vcs.github.io/jj/latest/revsets/" target="_blank" rel="noreferrer">jj revsets</a>.
            </p>
          {/if}
          <button
            type="submit"
            class="primary"
            disabled={creating || !selected || !revset.trim() || revsetStatus?.kind !== 'ok' || revsetStatus.count === 0}
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
