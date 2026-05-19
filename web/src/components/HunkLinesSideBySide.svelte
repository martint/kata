<script lang="ts">
  import { getContext } from 'svelte';
  import type {
    AnnotationInput,
    AnnotationView,
    CommentView,
    ComposerTarget,
    DraftResponseInput,
    Hunk,
    HunkLine,
    ResolutionAction,
    ResponseView,
    Side,
  } from '../lib/types';
  import type { FoldStore } from '../lib/foldStore';
  import AnnotationBubble from './AnnotationBubble.svelte';
  import AnnotationComposer, {
    type AnnotationComposerTarget,
  } from './AnnotationComposer.svelte';
  import Bubble from './Bubble.svelte';
  import Chevron from './Chevron.svelte';
  import CommentThread from './CommentThread.svelte';
  import { computeHunkWordDiff, wrapRanges } from '../lib/wordDiff';
  import { alignBlock, alignedRows } from '../lib/hunkAlign';
  import {
    intraLineSelectionFor,
    type IntraLineSelection,
  } from '../lib/intraLineSelection';

  interface Props {
    hunk: Hunk;
    filePath: string;
    comments: CommentView[];
    /** Author-attached context notes scoped to this file. Per-line
     *  filtering matches `comments`. */
    annotations?: AnnotationView[];
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
    composingAnnotation?: AnnotationComposerTarget | null;
    annotationAnchorIds?: { change: string; commit: string };
    canAnnotate?: boolean;
    onstartannotate?: (target: AnnotationComposerTarget) => void;
    oncancelannotate?: () => void;
    onsubmitannotation?: (input: AnnotationInput) => Promise<void>;
    ondeleteannotation?: (annotation: AnnotationView) => Promise<void>;
    oneditannotation?: (annotation: AnnotationView) => void;
    defaultThreadsCollapsed?: boolean;
  }
  const {
    hunk,
    filePath,
    comments,
    annotations = [],
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
    composingAnnotation = null,
    annotationAnchorIds = { change: '', commit: '' },
    canAnnotate = false,
    onstartannotate = () => {},
    oncancelannotate = () => {},
    onsubmitannotation = async () => {},
    ondeleteannotation = async () => {},
    oneditannotation = () => {},
    defaultThreadsCollapsed = false,
  }: Props = $props();

  /** Per-anchor thread fold — same idea as HunkLines, scoped to this
   *  component's filePath. Toggling bumps `foldVersion` so the
   *  $derived row computations re-run. */
  const foldStore = getContext<FoldStore | undefined>('kata-fold-store');
  let foldVersion = $state(0);
  function threadKey(side: Side, line: number): string {
    return `${filePath}:${side}:${line}`;
  }
  function isThreadCollapsed(side: Side, line: number): boolean {
    void foldVersion;
    const stored = foldStore?.get('thread', threadKey(side, line));
    return stored ?? defaultThreadsCollapsed;
  }
  function toggleThreadFold(side: Side, line: number) {
    if (!foldStore) return;
    const k = threadKey(side, line);
    const currently = foldStore.get('thread', k) ?? defaultThreadsCollapsed;
    foldStore.set('thread', k, !currently);
    foldVersion++;
  }

  /** Aggregate anchor range for the folded comments + notes at
   *  (side, line). See HunkLines.svelte for the design rationale. */
  function foldedRangeAt(side: Side, line: number): { start: number; end: number } | null {
    let start = Number.POSITIVE_INFINITY;
    let end = Number.NEGATIVE_INFINITY;
    for (const c of comments) {
      if (c.side !== side) continue;
      const eff =
        c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
          ? c.anchor.new_lines
          : c.lines;
      if (!eff) continue;
      if (eff.end !== line) continue;
      if (eff.start < start) start = eff.start;
      if (eff.end > end) end = eff.end;
    }
    for (const n of annotations) {
      if (n.side !== side) continue;
      const eff =
        n.anchor.kind === 'moved' || n.anchor.kind === 'drifted'
          ? n.anchor.new_lines
          : n.lines;
      if (!eff) continue;
      if (eff.end !== line) continue;
      if (eff.start < start) start = eff.start;
      if (eff.end > end) end = eff.end;
    }
    if (!Number.isFinite(start)) return null;
    return { start, end };
  }

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

  /** Hovered folded-thread marker — paints `.highlight-anchor` onto
   *  matching rows in the relevant side's table. See HunkLines.svelte
   *  for the design rationale; SBS just picks the right table from
   *  `side`. */
  let hoveredAnchor: { side: Side; start: number; end: number } | null = $state.raw(null);
  let hoveredEls: HTMLElement[] = [];
  $effect(() => {
    for (const el of hoveredEls) el.classList.remove('highlight-anchor');
    hoveredEls = [];
    if (!hoveredAnchor) return;
    const { side, start, end } = hoveredAnchor;
    const table = side === 'base' ? baseTableEl : tipTableEl;
    if (!table) return;
    for (let ln = start; ln <= end; ln++) {
      const matches = table.querySelectorAll(
        `[data-side="${side}"][data-line="${ln}"]`,
      );
      for (const el of matches) {
        (el as HTMLElement).classList.add('highlight-anchor');
        hoveredEls.push(el as HTMLElement);
      }
    }
  });

  /** Pill state for drag-to-select intra-line comments. SBS has two
   *  tables — base and tip — and a selection lives in exactly one of
   *  them; we try both on mouseup and use whichever resolves. See
   *  HunkLines.svelte for the per-mode design rationale. */
  let selectionPill: IntraLineSelection | null = $state.raw(null);
  $effect(() => {
    if (!pairEl) return;
    function onMouseUp() {
      requestAnimationFrame(() => {
        const fromBase = baseTableEl ? intraLineSelectionFor(baseTableEl) : null;
        const fromTip = !fromBase && tipTableEl ? intraLineSelectionFor(tipTableEl) : null;
        selectionPill = fromBase ?? fromTip;
      });
    }
    function onMouseDown(e: MouseEvent) {
      const t = e.target as HTMLElement | null;
      if (t?.closest('.intra-line-pill')) return;
      selectionPill = null;
    }
    document.addEventListener('mouseup', onMouseUp);
    document.addEventListener('mousedown', onMouseDown);
    return () => {
      document.removeEventListener('mouseup', onMouseUp);
      document.removeEventListener('mousedown', onMouseDown);
    };
  });

  function commentOnSelection() {
    const s = selectionPill;
    if (!s) return;
    onstartcompose({
      kind: 'line',
      file: filePath,
      side: s.side,
      startLine: s.line,
      endLine: s.line,
      columns: { start: s.startOffset, end: s.endOffset },
    });
    selectionPill = null;
    window.getSelection()?.removeAllRanges();
  }

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

  function annotationsAt(
    side: Side,
    line: number | null | undefined,
  ): AnnotationView[] {
    if (line == null) return [];
    return annotations.filter((n) => {
      if (n.side !== side) return false;
      const effective =
        n.anchor.kind === 'moved' || n.anchor.kind === 'drifted'
          ? n.anchor.new_lines
          : n.lines;
      return effective != null && effective.end === line;
    });
  }

  function isAnnotatingHere(side: Side, line: number | null | undefined): boolean {
    if (line == null) return false;
    return (
      composingAnnotation?.kind === 'line' &&
      composingAnnotation.file === filePath &&
      composingAnnotation.side === side &&
      composingAnnotation.endLine === line
    );
  }

  function startAnnotateHere(side: Side, line: number) {
    if (!canAnnotate) return;
    onstartannotate({
      kind: 'line',
      file: filePath,
      side,
      startLine: line,
      endLine: line,
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
    let out = html;
    const idx = hunkLineIndex(line);
    if (idx != null) {
      const wd = wordDiff.get(idx);
      if (wd) out = wrapRanges(out, wd.ranges, `wd-${wd.kind}`);
    }
    // Layer column-anchor overlays from any intra-line comments on this row.
    const side: Side =
      line.origin === 'removed' ? 'base' : line.tip_line != null ? 'tip' : 'base';
    const lineNum = side === 'base' ? line.base_line : line.tip_line;
    if (lineNum != null) {
      const cols = columnAnchorsFor(side, lineNum);
      if (cols.length > 0) out = wrapRanges(out, cols, 'column-anchor');
    }
    return out;
  }

  /** See HunkLines.svelte's columnAnchorsFor — same rule: only Valid
   *  or Moved single-line anchors contribute a highlight. */
  function columnAnchorsFor(side: Side, line: number): { start: number; end: number }[] {
    const out: { start: number; end: number }[] = [];
    for (const c of comments) {
      if (c.side !== side) continue;
      if (!c.columns) continue;
      const effective =
        c.anchor.kind === 'moved'
          ? c.anchor.new_lines
          : c.anchor.kind === 'valid'
            ? c.lines
            : null;
      if (!effective) continue;
      if (effective.start !== effective.end) continue;
      if (effective.end !== line) continue;
      out.push({ start: c.columns.start, end: c.columns.end });
    }
    return out;
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
                {#if canAnnotate}
                  <button
                    type="button"
                    class="add-note"
                    title="Add author note (only the review creator can do this)"
                    onclick={() => startAnnotateHere('base', row.left!.base_line!)}
                  >
                    N
                  </button>
                {/if}
              {/if}
              {#if row.left?.base_line != null && showComments}
                {@const ln = row.left.base_line}
                {@const count = threadsAt('base', ln).length + annotationsAt('base', ln).length}
                {@const folded = isThreadCollapsed('base', ln)}
                {#if count > 0}
                  <button
                    type="button"
                    class="thread-marker"
                    class:folded
                    aria-pressed={!folded}
                    aria-label="{count} comment{count === 1 ? '' : 's'}; click to {folded ? 'expand' : 'collapse'}"
                    title="{count} comment{count === 1 ? '' : 's'} — click to {folded ? 'expand' : 'collapse'}"
                    onclick={() => toggleThreadFold('base', ln)}
                    onmouseenter={() => {
                      const r = foldedRangeAt('base', ln);
                      hoveredAnchor = r ? { side: 'base', start: r.start, end: r.end } : null;
                    }}
                    onmouseleave={() => (hoveredAnchor = null)}
                  ><Chevron dir={folded ? 'right' : 'down'} size={12} filled /></button>
                {/if}
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
          {@const leftNotes = showComments ? annotationsAt('base', row.left?.base_line) : []}
          {@const leftAnnotating = isAnnotatingHere('base', row.left?.base_line)}
          {@const leftHasContent = leftThreads.length > 0 || leftNotes.length > 0}
          {@const leftCollapsed =
            leftHasContent &&
            row.left?.base_line != null &&
            isThreadCollapsed('base', row.left.base_line)}
          {#if (leftHasContent && !leftCollapsed) || leftAnnotating}
            <tr class="sbs-threads from-{row.left?.origin ?? 'context'}">
              <td colspan="2" class="thread-cell">
                <!-- Indent past the side's line-number gutter via
                     padding rather than an empty cell — see
                     HunkLines.svelte for the rationale. -->
                <div class="thread-sticky" style="--gutter-offset: 65px">
                  {#each leftNotes as n (n.annotation_id)}
                    <AnnotationBubble
                      annotation={n}
                      canEdit={canAnnotate}
                      onedit={oneditannotation}
                      ondelete={ondeleteannotation}
                    />
                  {/each}
                  {#if leftAnnotating && composingAnnotation}
                    <AnnotationComposer
                      target={composingAnnotation}
                      anchorIds={annotationAnchorIds}
                      {saving}
                      oncancel={oncancelannotate}
                      onsubmit={onsubmitannotation}
                    />
                  {/if}
                  {#if leftThreads.length > 0}
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
                  {/if}
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
                {#if canAnnotate}
                  <button
                    type="button"
                    class="add-note"
                    title="Add author note (only the review creator can do this)"
                    onclick={() => startAnnotateHere('tip', row.right!.tip_line!)}
                  >
                    N
                  </button>
                {/if}
              {/if}
              {#if row.right?.tip_line != null && showComments}
                {@const ln = row.right.tip_line}
                {@const count = threadsAt('tip', ln).length + annotationsAt('tip', ln).length}
                {@const folded = isThreadCollapsed('tip', ln)}
                {#if count > 0}
                  <button
                    type="button"
                    class="thread-marker"
                    class:folded
                    aria-pressed={!folded}
                    aria-label="{count} comment{count === 1 ? '' : 's'}; click to {folded ? 'expand' : 'collapse'}"
                    title="{count} comment{count === 1 ? '' : 's'} — click to {folded ? 'expand' : 'collapse'}"
                    onclick={() => toggleThreadFold('tip', ln)}
                    onmouseenter={() => {
                      const r = foldedRangeAt('tip', ln);
                      hoveredAnchor = r ? { side: 'tip', start: r.start, end: r.end } : null;
                    }}
                    onmouseleave={() => (hoveredAnchor = null)}
                  ><Chevron dir={folded ? 'right' : 'down'} size={12} filled /></button>
                {/if}
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
          {@const rightNotes = showComments ? annotationsAt('tip', row.right?.tip_line) : []}
          {@const rightAnnotating = isAnnotatingHere('tip', row.right?.tip_line)}
          {@const rightHasContent = rightThreads.length > 0 || rightNotes.length > 0}
          {@const rightCollapsed =
            rightHasContent &&
            row.right?.tip_line != null &&
            isThreadCollapsed('tip', row.right.tip_line)}
          {#if (rightHasContent && !rightCollapsed) || rightAnnotating}
            <tr class="sbs-threads from-{row.right?.origin ?? 'context'}">
              <td colspan="2" class="thread-cell">
                <!-- Indent past the side's line-number gutter via
                     padding rather than an empty cell — see
                     HunkLines.svelte for the rationale. -->
                <div class="thread-sticky" style="--gutter-offset: 65px">
                  {#each rightNotes as n (n.annotation_id)}
                    <AnnotationBubble
                      annotation={n}
                      canEdit={canAnnotate}
                      onedit={oneditannotation}
                      ondelete={ondeleteannotation}
                    />
                  {/each}
                  {#if rightAnnotating && composingAnnotation}
                    <AnnotationComposer
                      target={composingAnnotation}
                      anchorIds={annotationAnchorIds}
                      {saving}
                      oncancel={oncancelannotate}
                      onsubmit={onsubmitannotation}
                    />
                  {/if}
                  {#if rightThreads.length > 0}
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
                  {/if}
                </div>
              </td>
            </tr>
          {/if}
        {/each}
      </tbody>
    </table>
  </div>
</div>

{#if selectionPill}
  <button
    type="button"
    class="intra-line-pill"
    style:top="{selectionPill.rect.top + window.scrollY - 30}px"
    style:left="{selectionPill.rect.left + window.scrollX}px"
    onclick={commentOnSelection}
    onmousedown={(e) => e.preventDefault()}
  >
    Comment on selection
  </button>
{/if}

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

  /* See HunkLines.svelte for the column-anchor design rationale. */
  :global(.content .column-anchor) {
    box-shadow: inset 0 -2px 0 var(--link);
    cursor: pointer;
  }

  /* "Comment on selection" pill — see HunkLines.svelte. */
  .intra-line-pill {
    position: absolute;
    z-index: 10;
    background: var(--link);
    color: var(--on-accent);
    border: none;
    border-radius: 4px;
    padding: 4px 10px;
    font-size: 12px;
    font-family: ui-sans-serif, system-ui, sans-serif;
    cursor: pointer;
    box-shadow: 0 2px 6px rgba(0, 0, 0, 0.18);
  }
  .intra-line-pill:hover {
    filter: brightness(1.1);
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

  /* Amber sibling of `.add-comment`, sits just below it. See
   * HunkLines.svelte for the colour-coding rationale. */
  .add-note {
    position: absolute;
    right: -9px;
    top: calc(50% + 11px);
    transform: translateY(-50%);
    width: 18px;
    height: 18px;
    padding: 0;
    border: 1px solid var(--attention-border);
    border-radius: 4px;
    background: var(--bg-elevated);
    color: var(--attention-text);
    font-weight: 700;
    font-size: 11px;
    line-height: 16px;
    cursor: pointer;
    visibility: hidden;
    user-select: none;
  }

  .sbs-row:hover .add-note {
    visibility: visible;
  }

  .add-note:hover {
    background: var(--attention-border);
    color: var(--bg);
  }

  /* See HunkLines.svelte — match the adjacent diff row's tint so the
   * gutter and right-gap don't read as a dark stripe through the
   * column's color. */
  /* Thread rows are excluded from text selection so dragging a
   * vertical selection across them doesn't fragment the code-row
   * highlights or pollute the copied text. See HunkLines.svelte. */
  .sbs-threads {
    background: transparent;
    user-select: none;
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

  /* Thread fold marker — stroke-only chevron pinned to the left
   * edge of the gutter cell, vertically centered on the row
   * boundary so a long line number can flow across the gutter
   * without colliding. See HunkLines.svelte for the full design
   * rationale. */
  .thread-marker {
    position: absolute;
    left: -2px;
    /* Centered on the row boundary — see HunkLines.svelte for the
     * stacking-context rationale paired with the :has() rule below. */
    bottom: 0;
    transform: translateY(50%);
    width: 14px;
    height: 14px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--link);
    cursor: pointer;
    user-select: none;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  /* Hover feedback comes from `.highlight-anchor` on the comment's
   * anchored rows — see HunkLines.svelte. */

  /* Raise z-index on rows that host a marker so its bottom half,
   * which overflows into the next row's gutter cell, isn't clipped. */
  .ln:has(.thread-marker) {
    z-index: 2;
  }

  :global(.highlight-anchor) {
    background: var(--link-bg) !important;
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
