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
  import { hasUnreadReplies, isThreadFolded } from '../lib/resolution';
  import AnnotationBubble from './AnnotationBubble.svelte';
  import AnnotationComposer, {
    type AnnotationComposerTarget,
  } from './AnnotationComposer.svelte';
  import Bubble from './Bubble.svelte';
  import Chevron from './Chevron.svelte';
  import CommentThread from './CommentThread.svelte';
  import { computeHunkWordDiff, wrapRanges } from '../lib/wordDiff';
  import { alignBlock, alignedRows } from '../lib/hunkAlign';

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

  /** Per-thread fold — see HunkLines.svelte for the model. State
   *  lives in foldStore under kind `comment`, keyed by the top-level
   *  comment's (or annotation's) id. Aggregated per-line for the
   *  gutter marker. Shared `foldVersion` context wakes the
   *  aggregate when a fold happens in any other component. */
  const foldStore = getContext<FoldStore | undefined>('kata-fold-store');
  const foldVersionCtx = getContext<{ read: () => number; bump: () => void } | undefined>(
    'kata-fold-version',
  );

  function isFolded(id: string): boolean {
    foldVersionCtx?.read();
    return isThreadFolded(id, responses, foldStore, defaultThreadsCollapsed);
  }
  /** True when a thread is effectively expanded: not folded OR has
   *  unread replies (force-expand override). See HunkLines.svelte. */
  function isEffectivelyExpanded(commentId: string): boolean {
    return (
      !isFolded(commentId) ||
      hasUnreadReplies(commentId, responses, lastVisitAt, viewer)
    );
  }
  function toggleFoldOne(id: string) {
    if (!foldStore) return;
    foldStore.set('comment', id, !isFolded(id));
    foldVersionCtx?.bump();
  }

  /** All threads + notes anchored at this (side, line). */
  function entriesAt(side: Side, line: number): Array<
    { kind: 'comment'; c: CommentView } | { kind: 'note'; n: AnnotationView }
  > {
    const out: Array<
      { kind: 'comment'; c: CommentView } | { kind: 'note'; n: AnnotationView }
    > = [];
    for (const c of threadsAt(side, line)) out.push({ kind: 'comment', c });
    for (const n of annotationsAt(side, line)) out.push({ kind: 'note', n });
    return out;
  }
  /** True iff every thread + note at the line is effectively folded
   *  (folded and not unread-force-expanded). Drives the marker
   *  state and the click direction. */
  function allFoldedAt(side: Side, line: number): boolean {
    const entries = entriesAt(side, line);
    if (entries.length === 0) return true;
    for (const e of entries) {
      if (e.kind === 'comment') {
        if (isEffectivelyExpanded(e.c.comment_id)) return false;
      } else {
        if (!isFolded(e.n.annotation_id)) return false;
      }
    }
    return true;
  }

  /** Aggregate anchor range for any thread+note at (side, line) —
   *  used by the marker's hover-to-highlight effect. */
  function foldedRangeAt(side: Side, line: number): { start: number; end: number } | null {
    let start = Number.POSITIVE_INFINITY;
    let end = Number.NEGATIVE_INFINITY;
    for (const e of entriesAt(side, line)) {
      const target = e.kind === 'comment' ? e.c : e.n;
      const eff =
        target.anchor.kind === 'moved' || target.anchor.kind === 'drifted'
          ? target.anchor.new_lines
          : (target as { lines?: { start: number; end: number } }).lines;
      if (!eff) continue;
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
      // Outdated anchors get the gutter chevron, not the inline
      // highlight — see HunkLines.svelte.
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

  /** See HunkLines.svelte — full-row tint applies only to lines with at
   *  least one no-columns comment OR any annotation; outdated
   *  excluded. */
  const fullLineCommentedLines = $derived.by(() => {
    const set = new Set<string>();
    for (const c of comments) {
      if (!c.side) continue;
      if (c.anchor.kind === 'outdated') continue;
      if (c.columns) continue;
      const effective =
        c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
          ? c.anchor.new_lines
          : c.lines;
      if (!effective) continue;
      for (let l = effective.start; l <= effective.end; l++) {
        set.add(`${c.side}:${l}`);
      }
    }
    for (const n of annotations) {
      if (!n.side || !n.lines) continue;
      if (n.anchor.kind === 'outdated') continue;
      const effective =
        n.anchor.kind === 'moved' || n.anchor.kind === 'drifted'
          ? n.anchor.new_lines
          : n.lines;
      if (!effective) continue;
      for (let l = effective.start; l <= effective.end; l++) {
        set.add(`${n.side}:${l}`);
      }
    }
    return set;
  });

  /** See HunkLines.svelte's `outdatedEntriesFor` — the chevron in
   *  this prototype is reserved for outdated comments only. */
  function outdatedEntriesFor(side: Side, line: number): Array<
    { kind: 'comment'; c: CommentView } | { kind: 'note'; n: AnnotationView }
  > {
    const out: Array<
      { kind: 'comment'; c: CommentView } | { kind: 'note'; n: AnnotationView }
    > = [];
    for (const c of comments) {
      if (c.side !== side) continue;
      if (c.anchor.kind !== 'outdated') continue;
      if (!c.lines || c.lines.end !== line) continue;
      out.push({ kind: 'comment', c });
    }
    for (const n of annotations) {
      if (n.side !== side) continue;
      if (n.anchor.kind !== 'outdated') continue;
      if (!n.lines || n.lines.end !== line) continue;
      out.push({ kind: 'note', n });
    }
    return out;
  }

  function allOutdatedFoldedAt(side: Side, line: number): boolean {
    const entries = outdatedEntriesFor(side, line);
    if (entries.length === 0) return true;
    for (const en of entries) {
      if (en.kind === 'comment') {
        if (isEffectivelyExpanded(en.c.comment_id)) return false;
      } else {
        if (!isFolded(en.n.annotation_id)) return false;
      }
    }
    return true;
  }

  function toggleOutdatedAt(side: Side, line: number) {
    if (!foldStore) return;
    const entries = outdatedEntriesFor(side, line);
    const target = !allOutdatedFoldedAt(side, line);
    for (const en of entries) {
      foldStore.set(
        'comment',
        en.kind === 'comment' ? en.c.comment_id : en.n.annotation_id,
        target,
      );
    }
    foldVersionCtx?.bump();
  }

  function isCommented(side: Side, line: number | null | undefined): boolean {
    if (!showComments) return false;
    return line != null && commentedLines.has(`${side}:${line}`);
  }

  function isFullLineCommented(side: Side, line: number | null | undefined): boolean {
    if (!showComments) return false;
    return line != null && fullLineCommentedLines.has(`${side}:${line}`);
  }

  /** See HunkLines.svelte's `coveringEntriesFor` — matches anything
   *  whose anchor range covers `(side, line)`, not just items ending
   *  exactly at `line`. */
  function coveringEntriesFor(side: Side, line: number): Array<
    { kind: 'comment'; c: CommentView } | { kind: 'note'; n: AnnotationView }
  > {
    const out: Array<
      { kind: 'comment'; c: CommentView } | { kind: 'note'; n: AnnotationView }
    > = [];
    for (const c of comments) {
      if (c.side !== side) continue;
      const effective =
        c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
          ? c.anchor.new_lines
          : c.lines;
      if (!effective) continue;
      if (line < effective.start || line > effective.end) continue;
      out.push({ kind: 'comment', c });
    }
    for (const n of annotations) {
      if (n.side !== side) continue;
      const effective =
        n.anchor.kind === 'moved' || n.anchor.kind === 'drifted'
          ? n.anchor.new_lines
          : n.lines;
      if (!effective) continue;
      if (line < effective.start || line > effective.end) continue;
      out.push({ kind: 'note', n });
    }
    return out;
  }

  /** Click handler — see HunkLines.svelte's `onContentClick`. */
  function onContentClick(e: MouseEvent, side: Side, line: number | null | undefined) {
    if (line == null) return;
    if (!showComments) return;
    if (!foldStore) return;
    const t = e.target as HTMLElement | null;
    const onAnchor = !!t?.closest('.column-anchor');
    const onFullLine = isFullLineCommented(side, line);
    if (!onAnchor && !onFullLine) return;
    const entries = coveringEntriesFor(side, line);
    if (entries.length === 0) return;
    let anyExpanded = false;
    for (const en of entries) {
      if (en.kind === 'comment') {
        if (isEffectivelyExpanded(en.c.comment_id)) {
          anyExpanded = true;
          break;
        }
      } else {
        if (!isFolded(en.n.annotation_id)) {
          anyExpanded = true;
          break;
        }
      }
    }
    const target = anyExpanded;
    for (const en of entries) {
      foldStore.set(
        'comment',
        en.kind === 'comment' ? en.c.comment_id : en.n.annotation_id,
        target,
      );
    }
    foldVersionCtx?.bump();
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
        if (!isNaN(ln) && ln !== dragging.end) {
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
    if (!line) return html;
    const idx = hunkLineIndex(line);
    const wd = idx != null ? wordDiff.get(idx) : undefined;
    const side: Side =
      line.origin === 'removed' ? 'base' : line.tip_line != null ? 'tip' : 'base';
    const lineNum = side === 'base' ? line.base_line : line.tip_line;
    const cols = lineNum != null ? columnAnchorsFor(side, lineNum) : [];
    if (!html && !wd && cols.length === 0) return html;
    // See HunkLines.svelte — fall through with escaped plain text
    // when there's a wrap to apply but syntax-highlighted HTML isn't
    // ready, so the in-progress composer's column anchor still
    // visualizes.
    let out = html ?? escapeHtml(line.content.replace(/\n$/, ''));
    if (wd) out = wrapRanges(out, wd.ranges, `wd-${wd.kind}`);
    if (cols.length > 0) out = wrapRanges(out, cols, 'column-anchor');
    return out;
  }

  function escapeHtml(s: string): string {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  }

  /** See HunkLines.svelte's columnAnchorsFor — same multi-line role
   *  split (first → start col..EOL; middle → whole line; last →
   *  BOL..end col), plus the same in-progress-composer fallback so
   *  the precise range stays painted while the composer is open. */
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
      if (line < effective.start || line > effective.end) continue;
      if (effective.start === effective.end) {
        out.push({ start: c.columns.start, end: c.columns.end });
      } else if (line === effective.start) {
        out.push({ start: c.columns.start, end: Number.MAX_SAFE_INTEGER });
      } else if (line === effective.end) {
        out.push({ start: 0, end: c.columns.end });
      } else {
        out.push({ start: 0, end: Number.MAX_SAFE_INTEGER });
      }
    }
    if (
      composing?.kind === 'line' &&
      composing.file === filePath &&
      composing.side === side &&
      composing.columns &&
      line >= composing.startLine &&
      line <= composing.endLine
    ) {
      const { startLine, endLine, columns } = composing;
      if (startLine === endLine) {
        out.push({ start: columns.start, end: columns.end });
      } else if (line === startLine) {
        out.push({ start: columns.start, end: Number.MAX_SAFE_INTEGER });
      } else if (line === endLine) {
        out.push({ start: 0, end: columns.end });
      } else {
        out.push({ start: 0, end: Number.MAX_SAFE_INTEGER });
      }
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
          <!-- Higher z-index on rows with markers so the overflowing
               chevron half paints over the next row's sticky cell.
               See HunkLines.svelte. -->
          {@const leftHasMarker =
            leftLine != null &&
            showComments &&
            outdatedEntriesFor('base', leftLine).length > 0}
          {@const leftStackZ = leftHasMarker ? rows.length - i + 1 : undefined}
          <tr class="sbs-row {row.kind}">
            <!-- data-side/data-line are also on the gutter cell so the
                 drag-selection logic finds it via `elementFromPoint` even
                 when the user's cursor stays in the sticky gutter column. -->
            <td
              class="ln {row.left ? row.left.origin : 'empty'}"
              data-side="base"
              data-line={leftLine ?? ''}
              style:z-index={leftStackZ}
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
                {@const count = outdatedEntriesFor('base', ln).length}
                {@const folded = allOutdatedFoldedAt('base', ln)}
                {#if count > 0}
                  <button
                    type="button"
                    class="thread-marker"
                    class:folded
                    aria-pressed={!folded}
                    aria-label="{count} outdated comment{count === 1 ? '' : 's'}; click to {folded ? 'expand' : 'collapse'}"
                    title="{count} outdated comment{count === 1 ? '' : 's'} — click to {folded ? 'expand' : 'collapse'}"
                    onmousedown={(e) => e.preventDefault()}
                    onclick={() => toggleOutdatedAt('base', ln)}
                    onmouseenter={() => {
                      const r = foldedRangeAt('base', ln);
                      hoveredAnchor = r ? { side: 'base', start: r.start, end: r.end } : null;
                    }}
                    onmouseleave={() => (hoveredAnchor = null)}
                  ><Chevron dir={folded ? 'right' : 'down'} size={14} filled /></button>
                {/if}
              {/if}
              {row.left?.base_line ?? row.left?.tip_line ?? ''}
            </td>
            <td
              class={`content ${row.left ? row.left.origin : 'empty'}${isCommented('base', leftLine) ? ' commented' : ''}${isFullLineCommented('base', leftLine) ? ' commented-fullline' : ''}`}
              data-side="base"
              data-line={leftLine ?? ''}
              onclick={(e) => onContentClick(e, 'base', leftLine)}
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
          {@const leftAllFolded =
            row.left?.base_line != null && allFoldedAt('base', row.left.base_line)}
          {#if leftAnnotating || ((leftThreads.length > 0 || leftNotes.length > 0) && !leftAllFolded)}
            {@const leftGroupSize = leftThreads.length + leftNotes.length}
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
                      folded={isFolded(n.annotation_id)}
                      onfold={(a) => toggleFoldOne(a.annotation_id)}
                      showFold={leftGroupSize > 1}
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
                      {defaultThreadsCollapsed}
                      showFold={leftGroupSize > 1}
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
          {@const rightHasMarker =
            rightLine != null &&
            showComments &&
            outdatedEntriesFor('tip', rightLine).length > 0}
          {@const rightStackZ = rightHasMarker ? rows.length - i + 1 : undefined}
          <tr class="sbs-row {row.kind}">
            <td
              class="ln {row.right ? row.right.origin : 'empty'}"
              data-side="tip"
              data-line={rightLine ?? ''}
              style:z-index={rightStackZ}
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
                {@const count = outdatedEntriesFor('tip', ln).length}
                {@const folded = allOutdatedFoldedAt('tip', ln)}
                {#if count > 0}
                  <button
                    type="button"
                    class="thread-marker"
                    class:folded
                    aria-pressed={!folded}
                    aria-label="{count} outdated comment{count === 1 ? '' : 's'}; click to {folded ? 'expand' : 'collapse'}"
                    title="{count} outdated comment{count === 1 ? '' : 's'} — click to {folded ? 'expand' : 'collapse'}"
                    onmousedown={(e) => e.preventDefault()}
                    onclick={() => toggleOutdatedAt('tip', ln)}
                    onmouseenter={() => {
                      const r = foldedRangeAt('tip', ln);
                      hoveredAnchor = r ? { side: 'tip', start: r.start, end: r.end } : null;
                    }}
                    onmouseleave={() => (hoveredAnchor = null)}
                  ><Chevron dir={folded ? 'right' : 'down'} size={14} filled /></button>
                {/if}
              {/if}
              {row.right?.tip_line ?? row.right?.base_line ?? ''}
            </td>
            <td
              class={`content ${row.right ? row.right.origin : 'empty'}${isCommented('tip', rightLine) ? ' commented' : ''}${isFullLineCommented('tip', rightLine) ? ' commented-fullline' : ''}`}
              data-side="tip"
              data-line={rightLine ?? ''}
              onclick={(e) => onContentClick(e, 'tip', rightLine)}
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
          {@const rightAllFolded =
            row.right?.tip_line != null && allFoldedAt('tip', row.right.tip_line)}
          {#if rightAnnotating || ((rightThreads.length > 0 || rightNotes.length > 0) && !rightAllFolded)}
            {@const rightGroupSize = rightThreads.length + rightNotes.length}
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
                      folded={isFolded(n.annotation_id)}
                      onfold={(a) => toggleFoldOne(a.annotation_id)}
                      showFold={rightGroupSize > 1}
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
                      {defaultThreadsCollapsed}
                      showFold={rightGroupSize > 1}
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
    /* Same stacking-context trick as `.hunk` in HunkLines — keeps
     * the chevron's overflowing bottom half painted over the inter-
     * hunk separator below the table. */
    position: relative;
    z-index: 1;
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
    /* See HunkLines — `--border` so this rule is visible at the
     * same intensity as the separator pseudo and reads as one
     * continuous line through hunks and inter-hunk space. */
    border-right: 1px solid var(--border);
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

  /* See HunkLines.svelte — bg tint + underline for the click-on-
   * highlight prototype. */
  :global(.content .column-anchor) {
    background: var(--link-bg);
    box-shadow: inset 0 -2px 0 var(--link);
    cursor: pointer;
  }

  .ln.empty,
  .content.empty {
    background: var(--bg-panel);
  }

  .content.selected {
    box-shadow: inset 4px 0 0 var(--selection-rule);
    background-image: linear-gradient(var(--selection-tint), var(--selection-tint));
  }

  /* PROTOTYPE — see HunkLines.svelte. `.commented` carries the gutter
   * stripe (any comment on the line); `.commented-fullline` carries
   * the row tint + pointer cursor (at least one no-columns comment or
   * annotation, where the whole row is the click target). */
  .content.commented {
    box-shadow: inset 3px 0 0 var(--link);
  }
  .content.commented-fullline {
    background-image: linear-gradient(var(--selection-tint), var(--selection-tint));
    cursor: pointer;
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
    /* Position context for the gutter rule pseudo. */
    position: relative;
  }

  /* SBS thread-cell sits inside ONE side, so the rule's x is local
   * to the side (not wrapper). Both sides have the same 48-px col-
   * ln + 16-px padding + 1-px border = 65-px gutter, so hardcoding
   * is safe and avoids the wrapper-coord translation that the
   * inter-hunk separator pseudo needs. */
  .thread-cell::before {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 64px;
    width: 1px;
    background: var(--border);
    pointer-events: none;
  }

  /* Thread fold marker — stroke-only chevron pinned to the left
   * edge of the gutter cell, vertically centered on the row
   * boundary so a long line number can flow across the gutter
   * without colliding. See HunkLines.svelte for the full design
   * rationale. */
  .thread-marker {
    /* In the click-on-highlight prototype the chevron is reserved
     * for OUTDATED comments — the template only renders it when
     * `outdatedEntriesFor(side, line)` is non-empty. */
    position: absolute;
    left: -2px;
    /* Centered on the row boundary; the bottom half overflows the
     * cell and the row-index z-index on `.ln` (set inline below)
     * keeps it visible above the next row's sticky cell. See
     * HunkLines.svelte. */
    bottom: 0;
    transform: translateY(50%);
    width: 16px;
    height: 16px;
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
