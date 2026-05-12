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
    file: FileChange;
    patchset: Patchset;
    comments: CommentView[];
    responses: ResponseView[];
    composing: ComposerTarget | null;
    saving: boolean;
    /** Keep this file mounted regardless of viewport — used so the file
     *  currently hosting an open composer doesn't get virtualized away
     *  out from under the user. */
    forceRender: boolean;
    onstartcompose: (target: ComposerTarget) => void;
    oncancelcompose: () => void;
    onsubmit: (input: import('../lib/types').DraftCommentInput) => Promise<void>;
    onreply: (input: DraftResponseInput) => Promise<void>;
    onstatus: (commentId: string, action: ResolutionAction) => Promise<void>;
    ondelete: (comment: CommentView) => Promise<void>;
  }
  const {
    repo,
    file,
    patchset,
    comments,
    responses,
    composing,
    saving,
    forceRender,
    onstartcompose,
    oncancelcompose,
    onsubmit,
    onreply,
    onstatus,
    ondelete,
  }: Props = $props();

  let slotEl: HTMLElement | undefined = $state();
  let wrapEl: HTMLElement | undefined = $state();
  let inViewport = $state(false);
  let lastKnownHeight = $state<number | null>(null);

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

  const shouldRender = $derived(inViewport || forceRender);

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
   *  we've ever rendered it. Doesn't need to be accurate — once the
   *  file scrolls into view and renders, the ResizeObserver replaces
   *  this with the real value. */
  function estimateHeight(f: FileChange): number {
    if (f.binary || !f.hunks) return 80;
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
        {file}
        {patchset}
        {comments}
        {responses}
        {composing}
        {saving}
        {onstartcompose}
        {oncancelcompose}
        {onsubmit}
        {onreply}
        {onstatus}
        {ondelete}
      />
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
