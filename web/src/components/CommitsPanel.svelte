<script lang="ts">
  import type {
    CommentView,
    CommitInfo,
    ComposerTarget,
    DraftCommentInput,
    DraftResponseInput,
    PatchsetCompareView,
    ResolutionAction,
    ResponseView,
  } from '../lib/types';
  import { getContext } from 'svelte';
  import { renderMarkdown } from '../lib/markdown';
  import type { FoldStore } from '../lib/foldStore';
  import Bubble from './Bubble.svelte';
  import CommentComposer from './CommentComposer.svelte';
  import CommentThread from './CommentThread.svelte';

  interface Props {
    commits: CommitInfo[];
    /** All comments on the review (published + drafts). Used both to
     *  count per-commit on files this commit touched and to render
     *  commit-level / review-wide threads under their anchor row. */
    comments: CommentView[];
    /** Response views (published + drafts) for any commit-level threads
     *  rendered inline below. */
    responses: ResponseView[];
    selectedChangeId: string | null;
    /** Patchset currently being viewed. Threaded to CommentThread so
     *  each comment's "PS N" badge can render as a clickable jump. */
    currentPatchset: number;
    /** Anchor IDs review-wide comments are filed against — the current
     *  patchset's tip. The composer hands these to the server when the
     *  "All commits" bubble opens a new review-wide draft. */
    reviewAnchorIds: { change: string; commit: string };
    composing: ComposerTarget | null;
    saving: boolean;
    onselect: (changeId: string | null) => void;
    onstartcompose: (target: ComposerTarget) => void;
    oncancelcompose: () => void;
    onsubmit: (input: DraftCommentInput) => Promise<void>;
    onreply: (input: DraftResponseInput) => Promise<void>;
    onstatus: (commentId: string, action: ResolutionAction) => Promise<void>;
    ondelete: (comment: CommentView) => Promise<void>;
    onedit: (comment: CommentView) => void;
    onselectpatchset: (n: number, commentId?: string) => void;
    /** Timestamp of the viewer's previous open; threaded to
     *  CommentThread to drive the "new replies" badge. */
    lastVisitAt?: string | null;
    /** Currently signed-in author, so responses they wrote aren't
     *  counted as unread to themselves. */
    viewer?: string;
    /** When in patchset-compare mode, the pair-list summary the panel
     *  should render INSTEAD of the normal commit list. Each pair
     *  carries a status badge (= / ~ / + / -) and the row is clickable
     *  when the status is `changed` (the only one we have an interdiff
     *  for in the prototype). Null in normal mode. */
    compareView?: PatchsetCompareView | null;
    /** Which pair (by change_id) the user has selected for the per-commit
     *  compare view. Marks the row as selected. Null = cumulative
     *  landing view in compare mode. Ignored outside compare mode. */
    selectedCompareChange?: string | null;
    /** Fires when the user clicks a clickable pair row. `null` reverts
     *  to the cumulative view. Only called in compare mode. */
    onselectcomparecommit?: (changeId: string | null) => void;
    /** Surface a compare-fetch failure (compare summary or
     *  per-commit interdiff) inline in the panel instead of silently
     *  falling back to the normal commits view. Cleared by the
     *  parent on the next successful fetch. */
    compareError?: string | null;
    /** Gate for new-comment affordances in the panel (review-wide
     *  `+`, per-commit `+`, and the inline composer). Existing
     *  threads still render when false. Defaults to true. */
    commentsWriteable?: boolean;
  }
  const {
    commits,
    comments,
    responses,
    selectedChangeId,
    currentPatchset,
    reviewAnchorIds,
    composing,
    saving,
    onselect,
    onstartcompose,
    oncancelcompose,
    onsubmit,
    onreply,
    onstatus,
    ondelete,
    onedit,
    onselectpatchset,
    lastVisitAt = null,
    viewer = '',
    compareView = null,
    selectedCompareChange = null,
    onselectcomparecommit,
    compareError = null,
    commentsWriteable = true,
  }: Props = $props();

  let collapsed = $state(false);
  // Hide rows whose status is `same` in compare mode — useful for long
  // reviews where most commits don't change between rounds. Persisted
  // so the preference sticks across reloads.
  const HIDE_SAME_KEY = 'kata:compare:hide-same';
  let hideSameRows = $state(
    typeof localStorage !== 'undefined' &&
      localStorage.getItem(HIDE_SAME_KEY) === 'true',
  );
  $effect(() => {
    if (typeof localStorage === 'undefined') return;
    localStorage.setItem(HIDE_SAME_KEY, String(hideSameRows));
  });
  const visiblePairs = $derived.by(() => {
    if (!compareView) return [];
    if (!hideSameRows) return compareView.pairs;
    // "hide unchanged" filters both literal `same` rows and
    // `changed`-but-rebased-only rows. Description-only rewrites
    // stay visible since the metadata is genuinely different.
    return compareView.pairs.filter((p) => {
      if (p.status === 'same') return false;
      if (p.status === 'changed') {
        const descChanged =
          !!p.from_description &&
          !!p.to_description &&
          p.from_description !== p.to_description;
        const noContent =
          p.diff_counts != null && p.diff_counts.file_count === 0;
        if (noContent && !descChanged) return false;
      }
      return true;
    });
  });
  /** Commit-ids whose body is expanded. Body = lines after the first.
   *  Hydrated from the fold-store so an unfolded body survives reloads;
   *  only explicit unfolds are recorded (collapsed is the default). */
  const foldStore = getContext<FoldStore | undefined>('kata-fold-store');
  let expanded: Set<string> = $state(
    new Set(
      foldStore
        ? foldStore.ids('commit').filter((id) => foldStore.get('commit', id) === true)
        : [],
    ),
  );

  /** When an existing draft is being edited, hide it from the thread so
   *  the composer below takes its visual slot instead of stacking under
   *  the original draft bubble. */
  const editingCommentId = $derived(composing?.editing?.commentId ?? null);

  /** commit_id → number of comments on a file this commit touched.
   *  Doesn't include commit-level threads — those render inline. */
  const countByCommit = $derived.by(() => {
    const counts = new Map<string, number>();
    for (const c of commits) {
      const files = new Set(c.changed_files);
      let n = 0;
      for (const cm of comments) {
        if (cm.file && files.has(cm.file)) n++;
      }
      counts.set(c.commit_id, n);
    }
    return counts;
  });

  /** Comments with no file at all, split into:
   *   - `byChange`: keyed by `anchor_change_id` for comments tied to a
   *     specific commit in the visible revset (commit-level, rendered
   *     under that commit's row);
   *   - `reviewWide`: explicitly review-wide comments PLUS commit-level
   *     orphans whose anchor commit is no longer in the revset — both
   *     surface under the "All commits" row.
   *  Sorted by creation time within each bucket so threads read
   *  top-to-bottom. */
  const buckets = $derived.by(() => {
    const known = new Set(commits.map((c) => c.change_id));
    const byChange = new Map<string, CommentView[]>();
    const reviewWide: CommentView[] = [];
    for (const cm of comments) {
      if (cm.file != null) continue;
      if (cm.review_wide || !known.has(cm.anchor_change_id)) {
        reviewWide.push(cm);
      } else {
        const list = byChange.get(cm.anchor_change_id) ?? [];
        list.push(cm);
        byChange.set(cm.anchor_change_id, list);
      }
    }
    const cmp = (a: CommentView, b: CommentView) =>
      a.created_at.localeCompare(b.created_at);
    for (const list of byChange.values()) list.sort(cmp);
    reviewWide.sort(cmp);
    return { byChange, reviewWide };
  });

  /** Compare-mode variant of `buckets`. Same shape, but the "known
   *  change-ids" set is taken from the pair list — not from `commits`
   *  — so commit-level threads anchored to a `removed-from-from`
   *  commit still attach to their pair row rather than falling into
   *  the review-wide bucket. Returns `null` outside compare mode so
   *  the template can branch on it cheaply. */
  const compareBuckets = $derived.by(() => {
    if (!compareView) return null;
    const pairChangeIds = new Set(compareView.pairs.map((p) => p.change_id));
    const byChange = new Map<string, CommentView[]>();
    const reviewWide: CommentView[] = [];
    for (const cm of comments) {
      if (cm.file != null) continue;
      if (cm.review_wide || !pairChangeIds.has(cm.anchor_change_id)) {
        reviewWide.push(cm);
      } else {
        const list = byChange.get(cm.anchor_change_id) ?? [];
        list.push(cm);
        byChange.set(cm.anchor_change_id, list);
      }
    }
    const cmp = (a: CommentView, b: CommentView) =>
      a.created_at.localeCompare(b.created_at);
    for (const list of byChange.values()) list.sort(cmp);
    reviewWide.sort(cmp);
    return { byChange, reviewWide };
  });

  function short(id: string): string {
    return id.length > 12 ? id.slice(0, 12) : id;
  }

  function formatDate(iso: string): string {
    const d = new Date(iso);
    if (isNaN(d.getTime())) return iso;
    return d.toLocaleString();
  }

  /** Description minus the first line. Useful for "more details" toggles. */
  function bodyAfterFirstLine(description: string): string {
    const i = description.indexOf('\n');
    if (i < 0) return '';
    return description.slice(i + 1).replace(/\s+$/, '');
  }

  function toggleExpanded(commitId: string) {
    const next = new Set(expanded);
    if (next.has(commitId)) {
      next.delete(commitId);
      foldStore?.set('commit', commitId, false);
    } else {
      next.add(commitId);
      foldStore?.set('commit', commitId, true);
    }
    expanded = next;
  }

  function isComposingHere(changeId: string): boolean {
    return (
      composing?.kind === 'commit' && composing.change_id === changeId
    );
  }

  const isComposingReviewWide = $derived(composing?.kind === 'review');
</script>

<section class="commits">
  <header>
    <button
      class="toggle"
      aria-label={collapsed ? 'expand' : 'collapse'}
      onclick={() => (collapsed = !collapsed)}
    >
      {collapsed ? '▸' : '▾'}
    </button>
    <h3>Commits ({commits.length})</h3>
    {#if selectedChangeId !== null}
      <button class="show-all" onclick={() => onselect(null)}>Show all commits</button>
    {/if}
  </header>
  {#if !collapsed}
    {#if compareError}
      <p class="compare-error" role="alert">
        Compare-mode fetch failed: {compareError}
      </p>
    {/if}
    {#if compareView}
      <!-- Patchset-compare v2 prototype: per-change-id pair list.
           Status legend rendered as inline badges next to each row.
           Clickable rows have an interdiff endpoint pair the backend
           was able to resolve (`changed` always; added/removed when
           parent resolution succeeded). -->
      {#if compareView.compare_base_mismatch}
        <!-- Bases differ between the two patchsets — most likely the
             upstream branch moved between rounds. Cumulative diff +
             interdiff results will both reflect that upstream
             movement, not just author edits. A future iteration could
             reproject in-memory; today we just flag it. -->
        <p class="muted compare-warn">
          ⚠ Bases differ between PS{compareView.from.n}
          <code>({compareView.from.base_commit.slice(0, 12)})</code>
          and PS{compareView.to.n}
          <code>({compareView.to.base_commit.slice(0, 12)})</code>.
          The diff includes upstream movement, not just author edits.
        </p>
      {/if}
      {@const pairCounts = (() => {
        let same = 0, changed = 0, rebasedOnly = 0, added = 0, removed = 0;
        for (const p of compareView.pairs) {
          if (p.status === 'same') {
            same++;
          } else if (p.status === 'changed') {
            // Treat as "rebased only" when the rebase-based interdiff
            // came back empty (no actual content delta — the commit-id
            // changed only because its parent did). Description-only
            // rewrites still count as `changed` since the metadata
            // changed even if the content didn't.
            const descOnly =
              !!p.from_description &&
              !!p.to_description &&
              p.from_description !== p.to_description;
            if (
              p.diff_counts != null &&
              p.diff_counts.file_count === 0 &&
              !descOnly
            ) {
              rebasedOnly++;
            } else {
              changed++;
            }
          } else if (p.status === 'added-in-to') {
            added++;
          } else {
            removed++;
          }
        }
        return { same, changed, rebasedOnly, added, removed };
      })()}
      <p class="muted compare-summary">
        <span class="count count-changed">{pairCounts.changed} changed</span>
        ·
        <span class="count count-added">{pairCounts.added} added</span>
        ·
        <span class="count count-removed">{pairCounts.removed} removed</span>
        {#if pairCounts.rebasedOnly > 0}
          ·
          <span class="count count-rebased-only" title="commit-id changed only because its parent did; no content delta">
            {pairCounts.rebasedOnly} rebased
          </span>
        {/if}
        ·
        <span class="count count-same">{pairCounts.same} same</span>
        {#if pairCounts.same + pairCounts.rebasedOnly > 0}
          <label class="hide-same-toggle" title="Hide rows that aren't a real edit (same + rebased-only)">
            <input
              type="checkbox"
              bind:checked={hideSameRows}
            />
            hide unchanged
          </label>
        {/if}
      </p>
      <ul class="commit-list compare-pairs">
        <li class="commit pair-row {selectedCompareChange === null ? 'selected' : ''}">
          <div class="row">
            <span class="expand placeholder" aria-hidden="true"></span>
            <button
              class="row-button"
              onclick={() => onselectcomparecommit?.(null)}
            >
              <span class="all-label">Cumulative</span>
              <span class="meta">
                PS{compareView.from.n} → PS{compareView.to.n} (all commits combined)
              </span>
            </button>
          </div>
        </li>
        {#each visiblePairs as p (p.change_id)}
          {@const status = p.status}
          {@const clickable =
            (status === 'changed' && !!p.from_commit && !!p.to_commit) ||
            (status === 'added-in-to' && !!p.parent_commit && !!p.to_commit) ||
            (status === 'removed-from-from' &&
              !!p.parent_commit &&
              !!p.from_commit)}
          {@const desc =
            p.to_description ?? p.from_description ?? '(no description)'}
          {@const descChanged =
            status === 'changed' &&
            !!p.from_description &&
            !!p.to_description &&
            p.from_description !== p.to_description}
          {@const isRebasedOnly =
            status === 'changed' &&
            p.diff_counts != null &&
            p.diff_counts.file_count === 0 &&
            !descChanged}
          {@const badgeChar =
            isRebasedOnly
              ? '↻'
              : status === 'same'
                ? '='
                : status === 'changed'
                  ? '~'
                  : status === 'added-in-to'
                    ? '+'
                    : '−'}
          {@const statusLabel =
            isRebasedOnly ? 'rebased only' : status}
          {@const pairThreads =
            compareBuckets?.byChange.get(p.change_id) ?? []}
          {@const pairAnchorCommit =
            p.to_commit ?? p.from_commit ?? ''}
          {@const composingHere =
            composing?.kind === 'commit' &&
            composing.change_id === p.change_id}
          <li
            class="commit pair-row status-{status} {isRebasedOnly ? 'rebased-only' : ''} {clickable ? 'clickable' : 'inert'} {selectedCompareChange === p.change_id ? 'selected' : ''}"
          >
            <div class="row">
              <span class="status-badge" title={statusLabel}>{badgeChar}</span>
              <button
                class="row-button"
                disabled={!clickable}
                onclick={() => clickable && onselectcomparecommit?.(p.change_id)}
              >
                <span class="commit-id-cell">{p.change_id.slice(0, 12)}</span>
                <span class="description">{desc}</span>
                {#if p.diff_counts && !isRebasedOnly}
                  <span class="pair-counts" title="files changed / lines added / lines removed">
                    {p.diff_counts.file_count}f
                    <span class="adds">+{p.diff_counts.added}</span>
                    <span class="removes">−{p.diff_counts.removed}</span>
                  </span>
                {/if}
                {#if descChanged}
                  <!-- Description delta: same change-id, different
                       first-line subject. Common when an author
                       rewrites a commit message in response to a
                       "rename for clarity" comment. Hover the chip
                       to see both versions. -->
                  <span
                    class="desc-delta"
                    title={`from: ${p.from_description}\nto:   ${p.to_description}`}
                    >✎ desc</span
                  >
                {/if}
                <span class="meta">{statusLabel}</span>
              </button>
              {#if commentsWriteable && pairAnchorCommit}
                <button
                  type="button"
                  class="add-comment"
                  title="Comment on this commit"
                  aria-label="Comment on this commit"
                  disabled={composingHere}
                  onclick={(e) => {
                    e.stopPropagation();
                    onstartcompose({
                      kind: 'commit',
                      change_id: p.change_id,
                      commit_id: pairAnchorCommit,
                    });
                  }}
                >
                  <Bubble size={12} />
                </button>
              {/if}
            </div>
            {#if pairThreads.length > 0 || composingHere}
              <div class="commit-threads">
                {#if pairThreads.length > 0}
                  <CommentThread
                    comments={pairThreads}
                    {responses}
                    {saving}
                    {currentPatchset}
                    {editingCommentId}
                    {lastVisitAt}
                    {viewer}
                    {onreply}
                    {onstatus}
                    {ondelete}
                    {onedit}
                    {onselectpatchset}
                  />
                {/if}
                {#if composingHere && composing}
                  <CommentComposer
                    target={composing}
                    anchorIds={{
                      change: p.change_id,
                      commit: pairAnchorCommit,
                    }}
                    {saving}
                    oncancel={oncancelcompose}
                    onsubmit={onsubmit}
                  />
                {/if}
              </div>
            {/if}
          </li>
        {/each}
      </ul>
      <!-- "Review-wide" block in compare mode: just the threads (and
           composer) anchored to the review as a whole, plus orphan
           commit-level threads whose change-id isn't in any pair.
           Replaces the previous "show the whole commits list under
           the pair list" approach, which duplicated information for
           every commit row that had no threads. -->
      {#if (compareBuckets?.reviewWide.length ?? 0) > 0 ||
            isComposingReviewWide ||
            commentsWriteable}
        <div class="review-wide-block">
          <header class="review-wide-header">
            <span class="title">Review-wide</span>
            {#if commentsWriteable}
              <button
                type="button"
                class="add-comment"
                title="Comment on the whole review"
                aria-label="Comment on the whole review"
                disabled={isComposingReviewWide}
                onclick={() => onstartcompose({ kind: 'review' })}
              >
                <Bubble size={12} />
              </button>
            {/if}
          </header>
          {#if (compareBuckets?.reviewWide.length ?? 0) > 0}
            <CommentThread
              comments={compareBuckets?.reviewWide ?? []}
              {responses}
              {saving}
              {currentPatchset}
              {editingCommentId}
              {lastVisitAt}
              {viewer}
              {onreply}
              {onstatus}
              {ondelete}
              {onedit}
              {onselectpatchset}
            />
          {/if}
          {#if isComposingReviewWide && composing}
            <CommentComposer
              target={composing}
              anchorIds={reviewAnchorIds}
              {saving}
              oncancel={oncancelcompose}
              onsubmit={onsubmit}
            />
          {/if}
        </div>
      {/if}
    {:else if commits.length === 0}
      <p class="muted">The revset is empty.</p>
    {:else}
      <!-- Non-compare-mode commits panel: one row per commit in the
           selected patchset, with the "All commits" sentinel above
           that scopes the file diff back to the whole review. The
           compare-mode branch above handles the patchset-compare v2
           layout entirely separately. -->
      <ul class="commit-list">
        <li class="commit {selectedChangeId === null ? 'selected' : ''}">
          <div class="row">
            <span class="expand placeholder" aria-hidden="true"></span>
            <button class="row-button" onclick={() => onselect(null)}>
              <span class="all-label">All commits</span>
              <span class="meta">show the combined diff for the whole review</span>
            </button>
            <button
              type="button"
              class="add-comment"
              title="Comment on the whole review"
              aria-label="Comment on the whole review"
              disabled={isComposingReviewWide}
              onclick={(e) => {
                e.stopPropagation();
                onstartcompose({ kind: 'review' });
              }}
            >
              <Bubble size={12} />
            </button>
          </div>
          {#if buckets.reviewWide.length > 0 || isComposingReviewWide}
            <div class="commit-threads">
              {#if buckets.reviewWide.length > 0}
                <CommentThread
                  comments={buckets.reviewWide}
                  {responses}
                  {saving}
                  {currentPatchset}
                  {editingCommentId}
                  {lastVisitAt}
                  {viewer}
                  {onreply}
                  {onstatus}
                  {ondelete}
                  {onedit}
                  {onselectpatchset}
                />
              {/if}
              {#if isComposingReviewWide && composing}
                <CommentComposer
                  target={composing}
                  anchorIds={reviewAnchorIds}
                  {saving}
                  oncancel={oncancelcompose}
                  onsubmit={onsubmit}
                />
              {/if}
            </div>
          {/if}
        </li>
        {#each commits as c (c.commit_id)}
          {@const body = bodyAfterFirstLine(c.description)}
          {@const isExpanded = expanded.has(c.commit_id)}
          {@const count = countByCommit.get(c.commit_id) ?? 0}
          {@const threads = buckets.byChange.get(c.change_id) ?? []}
          {@const composingHere = isComposingHere(c.change_id)}
          <li class="commit {selectedChangeId === c.change_id ? 'selected' : ''}">
            <div class="row">
              {#if body}
                <button
                  class="expand"
                  aria-label={isExpanded ? 'collapse description' : 'expand description'}
                  aria-expanded={isExpanded}
                  title={isExpanded ? 'Collapse' : 'Show full description'}
                  onclick={() => toggleExpanded(c.commit_id)}
                >
                  {isExpanded ? '▾' : '▸'}
                </button>
              {:else}
                <span class="expand placeholder" aria-hidden="true"></span>
              {/if}
              <button class="row-button" onclick={() => onselect(c.change_id)}>
                <div class="row1">
                  <code class="change">{short(c.change_id)}</code>
                  <code class="commit">{short(c.commit_id)}</code>
                  <span class="desc">{c.description_first_line || '(no description)'}</span>
                  {#if count > 0}
                    <span
                      class="comment-count"
                      title="{count} comment{count === 1 ? '' : 's'} on files this commit touched"
                    >
                      {count} comment{count === 1 ? '' : 's'}
                    </span>
                  {/if}
                </div>
                <div class="row2">
                  <span class="meta">{c.author_email}</span>
                  <span class="meta">·</span>
                  <span class="meta" title={c.author_timestamp}>{formatDate(c.author_timestamp)}</span>
                </div>
              </button>
              <button
                type="button"
                class="add-comment"
                title="Comment on this commit"
                aria-label="Comment on this commit"
                disabled={composingHere}
                onclick={(e) => {
                  e.stopPropagation();
                  onstartcompose({
                    kind: 'commit',
                    change_id: c.change_id,
                    commit_id: c.commit_id,
                  });
                }}
              >
                <Bubble size={12} />
              </button>
            </div>
            {#if isExpanded && body}
              <div class="body markdown">{@html renderMarkdown(body)}</div>
            {/if}
            {#if threads.length > 0 || composingHere}
              <div class="commit-threads">
                {#if threads.length > 0}
                  <CommentThread
                    comments={threads}
                    {responses}
                    {saving}
                    {currentPatchset}
                    {editingCommentId}
                    {lastVisitAt}
                    {viewer}
                    {onreply}
                    {onstatus}
                    {ondelete}
                    {onedit}
                    {onselectpatchset}
                  />
                {/if}
                {#if composingHere && composing}
                  <CommentComposer
                    target={composing}
                    anchorIds={{ change: c.change_id, commit: c.commit_id }}
                    {saving}
                    oncancel={oncancelcompose}
                    onsubmit={onsubmit}
                  />
                {/if}
              </div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</section>

<style>
  .commits {
    border: 1px solid var(--border);
    border-radius: 6px;
    margin: 16px 0;
    overflow: hidden;
  }

  .commits header {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 8px 12px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
  }

  .commits header h3 {
    margin: 0;
  }

  .toggle {
    background: transparent;
    border: none;
    padding: 0;
    font-size: 16px;
    line-height: 1;
    color: var(--text-muted);
    cursor: pointer;
    width: 20px;
  }

  .commit-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .commit {
    border-bottom: 1px solid var(--bg-elevated);
  }

  .commit:last-child {
    border-bottom: none;
  }

  .commit.selected {
    background: var(--link-bg);
  }

  .row-button {
    flex: 1;
    min-width: 0;
    background: transparent;
    border: none;
    border-radius: 0;
    padding: 8px 12px;
    text-align: left;
    cursor: pointer;
    display: flex;
    flex-direction: column;
    gap: 2px;
    /* Allow text selection within the row so users can copy a change id
     * / commit id / title. A real click still selects the commit because
     * mousedown+mouseup at the same point doesn't trigger a selection. */
    user-select: text;
  }

  .row-button:hover {
    background: var(--bg-panel);
  }

  .commit.selected .row-button:hover {
    background: var(--link-bg); filter: brightness(0.9);
  }

  .all-label {
    font-weight: 500;
  }

  .show-all {
    margin-left: auto;
    font-size: 12px;
    padding: 2px 8px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--link);
    cursor: pointer;
  }

  .row1 {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }

  .row1 .change,
  .row1 .commit {
    color: var(--text-muted);
    background: var(--bg-elevated);
    padding: 1px 5px;
    border-radius: 3px;
    font-size: 11px;
  }

  .row1 .desc {
    font-weight: 500;
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .comment-count {
    flex: 0 0 auto;
    margin-left: auto;
    font-size: 11px;
    font-weight: 500;
    color: var(--link);
    background: var(--link-bg);
    padding: 1px 7px;
    border-radius: 9999px;
    line-height: 1.4;
  }

  .row2 {
    display: flex;
    gap: 6px;
    font-size: 12px;
    color: var(--text-muted);
  }

  .row {
    display: flex;
    align-items: stretch;
  }

  .expand {
    flex: 0 0 auto;
    width: 28px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 16px;
    line-height: 1;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }

  .expand:hover {
    color: var(--text);
    background: var(--bg-panel);
  }

  .commit.selected .expand:hover {
    background: var(--link-bg); filter: brightness(0.9);
  }

  .expand.placeholder {
    cursor: default;
    pointer-events: none;
  }

  /* Per-row comment-bubble button. Mirrors the file-gutter affordance
   * in HunkLines: hidden until the row is hovered, then a small
   * link-coloured chip that turns solid on its own hover. */
  .add-comment {
    flex: 0 0 auto;
    align-self: center;
    width: 22px;
    height: 22px;
    margin-right: 6px;
    padding: 0;
    border: 1px solid transparent;
    border-radius: 4px;
    background: transparent;
    color: var(--link);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    visibility: hidden;
    user-select: none;
  }

  .row:hover .add-comment,
  .add-comment:focus-visible {
    visibility: visible;
  }

  .add-comment:hover:not(:disabled) {
    background: var(--link);
    color: var(--on-accent);
    border-color: var(--link);
  }

  .add-comment:disabled {
    cursor: default;
    opacity: 0.4;
  }

  .commit-threads {
    padding: 0 12px 12px 36px;
  }

  .body {
    /* Negative top to tuck the description directly under the subject — the
     * .row-button's own bottom padding (8px) is already the visual gap. */
    margin: -4px 0 0;
    padding: 0 12px 12px 36px;
    background: transparent;
    font-size: 13px;
    color: var(--text);
  }

  /* Tighten markdown defaults so paragraphs/lists don't add their own
   * vertical padding inside the description. */
  .body :global(p),
  .body :global(ul),
  .body :global(ol),
  .body :global(pre) {
    margin: 4px 0;
  }
  .body :global(p:first-child),
  .body :global(ul:first-child),
  .body :global(ol:first-child),
  .body :global(pre:first-child) {
    margin-top: 0;
  }
  .body :global(p:last-child),
  .body :global(ul:last-child),
  .body :global(ol:last-child),
  .body :global(pre:last-child) {
    margin-bottom: 0;
  }

  /* Patchset-compare v2 prototype styling. The badges colour-code the
   * four pair statuses; inert rows fade out so the eye lands on the
   * clickable ones. */
  .compare-warn {
    padding: 6px 12px;
    margin: 0;
    font-size: 12px;
    background: var(--banner-warn-bg, #fff7e0);
    border-bottom: 1px solid var(--border);
  }
  .compare-warn code {
    background: var(--bg-elevated);
    padding: 0 4px;
    border-radius: 3px;
    font-size: 11px;
  }
  .pair-row .row {
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 4px 8px;
  }
  .status-badge {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 18px;
    height: 18px;
    font-family: var(--mono, monospace);
    font-size: 12px;
    border-radius: 3px;
    color: white;
    flex: 0 0 auto;
  }
  .status-same .status-badge {
    background: #999;
  }
  .status-changed .status-badge {
    background: #e08e00;
  }
  .status-added-in-to .status-badge {
    background: #2e8b3a;
  }
  .status-removed-from-from .status-badge {
    background: #c0392b;
  }
  .pair-row.inert {
    opacity: 0.55;
  }
  .pair-row .commit-id-cell {
    font-family: var(--mono, monospace);
    font-size: 12px;
    color: var(--muted);
    margin-right: 8px;
  }
  .pair-row .description {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .desc-delta {
    font-size: 11px;
    color: var(--link, #1f6feb);
    background: var(--banner-info-bg, #eef4ff);
    padding: 1px 6px;
    border-radius: 4px;
    flex: 0 0 auto;
    margin-left: 4px;
    cursor: help;
  }
  .pair-counts {
    flex: 0 0 auto;
    font-size: 11px;
    color: var(--muted);
    font-family: var(--mono, monospace);
    margin-left: 4px;
    display: inline-flex;
    gap: 4px;
    align-items: baseline;
  }
  .pair-counts .adds {
    color: #2e8b3a;
  }
  .pair-counts .removes {
    color: #c0392b;
  }
  .compare-summary {
    padding: 6px 12px;
    margin: 0;
    font-size: 11px;
    border-bottom: 1px solid var(--border);
    display: flex;
    gap: 6px;
    align-items: baseline;
    flex-wrap: wrap;
  }
  .compare-summary .count {
    font-weight: 500;
  }
  .compare-summary .count-changed {
    color: #e08e00;
  }
  .compare-summary .count-added {
    color: #2e8b3a;
  }
  .compare-summary .count-removed {
    color: #c0392b;
  }
  .compare-summary .count-same {
    color: var(--muted);
  }
  .compare-summary .count-rebased-only {
    color: var(--muted);
    cursor: help;
  }
  .pair-row.rebased-only {
    opacity: 0.7;
  }
  .pair-row.rebased-only .status-badge {
    background: #999;
  }
  .pair-row.rebased-only .description {
    color: var(--muted);
  }
  .hide-same-toggle {
    margin-left: auto;
    font-size: 11px;
    color: var(--muted);
    display: inline-flex;
    align-items: center;
    gap: 4px;
    cursor: pointer;
  }
  .hide-same-toggle input {
    margin: 0;
  }
  .review-wide-block {
    border-top: 1px solid var(--border);
    padding: 8px 12px 12px;
  }
  .review-wide-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 8px;
  }
  .review-wide-header .title {
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted);
  }
  .review-wide-header .add-comment {
    /* Always visible in this header (no hover-to-reveal) since it's
       the only entry point for review-wide comments in compare mode. */
    opacity: 1;
  }
  .compare-error {
    padding: 8px 12px;
    margin: 0;
    font-size: 12px;
    background: var(--banner-error-bg, #fdecea);
    color: var(--banner-error-fg, #b00020);
    border-bottom: 1px solid var(--border);
  }
  .compare-section-label {
    padding: 10px 12px 4px;
    margin: 0;
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--muted);
    border-top: 1px solid var(--border);
  }
  .pair-row .meta {
    font-size: 11px;
    color: var(--muted);
    text-transform: lowercase;
    margin-left: 8px;
  }
</style>
