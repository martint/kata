<script lang="ts">
  import type {
    CommentView,
    ComposerTarget,
    DraftResponseInput,
    Hunk,
    HunkLine,
    ResolutionAction,
    ResponseView,
    Side,
  } from '../lib/types';
  import Bubble from './Bubble.svelte';
  import CommentThread from './CommentThread.svelte';
  import { computeHunkWordDiff, wrapRanges } from '../lib/wordDiff';
  import { alignBlock, alignedRows } from '../lib/hunkAlign';

  interface Props {
    hunk: Hunk;
    filePath: string;
    comments: CommentView[];
    responses: ResponseView[];
    currentPatchset: number;
    composing: ComposerTarget | null;
    saving: boolean;
    highlights: { base: Map<number, string>; tip: Map<number, string> };
    onstartcompose: (target: ComposerTarget) => void;
    onreply: (input: DraftResponseInput) => Promise<void>;
    onstatus: (commentId: string, action: ResolutionAction) => Promise<void>;
    ondelete: (comment: CommentView) => Promise<void>;
    onedit: (comment: CommentView) => void;
    onselectpatchset: (n: number, commentId?: string) => void;
    /** Timestamp of the viewer's previous open. Threaded to
     *  CommentThread to drive the "new replies" badge. */
    lastVisitAt?: string | null;
    /** Currently signed-in author identity. */
    viewer?: string;
    /** Render the per-row inline comment threads and the gutter
     *  +comment buttons. When `false` the diff renders without any
     *  comment UI (diffs-only mode). */
    showComments?: boolean;
    /** Gate for the gutter `+ comment` buttons only — inline threads
     *  for existing comments still render when false. Defaults to
     *  true to preserve call-sites. See FileSlot's prop doc for the
     *  per-commit-compare design reason. */
    commentsWriteable?: boolean;
    /** Base-side width fraction (0..1). The tip side takes the rest.
     *  Default 0.5 (even split). Shared with every other SBS hunk on
     *  the page so dragging this hunk's divider rebalances them all. */
    sbsSplit?: number;
    /** Drag callback for the divider. The parent clamps + snaps. */
    setSbsSplit?: (next: number) => void;
  }
  const {
    hunk,
    filePath,
    comments,
    responses,
    currentPatchset,
    composing,
    saving,
    highlights,
    onstartcompose,
    onreply,
    onstatus,
    ondelete,
    onedit,
    onselectpatchset,
    lastVisitAt = null,
    viewer = '',
    showComments = true,
    commentsWriteable = true,
    sbsSplit = 0.5,
    setSbsSplit = () => {},
  }: Props = $props();

  type PairedRow =
    | { kind: 'context'; left: HunkLine; right: HunkLine }
    | { kind: 'change'; left: HunkLine | null; right: HunkLine | null };

  function pair(lines: HunkLine[]): PairedRow[] {
    const rows: PairedRow[] = [];
    let i = 0;
    while (i < lines.length) {
      if (lines[i].origin === 'context') {
        rows.push({ kind: 'context', left: lines[i], right: lines[i] });
        i++;
        continue;
      }
      const removes: HunkLine[] = [];
      while (i < lines.length && lines[i].origin === 'removed') {
        removes.push(lines[i]);
        i++;
      }
      const adds: HunkLine[] = [];
      while (i < lines.length && lines[i].origin === 'added') {
        adds.push(lines[i]);
        i++;
      }
      // Best-content alignment instead of strict index-pairing.
      // `alignedRows` slots unpaired removes / adds onto their own
      // row with the other side blank, so each paired remove/add
      // sits on the SAME row vertically — exactly the visual cue the
      // reader needs to see "this and that correspond." Strictly
      // index-paired N:N blocks degenerate to identical output via
      // the DP, so the common case is unchanged.
      const aligned = alignedRows(
        alignBlock(
          removes.map((l) => l.content.replace(/\n$/, '')),
          adds.map((l) => l.content.replace(/\n$/, '')),
        ),
      );
      for (const row of aligned) {
        rows.push({
          kind: 'change',
          left: row.removeIndex != null ? removes[row.removeIndex] : null,
          right: row.addIndex != null ? adds[row.addIndex] : null,
        });
      }
    }
    return rows;
  }

  const rows = $derived(pair(hunk.lines));
  /** When an existing draft is being edited, hide it from the thread so
   *  the composer below takes its visual slot instead of stacking under
   *  the original draft bubble. */
  const editingCommentId = $derived(composing?.editing?.commentId ?? null);
  let dragging: { side: Side; start: number; end: number } | null = $state(null);
  let baseSideEl: HTMLDivElement | undefined = $state();
  let tipSideEl: HTMLDivElement | undefined = $state();
  let baseTableEl: HTMLTableElement | undefined = $state();
  let tipTableEl: HTMLTableElement | undefined = $state();
  let pairEl: HTMLDivElement | undefined = $state();
  let dividerDragging = $state(false);
  let dragSelected: HTMLElement[] = [];

  /** Drag the gutter between the base and tip sides. Pointer X within
   *  `pairEl` becomes the new base-side fraction; the parent clamps
   *  and snaps. `setPointerCapture` lets the drag continue smoothly
   *  even when the pointer leaves the 1-px-wide divider, and the
   *  bound listeners are scoped to this pointer id so concurrent
   *  drags on other dividers don't interfere. */
  function onDividerDown(e: PointerEvent) {
    if (!pairEl) return;
    e.preventDefault();
    const target = e.currentTarget as HTMLElement;
    target.setPointerCapture(e.pointerId);
    dividerDragging = true;
    const rect = pairEl.getBoundingClientRect();
    const onMove = (ev: PointerEvent) => {
      if (ev.pointerId !== e.pointerId) return;
      setSbsSplit((ev.clientX - rect.left) / rect.width);
    };
    const onUp = (ev: PointerEvent) => {
      if (ev.pointerId !== e.pointerId) return;
      target.removeEventListener('pointermove', onMove);
      target.removeEventListener('pointerup', onUp);
      target.removeEventListener('pointercancel', onUp);
      dividerDragging = false;
    };
    target.addEventListener('pointermove', onMove);
    target.addEventListener('pointerup', onUp);
    target.addEventListener('pointercancel', onUp);
  }

  /** Double-click resets to the standard even split. The snap-to-middle
   *  behaviour during drag already makes the centre easy to land on,
   *  but this is the one-click escape hatch. */
  function onDividerDblClick() {
    setSbsSplit(0.5);
  }

  /** Set --content-vp-width on each side so sticky thread blocks know how
   *  wide to be when content scrolls horizontally beneath them. */
  $effect(() => {
    const ro = new ResizeObserver((entries) => {
      for (const e of entries) {
        (e.target as HTMLElement).style.setProperty(
          '--content-vp-width',
          `${e.contentRect.width}px`,
        );
      }
    });
    if (baseSideEl) ro.observe(baseSideEl);
    if (tipSideEl) ro.observe(tipSideEl);
    return () => ro.disconnect();
  });

  $effect(() => {
    for (const el of dragSelected) el.classList.remove('selected');
    dragSelected = [];
    if (!dragging) return;
    const tableEl = dragging.side === 'base' ? baseTableEl : tipTableEl;
    if (!tableEl) return;
    const min = Math.min(dragging.start, dragging.end);
    const max = Math.max(dragging.start, dragging.end);
    for (let ln = min; ln <= max; ln++) {
      const matches = tableEl.querySelectorAll(
        `[data-side="${dragging.side}"][data-line="${ln}"]`,
      );
      for (const el of matches) {
        (el as HTMLElement).classList.add('selected');
        dragSelected.push(el as HTMLElement);
      }
    }
  });

  function threadsAt(side: Side, line: number | null | undefined): CommentView[] {
    if (line == null) return [];
    return comments.filter((c) => {
      if (c.side !== side) return false;
      const effective =
        c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
          ? c.anchor.new_lines
          : c.lines;
      return effective != null && effective.end === line;
    });
  }

  /** See `HunkLines.svelte` — same idea, but indexed on the side
   *  this column renders so a multi-line range tints every covered
   *  row, not just the one the thread attaches to. Outdated anchors
   *  are skipped (their range points at content that has since
   *  changed); those threads render at the file level. */
  const commentedLines = $derived.by(() => {
    const set = new Set<string>();
    for (const c of comments) {
      if (!c.side) continue;
      if (c.anchor.kind === 'outdated') continue;
      const effective =
        c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
          ? c.anchor.new_lines
          : c.lines;
      if (!effective) continue;
      for (let l = effective.start; l <= effective.end; l++) {
        set.add(`${c.side}:${l}`);
      }
    }
    return set;
  });

  function isCommented(side: Side, line: number | null | undefined): boolean {
    if (!showComments) return false;
    return line != null && commentedLines.has(`${side}:${line}`);
  }

  function onPointerDown(e: PointerEvent, side: Side, line: number) {
    if (e.button !== 0) return;
    e.preventDefault();
    if (
      e.shiftKey &&
      composing?.kind === 'line' &&
      composing.file === filePath &&
      composing.side === side
    ) {
      onstartcompose({
        kind: 'line',
        file: filePath,
        side,
        startLine: Math.min(composing.startLine, line),
        endLine: Math.max(composing.endLine, line),
      });
      return;
    }
    dragging = { side, start: line, end: line };

    const onMove = (ev: PointerEvent) => {
      if (!dragging) return;
      const el = document.elementFromPoint(ev.clientX, ev.clientY);
      const cell = (el as HTMLElement | null)?.closest(
        '[data-line]',
      ) as HTMLElement | null;
      if (cell && cell.getAttribute('data-side') === side) {
        const ln = Number(cell.getAttribute('data-line'));
        if (!isNaN(ln)) {
          dragging = { ...dragging, end: ln };
        }
      }
    };
    const onUp = () => {
      document.removeEventListener('pointermove', onMove);
      document.removeEventListener('pointerup', onUp);
      if (dragging) {
        const start = Math.min(dragging.start, dragging.end);
        const end = Math.max(dragging.start, dragging.end);
        const s = dragging.side;
        dragging = null;
        onstartcompose({
          kind: 'line',
          file: filePath,
          side: s,
          startLine: start,
          endLine: end,
        });
      }
    };
    document.addEventListener('pointermove', onMove);
    document.addEventListener('pointerup', onUp);
  }

  function lineText(line: HunkLine | null): string {
    if (!line) return '';
    return line.content.replace(/\n$/, '');
  }

  function highlightedLeft(line: HunkLine | null): string | undefined {
    if (!line) return undefined;
    // Left column is the base view; a context line uses base_line (same
    // content as the right side's tip_line either way).
    if (line.base_line != null) return highlights.base.get(line.base_line);
    if (line.tip_line != null) return highlights.tip.get(line.tip_line);
    return undefined;
  }

  function highlightedRight(line: HunkLine | null): string | undefined {
    if (!line) return undefined;
    if (line.tip_line != null) return highlights.tip.get(line.tip_line);
    if (line.base_line != null) return highlights.base.get(line.base_line);
    return undefined;
  }

  /** Word-level diff per hunk-line index, computed against the original
   *  flat `hunk.lines`. Looked up by the rows below using each side's
   *  source HunkLine. */
  const wordDiff = $derived(computeHunkWordDiff(hunk.lines));

  function hunkLineIndex(line: HunkLine | null): number | null {
    if (!line) return null;
    const idx = hunk.lines.indexOf(line);
    return idx < 0 ? null : idx;
  }

  function withWordDiff(html: string | undefined, line: HunkLine | null): string | undefined {
    if (!html || !line) return html;
    const idx = hunkLineIndex(line);
    if (idx == null) return html;
    const wd = wordDiff.get(idx);
    if (!wd) return html;
    return wrapRanges(html, wd.ranges, wd.kind);
  }
</script>

<div class="sbs-pair" bind:this={pairEl}>
  <div
    class="sbs-side base"
    bind:this={baseSideEl}
    style:flex-basis="calc({sbsSplit * 100}% - 0.5px)"
  >
    <table class="hunk-half" bind:this={baseTableEl}>
      <colgroup>
        <col class="col-ln" />
        <col class="col-content" />
      </colgroup>
      <tbody>
        {#each rows as row, i (i)}
          {@const leftLine = row.left?.base_line ?? null}
          <tr class="sbs-row {row.kind}">
            <!-- data-side/data-line are also on the gutter cell so the
                 drag-selection logic finds it via `elementFromPoint` even
                 when the user's cursor stays in the sticky gutter column. -->
            <td
              class="ln {row.left ? row.left.origin : 'empty'}"
              data-side="base"
              data-line={leftLine ?? ''}
            >
              <!-- "+" button lives in the sticky gutter cell, not the
                   content cell, so it stays visible during horizontal
                   scroll of long lines. -->
              {#if row.left?.base_line != null && showComments && commentsWriteable}
                <button
                  type="button"
                  class="add-comment"
                  title="Click to comment; click-drag to extend"
                  onpointerdown={(e) => onPointerDown(e, 'base', row.left!.base_line!)}
                >
                  <Bubble size={12} />
                </button>
              {/if}
              {row.left?.base_line ?? row.left?.tip_line ?? ''}
            </td>
            <td
              class={`content ${row.left ? row.left.origin : 'empty'}${isCommented('base', leftLine) ? ' commented' : ''}`}
              data-side="base"
              data-line={leftLine ?? ''}
            >
              {#if row.left}
                {@const html = withWordDiff(highlightedLeft(row.left), row.left)}
                <pre>{#if html}{@html html}{:else}{lineText(row.left) || ' '}{/if}</pre>
              {:else}
                <!-- Empty <td>s collapse to zero height; a <pre> with a
                     single space gives the row the same line-box height
                     as its populated siblings, keeping this table in
                     lockstep with the right-side one. -->
                <pre> </pre>
              {/if}
            </td>
          </tr>
          {@const leftThreads = showComments ? threadsAt('base', row.left?.base_line) : []}
          {#if leftThreads.length > 0}
            <tr class="sbs-threads from-{row.left?.origin ?? 'context'}">
              <td colspan="2" class="thread-cell">
                <!-- Indent past the side's line-number gutter via
                     padding rather than an empty cell — see
                     HunkLines.svelte for the rationale. -->
                <div class="thread-sticky" style="--gutter-offset: 65px">
                  <CommentThread
                    comments={leftThreads}
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
                </div>
              </td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
  </div>
  <div
    class="sbs-divider"
    class:dragging={dividerDragging}
    role="separator"
    aria-orientation="vertical"
    aria-label="Drag to resize the side-by-side split (double-click to reset)"
    aria-valuenow={Math.round(sbsSplit * 100)}
    aria-valuemin={15}
    aria-valuemax={85}
    onpointerdown={onDividerDown}
    ondblclick={onDividerDblClick}
  >
    <div class="sbs-divider-handle" aria-hidden="true"></div>
  </div>
  <div
    class="sbs-side tip"
    bind:this={tipSideEl}
    style:flex-basis="calc({(1 - sbsSplit) * 100}% - 0.5px)"
  >
    <table class="hunk-half" bind:this={tipTableEl}>
      <colgroup>
        <col class="col-ln" />
        <col class="col-content" />
      </colgroup>
      <tbody>
        {#each rows as row, i (i)}
          {@const rightLine = row.right?.tip_line ?? null}
          <tr class="sbs-row {row.kind}">
            <td
              class="ln {row.right ? row.right.origin : 'empty'}"
              data-side="tip"
              data-line={rightLine ?? ''}
            >
              {#if row.right?.tip_line != null && showComments && commentsWriteable}
                <button
                  type="button"
                  class="add-comment"
                  title="Click to comment; click-drag to extend"
                  onpointerdown={(e) => onPointerDown(e, 'tip', row.right!.tip_line!)}
                >
                  <Bubble size={12} />
                </button>
              {/if}
              {row.right?.tip_line ?? row.right?.base_line ?? ''}
            </td>
            <td
              class={`content ${row.right ? row.right.origin : 'empty'}${isCommented('tip', rightLine) ? ' commented' : ''}`}
              data-side="tip"
              data-line={rightLine ?? ''}
            >
              {#if row.right}
                {@const html = withWordDiff(highlightedRight(row.right), row.right)}
                <pre>{#if html}{@html html}{:else}{lineText(row.right) || ' '}{/if}</pre>
              {:else}
                <pre> </pre>
              {/if}
            </td>
          </tr>
          {@const rightThreads = showComments ? threadsAt('tip', row.right?.tip_line) : []}
          {#if rightThreads.length > 0}
            <tr class="sbs-threads from-{row.right?.origin ?? 'context'}">
              <td colspan="2" class="thread-cell">
                <!-- Indent past the side's line-number gutter via
                     padding rather than an empty cell — see
                     HunkLines.svelte for the rationale. -->
                <div class="thread-sticky" style="--gutter-offset: 65px">
                  <CommentThread
                    comments={rightThreads}
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
                </div>
              </td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
  </div>
</div>

<style>
  .sbs-pair {
    display: flex;
    align-items: flex-start;
    gap: 0;
  }

  .sbs-side {
    /* flex-basis is set inline from the shared `sbsSplit` ratio
     * (parent decides the split, every SBS hunk on the page agrees).
     * grow:0 / shrink:1 lets the side respect its basis while still
     * collapsing under a narrow viewport. */
    flex: 0 1 auto;
    min-width: 0;
    overflow-x: auto;
    overscroll-behavior-x: contain;
  }

  /* Visual divider between the two sides. The 1-px line is the
   * `.sbs-divider` itself; the wider `.sbs-divider-handle` sits on
   * top via absolute positioning, giving the user a generous hit
   * area without adding any visible bulk. */
  .sbs-divider {
    flex: 0 0 1px;
    background: var(--border);
    align-self: stretch;
    position: relative;
    user-select: none;
    touch-action: none;
  }

  .sbs-divider.dragging,
  .sbs-divider:hover {
    background: var(--link);
  }

  .sbs-divider-handle {
    position: absolute;
    top: 0;
    bottom: 0;
    left: -3px;
    right: -3px;
    cursor: col-resize;
  }

  .hunk-half {
    width: max-content;
    min-width: 100%;
    /* `separate` (rather than collapse) is needed for sticky cells in
     * Firefox to keep their backgrounds — see HunkLines.svelte. */
    border-collapse: separate;
    border-spacing: 0;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12.5px;
    /* Match HunkLines so unified vs side-by-side feel identical. */
    line-height: 1.6;
  }

  /* See HunkLines.svelte for the rationale — keep cells top-aligned so
   * the inline-composer's padding-bottom on `.ln` doesn't drop the
   * code line into the composer overlay. */
  .hunk-half td {
    vertical-align: top;
  }

  .col-ln {
    width: 48px;
  }

  .ln {
    text-align: right;
    padding: 0 8px;
    color: var(--text-faint);
    user-select: none;
    border-right: 1px solid var(--border-muted);
    font-size: 11px;
    background: var(--bg);
    /* Pin the line-number gutter (and the "+" button it now contains) so
     * they stay visible while long lines scroll horizontally. */
    position: sticky;
    left: 0;
    z-index: 1;
    text-align: right;
  }

  .content {
    /* Symmetric padding — the "+" button moved into the sticky `.ln` cell,
     * so the content cell no longer reserves a left margin for it. */
    padding: 0 8px;
  }

  .content pre {
    margin: 0;
    font: inherit;
    white-space: pre;
  }

  .ln.added,
  .content.added {
    background: var(--add-bg);
  }
  .ln.added {
    background: var(--add-bg-strong);
  }

  .ln.removed,
  .content.removed {
    background: var(--remove-bg);
  }
  .ln.removed {
    background: var(--remove-bg-strong);
  }

  /* Word-level diff overlay: the column-tinted cells say a line
   * changed; these stronger backgrounds say which specific characters
   * differ. `:global` because we inject the spans into shiki's
   * pre-rendered HTML via `wrapRanges`. */
  :global(.content.removed .wd-removed) {
    background: var(--remove-word-bg);
    border-radius: 2px;
  }

  :global(.content.added .wd-added) {
    background: var(--add-word-bg);
    border-radius: 2px;
  }

  .ln.empty,
  .content.empty {
    background: var(--bg-panel);
  }

  .content.selected {
    box-shadow: inset 4px 0 0 var(--selection-rule);
    background-image: linear-gradient(var(--selection-tint), var(--selection-tint));
  }

  /* Content cell of a row covered by a posted comment's anchor range —
   * tints the row so multi-line ranges visibly span their lines instead
   * of looking attached to just the last one. Stripe matches the
   * `.thread-sticky` accent so the eye links the two together. */
  .content.commented {
    box-shadow: inset 3px 0 0 var(--link);
    background-image: linear-gradient(var(--selection-tint), var(--selection-tint));
  }

  /* Centered on the gutter/diff boundary so it never overlaps the
   * line number to its left. See HunkLines.svelte for the
   * unified-mode variant. */
  .add-comment {
    position: absolute;
    right: -9px;
    top: 50%;
    transform: translateY(-50%);
    width: 18px;
    height: 18px;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-elevated);
    color: var(--link);
    font-weight: 600;
    font-size: 12px;
    line-height: 16px;
    cursor: pointer;
    visibility: hidden;
    user-select: none;
    touch-action: none;
  }

  .sbs-row:hover .add-comment {
    visibility: visible;
  }

  .add-comment:hover {
    background: var(--link);
    color: var(--on-accent);
    border-color: var(--link);
  }

  /* See HunkLines.svelte — match the adjacent diff row's tint so the
   * gutter and right-gap don't read as a dark stripe through the
   * column's color. */
  .sbs-threads {
    background: transparent;
  }

  .sbs-threads.from-added {
    background: var(--add-bg);
  }

  .sbs-threads.from-removed {
    background: var(--remove-bg);
  }

  .thread-cell {
    padding: 0;
    background: transparent;
  }

  /* Blue tint + left stripe so inline threads visually separate from
   * surrounding diff rows. See HunkLines.svelte for rationale. */
  .thread-sticky {
    /* See HunkLines.svelte — same measured / fallback / right-trim
     * pattern. `--measured-gutter` is published by FileDiff and
     * cascades down; the inline `--gutter-offset` is the hardcoded
     * fallback (one line-number column's width plus a small gap). */
    --gutter: var(--measured-gutter, var(--gutter-offset));
    position: sticky;
    left: var(--gutter);
    margin-left: var(--gutter);
    width: calc(var(--content-vp-width, 100%) - var(--gutter) - 12px);
    background: var(--link-bg);
    padding: 8px 12px;
    border-top: 1px solid var(--border-muted);
    border-bottom: 1px solid var(--border-muted);
    border-left: 3px solid var(--link);
    box-sizing: border-box;
  }
</style>
