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

  import { getContext } from 'svelte';
  import { api } from '../lib/api';
  import type {
    AnnotationView,
    CommentView,
    ComposerTarget,
    DraftResponseInput,
    FileChange,
    Patchset,
    ResolutionAction,
    ResponseView,
  } from '../lib/types';
  import type { FoldStore } from '../lib/foldStore';
  import type { AnnotationComposerTarget } from './AnnotationComposer.svelte';
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
    /** Tip commit of the compare-with patchset, or `null` outside
     *  compare mode. Forwarded to `FileDiff` so the highlighting
     *  layer reads the right "base" file (the compared patchset's
     *  tip, not `patchset.base_commit`). */
    compareBaseCommit: string | null;
    /** Patchset-compare v2: when set, this slot renders the per-commit
     *  interdiff for the given commit pair. Overrides the patchset-
     *  scoped fetch path (`/file-diff`) with the generic commit-pair
     *  endpoint (`/diff?from=&to=&path=`).
     *
     *  `useRebase` flips the backend semantic: true for `changed`
     *  pair rows (rebase-based interdiff, libjj path) so
     *  downstream-of-rewrite commits don't inherit cumulative
     *  deltas; false for added/removed (plain parent..commit). */
    interdiffEndpoints?:
      | { from: string; to: string; useRebase: boolean }
      | null;
    /** Fetch the per-file diff up front, regardless of whether the
     *  slot is in the viewport. Set for files that have at least one
     *  comment — the comment-nav `< >` buttons need to land on those
     *  files reliably, and waiting for an in-flight fetch during the
     *  click made the page shift around as the slot mounted and
     *  upstream slots settled. The render still happens lazily; only
     *  the network round-trip is eager. */
    eagerFetch: boolean;
    comments: CommentView[];
    /** Annotations scoped to this slot's file. Passed straight through
     *  to `FileDiff` which re-filters down to per-line. */
    annotations?: AnnotationView[];
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
    /** Render the diff hunks. When `false` the file collapses to a
     *  flat comments-only listing (the old "compact" mode). */
    showDiffs: boolean;
    /** Render comment threads inline, the file-level thread, orphan
     *  threads, and the +comment buttons. When `false` the diff is
     *  rendered without any comment UI. */
    showComments: boolean;
    /** Gate for new-comment affordances (the `+ comment` buttons in
     *  the gutter / file header). Independent of `showComments`:
     *  existing threads still render when `false`, only the controls
     *  to add new comments are suppressed. Set false in per-commit
     *  compare view when the `to` patchset isn't the current one
     *  (Decision 2 of the comments-in-compare design — a new comment
     *  there would immediately read as stale in any non-compare
     *  view). Defaults to true to preserve old call-sites. */
    commentsWriteable?: boolean;
    /** Fraction of width the base (left) side takes in the
     *  side-by-side view. Shared across the page so dragging any
     *  divider rebalances every SBS hunk. 0.5 = even split. */
    sbsSplit: number;
    /** Setter the SBS divider calls during a drag (already clamped
     *  + snap-aware in `ReviewViewer`). */
    setSbsSplit: (next: number) => void;
    /** Shared cache of resolved file diffs keyed by
     *  `${patchset}|${compare}|${path}`. Lifted up to ReviewViewer
     *  so cached entries survive this slot virtualizing itself out
     *  of the DOM — without it, scrolling away from a file and
     *  back refetched the same hunks. */
    diffCache: Map<string, FileChange>;
    onstartcompose: (target: ComposerTarget) => void;
    oncancelcompose: () => void;
    onsubmit: (input: import('../lib/types').DraftCommentInput) => Promise<void>;
    onreply: (input: DraftResponseInput) => Promise<void>;
    onstatus: (commentId: string, action: ResolutionAction) => Promise<void>;
    ondelete: (comment: CommentView) => Promise<void>;
    onedit: (comment: CommentView) => void;
    onselectpatchset: (n: number, commentId?: string) => void;
    /** Currently-open annotation composer, if any. Non-null only when
     *  the user is annotating in *this* file. */
    composingAnnotation?: AnnotationComposerTarget | null;
    /** True when the current viewer is allowed to author annotations
     *  (i.e., they're the review creator). Gates the "Note" gutter
     *  button and the edit/delete affordances on existing bubbles. */
    canAnnotate?: boolean;
    onstartannotate?: (target: AnnotationComposerTarget) => void;
    oncancelannotate?: () => void;
    onsubmitannotation?: (
      input: import('../lib/types').AnnotationInput,
    ) => Promise<void>;
    ondeleteannotation?: (annotation: AnnotationView) => Promise<void>;
    oneditannotation?: (annotation: AnnotationView) => void;
    /** Timestamp of the viewer's previous open of the review. Threaded
     *  down to `FileDiff` → `CommentThread` so threads with responses
     *  newer than this get the "new replies" badge and stay expanded. */
    lastVisitAt?: string | null;
    /** Currently signed-in author identity. */
    viewer?: string;
  }
  const {
    repo,
    reviewNumber,
    file,
    patchset,
    compareWith,
    compareBaseCommit,
    interdiffEndpoints = null,
    eagerFetch,
    comments,
    annotations = [],
    responses,
    currentPatchset,
    composing,
    saving,
    forceRender,
    showDiffs,
    showComments,
    commentsWriteable = true,
    sbsSplit,
    setSbsSplit,
    diffCache,
    onstartcompose,
    oncancelcompose,
    onsubmit,
    onreply,
    onstatus,
    ondelete,
    onedit,
    onselectpatchset,
    composingAnnotation = null,
    canAnnotate = false,
    onstartannotate = () => {},
    oncancelannotate = () => {},
    onsubmitannotation = async () => {},
    ondeleteannotation = async () => {},
    oneditannotation = () => {},
    lastVisitAt = null,
    viewer = '',
  }: Props = $props();

  let slotEl: HTMLElement | undefined = $state();
  let wrapEl: HTMLElement | undefined = $state();
  let inViewport = $state(false);
  let lastKnownHeight = $state<number | null>(null);

  /** Whole-file toggle state, kept here rather than inside `FileDiff` so
   *  it survives this slot virtualizing the inner component out of the
   *  DOM. Without this, scrolling away from an unfolded file and back
   *  would silently re-fold it. */
  let wholeFile = $state(false);

  /** Whole-file collapse (▸/▾) state — same lift-to-slot rationale as
   *  `wholeFile`, plus an extra reason: persist across page reloads
   *  via the fold store so a file the user collapsed mid-review stays
   *  collapsed when they come back tomorrow. */
  const foldStore = getContext<FoldStore | undefined>('kata-fold-store');
  // `file.path` is stable for the slot's lifetime — the parent
  // `{#each}` keys slots by path, so seeing a new path means a new
  // slot instance — so capturing the initial value here is exactly
  // what we want.
  // svelte-ignore state_referenced_locally
  let collapsed = $state(foldStore?.get('file', file.path) ?? false);
  $effect(() => {
    foldStore?.set('file', file.path, collapsed);
  });

  /** Cache key for this slot's (patchset, compare, path) — or for the
   *  v2 per-commit interdiff, `(from_commit, to_commit, path)`. The
   *  two flows write into the same shared cache; the key namespaces
   *  them so a switch from one mode to the other doesn't reuse the
   *  wrong entry. */
  const cacheKey = $derived(
    interdiffEndpoints
      ? `interdiff|${interdiffEndpoints.useRebase ? 'r' : 'p'}|${interdiffEndpoints.from}|${interdiffEndpoints.to}|${file.path}`
      : `${patchset.n}|${compareWith ?? ''}|${file.path}`,
  );

  let loadingHunks = $state(false);
  let loadError = $state<string | null>(null);

  /** Fires when the file is close enough to be visible OR when the
   *  slot is marked `eagerFetch` (because the file carries one or
   *  more comments and the comment-nav needs the diff in cache to
   *  land reliably). Skip if the initial payload already had hunks
   *  (binary files, or a smaller endpoint that ships them eagerly)
   *  or if a previous fetch already populated the shared cache for
   *  this (patchset, compare, path).
   *
   *  `!= null` rather than `!== undefined` because the metadata
   *  endpoint serialises `hunks: None` as JSON `null` (not an absent
   *  field). Without that, every file would short-circuit here and
   *  the diff would stay stuck on "Diff omitted". */
  $effect(() => {
    if (!shouldRender && !eagerFetch) return;
    if (loadingHunks) return;
    if (file.hunks != null || file.binary) return;
    if (diffCache.has(cacheKey)) return;
    const key = cacheKey;
    loadingHunks = true;
    loadError = null;
    const fetcher = interdiffEndpoints
      ? api
          .diffCommits(
            repo,
            interdiffEndpoints.from,
            interdiffEndpoints.to,
            file.path,
            interdiffEndpoints.useRebase,
          )
          .then((res) => {
            // The generic /diff endpoint returns a discriminated union;
            // for a single-file fetch we want the `file` arm.
            if (res.kind !== 'file') {
              throw new Error('expected file-shape result from /diff');
            }
            // Strip the discriminator so the cached value matches the
            // FileChange shape the rest of the UI expects.
            const { kind: _kind, ...rest } = res;
            void _kind;
            return rest as FileChange;
          })
      : api.fileDiff(
          repo,
          reviewNumber,
          file.path,
          patchset.n,
          compareWith ?? undefined,
        );
    fetcher
      .then((updated) => {
        diffCache.set(key, updated);
      })
      .catch((e: Error) => {
        loadError = e.message;
      })
      .finally(() => {
        loadingHunks = false;
      });
  });

  /** What we actually hand to `FileDiff`: the cached resolved one if
   *  a previous fetch (in this slot or another mount of it) put one
   *  there, otherwise the metadata-only original. */
  const effectiveFile = $derived(diffCache.get(cacheKey) ?? file);

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

  /** In comments-only mode the page is tiny (each file collapses to a
   *  header plus a few comments), so virtualization buys nothing —
   *  always render. Otherwise mount only when the slot is near the
   *  viewport. */
  const shouldRender = $derived(!showDiffs || inViewport || forceRender);

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
        {compareBaseCommit}
        {comments}
        {annotations}
        {composingAnnotation}
        {canAnnotate}
        {onstartannotate}
        {oncancelannotate}
        {onsubmitannotation}
        {ondeleteannotation}
        {oneditannotation}
        {responses}
        {currentPatchset}
        {composing}
        {saving}
        {showDiffs}
        {showComments}
        {commentsWriteable}
        {sbsSplit}
        {setSbsSplit}
        loadingHunks={loadingHunks && !diffCache.has(cacheKey)}
        bind:wholeFile
        bind:collapsed
        {lastVisitAt}
        {viewer}
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
