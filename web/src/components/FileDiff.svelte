<script lang="ts">
  import { getContext, tick } from 'svelte';
  import { copyText } from '../lib/clipboard';
  import { api } from '../lib/api';
  import type {
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
  import Bubble from './Bubble.svelte';
  import CommentComposer from './CommentComposer.svelte';
  import CommentThread from './CommentThread.svelte';
  import HunkLines from './HunkLines.svelte';
  import HunkLinesSideBySide from './HunkLinesSideBySide.svelte';

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
    responses,
    currentPatchset,
    composing,
    saving,
    showDiffs = true,
    showComments = true,
    commentsWriteable = true,
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
    lastVisitAt = null,
    viewer = '',
  }: Props = $props();

  let collapsed = $state(false);

  /** Debug-mode hooks. `debug` comes from ReviewViewer's context
   *  (turned on by `?debug` in the URL). When true, the file header
   *  renders a "$" icon that toggles a panel showing the literal
   *  `jj diff` command equivalent for this file's current view —
   *  handy when cross-checking what the UI actually computes against
   *  what the CLI would say. */
  const debug = getContext<boolean>('kata-debug') ?? false;
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
  let hunksEl: HTMLDivElement | undefined = $state();
  let composeSelected: HTMLElement[] = [];

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
      wrapper.style.setProperty(
        '--measured-gutter',
        `${contentCell.offsetLeft}px`,
      );
    };
    const ro = new ResizeObserver(measure);
    const observeLnCells = () => {
      if (lnCells) for (const el of lnCells) ro.unobserve(el);
      lnCells = wrapper.querySelectorAll('td.ln');
      for (const el of lnCells) ro.observe(el);
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
   *  doesn't re-evaluate every row in every hunk. */
  $effect(() => {
    for (const el of composeSelected) el.classList.remove('selected');
    composeSelected = [];
    if (!sectionEl) return;
    if (composing?.kind !== 'line' || composing.file !== file.path) return;
    for (let ln = composing.startLine; ln <= composing.endLine; ln++) {
      const matches = sectionEl.querySelectorAll(
        `[data-side="${composing.side}"][data-line="${ln}"]`,
      );
      for (const el of matches) {
        (el as HTMLElement).classList.add('selected');
        composeSelected.push(el as HTMLElement);
      }
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

    const target = sectionEl.querySelector(
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
   *  hunk `i`'s expanded end and hunk `i+1`'s expanded start (on
   *  EITHER side — the gap separator is meaningful as soon as one
   *  side has unrendered content). Hidden when the two hunks now
   *  meet end-to-end. */
  function hasGapAfter(i: number): boolean {
    const hunks = file.hunks ?? [];
    const cur = hunks[i];
    const next = hunks[i + 1];
    if (!cur?.tip_range || !next?.tip_range) return true;
    const curExp = clippedExpansions.get(i) ?? { above: 0, below: 0 };
    const nextExp = clippedExpansions.get(i + 1) ?? { above: 0, below: 0 };
    const tipGap =
      next.tip_range.start - nextExp.above - (cur.tip_range.end + curExp.below) - 1;
    if (tipGap > 0) return true;
    if (cur.base_range && next.base_range) {
      const baseGap =
        next.base_range.start -
        nextExp.above -
        (cur.base_range.end + curExp.below) -
        1;
      if (baseGap > 0) return true;
    }
    return false;
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
    <div
      class="orphan-threads"
      style:margin-left="var(--measured-gutter, {gutterIndentPx}px)"
    >
      <p class="muted">
        Anchored outside the diff's context — the lines these comments
        attached to aren't part of the visible hunks.
      </p>
      <CommentThread
        comments={orphanLineComments}
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
          <li>
            <div class="compact-line-marker">
              <code>L{c.lines?.start}{c.lines && c.lines.end !== c.lines.start ? `–${c.lines.end}` : ''}</code>
              <span class="muted">{c.side ?? ''}</span>
            </div>
            <CommentThread
              comments={[c]}
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
      <div class="hunks-wrapper" bind:this={hunksWrapperEl}>
        <div class="hunks" bind:this={hunksEl}>
        {#each file.hunks as _, i (i)}
          {@const eh = withContext(file.hunks[i], i)}
          {#if canExpand && !wholeFile && canExpandAbove(eh, i)}
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
          {#if canExpand && !wholeFile && canExpandBelow(eh, i)}
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
          {#if i < file.hunks.length - 1 && !wholeFile && hasGapAfter(i)}
            <div class="hunk-gap">…</div>
          {/if}
        {/each}
        </div>
        {#if composing?.kind === 'line' && composing.file === file.path && composerTop != null}
          <div
            class="line-composer-overlay"
            bind:this={composerOverlayEl}
            style:top="{composerTop}px"
            style:left="var(--measured-gutter, {gutterIndentPx}px)"
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

  .orphan-threads > p.muted {
    margin: 0 0 6px;
    font-size: 12px;
    color: var(--warn-text);
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

  .compact-line-marker {
    display: flex;
    align-items: baseline;
    gap: 8px;
    margin-bottom: 4px;
    font-size: 12px;
  }

  .compact-line-marker code {
    background: var(--bg-elevated);
    color: var(--text-muted);
    padding: 1px 6px;
    border-radius: 3px;
    font-size: 11px;
  }

  .hunks-wrapper {
    position: relative;
  }

  .line-composer-overlay {
    /* `left` is set inline to (gutter width + 14) so the box itself
     * starts at the diff content edge — matches the inline threads.
     * `right: 12px` keeps the same breathing room on the far side. */
    position: absolute;
    right: 12px;
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
  }

  .hunk-gap {
    text-align: center;
    color: var(--text-faint);
    background: var(--bg-panel);
    border-top: 1px solid var(--border-muted);
    border-bottom: 1px solid var(--border-muted);
    padding: 2px 0;
    font-family: ui-monospace, monospace;
    font-size: 11px;
  }

  .expand-row {
    /* No background or borders — the old strong-blue text on a panel
     * fill drew the eye away from the diff content. The icon-only
     * button is enough of a target on its own. */
    padding: 2px 12px;
    text-align: left;
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
