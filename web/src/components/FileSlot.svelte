<script lang="ts">
  //! One slot per file in the review's file list. Decides whether to
  //! actually mount `FileDiff` for this file (when it's near the
  //! viewport) or render a same-height placeholder. This is the
  //! virtualization layer: only a handful of file-diffs exist in the DOM
  //! at any one time, which is what keeps the page small enough for an
  //! inline composer mount to be cheap.
  //!
  //! The slot tracks the actual rendered height of the underlying
  //! `FileDiff` via a ResizeObserver so the placeholder doesn't
  //! jitter the scroll position when the file scrolls back out of view.
  //! Before the file has ever been rendered, an estimate based on hunk
  //! line count is used.

  import { api } from '../lib/api';
  import type {
    CommentView,
    ComposerTarget,
    DraftResponseInput,
    FileChange,
    Patchset,
    ResolutionAction,
    ResponseView,
  } from '../lib/types';
  import FileDiff from './FileDiff.svelte';

  interface Props {
    repo: string;
    reviewNumber: number;
    file: FileChange;
    patchset: Patchset;
    /** When non-null, the viewer is in patchset-compare mode: per-file
     *  hunks must be fetched with the same `compare` query so they
     *  match the metadata response. */
    compareWith: number | null;
    /** Fetch the per-file diff up front, regardless of whether the
     *  slot is in the viewport. Set for files that have at least one
     *  comment — the comment-nav `< >` buttons need to land on those
     *  files reliably, and waiting for an in-flight fetch during the
     *  click made the page shift around as the slot mounted and
     *  upstream slots settled. The render still happens lazily; only
     *  the network round-trip is eager. */
    eagerFetch: boolean;
    comments: CommentView[];
    responses: ResponseView[];
    /** Patchset the viewer is currently showing. Threaded into
     *  CommentThread so each comment's "PS N" badge can render as a
     *  clickable jump when the comment came from a different round. */
    currentPatchset: number;
    composing: ComposerTarget | null;
    saving: boolean;
    /** Keep this file mounted regardless of viewport — used so the file
     *  currently hosting an open composer doesn't get virtualized away
     *  out from under the user. */
    forceRender: boolean;
    /** Render the file with its hunks hidden — only comments and the
     *  file header remain. */
    compact: boolean;
    onstartcompose: (target: ComposerTarget) => void;
    oncancelcompose: () => void;
    onsubmit: (input: import('../lib/types').DraftCommentInput) => Promise<void>;
    onreply: (input: DraftResponseInput) => Promise<void>;
    onstatus: (commentId: string, action: ResolutionAction) => Promise<void>;
    ondelete: (comment: CommentView) => Promise<void>;
    onedit: (comment: CommentView) => void;
    onselectpatchset: (n: number, commentId?: string) => void;
  }
  const {
    repo,
    reviewNumber,
    file,
    patchset,
    compareWith,
    eagerFetch,
    comments,
    responses,
    currentPatchset,
    composing,
    saving,
    forceRender,
    compact,
    onstartcompose,
    oncancelcompose,
    onsubmit,
    onreply,
    onstatus,
    ondelete,
    onedit,
    onselectpatchset,
  }: Props = $props();

  let slotEl: HTMLElement | undefined = $state();
  let wrapEl: HTMLElement | undefined = $state();
  let inViewport = $state(false);
  let lastKnownHeight = $state<number | null>(null);

  /** Lazy-fetched diff body. `open_review` ships only the file list
   *  (path, status, rename info) — hunks come in piecemeal as files
   *  scroll near the viewport. `resolved` holds the merged
   *  `FileChange` once the per-file fetch resolves; until then we
   *  fall back to `file` (which has `hunks: undefined`). */
  let resolved = $state<FileChange | null>(null);
  let loadingHunks = $state(false);
  let loadError = $state<string | null>(null);

  /** When the patchset or the compare target changes, the same path
   *  now describes a different delta — our cached `resolved` from a
   *  previous selection would otherwise stick around and the user
   *  would see stale hunks. Reset so the fetch effect below re-runs
   *  against the new endpoints. */
  $effect(() => {
    patchset.n;
    compareWith;
    resolved = null;
    loadError = null;
  });

  /** Fires when the file is close enough to be visible OR when the
   *  slot is marked `eagerFetch` (because the file carries one or
   *  more comments and the comment-nav needs the diff in cache to
   *  land reliably). Skip if the initial payload already had hunks
   *  (binary files, or a smaller endpoint that ships them eagerly).
   *
   *  `!= null` rather than `!== undefined` because the metadata
   *  endpoint serialises `hunks: None` as JSON `null` (not an absent
   *  field). Without that, every file would short-circuit here and
   *  the diff would stay stuck on "Diff omitted". */
  $effect(() => {
    if (!shouldRender && !eagerFetch) return;
    if (resolved !== null || loadingHunks) return;
    if (file.hunks != null || file.binary) return;
    loadingHunks = true;
    loadError = null;
    api
      .fileDiff(repo, reviewNumber, file.path, patchset.n, compareWith ?? undefined)
      .then((updated) => {
        resolved = updated;
      })
      .catch((e: Error) => {
        loadError = e.message;
      })
      .finally(() => {
        loadingHunks = false;
      });
  });

  /** What we actually hand to `FileDiff`: the lazy-loaded one if the
   *  fetch has resolved, otherwise the metadata-only original. */
  const effectiveFile = $derived(resolved ?? file);

  /** Generous rootMargin so files don't churn mount/unmount during normal
   *  scrolling — we keep ~3 viewport-heights' worth of files alive at a
   *  time. */
  $effect(() => {
    if (!slotEl) return;
    const io = new IntersectionObserver(
      (entries) => {
        inViewport = entries[0].isIntersecting;
      },
      { rootMargin: '1500px 0px' },
    );
    io.observe(slotEl);
    return () => io.disconnect();
  });

  /** In compact mode the page is tiny (each file collapses to a header
   *  plus a few comments), so virtualization buys nothing — always
   *  render. Otherwise mount only when the slot is near the viewport. */
  const shouldRender = $derived(compact || inViewport || forceRender);

  /** Cache the actual rendered height so the placeholder reproduces it
   *  exactly when the file scrolls away — otherwise total document
   *  height would shift and the user's scroll position would jump. */
  $effect(() => {
    if (!wrapEl) return;
    const ro = new ResizeObserver((entries) => {
      for (const e of entries) {
        if (e.contentRect.height > 0) {
          lastKnownHeight = e.contentRect.height;
        }
      }
    });
    ro.observe(wrapEl);
    return () => ro.disconnect();
  });

  /** Rough first-pass guess at how tall a file's diff will be before
   *  we've ever rendered it. Once the file scrolls into view, the
   *  ResizeObserver replaces this with the real value — but the
   *  estimate has to be close enough that a cross-file scroll lands
   *  near the right place before the in-flight per-file fetches
   *  finish. The old `80px` fallback for files without resolved
   *  hunks was off by ~1-2 orders of magnitude for typical files,
   *  which caused the initial-load comment-nav to overshoot wildly
   *  while slots above the target settled. */
  function estimateHeight(f: FileChange): number {
    if (f.binary) return 80;
    if (!f.hunks) {
      // No hunks yet — open_review ships file metadata only. Lean
      // on the (added + removed) line counts, which it does ship,
      // so the estimate scales with the file rather than collapsing
      // to a constant.
      const lines = f.added + f.removed;
      return Math.max(120, lines * 20 + 80);
    }
    const lineCount = f.hunks.reduce((sum, h) => sum + h.lines.length, 0);
    return lineCount * 20 + f.hunks.length * 30 + 60;
  }

  const placeholderHeight = $derived(
    lastKnownHeight ?? estimateHeight(file),
  );
</script>

<div bind:this={slotEl} class="file-slot" data-file-path={file.path}>
  {#if shouldRender}
    <div bind:this={wrapEl}>
      <FileDiff
        {repo}
        file={effectiveFile}
        {patchset}
        {comments}
        {responses}
        {currentPatchset}
        {composing}
        {saving}
        {compact}
        loadingHunks={loadingHunks && resolved === null}
        {onstartcompose}
        {oncancelcompose}
        {onsubmit}
        {onreply}
        {onstatus}
        {ondelete}
        {onedit}
        {onselectpatchset}
      />
      {#if loadError}
        <p class="muted error">Could not load diff: {loadError}</p>
      {/if}
    </div>
  {:else}
    <div
      class="file-placeholder"
      style:height="{placeholderHeight}px"
      aria-hidden="true"
    ></div>
  {/if}
</div>

<style>
  .file-placeholder {
    /* Matches `.file-diff`'s vertical rhythm so total document height
     * stays continuous as files swap between rendered and placeholder. */
    margin: 16px 0;
  }
</style>
