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
  import {
    intraLineSelectionFor,
    type IntraLineSelection,
  } from '../lib/intraLineSelection';

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

  /** Per-anchor thread fold state, persisted via foldStore. We track
   *  the set of (file:side:line) keys the user has explicitly
   *  toggled and merge with `defaultThreadsCollapsed` at read time —
   *  lets the user override the view-mode default on a per-thread
   *  basis. */
  const foldStore = getContext<FoldStore | undefined>('kata-fold-store');
  // Bumping this counter is how we re-trigger reads after a toggle.
  // The store is non-reactive (a plain object), so a $derived that
  // calls `foldStore.get(...)` wouldn't re-evaluate on its own.
  let foldVersion = $state(0);

  function threadKey(side: Side, line: number): string {
    return `${filePath}:${side}:${line}`;
  }

  function isThreadCollapsed(side: Side, line: number): boolean {
    // Read foldVersion to register a dependency — Svelte's $derived
    // tracking notices this and re-runs whenever toggleThreadFold
    // bumps the counter.
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

  /** Comments and annotations whose anchor still resolves to a row
   *  on this side at this line. Used to drive the gutter marker
   *  (which shows when any of these are folded) and the hover
   *  highlight (which paints the underlying line range). */
  function entriesFor(a: { side: Side; line: number }): {
    threads: CommentView[];
    notes: AnnotationView[];
  } {
    return {
      threads: threadsFor(a),
      notes: annotationsFor(a),
    };
  }

  /** Aggregate anchor range covered by the folded threads/notes at
   *  this row. Used so hovering the gutter marker can highlight the
   *  full line range an anchor spans, not just the row the marker
   *  itself sits on. */
  function foldedRange(a: { side: Side; line: number }): { start: number; end: number } | null {
    const { threads, notes } = entriesFor(a);
    let start = Number.POSITIVE_INFINITY;
    let end = Number.NEGATIVE_INFINITY;
    for (const c of threads) {
      const eff =
        c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
          ? c.anchor.new_lines
          : c.lines;
      if (!eff) continue;
      if (eff.start < start) start = eff.start;
      if (eff.end > end) end = eff.end;
    }
    for (const n of notes) {
      const eff =
        n.anchor.kind === 'moved' || n.anchor.kind === 'drifted'
          ? n.anchor.new_lines
          : n.lines;
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

  /** "Comment on selection" pill state: present iff the user has a
   *  text selection inside this hunk's `.content` cells (a single
   *  row). Cleared on outside mousedown or when the selection
   *  changes to something we can't anchor (multi-row, collapsed,
   *  outside this table). Click → open the line composer with the
   *  intra-line `columns` range prefilled. */
  let selectionPill: IntraLineSelection | null = $state.raw(null);
  $effect(() => {
    if (!tableEl) return;
    function onMouseUp() {
      // The selection isn't always finalised by the time the
      // mouseup handler runs; defer so the browser settles first.
      requestAnimationFrame(() => {
        if (!tableEl) return;
        selectionPill = intraLineSelectionFor(tableEl);
      });
    }
    function onMouseDown(e: MouseEvent) {
      // Mousedown on the pill itself shouldn't dismiss it — let the
      // click handler fire first. Anything else clears so the next
      // mouseup re-evaluates against a fresh selection.
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
    // Clear the underlying text selection too so it doesn't sit
    // there as visual noise once the composer takes over.
    window.getSelection()?.removeAllRanges();
  }

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
   *  Moved. We can't honour columns when the line has Drifted or
   *  gone Outdated — the character offsets index a different
   *  string than the one being rendered. */
  function columnAnchorsFor(a: { side: Side; line: number }): { start: number; end: number }[] {
    const out: { start: number; end: number }[] = [];
    for (const c of comments) {
      if (c.side !== a.side) continue;
      if (!c.columns) continue;
      // Single-line columns only — server enforces this on write.
      const effective =
        c.anchor.kind === 'moved'
          ? c.anchor.new_lines
          : c.anchor.kind === 'valid'
            ? c.lines
            : null;
      if (!effective) continue;
      if (effective.start !== effective.end) continue;
      if (effective.end !== a.line) continue;
      out.push({ start: c.columns.start, end: c.columns.end });
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
           `markerFolded` distinguishes folded vs expanded so the
           marker can show two visual states + serve as the single
           fold control in both directions. -->
      {@const markerCount =
        showComments && a ? threadsFor(a).length + annotationsFor(a).length : 0}
      {@const markerFolded = a != null && isThreadCollapsed(a.side, a.line)}
      <tr class={`${rowClass(line.origin)}${isCommented(a) ? ' commented' : ''}`}>
        {#if showBase}
          <!-- data-side/data-line are also on the gutter cell so that the
               drag-selection logic finds it via `elementFromPoint` while
               the user drags down the sticky gutter column. -->
          <td
            class="ln"
            data-side={a?.side ?? ''}
            data-line={a?.line ?? ''}
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
                onclick={() => toggleThreadFold(a.side, a.line)}
                onmouseenter={() => {
                  const r = foldedRange(a);
                  hoveredAnchor = r ? { side: a.side, start: r.start, end: r.end } : null;
                }}
                onmouseleave={() => (hoveredAnchor = null)}
              ><Chevron dir={markerFolded ? 'right' : 'down'} size={12} filled /></button>
            {/if}
            {line.base_line ?? ''}
          </td>
        {/if}
        {#if showTip}
          <td
            class="ln"
            data-side={a?.side ?? ''}
            data-line={a?.line ?? ''}
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
                onclick={() => toggleThreadFold(a.side, a.line)}
                onmouseenter={() => {
                  const r = foldedRange(a);
                  hoveredAnchor = r ? { side: a.side, start: r.start, end: r.end } : null;
                }}
                onmouseleave={() => (hoveredAnchor = null)}
              ><Chevron dir={markerFolded ? 'right' : 'down'} size={12} filled /></button>
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
        {@const hasContent = threads.length > 0 || notes.length > 0}
        {@const collapsed = hasContent && isThreadCollapsed(a.side, a.line)}
        {#if hasContent && !collapsed}
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
    border-right: 1px solid var(--border-muted);
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
    /* Centered exactly on the row boundary — half above, half below
     * — so the triangle reads as "between this line and the next."
     * Without raising the parent `.ln`'s z-index (rule below), the
     * bottom half would be painted over by the next sticky cell. */
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
  /* No hover tint on the marker itself — the `.highlight-anchor`
   * paint on the rows the comment covers is the hover feedback. */

  /* All sticky `.ln` cells share z-index: 1, so adjacent rows paint
   * in DOM order and a marker that overflows the row's bottom edge
   * gets clipped by the next row's cell. Raising the z-index just
   * for rows that actually host a marker keeps the bottom half
   * visible without otherwise changing the gutter's stacking. */
  .ln:has(.thread-marker) {
    z-index: 2;
  }

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
