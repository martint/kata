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

  interface Props {
    hunk: Hunk;
    filePath: string;
    comments: CommentView[];
    /** Author-attached context notes scoped to this file. May be empty.
     *  Filtered down to per-line just like `comments`. */
    annotations?: AnnotationView[];
    responses: ResponseView[];
    currentPatchset: number;
    composing: ComposerTarget | null;
    saving: boolean;
    /** Per-side, 1-based line number → rendered HTML. Populated from a
     *  whole-file tokenization so multi-line constructs render correctly. */
    highlights: { base: Map<number, string>; tip: Map<number, string> };
    /** 'both' for modified/renamed files; 'base' for deleted; 'tip' for added. */
    lineNumberMode?: 'both' | 'base' | 'tip';
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
     *  true to preserve call-sites that don't care about the
     *  per-commit-compare writeability gate. */
    commentsWriteable?: boolean;
    /** When non-null and matching this hunk's file, an annotation
     *  composer is open. Rendered inline at the anchor row. */
    composingAnnotation?: AnnotationComposerTarget | null;
    /** Anchor ids passed to the annotation composer. Side-aware:
     *  derived in FileDiff based on `composingAnnotation.side` so the
     *  annotation gets stored against the right commit. */
    annotationAnchorIds?: { change: string; commit: string };
    /** Whether the viewer is allowed to author annotations. Gates the
     *  "+ Note" gutter button and the edit/delete affordances on
     *  existing bubbles. */
    canAnnotate?: boolean;
    onstartannotate?: (target: AnnotationComposerTarget) => void;
    oncancelannotate?: () => void;
    onsubmitannotation?: (input: AnnotationInput) => Promise<void>;
    ondeleteannotation?: (annotation: AnnotationView) => Promise<void>;
    oneditannotation?: (annotation: AnnotationView) => void;
    /** Default fold state for threads at lines the user hasn't
     *  toggled explicitly. `diffs` view mode passes true so the
     *  diff stays clean and threads become clickable markers in the
     *  gutter; `both` mode passes false. */
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
    lineNumberMode = 'both',
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

  /** Per-thread fold state, persisted via foldStore (kind `comment`,
   *  keyed by comment_id or annotation_id). Default is resolution-
   *  aware: in Full view an open thread expands, a resolved thread
   *  folds. Annotations have no responses → always treated as
   *  `open`, so they follow `defaultThreadsCollapsed` directly.
   *
   *  The shared `foldVersion` context is what stitches the gutter
   *  marker's "any expanded?" computation to mutations that
   *  happen elsewhere (e.g. CommentThread's per-thread fold-back
   *  chevron). Reading `version.read()` registers this component's
   *  derivations as dependents; bumping wakes them. */
  const foldStore = getContext<FoldStore | undefined>('kata-fold-store');
  const foldVersionCtx = getContext<{ read: () => number; bump: () => void } | undefined>(
    'kata-fold-version',
  );

  function isFolded(id: string): boolean {
    foldVersionCtx?.read();
    return isThreadFolded(id, responses, foldStore, defaultThreadsCollapsed);
  }

  /** True when a thread is effectively expanded: either the user
   *  hasn't folded it, or it has unread replies (force-expand so a
   *  fresh response can't hide behind a resolver's fold). Used by
   *  both the per-line aggregate (gutter marker) and CommentThread's
   *  internal per-comment render decision. */
  function isEffectivelyExpanded(commentId: string): boolean {
    return (
      !isFolded(commentId) ||
      hasUnreadReplies(commentId, responses, lastVisitAt, viewer)
    );
  }

  /** Comments + annotations whose anchor resolves to a row on this
   *  (side, line). Each is its own thread; the gutter marker
   *  aggregates over the whole list. */
  function entriesFor(a: { side: Side; line: number }): Array<
    { kind: 'comment'; c: CommentView } | { kind: 'note'; n: AnnotationView }
  > {
    const out: Array<
      { kind: 'comment'; c: CommentView } | { kind: 'note'; n: AnnotationView }
    > = [];
    for (const c of threadsFor(a)) out.push({ kind: 'comment', c });
    for (const n of annotationsFor(a)) out.push({ kind: 'note', n });
    return out;
  }

  function idOf(e: { kind: 'comment'; c: CommentView } | { kind: 'note'; n: AnnotationView }): string {
    return e.kind === 'comment' ? e.c.comment_id : e.n.annotation_id;
  }

  /** Aggregate fold state for the line: true iff every thread at the
   *  line is effectively folded (i.e. folded and not unread-forced).
   *  The gutter marker uses this to pick ▶ (all folded) vs ▼ (any
   *  expanded) and to decide what one click should do — fold-all
   *  when everything is open, expand-all otherwise. */
  function allFoldedAt(a: { side: Side; line: number }): boolean {
    const entries = entriesFor(a);
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

  /** Normalize the fold state of every thread at the line. If any
   *  is currently expanded → fold them all; if all are folded →
   *  expand them all. So a click is always idempotent in its
   *  direction. */
  function toggleAllAt(a: { side: Side; line: number }) {
    if (!foldStore) return;
    const entries = entriesFor(a);
    const target = !allFoldedAt(a); // true = fold all, false = expand all
    for (const e of entries) {
      foldStore.set('comment', idOf(e), target);
    }
    foldVersionCtx?.bump();
  }

  /** Toggle fold of one item (annotation or thread) by id. Used by
   *  the per-bubble chevron on AnnotationBubble; CommentThread has
   *  its own self-contained toggle for comment threads. */
  function toggleFoldOne(id: string) {
    if (!foldStore) return;
    foldStore.set('comment', id, !isFolded(id));
    foldVersionCtx?.bump();
  }

  /** Aggregate anchor range covered by the threads/notes at this
   *  row. Used so hovering the gutter marker can highlight the full
   *  line range any anchor spans. */
  function foldedRange(a: { side: Side; line: number }): { start: number; end: number } | null {
    const entries = entriesFor(a);
    let start = Number.POSITIVE_INFINITY;
    let end = Number.NEGATIVE_INFINITY;
    for (const e of entries) {
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

  /** Hover-on-marker highlight state. Painted onto matching
   *  `[data-side][data-line]` cells via a `.highlight-anchor` class
   *  using the same direct-DOM trick as `dragSelected` — keeps the
   *  reactive cost off the per-row template. */
  let hoveredAnchor: { side: Side; start: number; end: number } | null = $state.raw(null);
  let hoveredEls: HTMLElement[] = [];
  $effect(() => {
    for (const el of hoveredEls) el.classList.remove('highlight-anchor');
    hoveredEls = [];
    if (!tableEl || !hoveredAnchor) return;
    const { side, start, end } = hoveredAnchor;
    for (let ln = start; ln <= end; ln++) {
      const matches = tableEl.querySelectorAll(
        `[data-side="${side}"][data-line="${ln}"]`,
      );
      for (const el of matches) {
        (el as HTMLElement).classList.add('highlight-anchor');
        hoveredEls.push(el as HTMLElement);
      }
    }
  });

  const showBase = $derived(lineNumberMode !== 'tip');
  const showTip = $derived(lineNumberMode !== 'base');
  /** When an existing draft is being edited, hide it from the thread so
   *  the composer below takes its visual slot instead of stacking under
   *  the original draft bubble. */
  const editingCommentId = $derived(composing?.editing?.commentId ?? null);
  /** Number of line-number gutter columns rendered before the content
   *  column. Used to size the thread's left padding so the comment
   *  body visually starts where the diff content does, even though
   *  the underlying `<td>` spans every column. */
  const lnCols = $derived((showBase ? 1 : 0) + (showTip ? 1 : 0));
  const colspan = $derived(lnCols + 1);

  let dragging: { side: Side; start: number; end: number } | null = $state(null);
  let tableEl: HTMLTableElement | undefined = $state();
  let dragSelected: HTMLElement[] = [];


  /** Apply the `.selected` class directly to matching rows when dragging.
   *  We bypass per-row Svelte reactivity here because toggling a top-level
   *  state like `composing` would otherwise re-evaluate every row's @const,
   *  which is O(file_size). */
  $effect(() => {
    for (const el of dragSelected) el.classList.remove('selected');
    dragSelected = [];
    if (!tableEl || !dragging) return;
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

  function rowClass(origin: HunkLine['origin']): string {
    switch (origin) {
      case 'added':
        return 'row added';
      case 'removed':
        return 'row removed';
      case 'context':
        return 'row context';
    }
  }

  /** Find the pre-tokenized HTML for a unified-diff line. Removed lines
   *  look up on the base side; added & context use the tip side. */
  function htmlFor(line: HunkLine): string | undefined {
    if (line.origin === 'removed') {
      return line.base_line != null ? highlights.base.get(line.base_line) : undefined;
    }
    if (line.tip_line != null) return highlights.tip.get(line.tip_line);
    if (line.base_line != null) return highlights.base.get(line.base_line);
    return undefined;
  }

  /** Word-level diff overlays per paired remove/add row in this hunk.
   *  Recomputed when the hunk lines change; cheap enough not to memo. */
  const wordDiff = $derived(computeHunkWordDiff(hunk.lines));

  /** Apply the word-diff overlay (if any) and any intra-line column
   *  highlights from comments anchored to this row. Word-diff goes
   *  on first (it has the loudest tint); column highlights layer on
   *  top so they remain visible inside word-diff regions. Falls back
   *  to the plain HTML when neither contributes — most rows go
   *  straight through. */
  function htmlWithWordDiff(line: HunkLine, idx: number): string | undefined {
    let base = htmlFor(line);
    if (!base) return base;
    const wd = wordDiff.get(idx);
    if (wd) {
      base = wrapRanges(base, wd.ranges, `wd-${wd.kind}`);
    }
    // Intra-line column anchors from any comment anchored to this
    // line. Only the still-Valid/Moved comments contribute a
    // highlight — drifted/outdated lines degrade to line-level
    // (the column offsets no longer map cleanly to the new text).
    const a = anchor(line);
    if (a) {
      const cols = columnAnchorsFor(a);
      if (cols.length > 0) {
        base = wrapRanges(base, cols, 'column-anchor');
      }
    }
    return base;
  }

  /** Column ranges to highlight on this row from any line-level
   *  comment with `columns` set whose anchor is still Valid or
   *  Moved. Drifted / outdated comments can't honour columns — the
   *  character offsets index a different string than the one being
   *  rendered, so those degrade to plain line-level marks.
   *
   *  Multi-line comments contribute differently per row:
   *  - first line: from `columns.start` to end-of-line (text-editor-
   *    style "from where the user clicked through the rest of the
   *    line");
   *  - middle lines: the whole line;
   *  - last line: from beginning-of-line to `columns.end`. */
  function columnAnchorsFor(a: { side: Side; line: number }): { start: number; end: number }[] {
    const out: { start: number; end: number }[] = [];
    for (const c of comments) {
      if (c.side !== a.side) continue;
      if (!c.columns) continue;
      const effective =
        c.anchor.kind === 'moved'
          ? c.anchor.new_lines
          : c.anchor.kind === 'valid'
            ? c.lines
            : null;
      if (!effective) continue;
      if (a.line < effective.start || a.line > effective.end) continue;
      if (effective.start === effective.end) {
        // Single-line comment — half-open `[start, end)` within the line.
        out.push({ start: c.columns.start, end: c.columns.end });
      } else if (a.line === effective.start) {
        // First line of a multi-line range: from the user's start col
        // through to EOL. `Number.MAX_SAFE_INTEGER` lets `wrapRanges`
        // clamp to the actual line length without us having to know it.
        out.push({ start: c.columns.start, end: Number.MAX_SAFE_INTEGER });
      } else if (a.line === effective.end) {
        // Last line: BOL to the user's end col.
        out.push({ start: 0, end: c.columns.end });
      } else {
        // Middle line: entirely covered.
        out.push({ start: 0, end: Number.MAX_SAFE_INTEGER });
      }
    }
    return out;
  }

  /** Which side+line a row anchors to. Removed → base side; added & context → tip. */
  function anchor(line: HunkLine): { side: Side; line: number } | null {
    if (line.origin === 'removed' && line.base_line != null) {
      return { side: 'base', line: line.base_line };
    }
    if (line.tip_line != null) {
      return { side: 'tip', line: line.tip_line };
    }
    if (line.base_line != null) {
      return { side: 'base', line: line.base_line };
    }
    return null;
  }

  function threadsFor(a: { side: Side; line: number }): CommentView[] {
    return comments.filter((c) => {
      if (c.side !== a.side) return false;
      const effective =
        c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
          ? c.anchor.new_lines
          : c.lines;
      return effective != null && effective.end === a.line;
    });
  }

  /** Annotations anchored to this `(side, line)`. Same anchor-revival
   *  semantics as `threadsFor` — moved/drifted anchors render at their
   *  current location, outdated ones fall through to the orphan
   *  bucket handled by `FileDiff`. */
  function annotationsFor(a: { side: Side; line: number }): AnnotationView[] {
    return annotations.filter((n) => {
      if (n.side !== a.side) return false;
      const effective =
        n.anchor.kind === 'moved' || n.anchor.kind === 'drifted'
          ? n.anchor.new_lines
          : n.lines;
      return effective != null && effective.end === a.line;
    });
  }

  /** True when an annotation composer is open AND its target is a
   *  line range ending on this (side, line). The composer renders on
   *  that "anchor" row — same convention used for displaying
   *  multi-line ranges. */
  function isAnnotatingHere(a: { side: Side; line: number }): boolean {
    return (
      composingAnnotation?.kind === 'line' &&
      composingAnnotation.file === filePath &&
      composingAnnotation.side === a.side &&
      composingAnnotation.endLine === a.line
    );
  }

  /** Start an annotation. Mirrors `onPointerDown` for the comment
   *  flow but skips the drag — annotations open with a single-line
   *  range by default; the composer can be edited to expand if
   *  needed (future enhancement). */
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

  /** Set of `side:line` keys for every line covered by a comment's
   *  anchored range. Used to tint the line so the reader can see what
   *  the thread (rendered below the last line) is talking about — a
   *  five-line range otherwise looks just like a thread attached to
   *  one line. The thread is still anchored to `effective.end`; this
   *  just decorates the rest of the range.
   *
   *  Outdated anchors are skipped: their `c.lines` points at a range
   *  whose content has since changed, so tinting the *new* content
   *  there would imply the comment is about lines it isn't. Those
   *  threads render at the file level instead — see the
   *  orphan-threads block in `FileDiff.svelte`. */
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

  function isCommented(a: { side: Side; line: number } | null): boolean {
    if (!showComments) return false;
    return a != null && commentedLines.has(`${a.side}:${a.line}`);
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
      const row = (el as HTMLElement | null)?.closest('[data-line]') as HTMLElement | null;
      if (row && row.getAttribute('data-side') === side) {
        const ln = Number(row.getAttribute('data-line'));
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
</script>

<table class="hunk" bind:this={tableEl}>
  <tbody>
    {#each hunk.lines as line, i (i)}
      {@const a = anchor(line)}
      {@const stripped = line.content.replace(/\n$/, '')}
      {@const html = htmlWithWordDiff(line, i)}
      <!-- Gutter-marker bookkeeping, hoisted up so the gutter cells
           below render the marker without recomputing per-cell.
           `markerCount === 0` means no comments at all on this row;
           `markerFolded` is the AGGREGATE state — true only when
           every thread at this line is currently folded. -->
      {@const markerCount =
        showComments && a ? threadsFor(a).length + annotationsFor(a).length : 0}
      {@const markerFolded = a != null && allFoldedAt(a)}
      <!-- Higher z-index for earlier rows that host a marker, so
           the chevron's overflowing bottom half paints over the
           next row's sticky `.ln`. Non-marker rows stay at the
           default z-index (no overflow to protect). Spec is z-
           index 1 baseline + (totalLines - i) so the earliest
           marker wins. -->
      {@const stackZ = markerCount > 0 ? hunk.lines.length - i + 1 : undefined}
      <tr class={`${rowClass(line.origin)}${isCommented(a) ? ' commented' : ''}`}>
        {#if showBase}
          <!-- data-side/data-line are also on the gutter cell so that the
               drag-selection logic finds it via `elementFromPoint` while
               the user drags down the sticky gutter column. -->
          <td
            class="ln"
            data-side={a?.side ?? ''}
            data-line={a?.line ?? ''}
            style:z-index={stackZ}
          >
            <!-- "+" button lives in the gutter cell (not the content
                 cell) so it stays visible while long lines scroll
                 horizontally — the gutter is `position: sticky`. -->
            {#if a && !showTip && showComments && commentsWriteable}
              <button
                type="button"
                class="add-comment"
                title="Click to comment; click-drag (or shift+click) to extend to multiple lines"
                onpointerdown={(e) => onPointerDown(e, a.side, a.line)}
              >
                <Bubble size={12} />
              </button>
              {#if canAnnotate}
                <button
                  type="button"
                  class="add-note"
                  title="Add author note (only the review creator can do this)"
                  onclick={() => startAnnotateHere(a.side, a.line)}
                >
                  N
                </button>
              {/if}
            {/if}
            {#if a && !showTip && markerCount > 0}
              <button
                type="button"
                class="thread-marker"
                class:folded={markerFolded}
                aria-pressed={!markerFolded}
                aria-label="{markerCount} comment{markerCount === 1 ? '' : 's'}; click to {markerFolded ? 'expand' : 'collapse'}"
                title="{markerCount} comment{markerCount === 1 ? '' : 's'} — click to {markerFolded ? 'expand' : 'collapse'}"
                onclick={() => toggleAllAt(a)}
                onmouseenter={() => {
                  const r = foldedRange(a);
                  hoveredAnchor = r ? { side: a.side, start: r.start, end: r.end } : null;
                }}
                onmouseleave={() => (hoveredAnchor = null)}
              ><Chevron dir={markerFolded ? 'right' : 'down'} size={14} filled /></button>
            {/if}
            {line.base_line ?? ''}
          </td>
        {/if}
        {#if showTip}
          <td
            class="ln"
            data-side={a?.side ?? ''}
            data-line={a?.line ?? ''}
            style:z-index={stackZ}
          >
            {#if a && showComments && commentsWriteable}
              <button
                type="button"
                class="add-comment"
                title="Click to comment; click-drag (or shift+click) to extend to multiple lines"
                onpointerdown={(e) => onPointerDown(e, a.side, a.line)}
              >
                <Bubble size={12} />
              </button>
            {/if}
            {#if a && markerCount > 0}
              <button
                type="button"
                class="thread-marker"
                class:folded={markerFolded}
                aria-pressed={!markerFolded}
                aria-label="{markerCount} comment{markerCount === 1 ? '' : 's'}; click to {markerFolded ? 'expand' : 'collapse'}"
                title="{markerCount} comment{markerCount === 1 ? '' : 's'} — click to {markerFolded ? 'expand' : 'collapse'}"
                onclick={() => toggleAllAt(a)}
                onmouseenter={() => {
                  const r = foldedRange(a);
                  hoveredAnchor = r ? { side: a.side, start: r.start, end: r.end } : null;
                }}
                onmouseleave={() => (hoveredAnchor = null)}
              ><Chevron dir={markerFolded ? 'right' : 'down'} size={14} filled /></button>
            {/if}
            {line.tip_line ?? ''}
          </td>
        {/if}
        <td
          class="content"
          data-side={a?.side ?? ''}
          data-line={a?.line ?? ''}
        >
          {#if html}
            <pre>{@html html || '&nbsp;'}</pre>
          {:else}
            <pre>{stripped || ' '}</pre>
          {/if}
        </td>
      </tr>
      {#if a && isAnnotatingHere(a)}
        <tr class="thread-row from-{line.origin}">
          <td colspan={colspan} class="thread-cell">
            <div class="thread-sticky" style="--gutter-offset: {lnCols * 65}px">
              {#if composingAnnotation}
                <AnnotationComposer
                  target={composingAnnotation}
                  anchorIds={annotationAnchorIds}
                  saving={saving}
                  oncancel={oncancelannotate}
                  onsubmit={onsubmitannotation}
                />
              {/if}
            </div>
          </td>
        </tr>
      {/if}
      {#if a && showComments}
        {@const threads = threadsFor(a)}
        {@const notes = annotationsFor(a)}
        <!-- Only suppress the entire row when EVERY thread + note at
             this line is folded (the gutter marker is then the only
             affordance). When some are folded and some expanded,
             pass everything to CommentThread and let it render the
             folded ones as header-only placeholders — that's the
             only way a user who folded one thread in a multi-thread
             line can find their way back to it without folding the
             whole group via the marker. -->
        {#if (threads.length > 0 || notes.length > 0) && !allFoldedAt(a)}
          {@const groupSize = threads.length + notes.length}
          <tr class="thread-row from-{line.origin}">
            <td colspan={colspan} class="thread-cell">
              <!-- Visual indent past the line-number gutter is done
                   with padding on the sticky wrapper rather than
                   empty gutter cells: empty cells confused the
                   sticky-width math (the thread-cell's column width
                   became the table's content column width, which can
                   be wider than the viewport) and produced an
                   awkward filler-coloured gap to the left. -->
              <div
                class="thread-sticky"
                style="--gutter-offset: {lnCols * 65}px"
              >
                {#each notes as n (n.annotation_id)}
                  <AnnotationBubble
                    annotation={n}
                    canEdit={canAnnotate}
                    onedit={oneditannotation}
                    ondelete={ondeleteannotation}
                    folded={isFolded(n.annotation_id)}
                    onfold={(a) => toggleFoldOne(a.annotation_id)}
                    showFold={groupSize > 1}
                  />
                {/each}
                {#if threads.length > 0}
                  <CommentThread
                    comments={threads}
                    {responses}
                    {saving}
                    {currentPatchset}
                    {editingCommentId}
                    {lastVisitAt}
                    {viewer}
                    {defaultThreadsCollapsed}
                    showFold={groupSize > 1}
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
      {/if}
    {/each}
  </tbody>
</table>

<style>
  .hunk {
    width: max-content;
    min-width: 100%;
    /* Firefox bug: `position: sticky` on a `<td>` doesn't reliably paint
     * its own background when the table uses `border-collapse: collapse`,
     * so the scrolling diff text bleeds visibly through the sticky
     * gutter cell. `separate` + zero border-spacing keeps the layout
     * identical while letting each sticky cell own its background. */
    border-collapse: separate;
    border-spacing: 0;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12.5px;
    /* Slightly looser than the global 1.45 — code lines packed at body
     * line-height read as cramped. Mirrored on .hunk-half. */
    line-height: 1.6;
    /* Stacking context for the table as a whole so the chevron
     * overflowing the last row paints over the inter-hunk
     * separator (.hunk-gap / .expand-row) below it. Within the
     * .hunks container, positioned z-index:1 paints after the
     * non-positioned separators, even though they come later in
     * DOM order. */
    position: relative;
    z-index: 1;
  }

  /* Keep code aligned to the top of its row even when the sibling .ln
   * cell grows tall via the `.composing-target` padding-bottom hack
   * (FileDiff applies it when an inline composer opens). Without this,
   * the default `baseline` vertical-align centers the code line in
   * the now-tall row, which looks like the line "shifts down" and
   * leaves the composer overlay sitting on top of it. */
  .hunk td {
    vertical-align: top;
  }

  /* `.row` itself needs no rules — line-height inherits from the table's
   * default, matching the side-by-side renderer. An explicit value here
   * previously made unified rows 2.5px taller and read as inconsistent
   * spacing across diff styles. */

  .row.added {
    background: var(--add-bg);
  }

  .row.removed {
    background: var(--remove-bg);
  }

  /* Word-level diff overlay: the line-level row tint already says
   * 'something on this line changed'; these stronger backgrounds
   * say 'these are the specific characters that differ'. The :global
   * selector reaches the spans we inject into shiki's pre-rendered
   * HTML via `wrapRanges`. */
  :global(.row.removed .wd-removed) {
    background: var(--remove-word-bg);
    border-radius: 2px;
  }

  :global(.row.added .wd-added) {
    background: var(--add-word-bg);
    border-radius: 2px;
  }

  /* Intra-line comment column anchor. Subtle inset border on top + bottom
   * (rather than a background tint that'd clash with word-diff highlights
   * in the same region) so the highlight stays visible even when nested
   * inside a wd-added/wd-removed run. Color picks up the comment palette
   * so the eye links it to the inline thread below. */
  :global(.content .column-anchor) {
    box-shadow: inset 0 -2px 0 var(--link);
    cursor: pointer;
  }

  /* Floating "Comment on selection" pill positioned in document
   * (not viewport) coordinates so it stays anchored to the text
   * during page scroll without a scroll listener. */
  .row.context {
    background: var(--bg);
  }

  /* Drag-selection lands on the content TD (that's where data-side/data-line
   * live), not the row, so the rule targets that cell. */
  .content.selected {
    box-shadow: inset 4px 0 0 var(--selection-rule);
    background-image: linear-gradient(var(--selection-tint), var(--selection-tint));
  }

  /* A row covered by a posted comment's anchor range. Tints the content
   * cell so the reader sees which line(s) the thread (rendered below
   * the last covered row) is about — particularly important for
   * multi-line ranges, which would otherwise look indistinguishable
   * from a thread attached to a single line. The left stripe matches
   * the `.thread-sticky` accent so the eye links the two together. */
  .row.commented .content {
    box-shadow: inset 3px 0 0 var(--link);
    background-image: linear-gradient(var(--selection-tint), var(--selection-tint));
  }

  .ln {
    width: 48px;
    text-align: right;
    padding: 0 8px;
    color: var(--text-faint);
    user-select: none;
    background: var(--bg);
    /* `--border` (not `--border-muted`) so this rule is visible at
     * the same intensity as the matching pseudo-element painted
     * through the inter-hunk separators; otherwise the in-table
     * rule reads as faint and the separator rule reads as crisp,
     * which made the line look broken at every hunk boundary. */
    border-right: 1px solid var(--border);
    /* Pin the gutter to the left edge of the horizontal scroll context so
     * the line number AND the "+" button stay visible while long lines
     * scroll out to the right. The textarea-focus workaround in
     * CommentComposer.svelte already absorbs the ~1.5s Firefox cost that
     * scaled with the number of sticky cells, so we can afford this. */
    position: sticky;
    left: 0;
    z-index: 1;
  }

  .row.added .ln {
    background: var(--add-bg-strong);
  }

  .row.removed .ln {
    background: var(--remove-bg-strong);
  }

  .content {
    /* Padding moved to symmetric 8px now that the "+" button lives inside
     * the .ln gutter cell — no need to reserve a left margin in the
     * content cell. */
    padding: 0 8px;
  }

  .content pre {
    margin: 0;
    font: inherit;
    white-space: pre;
    overflow-wrap: normal;
    word-break: normal;
  }

  /* "+" sits inside the sticky `.ln` cell — it scrolls vertically with
   * the row but stays pinned to the left edge during horizontal scroll.
   * Center the button on the gutter/diff boundary (the cell's right
   * border) so it never collides with the right-aligned line number on
   * its left, and overlaps only the diff cell's left padding (no
   * actual code) on its right. The .ln cell carries a stacking context
   * via `position: sticky`, so the button reliably paints above the
   * adjacent .content cell without any explicit z-index. */
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

  .row:hover .add-comment {
    visibility: visible;
  }

  .add-comment:hover {
    background: var(--link);
    color: var(--on-accent);
    border-color: var(--link);
  }

  /* Sibling of `.add-comment` — sits just below so both gutter
   * affordances stay close to the line they target. Amber palette
   * matches AnnotationBubble / AnnotationComposer so the reader
   * builds the colour mapping (blue = comment, amber = note). */
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

  .row:hover .add-note {
    visibility: visible;
  }

  .add-note:hover {
    background: var(--attention-border);
    color: var(--bg);
  }

  /* The thread-row's tinted background bleeds through wherever the
   * inner sticky block doesn't cover — that's the line-number gutter
   * area on the left and the small right-edge gap. Tinting it to
   * match the adjacent diff row's row-level color makes the thread
   * read as embedded in the (added/removed) hunk rather than
   * floating over the page background. */
  /* Thread rows interleave between code rows. Without this, dragging
   * a selection across them adds an awkward highlighted bar for each
   * thread that happens to sit in the middle of the selection — the
   * actual code rows look fragmented as a result. Threads are not
   * code, so excluding them from text selection is the right thing
   * for both visual continuity and copy-as-text. */
  .thread-row {
    background: transparent;
    user-select: none;
  }

  .thread-row.from-added {
    background: var(--add-bg);
  }

  .thread-row.from-removed {
    background: var(--remove-bg);
  }

  .thread-cell {
    padding: 0;
    background: transparent;
    /* Position context for the gutter-rule pseudo-element below. */
    position: relative;
  }

  /* Gutter rule through the thread-row — same stacked-gradient
   * approach as FileDiff's inter-hunk separators, so unified-both
   * gets BOTH the inner and outer rules continuous. The thread-
   * cell spans every column, so the table's `.ln` border-right
   * doesn't reach this row; the pseudo puts the rule(s) back. */
  .thread-cell::before {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: 0;
    right: 0;
    pointer-events: none;
    background:
      linear-gradient(
        to right,
        transparent 0,
        transparent calc(var(--measured-gutter, 65px) - 1px),
        var(--border) calc(var(--measured-gutter, 65px) - 1px),
        var(--border) var(--measured-gutter, 65px),
        transparent var(--measured-gutter, 65px),
        transparent 100%
      ),
      linear-gradient(
        to right,
        transparent 0,
        transparent calc(var(--measured-gutter-2, -1000px) - 1px),
        var(--border) calc(var(--measured-gutter-2, -1000px) - 1px),
        var(--border) var(--measured-gutter-2, -1000px),
        transparent var(--measured-gutter-2, -1000px),
        transparent 100%
      );
  }

  /* Thread fold marker — a solid disclosure triangle pinned to the
   * left edge of the sticky gutter, vertically centered on the row's
   * *bottom* boundary so it sits where the expanded thread will drop
   * down. Two design forces drive that placement:
   *
   *  - No bubble background. A filled bubble was reading as the
   *    same affordance as the chat-bubble "add a comment" glyph on
   *    the gutter's right edge; a bare solid triangle disambiguates
   *    while staying small and clickable on a clean diff.
   *  - On the bottom boundary, not centered on the line. The line-
   *    number text is vertically centered inside each row, so a
   *    4-or-more-digit number can flow across the gutter without
   *    the triangle overlapping it. Sitting on the bottom edge
   *    also visually points the eye at where the expanded thread
   *    will appear next.
   *
   * Triangle direction (▶ folded → ▼ expanded) is the only state
   * cue — color stays constant, so the affordance reads the same
   * whether folded or expanded. Hover gets a subtle tint so the
   * click target is discoverable. */
  .thread-marker {
    position: absolute;
    left: -2px;
    /* Centered on the row boundary — half above, half below — so
     * the triangle visually sits between this line and the next.
     * The bottom half overflows the cell; the row's `.ln` carries
     * a row-index-derived `z-index` (see the inline style on the
     * gutter cell) so each row paints over the one below it, which
     * keeps the overflowing half visible when adjacent lines both
     * host a marker. */
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
  /* No hover tint on the marker itself — the `.highlight-anchor`
   * paint on the rows the comment covers is the hover feedback. */


  /* Hover-highlight painted onto the rows an anchor covers. Applied
   * imperatively via `hoveredEls` (see the $effect) to keep per-row
   * reactivity off the bulk path. */
  :global(.highlight-anchor) {
    background: var(--link-bg) !important;
  }

  /* The sticky wrapper lets the thread stay at the visible viewport while
   * the underlying table scrolls horizontally. Width comes from the file's
   * .hunks ResizeObserver (see FileDiff).
   *
   * The blue tint + left stripe makes inline comment threads pop against
   * the surrounding diff rows; with the previous --bg-panel background
   * they blended into context lines and were easy to miss. */
  .thread-sticky {
    /* `--measured-gutter` is published by FileDiff via ResizeObserver on
     * the first `.content` cell — the rendered offset where the diff
     * content begins. `--gutter-offset` (set inline on this element)
     * is the hardcoded `lnCols * 65` fallback used before the first
     * measurement and if measurement fails. The block's tinted box
     * starts at the measured offset so its 3px left stripe lines up
     * with the .row.commented .content stripe; sticky `left` keeps
     * the block pinned during horizontal scroll, and the width is
     * trimmed to keep a little right-edge breathing room. */
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
