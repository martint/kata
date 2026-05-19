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
     *  edit/delete affordances on existing bubbles. Annotation
     *  creation is initiated from the selection popup in
     *  FileDiff, not from this component. */
    canAnnotate?: boolean;
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

  let tableEl: HTMLTableElement | undefined = $state();

  /** Gutter-drag is owned by FileDiff via the `lineDrag` context so a
   *  drag started in this hunk can paint `.selected` on rows in other
   *  hunks of the same file as the pointer crosses into them. The
   *  pointer handler below writes through the setter; the effect that
   *  applies `.selected` lives in FileDiff. */
  type LineDragState = {
    side: Side;
    start: number;
    end: number;
  } | null;
  const lineDrag = getContext<{
    current: () => LineDragState;
    set: (s: LineDragState) => void;
  } | undefined>('lineDrag');

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
    const wd = wordDiff.get(idx);
    const a = anchor(line);
    const cols = a ? columnAnchorsFor(a) : [];
    // No highlighting from any source → caller renders plain text.
    if (!base && !wd && cols.length === 0) return base;
    // Need to wrap (word-diff or column-anchor) but the renderer hasn't
    // produced syntax-highlighted HTML for this line yet (or never
    // will — binary file, unsupported language). Use escaped plain
    // text as the base so the wrap has something to apply against;
    // otherwise the column-anchor wouldn't visualize the in-progress
    // composer's range during the compose step and the reviewer
    // would briefly lose sight of what they're commenting on.
    if (!base) base = escapeHtml(line.content.replace(/\n$/, ''));
    if (wd) {
      base = wrapRanges(base, wd.ranges, `wd-${wd.kind}`);
    }
    if (cols.length > 0) {
      base = wrapRanges(base, cols, 'column-anchor');
    }
    return base;
  }

  function escapeHtml(s: string): string {
    return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
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
    // The in-progress composer's range is painted by the precise
    // selection-overlay in FileDiff (driven by both the user's
    // active selection AND the composer state). It used to be
    // synthesized here as a `.column-anchor` span wrap; that path
    // duplicated the overlay paint visually now.
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
  /** Per-line state for the gutter stripe.
   *
   *  `present`: line is inside at least one comment's range. Gets
   *  the basic stripe (replaces the old set membership).
   *  `startsHere` / `endsHere`: at least one comment's range
   *  starts / ends on this line. Drives the rounded end-caps so a
   *  multi-line range reads as one continuous pill and two adjacent
   *  single-line comments read as two separate pills (a single-
   *  line range has `startsHere && endsHere`, so the stripe gets
   *  caps on BOTH ends).
   *
   *  Outdated anchors are excluded — same reason the old
   *  `commentedLines` skipped them: tinting new content for an
   *  outdated thread misleads the reader. */
  const commentRangeByLine = $derived.by(() => {
    const map = new Map<
      string,
      { present: boolean; startsHere: boolean; endsHere: boolean }
    >();
    const get = (key: string) => {
      let v = map.get(key);
      if (!v) {
        v = { present: false, startsHere: false, endsHere: false };
        map.set(key, v);
      }
      return v;
    };
    for (const c of comments) {
      if (!c.side) continue;
      if (c.anchor.kind === 'outdated') continue;
      const effective =
        c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
          ? c.anchor.new_lines
          : c.lines;
      if (!effective) continue;
      for (let l = effective.start; l <= effective.end; l++) {
        get(`${c.side}:${l}`).present = true;
      }
      get(`${c.side}:${effective.start}`).startsHere = true;
      get(`${c.side}:${effective.end}`).endsHere = true;
    }
    return map;
  });

  /** Subset of `commentedLines` that have at least one comment WITHOUT
   *  `columns` — those get the full-row background tint in the click-
   *  on-highlight model. Lines with only column-anchored comments get
   *  no row tint; their highlight is the underlined-text bg painted
   *  by `.column-anchor`. Annotations are always whole-line, so any
   *  annotation on the line also triggers the full-row tint. Outdated
   *  anchors skipped here too (same reason as `commentedLines`). */
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

  /** Comments and annotations whose anchor is OUTDATED and whose
   *  effective end-line is this row. They get a gutter chevron (the
   *  inline-highlight model can't anchor a precise highlight, since
   *  the content has changed), clicking it toggles fold on the
   *  outdated items. */
  function outdatedEntriesFor(a: { side: Side; line: number }): Array<
    { kind: 'comment'; c: CommentView } | { kind: 'note'; n: AnnotationView }
  > {
    const out: Array<
      { kind: 'comment'; c: CommentView } | { kind: 'note'; n: AnnotationView }
    > = [];
    for (const c of comments) {
      if (c.side !== a.side) continue;
      if (c.anchor.kind !== 'outdated') continue;
      if (!c.lines || c.lines.end !== a.line) continue;
      out.push({ kind: 'comment', c });
    }
    for (const n of annotations) {
      if (n.side !== a.side) continue;
      if (n.anchor.kind !== 'outdated') continue;
      if (!n.lines || n.lines.end !== a.line) continue;
      out.push({ kind: 'note', n });
    }
    return out;
  }

  function allOutdatedFoldedAt(a: { side: Side; line: number }): boolean {
    const entries = outdatedEntriesFor(a);
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

  function toggleOutdatedAt(a: { side: Side; line: number }) {
    if (!foldStore) return;
    const entries = outdatedEntriesFor(a);
    const target = !allOutdatedFoldedAt(a);
    for (const e of entries) {
      foldStore.set('comment', idOf(e), target);
    }
    foldVersionCtx?.bump();
  }

  function isCommented(a: { side: Side; line: number } | null): boolean {
    if (!showComments) return false;
    return (
      a != null && (commentRangeByLine.get(`${a.side}:${a.line}`)?.present ?? false)
    );
  }

  function isFullLineCommented(a: { side: Side; line: number } | null): boolean {
    if (!showComments) return false;
    return a != null && fullLineCommentedLines.has(`${a.side}:${a.line}`);
  }

  /** End-cap modifiers for `.row.commented .content`. Returns a
   *  space-prefixed class string ready to concat into the `<tr>`
   *  class list. */
  function commentRangeClasses(
    a: { side: Side; line: number } | null,
  ): string {
    if (!a) return '';
    const r = commentRangeByLine.get(`${a.side}:${a.line}`);
    if (!r || !r.present) return '';
    let out = '';
    if (r.startsHere) out += ' range-start';
    if (r.endsHere) out += ' range-end';
    return out;
  }

  /** Comments / annotations whose anchor range COVERS this line —
   *  unlike `threadsFor` / `annotationsFor`, which only match items
   *  ending exactly at `a.line`. Needed so a click anywhere on a
   *  multi-line highlight (not just the last row) finds and toggles
   *  the comment. */
  function coveringEntriesFor(a: { side: Side; line: number }): Array<
    { kind: 'comment'; c: CommentView } | { kind: 'note'; n: AnnotationView }
  > {
    const out: Array<
      { kind: 'comment'; c: CommentView } | { kind: 'note'; n: AnnotationView }
    > = [];
    for (const c of comments) {
      if (c.side !== a.side) continue;
      const effective =
        c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
          ? c.anchor.new_lines
          : c.lines;
      if (!effective) continue;
      if (a.line < effective.start || a.line > effective.end) continue;
      out.push({ kind: 'comment', c });
    }
    for (const n of annotations) {
      if (n.side !== a.side) continue;
      const effective =
        n.anchor.kind === 'moved' || n.anchor.kind === 'drifted'
          ? n.anchor.new_lines
          : n.lines;
      if (!effective) continue;
      if (a.line < effective.start || a.line > effective.end) continue;
      out.push({ kind: 'note', n });
    }
    return out;
  }

  /** Click handler for `.content` cells. In the prototype, a click
   *  on a highlighted area (either a `.column-anchor` span or
   *  anywhere on a `.commented-fullline` row) toggles fold on every
   *  comment COVERING this line — works on any line of a multi-line
   *  range, not just the end line the chevron used to live on. Drag-
   *  selects still fire the selection popup as before; click events
   *  only fire for non-drag clicks, so the two paths don't collide. */
  function onContentClick(e: MouseEvent, a: { side: Side; line: number } | null) {
    if (!a) return;
    if (!showComments) return;
    if (!foldStore) return;
    const t = e.target as HTMLElement | null;
    const onAnchor = !!t?.closest('.column-anchor');
    const onFullLine = isFullLineCommented(a);
    if (!onAnchor && !onFullLine) return;
    const entries = coveringEntriesFor(a);
    if (entries.length === 0) return;
    // If any entry is currently expanded → fold all; otherwise expand
    // all. Same "normalize" rule the gutter marker's toggleAllAt uses.
    let anyExpanded = false;
    for (const e of entries) {
      if (e.kind === 'comment') {
        if (isEffectivelyExpanded(e.c.comment_id)) {
          anyExpanded = true;
          break;
        }
      } else {
        if (!isFolded(e.n.annotation_id)) {
          anyExpanded = true;
          break;
        }
      }
    }
    const target = anyExpanded;
    for (const e of entries) {
      foldStore.set('comment', idOf(e), target);
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
    lineDrag?.set({ side, start: line, end: line });

    const onMove = (ev: PointerEvent) => {
      const cur = lineDrag?.current();
      if (!cur) return;
      const el = document.elementFromPoint(ev.clientX, ev.clientY);
      const row = (el as HTMLElement | null)?.closest('[data-line]') as HTMLElement | null;
      if (row && row.getAttribute('data-side') === side) {
        const ln = Number(row.getAttribute('data-line'));
        if (!isNaN(ln) && ln !== cur.end) {
          lineDrag?.set({ ...cur, end: ln });
        }
      }
    };
    const onUp = () => {
      document.removeEventListener('pointermove', onMove);
      document.removeEventListener('pointerup', onUp);
      const cur = lineDrag?.current();
      lineDrag?.set(null);
      if (cur) {
        const start = Math.min(cur.start, cur.end);
        const end = Math.max(cur.start, cur.end);
        onstartcompose({
          kind: 'line',
          file: filePath,
          side: cur.side,
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
      <!-- Gutter-marker bookkeeping. In the click-on-highlight
           prototype the chevron is reserved for OUTDATED comments
           only — non-outdated comments use the inline highlight as
           their toggle affordance. Outdated comments can't have a
           meaningful inline highlight (their original content
           changed), so the chevron is the only marker available. -->
      {@const markerCount =
        showComments && a ? outdatedEntriesFor(a).length : 0}
      {@const markerFolded = a != null && allOutdatedFoldedAt(a)}
      <!-- Higher z-index for earlier rows that host a marker, so
           the chevron's overflowing bottom half paints over the
           next row's sticky `.ln`. Non-marker rows stay at the
           default z-index (no overflow to protect). Spec is z-
           index 1 baseline + (totalLines - i) so the earliest
           marker wins. -->
      {@const stackZ = markerCount > 0 ? hunk.lines.length - i + 1 : undefined}
      <tr class={`${rowClass(line.origin)}${isCommented(a) ? ' commented' : ''}${isFullLineCommented(a) ? ' commented-fullline' : ''}${commentRangeClasses(a)}`}>
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
            {/if}
            {#if a && !showTip && markerCount > 0}
              <button
                type="button"
                class="thread-marker"
                class:folded={markerFolded}
                aria-pressed={!markerFolded}
                aria-label="{markerCount} outdated comment{markerCount === 1 ? '' : 's'}; click to {markerFolded ? 'expand' : 'collapse'}"
                title="{markerCount} outdated comment{markerCount === 1 ? '' : 's'} — click to {markerFolded ? 'expand' : 'collapse'}"
                onmousedown={(e) => e.preventDefault()}
                onclick={() => toggleOutdatedAt(a)}
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
                aria-label="{markerCount} outdated comment{markerCount === 1 ? '' : 's'}; click to {markerFolded ? 'expand' : 'collapse'}"
                title="{markerCount} outdated comment{markerCount === 1 ? '' : 's'} — click to {markerFolded ? 'expand' : 'collapse'}"
                onmousedown={(e) => e.preventDefault()}
                onclick={() => toggleOutdatedAt(a)}
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
          onclick={(e) => onContentClick(e, a)}
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
    /* Click-target only — the visible highlight for both in-progress
     * composers AND published column-range comments is painted by
     * the precise overlay in FileDiff (no inter-line gap, uniform
     * color). This span wrap stays in the DOM so the
     * `onContentClick` handler can match `t.closest('.column-
     * anchor')` and toggle the right thread, but it carries no
     * visual style. */
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

  /* PROTOTYPE — click-on-highlight comment UI:
   * - `.row.commented` (any comment, including column-anchored) gets
   *   the vertical blue gutter stripe so the reader sees the line
   *   "has a comment" at a glance, same density signal the gutter
   *   chevron used to give.
   * - `.row.commented-fullline` (at least one no-columns comment OR
   *   any annotation on the line) ALSO gets the full-row tint and a
   *   pointer cursor — the whole content area becomes the click
   *   target that toggles the comment.
   * Lines that only have column-anchored comments don't get the
   *   full-row tint; their highlight is the underlined-text bg
   *   painted by `.column-anchor` above. */
  /* Gutter stripe is a pseudo-element so the FIRST and LAST line of
   * each comment range can grow a rounded "cap" (a small top/bottom
   * gap) — that's what tells a 2-line comment apart from two
   * adjacent single-line comments. The previous `box-shadow: inset`
   * stripe couldn't do per-row caps; the pseudo can.
   *
   * - middle of a multi-line range: full-height flat stripe (caps
   *   on the neighbours visually "round off" the range);
   * - `.range-start`: top inset by 2 px + rounded top corner — the
   *   start of a range looks like "the pill begins here";
   * - `.range-end`: bottom inset + rounded bottom corner — same
   *   trick at the end;
   * - single-line comment (`.range-start.range-end`): both gaps +
   *   both corners → a discrete pill. Two adjacent single-line
   *   comments therefore render as two clearly-separated pills.
   *
   * `.content` already has implicit static position; pseudo is
   * `pointer-events: none` so it can't swallow clicks meant for
   * the row. */
  .row.commented .content {
    position: relative;
  }
  .row.commented .content::before {
    content: '';
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    width: 3px;
    background: var(--link);
    pointer-events: none;
  }
  .row.commented.range-start .content::before {
    top: 2px;
    border-top-left-radius: 2px;
    border-top-right-radius: 2px;
  }
  .row.commented.range-end .content::before {
    bottom: 2px;
    border-bottom-left-radius: 2px;
    border-bottom-right-radius: 2px;
  }
  .row.commented-fullline .content {
    background-image: linear-gradient(var(--selection-tint), var(--selection-tint));
    cursor: pointer;
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
    /* In the click-on-highlight prototype the chevron is reserved
     * for OUTDATED comments — the template only renders it when
     * `outdatedEntriesFor(a)` is non-empty. Non-outdated comments
     * use the inline highlight as their toggle. */
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
     * trimmed to keep a little right-edge breathing room.
     *
     * `--message-max-w` caps the box at a comfortable reading width
     * on wide monitors — paired with the same cap (minus padding)
     * on `.line-composer-overlay` so the composer's white outer
     * rect lands exactly where the .comment box will appear inside
     * this stripe. Tune both together by changing this value. */
    --gutter: var(--measured-gutter, var(--gutter-offset));
    --message-max-w: 720px;
    position: sticky;
    left: var(--gutter);
    margin-left: var(--gutter);
    width: calc(var(--content-vp-width, 100%) - var(--gutter) - 12px);
    max-width: var(--message-max-w);
    background: var(--link-bg);
    padding: 8px 12px;
    border-top: 1px solid var(--border-muted);
    border-bottom: 1px solid var(--border-muted);
    border-left: 3px solid var(--link);
    box-sizing: border-box;
  }
</style>
