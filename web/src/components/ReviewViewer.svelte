<script lang="ts">
  import { onMount, tick } from 'svelte';
  import { api } from '../lib/api';
  import { subscribe as subscribeEvents } from '../lib/events';
  import type {
    CommentView,
    CommitDiffView,
    ComposerTarget,
    DraftCommentInput,
    DraftResponseInput,
    Patchset,
    ResponseView,
    ReviewView,
  } from '../lib/types';
  import { sortFilesLikeTree } from '../lib/tree';
  import { setTokenizationPaused } from '../lib/highlight.svelte';
  import { resolutionFor } from '../lib/resolution';
  import CommentComposer from './CommentComposer.svelte';
  import CommentThread from './CommentThread.svelte';
  import CommitsPanel from './CommitsPanel.svelte';
  import FileSlot from './FileSlot.svelte';
  import FileTree from './FileTree.svelte';
  import ReviewSummary from './ReviewSummary.svelte';

  /** State + action callbacks for the controls that App.svelte renders in
   *  the sticky top bar. Re-emitted whenever any of the underlying fields
   *  change; null only when the review viewer is unmounted.
   *
   *  Comment-level controls (filter chips, prev/next nav, comments-only
   *  toggle) live in a second sticky bar rendered by the viewer itself,
   *  directly above the commits panel — they share visual context with
   *  the comments and shouldn't be split across two bars. */
  export interface ReviewToolbarState {
    /** Draft session controls. Null when the user has no open drafts.
     *  `position` is 1-based among `count` drafts (in document order),
     *  or 0 when the current nav target isn't one of the viewer's
     *  drafts. The shell renders the prev/next around the count so the
     *  reader can step between their own pending drafts without
     *  scanning the comment-bar for them. */
    drafts: {
      count: number;
      position: number;
      saving: boolean;
      publish: () => Promise<void>;
      discard: () => Promise<void>;
      prev: () => void;
      next: () => void;
    } | null;
    /** Prev / next commit nav. `null` when the review has zero
     *  commits in its revset (nothing to scope to). Position 0 means
     *  the viewer is on "All commits"; 1..total points at an
     *  individual commit. `prev` / `next` cycle through the commits,
     *  bouncing through 0 ("All commits") between the ends so the
     *  user can always get back to the whole-review view. */
    commits: {
      total: number;
      position: number;
      /** Short label for the current selection — change-id prefix +
       *  description first-line, or "All commits". */
      label: string;
      prev: () => void;
      next: () => void;
    } | null;
    /** File-tree visibility. The top bar surfaces this so phones can
     *  toggle the drawer-style tree without scrolling. */
    tree: { collapsed: boolean; toggle: () => void };
  }

  interface Props {
    repo: string;
    view: ReviewView;
    /** Currently signed-in viewer's author identity. Used to gate
     *  "Edit summary" affordances to the review's creator only. Empty
     *  string before whoami has resolved (treated as not-creator). */
    viewer: string;
    /** Patchset to start on. Undefined means "the latest". */
    initialPatchset?: number;
    /** Patchset-compare to start on: when set, the viewer opens in
     *  compare mode showing the patchset[compare] → patchset[ps]
     *  delta. Undefined for the normal base..tip view. */
    initialCompareWith?: number;
    /** Fires when the user picks a different patchset or compare target.
     *  Reports both so App.svelte can keep the URL (`?ps=&cmp=`) in
     *  sync. `compare === null` means leaving compare mode. */
    onviewchange?: (patchset: number, compare: number | null) => void;
    /** Reports toolbar state up to the app shell so the publish / discard
     *  controls and the diff-collapse toggle can live in the always-visible
     *  top bar instead of scrolling away with the page. */
    ontoolbarchange?: (bar: ReviewToolbarState | null) => void;
  }
  let {
    repo,
    view,
    viewer,
    initialPatchset,
    initialCompareWith,
    onviewchange,
    ontoolbarchange,
  }: Props = $props();

  // We seed local state from the prop and then manage refreshes ourselves.
  // svelte-ignore state_referenced_locally
  let current: ReviewView = $state(view);
  // svelte-ignore state_referenced_locally
  let selectedPatchset = $state(initialPatchset ?? view.manifest.current_patchset);
  /** When non-null, the viewer is in compare mode: the diff describes
   *  the patchset[compareWith] → patchset[selectedPatchset] delta. */
  // svelte-ignore state_referenced_locally
  let compareWith: number | null = $state(initialCompareWith ?? null);
  // Use raw state so reads of composing.* don't create thousands of
  // per-property signal subscriptions across rows. We always reassign the
  // whole object (never mutate fields), so the granular reactivity isn't
  // useful here.
  let composing: ComposerTarget | null = $state.raw(null);
  let saving = $state(false);
  let error: string | null = $state(null);

  /** When true the file diffs collapse to comments-only mode. State lives
   *  here so the top-bar toggle stays in sync with the viewport. */
  let diffsCollapsed = $state(false);

  // --- Comment filter -------------------------------------------------
  // Two independent dimensions: lifecycle (draft / open / resolved) and
  // severity (must-do / suggestion / other). A comment is shown when
  // BOTH dimensions accept it — so flipping every chip off hides
  // everything. Resolved here covers both "resolved" and "wont-fix":
  // the user thinks of them as the same "done with it" bucket.
  type StatusBucket = 'draft' | 'open' | 'resolved';
  type FlagBucket = 'must-do' | 'suggestion' | 'other';
  const FILTER_KEY = 'kata:commentFilter';
  function readFilter(): {
    status: Record<StatusBucket, boolean>;
    flag: Record<FlagBucket, boolean>;
  } {
    const def = {
      status: { draft: true, open: true, resolved: true },
      flag: { 'must-do': true, suggestion: true, other: true },
    } as const;
    if (typeof localStorage === 'undefined') return structuredClone(def);
    try {
      const raw = localStorage.getItem(FILTER_KEY);
      if (!raw) return structuredClone(def);
      const v = JSON.parse(raw);
      return {
        status: { ...def.status, ...(v?.status ?? {}) },
        flag: { ...def.flag, ...(v?.flag ?? {}) },
      };
    } catch {
      return structuredClone(def);
    }
  }
  const initialFilter = readFilter();
  let filterStatus = $state<Record<StatusBucket, boolean>>(initialFilter.status);
  let filterFlag = $state<Record<FlagBucket, boolean>>(initialFilter.flag);
  $effect(() => {
    if (typeof localStorage === 'undefined') return;
    localStorage.setItem(
      FILTER_KEY,
      JSON.stringify({ status: filterStatus, flag: filterFlag }),
    );
  });

  /** Bound to the sticky `.comment-bar` so we can measure its height
   *  and expose it as a CSS variable. The file-header is also sticky
   *  at `top: var(--app-header-h)` — without offsetting it by the
   *  comment bar's height the two would collide and the comment bar
   *  (higher z-index) would cover the file name as the user scrolls. */
  let commentBarEl: HTMLElement | undefined = $state();
  $effect(() => {
    if (!commentBarEl) return;
    const update = () => {
      document.documentElement.style.setProperty(
        '--comment-bar-h',
        `${commentBarEl!.offsetHeight}px`,
      );
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(commentBarEl);
    return () => {
      ro.disconnect();
      // Clear so other (non-review) screens don't inherit the offset.
      document.documentElement.style.removeProperty('--comment-bar-h');
    };
  });

  const allComments: CommentView[] = $derived([
    ...current.comments,
    ...current.drafts.comments,
  ]);

  const allResponses: ResponseView[] = $derived([
    ...current.responses,
    ...current.drafts.responses,
  ]);

  /** Bucket a comment by its lifecycle state: draft vs open vs
   *  resolved (collapsing resolved + wont-fix into one bucket). */
  function statusBucket(c: CommentView, responses: ResponseView[]): StatusBucket {
    if (c.draft) return 'draft';
    return resolutionFor(c.comment_id, responses) === 'open' ? 'open' : 'resolved';
  }

  /** All comments minus the ones the user has filtered out. Every
   *  downstream view (review-wide list, per-file slots, the prev/next
   *  nav, the commits-panel counts) reads from this so the filter has
   *  one consistent effect across the page. */
  const visibleComments = $derived(
    allComments.filter(
      (c) => filterStatus[statusBucket(c, allResponses)] && filterFlag[c.flag],
    ),
  );
  async function toggleDiffs() {
    diffsCollapsed = !diffsCollapsed;
    // Toggling the view re-renders the whole file list, which scrolls
    // the page back to the top. If the user was reading a specific
    // comment (reached via prev/next), re-anchor on it after the
    // layout has flushed so they don't lose their place.
    if (navCommentId) {
      const target = orderedComments.find(
        (c) => c.comment_id === navCommentId,
      );
      if (target) {
        await tick();
        void scrollToComment(target.comment_id, target.file ?? null);
      }
    }
  }

  /** Comments in document order: review-wide first (only when viewing the
   *  full review — review-wide comments don't belong to any single
   *  commit), then per file (in the same DFS order as the file tree),
   *  then by line within a file. Drafts and published are merged.
   *  Used to drive the top-bar prev / next comment buttons. */
  const orderedComments = $derived.by(() => {
    const all = visibleComments;
    const scoped = scopedChangeId !== null;
    // When scoped to a single commit, `orderedFiles` already reflects
    // just that commit's files (it derives from `displayedDiff`), so
    // filtering by file membership scopes the nav automatically. In
    // non-scoped mode we keep every file-anchored comment in the nav
    // even if its file is missing from the diff (anchor moved out,
    // patchset-compare hiding unchanged files, etc.) — otherwise a
    // filter that should leave one draft visible can yield an empty
    // nav and look like the prev/next controls broke.
    const fileOrder = new Map(orderedFiles.map((f, i) => [f.path, i]));
    const reviewWide = scoped
      ? []
      : all
          .filter((c) => c.file == null)
          .sort((a, b) => a.created_at.localeCompare(b.created_at));
    const inFiles = all
      .filter((c) => c.file != null && (!scoped || fileOrder.has(c.file)))
      .sort((a, b) => {
        // Missing files sort to the end via `Infinity` rather than
        // throwing the comment out of the list.
        const ao = fileOrder.get(a.file!) ?? Number.POSITIVE_INFINITY;
        const bo = fileOrder.get(b.file!) ?? Number.POSITIVE_INFINITY;
        if (ao !== bo) return ao - bo;
        // File-level comments (no lines) sort before line-level within the file.
        const al = a.lines?.start ?? -1;
        const bl = b.lines?.start ?? -1;
        if (al !== bl) return al - bl;
        return a.created_at.localeCompare(b.created_at);
      });
    return [...reviewWide, ...inFiles];
  });

  /** Comment id the user last navigated to. We track the id rather than
   *  an index so reorderings (e.g. a draft discard) don't strand us on
   *  the wrong comment — `navPosition` re-derives from the live list. */
  let navCommentId: string | null = $state(null);
  const navPosition = $derived.by(() => {
    if (!navCommentId) return 0;
    const i = orderedComments.findIndex((c) => c.comment_id === navCommentId);
    return i < 0 ? 0 : i + 1;
  });

  function navTo(idx: number) {
    if (orderedComments.length === 0) return;
    // Wrap so prev from #1 lands on the last and next from last lands on
    // #1 — feels less like hitting a wall during triage.
    const n = orderedComments.length;
    const wrapped = ((idx - 1 + n) % n) + 1;
    const target = orderedComments[wrapped - 1];
    navCommentId = target.comment_id;
    void scrollToComment(target.comment_id, target.file ?? null);
  }

  /** Drafts in document order — same shape as `orderedComments` but
   *  pulled from `current.drafts.comments` directly so it ignores the
   *  comment-bar status/severity filter. The header's "N drafts"
   *  indicator counts ALL of the viewer's drafts, so the nav next to
   *  it should iterate ALL of them too. */
  const orderedDraftIds = $derived.by(() => {
    const fileOrder = new Map(orderedFiles.map((f, i) => [f.path, i]));
    return current.drafts.comments
      .slice()
      .sort((a, b) => {
        // Review-wide drafts sort before file-anchored ones, then by
        // file order, then by line, then by created_at as a tiebreak.
        const af = a.file == null ? -1 : (fileOrder.get(a.file) ?? Number.POSITIVE_INFINITY);
        const bf = b.file == null ? -1 : (fileOrder.get(b.file) ?? Number.POSITIVE_INFINITY);
        if (af !== bf) return af - bf;
        const al = a.lines?.start ?? -1;
        const bl = b.lines?.start ?? -1;
        if (al !== bl) return al - bl;
        return a.created_at.localeCompare(b.created_at);
      })
      .map((c) => c.comment_id);
  });

  const navDraftPosition = $derived.by(() => {
    if (!navCommentId) return 0;
    const i = orderedDraftIds.indexOf(navCommentId);
    return i < 0 ? 0 : i + 1;
  });

  function navDraftAt(idx: number) {
    if (orderedDraftIds.length === 0) return;
    const n = orderedDraftIds.length;
    const wrapped = ((idx - 1 + n) % n) + 1;
    const id = orderedDraftIds[wrapped - 1];
    const target = current.drafts.comments.find((c) => c.comment_id === id);
    if (!target) return;
    navCommentId = id;
    void scrollToComment(id, target.file ?? null);
  }
  function navDraftPrev() {
    navDraftAt(navDraftPosition === 0 ? orderedDraftIds.length : navDraftPosition - 1);
  }
  function navDraftNext() {
    navDraftAt(navDraftPosition === 0 ? 1 : navDraftPosition + 1);
  }

  /** True when `el` is already fully visible in the usable area —
   *  i.e. it sits between the bottom edge of the sticky bars and the
   *  bottom of the viewport. Used by comment-nav so prev/next becomes
   *  a no-op when the target is already on screen, instead of
   *  backtracking to re-center it. */
  function fullyVisible(el: HTMLElement): boolean {
    const rect = el.getBoundingClientRect();
    const usableTop = stickyTop();
    const usableBottom = window.innerHeight;
    return rect.top >= usableTop && rect.bottom <= usableBottom;
  }

  /** Pixels of context kept above a comment when we scroll one into
   *  view. Roughly six lines of diff text — enough to see what the
   *  comment is anchored to without pushing the comment itself off
   *  the bottom of the viewport. */
  const COMMENT_CONTEXT = 120;

  /** Scroll `el` so it sits `COMMENT_CONTEXT` pixels below the sticky
   *  bars. The user lands on the comment with the anchored lines (and
   *  a few rows of surrounding diff) still visible above it, instead
   *  of slammed up against the comment-bar's edge. */
  function scrollCommentIntoView(el: HTMLElement): void {
    const target =
      el.getBoundingClientRect().top + window.scrollY - stickyTop() - COMMENT_CONTEXT;
    window.scrollTo({ top: Math.max(0, target), behavior: 'auto' });
  }

  /** Scroll `el` so its top sits just below the sticky bars *and* the
   *  file's own sticky `.file-header` (which would otherwise cover
   *  the top of the comment). Review-wide comments live outside any
   *  file-diff, so the file-header term collapses to zero and we
   *  end up flush with the comment bar. */
  function scrollCommentFlush(el: HTMLElement): void {
    const fileHeader = el
      .closest('.file-diff')
      ?.querySelector('.file-header') as HTMLElement | null;
    const extra = fileHeader?.offsetHeight ?? 0;
    const target =
      el.getBoundingClientRect().top + window.scrollY - stickyTop() - extra;
    window.scrollTo({ top: Math.max(0, target), behavior: 'auto' });
  }

  /** Where to park a navigated-to comment vertically. In normal
   *  (diff-visible) mode we keep `COMMENT_CONTEXT` pixels of diff
   *  above so the reader sees what the comment is anchored to. In
   *  comments-only mode there's no diff above to look at — that
   *  buffer would just be dead space — so the comment goes flush
   *  with the bottom of the sticky bars. */
  function bringCommentIntoView(el: HTMLElement): void {
    if (diffsCollapsed) {
      scrollCommentFlush(el);
    } else {
      scrollCommentIntoView(el);
    }
  }

  /** Scroll a comment into view, mounting its file's slot if it's been
   *  virtualized away.
   *
   *  In normal (diff-visible) mode an already-visible target stays
   *  put so prev/next doesn't shake the page when two consecutive
   *  comments share the screen. In comments-only mode the page is
   *  dense — several comments often fit on screen at once, so that
   *  rule would make multiple arrow presses look like no-ops until
   *  the next comment ran off the bottom. Re-park on every press
   *  there instead, so each step visibly advances. */
  async function scrollToComment(commentId: string, file: string | null) {
    let el = document.querySelector<HTMLElement>(
      `[data-comment-id="${CSS.escape(commentId)}"]`,
    );
    if (el) {
      if (diffsCollapsed || !fullyVisible(el)) bringCommentIntoView(el);
      return;
    }
    if (file) {
      const slot = document.querySelector<HTMLElement>(
        `[data-file-path="${CSS.escape(file)}"]`,
      );
      if (slot) scrollTopOf(slot);
    }
    // Wait up to ~500ms for the FileSlot's IntersectionObserver to mount
    // the file, then for FileDiff to render its comment threads.
    for (let i = 0; i < 30; i++) {
      await new Promise((r) => requestAnimationFrame(r));
      el = document.querySelector<HTMLElement>(
        `[data-comment-id="${CSS.escape(commentId)}"]`,
      );
      if (el) {
        if (diffsCollapsed || !fullyVisible(el)) bringCommentIntoView(el);
        return;
      }
    }
  }

  function navPrev() {
    navTo(navPosition === 0 ? orderedComments.length : navPosition - 1);
  }
  function navNext() {
    navTo(navPosition === 0 ? 1 : navPosition + 1);
  }

  /** Sync `navCommentId` to whatever's at the top of the visible area
   *  as the user scrolls. This is what makes the `x/N` counter in the
   *  comment bar feel "alive" — the reader scrolls past a comment and
   *  the position ticks up without clicking prev/next.
   *
   *  Throttled via requestAnimationFrame: at most one recompute per
   *  frame even during fling scroll. The walk over `orderedComments` is
   *  O(N) but for the typical review size (tens of comments) that
   *  costs single-digit microseconds, well below a frame budget. */
  $effect(() => {
    if (typeof window === 'undefined') return;
    let scheduled = false;
    function sync() {
      scheduled = false;
      if (orderedComments.length === 0) return;
      const top = stickyTop();
      // First comment whose top is at or below the sticky bars wins —
      // that's "the next thing the reader will engage with." If they
      // all sit above the bars (whole review scrolled past), use the
      // last one so the counter pegs at N/N rather than blanking out.
      let last: string | null = null;
      for (const c of orderedComments) {
        const el = document.querySelector<HTMLElement>(
          `[data-comment-id="${CSS.escape(c.comment_id)}"]`,
        );
        if (!el) continue;
        last = c.comment_id;
        const rect = el.getBoundingClientRect();
        // -2 lets a comment that's *just* touching the sticky bar
        // edge count as "current," avoiding flicker when scrolling
        // lands exactly on the boundary.
        if (rect.top >= top - 2) {
          if (navCommentId !== c.comment_id) navCommentId = c.comment_id;
          return;
        }
      }
      if (last && navCommentId !== last) navCommentId = last;
    }
    function onScroll() {
      if (scheduled) return;
      scheduled = true;
      requestAnimationFrame(sync);
    }
    window.addEventListener('scroll', onScroll, { passive: true });
    // Run once on mount so the counter starts on whatever's at the
    // top, not stuck at 0/N.
    queueMicrotask(sync);
    return () => window.removeEventListener('scroll', onScroll);
  });

  /** Mirror toolbar state up to the app shell whenever it changes. The
   *  shell renders publish / discard, commit nav, and the file-tree
   *  toggle. Comment filter + prev/next + comments-only toggle live in
   *  the viewer's own sticky bar (see template), so they're omitted here. */
  $effect(() => {
    const hasDrafts =
      !!current.drafts.session && current.drafts.comments.length > 0;
    ontoolbarchange?.({
      drafts: hasDrafts
        ? {
            count: current.drafts.comments.length,
            position: navDraftPosition,
            saving,
            publish,
            discard,
            prev: navDraftPrev,
            next: navDraftNext,
          }
        : null,
      commits:
        current.commits.length > 0
          ? {
              total: current.commits.length,
              position: commitNavIndex + 1, // 1..N, 0 for "All"
              label: commitNavLabel,
              prev: commitNavPrev,
              next: commitNavNext,
            }
          : null,
      tree: {
        collapsed: treeCollapsed,
        toggle: () => (treeCollapsed = !treeCollapsed),
      },
    });
  });

  /** Make sure the toolbar clears when the viewer unmounts (e.g. user
   *  navigates back to the review list). */
  onMount(() => () => ontoolbarchange?.(null));

  /** Pause tokenization while a composer is open so the user can type
   *  without input lag — `codeToTokensBase` is synchronous (~200-500ms
   *  per big file) and tokenize bursts triggered by the layout shift of
   *  mounting the composer would otherwise queue keystrokes behind them. */
  $effect(() => {
    if (composing) {
      setTokenizationPaused(true);
      return () => setTokenizationPaused(false);
    }
  });

  /** The patchset the viewer is currently looking at. Falls back to the
   *  manifest's current patchset if `selectedPatchset` somehow drifted. */
  const viewing = $derived(
    current.manifest.patchsets.find((p) => p.n === selectedPatchset) ??
      current.manifest.patchsets.find((p) => p.n === current.manifest.current_patchset)!,
  );

  /** When non-null, the diff is scoped to a single commit instead of the
   *  full review range. The full ReviewView (comments, drafts, etc.) is
   *  still loaded — only the diff display changes. The endpoints' change
   *  ids ride along so we can build a per-commit "view patchset" that
   *  scopes file-content reads + new-comment anchoring to the clicked
   *  commit (otherwise highlights pull from the whole-review tip, whose
   *  line numbers may differ when later commits touch the same file). */
  let scopedChangeId: string | null = $state(null);
  let scopedDiff = $state<CommitDiffView | null>(null);

  const displayedFiles = $derived.by(() => {
    const sd = scopedDiff;
    return sd ? sd.files : current.diff.files;
  });
  /** Files reordered to match the file tree's DFS traversal so the diff
   *  panel reads top-to-bottom the way the sidebar does. */
  const orderedFiles = $derived(sortFilesLikeTree(displayedFiles));

  /** Patchset to thread through to FileSlot/FileDiff for file content,
   *  highlights, and new-comment anchors. In scoped view this points
   *  at the clicked commit and its parent; otherwise at the review's
   *  current patchset. */
  const viewingFor = $derived.by<Patchset>(() => {
    const sd = scopedDiff;
    if (!sd) return viewing;
    return {
      ...viewing,
      base_change: sd.base_change,
      base_commit: sd.base_commit,
      tip_change: sd.tip_change,
      tip_commit: sd.tip_commit,
    };
  });

  // Sidebar layout state, persisted to localStorage.
  const TREE_WIDTH_KEY = 'kata:treeWidth';
  const TREE_COLLAPSED_KEY = 'kata:treeCollapsed';
  const DEFAULT_TREE_WIDTH = 280;

  function readNumber(key: string, fallback: number): number {
    if (typeof localStorage === 'undefined') return fallback;
    const v = localStorage.getItem(key);
    const n = v == null ? NaN : Number(v);
    return Number.isFinite(n) ? n : fallback;
  }

  /** On phones the tree is a drawer that overlays the diff, so it has
   *  to start closed — otherwise the page loads with the diff dimmed
   *  behind a backdrop. Persisted desktop preference still applies on
   *  desktop. */
  function isPhoneViewport(): boolean {
    return (
      typeof window !== 'undefined' &&
      window.matchMedia('(max-width: 640px)').matches
    );
  }

  let treeCollapsed = $state(
    isPhoneViewport() ||
      (typeof localStorage !== 'undefined' &&
        localStorage.getItem(TREE_COLLAPSED_KEY) === 'true'),
  );
  let treeWidth = $state(readNumber(TREE_WIDTH_KEY, DEFAULT_TREE_WIDTH));

  $effect(() => {
    if (typeof localStorage === 'undefined') return;
    // Don't persist phone toggles — the drawer is transient there, so a
    // user opening it during navigation shouldn't pin the desktop view
    // open on their next visit.
    if (isPhoneViewport()) return;
    localStorage.setItem(TREE_COLLAPSED_KEY, String(treeCollapsed));
  });
  $effect(() => {
    if (typeof localStorage === 'undefined') return;
    localStorage.setItem(TREE_WIDTH_KEY, String(treeWidth));
  });

  function startResize(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    const startX = e.clientX;
    const startW = treeWidth;
    const onMove = (ev: PointerEvent) => {
      treeWidth = Math.max(180, Math.min(640, startW + (ev.clientX - startX)));
    };
    const onUp = () => {
      document.removeEventListener('pointermove', onMove);
      document.removeEventListener('pointerup', onUp);
      document.body.style.cursor = '';
      document.body.style.userSelect = '';
    };
    document.addEventListener('pointermove', onMove);
    document.addEventListener('pointerup', onUp);
    document.body.style.cursor = 'col-resize';
    document.body.style.userSelect = 'none';
  }

  /** Separate from `saving` (which is about write ops): true while a
   *  commit-scoped diff is being fetched, so the UI can show feedback. */
  let loadingDiff = $state(false);
  /** What we're loading, for the banner — easier to scan than just a spinner. */
  let loadingDiffLabel = $state('');

  async function selectCommit(changeId: string | null) {
    if (changeId === null) {
      scopedChangeId = null;
      scopedDiff = null;
      return;
    }
    loadingDiff = true;
    loadingDiffLabel = changeId.slice(0, 12);
    error = null;
    try {
      scopedDiff = await api.commitDiff(repo, current.manifest.review_id, changeId);
      scopedChangeId = changeId;
    } catch (e) {
      error = (e as Error).message;
    } finally {
      loadingDiff = false;
    }
  }

  /** Where the currently scoped commit sits in `current.commits`.
   *  -1 = "All commits", otherwise the 0-based index. The toolbar's
   *  prev/next bounce through -1 between the ends, so the user can
   *  always step back to the whole-review view without leaving the
   *  keyboard. */
  const commitNavIndex = $derived.by(() => {
    if (scopedChangeId === null) return -1;
    return current.commits.findIndex((c) => c.change_id === scopedChangeId);
  });
  const commitNavLabel = $derived.by(() => {
    if (commitNavIndex < 0) return 'All commits';
    const c = current.commits[commitNavIndex];
    if (!c) return 'All commits';
    const short = c.change_id.slice(0, 8);
    const subject = c.description_first_line.trim() || '(no description)';
    // Truncate so the top bar stays a single line on narrower screens.
    const trimmed = subject.length > 60 ? `${subject.slice(0, 57)}…` : subject;
    return `${short} · ${trimmed}`;
  });
  function selectCommitByIndex(i: number) {
    if (i < 0) {
      void selectCommit(null);
      return;
    }
    const c = current.commits[i];
    if (c) void selectCommit(c.change_id);
  }
  function commitNavPrev() {
    if (current.commits.length === 0) return;
    if (commitNavIndex < 0) {
      // From "All" → last commit.
      selectCommitByIndex(current.commits.length - 1);
    } else if (commitNavIndex === 0) {
      // Wrap to "All".
      selectCommitByIndex(-1);
    } else {
      selectCommitByIndex(commitNavIndex - 1);
    }
  }
  function commitNavNext() {
    if (current.commits.length === 0) return;
    if (commitNavIndex < 0) {
      selectCommitByIndex(0);
    } else if (commitNavIndex === current.commits.length - 1) {
      selectCommitByIndex(-1);
    } else {
      selectCommitByIndex(commitNavIndex + 1);
    }
  }

  /** Whole-review comments (no file, no lines). Filtered. */
  const reviewComments: CommentView[] = $derived(
    visibleComments.filter((c) => c.file == null),
  );

  /** Files actually rendered in the main panel. In comments-only mode
   *  files with no (visible) comments are hidden so the page is a flat
   *  list of feedback; the file being composed on stays visible so the
   *  inline composer doesn't disappear under the user. */
  const visibleFiles = $derived.by(() => {
    if (!diffsCollapsed) return orderedFiles;
    const withComments = new Set(
      visibleComments.map((c) => c.file).filter((p): p is string => !!p),
    );
    const composingFile =
      composing && 'file' in composing ? composing.file : null;
    return orderedFiles.filter(
      (f) => withComments.has(f.path) || f.path === composingFile,
    );
  });


  function short(id: string): string {
    return id.length > 12 ? id.slice(0, 12) : id;
  }

  /** Human label for one entry of the patchset dropdown.
   *
   * Three flavours after the `PSn` prefix:
   *   * `amended` — same `tip_change` as the previous patchset, just
   *     a different `tip_commit`. The author edited their tip commit
   *     in place (the normal jj amend flow).
   *   * `rewritten` — `parent_patchset` is null and we're not PS1, so
   *     the new tip is neither a descendant of the previous tip nor
   *     the same change. The history was genuinely thrown away.
   *   * (no suffix) — fast-forward: new commits stacked on top of the
   *     previous patchset's tip. Boring continuation, nothing to flag.
   */
  function patchsetLabel(p: import('./../lib/types').Patchset): string {
    let label = `PS${p.n}`;
    if (p.n === current.manifest.current_patchset) label += ' (latest)';
    if (p.parent_patchset == null) {
      if (p.n > 1) label += ' · rewritten';
    } else {
      const prev = current.manifest.patchsets.find(
        (x) => x.n === p.parent_patchset,
      );
      if (prev && prev.tip_change === p.tip_change) label += ' · amended';
    }
    return label;
  }

  /** Re-resolve the manifest's revset against the underlying jj repo,
   *  appending a new patchset if the branch has moved. The server's
   *  SSE event flow will also push the update to other viewers. */
  let refreshing = $state(false);
  async function manualRefresh() {
    if (refreshing) return;
    refreshing = true;
    error = null;
    try {
      await api.refreshReview(repo, current.manifest.review_id);
      await refresh();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      refreshing = false;
    }
  }

  async function refresh() {
    const wasOnLatest = selectedPatchset === current.manifest.current_patchset;
    const compare = compareWith ?? undefined;
    const next = await api.openReview(
      repo,
      current.manifest.review_id,
      selectedPatchset,
      compare,
    );
    // If the user was tracking the latest patchset and a new one just landed,
    // follow it forward; otherwise stay where they are.
    if (wasOnLatest && next.manifest.current_patchset !== selectedPatchset) {
      current = await api.openReview(
        repo,
        current.manifest.review_id,
        next.manifest.current_patchset,
        compare,
      );
      selectedPatchset = current.manifest.current_patchset;
    } else {
      current = next;
    }
  }

  async function selectPatchset(n: number) {
    if (n === selectedPatchset) return;
    // Stepping out of the patchset that compare was anchored at would
    // ask the server to diff a patchset against itself; just leave
    // compare mode in that case rather than silently swallowing the
    // selection.
    const nextCompare = compareWith === n ? null : compareWith;
    saving = true;
    error = null;
    try {
      current = await api.openReview(
        repo,
        current.manifest.review_id,
        n,
        nextCompare ?? undefined,
      );
      selectedPatchset = n;
      compareWith = nextCompare;
      // Discarding the per-commit scope: it was tied to the previous PS.
      scopedChangeId = null;
      scopedDiff = null;
      onviewchange?.(n, nextCompare);
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  /** Switch into (or out of) patchset-compare mode. `n === null`
   *  leaves compare mode and goes back to the normal base..tip view.
   *  Per-commit scoping doesn't compose with patchset-compare (a
   *  commit's diff is base..commit, which has no analogue between
   *  two patchsets), so dropping the scope when entering compare
   *  mode is intentional. */
  async function selectCompareWith(n: number | null) {
    if (n === compareWith) return;
    if (n !== null && n === selectedPatchset) return;
    saving = true;
    error = null;
    try {
      current = await api.openReview(
        repo,
        current.manifest.review_id,
        selectedPatchset,
        n ?? undefined,
      );
      compareWith = n;
      scopedChangeId = null;
      scopedDiff = null;
      onviewchange?.(selectedPatchset, n);
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  /** Auto-refresh on any public state change for this review so the user
   *  sees other authors' published comments / status flips without a
   *  manual reload. Drafts are local-only so they don't trigger events.
   *  `review-branch-moved` flips the local stale flag in place — it
   *  doesn't re-fetch, because nothing the user can see has actually
   *  changed; we just want the Refresh affordance to surface. */
  onMount(() =>
    subscribeEvents((event) => {
      if (
        event.repo !== repo ||
        event.review_id !== current.manifest.review_id
      ) {
        return;
      }
      if (
        event.kind === 'session-published' ||
        event.kind === 'session-discarded' ||
        event.kind === 'review-updated'
      ) {
        void refresh();
      } else if (event.kind === 'review-branch-moved') {
        current = { ...current, is_stale: true };
      }
    }),
  );

  /** Comment-permalink hash. URLs like `…/r/<repo>/<rid>#c-<commentId>`
   *  scroll to that comment on load; we also listen for `hashchange`
   *  so clicking a permalink from elsewhere in the app jumps without a
   *  reload. The file is looked up so `scrollToComment` knows which
   *  FileSlot to mount when the comment is currently virtualized away. */
  function jumpToHash() {
    const hash = window.location.hash;
    if (!hash.startsWith('#c-')) return;
    const commentId = decodeURIComponent(hash.slice(3));
    const comment = [
      ...current.comments,
      ...current.drafts.comments,
    ].find((c) => c.comment_id === commentId);
    if (!comment) return;
    void scrollToComment(comment.comment_id, comment.file ?? null);
  }
  onMount(() => {
    // Wait one frame for FileSlots to register; scrollToComment also
    // retries internally, so this is belt-and-braces.
    requestAnimationFrame(jumpToHash);
    window.addEventListener('hashchange', jumpToHash);
    return () => window.removeEventListener('hashchange', jumpToHash);
  });

  const reviewAnchorIds = $derived({
    change: viewing.tip_change,
    commit: viewing.tip_commit,
  });

  function startCompose(target: ComposerTarget) {
    composing = target;
  }

  function cancelCompose() {
    composing = null;
  }

  async function ensureSession(): Promise<string> {
    if (current.drafts.session) return current.drafts.session.session_id;
    const session = await api.startSession(repo, current.manifest.review_id);
    // Persist locally so the optimistic-update paths below don't have to
    // refetch the review just to learn the new session_id.
    current = { ...current, drafts: { ...current.drafts, session } };
    return session.session_id;
  }

  async function submitComment(input: DraftCommentInput) {
    saving = true;
    error = null;
    try {
      const sid = await ensureSession();
      const editingId = composing?.editing?.commentId;
      const saved = editingId
        ? await api.updateComment(
            repo,
            current.manifest.review_id,
            sid,
            editingId,
            input,
          )
        : await api.createComment(repo, current.manifest.review_id, sid, input);
      // Splice into local drafts instead of refetching the whole review.
      // `openReview` re-runs `jj diff` and resolves every comment's
      // anchor, which on large diffs takes seconds; the local view
      // already has everything except the new draft.
      //
      // The new draft was just authored against the patchset we're
      // viewing, so `anchor: { kind: 'valid' }` is trivially correct
      // until the server-side anchor resolution kicks back in (next
      // SSE event or manual refresh).
      const view: CommentView = {
        ...saved,
        anchor: { kind: 'valid' },
        draft: true,
      };
      const next = editingId
        ? current.drafts.comments.map((c) =>
            c.comment_id === editingId ? view : c,
          )
        : [...current.drafts.comments, view];
      current = {
        ...current,
        drafts: { ...current.drafts, comments: next },
      };
      composing = null;
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  async function saveSummary(next: string | null) {
    saving = true;
    error = null;
    try {
      const updated = await api.updateSummary(
        repo,
        current.manifest.review_id,
        next,
      );
      // Merge in place so we don't lose patchset / diff / comment state.
      current = { ...current, manifest: updated };
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  function startEdit(comment: CommentView) {
    // Re-open the composer at the existing comment's anchor with the
    // body/flag pre-filled. The submit handler picks the PUT path when
    // `composing.editing.commentId` is set.
    const editing = {
      commentId: comment.comment_id,
      body: comment.body,
      flag: comment.flag,
    };
    if (comment.file && comment.lines && comment.side) {
      composing = {
        kind: 'line',
        file: comment.file,
        side: comment.side,
        startLine: comment.lines.start,
        endLine: comment.lines.end,
        editing,
      };
    } else if (comment.file) {
      composing = { kind: 'file', file: comment.file, editing };
    } else {
      composing = { kind: 'review', editing };
    }
  }

  async function submitResponse(input: DraftResponseInput) {
    saving = true;
    error = null;
    try {
      const sid = await ensureSession();
      const saved = await api.createResponse(
        repo,
        current.manifest.review_id,
        sid,
        input,
      );
      const view: ResponseView = { ...saved, draft: true };
      current = {
        ...current,
        drafts: {
          ...current.drafts,
          responses: [...current.drafts.responses, view],
        },
      };
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  async function setStatus(commentId: string, action: import('../lib/types').ResolutionAction) {
    await submitResponse({ in_reply_to: commentId, action, body: '' });
  }

  async function deleteComment(comment: CommentView) {
    if (!confirm('Delete this draft comment?')) return;
    saving = true;
    error = null;
    try {
      await api.deleteComment(
        repo,
        current.manifest.review_id,
        comment.session_id,
        comment.comment_id,
      );
      current = {
        ...current,
        drafts: {
          ...current.drafts,
          comments: current.drafts.comments.filter(
            (c) => c.comment_id !== comment.comment_id,
          ),
        },
      };
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  async function publish() {
    if (!current.drafts.session) return;
    saving = true;
    error = null;
    try {
      await api.publishSession(
        repo,
        current.manifest.review_id,
        current.drafts.session.session_id,
      );
      await refresh();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  /** Visible bottom of the sticky bar stack — i.e. the lowest y in
   *  the viewport that's still covered when the user has scrolled down.
   *  Any scroll target we want flush against the bars has to land
   *  exactly here, not one pixel above (we'd see the previous file's
   *  sticky header in that pixel) and not one pixel below (gap).
   *
   *  Computing this as `header.offsetHeight + bar.offsetHeight` looks
   *  obvious but is wrong: `offsetHeight` includes the header's
   *  border-bottom, while the bar's `top` is bound to the *content*
   *  height var (`--app-header-h`), so the bar overlaps the header's
   *  border by one pixel — covered by the header's higher z-index. Use
   *  the bar's computed `top` (= where it actually sticks) plus its
   *  own offsetHeight, which together describe its real stuck bottom.
   *  Falls back to the header bottom on screens with no comment bar
   *  (the review-list view, etc.). */
  function stickyTop(): number {
    if (typeof document === 'undefined') return 0;
    const bar = document.querySelector('.comment-bar') as HTMLElement | null;
    if (bar) {
      const top = parseFloat(getComputedStyle(bar).top) || 0;
      return top + bar.offsetHeight;
    }
    const header = document.querySelector('header.app') as HTMLElement | null;
    return header?.offsetHeight ?? 0;
  }

  /** Scroll the window so `el`'s top sits just below the sticky bars. */
  function scrollTopOf(el: HTMLElement): void {
    const target = el.getBoundingClientRect().top + window.scrollY - stickyTop();
    window.scrollTo({ top: Math.max(0, target), behavior: 'auto' });
  }

  async function scrollToFile(path: string) {
    const sel = `[data-file-path="${CSS.escape(path)}"]`;
    const target = document.querySelector(sel) as HTMLElement | null;
    if (!target) return;
    // On phones the tree is an overlay drawer — close it before we
    // start scrolling so the user actually sees the diff they jumped
    // to (and the layout has already settled into one-pane mode).
    if (
      typeof window !== 'undefined' &&
      window.matchMedia('(max-width: 640px)').matches
    ) {
      treeCollapsed = true;
    }
    scrollTopOf(target);
    // Slots above the target are virtualized placeholders sized from
    // an estimate. As they enter the viewport during the scroll, the
    // IntersectionObserver mounts the real FileDiff and the
    // ResizeObserver updates `lastKnownHeight` — the document layout
    // shifts and the slot we wanted ends up off-screen. Re-aim across
    // a handful of frames until the slot's position is stable.
    let stableFrames = 0;
    let lastTop = Number.NaN;
    for (let i = 0; i < 30 && stableFrames < 3; i++) {
      await new Promise((r) => requestAnimationFrame(r));
      const cur = document.querySelector(sel) as HTMLElement | null;
      if (!cur) return;
      const top = cur.getBoundingClientRect().top;
      if (Number.isFinite(lastTop) && Math.abs(top - lastTop) < 0.5) {
        stableFrames++;
      } else {
        stableFrames = 0;
        scrollTopOf(cur);
      }
      lastTop = top;
    }
  }

  async function discard() {
    if (!current.drafts.session) return;
    if (!confirm('Discard this draft session? Your drafts will be marked discarded.')) {
      return;
    }
    saving = true;
    error = null;
    try {
      await api.discardSession(
        repo,
        current.manifest.review_id,
        current.drafts.session.session_id,
      );
      await refresh();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }
</script>

<section class="header">
  <h2>{current.manifest.review_id}</h2>
  <p class="muted">
    {#if current.manifest.bookmark}bookmark: <strong>{current.manifest.bookmark}</strong> ·{/if}
    revset: <code>{current.manifest.revset}</code>
    · by <strong>{current.manifest.created_by}</strong>
  </p>
  <p class="muted patchset-row">
    {#if current.manifest.patchsets.length > 1}
      <label>
        Patchset
        <select
          value={selectedPatchset}
          onchange={(e) =>
            selectPatchset(Number((e.currentTarget as HTMLSelectElement).value))}
        >
          {#each current.manifest.patchsets as p (p.n)}
            <option value={p.n}>{patchsetLabel(p)}</option>
          {/each}
        </select>
      </label>
      <!-- "compare against" lives next to the patchset selector so the
           pair is read as one unit: "show me patchset N against
           [base / patchset M]". -->
      <label>
        compared to
        <select
          value={compareWith ?? ''}
          onchange={(e) => {
            const v = (e.currentTarget as HTMLSelectElement).value;
            selectCompareWith(v === '' ? null : Number(v));
          }}
        >
          <option value="">base</option>
          {#each current.manifest.patchsets as p (p.n)}
            {#if p.n !== selectedPatchset}
              <option value={p.n}>PS{p.n}</option>
            {/if}
          {/each}
        </select>
      </label>
      ·
    {/if}
    {#if compareWith !== null}
      <span class="compare-banner">
        Comparing <strong>PS{compareWith}</strong> →
        <strong>PS{selectedPatchset}</strong>
      </span>
    {:else}
      base <code>{short(viewing.base_change)}</code> → tip
      <code>{short(viewing.tip_change)}</code>
    {/if}
    {#if current.is_stale || refreshing}
      <button
        type="button"
        class="refresh-btn"
        onclick={manualRefresh}
        disabled={refreshing}
        title="The branch has moved since the latest patchset was recorded — refresh to capture it"
      >
        {refreshing ? 'Refreshing…' : '↻ Refresh'}
      </button>
    {/if}
  </p>
</section>

<ReviewSummary
  summary={current.manifest.summary}
  editable={!!viewer && viewer === current.manifest.created_by}
  {saving}
  onsave={saveSummary}
/>

{#if error}
  <p class="error">{error}</p>
{/if}

<div class="review-layout">
  <!-- The tree pane stays mounted and is toggled via CSS. Unmounting it
       (the old `{#if}` shape) rebuilt the full FileTree on every expand,
       which for a 100-file review tipped past a second of mount work. -->
  <!-- Backdrop is only visible on phones (see CSS); on desktop the tree
       is in-flow and this element collapses to nothing. Implemented as
       a button so keyboard + screen-reader users can dismiss it too. -->
  {#if !treeCollapsed}
    <button
      type="button"
      class="tree-backdrop"
      aria-label="Close file tree"
      onclick={() => (treeCollapsed = true)}
    ></button>
  {/if}
  <aside
    class="tree-pane"
    class:hidden={treeCollapsed}
    style:width="{treeWidth}px"
  >
    <FileTree files={visibleFiles} onselect={scrollToFile}>
      {#snippet headerLeft()}
        <button
          class="tree-toggle"
          title="Hide files"
          onclick={() => (treeCollapsed = true)}
        >
          ◂
        </button>
      {/snippet}
    </FileTree>
  </aside>
  {#if treeCollapsed}
    <button
      class="tree-reopen"
      title="Show files"
      onclick={() => (treeCollapsed = false)}
    >
      ▸
    </button>
  {:else}
    <div
      class="tree-resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize file tree"
      onpointerdown={startResize}
    ></div>
  {/if}
  <div class="main-pane">
    <!-- Sticky bar grouping every comment-level control: lifecycle +
         severity filter chips on the left, prev/next nav and the
         comments-only toggle on the right. Sticky at the top of the
         scroll container so it remains visible as the user scrolls
         through long file diffs. -->
    <div class="comment-bar" bind:this={commentBarEl} role="group" aria-label="Comment controls">
      <div class="filter-chips">
        <span class="label">Status</span>
        <button
          type="button"
          class="chip status-draft"
          class:on={filterStatus.draft}
          aria-pressed={filterStatus.draft}
          onclick={() => (filterStatus = { ...filterStatus, draft: !filterStatus.draft })}
        >Draft</button>
        <button
          type="button"
          class="chip status-open"
          class:on={filterStatus.open}
          aria-pressed={filterStatus.open}
          onclick={() => (filterStatus = { ...filterStatus, open: !filterStatus.open })}
        >Open</button>
        <button
          type="button"
          class="chip status-resolved"
          class:on={filterStatus.resolved}
          aria-pressed={filterStatus.resolved}
          onclick={() => (filterStatus = { ...filterStatus, resolved: !filterStatus.resolved })}
        >Resolved</button>
        <span class="sep" aria-hidden="true"></span>
        <span class="label">Severity</span>
        <button
          type="button"
          class="chip flag-must-do"
          class:on={filterFlag['must-do']}
          aria-pressed={filterFlag['must-do']}
          onclick={() => (filterFlag = { ...filterFlag, 'must-do': !filterFlag['must-do'] })}
        >Must do</button>
        <button
          type="button"
          class="chip flag-suggestion"
          class:on={filterFlag.suggestion}
          aria-pressed={filterFlag.suggestion}
          onclick={() => (filterFlag = { ...filterFlag, suggestion: !filterFlag.suggestion })}
        >Suggestion</button>
        <button
          type="button"
          class="chip flag-other"
          class:on={filterFlag.other}
          aria-pressed={filterFlag.other}
          onclick={() => (filterFlag = { ...filterFlag, other: !filterFlag.other })}
        >Other</button>
      </div>
      <div class="comment-bar-actions">
        {#if orderedComments.length > 0}
          <div class="comment-nav" role="group" aria-label="Comment navigation">
            <button
              type="button"
              onclick={navPrev}
              title="Previous comment"
              aria-label="Previous comment"
            >‹</button>
            <span class="position" aria-live="polite">
              {navPosition || '–'}/{orderedComments.length}
            </span>
            <button
              type="button"
              onclick={navNext}
              title="Next comment"
              aria-label="Next comment"
            >›</button>
          </div>
        {/if}
        <button
          type="button"
          onclick={toggleDiffs}
          title={diffsCollapsed ? 'Show file diffs' : 'Hide file diffs, leave only comments'}
        >
          {diffsCollapsed ? 'Show diffs' : 'Comments only'}
        </button>
      </div>
    </div>
    <!-- Hidden in compare mode: the commits panel scopes the file diff
         to base..commit for a single commit, which has no meaning
         between two patchsets — and per-commit comment counts would
         mix the from/to patchset comments confusingly. The selectors
         above are enough to switch back to a single-patchset view. -->
    {#if compareWith === null}
      <CommitsPanel
        commits={current.commits}
        comments={visibleComments}
        selectedChangeId={scopedChangeId}
        onselect={selectCommit}
      />
    {/if}

    {#if loadingDiff}
      <div class="diff-loading" role="status" aria-live="polite">
        <span class="spinner" aria-hidden="true"></span>
        Loading diff for <code>{loadingDiffLabel}</code>…
      </div>
    {/if}

    <section class="review-comments">
  <header>
    <h3>Review-wide comments</h3>
    <button
      type="button"
      class="primary"
      onclick={() => startCompose({ kind: 'review' })}
      disabled={composing?.kind === 'review'}
    >
      Add comment
    </button>
  </header>
  {#if reviewComments.length > 0}
    <CommentThread
      comments={reviewComments}
      responses={allResponses}
      {saving}
      onreply={submitResponse}
      onstatus={setStatus}
      ondelete={deleteComment}
      onedit={startEdit}
    />
  {:else if !composing || composing.kind !== 'review'}
    <p class="muted">No review-wide comments yet.</p>
  {/if}
  {#if composing && composing.kind === 'review'}
    <CommentComposer
      target={composing}
      anchorIds={reviewAnchorIds}
      {saving}
      oncancel={cancelCompose}
      onsubmit={submitComment}
    />
  {/if}
</section>

{#if orderedFiles.length === 0}
      <p class="muted">No files changed.</p>
    {:else if visibleFiles.length === 0}
      <p class="muted">No files have comments.</p>
    {:else}
      {#each visibleFiles as f (f.path)}
        <!-- composing is narrowed to the targeted file only; other slots
             receive `null` and don't churn when the composer opens
             elsewhere. forceRender keeps the file hosting the composer
             alive in the DOM regardless of viewport, so the inline
             composer doesn't get virtualized out from under the user. -->
        <FileSlot
          {repo}
          reviewId={current.manifest.review_id}
          file={f}
          patchset={viewingFor}
          {compareWith}
          comments={visibleComments}
          responses={allResponses}
          composing={composing &&
          'file' in composing &&
          composing.file === f.path
            ? composing
            : null}
          forceRender={!!(composing &&
            'file' in composing &&
            composing.file === f.path)}
          compact={diffsCollapsed}
          {saving}
          onstartcompose={startCompose}
          oncancelcompose={cancelCompose}
          onsubmit={submitComment}
          onreply={submitResponse}
          onstatus={setStatus}
          ondelete={deleteComment}
          onedit={startEdit}
        />
      {/each}
    {/if}
  </div>
</div>

<style>
  .header {
    margin-bottom: 16px;
  }

  /* Small inline button sitting at the end of the patchset row. Padded
   * smaller than the default button so it sits next to the inline
   * `<code>` tags without dominating the row. */
  .refresh-btn {
    margin-left: 12px;
    padding: 2px 10px;
    font-size: 12px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--link);
    cursor: pointer;
  }

  .refresh-btn:hover {
    background: var(--link-bg);
  }

  .refresh-btn:disabled {
    opacity: 0.6;
    cursor: default;
  }

  /* Compare-mode badge: replaces the usual "base xxx → tip yyy"
   * text in the patchset row so the user always sees they're
   * looking at a patchset-to-patchset delta, not the full review. */
  .compare-banner {
    display: inline-block;
    padding: 1px 8px;
    border-radius: 3px;
    background: var(--link-bg);
    color: var(--link);
    font-size: 12px;
  }

  .compare-banner strong {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  }

  .review-layout {
    display: flex;
    align-items: flex-start;
    gap: 0;
  }

  /* Sticky bar of comment-level controls — filter chips on the left,
   * prev/next nav and the diff-collapse toggle on the right. Sits
   * directly below the app header (`top: var(--app-header-h)`) so it
   * stays in view as the user scrolls through the file diffs. The
   * solid background + bottom border keep the page content from
   * showing through. */
  .comment-bar {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px 12px;
    /* Spacing below the bar lives inside the box (not as margin) so
     * `offsetHeight` reflects the full visual footprint. Anything we
     * measure via offsetHeight is what `stickyTop` then subtracts when
     * positioning scroll targets — a margin here would leak as a
     * transparent strip below the bar where the previous file's
     * sticky header could peek through. */
    padding: 8px 0 16px;
    background: var(--bg);
    border-bottom: 1px solid var(--border);
    position: sticky;
    top: var(--app-header-h);
    z-index: 20;
    font-size: 12px;
  }

  .comment-bar .filter-chips {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 6px;
    flex: 1 1 auto;
    min-width: 0;
  }

  .comment-bar-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex: 0 0 auto;
  }

  .comment-bar-actions .comment-nav {
    display: flex;
    align-items: center;
    gap: 4px;
  }

  .comment-bar-actions .comment-nav button {
    padding: 2px 8px;
    font-size: 14px;
    line-height: 1;
  }

  .comment-bar-actions .comment-nav .position {
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
    min-width: 3.5em;
    text-align: center;
  }

  .comment-bar-actions > button {
    padding: 4px 10px;
    font-size: 12px;
  }

  .comment-bar .label {
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
    font-size: 11px;
    margin-right: 2px;
  }

  .comment-bar .sep {
    width: 1px;
    height: 16px;
    background: var(--border);
    margin: 0 6px;
  }

  .comment-bar .chip {
    border: 1px solid var(--border);
    border-radius: 9999px;
    padding: 2px 10px;
    font-size: 11px;
    line-height: 1.5;
    background: var(--bg);
    color: var(--text-faint);
    cursor: pointer;
    /* When off, render in a desaturated neutral; when on (via .on) we
     * adopt the same tint flags use elsewhere so the bar reads at a
     * glance. */
  }

  .comment-bar .chip:hover {
    background: var(--bg-panel);
  }

  .comment-bar .chip.on.status-draft {
    background: var(--attention-bg);
    color: var(--attention-text);
    border-color: var(--attention-border);
  }

  .comment-bar .chip.on.status-open {
    background: var(--link-bg);
    color: var(--link);
    border-color: var(--link);
  }

  .comment-bar .chip.on.status-resolved {
    background: var(--success-bg);
    color: var(--success-text);
    border-color: var(--success-text);
  }

  .comment-bar .chip.on.flag-must-do {
    background: var(--error-bg);
    color: var(--error-text);
    border-color: var(--error-text);
  }

  .comment-bar .chip.on.flag-suggestion {
    background: var(--link-bg);
    color: var(--link);
    border-color: var(--link);
  }

  .comment-bar .chip.on.flag-other {
    background: var(--bg-elevated);
    color: var(--text-muted);
    border-color: var(--text-faint);
  }

  /* On phones the header wraps to two lines so a fixed `top` offset
   * would overlap content. Drop the sticky behavior — the bar still
   * sits above the commits panel, just not pinned. */
  @media (max-width: 640px) {
    .comment-bar {
      position: static;
    }
  }

  .tree-pane {
    flex: 0 0 auto;
    position: sticky;
    top: calc(var(--app-header-h) + 16px);
    max-height: calc(100vh - var(--app-header-h) - 32px);
    /* Match the CommitsPanel's 16px top margin so the two panels align
     * along their top edges before sticky kicks in. */
    margin-top: 16px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    display: flex;
    flex-direction: column;
    overflow: hidden;
  }

  /* Hidden state for the tree pane: kept in the DOM (so its FileTree
   * children don't have to rebuild on re-open) but pulled out of layout. */
  .tree-pane.hidden {
    display: none;
  }

  /* The backdrop is invisible / non-blocking on desktop — the tree is
   * in-flow there. On phones (see media query below) it becomes a
   * dimming layer over the page that closes the drawer on tap. */
  .tree-backdrop {
    display: none;
  }

  /* --- Phone layout ------------------------------------------------- */
  @media (max-width: 640px) {
    .review-layout {
      display: block;
    }

    /* Tree turns into a slide-in drawer over the diff. width: 80vw with
     * a cap so it doesn't get absurd on slightly-wider phones in
     * landscape orientation. The inline `style:width` from the desktop
     * resizer is overridden here. */
    .tree-pane {
      position: fixed;
      top: var(--app-header-h);
      left: 0;
      bottom: 0;
      margin: 0;
      width: 80vw !important;
      max-width: 320px;
      border-radius: 0;
      border-left: none;
      border-top: none;
      border-bottom: none;
      box-shadow: 0 0 24px rgba(0, 0, 0, 0.25);
      z-index: 25;
    }

    .tree-pane.hidden {
      /* Slide off-screen instead of display:none so a future open
       * animates and so the FileTree's state stays alive. */
      display: flex;
      transform: translateX(-100%);
    }

    .tree-backdrop {
      display: block;
      position: fixed;
      inset: var(--app-header-h) 0 0 0;
      background: rgba(0, 0, 0, 0.35);
      border: none;
      padding: 0;
      cursor: pointer;
      z-index: 24;
    }

    .tree-resizer,
    .tree-reopen {
      display: none;
    }

    .main-pane {
      margin-left: 0;
    }
  }

  /* The collapse toggle gets passed into FileTree's header via a snippet,
   * so this rule needs to apply across component boundaries. */
  :global(.tree-toggle),
  .tree-reopen {
    width: 22px;
    height: 22px;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg);
    color: var(--text-muted);
    cursor: pointer;
    font-size: 12px;
    line-height: 18px;
  }

  :global(.tree-toggle:hover),
  .tree-reopen:hover {
    background: var(--bg-elevated);
  }

  .tree-reopen {
    position: sticky;
    top: calc(var(--app-header-h) + 16px);
    margin-right: 8px;
    flex: 0 0 auto;
  }

  .tree-resizer {
    flex: 0 0 6px;
    cursor: col-resize;
    position: sticky;
    top: calc(var(--app-header-h) + 16px);
    height: calc(100vh - var(--app-header-h) - 32px);
    background: transparent;
    margin: 0 4px;
  }

  .tree-resizer:hover,
  .tree-resizer:active {
    background: var(--link);
    opacity: 0.4;
  }

  .main-pane {
    flex: 1;
    min-width: 0;
    margin-left: 8px;
  }

  .diff-loading {
    display: flex;
    align-items: center;
    gap: 8px;
    margin: 12px 0;
    padding: 10px 12px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-size: 13px;
    color: var(--text-muted);
  }

  .diff-loading code {
    background: var(--bg-elevated);
    padding: 1px 5px;
    border-radius: 3px;
    font-size: 12px;
  }

  .diff-loading .spinner {
    width: 12px;
    height: 12px;
    border: 2px solid var(--border);
    border-top-color: var(--link);
    border-radius: 50%;
    animation: diff-loading-spin 0.7s linear infinite;
  }

  @keyframes diff-loading-spin {
    to { transform: rotate(360deg); }
  }

  .review-comments {
    margin: 16px 0;
  }

  .review-comments header {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-bottom: 8px;
  }

  .review-comments h3 {
    margin: 0;
  }
</style>
