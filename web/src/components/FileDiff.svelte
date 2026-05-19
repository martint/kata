<script lang="ts">
  import { getContext, setContext, tick } from 'svelte';
  import { copyText } from '../lib/clipboard';
  import { api } from '../lib/api';
  import type {
    AnnotationView,
    CommentView,
    ComposerTarget,
    DraftCommentInput,
    FileChange,
    Hunk,
    HunkLine,
    Patchset,
  } from '../lib/types';
  import { SvelteMap } from 'svelte/reactivity';
  import {
    langForPath,
    loadLang,
    tokenizeWholeFile,
    themeState,
    type LineHighlights,
  } from '../lib/highlight.svelte';
  import { isThreadFolded } from '../lib/resolution';
  import { diffSelectionFor, type DiffSelection } from '../lib/diffSelection';
  import { buildCopyText } from '../lib/diffCopy';
  import { installSelectionClamp } from '../lib/selectionClamp';
  import Bubble from './Bubble.svelte';
  import Chevron from './Chevron.svelte';
  import CommentComposer from './CommentComposer.svelte';
  import CommentThread from './CommentThread.svelte';
  import HunkLines from './HunkLines.svelte';
  import HunkLinesSideBySide from './HunkLinesSideBySide.svelte';
  import SelectionPopup from './SelectionPopup.svelte';
  import type { AnnotationComposerTarget } from './AnnotationComposer.svelte';

  import type { DraftResponseInput, ResolutionAction, ResponseView } from '../lib/types';

  interface Props {
    repo: string;
    file: FileChange;
    /** The patchset whose endpoints back the displayed diff. */
    patchset: Patchset;
    /** Tip commit of the patchset the viewer is comparing against, or
     *  `null` for the normal (base..tip) view. When set, the actual
     *  diff base is this commit rather than `patchset.base_commit` —
     *  used here so `highlightsBase` is built from the same commit
     *  the hunks' `base_line` numbers index into, otherwise removed-
     *  side lines would render with HTML pulled from the wrong file
     *  and show wildly unrelated content. */
    compareBaseCommit?: string | null;
    comments: CommentView[];
    /** Author annotations across the review. This component filters
     *  down to the ones scoped to its file (and to per-line on the
     *  way into HunkLines / HunkLinesSideBySide). */
    annotations?: AnnotationView[];
    composingAnnotation?: AnnotationComposerTarget | null;
    canAnnotate?: boolean;
    onstartannotate?: (target: AnnotationComposerTarget) => void;
    oncancelannotate?: () => void;
    onsubmitannotation?: (
      input: import('../lib/types').AnnotationInput,
    ) => Promise<void>;
    ondeleteannotation?: (annotation: AnnotationView) => Promise<void>;
    oneditannotation?: (annotation: AnnotationView) => void;
    responses: ResponseView[];
    currentPatchset: number;
    composing: ComposerTarget | null;
    saving: boolean;
    /** Render the diff hunks. When `false` the file collapses to a
     *  flat comments-only listing (the old "compact" mode). */
    showDiffs?: boolean;
    /** Render comment UI: the file-level thread, the inline per-row
     *  thread rows in HunkLines/SideBySide, orphan threads, the
     *  +comment buttons, and the file-comment button in the header.
     *  When `false`, the diff is shown without any of these. */
    showComments?: boolean;
    /** Gate for the `+ comment` affordances only — existing threads
     *  still render when false. See FileSlot's prop doc for the
     *  per-commit-compare design reason. */
    commentsWriteable?: boolean;
    /** Default per-thread collapse state. See FileSlot for details. */
    defaultThreadsCollapsed?: boolean;
    /** Base-side width fraction for the side-by-side renderer. 0.5
     *  by default (even split). Threaded down to every
     *  HunkLinesSideBySide instance so the page's SBS split is
     *  uniform. */
    sbsSplit?: number;
    /** Drag handler for the SBS divider. The setter sees ratios
     *  already constrained by the parent (clamp + snap). */
    setSbsSplit?: (next: number) => void;
    /** True while `FileSlot` is fetching the per-file diff. We render a
     *  "Loading…" placeholder instead of "Diff omitted" so the user
     *  gets feedback rather than confusing them into thinking the
     *  diff was suppressed. */
    loadingHunks?: boolean;
    onstartcompose: (target: ComposerTarget) => void;
    oncancelcompose: () => void;
    onsubmit: (input: DraftCommentInput) => Promise<void>;
    onreply: (input: DraftResponseInput) => Promise<void>;
    onstatus: (commentId: string, action: ResolutionAction) => Promise<void>;
    ondelete: (comment: CommentView) => Promise<void>;
    onedit: (comment: CommentView) => void;
    onselectpatchset: (n: number, commentId?: string) => void;
    /** Whole-file view toggle. Lifted to `FileSlot` (which always stays
     *  mounted, even when this component is virtualized out) so the
     *  user's choice persists when they scroll away and come back —
     *  otherwise the default `false` would re-fold the file every
     *  remount. */
    wholeFile?: boolean;
    /** Whole-file collapse toggle (▸/▾). Same rationale as `wholeFile`
     *  — lifted to `FileSlot` so virtualisation doesn't silently
     *  re-expand a file the user collapsed, and so the fold-store
     *  can persist the value across page reloads. */
    collapsed?: boolean;
    /** Timestamp of the viewer's previous open. Threaded to
     *  `CommentThread` to flag threads with new replies since then. */
    lastVisitAt?: string | null;
    /** Currently signed-in author identity. */
    viewer?: string;
  }
  let {
    repo,
    file,
    patchset,
    compareBaseCommit = null,
    comments,
    annotations = [],
    composingAnnotation = null,
    canAnnotate = false,
    onstartannotate = () => {},
    oncancelannotate = () => {},
    onsubmitannotation = async () => {},
    ondeleteannotation = async () => {},
    oneditannotation = () => {},
    responses,
    currentPatchset,
    composing,
    saving,
    showDiffs = true,
    showComments = true,
    commentsWriteable = true,
    defaultThreadsCollapsed = false,
    sbsSplit = 0.5,
    setSbsSplit = () => {},
    loadingHunks = false,
    onstartcompose,
    oncancelcompose,
    onsubmit,
    onreply,
    onstatus,
    ondelete,
    onedit,
    onselectpatchset,
    wholeFile = $bindable(false),
    collapsed = $bindable(false),
    lastVisitAt = null,
    viewer = '',
  }: Props = $props();

  /** Debug-mode hooks. `debug` comes from ReviewViewer's context
   *  (turned on by `?debug` in the URL). When true, the file header
   *  renders a "$" icon that toggles a panel showing the literal
   *  `jj diff` command equivalent for this file's current view —
   *  handy when cross-checking what the UI actually computes against
   *  what the CLI would say. */
  const debug = getContext<boolean>('kata-debug') ?? false;
  // Shared fold store + version (set up by ReviewViewer) — used by
  // the orphan-section group fold control below. Optional in
  // standalone tests; when missing, the toggle still works but
  // mutations aren't persisted across reloads.
  const foldStore = getContext<import('../lib/foldStore').FoldStore | undefined>(
    'kata-fold-store',
  );
  const foldVersionCtx = getContext<{ read: () => number; bump: () => void } | undefined>(
    'kata-fold-version',
  );

  /** Are all orphan-line comments currently folded? Drives the
   *  section chevron's direction (▶ vs ▼) and the click action
   *  (fold-all vs expand-all). Reads `foldVersion` to register a
   *  reactive dependency. */
  function orphanSectionAllFolded(): boolean {
    foldVersionCtx?.read();
    if (orphanLineComments.length === 0) return true;
    const allResponses = [...responses];
    for (const c of orphanLineComments) {
      if (!isThreadFolded(c.comment_id, allResponses, foldStore, defaultThreadsCollapsed)) {
        return false;
      }
    }
    return true;
  }

  function toggleOrphanSection() {
    if (!foldStore) return;
    const target = !orphanSectionAllFolded();
    for (const c of orphanLineComments) {
      foldStore.set('comment', c.comment_id, target);
    }
    foldVersionCtx?.bump();
  }
  let debugOpen = $state(false);
  /** Build the literal commit-to-commit `jj diff` command for the
   *  endpoints this file is currently being diffed against. Works
   *  for every mode (normal, compare cumulative, scoped commit,
   *  per-commit interdiff) because `patchset` is the synthetic
   *  view-endpoints set by ReviewViewer's `viewingFor` derivation,
   *  not the raw review-manifest patchset. For the libjj
   *  rebase-based interdiff path the UI's actual diff is not
   *  literal commit-to-commit, so the command output won't match —
   *  the debug panel notes that. */
  const jjCommand = $derived.by(() => {
    const base = patchset.base_commit;
    const tip = patchset.tip_commit;
    return `jj diff --from ${base} --to ${tip} -- ${shellQuote(file.path)}`;
  });
  /** POSIX-shell single-quote a path. Paths in our corpus don't have
   *  control chars; the only risk is spaces or special globbing
   *  chars. Wrap in single quotes and escape any embedded single
   *  quotes with the standard `'\''` dance. */
  function shellQuote(s: string): string {
    if (/^[A-Za-z0-9_./\-]+$/.test(s)) return s;
    return "'" + s.replace(/'/g, "'\\''") + "'";
  }

  /** When an existing draft is being edited, hide it from the thread so
   *  the composer below takes its visual slot instead of stacking under
   *  the original draft bubble. */
  const editingCommentId = $derived(composing?.editing?.commentId ?? null);
  let sectionEl: HTMLElement | undefined = $state();
  let hunksWrapperEl: HTMLDivElement | undefined = $state();
  let composerOverlayEl: HTMLDivElement | undefined = $state();
  let composerTargetEl: HTMLElement | null = null;
  /** Vertical position of the line composer overlay relative to
   *  `hunksWrapperEl`. Null while no line composer is open. */
  let composerTop: number | null = $state(null);

  /** Floating-selection-popup state. Lives at the file level rather
   *  than per-hunk so a selection that spans hunk boundaries (the
   *  startContainer is in one HunkLines table, the endContainer in
   *  another) still resolves — per-hunk handlers can't see across.
   *  Resolved against `hunksWrapperEl` which covers every hunk's
   *  content cell + every separator's neighbours. */
  let selectionPopup: DiffSelection | null = $state.raw(null);
  /** Viewport-coord anchor for the floating popup — captured from
   *  the mouseup's `clientX/Y` so the popup lands where the user
   *  released the pointer rather than at some far corner of the
   *  selection's bounding rect (which for a multi-line drag spans
   *  hundreds of px and pushes the popup off-screen). */
  let popupAnchorX = $state(0);
  let popupAnchorY = $state(0);
  /** True while a drag-to-select is in progress inside this file's
   *  diff. Used (together with `selectionPopup`) to hide the per-row
   *  `+ comment` / `+ note` gutter buttons so they don't compete
   *  visually with the text-selection workflow. */
  let dragSelecting = $state(false);
  /** Hide the gutter affordances while EITHER the user is dragging a
   *  text selection or the SelectionPopup is open — the two
   *  affordances are mutually exclusive paths to "make a comment",
   *  and the gutter button hovering in mid-drag (or sitting under
   *  the popup) reads as visual noise. */
  const hideGutterAffordances = $derived(
    dragSelecting || selectionPopup !== null,
  );
  $effect(() => {
    if (!hunksWrapperEl) return;
    function onMouseUp(e: MouseEvent) {
      // Clear the drag-selecting flag on any mouseup. Deferred via
      // setTimeout so the gutter buttons don't briefly reappear
      // between mouseup and the popup-appearance handler below — the
      // popup itself takes over the hide-affordance role once it
      // mounts.
      setTimeout(() => {
        dragSelecting = false;
      }, 0);
      // Skip when the mouseup is on the popup itself — the popup's
      // own click handlers (commentOnSelection / copySelection)
      // manage `selectionPopup` directly. Without this guard the
      // rAF below re-runs `diffSelectionFor` against the still-
      // alive selection and re-sets `selectionPopup` to the same
      // DiffSelection, making the popup reappear right after the
      // click action set it to null.
      const t = e.target as HTMLElement | null;
      if (t?.closest('.selection-popup')) return;
      // Skip when the mouseup is on a button or other interactive
      // element. A click on the chevron / "+ comment" button can
      // sometimes leave the browser with a stray text selection
      // (especially in a double-click sequence where the reflow
      // after the first click puts the second click on text), and
      // we don't want that to spawn the selection popup — the user's
      // intent was to operate the button, not to comment on text.
      if (t?.closest('button')) return;
      const x = e.clientX;
      const y = e.clientY;
      // Defer to next frame: the selection isn't always finalised by
      // the time mouseup fires. A second deferral (via setTimeout)
      // covers browsers that finalise after the rAF callback for
      // long multi-line drags.
      requestAnimationFrame(() => {
        setTimeout(() => {
          if (!hunksWrapperEl) return;
          const sel = diffSelectionFor(hunksWrapperEl);
          if (sel) {
            popupAnchorX = x;
            popupAnchorY = y;
          }
          selectionPopup = sel;
        }, 0);
      });
    }
    function onMouseDown(e: MouseEvent) {
      // Mousedown on the popup itself shouldn't dismiss it — let the
      // click handler fire first. Anything else clears so the next
      // mouseup re-evaluates against a fresh selection.
      const t = e.target as HTMLElement | null;
      if (t?.closest('.selection-popup')) return;
      selectionPopup = null;
      // Drop the overlay's "last valid selection" cache before
      // collapsing the document selection. Without this the cache
      // keeps the overlay painted: the click sets `dragSelecting =
      // true` a few lines below (any `.content` mousedown might be
      // the start of a new drag), and the selectionchange handler
      // would then fall back to `lastValidSel` instead of clearing
      // the overlay.
      lastValidSel = null;
      // Click outside the popup clears any document selection. The
      // browser already collapses the selection on most clicks (its
      // default mousedown behaviour), but interactive targets that
      // `preventDefault` mousedown (chevrons, gutter buttons) would
      // otherwise leave the prior selection painted. Explicitly
      // clearing here makes "click anywhere outside the highlight"
      // a consistent way to dismiss the selection.
      if (typeof window !== 'undefined') {
        window.getSelection()?.removeAllRanges();
      }
      // Activate the drag-selecting flag only when the drag starts
      // inside a content cell — that's the only place a text-select
      // drag is meaningful. Gutter / thread / separator clicks
      // wouldn't trigger a text selection, so the gutter buttons
      // shouldn't hide there.
      if (t?.closest('.content')) {
        dragSelecting = true;
        // Warm the file-content cache for both sides so a multi-hunk
        // selection that crosses an inter-hunk gap has the underlying
        // source available by the time the user copies. Both sides
        // because the drag side isn't determined until mouseup.
        void ensureFileContent('tip');
        void ensureFileContent('base');
      }
    }
    document.addEventListener('mouseup', onMouseUp);
    document.addEventListener('mousedown', onMouseDown);
    return () => {
      document.removeEventListener('mouseup', onMouseUp);
      document.removeEventListener('mousedown', onMouseDown);
    };
  });

  /** Keep drag-select bounded to the table the drag started in. Stops
   *  the selection from spilling across the SBS divider (mixing base
   *  + tip) or across hunk boundaries in unified mode (no useful
   *  anchor across an inter-hunk gap). The clamp listens on `document`
   *  internally so it can react to selection changes wherever the
   *  pointer goes; activation is gated on a mousedown inside
   *  `hunksWrapperEl`. */
  $effect(() => {
    if (!hunksWrapperEl) return;
    return installSelectionClamp(hunksWrapperEl);
  });

  /** Precise selection-paint overlays for the current text drag.
   *  Each rect covers one selected line's selected character range
   *  (computed via `Range.getBoundingClientRect`), with its height
   *  extended to meet the next rect's top so the inter-line gap is
   *  filled. Positioned in `hunksWrapperEl`-relative coords so the
   *  overlay layer scrolls / re-flows together with the diff. */
  let selectionOverlays: { top: number; left: number; width: number; height: number }[] = $state([]);

  /** Single shared implementation of the "tag every cell in a line
   *  range, then bridge inter-hunk separators between the topmost
   *  and bottommost cell" dance. Each paint path gets its own
   *  instance with closure-scoped tracking arrays, so cleanup
   *  doesn't trample another path's work.
   *
   *  - `cellClass`: row class to add to every matching `[data-side]
   *    [data-line]` cell. `null` skips the per-cell paint (used by
   *    the published-comments path — HunkLines already paints
   *    `.commented` on those cells; we only want the inter-hunk
   *    bridge).
   *  - `bridgeClass`: class added to inter-hunk separators inside
   *    the painted range. Distinct classes (`drag-bridge` for the
   *    transient cyan stripe, `comment-bridge` for the persistent
   *    blue published-comment stripe) so the two CSS rules can use
   *    their own color + width, and so the paint paths can't step
   *    on each other's separator state.
   *
   *  The bridge `--bridge-offset` follows the same gutter pick the
   *  composer overlay uses — left gutter for base / unified, right
   *  gutter for SBS tip — so the stripe lands in the same column
   *  as the inset box-shadow stripe on the cells. */
  function makeStripePaint(cellClass: string | null, bridgeClass: string) {
    let cells: HTMLElement[] = [];
    let bridges: HTMLElement[] = [];
    return {
      clear() {
        if (cellClass !== null)
          for (const c of cells) c.classList.remove(cellClass);
        for (const b of bridges) {
          b.classList.remove(bridgeClass);
          b.style.removeProperty('--bridge-offset');
        }
        cells = [];
        bridges = [];
      },
      paint(side: 'base' | 'tip', startLine: number, endLine: number) {
        if (!hunksWrapperEl) return;
        const wrapper = hunksWrapperEl;
        // Track the topmost / bottommost matching cell across this
        // call. For multi-call patterns (one paint() per published
        // comment, say) each call extends `cells` so the bridge
        // computed below covers only what *this* call painted.
        const startIdx = cells.length;
        for (let ln = startLine; ln <= endLine; ln++) {
          for (const cell of wrapper.querySelectorAll<HTMLElement>(
            `[data-side="${side}"][data-line="${ln}"]`,
          )) {
            if (cellClass !== null) cell.classList.add(cellClass);
            cells.push(cell);
          }
        }
        if (cells.length === startIdx) return;
        const firstRect = cells[startIdx].getBoundingClientRect();
        const lastRect = cells[cells.length - 1].getBoundingClientRect();
        const bridgeVar =
          sideBySide && side === 'tip'
            ? 'var(--measured-gutter-2)'
            : 'var(--measured-gutter)';
        for (const sep of wrapper.querySelectorAll<HTMLElement>(
          '.hunk-gap, .expand-row',
        )) {
          const r = sep.getBoundingClientRect();
          if (r.top >= firstRect.top && r.bottom <= lastRect.bottom) {
            sep.style.setProperty('--bridge-offset', bridgeVar);
            sep.classList.add(bridgeClass);
            bridges.push(sep);
          }
        }
      },
    };
  }
  /** Active text-selection / column-range composer path. */
  const overlayPaint = makeStripePaint('text-selection-stripe', 'drag-bridge');
  /** Gutter drag path (lifted to file level via `lineDrag` context
   *  so the highlight extends across hunks the drag traverses). */
  const gutterPaint = makeStripePaint('selected', 'drag-bridge');
  /** Whole-line composer path (line-range comment opened from a
   *  gutter drag — no `columns`). */
  const composerPaint = makeStripePaint('selected', 'drag-bridge');
  /** Published whole-line comments. Cells already carry `.commented`
   *  via HunkLines; this instance only contributes the inter-hunk
   *  bridge so a multi-hunk posted comment's blue gutter stripe
   *  reads as one continuous rule. */
  const commentedPaint = makeStripePaint(null, 'comment-bridge');
  /** Last `DiffSelection` that resolved successfully inside this
   *  file's wrapper. Re-used while a drag is in progress but the
   *  selection has transiently extended into a non-content area
   *  (mouse on an inter-hunk separator) or into a different file
   *  (mouse on another FileDiff's hunks). Without this cache the
   *  overlay would flicker off whenever the cursor passes over
   *  unselectable territory and the user would lose sight of what
   *  they've selected mid-drag. Cleared when the drag ends or when
   *  a fresh valid selection arrives. */
  let lastValidSel: ReturnType<typeof diffSelectionFor> = null;
  $effect(() => {
    if (!hunksWrapperEl) return;
    // Read `composing` and `comments` so the effect re-runs when
    // the composer opens / closes or when published comments
    // change. Without this dep the listener-only path wouldn't
    // re-evaluate on composer state changes or on a fresh draft
    // landing — it only fires on `selectionchange`.
    void composing;
    void comments;
    // Re-run when the user expands / collapses inter-hunk context.
    // Newly revealed lines need their own overlay rects (the
    // previous render's gap-fill stretched the rect for the line
    // before the gap to bridge it visually), and lines that go
    // back into a gap need their rects dropped. Same reason for
    // the file-header toggles: `wholeFile` swaps between hunks-only
    // and whole-file rendering (every line gets a cell), and
    // `collapsed` removes the hunks entirely.
    void expansions;
    void wholeFile;
    void collapsed;
    /** Compute the per-line overlay rectangles for one (side, lines,
     *  cols) range, with the inter-line gap filled by stretching
     *  each rect's bottom to meet the next. Returns an empty array
     *  if no cells in this file match the range. The per-line
     *  gutter stripe + inter-hunk bridge live in `overlayPaint`
     *  (called separately by the caller for the range that should
     *  get the row-level cue). */
    function rectsForRange(
      side: 'base' | 'tip',
      startLine: number,
      endLine: number,
      startCol: number,
      endCol: number,
    ): typeof selectionOverlays {
      if (!hunksWrapperEl) return [];
      // Look up each line's cell across the WHOLE wrapper rather
      // than a single hunk's table — multi-hunk selections render
      // overlays for every line that has a cell in this file, and
      // the gap-fill loop below stretches each rect to bridge the
      // inter-hunk separator visually. Hidden lines (between
      // rendered hunks) have no cell — the gap-fill covers them.
      const wrapperRect = hunksWrapperEl.getBoundingClientRect();
      const out: typeof selectionOverlays = [];
      for (let ln = startLine; ln <= endLine; ln++) {
        const pre = hunksWrapperEl.querySelector(
          `td.content[data-side="${side}"][data-line="${ln}"] pre`,
        ) as HTMLElement | null;
        if (!pre) continue;
        const lineLen = pre.textContent?.length ?? 0;
        const lStart = ln === startLine ? startCol : 0;
        const lEnd = ln === endLine ? endCol : lineLen;
        if (lEnd <= lStart) continue;
        const startLoc = locateInPre(pre, lStart);
        const endLoc = locateInPre(pre, lEnd);
        if (!startLoc || !endLoc) continue;
        const subRange = document.createRange();
        try {
          subRange.setStart(startLoc.node, startLoc.offset);
          subRange.setEnd(endLoc.node, endLoc.offset);
        } catch {
          continue;
        }
        const rect = subRange.getBoundingClientRect();
        if (rect.width === 0 || rect.height === 0) continue;
        out.push({
          top: rect.top - wrapperRect.top,
          left: rect.left - wrapperRect.left,
          width: rect.width,
          height: rect.height,
        });
      }
      // Gap-fill within this range only — adjacent rects bridge to
      // each other; the last keeps its natural height.
      for (let i = 0; i < out.length - 1; i++) {
        out[i].height = out[i + 1].top - out[i].top;
      }
      return out;
    }

    function update() {
      if (!hunksWrapperEl) {
        selectionOverlays = [];
        return;
      }
      // Reset stripes / overlays at the top — each call rebuilds.
      overlayPaint.clear();
      const allOverlays: typeof selectionOverlays = [];

      // 1. Published column-range comments on this file. Paint them
      //    via the precise overlay so they match the in-progress
      //    composer's visual exactly (same color, same gap fill).
      //    No stripe — published comments get their own gutter
      //    treatment via `.commented` in HunkLines / SBS.
      for (const c of comments) {
        if (c.file !== file.path) continue;
        if (!c.columns || !c.side) continue;
        if (c.anchor.kind === 'outdated') continue;
        const effective =
          c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
            ? c.anchor.new_lines
            : c.lines;
        if (!effective) continue;
        allOverlays.push(
          ...rectsForRange(
            c.side,
            effective.start,
            effective.end,
            c.columns.start,
            c.columns.end,
          ),
        );
      }

      // 2. Composer takes precedence over the live selection (the
      //    textarea autofocus collapses the document selection on
      //    mount, so `diffSelectionFor` would return null at that
      //    moment).
      let sel: DiffSelection | null = null;
      if (
        composing?.kind === 'line' &&
        composing.file === file.path &&
        composing.columns
      ) {
        sel = {
          side: composing.side,
          startLine: composing.startLine,
          endLine: composing.endLine,
          startCol: composing.columns.start,
          endCol: composing.columns.end,
          multiLine: composing.startLine !== composing.endLine,
          rect: new DOMRect(),
        };
      } else {
        sel = diffSelectionFor(hunksWrapperEl);
      }
      // During an active drag, hold the last valid selection through
      // transient invalid states.
      if (!sel) {
        if (dragSelecting && lastValidSel) {
          sel = lastValidSel;
        } else {
          selectionOverlays = allOverlays;
          lastValidSel = null;
          return;
        }
      } else {
        lastValidSel = sel;
      }
      allOverlays.push(
        ...rectsForRange(
          sel.side,
          sel.startLine,
          sel.endLine,
          sel.startCol,
          sel.endCol,
        ),
      );
      selectionOverlays = allOverlays;
      // Row-level cue (gutter stripe + inter-hunk bridge) for the
      // active selection / composer range. Published comments don't
      // get the row-level cue here — they have their own gutter
      // treatment via `.commented` in HunkLines / SBS.
      overlayPaint.paint(sel.side, sel.startLine, sel.endLine);
    }
    document.addEventListener('selectionchange', update);
    // Run once on attach (and on every effect re-run, e.g. when
    // `composing` changes) so the overlay reflects the current
    // state without waiting for the next selectionchange.
    update();
    return () => {
      document.removeEventListener('selectionchange', update);
      overlayPaint.clear();
      selectionOverlays = [];
    };
  });

  /** Gutter-drag state lifted to the file level so a drag that
   *  starts in one hunk and crosses into another paints `.selected`
   *  on rows in every hunk it covers (not just the starting hunk's
   *  table). HunkLines / HunkLinesSideBySide write to this via the
   *  `lineDrag` context below; the effect further down does all of
   *  the DOM work in a single place. */
  type LineDragState = {
    side: 'base' | 'tip';
    start: number;
    end: number;
  } | null;
  let lineDrag: LineDragState = $state.raw(null);
  setContext('lineDrag', {
    current: () => lineDrag,
    set: (s: LineDragState) => {
      lineDrag = s;
    },
  });

  $effect(() => {
    void lineDrag;
    gutterPaint.clear();
    if (!lineDrag) return;
    const min = Math.min(lineDrag.start, lineDrag.end);
    const max = Math.max(lineDrag.start, lineDrag.end);
    gutterPaint.paint(lineDrag.side, min, max);
  });

  /** Walk text nodes inside `pre` to find the (node, offset) for the
   *  character at index `col`. Mirrors the existing helper in
   *  selectionClamp / diffSelection — kept inline here because the
   *  overlay code is the only caller. */
  function locateInPre(pre: HTMLElement, col: number): { node: Node; offset: number } | null {
    let remaining = col;
    const walker = document.createTreeWalker(pre, NodeFilter.SHOW_TEXT);
    let node: Node | null = walker.nextNode();
    while (node) {
      const len = (node as Text).data.length;
      if (remaining <= len) return { node, offset: remaining };
      remaining -= len;
      node = walker.nextNode();
    }
    const last = pre.lastChild;
    if (last?.nodeType === Node.TEXT_NODE) {
      return { node: last, offset: (last as Text).data.length };
    }
    return null;
  }

  function commentOnSelection() {
    const s = selectionPopup;
    if (!s) return;
    onstartcompose({
      kind: 'line',
      file: file.path,
      side: s.side,
      startLine: s.startLine,
      endLine: s.endLine,
      columns: { start: s.startCol, end: s.endCol },
    });
    selectionPopup = null;
    // Collapse the document selection. The in-progress composer's
    // synthetic `column-anchor` (painted by `columnAnchorsFor` in
    // HunkLines / SBS) now carries the precise-range visual, so we
    // no longer need the browser's native selection paint. Leaving
    // the selection alive caused a regression where a later click
    // on a chevron in a different hunk would EXTEND the stale
    // anchor across hunks, making the entire intervening text look
    // selected.
    window.getSelection()?.removeAllRanges();
  }

  /** Open the annotation composer anchored to the current text
   *  selection. Same shape as `commentOnSelection` — line-level
   *  target on this file's `side`; annotations don't carry
   *  columns yet (the storage model has no `columns` field on
   *  Annotation), so the precise sub-line range is lost. Worth
   *  picking up later if author notes start wanting partial-
   *  selection precision. */
  function noteOnSelection() {
    const s = selectionPopup;
    if (!s) return;
    onstartannotate({
      kind: 'line',
      file: file.path,
      side: s.side,
      startLine: s.startLine,
      endLine: s.endLine,
    });
    selectionPopup = null;
    window.getSelection()?.removeAllRanges();
  }

  /** Cached file content for each side of the patchset. Populated
   *  lazily by `ensureFileContent` (kicked off as soon as a drag
   *  begins inside this file's `.content`, see the mousedown handler).
   *  Used to inject the underlying source for lines that aren't
   *  rendered as cells — inter-hunk gaps in particular. `null` means
   *  not-yet-fetched; once a fetch resolves the value is the full
   *  file text. A failure stays at `null` so the next attempt retries
   *  through `api.readFile`'s own cache. */
  let tipContent: string | null = $state(null);
  let baseContent: string | null = $state(null);
  let tipContentPending: Promise<void> | null = null;
  let baseContentPending: Promise<void> | null = null;

  function ensureFileContent(side: 'base' | 'tip'): Promise<void> {
    if (side === 'tip') {
      if (tipContent !== null) return Promise.resolve();
      if (tipContentPending) return tipContentPending;
      tipContentPending = api
        .readFile(repo, patchset.tip_commit, file.path)
        .then((text) => {
          tipContent = text;
        })
        .catch(() => {
          // Swallow — copy falls back to rendered-cell text.
        })
        .finally(() => {
          tipContentPending = null;
        });
      return tipContentPending;
    }
    if (baseContent !== null) return Promise.resolve();
    if (baseContentPending) return baseContentPending;
    const commit = compareBaseCommit ?? patchset.base_commit;
    baseContentPending = api
      .readFile(repo, commit, file.path)
      .then((text) => {
        baseContent = text;
      })
      .catch(() => {})
      .finally(() => {
        baseContentPending = null;
      });
    return baseContentPending;
  }

  /** Pick the cached source for `side` — null when the fetch hasn't
   *  resolved (or has failed); the copy helper then emits empty
   *  strings for hidden lines rather than stale rendered text. */
  function fileTextFor(side: 'base' | 'tip'): string | null {
    return side === 'tip' ? tipContent : baseContent;
  }

  async function copySelection() {
    const s = selectionPopup;
    if (!s || !hunksWrapperEl) return;
    // Make sure the side's file content is cached before building so
    // hidden inter-hunk lines copy with their real text rather than
    // an empty placeholder.
    await ensureFileContent(s.side);
    const text = buildCopyText(hunksWrapperEl, s, fileTextFor(s.side));
    if (text.length > 0) {
      await copyText(text);
    }
    selectionPopup = null;
    window.getSelection()?.removeAllRanges();
  }

  /** Document-level copy interceptor for Ctrl+C inside this file.
   *  Runs in the capture phase and `stopImmediatePropagation`s when
   *  the selection lives in this wrapper, so it wins over any other
   *  copy handler (the global `installDiffCopyHandler` in particular)
   *  that would otherwise overwrite `clipboardData` in the bubble
   *  phase. Selections outside this file are ignored — the other
   *  FileDiffs' handlers, or the global one, handle those. */
  $effect(() => {
    if (!hunksWrapperEl) return;
    const wrapper = hunksWrapperEl;
    function onCopy(e: ClipboardEvent) {
      const sel = window.getSelection();
      if (!sel || sel.isCollapsed || sel.rangeCount === 0) return;
      const range = sel.getRangeAt(0);
      // Only handle when the selection is anchored inside this
      // file. `commonAncestorContainer` lands inside the wrapper for
      // any selection that started and ended here.
      if (!wrapper.contains(range.commonAncestorContainer)) return;
      const diffSel = diffSelectionFor(wrapper);
      if (!diffSel) return;
      const text = buildCopyText(wrapper, diffSel, fileTextFor(diffSel.side));
      if (text.length === 0) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      e.clipboardData?.setData('text/plain', text);
      e.clipboardData?.setData(
        'text/html',
        text
          .replace(/&/g, '&amp;')
          .replace(/</g, '&lt;')
          .replace(/>/g, '&gt;'),
      );
    }
    document.addEventListener('copy', onCopy, true);
    return () => document.removeEventListener('copy', onCopy, true);
  });
  let hunksEl: HTMLDivElement | undefined = $state();

  /** Track the visible width of the file's .hunks scroll viewport so the
   *  sticky thread wrappers inside know how wide to render. */
  $effect(() => {
    if (!hunksEl) return;
    const ro = new ResizeObserver((entries) => {
      for (const e of entries) {
        (e.target as HTMLElement).style.setProperty(
          '--content-vp-width',
          `${e.contentRect.width}px`,
        );
      }
    });
    ro.observe(hunksEl);
    return () => ro.disconnect();
  });

  /** Measure the actual rendered offset of the first `.content` cell and
   *  publish it as `--measured-gutter` on the hunks wrapper. The inline
   *  thread's left stripe, the line-composer overlay, and the orphan-
   *  threads block all indent past the gutter; without measuring, they
   *  use a hardcoded `lnCols * 65` that drifts off whenever the table's
   *  auto-layout expands the line-number column (e.g. 5-digit line
   *  numbers, larger font). The .content cell's `offsetLeft` is the
   *  truth — observe it via the gutter cells (which actually trigger
   *  width changes) so the variable tracks the real gutter. */
  $effect(() => {
    if (!hunksWrapperEl) return;
    const wrapper = hunksWrapperEl;
    let lnCells: NodeListOf<Element> | null = null;
    const measure = () => {
      const contentCell = wrapper.querySelector<HTMLTableCellElement>(
        'td.content',
      );
      if (!contentCell || contentCell.offsetLeft <= 0) return;
      // `--measured-gutter` = the OUTER (rightmost) gutter rule's x.
      // For unified mode that's just the `.content` cell's left
      // edge; for SBS it's the LEFT side's `.content`. Raw offset
      // (not rounded) so it aligns with the table's `.ln` border-
      // right at the same sub-pixel offset.
      wrapper.style.setProperty(
        '--measured-gutter',
        `${contentCell.offsetLeft}px`,
      );

      // Second gutter rule, when present:
      //   - Unified-both mode (default): two `.ln` cells per row, so
      //     there's an INNER gutter between them at the first `.ln`'s
      //     right edge. Captured as the FIRST row's first `.ln` cell
      //     offsetLeft + offsetWidth.
      //   - SBS mode: the tip side has its own gutter. Captured as
      //     the first `.content` cell living inside `.sbs-side.tip`.
      // Whichever applies, it lands in `--measured-gutter-2`. The
      // two modes are mutually exclusive (SBS doesn't render two
      // unified `.ln` columns), so one variable handles both.
      //
      // For SBS we use `getBoundingClientRect` minus the wrapper's
      // rect rather than `offsetLeft`: a `<td>`'s offsetParent is
      // its containing `<table>` (table-display elements form their
      // own offset chain), so `offsetLeft` for the tip-side td would
      // come back as the tip table's own local gutter offset
      // (~65 px) — useless for absolute positioning inside the
      // wrapper. The wrapper-rect math gives the real viewport-
      // coord position to which the composer overlay is anchored.
      let secondGutter: number | null = null;
      const tipContent = wrapper.querySelector<HTMLTableCellElement>(
        '.sbs-side.tip td.content',
      );
      if (tipContent) {
        const tipRect = tipContent.getBoundingClientRect();
        const wrapperRect = wrapper.getBoundingClientRect();
        const relLeft = tipRect.left - wrapperRect.left;
        if (relLeft > 0) secondGutter = relLeft;
      }
      if (secondGutter === null) {
        // Unified-both: two `.ln` cells in the first row. The first
        // one's right edge is the inner gutter. Same wrapper-rect
        // math for consistency.
        const firstRow = wrapper.querySelector<HTMLTableRowElement>('tr');
        const lns = firstRow?.querySelectorAll<HTMLTableCellElement>('td.ln');
        if (lns && lns.length >= 2) {
          const lnRect = lns[0].getBoundingClientRect();
          const wrapperRect = wrapper.getBoundingClientRect();
          const inner = lnRect.right - wrapperRect.left;
          if (inner > 0) secondGutter = inner;
        }
      }
      if (secondGutter !== null && secondGutter > 0) {
        wrapper.style.setProperty(
          '--measured-gutter-2',
          `${secondGutter}px`,
        );
      } else {
        wrapper.style.removeProperty('--measured-gutter-2');
      }
    };
    const ro = new ResizeObserver(measure);
    let observedSides: NodeListOf<Element> | null = null;
    const observeLnCells = () => {
      if (lnCells) for (const el of lnCells) ro.unobserve(el);
      lnCells = wrapper.querySelectorAll('td.ln');
      for (const el of lnCells) ro.observe(el);
      // Also observe `.sbs-side` so a divider drag — which resizes
      // the tip side without changing any `.ln` cell width — still
      // triggers a re-measure. Without this the tip gutter's x
      // coord (`--measured-gutter-2`) drifts away from the actual
      // tip column position as the user drags, and the composer
      // overlay stays anchored to the pre-drag position.
      if (observedSides) for (const el of observedSides) ro.unobserve(el);
      observedSides = wrapper.querySelectorAll('.sbs-side');
      for (const el of observedSides) ro.observe(el);
      measure();
    };
    observeLnCells();
    // Re-observe when hunks are added/removed (context expansion, etc.).
    const mo = new MutationObserver(observeLnCells);
    mo.observe(wrapper, { childList: true, subtree: true });
    return () => {
      ro.disconnect();
      mo.disconnect();
    };
  });

  /** Apply the range-selection highlight to rows within this file when a
   *  line-level composer is open here. Direct DOM so toggling `composing`
   *  doesn't re-evaluate every row in every hunk.
   *
   *  Skipped for column-range composers — those keep the user's native
   *  text selection alive (see `commentOnSelection`), which already
   *  shows the precise sub-line range the comment will anchor to.
   *  Tinting whole rows on top would make it look like the comment
   *  covers whole lines, hiding what the reviewer actually selected. */
  $effect(() => {
    composerPaint.clear();
    if (composing?.kind !== 'line' || composing.file !== file.path) return;
    if (composing.columns) return;
    composerPaint.paint(composing.side, composing.startLine, composing.endLine);
  });

  /** Bridge inter-hunk separators for every multi-hunk published
   *  comment on this file so the blue gutter stripe `.row.commented
   *  .content` paints stays continuous across the gap. Re-runs on
   *  comment changes (a freshly posted comment included) and on
   *  expansion / collapse toggles since the bridged separators
   *  appear and disappear with them. */
  $effect(() => {
    void expansions;
    void wholeFile;
    void collapsed;
    commentedPaint.clear();
    for (const c of comments) {
      if (c.file !== file.path) continue;
      if (c.anchor.kind === 'outdated') continue;
      if (!c.side) continue;
      const effective =
        c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
          ? c.anchor.new_lines
          : c.lines;
      // Single-line comments need no bridge — no separator can sit
      // inside one line. Column-range comments DO need it: their
      // tinted overlay rectangles already gap-fill (see
      // `rectsForRange`), but the row-level `.commented` blue
      // gutter stripe HunkLines paints on each row would read as
      // discontinuous across an inter-hunk separator without this.
      if (!effective || effective.start === effective.end) continue;
      commentedPaint.paint(c.side, effective.start, effective.end);
    }
  });

  /** Position the line-level composer overlay below the target row. The
   *  `composing-target` class on the row adds `padding-bottom` so the
   *  overlay has space to occupy without overlapping the next row. */
  $effect(() => {
    if (composerTargetEl) {
      composerTargetEl.classList.remove('composing-target');
      composerTargetEl = null;
    }
    composerTop = null;

    if (!composing || composing.kind !== 'line' || composing.file !== file.path) {
      return;
    }
    if (!sectionEl || !hunksWrapperEl) return;

    // Query against `hunksWrapperEl` — the same element the
    // SelectionPopup's resolver was bound to. Querying against
    // `sectionEl` could theoretically pick up rows from somewhere
    // else inside the section (file-level threads etc.) that share
    // the same data-line/data-side; staying inside the hunks
    // wrapper guarantees we get the row that actually backed the
    // selection.
    const target = hunksWrapperEl.querySelector(
      `[data-side="${composing.side}"][data-line="${composing.endLine}"]`,
    ) as HTMLElement | null;
    if (!target) return;

    target.classList.add('composing-target');
    composerTargetEl = target;

    requestAnimationFrame(() => {
      if (!composerTargetEl || !hunksWrapperEl) return;
      const tRect = composerTargetEl.getBoundingClientRect();
      const wRect = hunksWrapperEl.getBoundingClientRect();
      const padding =
        parseFloat(getComputedStyle(composerTargetEl).paddingBottom) || 0;
      composerTop = tRect.bottom - wRect.top - padding;
    });
  });

  /** Keep the target row's padding in sync with the composer's actual
   *  rendered height so the gap matches whatever the composer needs. */
  $effect(() => {
    if (!composerOverlayEl || !composerTargetEl) return;
    const ro = new ResizeObserver((entries) => {
      for (const e of entries) {
        if (composerTargetEl) {
          composerTargetEl.style.setProperty(
            '--composer-h',
            `${Math.ceil(e.contentRect.height) + 4}px`,
          );
        }
      }
    });
    ro.observe(composerOverlayEl);
    return () => ro.disconnect();
  });

  const fileComments = $derived(comments.filter((c) => c.file === file.path));
  const fileAnnotations = $derived(
    annotations.filter((n) => n.file === file.path),
  );
  /** Whole-file comments — those targeting this file with no line range. */
  const fileLevelComments = $derived(
    fileComments.filter((c) => c.lines == null),
  );
  /** Line-level comments on this file, sorted by line for compact-mode
   *  rendering. Each entry is one thread root; the inline hunk view
   *  groups these into row overlays instead. */
  const lineCommentsSorted = $derived(
    fileComments
      .filter((c) => c.lines != null)
      .slice()
      .sort((a, b) => {
        const al = a.lines?.start ?? 0;
        const bl = b.lines?.start ?? 0;
        if (al !== bl) return al - bl;
        return a.created_at.localeCompare(b.created_at);
      }),
  );

  /** One line of code shown above a comment in comments-only mode —
   *  enough to remind the reader what the thread is about without
   *  having to switch back to the diff view. */
  interface ContextLine {
    lineNum: number;
    origin: 'context' | 'added' | 'removed';
    text: string;
    /** Pre-tokenized HTML. `undefined` means render `text` as plain. */
    html?: string;
  }

  /** Code context for the line-level comment `c` to show above its
   *  thread in comments-only mode. The lines are pulled from the
   *  file's own hunks — comments anchored in the rendered region
   *  share their syntax highlight with the inline diff. */
  function contextLinesFor(c: CommentView): { lines: ContextLine[]; note?: string } {
    // Outdated anchors: the lines on the current patchset no longer
    // match what the comment was about; render the frozen original
    // text the backend captured at write time.
    if (c.anchor.kind === 'outdated') {
      const lines = c.anchor.original_content
        .split('\n')
        .filter((l, i, all) => i < all.length - 1 || l.length > 0)
        .map((text, i) => ({
          lineNum: (c.lines?.start ?? 0) + i,
          origin: 'context' as const,
          text,
        }));
      return { lines, note: 'outdated — original lines shown' };
    }
    const effective =
      c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
        ? c.anchor.new_lines
        : c.lines;
    if (!effective || !c.side) return { lines: [] };
    const side = c.side;
    const lines: ContextLine[] = [];
    for (const h of file.hunks ?? []) {
      for (const ln of h.lines) {
        const num = side === 'tip' ? ln.tip_line : ln.base_line;
        if (num == null) continue;
        if (num < effective.start || num > effective.end) continue;
        lines.push({
          lineNum: num,
          origin: ln.origin,
          text: ln.content.replace(/\n$/, ''),
          html:
            side === 'tip'
              ? highlightsTip.get(num)
              : highlightsBase.get(num),
        });
      }
    }
    lines.sort((a, b) => a.lineNum - b.lineNum);
    const note = c.anchor.kind === 'drifted' ? 'lines have drifted' : undefined;
    return { lines, note };
  }

  /** Every (side, line) the file's hunks actually render. Inline
   *  comment threads attach next to a matching row, so a comment
   *  anchored to a line that fell outside the diff's surrounding
   *  context becomes invisible — visible only in comments-only mode
   *  (which lists every line comment irrespective of hunk
   *  coverage). We surface those orphan threads explicitly below. */
  const renderedLineKeys = $derived.by(() => {
    const set = new Set<string>();
    for (const h of file.hunks ?? []) {
      for (const ln of h.lines) {
        if (ln.base_line != null) set.add(`base:${ln.base_line}`);
        if (ln.tip_line != null) set.add(`tip:${ln.tip_line}`);
      }
    }
    return set;
  });

  /** Line comments whose anchor line isn't in any rendered hunk row.
   *  Render them at the file level so the inline-diff view doesn't
   *  silently drop them. */
  const orphanLineComments = $derived(
    lineCommentsSorted.filter((c) => {
      if (!c.side || !c.lines) return false;
      // Hunks may not be loaded yet — the open_review payload ships
      // file metadata only and hunks stream in per FileSlot. While
      // we wait, don't classify everything as orphan.
      if (!file.hunks) return false;
      const effective =
        c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
          ? c.anchor.new_lines
          : c.lines;
      return !renderedLineKeys.has(`${c.side}:${effective.end}`);
    }),
  );

  /** Anchor a file-level comment to the tip side of the viewed patchset. */
  const fileAnchorIds = $derived({
    change: patchset.tip_change,
    commit: patchset.tip_commit,
  });

  /** Anchor ids for line-level composers, picked based on the side the
   *  user clicked. */
  const lineAnchorIds = $derived.by(() => {
    if (composing?.kind === 'line') {
      return composing.side === 'tip'
        ? { change: patchset.tip_change, commit: patchset.tip_commit }
        : { change: patchset.base_change, commit: patchset.base_commit };
    }
    return fileAnchorIds;
  });

  /** Anchor ids for the annotation composer. Same picking logic as
   *  `lineAnchorIds` — file-wide annotations fall back to the tip
   *  side's anchor by default. */
  const annotationAnchorIds = $derived.by(() => {
    if (composingAnnotation?.kind === 'line') {
      return composingAnnotation.side === 'tip'
        ? { change: patchset.tip_change, commit: patchset.tip_commit }
        : { change: patchset.base_change, commit: patchset.base_commit };
    }
    return { change: patchset.tip_change, commit: patchset.tip_commit };
  });

  /** Reactive narrow-viewport flag. Updated via the media-query listener
   *  in the effect below; used to fall back to unified diff on phones
   *  (side-by-side is unreadable at <640px). */
  let narrowViewport = $state(false);
  $effect(() => {
    if (typeof window === 'undefined') return;
    const mq = window.matchMedia('(max-width: 640px)');
    narrowViewport = mq.matches;
    const update = (e: MediaQueryListEvent) => (narrowViewport = e.matches);
    mq.addEventListener('change', update);
    return () => mq.removeEventListener('change', update);
  });

  /** Side-by-side for modifications + renames on a wide enough screen;
   *  unified everywhere else. Unified is also easier to read when only
   *  one side of content exists (pure add/delete). */
  const sideBySide = $derived(
    !narrowViewport && (file.status === 'modified' || file.status === 'renamed'),
  );

  /** Files where it makes sense to expand context — both sides exist. */
  const canExpand = $derived(
    file.status === 'modified' || file.status === 'renamed',
  );

  /** Which line-number columns to show:
   *  - added file: only tip lines exist
   *  - deleted file: only base lines exist
   *  - modified / renamed: both */
  const lineNumberMode: 'both' | 'base' | 'tip' = $derived(
    file.status === 'added' ? 'tip' : file.status === 'deleted' ? 'base' : 'both',
  );

  /** Fallback distance from the file-diff's left edge to where the
   *  content cell of a diff row starts, in pixels. The runtime measures
   *  the actual offset and publishes it as `--measured-gutter`; this
   *  hardcoded value is what we use before the first measurement and
   *  if measurement somehow fails. One `.ln` cell is 48 (declared
   *  width) + 16 (8 px padding each side) + 1 (right border) = 65 px;
   *  side-by-side has one per half, unified-both has two. */
  const gutterIndentPx = $derived(
    sideBySide || lineNumberMode !== 'both' ? 65 : 130,
  );

  // ---- hunk context expansion -------------------------------------------

  const STEP = 20;
  type Expansion = { above: number; below: number };
  let expansions: Map<number, Expansion> = $state(new Map());
  /** Tip-side file text split into lines (1-based via index+1). Populated by
   *  the tokenize effect when the tip side exists. */
  let tipLines: string[] | null = $state.raw(null);

  function expansionFor(i: number): Expansion {
    return expansions.get(i) ?? { above: 0, below: 0 };
  }

  /** Clipped per-hunk expansions, accounting for both file bounds AND
   *  adjacent-hunk claims on the same gap. Computed in one pass over
   *  all hunks because the second hunk's `above` cap depends on what
   *  the first hunk's `below` has already consumed of the shared gap
   *  (and vice versa).
   *
   *  Without this coordination, clicking "expand below" on hunk A and
   *  "expand above" on hunk B (gap = 1) would each pull in the
   *  one-line gap independently — the same source line appearing in
   *  both rendered hunks. Allocation order: hunk A's `below` is
   *  applied first; whatever's left of the gap is available to hunk
   *  B's `above`. Symmetric for the tip-side and base-side caps. */
  const clippedExpansions = $derived.by<Map<number, Expansion>>(() => {
    const out = new Map<number, Expansion>();
    const hunks = file.hunks ?? [];
    if (!tipLines) return out;
    // Track the next allowed first-line on each side, per neighbour
    // gap. Starts at the previous hunk's end+1; gets pushed forward
    // as we allocate context into the gap.
    let prevTipFloor = 1;
    let prevBaseFloor: number | null = 1;
    for (let i = 0; i < hunks.length; i++) {
      const h = hunks[i];
      const exp = wholeFile ? wholeFileExpansion(i) : expansionFor(i);
      const tipStart = h.tip_range?.start;
      const tipEnd = h.tip_range?.end;
      const baseStart = h.base_range?.start;
      const baseEnd = h.base_range?.end;
      if (tipStart == null || tipEnd == null) {
        out.set(i, { above: 0, below: 0 });
        continue;
      }
      // `above`: cap by current floor (= previous hunk's end+1).
      const aboveCapTip = Math.max(0, tipStart - prevTipFloor);
      const aboveCapBase =
        baseStart != null && prevBaseFloor != null
          ? Math.max(0, baseStart - prevBaseFloor)
          : Infinity;
      const above = Math.min(exp.above, aboveCapTip, aboveCapBase);

      // `below`: cap so the next hunk's `above` (and the file end)
      // still have room. We allocate `above` to this hunk's gap from
      // the previous neighbour, leaving the rest of the gap empty.
      // Then we look ahead to find the next neighbour and split the
      // *downstream* gap between this hunk's `below` and the next
      // hunk's `above`. First come first serve: `below` gets first
      // claim, the next hunk's `above` is capped against what's left
      // in the gap.
      const next = i < hunks.length - 1 ? hunks[i + 1] : null;
      const nextTipStart = next?.tip_range?.start ?? tipLines.length + 1;
      const nextBaseStart = next?.base_range?.start;
      const belowCapTip = Math.max(0, nextTipStart - 1 - tipEnd);
      const belowCapBase =
        baseEnd != null && nextBaseStart != null
          ? Math.max(0, nextBaseStart - 1 - baseEnd)
          : Infinity;
      const below = Math.min(exp.below, belowCapTip, belowCapBase);

      out.set(i, { above, below });

      // Push floors past this hunk + its `below` so the next hunk's
      // `above` cap is shrunk accordingly.
      prevTipFloor = tipEnd + below + 1;
      prevBaseFloor =
        baseEnd != null ? baseEnd + below + 1 : prevBaseFloor;
    }
    return out;
  });

  /** Toggle whole-file mode while keeping the user anchored to whatever
   *  they were already looking at:
   *
   *  - If a line of this file is currently in the viewport, capture its
   *    (side, line) and screen-Y; after the toggle, scroll so the same
   *    line lands at the same Y. (When collapsing to diff-only, the
   *    line may have been synthetic context that no longer exists —
   *    fall back to the closest surviving line on the same side.)
   *  - If the file is entirely above the viewport, every later file
   *    just shifted by the file's height delta; scroll to undo that so
   *    the user's visible content stays put.
   *  - If the file is entirely below the viewport, no adjustment — the
   *    growth is below where the user is looking. */
  async function toggleWholeFile() {
    if (!sectionEl) {
      wholeFile = !wholeFile;
      return;
    }
    const beforeRect = sectionEl.getBoundingClientRect();

    let anchorSide: string | null = null;
    let anchorLine: string | null = null;
    let anchorY = 0;
    const rows = sectionEl.querySelectorAll<HTMLElement>('[data-side][data-line]');
    for (const el of rows) {
      const rect = el.getBoundingClientRect();
      if (rect.bottom > 0 && rect.top < window.innerHeight) {
        anchorSide = el.dataset.side ?? null;
        anchorLine = el.dataset.line ?? null;
        anchorY = rect.top;
        break;
      }
    }

    wholeFile = !wholeFile;
    await tick();
    if (!sectionEl) return;

    if (anchorSide != null && anchorLine != null) {
      let target = sectionEl.querySelector<HTMLElement>(
        `[data-side="${anchorSide}"][data-line="${anchorLine}"]`,
      );
      if (!target) {
        const wantLine = parseInt(anchorLine, 10);
        const survivors = sectionEl.querySelectorAll<HTMLElement>(
          `[data-side="${anchorSide}"][data-line]`,
        );
        let best: HTMLElement | null = null;
        let bestDelta = Infinity;
        for (const el of survivors) {
          const ln = parseInt(el.dataset.line ?? '', 10);
          if (!Number.isFinite(ln)) continue;
          const delta = Math.abs(ln - wantLine);
          if (delta < bestDelta) {
            bestDelta = delta;
            best = el;
          }
        }
        target = best;
      }
      if (target) {
        const newY = target.getBoundingClientRect().top;
        const delta = newY - anchorY;
        if (delta !== 0) window.scrollBy(0, delta);
      }
    } else if (beforeRect.bottom <= 0) {
      const afterRect = sectionEl.getBoundingClientRect();
      const heightDelta = afterRect.height - beforeRect.height;
      if (heightDelta !== 0) window.scrollBy(0, heightDelta);
    }
  }

  /** Per-hunk expansion when `wholeFile` is on: expand every hunk to
   *  the edges of the file and fill the gaps between adjacent hunks
   *  with the surrounding code. Each gap is attributed to the
   *  preceding hunk's `below` so we don't double-fill from both sides.
   *  The first hunk's `above` reaches line 1; the last hunk's `below`
   *  reaches EOF. */
  function wholeFileExpansion(i: number): Expansion {
    const hunks = file.hunks ?? [];
    const cur = hunks[i];
    if (!cur?.tip_range || !tipLines) return { above: 0, below: 0 };
    const above = i === 0 ? cur.tip_range.start - 1 : 0;
    const next = i < hunks.length - 1 ? hunks[i + 1] : null;
    const nextStart = next?.tip_range?.start ?? tipLines.length + 1;
    const below = Math.max(0, nextStart - 1 - cur.tip_range.end);
    return { above, below };
  }
  /** True when at least one source line is still skipped between
   *  hunk `i`'s expanded end and hunk `i+1`'s expanded start. */
  function hasGapAfter(i: number): boolean {
    return gapAfter(i) > 0;
  }

  /** Size (in source lines) of the unrendered gap between hunk `i`
   *  and hunk `i+1`, taking the larger of the base / tip side gaps
   *  so we don't under-report on renames. Returns 0 when the hunks
   *  already meet end-to-end (or when one side is missing). */
  function gapAfter(i: number): number {
    const hunks = file.hunks ?? [];
    const cur = hunks[i];
    const next = hunks[i + 1];
    if (!cur?.tip_range || !next?.tip_range) return 0;
    const curExp = clippedExpansions.get(i) ?? { above: 0, below: 0 };
    const nextExp = clippedExpansions.get(i + 1) ?? { above: 0, below: 0 };
    let gap = Math.max(
      0,
      next.tip_range.start - nextExp.above - (cur.tip_range.end + curExp.below) - 1,
    );
    if (cur.base_range && next.base_range) {
      const baseGap =
        next.base_range.start -
        nextExp.above -
        (cur.base_range.end + curExp.below) -
        1;
      gap = Math.max(gap, baseGap);
    }
    return gap;
  }

  /** When the gap is small enough to fill with one click, collapse
   *  the three-row "expand-below / … / expand-above" UI into a
   *  single combined button. STEP is the chunk size each
   *  directional expand uses; a gap at or below that is fully
   *  cleared by one expansion in either direction. */
  function fillableGapAfter(i: number): boolean {
    const g = gapAfter(i);
    return g > 0 && g <= STEP;
  }

  /** Expand the entire (small) gap between hunks `i` and `i+1` in
   *  one click. Adding the gap size to `i.below` is enough — the
   *  clipping in `clippedExpansions` keeps the next hunk from
   *  double-claiming any line. */
  function expandGapAfter(i: number) {
    const g = gapAfter(i);
    if (g > 0) expand(i, 'below', g);
  }

  function expand(i: number, direction: 'above' | 'below', amount: number) {
    if (!tipLines) return;
    const next = new Map(expansions);
    const cur = expansionFor(i);
    next.set(i, { ...cur, [direction]: cur[direction] + amount });
    expansions = next;
  }

  /** Apply the user's expansion settings to a hunk, producing a hunk
   *  with extra context lines prepended/appended. The clipping that
   *  prevents adjacent hunks from rendering the same source line is
   *  done globally in `clippedExpansions` (above) so the second hunk
   *  in a pair sees the first hunk's already-claimed expansion. */
  function withContext(hunk: Hunk, i: number): Hunk {
    const exp = clippedExpansions.get(i) ?? { above: 0, below: 0 };
    if ((exp.above === 0 && exp.below === 0) || !tipLines) return hunk;

    const baseStart = hunk.base_range?.start;
    const tipStart = hunk.tip_range?.start;
    const baseEnd = hunk.base_range?.end;
    const tipEnd = hunk.tip_range?.end;
    if (tipStart == null || tipEnd == null) return hunk;

    const above = exp.above;
    const below = exp.below;

    const before: HunkLine[] = [];
    for (let k = above; k > 0; k--) {
      const tipLn = tipStart - k;
      if (tipLn < 1) continue;
      const content = tipLines[tipLn - 1];
      if (content === undefined) continue;
      before.push({
        origin: 'context',
        base_line: baseStart != null ? baseStart - k : undefined,
        tip_line: tipLn,
        content: content + '\n',
      });
    }

    const after: HunkLine[] = [];
    for (let k = 1; k <= below; k++) {
      const tipLn = tipEnd + k;
      if (tipLn > tipLines.length) break;
      const content = tipLines[tipLn - 1];
      if (content === undefined) break;
      after.push({
        origin: 'context',
        base_line: baseEnd != null ? baseEnd + k : undefined,
        tip_line: tipLn,
        content: content + '\n',
      });
    }

    return {
      base_range:
        hunk.base_range && baseStart != null && baseEnd != null
          ? { start: baseStart - before.length, end: baseEnd + after.length }
          : hunk.base_range,
      tip_range: {
        start: tipStart - before.length,
        end: tipEnd + after.length,
      },
      lines: [...before, ...hunk.lines, ...after],
    };
  }

  /** `eh` is the with-context hunk; we use ITS tip/base range
   *  (already extended by the user's clipped expansions) to decide
   *  whether more room remains. The neighbour's expanded range is
   *  encoded in `clippedExpansions[i-1].below` / `[i+1].above`. */
  function canExpandAbove(eh: Hunk, i: number): boolean {
    if (!eh.tip_range) return false;
    if (eh.tip_range.start <= 1) return false;
    const hunks = file.hunks ?? [];
    const prev = i > 0 ? hunks[i - 1] : null;
    const prevExp = clippedExpansions.get(i - 1) ?? { above: 0, below: 0 };
    if (prev?.tip_range?.end != null) {
      const prevExpandedEnd = prev.tip_range.end + prevExp.below;
      if (eh.tip_range.start <= prevExpandedEnd + 1) return false;
    }
    if (eh.base_range && prev?.base_range?.end != null) {
      const prevExpandedBaseEnd = prev.base_range.end + prevExp.below;
      if (eh.base_range.start <= prevExpandedBaseEnd + 1) return false;
    }
    return true;
  }
  function canExpandBelow(eh: Hunk, i: number): boolean {
    if (!eh.tip_range) return false;
    if (tipLines != null && eh.tip_range.end >= tipLines.length) return false;
    const hunks = file.hunks ?? [];
    const next = i < hunks.length - 1 ? hunks[i + 1] : null;
    const nextExp = clippedExpansions.get(i + 1) ?? { above: 0, below: 0 };
    if (next?.tip_range?.start != null) {
      const nextExpandedStart = next.tip_range.start - nextExp.above;
      if (eh.tip_range.end >= nextExpandedStart - 1) return false;
    }
    if (eh.base_range && next?.base_range?.start != null) {
      const nextExpandedBaseStart = next.base_range.start - nextExp.above;
      if (eh.base_range.end >= nextExpandedBaseStart - 1) return false;
    }
    return true;
  }

  // Highlights are per-file, indexed by (side, 1-based line number). We
  // tokenize each whole side in one pass so multi-line constructs (block
  // comments, template literals, heredocs) keep grammar state across lines.
  // SvelteMap so `.get()` is reactive per key — rows light up as their
  // line becomes available without re-running the whole row's reactivity.
  let highlightsBase: LineHighlights = $state(new SvelteMap());
  let highlightsTip: LineHighlights = $state(new SvelteMap());
  const highlights = $derived({ base: highlightsBase, tip: highlightsTip });

  const baseSideExists = $derived(
    file.status !== 'added' && !file.binary && file.hunks != null,
  );
  const tipSideExists = $derived(
    file.status !== 'deleted' && !file.binary && file.hunks != null,
  );

  /** Path on the base side. Renames moved the file, so base lives at
   *  `old_path`. Falls back to `path` for non-renames. */
  const basePath = $derived(file.old_path ?? file.path);

  /** Tokenization-relevant inputs, factored out as primitive-valued
   *  $derived's so the effect below only re-runs when the *content* we'd
   *  fetch actually changes — not whenever `file` is replaced with a new
   *  object that still points at the same path/status (e.g. when toggling
   *  the commit-scoped view). Avoids the highlight flash on every click. */
  const tipPath = $derived(file.path);
  // In compare mode the diff base is the *other* patchset's tip, not
  // `patchset.base_commit`. Use it for the highlight pass so
  // `highlightsBase` is indexed by line numbers from the same file the
  // hunks' `base_line` values reference; otherwise the renderer pulls
  // HTML from the wrong file and removed-side rows display unrelated
  // content (e.g. a `{` where the diff hunk says `/**`).
  const baseCommit = $derived(compareBaseCommit ?? patchset.base_commit);
  const tipCommit = $derived(patchset.tip_commit);
  const fileLang = $derived(langForPath(file.path));

  /** Load each side's full text, then tokenize each in one pass. Reading
   *  themeState.value here makes the effect re-run on OS theme toggle so
   *  highlights re-color without a reload.
   *
   *  No viewport gate here — `FileSlot` only mounts us when we're near
   *  the viewport, so reaching this effect already implies "worth
   *  tokenizing."
   *
   *  Tip text loading is independent of the language: we want `tipLines`
   *  populated even for files with unknown extensions so context
   *  expansion (and the "Whole file" toggle) still works for them.
   *  Highlighting is the part that needs a recognized language. */
  $effect(() => {
    void themeState.value;
    // Pin the primitive deps so this effect re-runs only on real changes.
    const lang = fileLang;
    const wantBase = baseSideExists;
    const wantTip = tipSideExists;
    const bPath = basePath;
    const tPath = tipPath;
    const bCommit = baseCommit;
    const tCommit = tipCommit;
    let cancelled = false;
    const isCancelled = () => cancelled;
    // Reset on re-run (theme toggle, etc.) so we don't see stale colors.
    highlightsBase = new SvelteMap();
    highlightsTip = new SvelteMap();

    (async () => {
      const [baseText, tipText, h] = await Promise.all([
        wantBase
          ? api.readFile(repo, bCommit, bPath).catch(() => null)
          : Promise.resolve(null),
        wantTip
          ? api.readFile(repo, tCommit, tPath).catch(() => null)
          : Promise.resolve(null),
        lang != null ? loadLang(lang) : Promise.resolve(null),
      ]);
      if (cancelled) return;

      if (tipText != null) {
        const lines = tipText.split('\n');
        if (lines.length > 0 && lines[lines.length - 1] === '') lines.pop();
        tipLines = lines;
      }

      if (lang != null && h != null) {
        await Promise.all([
          baseText != null
            ? tokenizeWholeFile(h, baseText, lang, highlightsBase, { isCancelled })
            : Promise.resolve(),
          tipText != null
            ? tokenizeWholeFile(h, tipText, lang, highlightsTip, { isCancelled })
            : Promise.resolve(),
        ]);
      }
    })();
    return () => {
      cancelled = true;
    };
  });
</script>

<section
  bind:this={sectionEl}
  class="file-diff"
  data-file-path={file.path}
>
  <header class="file-header">
    {#if showDiffs}
      <!-- The fold toggle only makes sense when the diff is actually
           on screen — in comments-only mode there's nothing under
           the header to hide or reveal. -->
      <button
        class="toggle"
        aria-label={collapsed ? 'expand' : 'collapse'}
        onclick={() => (collapsed = !collapsed)}
        data-tour="file-fold"
      >
        {collapsed ? '▸' : '▾'}
      </button>
    {/if}
    <span class="status status-{file.status}">{file.status[0].toUpperCase()}</span>
    <!-- The path wrapper is direction:rtl so `text-overflow: ellipsis`
         falls on the left ("…/short/tail.rs") instead of cutting off the
         filename. <bdi> keeps the path itself rendered LTR. -->
    <span class="path">
      <bdi>
        {#if file.status === 'renamed' && file.old_path}
          <span class="muted">{file.old_path} →</span> {file.path}
        {:else}
          {file.path}
        {/if}
      </bdi>
    </span>
    {#if file.binary}
      <span class="meta">binary</span>
    {/if}
    {#if showDiffs && canExpand && !collapsed}
      <!-- Toggle between the default hunks-with-context view and a
           continuous "whole file" view. Only meaningful when both
           sides exist (modified/renamed), the diff is actually
           rendered (not in comments-only mode, not collapsed), and
           the full tip text has been loaded.
           The icon switches state with the toggle: outward-pointing
           arrows when diff-only (click to expand) and inward-pointing
           arrows when whole-file (click to collapse). -->
      <button
        type="button"
        class="whole-file"
        class:on={wholeFile}
        aria-label={wholeFile ? 'Collapse to diff hunks' : 'Expand the whole file'}
        aria-pressed={wholeFile}
        title={wholeFile ? 'Collapse to diff hunks' : 'Expand the whole file'}
        disabled={tipLines == null}
        onclick={toggleWholeFile}
      >
        <svg
          width="14"
          height="14"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          stroke-width="1.25"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
          focusable="false"
        >
          {#if wholeFile}
            <!-- Collapse: two boundary lines with the stems running from
                 each line to the chevron's apex inside, so the arrows
                 point inward and visually fold shut. Chevrons use a
                 narrower base and taller rise so each head reads as a
                 sharp arrow rather than a shallow triangle, and the
                 apex points stay a few px apart so the two heads
                 don't look fused. -->
            <line x1="2" y1="1" x2="14" y2="1" />
            <line x1="8" y1="1" x2="8" y2="6" />
            <path d="M6 3 L8 6 L10 3" />
            <path d="M6 13 L8 10 L10 13" />
            <line x1="8" y1="10" x2="8" y2="15" />
            <line x1="2" y1="15" x2="14" y2="15" />
          {:else}
            <!-- Expand: a spine line in the middle with stems running
                 from it outward to the chevron apexes, so the arrows
                 point outward. Chevrons match the collapse-icon
                 geometry — a narrow base with the apex pushed to the
                 viewBox edge — and the stems stop short of the spine
                 so the arrows aren't fused to it. -->
            <path d="M6 4 L8 1 L10 4" />
            <line x1="8" y1="1" x2="8" y2="6" />
            <line x1="2" y1="8" x2="14" y2="8" />
            <line x1="8" y1="10" x2="8" y2="15" />
            <path d="M6 12 L8 15 L10 12" />
          {/if}
        </svg>
      </button>
    {/if}
    {#if showComments && commentsWriteable}
      <button
        type="button"
        class="file-comment"
        aria-label="Comment on this file"
        title="Comment on this file"
        onclick={() => onstartcompose({ kind: 'file', file: file.path })}
      >
        <Bubble size={14} />
      </button>
    {/if}
    {#if debug}
      <!-- Debug affordance, only visible with `?debug` in the URL.
           Toggles an inline panel that drops down inside the header
           (flex-wrap onto a new row) so it stays attached to the
           sticky header as the file content scrolls. -->
      <button
        type="button"
        class="debug-cmd"
        aria-label={debugOpen ? 'Hide jj command' : 'Show jj command'}
        aria-pressed={debugOpen}
        title="Show / copy the jj equivalent command for this view"
        onclick={() => (debugOpen = !debugOpen)}
      >
        <!-- Bug glyph (stroke-only to match the other header icons).
             Oval body + center seam, two antennae poking up, three
             legs each side. -->
        <svg
          width="14"
          height="14"
          viewBox="0 0 16 16"
          fill="none"
          stroke="currentColor"
          stroke-width="1.25"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
          focusable="false"
        >
          <path d="M5 2 L6.5 4" />
          <path d="M11 2 L9.5 4" />
          <rect x="4.5" y="4" width="7" height="9" rx="3" />
          <line x1="8" y1="4" x2="8" y2="13" />
          <line x1="4.5" y1="7" x2="2" y2="6" />
          <line x1="4.5" y1="9" x2="1.5" y2="9.5" />
          <line x1="4.5" y1="11" x2="2" y2="12.5" />
          <line x1="11.5" y1="7" x2="14" y2="6" />
          <line x1="11.5" y1="9" x2="14.5" y2="9.5" />
          <line x1="11.5" y1="11" x2="14" y2="12.5" />
        </svg>
      </button>
    {/if}
    {#if debug && debugOpen}
      <div class="debug-panel">
        <code>{jjCommand}</code>
        <button
          type="button"
          class="debug-copy"
          title="Copy to clipboard"
          onclick={() => void copyText(jjCommand)}
        >Copy</button>
        <p class="debug-note muted">
          Literal commit-to-commit diff. In per-commit compare mode
          (Changed pairs) the UI computes a rebase-based interdiff via
          jj-lib, so the literal command's output will differ.
        </p>
      </div>
    {/if}
  </header>

  {#if showComments && fileLevelComments.length > 0}
    <div class="file-thread">
      <CommentThread
        comments={fileLevelComments}
        {responses}
        {saving}
        {currentPatchset}
        {editingCommentId}
        {lastVisitAt}
        {viewer}
        {defaultThreadsCollapsed}
        showFold
        {onreply}
        {onstatus}
        {ondelete}
        {onedit}
        {onselectpatchset}
      />
    </div>
  {/if}

  <!-- Orphan line comments: anchored to a line that the diff's
       surrounding context didn't include, so the inline hunk view
       has no row to attach them to. Render at the file level so
       they're not silently dropped in show-diffs mode. Suppressed
       in comments-only mode because the compact-line-list below
       already shows every line comment irrespective of hunk
       coverage; suppressed in diffs-only mode because comments
       are intentionally hidden there. -->
  {#if showDiffs && showComments && orphanLineComments.length > 0}
    {@const orphanFolded = orphanSectionAllFolded()}
    <div
      class="orphan-threads"
      style:margin-left="var(--measured-gutter, {gutterIndentPx}px)"
    >
      <header class="orphan-header">
        <!-- Section-level fold for the whole orphan group. Acts as
             the gutter marker would for an in-diff line: one click
             collapses every orphan to nothing, another expands them
             back. Per-comment chevrons inside still work when the
             section has more than one orphan. -->
        <button
          type="button"
          class="orphan-toggle"
          aria-expanded={!orphanFolded}
          aria-label="{orphanFolded ? 'Expand' : 'Fold'} the orphan-comments section"
          title="{orphanFolded ? 'Expand' : 'Fold'} this group"
          onclick={toggleOrphanSection}
        ><Chevron dir={orphanFolded ? 'right' : 'down'} size={14} filled /></button>
        <p class="muted">
          Anchored outside the diff's context — the lines these comments
          attached to aren't part of the visible hunks.
        </p>
      </header>
      {#if !orphanFolded}
        <CommentThread
          comments={orphanLineComments}
          {responses}
          {saving}
          {currentPatchset}
          {editingCommentId}
          {lastVisitAt}
          {viewer}
          {defaultThreadsCollapsed}
          showFold
          {onreply}
          {onstatus}
          {ondelete}
          {onedit}
          {onselectpatchset}
        />
      {/if}
    </div>
  {/if}

  {#if showComments && composing && composing.kind === 'file' && composing.file === file.path}
    <div class="file-composer">
      <CommentComposer
        target={composing}
        anchorIds={fileAnchorIds}
        {saving}
        oncancel={oncancelcompose}
        onsubmit={onsubmit}
      />
    </div>
  {/if}

  {#if !showDiffs}
    {#if lineCommentsSorted.length > 0}
      <ul class="compact-line-list">
        {#each lineCommentsSorted as c (c.comment_id)}
          {@const ctx = contextLinesFor(c)}
          <li>
            <div class="compact-context">
              <header class="compact-context-head">
                <code
                  >L{c.lines?.start}{c.lines && c.lines.end !== c.lines.start
                    ? `–${c.lines.end}`
                    : ''}</code
                >
                <span class="muted">{c.side ?? ''}</span>
                {#if ctx.note}<span class="muted">· {ctx.note}</span>{/if}
              </header>
              {#if ctx.lines.length > 0}
                <div class="compact-context-body">
                  {#each ctx.lines as line (line.lineNum)}
                    <div class="compact-context-row origin-{line.origin}">
                      <span class="ln">{line.lineNum}</span>
                      <pre
                        class="content">{#if line.html}{@html line.html}{:else}{line.text || ' '}{/if}</pre>
                    </div>
                  {/each}
                </div>
              {/if}
            </div>
            <CommentThread
              comments={[c]}
              {responses}
              {saving}
              {currentPatchset}
              {editingCommentId}
              {lastVisitAt}
              {viewer}
              {defaultThreadsCollapsed}
              showFold
              {onreply}
              {onstatus}
              {ondelete}
              {onedit}
              {onselectpatchset}
            />
          </li>
        {/each}
      </ul>
    {:else if fileLevelComments.length === 0}
      <p class="placeholder muted">No comments on this file.</p>
    {/if}
  {:else if !collapsed}
    {#if file.binary}
      <p class="placeholder">Binary file — diff is not shown.</p>
    {:else if loadingHunks}
      <p class="placeholder muted">Loading diff…</p>
    {:else if !file.hunks}
      <p class="placeholder">Diff omitted (file may exceed the size limit).</p>
    {:else}
      <div
        class="hunks-wrapper"
        class:hide-gutter-affordances={hideGutterAffordances}
        bind:this={hunksWrapperEl}
      >
        {#each selectionOverlays as o}
          <div
            class="selection-overlay"
            style:top="{o.top}px"
            style:left="{o.left}px"
            style:width="{o.width}px"
            style:height="{o.height}px"
          ></div>
        {/each}
        <div class="hunks" bind:this={hunksEl}>
        {#each file.hunks as _, i (i)}
          {@const eh = withContext(file.hunks[i], i)}
          <!-- When the gap above us is small enough to be filled by a
               single click, the previous hunk renders a combined
               "expand entire gap" row in place of its own expand-
               below. Skip our expand-above here so we don't stack
               two affordances for the same gap. -->
          {@const combinedAbove = i > 0 && fillableGapAfter(i - 1)}
          {@const combinedBelow =
            i < file.hunks.length - 1 && fillableGapAfter(i)}
          {#if canExpand && !wholeFile && !combinedAbove && canExpandAbove(eh, i)}
            <div class="expand-row above">
              <button
                onclick={() => expand(i, 'above', STEP)}
                disabled={tipLines == null}
                aria-label="Show {STEP} lines above"
                title="Show {STEP} lines above"
              >
                <!-- "Show above": arrow points up to indicate where the
                     new context will appear, with a horizontal line
                     beneath marking the hunk's current top. -->
                <svg
                  width="14"
                  height="14"
                  viewBox="0 0 16 16"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.25"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  aria-hidden="true"
                  focusable="false"
                >
                  <path d="M6 4 L8 1 L10 4" />
                  <line x1="8" y1="1" x2="8" y2="13" />
                  <line x1="2" y1="13" x2="14" y2="13" />
                </svg>
              </button>
            </div>
          {/if}
          {#if sideBySide}
            <HunkLinesSideBySide
              hunk={eh}
              filePath={file.path}
              comments={fileComments}
              annotations={fileAnnotations}
              {composingAnnotation}
              {annotationAnchorIds}
              {canAnnotate}
              {oncancelannotate}
              {onsubmitannotation}
              {ondeleteannotation}
              {oneditannotation}
              {defaultThreadsCollapsed}
              {responses}
              {currentPatchset}
              {composing}
              {saving}
              {highlights}
              {lastVisitAt}
              {viewer}
              {showComments}
              {commentsWriteable}
              {sbsSplit}
              {setSbsSplit}
              {onstartcompose}
              {onreply}
              {onstatus}
              {ondelete}
              {onedit}
              {onselectpatchset}
            />
          {:else}
            <HunkLines
              hunk={eh}
              filePath={file.path}
              comments={fileComments}
              annotations={fileAnnotations}
              {composingAnnotation}
              {annotationAnchorIds}
              {canAnnotate}
              {oncancelannotate}
              {onsubmitannotation}
              {ondeleteannotation}
              {oneditannotation}
              {defaultThreadsCollapsed}
              {responses}
              {currentPatchset}
              {composing}
              {saving}
              {highlights}
              {lineNumberMode}
              {lastVisitAt}
              {viewer}
              {showComments}
              {commentsWriteable}
              {onstartcompose}
              {onreply}
              {onstatus}
              {ondelete}
              {onedit}
              {onselectpatchset}
            />
          {/if}
          {#if canExpand && !wholeFile && !combinedBelow && canExpandBelow(eh, i)}
            <div class="expand-row below">
              <button
                onclick={() => expand(i, 'below', STEP)}
                disabled={tipLines == null}
                aria-label="Show {STEP} lines below"
                title="Show {STEP} lines below"
              >
                <!-- "Show below": horizontal line at the hunk's current
                     bottom with an arrow pointing down into the lines
                     that would appear next. -->
                <svg
                  width="14"
                  height="14"
                  viewBox="0 0 16 16"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.25"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  aria-hidden="true"
                  focusable="false"
                >
                  <line x1="2" y1="3" x2="14" y2="3" />
                  <line x1="8" y1="3" x2="8" y2="15" />
                  <path d="M6 12 L8 15 L10 12" />
                </svg>
              </button>
            </div>
          {/if}
          {#if combinedBelow}
            <!-- Single combined "expand the entire gap" row that
                 replaces the 3-row expand-below / … / expand-above
                 stack when the gap is small enough to be filled by
                 one click. -->
            {@const gap = gapAfter(i)}
            <div class="expand-row combined">
              <button
                onclick={() => expandGapAfter(i)}
                disabled={tipLines == null}
                aria-label="Show all {gap} hidden line{gap === 1 ? '' : 's'}"
                title="Show all {gap} hidden line{gap === 1 ? '' : 's'}"
              >
                <!-- Combined "expand the whole gap" icon: same
                     outward-pointing geometry as the
                     expand-whole-file button — a center spine with
                     arrows pushing away from it on both sides — so
                     the two affordances read as members of the same
                     family. -->
                <svg
                  width="14"
                  height="14"
                  viewBox="0 0 16 16"
                  fill="none"
                  stroke="currentColor"
                  stroke-width="1.25"
                  stroke-linecap="round"
                  stroke-linejoin="round"
                  aria-hidden="true"
                  focusable="false"
                >
                  <path d="M6 4 L8 1 L10 4" />
                  <line x1="8" y1="1" x2="8" y2="6" />
                  <line x1="2" y1="8" x2="14" y2="8" />
                  <line x1="8" y1="10" x2="8" y2="15" />
                  <path d="M6 12 L8 15 L10 12" />
                </svg>
              </button>
            </div>
          {:else if i < file.hunks.length - 1 && !wholeFile && hasGapAfter(i)}
            <!-- Inter-hunk separator. The grey background is the
                 only signal; no text, no borders. The continuous
                 gutter line painted on the wrapper passes through
                 it unbroken. -->
            <div class="hunk-gap" aria-hidden="true"></div>
          {/if}
        {/each}
        </div>
        {#if composing?.kind === 'line' && composing.file === file.path && composerTop != null}
          <div
            class="line-composer-overlay"
            bind:this={composerOverlayEl}
            style:top="{composerTop}px"
            style:left="calc(var({sideBySide && composing.side === 'tip' ? '--measured-gutter-2' : '--measured-gutter'}, {gutterIndentPx}px) + 12px)"
          >
            <CommentComposer
              target={composing}
              anchorIds={lineAnchorIds}
              {saving}
              oncancel={oncancelcompose}
              onsubmit={onsubmit}
            />
          </div>
        {/if}
      </div>
    {/if}
  {/if}
</section>

<SelectionPopup
  selection={selectionPopup}
  anchorX={popupAnchorX}
  anchorY={popupAnchorY}
  oncomment={commentOnSelection}
  oncopy={copySelection}
  onnote={canAnnotate ? noteOnSelection : undefined}
/>

<style>
  .file-diff {
    border: 1px solid var(--border);
    border-radius: 6px;
    margin: 16px 0;
    /* overflow: hidden cuts off sticky-positioned children, so don't clip
     * here. The hunk tables manage their own overflow as needed. */
  }

  .file-header {
    display: flex;
    align-items: center;
    /* `flex-wrap` lets the debug-mode `jj diff` panel sit on a
     * second row INSIDE this sticky element. Without wrap, the
     * panel would overflow the single-row layout (or — worse —
     * push existing controls out of view). */
    flex-wrap: wrap;
    gap: 8px;
    padding: 8px 12px;
    /* Drag-selecting in a diff that runs to the end of the file
     * shouldn't extend the selection into the NEXT file's header
     * (which sits just below the last hunk). `user-select: none`
     * stops the browser's range at the previous content's edge.
     * The `.path` child re-enables selection for the file name
     * specifically — that text is the most-copied bit of the
     * header (paste into chat / a terminal / a bug report). */
    user-select: none;
    /* `--bg-elevated` is a step darker than `--bg-panel` — strong
     * enough that the file boundary registers immediately while
     * scrolling (--bg-panel was almost indistinguishable from the
     * page background) but still neutral enough to not compete with
     * the diff itself. */
    background: var(--bg-elevated);
    border-bottom: 1px solid var(--border);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 13px;
    font-weight: 500;
    /* Pin under the top header (which carries both rows in review
     * mode — see App.svelte). `--app-header-h` is set live by
     * App.svelte's ResizeObserver so this stays correct as the
     * second row appears / disappears. */
    position: sticky;
    top: var(--app-header-h);
    z-index: 10;
  }

  .toggle {
    background: transparent;
    border: none;
    padding: 0;
    font-size: 12px;
    color: var(--text-muted);
    cursor: pointer;
    width: 16px;
  }

  .file-header .path {
    flex: 1 1 auto;
    /* Re-enable selection that the outer `.file-header` disabled.
     * The path is the most-copied bit of the header (paste into
     * chat, a terminal, a bug report); the header-wide
     * `user-select: none` only exists to keep drag-selections in
     * the diff from spilling into the next file's header. */
    user-select: text;
    /* Without min-width:0 the path's intrinsic width pins the header
     * open, which on a phone overflows the viewport and squeezes the
     * diff into a horizontal-scroll slice. direction:rtl moves the
     * ellipsis to the *left* so we keep the filename visible when the
     * path is too long; text-align:left undoes the right-alignment
     * that direction:rtl otherwise imposes on short paths. */
    min-width: 0;
    direction: rtl;
    text-align: left;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .file-header .meta {
    color: var(--text-muted);
    font-size: 12px;
    /* `meta` is the lowest-priority cell in the header: when the path
     * eats all the available width, the hunk count disappears before
     * the filename does. */
    flex: 0 1 auto;
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    flex-shrink: 100;
  }

  /* Keep status badge + the icon button at full size — both are too
   * small to shrink usefully. */
  .file-header .status,
  .file-comment,
  .whole-file {
    flex-shrink: 0;
  }

  .whole-file {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 22px;
    padding: 0;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--text-muted);
    cursor: pointer;
  }

  .whole-file:hover:not(:disabled) {
    background: var(--link-bg);
    color: var(--link);
    border-color: var(--link);
  }

  .whole-file.on {
    background: var(--link-bg);
    color: var(--link);
    border-color: var(--link);
  }

  .whole-file:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .file-comment {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 26px;
    height: 22px;
    padding: 0;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--link);
    cursor: pointer;
  }

  .file-comment:hover {
    background: var(--link-bg);
  }

  .file-thread {
    /* Same accent as inline line-threads (see HunkLines.svelte) — keeps
     * the "this is commentary, not code" cue consistent across file-
     * level and line-level anchors. */
    padding: 8px 12px 8px 14px;
    background: var(--link-bg);
    border-bottom: 1px solid var(--border);
    border-left: 3px solid var(--link);
  }

  /* Orphan-threads block: line comments anchored outside the diff's
   * visible context. The amber left-accent + warn tint distinguishes
   * them from file-level threads (link blue) — they're "should be
   * inline but couldn't be" rather than "explicitly file-scoped."
   *
   * The whole box (background + border + accent stripe) is pushed
   * past the line-number gutter via `margin-left` so it visually
   * starts where the diff content begins — matches the inline
   * threads. `margin-right` keeps the same right-edge breathing
   * room those threads use. */
  .orphan-threads {
    padding: 8px 12px;
    /* `margin-left` is set inline from the file's gutter width so the
     * box aligns with where inline threads start. */
    margin-right: 12px;
    background: var(--warn-bg);
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
    border-left: 3px solid var(--warn-text);
  }

  .orphan-threads > .orphan-header {
    display: flex;
    align-items: center;
    gap: 6px;
    margin-bottom: 6px;
  }

  .orphan-threads .orphan-header p.muted {
    margin: 0;
    font-size: 12px;
    color: var(--warn-text);
  }

  /* Section toggle uses the same filled-triangle Chevron the gutter
   * marker uses, in the warn (orphan) palette so it tracks the rest
   * of the section's accent. Acts as the "gutter" for this group —
   * one click folds every orphan to nothing. */
  .orphan-toggle {
    background: transparent;
    border: none;
    color: var(--warn-text);
    cursor: pointer;
    padding: 0 2px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .orphan-toggle:hover {
    filter: brightness(1.2);
  }

  .file-composer {
    padding: 8px 12px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
  }

  .compact-line-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .compact-line-list > li {
    padding: 8px 12px;
    border-top: 1px solid var(--border-muted);
  }

  .compact-line-list > li:first-child {
    border-top: none;
  }

  /* Comments-only mode: show the code lines the comment anchors to
   * directly above its thread, so the reader doesn't have to flip
   * back to the diff to see what the comment is about. Bordered
   * panel echoes the diff hunk's visual weight without competing
   * with the inline diff. */
  .compact-context {
    margin-bottom: 6px;
    border: 1px solid var(--border-muted);
    border-radius: 4px;
    background: var(--bg-panel);
    overflow: hidden;
  }

  .compact-context-head {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 4px 8px;
    border-bottom: 1px solid var(--border-muted);
    font-size: 11px;
  }

  .compact-context-head code {
    background: var(--bg-elevated);
    color: var(--text-muted);
    padding: 1px 6px;
    border-radius: 3px;
    font-size: 11px;
  }

  .compact-context-body {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12.5px;
    line-height: 1.6;
  }

  /* Mini diff row — line number gutter on the left, content on the
   * right. Origin-class tints match HunkLines so an added line on
   * this view reads as the same colour as in the full diff. */
  .compact-context-row {
    display: grid;
    grid-template-columns: 48px 1fr;
    align-items: baseline;
  }

  .compact-context-row .ln {
    padding: 0 8px;
    text-align: right;
    color: var(--text-faint);
    user-select: none;
    font-size: 11px;
    border-right: 1px solid var(--border-muted);
  }

  .compact-context-row .content {
    margin: 0;
    padding: 0 8px;
    white-space: pre;
    overflow-x: auto;
    font: inherit;
  }

  .compact-context-row.origin-added {
    background: var(--add-bg);
  }
  .compact-context-row.origin-added .ln {
    background: var(--add-bg-strong);
  }
  .compact-context-row.origin-removed {
    background: var(--remove-bg);
  }
  .compact-context-row.origin-removed .ln {
    background: var(--remove-bg-strong);
  }

  .hunks-wrapper {
    position: relative;
  }

  /* When the reader is mid drag-select OR the SelectionPopup is
   * open, suppress the per-row `+ comment` / `+ note` gutter
   * buttons. The two "make a comment" affordances (gutter click vs
   * selection popup) should never both be live — without this rule
   * the gutter buttons keep firing their `.row:hover` rule as the
   * pointer slides across rows during a text drag, and they sit
   * under / next to the popup once it appears. `:global` because
   * the buttons live in HunkLines / HunkLinesSideBySide; `!important`
   * to beat the `.row:hover .add-comment { visibility: visible }`
   * rule in those components. */
  .hunks-wrapper.hide-gutter-affordances :global(.add-comment),
  .hunks-wrapper.hide-gutter-affordances :global(.add-note) {
    visibility: hidden !important;
  }

  .line-composer-overlay {
    /* Match the eventual posted `.comment` box, not the wider
     * `.thread-sticky` blue stripe around it: `left` is set inline
     * to (gutter width + 12), then `max-width` caps the box at the
     * same width the `.comment` inside thread-sticky tops out at
     * (see `--message-max-w` on `.thread-sticky`). `right: 24px` is
     * the upper bound when the viewport is narrow enough that the
     * cap doesn't apply — 12 of stripe right margin + 12 of stripe
     * inner padding. Same outer rect as the comment, so submitting
     * the draft doesn't visibly snap the white box smaller or
     * shift it sideways. */
    position: absolute;
    right: 24px;
    /* Cap matches `--message-max-w` (720px) minus thread-sticky's
     * 24px horizontal padding — so the composer's outer rect lands
     * exactly where the .comment box will appear inside the eventual
     * thread-sticky stripe. Tune both together by changing
     * --message-max-w on .thread-sticky / .file-thread. */
    max-width: 696px;
    /* Must beat .file-header (z-index: 10) so the composer isn't behind
     * the sticky header when commenting on a line near the top. */
    z-index: 12;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 6px;
  }

  /* Push table content down to make room for the absolute-positioned
   * composer overlay. Height set dynamically via --composer-h as the
   * composer's textarea grows / preview toggles. */
  :global(.composing-target) {
    padding-bottom: var(--composer-h, 220px) !important;
  }

  .hunks {
    background: var(--bg);
    /* Single horizontal scroll context for the whole file — long lines
     * scroll the entire hunk pack, not each line independently. */
    overflow-x: auto;
    /* Disable the browser's overscroll-bounce so users can't drag past
     * the diff's left/right edges. */
    overscroll-behavior-x: contain;
    /* Per CSS, `overflow-x: auto` implies `overflow-y: auto` (the
     * other axis can't stay `visible`), so the last hunk's row
     * gutter would clip any element that overflows downward. The
     * outdated-comment chevron is positioned at the bottom of its
     * row with `translateY(50%)` so half of it overflows past the
     * row's bottom edge — fine for any row above an inter-hunk
     * separator (the chevron paints over the separator), but on
     * the last row of the last hunk it would land in this
     * clipped area. Reserve 8px of room so the overflow stays
     * visible. */
    padding-bottom: 8px;
  }

  /* PROTOTYPE: precise selection paint. The browser's native
   * `::selection` is suppressed everywhere inside the diff; the
   * custom `.selection-overlay` divs (positioned absolutely in
   * `hunksWrapperEl` by the effect in the script block) provide all
   * the selection paint. This gives us per-line precision (rect
   * width follows the actual selected character range) AND an
   * inter-line gap fill (each rect's height is stretched to meet
   * the next rect's top). */
  .hunks :global(::selection) {
    background: transparent;
  }
  .selection-overlay {
    position: absolute;
    background: var(--selection-tint);
    pointer-events: none;
    z-index: 1;
  }
  /* Per-row gutter stripe for any row that overlaps the current text
   * selection — partial first/last lines get the same cue as full
   * middle lines. Cell-level rather than tr-level because the cells
   * carry the `data-side` / `data-line` we match on. */
  .hunks :global(.content.text-selection-stripe) {
    box-shadow: inset 4px 0 0 var(--selection-rule);
  }

  /* Inter-hunk separator + the expand-context affordances share the
   * same panel-grey backdrop so all "artificial space" between
   * content reads as one cohesive band. */
  .hunk-gap,
  .expand-row {
    position: relative;
    background: var(--bg-panel);
  }

  /* Extend the gutter stripe across inter-hunk gaps when a paint
   * path's range spans across them. The stripe lives on a pseudo-
   * element rather than `box-shadow` so its `left` can be set
   * per-side (--bridge-offset published by the paint effects) —
   * base/unified use --measured-gutter, SBS tip uses
   * --measured-gutter-2.
   *
   * Two classes so the transient drag/selection stripe (cyan,
   * 4px to match `.selected` / `.text-selection-stripe`) and the
   * persistent posted-comment stripe (blue, 3px to match
   * `.row.commented .content`) can coexist with their own colors
   * and widths, and so each effect manages its own bridge state
   * without stepping on the other. */
  .hunk-gap.drag-bridge::before,
  .expand-row.drag-bridge::before,
  .hunk-gap.comment-bridge::before,
  .expand-row.comment-bridge::before {
    content: '';
    position: absolute;
    top: 0;
    bottom: 0;
    left: var(--bridge-offset, 0);
    pointer-events: none;
  }
  .hunk-gap.drag-bridge::before,
  .expand-row.drag-bridge::before {
    width: 4px;
    background: var(--selection-rule);
  }
  .hunk-gap.comment-bridge::before,
  .expand-row.comment-bridge::before {
    width: 3px;
    background: var(--link);
  }

  .hunk-gap {
    height: 8px;
  }

  /* Flex (not block + text-align) so the button is vertically
   * centered in the row — text-align only positions inline content
   * horizontally, leaving the button baseline-aligned and sitting
   * a couple of px above the visual middle. */
  .expand-row {
    display: flex;
    align-items: center;
    padding: 2px 12px;
  }

  /* Gutter rules through the separator. A single ::before paints
   * BOTH the outer (`--measured-gutter`) and the inner / right-side
   * (`--measured-gutter-2`) rules via stacked linear-gradients —
   * unified-both has an inner rule between the two `.ln` cells,
   * SBS has a right-side rule; the second variable handles whichever
   * applies. The off-screen-left fallback (-1000px) on `--measured-
   * gutter-2` collapses the second line out of view in unified-tip
   * / unified-base where only one gutter exists. */
  .hunk-gap::before,
  .expand-row::before {
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

  .expand-row button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: none;
    /* Subdued by default; lights up to --link on hover so the affordance
     * is still discoverable without competing with the diff. */
    color: var(--text-faint);
    cursor: pointer;
    padding: 2px 6px;
    border-radius: 3px;
  }

  .expand-row button:hover:not(:disabled) {
    color: var(--link);
    background: var(--link-bg);
  }

  .expand-row button:disabled {
    opacity: 0.5;
    cursor: default;
  }

  .placeholder {
    color: var(--text-muted);
    padding: 12px;
    margin: 0;
    font-style: italic;
  }

  /* Debug-mode jj-command panel. Only visible with `?debug` in the
     URL — kept understated (no bright accents) since it's a
     diagnostic, not a primary affordance. */
  .debug-cmd {
    background: transparent;
    border: none;
    color: var(--text-faint);
    cursor: pointer;
    padding: 2px 6px;
    border-radius: 3px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .debug-cmd:hover {
    color: var(--link);
    background: var(--link-bg);
  }
  .debug-cmd[aria-pressed='true'] {
    color: var(--link);
    background: var(--link-bg);
  }
  .debug-panel {
    /* Drops onto a second flex row inside the sticky `.file-header`
     * — `flex-basis: 100%` forces a wrap regardless of available
     * width. Inherits the header's sticky positioning, so the panel
     * stays pinned with the header as the file scrolls. */
    flex: 0 0 100%;
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    /* Negative horizontal margin to undo the header's 12px padding
     * so the panel spans full width; vertical margin pulls it up
     * tight against the header row. */
    margin: 4px -12px -8px;
    padding: 6px 12px;
    background: var(--bg-panel);
    border-top: 1px solid var(--border-muted);
    font-size: 12px;
  }
  .debug-panel code {
    flex: 1;
    min-width: 0;
    overflow-x: auto;
    white-space: nowrap;
    background: var(--bg-elevated);
    padding: 3px 6px;
    border-radius: 3px;
    font-family: ui-monospace, monospace;
  }
  .debug-copy {
    flex: 0 0 auto;
    background: var(--bg-elevated);
    border: 1px solid var(--border);
    color: var(--text);
    padding: 3px 8px;
    border-radius: 3px;
    cursor: pointer;
    font-size: 12px;
  }
  .debug-copy:hover {
    background: var(--bg-panel);
  }
  .debug-note {
    flex-basis: 100%;
    margin: 0;
    font-size: 11px;
  }
</style>
