<script lang="ts">
  import type {
    CommentView,
    CommitInfo,
    ComposerTarget,
    DraftCommentInput,
    DraftResponseInput,
    ResolutionAction,
    ResponseView,
  } from '../lib/types';
  import { renderMarkdown } from '../lib/markdown';
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
  }: Props = $props();

  let collapsed = $state(false);
  /** Commit-ids whose body is expanded. Body = lines after the first. */
  let expanded: Set<string> = $state(new Set());

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
    if (next.has(commitId)) next.delete(commitId);
    else next.add(commitId);
    expanded = next;
  }

  function isComposingHere(changeId: string): boolean {
    return (
      composing?.kind === 'commit' &&
      composing.change_id === changeId &&
      !composing.editing
    );
  }

  const isComposingReviewWide = $derived(
    composing?.kind === 'review' && !composing.editing,
  );
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
    {#if commits.length === 0}
      <p class="muted">The revset is empty.</p>
    {:else}
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
</style>
