<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import { subscribe as subscribeEvents } from '../lib/events';
  import type {
    CommentView,
    ComposerTarget,
    Diff,
    DraftCommentInput,
    DraftResponseInput,
    ResponseView,
    ReviewView,
  } from '../lib/types';
  import { sortFilesLikeTree } from '../lib/tree';
  import { setTokenizationPaused } from '../lib/highlight.svelte';
  import CommentComposer from './CommentComposer.svelte';
  import CommentThread from './CommentThread.svelte';
  import CommitsPanel from './CommitsPanel.svelte';
  import FileSlot from './FileSlot.svelte';
  import FileTree from './FileTree.svelte';

  /** State + action callbacks for the global publish/discard toolbar that
   *  App.svelte renders in the app header. Re-emitted whenever drafts or
   *  `saving` change; null when there's nothing to publish. */
  export interface DraftBarState {
    count: number;
    saving: boolean;
    publish: () => Promise<void>;
    discard: () => Promise<void>;
  }

  interface Props {
    repo: string;
    view: ReviewView;
    /** Patchset to start on. Undefined means "the latest". */
    initialPatchset?: number;
    /** Fires when the user picks a different patchset from the dropdown.
     *  Does not fire when the viewer auto-follows a newly-appended patchset
     *  (so the URL stays clean if the user wasn't pinning a specific PS). */
    onpatchsetchange?: (n: number) => void;
    /** Reports draft toolbar state up to the app shell so the publish /
     *  discard controls can live in the always-visible top bar instead of
     *  scrolling away with the page. */
    ondraftbarchange?: (bar: DraftBarState | null) => void;
  }
  let {
    repo,
    view,
    initialPatchset,
    onpatchsetchange,
    ondraftbarchange,
  }: Props = $props();

  // We seed local state from the prop and then manage refreshes ourselves.
  // svelte-ignore state_referenced_locally
  let current: ReviewView = $state(view);
  // svelte-ignore state_referenced_locally
  let selectedPatchset = $state(initialPatchset ?? view.manifest.current_patchset);
  // Use raw state so reads of composing.* don't create thousands of
  // per-property signal subscriptions across rows. We always reassign the
  // whole object (never mutate fields), so the granular reactivity isn't
  // useful here.
  let composing: ComposerTarget | null = $state.raw(null);
  let saving = $state(false);
  let error: string | null = $state(null);

  /** Mirror draft state up to the app shell whenever it changes. The shell
   *  renders the publish / discard buttons in the sticky top bar; keeping
   *  the state authoritative here means the actual API calls stay local. */
  $effect(() => {
    const hasDrafts =
      !!current.drafts.session && current.drafts.comments.length > 0;
    ondraftbarchange?.(
      hasDrafts
        ? {
            count: current.drafts.comments.length,
            saving,
            publish,
            discard,
          }
        : null,
    );
  });

  /** Make sure the toolbar clears when the viewer unmounts (e.g. user
   *  navigates back to the review list). */
  onMount(() => () => ondraftbarchange?.(null));

  /** Pause tokenization while a composer is open so the user can type
   *  without input lag — `codeToTokensBase` is synchronous (~200-500ms
   *  per big file) and tokenize bursts triggered by the layout shift of
   *  mounting the composer would otherwise queue keystrokes behind them. */
  $effect(() => {
    if (composing) {
      setTokenizationPaused(true);
      return () => setTokenizationPaused(false);
    }
  });

  /** The patchset the viewer is currently looking at. Falls back to the
   *  manifest's current patchset if `selectedPatchset` somehow drifted. */
  const viewing = $derived(
    current.manifest.patchsets.find((p) => p.n === selectedPatchset) ??
      current.manifest.patchsets.find((p) => p.n === current.manifest.current_patchset)!,
  );

  /** When non-null, the diff is scoped to a single commit instead of the
   *  full review range. The full ReviewView (comments, drafts, etc.) is
   *  still loaded — only the diff display changes. */
  let scopedChangeId: string | null = $state(null);
  let scopedDiff: Diff | null = $state(null);

  const displayedDiff = $derived(scopedDiff ?? current.diff);
  /** Files reordered to match the file tree's DFS traversal so the diff
   *  panel reads top-to-bottom the way the sidebar does. */
  const orderedFiles = $derived(sortFilesLikeTree(displayedDiff.files));

  // Sidebar layout state, persisted to localStorage.
  const TREE_WIDTH_KEY = 'kata:treeWidth';
  const TREE_COLLAPSED_KEY = 'kata:treeCollapsed';
  const DEFAULT_TREE_WIDTH = 280;

  function readNumber(key: string, fallback: number): number {
    if (typeof localStorage === 'undefined') return fallback;
    const v = localStorage.getItem(key);
    const n = v == null ? NaN : Number(v);
    return Number.isFinite(n) ? n : fallback;
  }

  let treeCollapsed = $state(
    typeof localStorage !== 'undefined' &&
      localStorage.getItem(TREE_COLLAPSED_KEY) === 'true',
  );
  let treeWidth = $state(readNumber(TREE_WIDTH_KEY, DEFAULT_TREE_WIDTH));

  $effect(() => {
    if (typeof localStorage === 'undefined') return;
    localStorage.setItem(TREE_COLLAPSED_KEY, String(treeCollapsed));
  });
  $effect(() => {
    if (typeof localStorage === 'undefined') return;
    localStorage.setItem(TREE_WIDTH_KEY, String(treeWidth));
  });

  function startResize(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    const startX = e.clientX;
    const startW = treeWidth;
    const onMove = (ev: PointerEvent) => {
      treeWidth = Math.max(180, Math.min(640, startW + (ev.clientX - startX)));
    };
    const onUp = () => {
      document.removeEventListener('pointermove', onMove);
      document.removeEventListener('pointerup', onUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };
    document.addEventListener('pointermove', onMove);
    document.addEventListener('pointerup', onUp);
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  }

  /** Separate from `saving` (which is about write ops): true while a
   *  commit-scoped diff is being fetched, so the UI can show feedback. */
  let loadingDiff = $state(false);
  /** What we're loading, for the banner — easier to scan than just a spinner. */
  let loadingDiffLabel = $state('');

  async function selectCommit(changeId: string | null) {
    if (changeId === null) {
      scopedChangeId = null;
      scopedDiff = null;
      return;
    }
    loadingDiff = true;
    loadingDiffLabel = changeId.slice(0, 12);
    error = null;
    try {
      scopedDiff = await api.commitDiff(repo, current.manifest.review_id, changeId);
      scopedChangeId = changeId;
    } catch (e) {
      error = (e as Error).message;
    } finally {
      loadingDiff = false;
    }
  }

  const allComments: CommentView[] = $derived([
    ...current.comments,
    ...current.drafts.comments,
  ]);

  const allResponses: ResponseView[] = $derived([
    ...current.responses,
    ...current.drafts.responses,
  ]);

  /** Whole-review comments (no file, no lines). */
  const reviewComments: CommentView[] = $derived(
    allComments.filter((c) => c.file == null),
  );


  function short(id: string): string {
    return id.length > 12 ? id.slice(0, 12) : id;
  }

  async function refresh() {
    const wasOnLatest = selectedPatchset === current.manifest.current_patchset;
    const next = await api.openReview(
      repo,
      current.manifest.review_id,
      selectedPatchset,
    );
    // If the user was tracking the latest patchset and a new one just landed,
    // follow it forward; otherwise stay where they are.
    if (wasOnLatest && next.manifest.current_patchset !== selectedPatchset) {
      current = await api.openReview(
        repo,
        current.manifest.review_id,
        next.manifest.current_patchset,
      );
      selectedPatchset = current.manifest.current_patchset;
    } else {
      current = next;
    }
  }

  async function selectPatchset(n: number) {
    if (n === selectedPatchset) return;
    saving = true;
    error = null;
    try {
      current = await api.openReview(repo, current.manifest.review_id, n);
      selectedPatchset = n;
      // Discarding the per-commit scope: it was tied to the previous PS.
      scopedChangeId = null;
      scopedDiff = null;
      onpatchsetchange?.(n);
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  /** Auto-refresh on any public state change for this review so the user
   *  sees other authors' published comments / status flips without a
   *  manual reload. Drafts are local-only so they don't trigger events. */
  onMount(() =>
    subscribeEvents((event) => {
      if (
        event.repo === repo &&
        (event.kind === 'session-published' ||
          event.kind === 'session-discarded' ||
          event.kind === 'review-updated') &&
        event.review_id === current.manifest.review_id
      ) {
        void refresh();
      }
    }),
  );

  const reviewAnchorIds = $derived({
    change: viewing.tip_change,
    commit: viewing.tip_commit,
  });

  function startCompose(target: ComposerTarget) {
    composing = target;
  }

  function cancelCompose() {
    composing = null;
  }

  async function ensureSession(): Promise<string> {
    let sid = current.drafts.session?.session_id;
    if (!sid) {
      const session = await api.startSession(repo, current.manifest.review_id);
      sid = session.session_id;
    }
    return sid;
  }

  async function submitComment(input: DraftCommentInput) {
    saving = true;
    error = null;
    try {
      const sid = await ensureSession();
      await api.createComment(repo, current.manifest.review_id, sid, input);
      await refresh();
      composing = null;
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  async function submitResponse(input: DraftResponseInput) {
    saving = true;
    error = null;
    try {
      const sid = await ensureSession();
      await api.createResponse(repo, current.manifest.review_id, sid, input);
      await refresh();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  async function setStatus(commentId: string, action: import('../lib/types').ResolutionAction) {
    await submitResponse({ in_reply_to: commentId, action, body: '' });
  }

  async function deleteComment(comment: CommentView) {
    if (!confirm('Delete this draft comment?')) return;
    saving = true;
    error = null;
    try {
      await api.deleteComment(
        repo,
        current.manifest.review_id,
        comment.session_id,
        comment.comment_id,
      );
      await refresh();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  async function publish() {
    if (!current.drafts.session) return;
    saving = true;
    error = null;
    try {
      await api.publishSession(
        repo,
        current.manifest.review_id,
        current.drafts.session.session_id,
      );
      await refresh();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  function scrollToFile(path: string) {
    const el = document.querySelector(
      `[data-file-path="${CSS.escape(path)}"]`,
    ) as HTMLElement | null;
    if (el) {
      el.scrollIntoView({ behavior: 'smooth', block: 'start' });
    }
  }

  async function discard() {
    if (!current.drafts.session) return;
    if (!confirm('Discard this draft session? Your drafts will be marked discarded.')) {
      return;
    }
    saving = true;
    error = null;
    try {
      await api.discardSession(
        repo,
        current.manifest.review_id,
        current.drafts.session.session_id,
      );
      await refresh();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }
</script>

<section class="header">
  <h2>{current.manifest.review_id}</h2>
  <p class="muted">
    {#if current.manifest.bookmark}bookmark: <strong>{current.manifest.bookmark}</strong> ·{/if}
    revset: <code>{current.manifest.revset}</code>
  </p>
  <p class="muted patchset-row">
    {#if current.manifest.patchsets.length > 1}
      <label>
        Patchset
        <select
          value={selectedPatchset}
          onchange={(e) =>
            selectPatchset(Number((e.currentTarget as HTMLSelectElement).value))}
        >
          {#each current.manifest.patchsets as p (p.n)}
            <option value={p.n}>
              PS{p.n}{p.n === current.manifest.current_patchset ? ' (latest)' : ''}{p.parent_patchset
                ? ''
                : p.n > 1
                  ? ' · rewritten'
                  : ''}
            </option>
          {/each}
        </select>
      </label>
      ·
    {/if}
    base <code>{short(viewing.base_change)}</code> → tip
    <code>{short(viewing.tip_change)}</code>
  </p>
</section>

{#if error}
  <p class="error">{error}</p>
{/if}

<div class="review-layout">
  <!-- The tree pane stays mounted and is toggled via CSS. Unmounting it
       (the old `{#if}` shape) rebuilt the full FileTree on every expand,
       which for a 100-file review tipped past a second of mount work. -->
  <aside
    class="tree-pane"
    class:hidden={treeCollapsed}
    style:width="{treeWidth}px"
  >
    <FileTree files={displayedDiff.files} onselect={scrollToFile}>
      {#snippet headerLeft()}
        <button
          class="tree-toggle"
          title="Hide files"
          onclick={() => (treeCollapsed = true)}
        >
          ◂
        </button>
      {/snippet}
    </FileTree>
  </aside>
  {#if treeCollapsed}
    <button
      class="tree-reopen"
      title="Show files"
      onclick={() => (treeCollapsed = false)}
    >
      ▸
    </button>
  {:else}
    <div
      class="tree-resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize file tree"
      onpointerdown={startResize}
    ></div>
  {/if}
  <div class="main-pane">
    <CommitsPanel
      commits={current.commits}
      comments={allComments}
      selectedChangeId={scopedChangeId}
      onselect={selectCommit}
    />

    {#if loadingDiff}
      <div class="diff-loading" role="status" aria-live="polite">
        <span class="spinner" aria-hidden="true"></span>
        Loading diff for <code>{loadingDiffLabel}</code>…
      </div>
    {/if}

    <section class="review-comments">
  <header>
    <h3>Review-wide comments</h3>
    <button
      type="button"
      class="primary"
      onclick={() => startCompose({ kind: 'review' })}
      disabled={composing?.kind === 'review'}
    >
      Add comment
    </button>
  </header>
  {#if reviewComments.length > 0}
    <CommentThread
      comments={reviewComments}
      responses={allResponses}
      {saving}
      onreply={submitResponse}
      onstatus={setStatus}
      ondelete={deleteComment}
    />
  {:else if !composing || composing.kind !== 'review'}
    <p class="muted">No review-wide comments yet.</p>
  {/if}
  {#if composing && composing.kind === 'review'}
    <CommentComposer
      target={composing}
      anchorIds={reviewAnchorIds}
      {saving}
      oncancel={cancelCompose}
      onsubmit={submitComment}
    />
  {/if}
</section>

{#if orderedFiles.length === 0}
      <p class="muted">No files changed.</p>
    {:else}
      {#each orderedFiles as f (f.path)}
        <!-- composing is narrowed to the targeted file only; other slots
             receive `null` and don't churn when the composer opens
             elsewhere. forceRender keeps the file hosting the composer
             alive in the DOM regardless of viewport, so the inline
             composer doesn't get virtualized out from under the user. -->
        <FileSlot
          {repo}
          file={f}
          patchset={viewing}
          comments={allComments}
          responses={allResponses}
          composing={composing &&
          'file' in composing &&
          composing.file === f.path
            ? composing
            : null}
          forceRender={!!(composing &&
            'file' in composing &&
            composing.file === f.path)}
          {saving}
          onstartcompose={startCompose}
          oncancelcompose={cancelCompose}
          onsubmit={submitComment}
          onreply={submitResponse}
          onstatus={setStatus}
          ondelete={deleteComment}
        />
      {/each}
    {/if}
  </div>
</div>

<style>
  .header {
    margin-bottom: 16px;
  }

  .review-layout {
    display: flex;
    align-items: flex-start;
    gap: 0;
  }

  .tree-pane {
    flex: 0 0 auto;
    position: sticky;
    top: calc(var(--app-header-h) + 16px);
    max-height: calc(100vh - var(--app-header-h) - 32px);
    /* Match the CommitsPanel's 16px top margin so the two panels align
     * along their top edges before sticky kicks in. */
    margin-top: 16px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  /* Hidden state for the tree pane: kept in the DOM (so its FileTree
   * children don't have to rebuild on re-open) but pulled out of layout. */
  .tree-pane.hidden {
    display: none;
  }

  /* The collapse toggle gets passed into FileTree's header via a snippet,
   * so this rule needs to apply across component boundaries. */
  :global(.tree-toggle),
  .tree-reopen {
    width: 22px;
    height: 22px;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg);
    color: var(--text-muted);
    cursor: pointer;
    font-size: 12px;
    line-height: 18px;
  }

  :global(.tree-toggle:hover),
  .tree-reopen:hover {
    background: var(--bg-elevated);
  }

  .tree-reopen {
    position: sticky;
    top: calc(var(--app-header-h) + 16px);
    margin-right: 8px;
    flex: 0 0 auto;
  }

  .tree-resizer {
    flex: 0 0 6px;
    cursor: col-resize;
    position: sticky;
    top: calc(var(--app-header-h) + 16px);
    height: calc(100vh - var(--app-header-h) - 32px);
    background: transparent;
    margin: 0 4px;
  }

  .tree-resizer:hover,
  .tree-resizer:active {
    background: var(--link);
    opacity: 0.4;
  }

  .main-pane {
    flex: 1;
    min-width: 0;
    margin-left: 8px;
  }

  .diff-loading {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 12px 0;
    padding: 10px 12px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-size: 13px;
    color: var(--text-muted);
  }

  .diff-loading code {
    background: var(--bg-elevated);
    padding: 1px 5px;
    border-radius: 3px;
    font-size: 12px;
  }

  .diff-loading .spinner {
    width: 12px;
    height: 12px;
    border: 2px solid var(--border);
    border-top-color: var(--link);
    border-radius: 50%;
    animation: diff-loading-spin 0.7s linear infinite;
  }

  @keyframes diff-loading-spin {
    to { transform: rotate(360deg); }
  }

  .review-comments {
    margin: 16px 0;
  }

  .review-comments header {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
  }

  .review-comments h3 {
    margin: 0;
  }
</style>
