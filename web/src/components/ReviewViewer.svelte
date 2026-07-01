<script lang="ts">
  import { onMount, setContext, tick, untrack } from 'svelte';
  import { SvelteMap, SvelteSet } from 'svelte/reactivity';
  import { ApiError, api } from '../lib/api';
  import { subscribe as subscribeEvents } from '../lib/events';
  import type {
    AnnotationInput,
    AnnotationView,
    CommentView,
    CommitDiffView,
    ComposerTarget,
    DraftCommentInput,
    DraftResponseInput,
    FileChange,
    Patchset,
    PatchsetCompareView,
    PatchsetPair,
    ResponseView,
    ReviewView,
  } from '../lib/types';
  import type { AnnotationComposerTarget } from './AnnotationComposer.svelte';
  import { sortFilesLikeTree } from '../lib/tree';
  import { setTokenizationPaused } from '../lib/highlight.svelte';
  import { resolutionFor } from '../lib/resolution';
  import { createFoldStore, type FoldStore } from '../lib/foldStore';
  import { parseLineRangeHash } from '../lib/linkHash';
  import { searchReview, selectorForMatch, type SearchMatch } from '../lib/search';
  import {
    annotationComposerSurvivesPatchset,
    composerSurvivesPatchset,
    pruneFileDiffCache,
  } from '../lib/patchsetSwap';
  import { compareBaseCommit as compareBaseCommitFor } from '../lib/compareBase';
  import Chevron from './Chevron.svelte';
  import CommitsPanel from './CommitsPanel.svelte';
  import FileSlot from './FileSlot.svelte';
  import FileTree from './FileTree.svelte';
  import RevId from './RevId.svelte';
  import ReviewSummary from './ReviewSummary.svelte';

  /** State + action callbacks for the controls that App.svelte renders in
   *  the sticky top bar. Re-emitted whenever any of the underlying fields
   *  change; null only when the review viewer is unmounted.
   *
   *  The top bar is two-row in review mode: row 1 carries the global app
   *  controls (back, commit-nav, drafts) and row 2 carries review-level
   *  state (title, filter chips, comment-nav, comments-only). Putting
   *  the comment-nav in a fixed-at-top container — rather than a sticky
   *  bar that shares the scroll viewport — keeps the prev / next buttons
   *  pinned in one spot so repeat clicks don't chase the bar around. */
  type StatusBucket = 'draft' | 'open' | 'resolved';
  type FlagBucket = 'must-do' | 'suggestion' | 'question';
  export interface ReviewToolbarState {
    /** Compact review identity for the row-2 left side: `#N name` plus
     *  whether to render the Archived pill. Replaces the in-body header
     *  `<h2>` so the title is visible while the user scrolls. */
    title: {
      number: number;
      name: string;
      archived: boolean;
      /** PR provenance, when the review was created via the
       *  GitHub PR import flow. Drives the "View on GitHub" link
       *  in the review header. `null` for native kata reviews. */
      githubPr: { number: number; html_url: string } | null;
    } | null;
    /** Draft session controls. Null when the user has no open drafts
     *  at all (neither draft comments nor draft replies). `count` is
     *  the combined total — comments + replies — so the user sees a
     *  non-zero indicator when the session has anything to publish.
     *
     *  `nav` is non-null only when there are draft comments to step
     *  through. Draft replies are anchored inside a comment thread
     *  and don't have an independent scroll target, so prev/next
     *  walks comments only. A session with only draft replies still
     *  surfaces publish/discard but no nav. */
    drafts: {
      count: number;
      saving: boolean;
      publish: () => Promise<void>;
      /** Set when the review is bound to a GitHub PR
       *  (`manifest.github_pr` is present). The toolbar swaps the
       *  generic Publish button for a "Publish to GitHub" path
       *  that prompts for the GH review event + optional body and
       *  posts via the publish-github endpoint. `null` for native
       *  kata reviews. */
      publishToGithub:
        | ((event: 'COMMENT' | 'APPROVE' | 'REQUEST_CHANGES', body?: string) => Promise<boolean>)
        | null;
      /** PR URL on github.com for the "View on GitHub" affordance
       *  in the toolbar. Mirrors `publishToGithub`'s presence —
       *  null for native reviews. */
      githubPrUrl: string | null;
      discard: () => Promise<void>;
      nav: {
        position: number;
        prev: () => void;
        next: () => void;
      } | null;
    } | null;
    /** GitHub-review actions that apply regardless of whether the
     *  user has drafts queued: quick-approve, request-changes, and
     *  refresh-from-github. `null` for reviews that aren't bound to
     *  a github.com PR (native kata reviews have nothing to do here).
     *
     *  Separate from `drafts.publishToGithub` on purpose. Drafts are
     *  the "publish these queued comments (with an optional review
     *  event)" path; these are the verdict-only + housekeeping path
     *  that must be reachable when the user has nothing to say. */
    githubActions: {
      approve: () => Promise<boolean>;
      requestChanges: (body: string) => Promise<boolean>;
      refresh: () => Promise<void>;
      /** True while an approve / request-changes publish is in
       *  flight. Grays out the buttons so the user can't double-
       *  submit, and drives the "Approving…" / "Requesting
       *  changes…" in-flight labels. Mirrors `drafts.saving` for
       *  the split button. */
      saving: boolean;
      /** True while a refresh-from-github is in flight. Drives the
       *  "Refreshing…" label on the refresh button. All three
       *  buttons are disabled during refresh — a verdict submit
       *  mid-refresh would race the head SHA and trip head-drift
       *  refusal, so we serialise. */
      refreshing: boolean;
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
    /** Comment lifecycle / severity filter, plus the "filter hides N"
     *  hint surfaced when every comment is filtered out. `null` while
     *  the review has zero comments (no chips needed). */
    filter: {
      status: Record<StatusBucket, boolean>;
      flag: Record<FlagBucket, boolean>;
      toggleStatus: (key: StatusBucket) => void;
      toggleFlag: (key: FlagBucket) => void;
      /** When > 0, every comment is hidden by the chip combination. The
       *  shell renders a one-click "show all" hint. */
      hiddenCount: number;
      reset: () => void;
    } | null;
    /** Prev / next nav across the visible-after-filter comments.
     *  `null` while no comments would be in the nav. Position 0 means
     *  no current selection; 1..total points at one. */
    comments: {
      total: number;
      position: number;
      prev: () => void;
      next: () => void;
    } | null;
    /** Three-mode view selector: `both`, `diffs` (hide comments), or
     *  `comments` (hide diffs — old "compact" mode). `null` if the
     *  review has nothing meaningful to show in any of the modes. */
    view: {
      mode: 'both' | 'diffs' | 'comments';
      set: (m: 'both' | 'diffs' | 'comments') => void;
    } | null;
    /** Patchset selector + compare-with selector. `null` for reviews
     *  with only one patchset (nothing to switch between). Lives in
     *  the row-2 header next to the title — the dropdowns are the
     *  primary controls for shifting which round the diff is showing
     *  against, so they sit alongside the review identity rather than
     *  in the body. */
    patchsets: {
      options: { n: number; label: string }[];
      selected: number;
      compareWith: number | null;
      select: (n: number) => void;
      selectCompareWith: (n: number | null) => void;
    } | null;
    /** File-tree visibility. The top bar surfaces this so phones can
     *  toggle the drawer-style tree without scrolling. */
    tree: { collapsed: boolean; toggle: () => void };
    /** Cmd+F-style in-app search across diff lines, comments, and
     *  annotations. The shell renders the icon button when `open`
     *  is false, the expanded strip when true. State + callbacks
     *  all live here so the search engine and the match navigation
     *  stay co-located with the rest of the review's reactive
     *  state. */
    search: {
      open: boolean;
      query: string;
      total: number;
      position: number;
      loading: boolean;
      onQueryInput: (q: string) => void;
      onNext: () => void;
      onPrev: () => void;
      onOpen: () => void;
      onClose: () => void;
    };
    /** Lifecycle actions surfaced as a kebab next to the title in
     *  row-2 of App.svelte's header. Always non-null once whoami
     *  resolves — non-creators see the menu with items disabled and
     *  a "(creator only)" suffix, so the gate is visible rather
     *  than mysterious. `null` only when `viewer` is empty (whoami
     *  hasn't landed yet); the kebab stays out of the way until we
     *  know who's looking. */
    actions: {
      archive: () => Promise<void>;
      delete: () => Promise<void>;
      archived: boolean;
      busy: boolean;
      /** True when this viewer is the review's creator. */
      manageable: boolean;
    } | null;
  }

  interface Props {
    repo: string;
    view: ReviewView;
    /** Currently signed-in viewer's author identity. Used (with
     *  `isAdmin`) to gate creator-only affordances. Empty string
     *  before whoami has resolved (treated as not-creator). */
    viewer: string;
    /** Whether the viewer is a global admin — passes every
     *  creator-only gate (archive/delete, edit summary/revset,
     *  annotations) exactly as the creator would. The server enforces
     *  the same; this only mirrors it so the controls are offered. */
    isAdmin?: boolean;
    /** Patchset to start on. Undefined means "the latest". */
    initialPatchset?: number;
    /** Patchset-compare to start on: when set, the viewer opens in
     *  compare mode showing the patchset[compare] → patchset[ps]
     *  delta. Undefined for the normal base..tip view. */
    initialCompareWith?: number;
    /** Compare-mode commit selection: when set together with
     *  `initialCompareWith`, the viewer opens the per-commit interdiff
     *  for this change-id from the pair list. Ignored outside compare
     *  mode. Undefined means "cumulative view across all commits". */
    initialCommit?: string;
    /** Non-compare-mode commit-panel scoping: when set, the file
     *  panel renders only the named commit's diff (`parent..commit`)
     *  instead of the whole review. Ignored in compare mode (the
     *  pair-list selection takes its place). Undefined means
     *  "show the whole review". */
    initialScope?: string;
    /** Opt-in debug affordances (URL `?debug`). Currently a per-file
     *  "show jj equivalent" icon in the file header. Threaded down
     *  through a context so deeply-nested components can read it
     *  without prop-drilling. */
    debug?: boolean;
    /** Fires when the user picks a different patchset, compare
     *  target, per-commit compare selection, or non-compare scoped
     *  commit. App.svelte mirrors all four fields into the URL and
     *  pushes a history entry per change so the browser back button
     *  undoes the action. `null` for `compare`/`commit`/`scope`
     *  means "no selection." */
    onviewchange?: (state: {
      patchset?: number;
      compareWith?: number | null;
      commit?: string | null;
      scope?: string | null;
    }) => void;
    /** Reports toolbar state up to the app shell so the publish / discard
     *  controls and the diff-collapse toggle can live in the always-visible
     *  top bar instead of scrolling away with the page. */
    ontoolbarchange?: (bar: ReviewToolbarState | null) => void;
  }
  let {
    repo,
    view,
    viewer,
    isAdmin = false,
    initialPatchset,
    initialCompareWith,
    initialCommit,
    initialScope,
    debug = false,
    onviewchange,
    ontoolbarchange,
  }: Props = $props();

  // Expose the debug flag via Svelte context so leaf components
  // (FileDiff) can read it without prop-drilling. Set once on mount
  // — the flag is part of the URL state, so a `?debug` toggle
  // remounts ReviewViewer (via the {#key} in App.svelte) and the
  // new context value flows through.
  setContext<boolean>('kata-debug', debug);

  // Persistent fold/expand state, keyed by (repo, review number). The
  // store outlives any single component instance, so a file you
  // collapsed survives both scrolling-induced re-mounts and full page
  // refreshes. Constructed once per viewer mount — the {#key} in
  // App.svelte already remounts the viewer when repo or review number
  // changes, so we don't need to react.
  // svelte-ignore state_referenced_locally
  const foldStore = createFoldStore(repo, view.manifest.number);
  setContext<FoldStore>('kata-fold-store', foldStore);
  // Shared reactive version counter so a fold toggle in any one
  // component (e.g. CommentThread's per-thread chevron) wakes every
  // other consumer (e.g. HunkLines' aggregate marker, which needs
  // to re-filter its threads) — the foldStore itself is a plain JS
  // object with no built-in reactivity. Consumers call
  // `version.read()` to register a dependency and `version.bump()`
  // after a mutation.
  let foldVersion = $state(0);
  setContext<{ read: () => number; bump: () => void }>('kata-fold-version', {
    read: () => foldVersion,
    bump: () => {
      foldVersion++;
    },
  });
  // Per-session set of comment / annotation ids whose unread-replies
  // force-expand the user has explicitly dismissed by clicking a fold
  // control on the thread. `hasUnreadReplies` consults this so that
  // clicking fold on a force-expanded thread actually hides it —
  // without it the click was a silent no-op (target=true matched the
  // stored=true, no write happened, and the unread force kept the
  // thread visible). Session-local: a fresh page load resurfaces the
  // "since you were here" highlight until the user acknowledges
  // again, which is what we want for that surfacing to keep working.
  const acknowledgedUnread = new SvelteSet<string>();
  setContext<SvelteSet<string>>('kata-acknowledged-unread', acknowledgedUnread);
  // File paths whose CommentThread currently has an active reply
  // composer (the user clicked Reply and may be mid-typing). The
  // composer body lives in local component state, so if FileSlot
  // virtualises the FileDiff away mid-draft the typed text is
  // lost. CommentThread updates this set on reply start / cancel /
  // submit; FileSlot's `forceRender` (below) keeps the slot
  // mounted while its path is in the set.
  const filesWithReplyInProgress = new SvelteSet<string>();
  setContext<SvelteSet<string>>(
    'kata-files-with-reply',
    filesWithReplyInProgress,
  );
  // Garbage-collect entries that no longer match anything in this
  // review — renamed files, deleted comments, dropped commits would
  // otherwise grow the per-review blob indefinitely. One-shot on
  // mount: the lifetimes we care about (page-reload survivability)
  // are already covered, and the blob is small enough that intra-
  // session orphans are harmless until the next reload.
  onMount(() => {
    foldStore.prune('file', view.diff.files.map((f) => f.path));
    // `comment` kind now stores per-thread fold state, keyed by the
    // top-level comment's id OR an annotation's id (they're separate
    // id spaces but share the same fold semantics). Keep both kinds
    // of ids alive across the prune.
    foldStore.prune('comment', [
      ...view.comments.map((c) => c.comment_id),
      ...(view.annotations ?? []).map((a) => a.annotation_id),
    ]);
    foldStore.prune('commit', view.commits.map((c) => c.commit_id));
  });

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
  /** Annotation composer state, independent of `composing` so the two
   *  flows don't collide. Annotations have their own component (no
   *  flag, no draft session) and only the review creator can author
   *  them — guarded by `canAnnotate` below. */
  let composingAnnotation: AnnotationComposerTarget | null = $state.raw(null);
  let saving = $state(false);
  let error: string | null = $state(null);

  // Reactive context the comment composer reads to warn when a
  // comment is being written against a patchset that's no longer
  // the latest — its anchor may drift once the reader moves to a
  // newer round. A function (not a value) so consumers re-read it
  // as `selectedPatchset` / the manifest change; returns null when
  // the viewed patchset *is* the latest.
  setContext<() => { viewing: number; latest: number } | null>(
    'kata-patchset-warning',
    () => {
      const latest = current.manifest.current_patchset;
      return selectedPatchset < latest
        ? { viewing: selectedPatchset, latest }
        : null;
    },
  );

  // --- View mode ------------------------------------------------------
  // Three mutually-exclusive display modes: diffs + comments (default),
  // diffs with comments collapsed (compact reading), and comments only
  // (the old "compact" mode that hides the diff hunks and lists
  // comments flat). The state is the enum; the downstream booleans
  // `showDiffs` / `showComments` / `defaultThreadsCollapsed` are
  // derived so child components keep their simple props.
  //
  // `diffs` no longer *hides* comments — it renders them collapsed
  // with a clickable gutter marker on the anchor line. The user can
  // still expand any individual thread; the per-anchor fold state
  // persists in foldStore under the `thread` kind.
  type ViewMode = 'both' | 'diffs' | 'comments';
  const VIEW_KEY = 'kata:viewMode';
  function readViewMode(): ViewMode {
    if (typeof localStorage === 'undefined') return 'both';
    const raw = localStorage.getItem(VIEW_KEY);
    if (raw === 'both' || raw === 'diffs' || raw === 'comments') return raw;
    return 'both';
  }
  let viewMode = $state<ViewMode>(readViewMode());
  const showDiffs = $derived(viewMode !== 'comments');
  // `diffs` mode hides the comment UI entirely so the reviewer can
  // read the code without distraction; `both` and `comments` render
  // the threads. Per-line + comment buttons, inline threads, file-
  // and orphan-thread blocks, and outdated chevrons all gate on
  // this — diffs mode is pure code.
  const showComments = $derived(viewMode !== 'diffs');
  /** Default per-thread collapse state when the user hasn't toggled a
   *  specific anchor yet. `diffs` mode wants every thread collapsed
   *  so the diff stays clean; `both` mode wants them expanded so the
   *  conversation reads inline. */
  const defaultThreadsCollapsed = $derived(viewMode === 'diffs');
  $effect(() => {
    if (typeof localStorage === 'undefined') return;
    localStorage.setItem(VIEW_KEY, viewMode);
  });

  // --- Side-by-side split -------------------------------------------------
  // Fraction of width occupied by the base (left) side in the SBS view —
  // the tip side gets the rest. Shared across every SBS hunk on the page
  // so dragging one divider rebalances them all. 0.5 = even split;
  // SBS_SNAP keeps the drag soft-locked to the middle when the user is
  // close, so resetting to standard takes no precision.
  const SBS_SPLIT_KEY = 'kata:sbsSplit';
  const SBS_MIN = 0.15;
  const SBS_MAX = 0.85;
  const SBS_SNAP = 0.03;
  function readSbsSplit(): number {
    if (typeof localStorage === 'undefined') return 0.5;
    try {
      const raw = localStorage.getItem(SBS_SPLIT_KEY);
      if (!raw) return 0.5;
      const v = parseFloat(raw);
      if (!Number.isFinite(v)) return 0.5;
      return Math.max(SBS_MIN, Math.min(SBS_MAX, v));
    } catch {
      return 0.5;
    }
  }
  let sbsSplit = $state(readSbsSplit());
  $effect(() => {
    if (typeof localStorage === 'undefined') return;
    localStorage.setItem(SBS_SPLIT_KEY, String(sbsSplit));
  });
  function setSbsSplit(next: number) {
    const clamped = Math.max(SBS_MIN, Math.min(SBS_MAX, next));
    sbsSplit = Math.abs(clamped - 0.5) < SBS_SNAP ? 0.5 : clamped;
  }

  // --- Comment filter -------------------------------------------------
  // Two independent dimensions: lifecycle (draft / open / resolved) and
  // severity (must-do / suggestion / question) — bucket types are declared
  // alongside `ReviewToolbarState` above so the toolbar interface can
  // type them.  A comment is shown when BOTH dimensions accept it — so
  // flipping every chip off hides everything. Resolved here covers
  // both "resolved" and "wont-fix": the user thinks of them as the
  // same "done with it" bucket.
  const FILTER_KEY = 'kata:commentFilter';
  function readFilter(): {
    status: Record<StatusBucket, boolean>;
    flag: Record<FlagBucket, boolean>;
  } {
    const def = {
      status: { draft: true, open: true, resolved: true },
      flag: { 'must-do': true, suggestion: true, question: true },
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

  /** Reset every filter chip back to "on". Used by the "filter hides N
   *  comments" hint when the combination of toggles leaves the page
   *  with nothing visible. */
  function resetFilter() {
    filterStatus = { draft: true, open: true, resolved: true };
    filterFlag = { 'must-do': true, suggestion: true, question: true };
  }

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
  /** Switch the view mode. Toggling between diffs ↔ both adds /
   *  removes inline comment threads, which would otherwise shove
   *  the page content up or down by many pixels — the reviewer's
   *  cursor in the diff would jump to a different line. Anchor to
   *  the diff cell at the top of the viewport, switch the mode,
   *  then scroll the page so that same cell lands back where it
   *  was. When the anchor cell is no longer in the DOM (e.g.
   *  going into comments mode, which throws away the hunks),
   *  fall back to the prior behavior of re-anchoring on the
   *  active nav comment. */
  async function setViewMode(m: ViewMode) {
    if (m === viewMode) return;
    const anchor = captureTopCell();
    viewMode = m;
    await tick();
    if (anchor && anchor.el.isConnected) {
      const newY = anchor.el.getBoundingClientRect().top;
      window.scrollBy(0, newY - anchor.offsetFromTop);
    } else {
      await rescrollNavComment();
    }
  }

  // ---- In-app search ----------------------------------------------
  //
  // Cmd+F-style search across the review's diff text, comment bodies,
  // and annotation bodies. The data side lives in `lib/search.ts`;
  // this component owns the state, the force-load that makes lazy
  // files searchable, the prev/next walk, and the scroll-to-match.
  // The actual rendering of the matched substring is in HunkLines /
  // HunkLinesSideBySide / CommentThread / AnnotationBubble, which
  // read the active match info from the `kata-search` context
  // provided below.
  let searchOpen = $state(false);
  let searchQuery = $state('');
  let searchLoading = $state(false);
  /** Help overlay visibility. Toggled by `?` and closed by `Esc`. */
  let helpOpen = $state(false);
  /** 1-based index of the currently-focused match. 0 means "no
   *  selection" (either no query or zero matches). The 1-based
   *  shape matches what the toolbar UI displays ("M of N") and
   *  what the comment-nav `position` field already uses, so the
   *  shell doesn't have to do arithmetic. */
  let searchPosition = $state(0);

  /** Cache key for a file's resolved hunks under the active view —
   *  mirrors `FileSlot`'s `cacheKey` so the search source reads back
   *  the exact entry the slot wrote. Per-commit interdiff
   *  (compare mode) keys by its endpoint pair; everything else keys
   *  by `(patchset, compareWith, path)`. */
  function searchCacheKey(path: string): string {
    const ep = interdiffEndpoints;
    return ep
      ? `interdiff|${ep.useRebase ? 'r' : 'p'}|${ep.from}|${ep.to}|${path}`
      : `${selectedPatchset}|${compareWith ?? ''}|${path}`;
  }

  /** Files to search, with hunks resolved the same way `FileSlot`
   *  resolves them for rendering: prefer inline hunks (the scoped
   *  `commit_diff` path ships them), else the shared cache, else the
   *  metadata-only original.
   *
   *  Crucially this scans `displayedFiles` — the file set the viewer
   *  is actually rendering — not the unscoped `current.diff.files`.
   *  When the diff is scoped to one commit (`?scope=`) or to a
   *  per-commit interdiff, the cumulative review diff carries files
   *  and line numbers that aren't in the DOM; searching it counts
   *  matches the reader can neither navigate to nor see highlighted. */
  function searchFiles(): FileChange[] {
    return displayedFiles.map((f) =>
      f.hunks != null ? f : (fileDiffCache.get(searchCacheKey(f.path)) ?? f),
    );
  }

  /** Build the search source from the resolved-via-cache file list
   *  plus comments / responses / annotations / commits / review
   *  metadata. Comment / response / annotation lists are pre-merged
   *  published + drafts so the user finds their own unpublished
   *  text — they're the most likely thing to look for while
   *  drafting a follow-up reply. */
  const searchSource = $derived.by(() => {
    const resolvedFiles = searchFiles();
    const allComments = [
      ...current.comments,
      ...current.drafts.comments,
    ];
    const allResponsesForSearch = [
      ...current.responses,
      ...current.drafts.responses,
    ];
    return {
      files: resolvedFiles,
      comments: allComments,
      responses: allResponsesForSearch,
      annotations: current.annotations ?? [],
      commits: current.commits.map((c) => ({
        change_id: c.change_id,
        description_first_line: c.description_first_line,
      })),
      reviewName: current.manifest.name,
      reviewSummary: current.manifest.summary,
    };
  });

  const searchMatches: SearchMatch[] = $derived(
    searchOpen ? searchReview(searchQuery, searchSource) : [],
  );

  const currentSearchMatch: SearchMatch | null = $derived(
    searchPosition > 0 && searchPosition <= searchMatches.length
      ? searchMatches[searchPosition - 1]
      : null,
  );

  // Expose the active search state to descendants so the renderers
  // (HunkLines / CommentThread / AnnotationBubble) can decide whether
  // a given cell or comment body has a highlight to draw without
  // each one having to be prop-drilled. The `currentMatch` reactive
  // accessor is read by leaf components; Svelte 5's context is
  // a plain getter, so wrapping in a function keeps it reactive.
  setContext('kata-search', {
    query: () => searchQuery,
    matches: () => searchMatches,
    currentMatch: () => currentSearchMatch,
  });

  /** Reset the position when the query changes; keep it on the
   *  first match (1) if any matches were found, else 0. */
  $effect(() => {
    // Read both query and total so the effect re-runs on either.
    const q = searchQuery;
    const total = searchMatches.length;
    if (q.length === 0 || total === 0) {
      searchPosition = 0;
      return;
    }
    if (searchPosition === 0) {
      searchPosition = 1;
    } else if (searchPosition > total) {
      searchPosition = total;
    }
  });

  /** Reactively jump to the current match whenever it changes —
   *  via typing a new query, walking next/prev, or the
   *  `searchOpen` flip restoring a stored query.
   *
   *  Wrapped in `tick()` so the file's hunks finish rendering
   *  before we try to scroll to a `[data-comment-id]` /
   *  `[data-line]` that might have just been mounted. */
  $effect(() => {
    const m = currentSearchMatch;
    if (!m) return;
    void (async () => {
      await tick();
      await jumpToMatch(m);
    })();
  });

  /** File path of the match's destination if the target lives
   *  inside a FileSlot — which means it can be virtualised away
   *  and we may need to bring the slot into the viewport first
   *  so its IntersectionObserver mounts the FileDiff. Returns
   *  `null` for matches whose target lives outside the slot
   *  system (commits panel, header chrome, review-wide buckets). */
  function fileForMatch(m: SearchMatch): string | null {
    switch (m.kind) {
      case 'line':
      case 'file':
        return m.file;
      case 'comment':
      case 'annotation':
      case 'response':
        return m.file;
      case 'commit':
      case 'review-meta':
        return null;
    }
  }

  async function jumpToMatch(m: SearchMatch): Promise<void> {
    // review-meta jumps to top — no element-resolution needed.
    if (m.kind === 'review-meta') {
      window.scrollTo({ top: 0, behavior: 'auto' });
      return;
    }

    const sel = selectorForMatch(m);
    if (!sel) return;

    let el = document.querySelector<HTMLElement>(sel);

    // Target inside a FileSlot? If the slot is virtualised away,
    // the data-line / data-comment-id / data-response-id node
    // isn't in the DOM yet. Bring the slot into view so its
    // IntersectionObserver mounts the FileDiff, then poll on
    // rAF until the actual match element appears (or the budget
    // runs out). Same wait shape as `scrollToLineRange`.
    const filePath = fileForMatch(m);
    if (!el && filePath) {
      const slot = document.querySelector<HTMLElement>(
        `.file-slot[data-file-path="${CSS.escape(filePath)}"]`,
      );
      if (!slot) return;
      scrollTopOf(slot);
      const deadline = performance.now() + 4000;
      while (!el && performance.now() < deadline) {
        await new Promise((r) => requestAnimationFrame(r));
        el = document.querySelector<HTMLElement>(sel);
      }
      if (!el) return;
    }
    if (!el) return;

    // Park the match below the sticky header. Then stabilise — as
    // virtualised slots above continue to mount in (their height
    // estimate gets replaced with the real height), the document
    // re-flows and the match drifts. Re-aim across a handful of
    // frames until the position is stable. Without this loop a
    // search hit two-thirds down the review lands off-screen
    // (was finding #6 in the audit).
    scrollTopOf(el);
    let stableFrames = 0;
    let lastTop = Number.NaN;
    for (let i = 0; i < 30 && stableFrames < 3; i++) {
      await new Promise((r) => requestAnimationFrame(r));
      const cur = document.querySelector<HTMLElement>(sel);
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

  /** Force-load every text file's hunks so the search source has
   *  full coverage. Binary files and already-loaded files are
   *  skipped; the rest fire `api.fileDiff` in parallel and land in
   *  `fileDiffCache` (which the search source reads via
   *  `searchSource`). The user sees "loading…" in the search
   *  counter until they all complete. */
  async function forceLoadAllFilesForSearch(): Promise<void> {
    // Cover the file set the viewer is rendering — `displayedFiles`,
    // which follows the active scope / compare view — using the same
    // endpoint each `FileSlot` would. A scoped `commit_diff` view
    // already ships its hunks inline (`f.hunks != null`), so those
    // files are skipped and the search source reads them directly.
    const ep = interdiffEndpoints;
    const needed: { key: string; path: string }[] = [];
    for (const f of displayedFiles) {
      if (f.binary) continue;
      if (f.hunks != null) continue;
      const key = searchCacheKey(f.path);
      if (fileDiffCache.has(key)) continue;
      needed.push({ key, path: f.path });
    }
    if (needed.length === 0) return;
    searchLoading = true;
    try {
      await Promise.all(
        needed.map(async ({ key, path }) => {
          try {
            let updated: FileChange;
            if (ep) {
              const res = await api.diffCommits(
                repo,
                ep.from,
                ep.to,
                path,
                ep.useRebase,
              );
              // The generic /diff endpoint returns a discriminated
              // union; a single-file fetch yields the `file` arm.
              if (res.kind !== 'file') return;
              const { kind: _kind, ...rest } = res;
              void _kind;
              updated = rest as FileChange;
            } else {
              updated = await api.fileDiff(
                repo,
                current.manifest.number,
                path,
                selectedPatchset,
                compareWith ?? undefined,
              );
            }
            fileDiffCache.set(key, updated);
          } catch {
            // Best-effort: a single file failing shouldn't block the
            // rest of the search. The file simply won't contribute
            // matches.
          }
        }),
      );
    } finally {
      searchLoading = false;
    }
  }

  function openSearch() {
    if (searchOpen) return;
    searchOpen = true;
  }

  /** Keep the search index's diff coverage in step with the rendered
   *  view. Re-runs whenever the bar is open and the displayed file
   *  set changes — switching commit scope, compare pair, or patchset
   *  while searching. `untrack` wraps the force-load so its internal
   *  reads of `displayedFiles` / `fileDiffCache` don't make the effect
   *  re-fire on its own cache writes; the dependencies that matter are
   *  read explicitly above the call. */
  $effect(() => {
    if (!searchOpen) return;
    // Explicit dependencies: the view selectors that change which
    // files (and which endpoint) the search should cover.
    void scopedChangeId;
    void interdiffEndpoints;
    void selectedPatchset;
    void compareWith;
    void untrack(() => forceLoadAllFilesForSearch());
  });

  function closeSearch() {
    searchOpen = false;
    searchQuery = '';
    searchPosition = 0;
  }

  function setSearchQuery(q: string) {
    searchQuery = q;
  }

  function searchNext() {
    const total = searchMatches.length;
    if (total === 0) return;
    searchPosition = searchPosition >= total ? 1 : searchPosition + 1;
  }

  function searchPrev() {
    const total = searchMatches.length;
    if (total === 0) return;
    searchPosition = searchPosition <= 1 ? total : searchPosition - 1;
  }
  /** Find the topmost diff content cell that's at least partially
   *  visible below the sticky app header. Returns the element + its
   *  current viewport y so the caller can restore that y after a
   *  re-layout. `null` when no diff cell is visible (e.g. the user
   *  is scrolled into a placeholder or above all files). */
  function captureTopCell(): { el: HTMLElement; offsetFromTop: number } | null {
    const headerBottom =
      document.querySelector<HTMLElement>('header.app')
        ?.getBoundingClientRect().bottom ?? 0;
    const cells = document.querySelectorAll<HTMLElement>(
      'td.content[data-line]',
    );
    for (const cell of cells) {
      const r = cell.getBoundingClientRect();
      if (r.bottom > headerBottom) {
        return { el: cell, offsetFromTop: r.top };
      }
    }
    return null;
  }
  async function rescrollNavComment() {
    if (!navCommentId) return;
    const target = orderedComments.find(
      (c) => c.comment_id === navCommentId,
    );
    if (!target) return;
    await tick();
    void scrollToComment(target.comment_id, target.file ?? null);
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
  /** Set to `true` while an explicit prev/next click (or
   *  patchset-jump) is mid-scroll. Suppresses the scroll-sync effect
   *  for the full duration of the operation — including the network
   *  round-trip when the target's file slot needs to mount — so the
   *  click's selection isn't clobbered by sync re-evaluating from a
   *  transient layout. Released by a timer once scrollToComment
   *  finishes, with a small grace period for any final scroll event
   *  to drain. */
  let navigating = false;
  /** Bumped on every explicit nav so an in-flight `scrollToComment`
   *  from a prior click can detect that it's been superseded and
   *  exit its parking loop instead of fighting the new target — the
   *  stabilization loop scrolls every frame, and a stale instance
   *  would re-park the previous comment in lockstep with the new
   *  click's parking attempts. */
  let navGeneration = 0;

  /** Path of the file the reader is currently looking at, derived
   *  from the live scroll position. The file tree uses this to
   *  highlight the current file as the reader scrolls past long
   *  diffs, so the tree stays oriented to the page. `null` while no
   *  file is in view (e.g. the user is scrolled above the first
   *  file slot, inside the commits panel). */
  let activeFilePath: string | null = $state(null);
  /** Position of the active file in `orderedFiles` (1-based); 0 when
   *  no file is in view. Drives the file-nav prev/next position
   *  indicator in the file-tree header. */
  const navFilePosition = $derived.by(() => {
    if (!activeFilePath) return 0;
    const i = orderedFiles.findIndex((f) => f.path === activeFilePath);
    return i < 0 ? 0 : i + 1;
  });
  function fileNavTo(idx: number) {
    if (orderedFiles.length === 0) return;
    const n = orderedFiles.length;
    const wrapped = ((idx - 1 + n) % n) + 1;
    void scrollToFile(orderedFiles[wrapped - 1].path);
  }
  function fileNavPrev() {
    fileNavTo(navFilePosition === 0 ? orderedFiles.length : navFilePosition - 1);
  }
  function fileNavNext() {
    fileNavTo(navFilePosition === 0 ? 1 : navFilePosition + 1);
  }
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

  /** Pixels of context kept above a comment when we scroll one into
   *  view. Roughly six lines of diff text — enough to see what the
   *  comment is anchored to without pushing the comment itself off
   *  the bottom of the viewport. */
  const COMMENT_CONTEXT = 120;

  /** Compute the scroll target that would park `el` `COMMENT_CONTEXT`
   *  pixels below the sticky bars (normal mode) or flush with them
   *  (comments-only mode). Doesn't actually scroll — callers gate on
   *  the click's direction. */
  function commentParkTarget(el: HTMLElement): number {
    const rect = el.getBoundingClientRect();
    if (!showDiffs) {
      const fileHeader = el
        .closest('.file-diff')
        ?.querySelector('.file-header') as HTMLElement | null;
      const extra = fileHeader?.offsetHeight ?? 0;
      return rect.top + window.scrollY - stickyTop() - extra;
    }
    return rect.top + window.scrollY - stickyTop() - COMMENT_CONTEXT;
  }

  /** Always park the target at `COMMENT_CONTEXT` below the sticky
   *  bar. The earlier "respect click direction" heuristic produced
   *  more surprises than it prevented — sync-driven re-selection
   *  after the click felt like the click had stayed on the previous
   *  comment, and across-file nav landed at the slot top instead of
   *  the comment. Consistent parking + the sync suppression below
   *  give a clean result. */
  function bringCommentIntoView(el: HTMLElement): void {
    const target = commentParkTarget(el);
    const clamped = Math.max(0, target);
    if (clamped === window.scrollY) return;
    window.scrollTo({ top: clamped, behavior: 'auto' });
  }

  /** The (side, line) of a comment's anchor row in the diff. Mirrors
   *  the gutter-marker placement in HunkLines (`threadsFor` /
   *  `entriesAt`): moved/drifted anchors sit at their new line,
   *  everything else — valid *and* outdated — at the original
   *  `c.lines.end`. An outdated comment whose original line is still
   *  rendered DOES carry a marker, so it must resolve here too;
   *  whether the row is actually on screen is decided later by the
   *  DOM lookup (which falls back to the orphan group when it isn't).
   *  `null` only for comments with no line anchor at all (file-level /
   *  review-wide). */
  function commentAnchorRow(
    c: CommentView,
  ): { side: 'base' | 'tip'; line: number } | null {
    if (!c.side) return null;
    const eff =
      c.anchor.kind === 'moved' || c.anchor.kind === 'drifted'
        ? c.anchor.new_lines
        : c.lines;
    if (!eff) return null;
    return { side: c.side, line: eff.end };
  }

  /** One transient nav indicator at a time. A prev/next jump marks the
   *  comment it landed on so the reader can spot it without the thread
   *  having to be unfolded; a rapid second jump cancels the prior mark
   *  and restarts on the new one. Two visuals: `nav-pulse` scales the
   *  gutter marker (for line-anchored comments), `nav-flash` outlines
   *  an element (for comments shown as a bubble — orphan / file-level /
   *  review-wide / comments-only mode). */
  let navIndicatorEl: HTMLElement | null = null;
  let navIndicatorTimer: ReturnType<typeof setTimeout> | null = null;
  function applyNavIndicator(el: HTMLElement | null, cls: 'nav-pulse' | 'nav-flash') {
    if (navIndicatorEl) {
      navIndicatorEl.classList.remove('nav-pulse', 'nav-flash');
    }
    if (navIndicatorTimer) clearTimeout(navIndicatorTimer);
    navIndicatorEl = null;
    navIndicatorTimer = null;
    if (!el) return;
    // Restart the CSS animation even on the same element: drop the
    // class, force a reflow, re-add.
    el.classList.remove(cls);
    void el.offsetWidth;
    el.classList.add(cls);
    navIndicatorEl = el;
    navIndicatorTimer = setTimeout(() => {
      el.classList.remove(cls);
      navIndicatorEl = null;
      navIndicatorTimer = null;
    }, 1300);
  }

  /** Pulse the gutter marker for the comment at (side, line). Scoped to
   *  the file slot so a repeated line number in another file can't
   *  steal the pulse. */
  function pulseMarker(file: string | null, side: 'base' | 'tip', line: number) {
    const root: ParentNode =
      (file && document.querySelector(`[data-file-path="${CSS.escape(file)}"]`)) ||
      document;
    const cell = root.querySelector<HTMLElement>(
      `[data-side="${side}"][data-line="${line}"]`,
    );
    const marker = cell
      ?.closest('tr')
      ?.querySelector<HTMLElement>('.thread-marker');
    applyNavIndicator(marker ?? null, 'nav-pulse');
  }

  /** Flash an element that stands in for a comment with no gutter marker
   *  (a folded bubble's header, or the file's "anchored outside the
   *  diff" orphan group). */
  function flashElement(el: HTMLElement) {
    const target = el.classList.contains('orphan-threads')
      ? (el.querySelector<HTMLElement>('.orphan-header') ?? el)
      : el;
    applyNavIndicator(target, 'nav-flash');
  }

  /** Scroll a comment into view, mounting its file's slot if it's been
   *  virtualized away.
   *
   *  Navigation never changes fold state. When the diff is visible and
   *  the comment has a line anchor, we park its anchor *row* (always
   *  in the DOM) and pulse the gutter marker — a folded thread stays
   *  folded, and the pulse shows where the jump landed. Only the
   *  fallback path (comments-only mode, or file-level / review-wide /
   *  outdated comments with no diff row) targets the thread bubble,
   *  which it unfolds so the body is reachable.
   *
   *  In normal (diff-visible) mode an already-visible target stays
   *  put so prev/next doesn't shake the page when two consecutive
   *  comments share the screen. In comments-only mode the page is
   *  dense — several comments often fit on screen at once, so that
   *  rule would make multiple arrow presses look like no-ops until
   *  the next comment ran off the bottom. Re-park on every press
   *  there instead, so each step visibly advances. */
  /** Watch for a reader-initiated scroll gesture so the re-park loops
   *  below can stop fighting it. They programmatically re-scroll to a
   *  target every frame until its layout holds for ~320ms; for a target
   *  that keeps reflowing (a big file, a slow async syntax-highlight
   *  pass) that can run for the whole time budget, and without yielding
   *  here every wheel tick gets snapped straight back — the page feels
   *  stuck. `scrollTo` fires only `scroll` events, never wheel / touch /
   *  keydown, so watching those cleanly tells the reader's intent apart
   *  from our own scrolling. */
  function watchUserScroll(): { interrupted: () => boolean; stop: () => void } {
    const scrollKeys = new Set([
      'ArrowUp',
      'ArrowDown',
      'PageUp',
      'PageDown',
      'Home',
      'End',
      ' ',
      'Spacebar',
    ]);
    let hit = false;
    const onMove = () => {
      hit = true;
    };
    const onKey = (e: KeyboardEvent) => {
      if (scrollKeys.has(e.key)) hit = true;
    };
    window.addEventListener('wheel', onMove, { passive: true });
    window.addEventListener('touchmove', onMove, { passive: true });
    window.addEventListener('keydown', onKey);
    return {
      interrupted: () => hit,
      stop: () => {
        window.removeEventListener('wheel', onMove);
        window.removeEventListener('touchmove', onMove);
        window.removeEventListener('keydown', onKey);
      },
    };
  }

  async function scrollToComment(commentId: string, file: string | null) {
    const myGen = ++navGeneration;
    const superseded = () => myGen !== navGeneration;
    navigating = true;
    const userScroll = watchUserScroll();
    try {
      // Prefer the comment's anchor row (its gutter marker is in the
      // DOM regardless of fold state) so navigation can leave a folded
      // thread folded. The bubble fallback is for comments with no
      // diff row, or comments-only mode where the rows aren't rendered
      // — that path still needs the thread mounted, so it unfolds.
      const comment = [
        ...current.comments,
        ...current.drafts.comments,
      ].find((c) => c.comment_id === commentId);
      const anchor =
        showDiffs && comment ? commentAnchorRow(comment) : null;
      const slotSel = file
        ? `[data-file-path="${CSS.escape(file)}"]`
        : null;
      // Resolve where to park, and how to mark it, each time it's
      // needed (the DOM comes and goes as slots mount). Navigation
      // never unfolds the target — a folded thread stays folded.
      // Tiers, best first:
      //
      //   1. `box` — the comment's own body is on screen (expanded
      //      thread). Highlight the box itself: it's the clearest
      //      "this one", and the reader is already looking right at it.
      //   2. `marker` — the comment is folded but its anchored line is
      //      rendered, so it carries a gutter marker. Pulse that.
      //      Scoped to the file slot since line numbers repeat across
      //      files. An outdated comment whose original line is still
      //      rendered resolves here too.
      //   3. `flash` — a folded bubble header (file-level / review-wide
      //      / comments-only), or the file's orphan group when a
      //      comment anchored outside the hunks is fully collapsed.
      //      Outline that element.
      //
      // No whole-file-slot fallback: outlining the entire file read as
      // a bug, and every real comment resolves to a tier above.
      type NavTarget = { el: HTMLElement; kind: 'box' | 'marker' | 'flash' };
      function findTarget(): NavTarget | null {
        const slot = slotSel
          ? document.querySelector<HTMLElement>(slotSel)
          : null;
        const root: ParentNode = slot ?? document;
        const box = document.querySelector<HTMLElement>(
          `[data-comment-id="${CSS.escape(commentId)}"]:not(.collapsed)`,
        );
        if (box) return { el: box, kind: 'box' };
        if (anchor) {
          const row = root.querySelector<HTMLElement>(
            `[data-side="${anchor.side}"][data-line="${anchor.line}"]`,
          );
          if (row) return { el: row, kind: 'marker' };
        }
        const header = document.querySelector<HTMLElement>(
          `[data-comment-id="${CSS.escape(commentId)}"]`,
        );
        if (header) return { el: header, kind: 'flash' };
        const orphan = slot?.querySelector<HTMLElement>('.orphan-threads');
        return orphan ? { el: orphan, kind: 'flash' } : null;
      }
      // Unified time budget for the whole operation. Cross-file nav
      // from a fresh page can have many intermediate slots to mount
      // before the target is reachable, and each mount triggers a
      // network round-trip that re-flows the layout. Splitting the
      // budget between retry-for-element and stabilization had the
      // retry phase use up most of it on a slow setup, leaving
      // stabilization no time to ride out the settling.
      const TOTAL_TIME_MS = 10000;
      const startTime = performance.now();
      const remaining = () => performance.now() - startTime < TOTAL_TIME_MS;

      let target = findTarget();
      if (!target) {
        // Target isn't in the DOM yet — its FileSlot is virtualized
        // away. Bring the slot into the viewport so the
        // IntersectionObserver mounts the file, then wait for the
        // box / row / group to appear.
        if (slotSel) {
          const slot = document.querySelector<HTMLElement>(slotSel);
          if (slot) scrollTopOf(slot);
        }
        while (!target && remaining() && !superseded()) {
          await new Promise((r) => requestAnimationFrame(r));
          target = findTarget();
        }
        if (!target || superseded()) return;
      }
      // Park the comment. Loop until the page settles against
      // placeholder slots above the target whose heights change as
      // they mount in. Re-parking every frame keeps the comment at
      // the right offset even mid-settle; exit once we get ~320ms of
      // true stability, otherwise ride out the remaining budget. Re-
      // resolve each frame so a row→box transition (the thread mounts
      // expanded as its file does) is tracked.
      let stableFrames = 0;
      let lastTop = Number.NaN;
      const STABLE_REQUIRED = 20;
      while (
        stableFrames < STABLE_REQUIRED &&
        remaining() &&
        !superseded() &&
        !userScroll.interrupted()
      ) {
        bringCommentIntoView(target.el);
        await new Promise((r) => requestAnimationFrame(r));
        const cur = findTarget();
        if (!cur) return;
        target = cur;
        const top = cur.el.getBoundingClientRect().top;
        if (Number.isFinite(lastTop) && Math.abs(top - lastTop) < 0.5) {
          stableFrames++;
        } else {
          stableFrames = 0;
        }
        lastTop = top;
      }
      // Mark where the jump landed so the reader can see which comment
      // is selected. A visible box or a bubble/orphan element flashes
      // its outline; a folded line comment pulses its gutter marker.
      // Skip it if the reader scrolled away mid-settle — the landing
      // is no longer where they're looking.
      if (!superseded() && !userScroll.interrupted()) {
        if (target.kind === 'marker' && anchor) {
          pulseMarker(file, anchor.side, anchor.line);
        } else {
          flashElement(target.el);
        }
      }
    } finally {
      userScroll.stop();
      // Only release the flag if we're still the latest scrollToComment
      // — a newer call has its own lifecycle to manage navigating.
      if (!superseded()) {
        setTimeout(() => {
          if (!superseded()) navigating = false;
        }, 200);
      }
    }
  }

  /** Lightweight permalink jumper for annotations. Brings the
   *  hosting file slot into view, then waits for the bubble's
   *  `data-annotation-id` to appear and scrolls it into the
   *  upper-third of the viewport. Skips the full re-park / nav-
   *  generation handshake of `scrollToComment` — annotations
   *  have no thread state that the user navigates between, so
   *  the simpler version is enough. */
  async function scrollToAnnotation(annotationId: string, file: string | null) {
    if (file) {
      const slot = document.querySelector<HTMLElement>(
        `[data-file-path="${CSS.escape(file)}"]`,
      );
      if (slot) scrollTopOf(slot);
    }
    const sel = `[data-annotation-id="${CSS.escape(annotationId)}"]`;
    const start = performance.now();
    let el = document.querySelector<HTMLElement>(sel);
    while (!el && performance.now() - start < 4000) {
      await new Promise((r) => requestAnimationFrame(r));
      el = document.querySelector<HTMLElement>(sel);
    }
    if (!el) return;
    el.scrollIntoView({ block: 'center', behavior: 'smooth' });
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
      // Skip while an explicit nav is in flight. The click's
      // intermediate scrolls dispatch scroll events that would
      // otherwise let sync re-claim navCommentId from whatever
      // happens to be at the heuristic position mid-stabilization.
      if (navigating) return;
      // Pick the comment whose top is closest to the park position
      // (stickyTop + COMMENT_CONTEXT). That's where bringCommentIntoView
      // lands a navigated-to target, so on free-scroll the counter
      // tracks "the comment under the reader's eye" instead of the
      // first one barely peeking out below the sticky bar — which
      // used to pick the previous comment when two were on screen.
      const ideal = stickyTop() + COMMENT_CONTEXT;
      let bestId: string | null = null;
      let bestDist = Number.POSITIVE_INFINITY;
      let fallback: string | null = null;
      for (const c of orderedComments) {
        const el = document.querySelector<HTMLElement>(
          `[data-comment-id="${CSS.escape(c.comment_id)}"]`,
        );
        if (!el) continue;
        const rect = el.getBoundingClientRect();
        // Track the last in-document comment encountered as a
        // fallback for the "whole review scrolled past" state where
        // every comment sits above the viewport — without it the
        // counter would blank out at N/0.
        if (rect.top < 0) fallback = c.comment_id;
        if (rect.top > window.innerHeight) break;
        const dist = Math.abs(rect.top - ideal);
        if (dist < bestDist) {
          bestDist = dist;
          bestId = c.comment_id;
        }
      }
      const next = bestId ?? fallback;
      if (next && navCommentId !== next) navCommentId = next;
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

  /** Track which file slot the reader is sitting on by reading the
   *  live scroll position. The file tree highlights `activeFilePath`
   *  so the reader can see where they are in the change list as
   *  they scroll. Same rAF throttle as the comment sync above. */
  $effect(() => {
    if (typeof window === 'undefined') return;
    let scheduled = false;
    function syncActiveFile() {
      scheduled = false;
      // Skip while an explicit file/comment nav is parking its
      // target — its intermediate scrolls would otherwise let the
      // spy reclaim `activeFilePath` from whatever slot is at the
      // threshold mid-stabilization, which reads as "jumped to the
      // next file". `scrollToFile` sets the highlight up front.
      if (navigating) return;
      if (orderedFiles.length === 0) {
        if (activeFilePath !== null) activeFilePath = null;
        return;
      }
      // Pick the last file whose top is at or above the sticky bar —
      // that's the slot the reader is currently inside. If we're
      // scrolled above the first slot, leave it un-highlighted.
      const threshold = stickyTop() + 4;
      let candidate: string | null = null;
      for (const f of orderedFiles) {
        const slot = document.querySelector<HTMLElement>(
          `[data-file-path="${CSS.escape(f.path)}"]`,
        );
        if (!slot) continue;
        const rect = slot.getBoundingClientRect();
        if (rect.top <= threshold) candidate = f.path;
        else break;
      }
      // End-of-page rescue: when the page can't scroll any further
      // (last file is too short to push itself up past the sticky
      // bar), the loop above stops on the previous file even though
      // the user is clearly looking at the last one. Force the last
      // file active when we're at — or within a pixel of — the
      // bottom. Uses Math.ceil on scrollY to handle fractional
      // scroll positions some browsers report.
      const docHeight = document.documentElement.scrollHeight;
      const viewportBottom = Math.ceil(window.scrollY) + window.innerHeight;
      if (viewportBottom >= docHeight - 1) {
        candidate = orderedFiles[orderedFiles.length - 1].path;
      }
      if (activeFilePath !== candidate) activeFilePath = candidate;
    }
    function onScroll() {
      if (scheduled) return;
      scheduled = true;
      requestAnimationFrame(syncActiveFile);
    }
    window.addEventListener('scroll', onScroll, { passive: true });
    queueMicrotask(syncActiveFile);
    return () => window.removeEventListener('scroll', onScroll);
  });

  /** Mirror toolbar state up to the app shell whenever it changes. The
   *  shell renders both header rows from this state — see the
   *  ReviewToolbarState interface for the breakdown. */
  $effect(() => {
    const draftComments = current.drafts.comments.length;
    // Only count *written* draft responses — actual replies with a
    // body. The Resolve / Won't fix / Unresolve buttons also create
    // draft responses (so they batch atomically with other edits in
    // the same session) but those fire with body='' and don't
    // register as "drafts" to the user — the comment just shows as
    // resolved and the action feels instant. Counting them would
    // produce surprising "N drafts" when the user thinks they've
    // drafted nothing. They still ride along on publish whenever
    // there's at least one written draft to trigger it.
    const writtenReplies = current.drafts.responses.filter(
      (r) => r.body.trim().length > 0,
    ).length;
    const hasDrafts =
      !!current.drafts.session && draftComments + writtenReplies > 0;
    const hasComments = allComments.length > 0;
    const hiddenCount = hasComments && visibleComments.length === 0
      ? allComments.length
      : 0;
    ontoolbarchange?.({
      title: {
        number: current.manifest.number,
        name: current.manifest.name,
        archived: !!current.manifest.archived_at,
        githubPr: current.manifest.github_pr
          ? {
              number: current.manifest.github_pr.number,
              html_url: current.manifest.github_pr.html_url,
            }
          : null,
      },
      actions: viewer
        ? {
            archive: toggleArchive,
            delete: deleteReviewNow,
            archived: !!current.manifest.archived_at,
            busy: lifecycleBusy,
            manageable: canArchive,
          }
        : null,
      drafts: hasDrafts
        ? {
            count: draftComments + writtenReplies,
            saving,
            publish,
            publishToGithub: current.manifest.github_pr ? publishToGithub : null,
            githubPrUrl: current.manifest.github_pr?.html_url ?? null,
            discard,
            nav:
              draftComments > 0
                ? {
                    position: navDraftPosition,
                    prev: navDraftPrev,
                    next: navDraftNext,
                  }
                : null,
          }
        : null,
      githubActions: current.manifest.github_pr
        ? {
            approve: () => publishVerdict('APPROVE'),
            requestChanges: (body: string) =>
              publishVerdict('REQUEST_CHANGES', body),
            refresh: refreshFromGithub,
            saving,
            refreshing: refreshingFromGithub,
          }
        : null,
      commits:
        commitNavEntries.length > 0
          ? {
              // In compare mode `total` is the count of clickable pair
              // rows (today: `changed`-status pairs); outside compare
              // mode it's the patchset's commit count. The control's
              // 1..N / 0-for-sentinel contract is unchanged.
              total: commitNavEntries.length,
              position: commitNavIndex + 1,
              label: commitNavLabel,
              prev: commitNavPrev,
              next: commitNavNext,
            }
          : null,
      filter: hasComments
        ? {
            status: filterStatus,
            flag: filterFlag,
            toggleStatus: (key) =>
              (filterStatus = { ...filterStatus, [key]: !filterStatus[key] }),
            toggleFlag: (key) =>
              (filterFlag = { ...filterFlag, [key]: !filterFlag[key] }),
            hiddenCount,
            reset: resetFilter,
          }
        : null,
      comments:
        // Comment-nav < > would scroll to anchors that are hidden
        // in diffs mode (showComments=false). Suppress the whole
        // nav in that mode.
        showComments && orderedComments.length > 0
          ? {
              total: orderedComments.length,
              position: navPosition,
              prev: navPrev,
              next: navNext,
            }
          : null,
      view: hasComments
        ? { mode: viewMode, set: setViewMode }
        : null,
      patchsets:
        current.manifest.patchsets.length > 1
          ? {
              options: current.manifest.patchsets.map((p) => ({
                n: p.n,
                label: patchsetLabel(p),
              })),
              selected: selectedPatchset,
              compareWith,
              select: selectPatchset,
              selectCompareWith,
            }
          : null,
      tree: {
        collapsed: treeCollapsed,
        toggle: () => (treeCollapsed = !treeCollapsed),
      },
      search: {
        open: searchOpen,
        query: searchQuery,
        total: searchMatches.length,
        position: searchPosition,
        loading: searchLoading,
        onQueryInput: setSearchQuery,
        onNext: searchNext,
        onPrev: searchPrev,
        onOpen: openSearch,
        onClose: closeSearch,
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
  // Seed scopedChangeId synchronously from the URL so the commits
  // panel reflects the correct selection from the very first frame —
  // without this the panel briefly highlights "All commits" before
  // selectCommit's async fetch resolves and corrects it. While the
  // commit_diff fetch is in flight, the file-render template guards
  // on `scopedChangeId && !scopedDiff` to show a loading placeholder
  // instead of the unscoped fallback files (which is what
  // `displayedFiles` resolves to until scopedDiff is populated).
  // svelte-ignore state_referenced_locally
  let scopedChangeId: string | null = $state(initialScope ?? null);
  let scopedDiff = $state<CommitDiffView | null>(null);

  onMount(() => {
    if (initialScope) void selectCommit(initialScope);
  });

  // ---- Patchset-compare v2 (per-commit interdiff) -------------------
  // Cached pair-list + cumulative summary for the active compare. Fetched
  // whenever `compareWith` flips to a new patchset; null outside compare
  // mode or while a fetch is in flight.
  let compareView = $state<PatchsetCompareView | null>(null);
  // Which change-id from `compareView.pairs` the user clicked into for the
  // per-commit interdiff view. Null = cumulative landing view.
  // svelte-ignore state_referenced_locally
  let selectedCompareCommit: string | null = $state(initialCommit ?? null);
  // The file list backing the per-commit interdiff view. Set after the
  // `diff_commits` fetch completes; null in cumulative mode or while
  // loading. We only store the file-level metadata here — hunks ship
  // lazily through the existing FileSlot flow (which now needs to be
  // taught to fetch via `diff_commits` instead of `file_diff` when an
  // interdiff endpoint pair is in effect; see `compareEndpoints` below).
  let compareInterdiffFiles = $state<FileChange[] | null>(null);

  const displayedFiles = $derived.by(() => {
    if (compareInterdiffFiles) return compareInterdiffFiles;
    const sd = scopedDiff;
    return sd ? sd.files : current.diff.files;
  });
  /** Files reordered to match the file tree's DFS traversal so the diff
   *  panel reads top-to-bottom the way the sidebar does. */
  const orderedFiles = $derived(sortFilesLikeTree(displayedFiles));

  /** Shared cache of resolved per-file diffs, keyed by
   *  `${patchset}|${compare}|${path}`. FileSlot virtualizes itself
   *  out of the DOM once a file is far enough off-screen, so without
   *  this every scroll-back refetched the same hunks. Scoped to this
   *  review (the map dies with the component); composite key keeps
   *  patchset switches from clobbering each other's entries. */
  const fileDiffCache = new SvelteMap<string, FileChange>();

  /** Patchset to thread through to FileSlot/FileDiff for file content,
   *  highlights, and new-comment anchors.
   *
   *  Three modes:
   *  - Per-commit compare view: points at the selected pair's
   *    endpoints so anchors resolve against those commits — file-level
   *    and line-level new comments end up anchored to `to_commit`
   *    (per Decision 3 of the comments-in-compare design).
   *  - Per-commit scoped view (old `commit_diff` path): points at the
   *    clicked commit and its parent.
   *  - Otherwise: the review's selected patchset endpoints. */
  const viewingFor = $derived.by<Patchset>(() => {
    const pair = selectedComparePair;
    const ep = interdiffEndpoints;
    if (pair && ep) {
      // Use the pair's change_id on both sides — we don't have the
      // parent's change_id for added/removed without an extra jj
      // round-trip, and anchor_change_id is mostly informational
      // (the load-bearing field for the anchor system is
      // anchor_commit_id, which we set correctly). The to-side
      // commit is what Decision 3 says new comments anchor against.
      return {
        ...viewing,
        base_change: pair.change_id,
        base_commit: ep.from,
        tip_change: pair.change_id,
        tip_commit: ep.to,
      };
    }
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

  /** True when new comments should be allowed in the current view.
   *  Per Decision 2 of the comments-in-compare design: per-commit
   *  compare view only allows new comments when the `to` patchset is
   *  the review's current one — otherwise the comment would anchor
   *  to a non-current commit and immediately read as "drifted" /
   *  "outdated" in the normal review view. Cumulative compare and
   *  non-compare views never gate writes here. */
  const commentsWriteable = $derived.by<boolean>(() => {
    if (!selectedComparePair) return true;
    return selectedPatchset === current.manifest.current_patchset;
  });

  /** Comments scoped to the active per-commit pair. Filters
   *  `visibleComments` down to those whose `anchor_commit_id` matches
   *  one side of the displayed pair (Decision 1 = option c). Outside
   *  per-commit compare view this is just `visibleComments` — no
   *  filtering. The CommitsPanel still receives the unfiltered list
   *  for its commit-level / review-wide threads. */
  const visibleCommentsForFiles = $derived.by<CommentView[]>(() => {
    const pair = selectedComparePair;
    if (!pair) return visibleComments;
    const sides = new Set<string>();
    if (pair.from_commit) sides.add(pair.from_commit);
    if (pair.to_commit) sides.add(pair.to_commit);
    return visibleComments.filter((c) => sides.has(c.anchor_commit_id));
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
      onviewchange?.({
        patchset: selectedPatchset,
        compareWith,
        scope: null,
      });
      return;
    }
    loadingDiff = true;
    loadingDiffLabel = changeId.slice(0, 12);
    error = null;
    try {
      scopedDiff = await api.commitDiff(repo, current.manifest.number, changeId);
      scopedChangeId = changeId;
      onviewchange?.({
        patchset: selectedPatchset,
        compareWith,
        scope: changeId,
      });
    } catch (e) {
      error = (e as Error).message;
    } finally {
      loadingDiff = false;
    }
  }

  // ---- Patchset-compare v2 fetch logic ------------------------------
  // (1) Fetch the pair-list summary whenever the active compare pair
  //     changes. `compareView` then drives the CommitsPanel's
  //     per-change-id badges.
  // (2) When the user clicks a `changed` pair (selectedCompareCommit
  //     set), fetch the interdiff between PS_a's and PS_b's versions
  //     of that change-id and stash the file list.
  //
  // Errors from either fetch land in `compareError` (separate from the
  // global `error`) so the side panel can show a dedicated banner —
  // silently falling back to the normal commits panel hid real
  // failures during prototype dogfooding.
  let compareLoadKey = $state(''); // memo so we don't refetch needlessly
  let compareError: string | null = $state(null);
  $effect(() => {
    if (compareWith == null) {
      compareView = null;
      compareError = null;
      return;
    }
    const key = `${selectedPatchset}|${compareWith}`;
    if (key === compareLoadKey) return;
    compareLoadKey = key;
    compareError = null;
    const reviewNumber = current.manifest.number;
    void api
      .comparePatchsets(repo, reviewNumber, compareWith, selectedPatchset)
      .then((cv) => {
        // Drop stale responses if the user re-selected in the meantime.
        if (compareLoadKey !== key) return;
        compareView = cv;
        compareError = null;
      })
      .catch((e: Error) => {
        if (compareLoadKey !== key) return;
        compareError = e.message;
        compareView = null;
      });
  });

  /** Pair object for the selected per-commit compare row (if any). */
  const selectedComparePair = $derived.by<PatchsetPair | null>(() => {
    if (!compareView || !selectedCompareCommit) return null;
    return compareView.pairs.find((p) => p.change_id === selectedCompareCommit) ?? null;
  });

  /** Endpoint pair the FileSlot grid fetches against when we're in
   *  the per-commit compare view. Three cases:
   *  - `changed`: (from_commit, to_commit) — needs the rebase-based
   *    interdiff (set `useRebase` true) so downstream-of-rewrite
   *    commits don't all show the same inherited cumulative delta.
   *  - `added-in-to`: (parent_commit, to_commit) — the commit's own
   *    parent..commit diff. Plain commit-to-commit; no rebase.
   *  - `removed-from-from`: (parent_commit, from_commit) — same
   *    shape, for the dropped commit's content. No rebase.
   *  Returns null for `same` (nothing to render) and as a fallback if
   *  the required commits are missing. */
  const interdiffEndpoints = $derived.by<
    { from: string; to: string; useRebase: boolean } | null
  >(() => {
    const pair = selectedComparePair;
    if (!pair) return null;
    switch (pair.status) {
      case 'changed':
        return pair.from_commit && pair.to_commit
          ? { from: pair.from_commit, to: pair.to_commit, useRebase: true }
          : null;
      case 'added-in-to':
        return pair.parent_commit && pair.to_commit
          ? {
              from: pair.parent_commit,
              to: pair.to_commit,
              useRebase: false,
            }
          : null;
      case 'removed-from-from':
        return pair.parent_commit && pair.from_commit
          ? {
              from: pair.parent_commit,
              to: pair.from_commit,
              useRebase: false,
            }
          : null;
      case 'same':
        return null;
    }
  });

  /** The commit the displayed diff's `base_line` numbers index into,
   *  or `null` outside compare mode. Threaded into `FileSlot` →
   *  `FileDiff` so the per-file highlight pass (and base-side content
   *  reads) pull from the same file version the hunks reference.
   *  Without this, removed-side rows in compare mode render with HTML
   *  pulled from `patchset.base_commit`'s file instead — line content
   *  reads as wildly unrelated to the actual diff.
   *
   *  Two compare shapes need different answers:
   *  - Per-commit interdiff (a pair row is selected): the base is the
   *    pair's from-side commit (`interdiffEndpoints.from` — the
   *    parent for an added/removed pair, the from-side commit for a
   *    changed pair), which `viewingFor` already points the diff at.
   *    The compared patchset's tip would be the wrong file entirely.
   *  - Whole-patchset cumulative compare (no pair selected): the base
   *    is the compared patchset's tip. */
  const compareBaseCommit = $derived(
    compareBaseCommitFor(
      interdiffEndpoints?.from ?? null,
      compareWith,
      current.manifest.patchsets,
    ),
  );

  // Fetch the file list backing the per-commit interdiff whenever the
  // endpoint pair changes. The per-file hunks ship lazily via FileSlot's
  // own fetch (which now goes through `/diff` thanks to the
  // `interdiffEndpoints` prop).
  let interdiffLoadKey = $state('');
  $effect(() => {
    const ep = interdiffEndpoints;
    if (!ep) {
      compareInterdiffFiles = null;
      interdiffLoadKey = '';
      return;
    }
    const key = `${ep.from}|${ep.to}`;
    if (key === interdiffLoadKey) return;
    interdiffLoadKey = key;
    loadingDiff = true;
    loadingDiffLabel = `interdiff ${selectedCompareCommit?.slice(0, 12) ?? ''}`;
    void api
      .diffCommits(repo, ep.from, ep.to, undefined, ep.useRebase)
      .then((res) => {
        if (interdiffLoadKey !== key) return;
        if (res.kind !== 'diff') {
          throw new Error('expected diff-shape result from /diff (no path)');
        }
        compareInterdiffFiles = res.files;
      })
      .catch((e: Error) => {
        if (interdiffLoadKey !== key) return;
        compareError = `Loading interdiff failed: ${e.message}`;
        compareInterdiffFiles = null;
      })
      .finally(() => {
        if (interdiffLoadKey !== key) return;
        loadingDiff = false;
      });
  });

  /** Pick a per-commit compare row (or null to drop back to the
   *  cumulative view). Called by CommitsPanel in compare mode. */
  function selectCompareCommit(changeId: string | null) {
    if (changeId === selectedCompareCommit) return;
    selectedCompareCommit = changeId;
    onviewchange?.({
      patchset: selectedPatchset,
      compareWith,
      commit: changeId,
    });
  }

  /** Walkable entries for the top-bar `< >` commit nav.
   *
   *  Outside compare mode this is every commit in the selected
   *  patchset (the `current.commits` list). In compare mode it's the
   *  clickable pair-list entries from `compareView.pairs` — today only
   *  `status === 'changed'`, since `added`/`removed` don't yet have an
   *  interdiff endpoint pair in the prototype. The two modes share
   *  one shape so the nav buttons / label / wraparound logic below
   *  can stay mode-agnostic. */
  type CommitNavEntry = { changeId: string; label: string };
  // Match the CommitsPanel's clickability rule: a pair is walkable
  // iff it has the commit-ids the interdiff endpoint pair needs.
  // Keep this in sync with the corresponding rule in CommitsPanel
  // (status-specific availability check on parent_commit / from /
  // to_commit) — both decide the same thing from the same data.
  function pairIsClickable(p: PatchsetPair): boolean {
    switch (p.status) {
      case 'changed':
        return !!p.from_commit && !!p.to_commit;
      case 'added-in-to':
        return !!p.parent_commit && !!p.to_commit;
      case 'removed-from-from':
        return !!p.parent_commit && !!p.from_commit;
      case 'same':
        return false;
    }
  }
  const commitNavEntries = $derived.by<CommitNavEntry[]>(() => {
    if (compareView) {
      return compareView.pairs
        .filter(pairIsClickable)
        .map((p) => {
          const subject =
            (p.to_description ?? p.from_description ?? '').trim() ||
            '(no description)';
          const trimmed =
            subject.length > 60 ? `${subject.slice(0, 57)}…` : subject;
          const badge =
            p.status === 'changed'
              ? '~'
              : p.status === 'added-in-to'
                ? '+'
                : '−';
          return {
            changeId: p.change_id,
            label: `${badge} ${p.change_id.slice(0, 8)} · ${trimmed}`,
          };
        });
    }
    return current.commits.map((c) => {
      const subject = c.description_first_line.trim() || '(no description)';
      const trimmed =
        subject.length > 60 ? `${subject.slice(0, 57)}…` : subject;
      return {
        changeId: c.change_id,
        label: `${c.change_id.slice(0, 8)} · ${trimmed}`,
      };
    });
  });

  /** Which entry the nav considers active. Pulls from the matching
   *  selection state — `selectedCompareCommit` in compare mode,
   *  `scopedChangeId` otherwise — so the two flows never disagree
   *  with the top-bar label. */
  const commitNavSelectedId = $derived.by<string | null>(() =>
    compareView ? selectedCompareCommit : scopedChangeId,
  );

  /** Where the active entry sits in `commitNavEntries`. -1 = the
   *  "All commits" / "Cumulative" sentinel; the prev/next buttons
   *  bounce through -1 between the ends so the user can always step
   *  back to the unscoped view without leaving the keyboard. */
  const commitNavIndex = $derived.by(() => {
    const id = commitNavSelectedId;
    if (id === null) return -1;
    return commitNavEntries.findIndex((e) => e.changeId === id);
  });
  const commitNavLabel = $derived.by(() => {
    const defaultLabel = compareView ? 'Cumulative' : 'All commits';
    if (commitNavIndex < 0) return defaultLabel;
    return commitNavEntries[commitNavIndex]?.label ?? defaultLabel;
  });

  /** Route a nav-button click to the right setter based on which mode
   *  we're in. In compare mode this drives the pair-list selection
   *  (and the URL's `&commit=` param) — same setter the side panel
   *  uses, so both controls stay in sync. */
  function applyNavSelection(changeId: string | null) {
    if (compareView) {
      selectCompareCommit(changeId);
    } else {
      void selectCommit(changeId);
    }
  }
  function selectCommitByIndex(i: number) {
    if (i < 0) {
      applyNavSelection(null);
      return;
    }
    const e = commitNavEntries[i];
    if (e) applyNavSelection(e.changeId);
  }
  function commitNavPrev() {
    if (commitNavEntries.length === 0) return;
    if (commitNavIndex < 0) {
      selectCommitByIndex(commitNavEntries.length - 1);
    } else if (commitNavIndex === 0) {
      selectCommitByIndex(-1);
    } else {
      selectCommitByIndex(commitNavIndex - 1);
    }
  }
  function commitNavNext() {
    if (commitNavEntries.length === 0) return;
    if (commitNavIndex < 0) {
      selectCommitByIndex(0);
    } else if (commitNavIndex === commitNavEntries.length - 1) {
      selectCommitByIndex(-1);
    } else {
      selectCommitByIndex(commitNavIndex + 1);
    }
  }


  /** Files actually rendered in the main panel. In comments-only mode
   *  files with no (visible) comments are hidden so the page is a flat
   *  list of feedback; the file being composed on stays visible so the
   *  inline composer doesn't disappear under the user. */
  const visibleFiles = $derived.by(() => {
    if (showDiffs) return orderedFiles;
    const withComments = new Set(
      visibleComments.map((c) => c.file).filter((p): p is string => !!p),
    );
    const composingFile =
      composing && 'file' in composing ? composing.file : null;
    return orderedFiles.filter(
      (f) => withComments.has(f.path) || f.path === composingFile,
    );
  });

  /** File paths that carry at least one (visible) comment. Used to
   *  flip on `eagerFetch` for those FileSlots: their per-file diff
   *  loads in the background so the comment-nav can land on them
   *  reliably without waiting for an on-demand fetch mid-scroll. */
  const filesWithComments = $derived.by(() => {
    const set = new Set<string>();
    for (const c of allComments) {
      if (c.file) set.add(c.file);
    }
    return set;
  });


  function short(id: string): string {
    return id.length > 12 ? id.slice(0, 12) : id;
  }

  function formatDate(iso: string): string {
    const d = new Date(iso);
    if (isNaN(d.getTime())) return iso;
    return d.toLocaleString();
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

  /** Whether the current viewer can archive / unarchive — gated to
   *  the review's creator. Empty `viewer` (whoami hasn't resolved yet)
   *  hides the affordance. */
  const canArchive = $derived(
    (!!viewer && viewer === current.manifest.created_by) || isAdmin,
  );
  /** True while an archive / delete request is in flight. Disables
   *  the menu items so a double-click can't fire two requests. */
  let lifecycleBusy = $state(false);
  async function toggleArchive() {
    if (lifecycleBusy) return;
    const isArchived = !!current.manifest.archived_at;
    const verb = isArchived ? 'Unarchive' : 'Archive';
    if (!confirm(`${verb} this review?`)) return;
    lifecycleBusy = true;
    error = null;
    try {
      const updated = isArchived
        ? await api.unarchiveReview(repo, current.manifest.number)
        : await api.archiveReview(repo, current.manifest.number);
      current = { ...current, manifest: updated };
    } catch (e) {
      error = (e as Error).message;
    } finally {
      lifecycleBusy = false;
    }
  }
  async function deleteReviewNow() {
    if (lifecycleBusy) return;
    const ok = confirm(
      `Permanently delete review #${current.manifest.number} "${current.manifest.name}"?\n\nThis removes all sessions, comments, responses, and annotations. Cannot be undone.`,
    );
    if (!ok) return;
    lifecycleBusy = true;
    error = null;
    try {
      await api.deleteReview(repo, current.manifest.number);
      // App.svelte's SSE handler navigates back to the list when it
      // sees the `review-deleted` event, so nothing local to do here
      // beyond clearing the busy flag.
    } catch (e) {
      error = (e as Error).message;
    } finally {
      lifecycleBusy = false;
    }
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
      await api.refreshReview(repo, current.manifest.number);
      await refresh();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      refreshing = false;
    }
  }

  /** Revset edit state. Only the review's creator sees the edit
   *  affordance; the input pre-fills with the current revset and a
   *  no-op save (same string) silently closes the editor. Server
   *  errors (bad revset, "multiple heads", etc.) render inline
   *  rather than as a global error so the reader keeps their
   *  in-progress draft. */
  let editingRevset = $state(false);
  let revsetDraft = $state('');
  let savingRevset = $state(false);
  let revsetError: string | null = $state(null);
  let revsetInputEl: HTMLInputElement | undefined = $state();
  const canEditRevset = $derived(
    (!!viewer && viewer === current.manifest.created_by) || isAdmin,
  );
  async function startEditRevset() {
    revsetDraft = current.manifest.revset;
    revsetError = null;
    editingRevset = true;
    // Wait for the input to mount, then focus + select so the user
    // can start typing immediately (or hit Escape with no fuss).
    await tick();
    revsetInputEl?.focus();
    revsetInputEl?.select();
  }
  function cancelEditRevset() {
    editingRevset = false;
    revsetError = null;
  }
  async function saveRevset() {
    const next = revsetDraft.trim();
    if (next.length === 0 || next === current.manifest.revset) {
      cancelEditRevset();
      return;
    }
    if (savingRevset) return;
    savingRevset = true;
    revsetError = null;
    try {
      await api.updateRevset(repo, current.manifest.number, next);
      await refresh();
      editingRevset = false;
    } catch (e) {
      revsetError = (e as Error).message;
    } finally {
      savingRevset = false;
    }
  }
  function onRevsetKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      e.preventDefault();
      void saveRevset();
    } else if (e.key === 'Escape') {
      e.preventDefault();
      cancelEditRevset();
    }
  }

  async function refresh() {
    const wasOnLatest = selectedPatchset === current.manifest.current_patchset;
    const compare = compareWith ?? undefined;
    const next = await api.openReview(
      repo,
      current.manifest.number,
      selectedPatchset,
      compare,
    );
    // If the user was tracking the latest patchset and a new one just landed,
    // follow it forward; otherwise stay where they are.
    if (wasOnLatest && next.manifest.current_patchset !== selectedPatchset) {
      const newPs = next.manifest.current_patchset;
      // Compare-with would diff a patchset against itself if it now
      // matches the new selection — drop it. Otherwise carry over.
      const nextCompare = compareWith === newPs ? null : compareWith;
      current = await api.openReview(
        repo,
        current.manifest.number,
        newPs,
        nextCompare ?? undefined,
      );
      selectedPatchset = newPs;
      compareWith = nextCompare;
      // Per-patchset scoped state is tied to commit IDs from the
      // OLD patchset — a scoped commit or compare-pair selection
      // from PS_old doesn't carry over to PS_new (the change-id may
      // not even live in the new diff). Mirror the cleanup
      // `selectPatchset` does so a fresh-patchset auto-advance
      // doesn't leave the UI rendering the old scope under the new
      // selection.
      scopedChangeId = null;
      scopedDiff = null;
      selectedCompareCommit = null;
      compareInterdiffFiles = null;
      // Cancel in-progress composers whose anchor no longer makes
      // sense against the new patchset. The new diff may have
      // deleted the file the user was commenting on, or rewritten
      // the commit they were anchored to — in either case the
      // composer's overlay would render against content that no
      // longer exists. Composers on still-present anchors stay
      // open: the body is the user's work, and the comment will
      // submit fine (anchor_commit_id resolves via the same
      // re-anchor path Outdated comments use).
      composing = composerSurvivesPatchset(composing, current);
      composingAnnotation = annotationComposerSurvivesPatchset(
        composingAnnotation,
        current,
      );
      // Drop stale per-file diff cache entries. Keys from the old
      // patchset (or any prior interdiff scope) will never be
      // looked up again once `selectedPatchset` advances, so
      // they're dead weight — slow memory leak across long
      // sessions otherwise.
      pruneFileDiffCache(fileDiffCache, newPs, nextCompare);
      onviewchange?.({
        patchset: newPs,
        compareWith: nextCompare,
        commit: null,
        scope: null,
      });
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
        current.manifest.number,
        n,
        nextCompare ?? undefined,
      );
      selectedPatchset = n;
      compareWith = nextCompare;
      // Discarding the per-commit scope: it was tied to the previous PS.
      scopedChangeId = null;
      scopedDiff = null;
      selectedCompareCommit = null;
      compareInterdiffFiles = null;
      onviewchange?.({
        patchset: n,
        compareWith: nextCompare,
        commit: null,
        scope: null,
      });
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  /** Switch to patchset `n` and, after the diff/comments for that
   *  patchset have loaded, scroll to the comment whose id matches
   *  `commentId` (if given). Used by the per-comment "added in PS N"
   *  jump-button so the reader lands directly on the comment in the
   *  patchset it was originally written against. */
  async function jumpToPatchset(n: number, commentId?: string) {
    if (n !== selectedPatchset) {
      await selectPatchset(n);
    }
    if (!commentId) return;
    // `current` now reflects the new patchset. Look the comment up
    // (it might also be in the user's drafts) so we know which file
    // to scroll the slot for.
    const target = [...current.comments, ...current.drafts.comments].find(
      (c) => c.comment_id === commentId,
    );
    if (!target) return;
    navCommentId = commentId;
    await tick();
    await scrollToComment(commentId, target.file ?? null);
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
        current.manifest.number,
        selectedPatchset,
        n ?? undefined,
      );
      compareWith = n;
      scopedChangeId = null;
      scopedDiff = null;
      selectedCompareCommit = null;
      compareInterdiffFiles = null;
      onviewchange?.({
        patchset: selectedPatchset,
        compareWith: n,
        commit: null,
        scope: null,
      });
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
      // Workspace-level events (workspace-registered / unregistered)
      // are review-shell concerns handled in App.svelte; they have
      // no review_id to filter against, so bail before the field
      // narrowing below would discriminate the union.
      if (
        event.kind === 'workspace-registered' ||
        event.kind === 'workspace-unregistered'
      ) {
        return;
      }
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

  /** Global keyboard shortcuts:
   *  - `/`      opens the search bar.
   *  - `Esc`    closes the search bar / help overlay.
   *  - `j` / `k`  next / previous comment (in reading order, the
   *               same walk the toolbar's `< >` cluster drives).
   *  - `n` / `p`  next / previous file (jump to the next / previous
   *               file in the file-tree's order).
   *  - `?`      toggles the keyboard-shortcut help overlay.
   *
   *  Every shortcut bails when the focus is inside an input,
   *  textarea, select, or contenteditable — the user is typing
   *  there and the bare letter belongs to that field. Modifier
   *  combinations (Ctrl/Cmd/Alt) also bail so we don't shadow
   *  browser / OS shortcuts. */
  onMount(() => {
    function isEditableTarget(t: EventTarget | null): boolean {
      const el = t as HTMLElement | null;
      if (!el) return false;
      const tag = el.tagName;
      if (tag === 'INPUT' || tag === 'TEXTAREA' || tag === 'SELECT') return true;
      if (el.isContentEditable) return true;
      return false;
    }
    function onKey(e: KeyboardEvent) {
      // All shortcuts here ignore modifier-key combinations so they
      // don't shadow browser / OS chords like Ctrl+/, Cmd+K, etc.
      if (e.metaKey || e.ctrlKey || e.altKey) return;

      if (e.key === '/') {
        if (isEditableTarget(e.target)) return;
        e.preventDefault();
        openSearch();
        return;
      }

      if (e.key === 'Escape') {
        if (helpOpen) {
          e.preventDefault();
          helpOpen = false;
          return;
        }
        if (searchOpen) {
          // Don't close on Esc while a composer / input is focused;
          // that key is reserved for whatever the focused control
          // wants to do with it. Exception: the search input itself
          // — `Esc` from inside the field should still dismiss the
          // bar (matches Cmd+F behaviour in the browser).
          if (isEditableTarget(e.target)) {
            const el = e.target as HTMLElement | null;
            if (!el?.classList?.contains?.('search-input')) return;
          }
          e.preventDefault();
          closeSearch();
        }
        return;
      }

      // Letter-key shortcuts: bail when typing inside a composer.
      if (isEditableTarget(e.target)) return;

      if (e.key === '?') {
        e.preventDefault();
        helpOpen = !helpOpen;
      } else if (e.key === 'j') {
        e.preventDefault();
        navNext();
      } else if (e.key === 'k') {
        e.preventDefault();
        navPrev();
      } else if (e.key === 'n') {
        e.preventDefault();
        fileNavNext();
      } else if (e.key === 'p') {
        e.preventDefault();
        fileNavPrev();
      }
    }
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  });

  /** Hash-routed scroll jumps. Two flavours:
   *  - `#c-<commentId>` for comment permalinks (Copy-Link button, or
   *    any inbound permalink from elsewhere in the app).
   *  - `#file-<encoded path>` for the file-tree's onselect — sets the
   *    hash so browser back undoes the jump.
   *  Listens on mount + on hashchange. Other state-changing
   *  navigation (patchset, compare, scope) routes through the
   *  `?ps=`/`?cmp=`/`?commit=`/`?scope=` query path + pushState
   *  in `App.svelte`. */
  function jumpToHash() {
    const hash = window.location.hash;
    if (hash.startsWith('#c-')) {
      const commentId = decodeURIComponent(hash.slice(3));
      const comment = [
        ...current.comments,
        ...current.drafts.comments,
      ].find((c) => c.comment_id === commentId);
      if (!comment) return;
      void scrollToComment(comment.comment_id, comment.file ?? null);
    } else if (hash.startsWith('#n-')) {
      // Same shape as `#c-` but for annotations. Looks the
      // annotation up so we can pre-scroll the right slot into
      // view (and so a stale permalink with no matching note
      // is a graceful no-op rather than a hang on the retry
      // loop).
      const annotationId = decodeURIComponent(hash.slice(3));
      const annotation = (current.annotations ?? []).find(
        (a) => a.annotation_id === annotationId,
      );
      if (!annotation) return;
      void scrollToAnnotation(annotation.annotation_id, annotation.file ?? null);
    } else if (hash.startsWith('#file-')) {
      const path = decodeURIComponent(hash.slice(6));
      void scrollToFile(path);
    } else if (hash.startsWith('#L:')) {
      // Line-range permalink: `#L:<file>:<side>:<startLine>[-<endLine>]`.
      // Produced by SelectionPopup's "Copy permalink" button.
      const link = parseLineRangeHash(hash);
      if (!link) return;
      void scrollToLineRange(link.file, link.side, link.startLine, link.endLine);
    }
  }

  /** File-tree click handler: update the URL hash so browser back
   *  undoes the jump, then let the `hashchange` listener above do
   *  the scroll. When the user clicks the same file again (hash
   *  already matches), `hashchange` won't fire, so call
   *  `scrollToFile` directly. */
  function goToFile(path: string) {
    const newHash = '#file-' + encodeURIComponent(path);
    if (window.location.hash === newHash) {
      void scrollToFile(path);
    } else {
      window.location.hash = newHash;
    }
  }
  onMount(() => {
    // Wait one frame for FileSlots to register; scrollToComment also
    // retries internally, so this is belt-and-braces.
    requestAnimationFrame(jumpToHash);
    window.addEventListener('hashchange', jumpToHash);
    // Escape closes the file-tree drawer on phones, where it's a
    // modal overlay over the diff — the backdrop tap and the ☰
    // toggle already close it, Escape is the missing keyboard
    // gesture. A no-op on desktop, where the tree is in-flow (and
    // Escape stays free for the comment composer).
    const onKeydown = (e: KeyboardEvent) => {
      if (
        e.key === 'Escape' &&
        !treeCollapsed &&
        window.matchMedia('(max-width: 640px)').matches
      ) {
        treeCollapsed = true;
      }
    };
    window.addEventListener('keydown', onKeydown);
    return () => {
      window.removeEventListener('hashchange', jumpToHash);
      window.removeEventListener('keydown', onKeydown);
    };
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
    const session = await api.startSession(repo, current.manifest.number);
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
            current.manifest.number,
            sid,
            editingId,
            input,
          )
        : await api.createComment(repo, current.manifest.number, sid, input);
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
      // Brand new drafts: force expanded. `defaultThreadsCollapsed`
      // would otherwise fold them on creation in modes where the
      // default is "collapsed" (e.g. comments-only mode), which
      // hides what the user just typed behind a click-to-expand —
      // not what they expect right after authoring.
      if (!editingId) {
        foldStore.set('comment', view.comment_id, false);
        foldVersion++;
      }
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
        current.manifest.number,
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
      // file == null: route as 'commit' if the anchor is still in the
      // visible commit list, otherwise fall back to 'review' (orphan).
      const target = current.commits.find(
        (c) => c.change_id === comment.anchor_change_id,
      );
      composing = target
        ? {
            kind: 'commit',
            change_id: target.change_id,
            commit_id: target.commit_id,
            editing,
          }
        : { kind: 'review', editing };
    }
  }

  // ---- Annotations --------------------------------------------------

  /** Gate for the author-only annotation flow. The button + composer
   *  affordances stay hidden for non-creators; the server enforces
   *  the same rule, so this is purely a UX gate. */
  const canAnnotate = $derived(
    (!!viewer && viewer === current.manifest.created_by) || isAdmin,
  );

  function startAnnotate(target: AnnotationComposerTarget) {
    composingAnnotation = target;
  }

  function cancelAnnotate() {
    composingAnnotation = null;
  }

  function startEditAnnotation(annotation: AnnotationView) {
    if (!canAnnotate) return;
    const editing = {
      annotationId: annotation.annotation_id,
      body: annotation.body,
    };
    if (annotation.file && annotation.lines && annotation.side) {
      composingAnnotation = {
        kind: 'line',
        file: annotation.file,
        side: annotation.side,
        startLine: annotation.lines.start,
        endLine: annotation.lines.end,
        editing,
      };
    } else if (annotation.file) {
      composingAnnotation = {
        kind: 'file',
        file: annotation.file,
        editing,
      };
    }
  }

  async function submitAnnotation(input: AnnotationInput) {
    saving = true;
    error = null;
    try {
      const editingId = composingAnnotation?.editing?.annotationId;
      const saved = editingId
        ? await api.updateAnnotation(
            repo,
            current.manifest.number,
            editingId,
            input,
          )
        : await api.createAnnotation(repo, current.manifest.number, input);
      // Splice into local state. Anchor is trivially valid here — it
      // was just authored against the patchset we're viewing; the
      // server-side resolver will reconfirm on next load.
      const view: AnnotationView = { ...saved, anchor: { kind: 'valid' } };
      const prev = current.annotations ?? [];
      const next = editingId
        ? prev.map((a) => (a.annotation_id === editingId ? view : a))
        : [...prev, view];
      current = { ...current, annotations: next };
      composingAnnotation = null;
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  async function deleteAnnotation(annotation: AnnotationView) {
    if (!canAnnotate) return;
    saving = true;
    error = null;
    try {
      await api.deleteAnnotation(
        repo,
        current.manifest.number,
        annotation.annotation_id,
      );
      current = {
        ...current,
        annotations: (current.annotations ?? []).filter(
          (a) => a.annotation_id !== annotation.annotation_id,
        ),
      };
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  async function submitResponse(input: DraftResponseInput) {
    saving = true;
    error = null;
    try {
      const sid = await ensureSession();
      const saved = await api.createResponse(
        repo,
        current.manifest.number,
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
        current.manifest.number,
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

  async function deleteResponse(response: ResponseView) {
    if (!confirm('Delete this draft reply?')) return;
    saving = true;
    error = null;
    try {
      await api.deleteResponse(
        repo,
        current.manifest.number,
        response.session_id,
        response.response_id,
      );
      current = {
        ...current,
        drafts: {
          ...current.drafts,
          responses: current.drafts.responses.filter(
            (r) => r.response_id !== response.response_id,
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
        current.manifest.number,
        current.drafts.session.session_id,
      );
      await refresh();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      saving = false;
    }
  }

  /** Publish the current session as a GitHub PR review. Only
   *  reachable when `manifest.github_pr` is set; the toolbar
   *  guards this in `publishToGithub`'s presence.
   *
   *  Head-drift recovery: when the server refuses because the PR
   *  head moved (a force-push or rebase since import), we offer to
   *  re-import + retry in one click. Without this, the user has to
   *  manually navigate home, paste the URL, and come back — which
   *  loses their session context. */
  async function publishToGithub(
    event: 'COMMENT' | 'APPROVE' | 'REQUEST_CHANGES',
    body?: string,
  ): Promise<boolean> {
    if (!current.drafts.session) return false;
    const gh = current.manifest.github_pr;
    if (!gh) return false;
    saving = true;
    error = null;
    let ok = false;
    try {
      await api.publishToGithub(
        repo,
        current.manifest.number,
        current.drafts.session.session_id,
        event,
        body,
      );
      await refresh();
      ok = true;
    } catch (e) {
      const msg = (e as Error).message;
      // Head-drift refusal is the only failure with a one-click
      // recovery — offer it explicitly. We match on the typed
      // `error_kind` discriminator the server emits, not the
      // prose, so re-wording the message doesn't silently break
      // the recovery path.
      if (e instanceof ApiError && e.kind === 'head_drift') {
        const yes = confirm(
          `${msg}\n\nRe-import this PR now and retry publishing? ` +
          `(Existing imported discussion stays; only the head SHA + ` +
          `any new GitHub comments are refreshed.)`,
        );
        if (yes) {
          try {
            await api.importGithubPr(gh.html_url);
            await refresh();
            await api.publishToGithub(
              repo,
              current.manifest.number,
              current.drafts.session.session_id,
              event,
              body,
            );
            await refresh();
            error = null;
            ok = true;
          } catch (e2) {
            error = (e2 as Error).message;
          }
        } else {
          error = msg;
        }
      } else {
        error = msg;
      }
    } finally {
      saving = false;
    }
    return ok;
  }

  /** Verdict-only publish: fires an APPROVE or REQUEST_CHANGES review
   *  even when the user has no draft comments queued. Uses the
   *  standard publish endpoint with a session id (creating one on
   *  demand via `ensureSession`), since the server keys the publish
   *  by session — the endpoint accepts a session with zero drafts so
   *  long as the event is non-neutral (see publish.rs).
   *
   *  Shares saving/error state with the draft-cluster `publishToGithub`
   *  so both paths compete for the same "Publishing…" indicator, and
   *  goes through the same head-drift recovery flow. */
  async function publishVerdict(
    event: 'APPROVE' | 'REQUEST_CHANGES',
    body?: string,
  ): Promise<boolean> {
    const gh = current.manifest.github_pr;
    if (!gh) return false;
    saving = true;
    error = null;
    let ok = false;
    try {
      const sid = await ensureSession();
      await api.publishToGithub(repo, current.manifest.number, sid, event, body);
      await refresh();
      ok = true;
    } catch (e) {
      const msg = (e as Error).message;
      if (e instanceof ApiError && e.kind === 'head_drift') {
        const yes = confirm(
          `${msg}\n\nRe-import this PR now and retry? ` +
          `(Existing imported discussion stays; only the head SHA + ` +
          `any new GitHub comments are refreshed.)`,
        );
        if (yes) {
          try {
            await api.importGithubPr(gh.html_url);
            await refresh();
            const sid = await ensureSession();
            await api.publishToGithub(
              repo,
              current.manifest.number,
              sid,
              event,
              body,
            );
            await refresh();
            error = null;
            ok = true;
          } catch (e2) {
            error = (e2 as Error).message;
          }
        } else {
          error = msg;
        }
      } else {
        error = msg;
      }
    } finally {
      saving = false;
    }
    return ok;
  }

  /** Re-import the PR discussion + head SHA from github.com. Same
   *  code path the head-drift recovery uses in publishToGithub;
   *  exposed as a manual "Refresh" button so the user can pull new
   *  upstream comments (or a moved head) without having to publish
   *  first. Idempotent — every imported item is deduped via the
   *  `github_comment_map` on the server. */
  let refreshingFromGithub = $state(false);
  async function refreshFromGithub() {
    const gh = current.manifest.github_pr;
    if (!gh || refreshingFromGithub) return;
    refreshingFromGithub = true;
    error = null;
    try {
      await api.importGithubPr(gh.html_url);
      await refresh();
    } catch (e) {
      error = (e as Error).message;
    } finally {
      refreshingFromGithub = false;
    }
  }

  /** Visible bottom of the sticky bar stack — i.e. the lowest y in
   *  the viewport that's still covered when the user has scrolled down.
   *  Any scroll target we want flush against the bars has to land
   *  exactly here, not one pixel above (we'd see the previous file's
   *  sticky header in that pixel) and not one pixel below (gap).
   *
   *  Both rows of the top header live in the same `header.app`
   *  element, so its rendered `offsetHeight` is the exact pixel
   *  threshold to subtract when positioning scroll targets. */
  function stickyTop(): number {
    if (typeof document === 'undefined') return 0;
    const header = document.querySelector('header.app') as HTMLElement | null;
    return header?.offsetHeight ?? 0;
  }

  /** Scroll the window so `el`'s top sits just below the sticky bars. */
  function scrollTopOf(el: HTMLElement): void {
    const target = el.getBoundingClientRect().top + window.scrollY - stickyTop();
    window.scrollTo({ top: Math.max(0, target), behavior: 'auto' });
  }

  /** Scroll a specific diff line range into view, highlighting each
   *  row in the range so the reader can see what the permalink
   *  pointed at. Used by the `#L:` permalink handler — locates the
   *  start row by its `data-side` + `data-line` attributes, mounting
   *  the file slot first if it's been virtualized away. Mirrors
   *  `scrollToComment`'s mount-wait pattern so a link landing on a
   *  far-away file still works after the slot's diff fetch resolves
   *  and the layout re-flows. Highlight auto-clears after 4 s. */
  async function scrollToLineRange(
    path: string,
    side: 'base' | 'tip',
    startLine: number,
    endLine: number,
  ) {
    const lineSel = `[data-side="${side}"][data-line="${startLine}"]`;
    let row = document.querySelector(lineSel) as HTMLElement | null;
    if (!row) {
      // Mount the file's slot, then poll for the row to appear.
      const slot = document.querySelector(
        `[data-file-path="${CSS.escape(path)}"]`,
      ) as HTMLElement | null;
      if (!slot) return;
      scrollTopOf(slot);
      const deadline = performance.now() + 10000;
      while (!row && performance.now() < deadline) {
        await new Promise((r) => requestAnimationFrame(r));
        row = document.querySelector(lineSel) as HTMLElement | null;
      }
      if (!row) return;
    }
    if (
      typeof window !== 'undefined' &&
      window.matchMedia('(max-width: 640px)').matches
    ) {
      treeCollapsed = true;
    }
    // Park below BOTH sticky bars (app header + file header) plus a
    // bit of breathing room — same shape as `commentParkTarget`, so
    // a line permalink lands at the same visual position as a comment
    // permalink. Plain `scrollTopOf` only subtracted the app-header
    // height, leaving the row tucked under the file header (or past
    // the top once the layout settled).
    const parkRow = (el: HTMLElement) => {
      const rect = el.getBoundingClientRect();
      const fileHeader = el
        .closest('.file-diff')
        ?.querySelector('.file-header') as HTMLElement | null;
      const fhExtra = fileHeader?.offsetHeight ?? 0;
      const target =
        rect.top + window.scrollY - stickyTop() - fhExtra - COMMENT_CONTEXT;
      window.scrollTo({ top: Math.max(0, target), behavior: 'auto' });
    };
    parkRow(row);
    // Stabilization loop: as virtualized slots above the row mount in,
    // the document re-flows and the row drifts. Re-aim until the row
    // is at a stable position.
    let stableFrames = 0;
    let lastTop = Number.NaN;
    for (let i = 0; i < 30 && stableFrames < 3; i++) {
      await new Promise((r) => requestAnimationFrame(r));
      const cur = document.querySelector(lineSel) as HTMLElement | null;
      if (!cur) return;
      const top = cur.getBoundingClientRect().top;
      if (Number.isFinite(lastTop) && Math.abs(top - lastTop) < 0.5) {
        stableFrames++;
      } else {
        stableFrames = 0;
        parkRow(cur);
      }
      lastTop = top;
    }
    // Highlight every row in the range so the reader can see what
    // the link points at. The `.highlight-anchor` class is the same
    // hover-feedback paint used by the comment gutter marker — re-
    // using it keeps the visual language consistent. Auto-clear
    // after 4 s so it doesn't linger forever; the next mousedown
    // outside the range also feels natural to drop it.
    const highlighted: HTMLElement[] = [];
    for (let ln = startLine; ln <= endLine; ln++) {
      const cells = document.querySelectorAll(
        `[data-side="${side}"][data-line="${ln}"]`,
      );
      for (const c of cells) {
        (c as HTMLElement).classList.add('highlight-anchor');
        highlighted.push(c as HTMLElement);
      }
    }
    setTimeout(() => {
      for (const el of highlighted) el.classList.remove('highlight-anchor');
    }, 4000);
  }

  async function scrollToFile(path: string) {
    const sel = `[data-file-path="${CSS.escape(path)}"]`;
    if (!document.querySelector(sel)) return;
    // On phones the tree is an overlay drawer — close it before we
    // start scrolling so the user actually sees the diff they jumped
    // to (and the layout has already settled into one-pane mode).
    if (
      typeof window !== 'undefined' &&
      window.matchMedia('(max-width: 640px)').matches
    ) {
      treeCollapsed = true;
    }
    // Honour the click immediately: highlight the target in the tree
    // and suppress the scroll-spy so the intermediate scrolls below
    // can't reclaim `activeFilePath` from whatever slot happens to
    // sit at the threshold mid-stabilization.
    activeFilePath = path;
    const myGen = ++navGeneration;
    const superseded = () => myGen !== navGeneration;
    navigating = true;
    const userScroll = watchUserScroll();
    try {
      // Slots above the target are virtualized placeholders sized
      // from an estimate; each one mounts a real FileDiff (and fires
      // an async per-file hunk fetch) as it enters the viewport,
      // re-flowing the document and shoving the target off-screen.
      // A short fixed-frame loop exits before those network round-
      // trips land — the layout then jumps and the reader ends up on
      // the *next* file. Re-park every frame against a generous time
      // budget, the same way `scrollToComment` does, and only quit
      // once the position has held for ~320ms of real stability.
      const TOTAL_TIME_MS = 10000;
      const startTime = performance.now();
      const remaining = () => performance.now() - startTime < TOTAL_TIME_MS;
      let stableFrames = 0;
      let lastTop = Number.NaN;
      const STABLE_REQUIRED = 20;
      while (
        stableFrames < STABLE_REQUIRED &&
        remaining() &&
        !superseded() &&
        !userScroll.interrupted()
      ) {
        const el = document.querySelector(sel) as HTMLElement | null;
        if (!el) return;
        scrollTopOf(el);
        await new Promise((r) => requestAnimationFrame(r));
        const cur = document.querySelector(sel) as HTMLElement | null;
        if (!cur) return;
        const top = cur.getBoundingClientRect().top;
        if (Number.isFinite(lastTop) && Math.abs(top - lastTop) < 0.5) {
          stableFrames++;
        } else {
          stableFrames = 0;
        }
        lastTop = top;
      }
    } finally {
      userScroll.stop();
      // Release only if we're still the latest nav — a newer click
      // owns the flag's lifecycle otherwise. The grace period lets
      // the final re-park's scroll event drain before the spy
      // resumes.
      if (!superseded()) {
        setTimeout(() => {
          if (!superseded()) navigating = false;
        }, 200);
      }
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
        current.manifest.number,
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

<section class="header" data-tour="review-summary">
  <!-- Title (#N name + Archived pill) lives in the top header bar so
       it stays visible while the user scrolls. See App.svelte's
       `.header-row.review` row. -->
  <p class="muted">
    {#if current.manifest.bookmark}bookmark: <strong>{current.manifest.bookmark}</strong> ·{/if}
    revset:
    {#if editingRevset}
      <input
        class="revset-input"
        type="text"
        bind:value={revsetDraft}
        bind:this={revsetInputEl}
        onkeydown={onRevsetKeydown}
        disabled={savingRevset}
        spellcheck="false"
        autocomplete="off"
        autocapitalize="off"
        aria-label="Revset"
      />
      <button
        type="button"
        class="revset-save"
        onclick={saveRevset}
        disabled={savingRevset}
      >{savingRevset ? 'Saving…' : 'Save'}</button>
      <button
        type="button"
        class="revset-cancel"
        onclick={cancelEditRevset}
        disabled={savingRevset}
      >Cancel</button>
    {:else}
      <code>{current.manifest.revset}</code>
      {#if canEditRevset}
        <button
          type="button"
          class="revset-edit"
          onclick={startEditRevset}
          title="Edit revset"
          aria-label="Edit revset"
        >✎</button>
      {/if}
    {/if}
    · by <strong>{current.manifest.created_by}</strong>
  </p>
  {#if revsetError}
    <p class="revset-error" role="alert">⚠ {revsetError}</p>
  {/if}
  <!-- Patchset / compared-to selectors live in the top header
       (App.svelte's row-2 header) so the dropdowns sit alongside the
       review identity that they switch between. Only the base→tip
       endpoint identifiers + the "refresh against the live branch"
       affordance stay here. -->
  <p class="muted patchset-row">
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

{#if current.unread}
  {@const u = current.unread}
  {@const parts = [
    { n: u.new_patchsets ?? 0, one: 'new patchset', many: 'new patchsets' },
    { n: u.new_comments ?? 0, one: 'new comment', many: 'new comments' },
    { n: u.new_replies ?? 0, one: 'new reply', many: 'new replies' },
    { n: u.new_annotations ?? 0, one: 'new annotation', many: 'new annotations' },
  ]
    .filter((p) => p.n > 0)
    .map((p) => `${p.n} ${p.n === 1 ? p.one : p.many}`)}
  {#if parts.length > 0}
    <div class="ops-since-banner" role="status">
      <strong>Since you were here:</strong>
      {parts.join(', ')}
    </div>
  {/if}
{/if}

{#if current.revset_error}
  {@const err = current.revset_error}
  {@const headline = err.message.split('\n')[0] ?? err.message}
  {@const candidates = err.divergent_commits ?? []}
  <div class="revset-error-banner" role="status">
    <p class="headline">
      <strong>Revset can't be resolved:</strong>
      <code>{headline}</code>
    </p>
    {#if candidates.length > 0}
      <p class="resolution">
        Run <code>jj abandon</code> for the version you don't want:
      </p>
      <ul class="divergent-list">
        {#each candidates as c}
          <li>
            <RevId id={c.commit_id} kind="commit" {repo} />
            <span class="when" title={c.author_timestamp}>
              {formatDate(c.author_timestamp)}
            </span>
            {#if c.description_first_line}
              <span class="desc">— {c.description_first_line}</span>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </div>
{/if}

{#if current.diff_unavailable}
  <div class="revset-error-banner" role="status">
    <p class="headline">
      <strong>Diff unavailable.</strong>
      <span>
        The commits this review compares appear to have been
        garbage-collected from the repository, so the file diff and
        commit list can't be shown. Comments and annotations below are
        preserved.
      </span>
    </p>
  </div>
{/if}

<ReviewSummary
  summary={current.manifest.summary}
  editable={(!!viewer && viewer === current.manifest.created_by) || isAdmin}
  {saving}
  onsave={saveSummary}
/>

{#if error}
  <p class="error">{error}</p>
{/if}

<div class="review-layout" class:tree-collapsed={treeCollapsed}>
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
    <FileTree
      files={visibleFiles}
      activePath={activeFilePath}
      onselect={goToFile}
      navTotal={orderedFiles.length}
      navPosition={navFilePosition}
      onprev={fileNavPrev}
      onnext={fileNavNext}
    />
  </aside>
  {#if !treeCollapsed}
    <div
      class="tree-resizer"
      role="separator"
      aria-orientation="vertical"
      aria-label="Resize file tree"
      onpointerdown={startResize}
    ></div>
  {/if}
  <!-- Floating toggle that rides the boundary between the tree and
       the main pane. Visible in both states so the affordance is
       always discoverable; the chevron flips with the state. -->
  <button
    type="button"
    class="panel-toggle"
    class:collapsed={treeCollapsed}
    aria-label={treeCollapsed ? 'Show file tree' : 'Hide file tree'}
    aria-expanded={!treeCollapsed}
    title={treeCollapsed ? 'Show file tree' : 'Hide file tree'}
    onclick={() => (treeCollapsed = !treeCollapsed)}
  ><Chevron dir={treeCollapsed ? 'right' : 'left'} size={12} /></button>
  <div class="main-pane">
    <!-- Sticky bar grouping every comment-level control: lifecycle +
         severity filter chips on the left, prev/next nav and the
         comments-only toggle on the right. These now render in the
         second row of `header.app` (see App.svelte); the viewer
         emits their state through ontoolbarchange. -->
    <!-- In normal mode, the commits panel scopes the file diff to
         base..commit for a single commit. In compare mode, the same
         panel renders the v2 pair list (one row per change-id with a
         status badge) — clicking a 'changed' pair swaps the files
         panel to the per-commit interdiff. CommitsPanel branches
         internally on whether `compareView` is set. -->
      <CommitsPanel
        {repo}
        commits={current.commits}
        comments={visibleComments}
        responses={allResponses}
        selectedChangeId={scopedChangeId}
        currentPatchset={selectedPatchset}
        {reviewAnchorIds}
        {composing}
        {saving}
        lastVisitAt={current.last_visit_at ?? null}
        {viewer}
        {compareView}
        {compareError}
        selectedCompareChange={selectedCompareCommit}
        onselectcomparecommit={selectCompareCommit}
        {commentsWriteable}
        onselect={selectCommit}
        onstartcompose={startCompose}
        oncancelcompose={cancelCompose}
        onsubmit={submitComment}
        onreply={submitResponse}
        onstatus={setStatus}
        ondelete={deleteComment}
        ondeleteresponse={deleteResponse}
        onedit={startEdit}
        onselectpatchset={jumpToPatchset}
      />

    {#if loadingDiff}
      <div class="diff-loading" role="status" aria-live="polite">
        <span class="spinner" aria-hidden="true"></span>
        Loading diff for <code>{loadingDiffLabel}</code>…
      </div>
    {/if}

    {#if compareView}
      <!-- Mode breadcrumb: tells the reader which compare view the
           files panel is currently showing (cumulative diff between
           the two patchsets vs. a single commit's interdiff), with a
           one-click escape back to the cumulative view. Only renders
           in compare mode; non-compare reviews use the normal commits
           panel for this. -->
      <div class="compare-breadcrumb" role="status">
        <span class="label">Showing:</span>
        {#if selectedComparePair}
          {@const p = selectedComparePair}
          {@const badge =
            p.status === 'changed'
              ? '~'
              : p.status === 'added-in-to'
                ? '+'
                : p.status === 'removed-from-from'
                  ? '−'
                  : '='}
          <span class="crumb">
            <strong>{badge}</strong>
            <RevId id={p.change_id} kind="change" {repo} length={8} />
            <span class="truncate"
              >· {p.to_description ?? p.from_description ?? '(no description)'}</span
            >
          </span>
          <button
            type="button"
            class="back-link"
            onclick={() => selectCompareCommit(null)}
            title="Back to cumulative view"
          >
            ← cumulative
          </button>
        {:else}
          <span class="crumb">
            <strong>Cumulative</strong>
            <span class="truncate"
              >· PS{compareView.from.n} → PS{compareView.to.n}
              ({compareView.cumulative.files.length} files)</span
            >
          </span>
        {/if}
      </div>
    {/if}

{#if scopedChangeId && !scopedDiff}
      <!-- We've committed to showing a scoped commit (from
           `?scope=<id>` in the URL) but the commit_diff fetch
           hasn't landed yet. Suppress the file list — without
           this guard `displayedFiles` falls back to the *unscoped*
           review files for one frame, which reads as a confusing
           flash. The placeholder also covers the brief window
           between initial render and `onMount` setting
           `loadingDiff` true, so the user sees a stable "loading"
           state instead of nothing. -->
      <p class="muted">Loading commit diff…</p>
    {:else if orderedFiles.length === 0}
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
          reviewNumber={current.manifest.number}
          file={f}
          patchset={viewingFor}
          {compareWith}
          {compareBaseCommit}
          interdiffEndpoints={interdiffEndpoints}
          {commentsWriteable}
          eagerFetch={filesWithComments.has(f.path)}
          comments={visibleCommentsForFiles}
          annotations={current.annotations ?? []}
          composingAnnotation={composingAnnotation &&
          'file' in composingAnnotation &&
          composingAnnotation.file === f.path
            ? composingAnnotation
            : null}
          {canAnnotate}
          onstartannotate={startAnnotate}
          oncancelannotate={cancelAnnotate}
          onsubmitannotation={submitAnnotation}
          ondeleteannotation={deleteAnnotation}
          oneditannotation={startEditAnnotation}
          responses={allResponses}
          currentPatchset={selectedPatchset}
          composing={composing &&
          'file' in composing &&
          composing.file === f.path
            ? composing
            : null}
          forceRender={!!(composing &&
            'file' in composing &&
            composing.file === f.path) ||
            filesWithReplyInProgress.has(f.path)}
          {showDiffs}
          {showComments}
          {defaultThreadsCollapsed}
          {sbsSplit}
          {setSbsSplit}
          diffCache={fileDiffCache}
          {saving}
          lastVisitAt={current.last_visit_at ?? null}
          {viewer}
          onstartcompose={startCompose}
          oncancelcompose={cancelCompose}
          onsubmit={submitComment}
          onreply={submitResponse}
          onstatus={setStatus}
          ondelete={deleteComment}
          ondeleteresponse={deleteResponse}
          onedit={startEdit}
          onselectpatchset={jumpToPatchset}
        />
      {/each}
    {/if}
  </div>
</div>

{#if helpOpen}
  <!-- Keyboard-shortcut help overlay. Toggled by `?` and dismissed
       by `Esc` (handled in onKey) or by clicking the backdrop. The
       backdrop is a sibling click target with the same role pattern
       Sheet uses — keep it role="presentation" so AT only sees the
       dialog itself. -->
  <div
    class="help-overlay"
    role="presentation"
    onclick={(e) => { if (e.target === e.currentTarget) helpOpen = false; }}
  >
    <div class="help-panel" role="dialog" aria-label="Keyboard shortcuts">
      <div class="help-header">
        <h2>Keyboard shortcuts</h2>
        <button
          type="button"
          class="help-close"
          onclick={() => (helpOpen = false)}
          aria-label="Close shortcuts"
        >×</button>
      </div>
      <dl class="help-list">
        <dt><kbd>/</kbd></dt>
        <dd>Open search</dd>
        <dt><kbd>j</kbd> <span class="help-sep">/</span> <kbd>k</kbd></dt>
        <dd>Next / previous comment</dd>
        <dt><kbd>n</kbd> <span class="help-sep">/</span> <kbd>p</kbd></dt>
        <dd>Next / previous file</dd>
        <dt><kbd>?</kbd></dt>
        <dd>Toggle this help</dd>
        <dt><kbd>Esc</kbd></dt>
        <dd>Close search / this help</dd>
      </dl>
      <p class="help-footnote">
        Letter shortcuts are inert while typing in a comment or any
        other input.
      </p>
    </div>
  </div>
{/if}

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

  /* Pencil button next to the revset code. Faint by default —
   * it's a structural-edit affordance for the creator and shouldn't
   * compete with the revset itself for attention. */
  .revset-edit {
    margin-left: 4px;
    padding: 0 5px;
    background: transparent;
    border: 1px solid transparent;
    border-radius: 3px;
    color: var(--text-faint);
    cursor: pointer;
    font-size: 12px;
    line-height: 1.4;
  }

  .revset-edit:hover {
    background: var(--bg-panel);
    border-color: var(--border);
    color: var(--link);
  }

  /* Inline revset editor — sits in the same paragraph as the rest
   * of the identity line. Sized in `em` so it stretches with the
   * surrounding font. */
  .revset-input {
    margin: 0 6px;
    padding: 2px 6px;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
    width: 28em;
    max-width: 100%;
    box-sizing: border-box;
  }

  .revset-save,
  .revset-cancel {
    margin-right: 4px;
    padding: 2px 10px;
    font-size: 12px;
    border-radius: 4px;
    cursor: pointer;
  }

  .revset-save {
    background: var(--link);
    color: var(--on-accent);
    border: 1px solid var(--link);
  }

  .revset-cancel {
    background: var(--bg);
    border: 1px solid var(--border);
    color: var(--text);
  }

  .revset-save:disabled,
  .revset-cancel:disabled {
    opacity: 0.6;
    cursor: default;
  }

  /* Inline error under the revset row — failed `update_revset`
   * lands here without taking down the whole viewer. */
  .revset-error {
    margin: 4px 0 0 0;
    padding: 4px 10px;
    border: 1px solid var(--error-border);
    border-radius: 4px;
    background: var(--error-bg);
    color: var(--error-text);
    font-size: 12px;
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

  /* Warning banner shown when the review's revset can't be resolved
   * (typically a divergent change ID). The first line of jj's error
   * is inline; the rest of jj's hint output (including the
   * `jj abandon` resolution path) lives on the title attribute so a
   * hover surfaces what to do next. */
  /* "Since you were here" — the activity summary built from
   * `current.ops_since`. Sits above the review summary, same slot as
   * the revset-error banner. Informational (link palette) rather
   * than warning, since there's nothing to fix. */
  .ops-since-banner {
    margin: 12px 0;
    padding: 8px 12px;
    background: var(--link-bg);
    border-left: 3px solid var(--link);
    border-radius: 4px;
    color: var(--text);
    font-size: 13px;
  }

  /* Banner shown when the live revset can't be resolved. Sits above
   * the review summary so the reader sees the problem before they
   * try to act on the diff. For divergent change IDs it lists the
   * conflicting commit IDs inline so the reader can copy one into
   * `jj abandon`. */
  .revset-error-banner {
    margin: 12px 0;
    padding: 8px 12px;
    background: var(--warn-bg);
    border: 1px solid var(--attention-border);
    border-left: 3px solid var(--warn-text);
    border-radius: 4px;
    color: var(--warn-text);
    font-size: 13px;
  }

  .revset-error-banner p {
    margin: 0;
  }

  .revset-error-banner .resolution {
    margin-top: 4px;
    font-size: 12px;
  }

  .revset-error-banner code {
    background: rgba(0, 0, 0, 0.08);
    padding: 1px 5px;
    border-radius: 3px;
    font-size: 12px;
  }

  .revset-error-banner .divergent-list {
    margin: 4px 0 0;
    padding-left: 18px;
    font-size: 12px;
  }

  .revset-error-banner .divergent-list li {
    margin: 2px 0;
  }

  .revset-error-banner .divergent-list .when {
    margin-left: 6px;
    color: var(--muted-fg, currentColor);
    opacity: 0.85;
  }

  .revset-error-banner .divergent-list .desc {
    margin-left: 6px;
    opacity: 0.85;
  }

  .review-layout {
    display: flex;
    align-items: flex-start;
    gap: 0;
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
      /* Darker than a typical scrim — the file-tree drawer is a
       * light panel and at 0.35 the dimmed diff behind it stayed
       * bright enough that the drawer didn't read as modal. */
      background: rgba(0, 0, 0, 0.55);
      border: none;
      padding: 0;
      cursor: pointer;
      z-index: 24;
    }

    .tree-resizer {
      display: none;
    }

    .main-pane {
      margin-left: 0;
    }

    /* Phones use the drawer pattern; the top-bar ☰ button opens
     * and closes it. The in-layout panel-toggle would float over
     * the diff content with nowhere meaningful to dock. */
    .panel-toggle {
      display: none;
    }
  }

  /* Sticky in-layout tree-pane toggle. Anchored after the tree-pane
   * + resizer when expanded (translateX pulls it back over the tree
   * pane's right edge) and at the start of the flex flow when
   * collapsed (translateX shifts it into the page's left padding so
   * it doesn't sit on top of the commits panel or the diff's left
   * gutter). `margin-right: -16px` cancels its own width so the
   * main-pane keeps its flush-left alignment with the description
   * box above it. */
  .panel-toggle {
    position: sticky;
    top: calc(var(--app-header-h) + 24px);
    align-self: flex-start;
    flex: 0 0 16px;
    width: 16px;
    height: 36px;
    margin: 16px -16px 0 0;
    padding: 0;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg);
    color: var(--text-muted);
    cursor: pointer;
    line-height: 1;
    z-index: 11;
    display: flex;
    align-items: center;
    justify-content: center;
    /* Expanded default: ride the right edge of the tree-pane, just
     * inside the panel. Numbers: resizer is 14px (6 + 4 + 4) wide
     * and sits after the tree, so the button's natural left is
     * tree-width + 14; shift back 22 to land at tree-width - 8. */
    transform: translateX(-22px);
  }

  .panel-toggle.collapsed {
    /* Collapsed: button is the first flex child (tree-pane is
     * display:none, no resizer), so its natural left is 0. Shift
     * left into the page padding so it doesn't overlap the diff's
     * "+" gutter or the commits panel's leading chevron. */
    transform: translateX(-20px);
  }

  .panel-toggle:hover {
    background: var(--bg-elevated);
    color: var(--text);
    border-color: var(--text-muted);
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

  /* When the file tree is folded, drop the 8px gutter that was
   * reserved for the resizer — without it the commits panel and the
   * file diffs hang inboard of the description header above, looking
   * misaligned with the rest of the page. */
  .review-layout.tree-collapsed .main-pane {
    margin-left: 0;
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

  /* Patchset-compare v2 mode breadcrumb. Lives above the files panel
     in compare mode, tells the reader which compare-mode view the
     panel is showing, and provides a one-click escape back to the
     cumulative view. */
  .compare-breadcrumb {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 8px 12px;
    margin: 12px 0;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-size: 13px;
  }
  .compare-breadcrumb .label {
    color: var(--muted);
    font-size: 11px;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }
  .compare-breadcrumb .crumb {
    display: flex;
    align-items: baseline;
    gap: 6px;
    min-width: 0;
    flex: 1;
  }
  .compare-breadcrumb .truncate {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    color: var(--muted);
  }
  .compare-breadcrumb .back-link {
    background: transparent;
    border: none;
    color: var(--link, #1f6feb);
    cursor: pointer;
    font: inherit;
    padding: 0;
  }
  .compare-breadcrumb .back-link:hover {
    text-decoration: underline;
  }

  /* Keyboard-shortcut help overlay. Centred modal over a dim
   * backdrop, dismissed via Esc / ?, the close button, or a
   * backdrop click. */
  .help-overlay {
    position: fixed;
    inset: 0;
    z-index: 2000;
    background: rgba(0, 0, 0, 0.45);
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 16px;
  }
  .help-panel {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.25);
    width: min(420px, 100%);
    padding: 18px 22px 20px;
    color: var(--text);
  }
  .help-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin-bottom: 10px;
  }
  .help-header h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
  }
  .help-close {
    background: transparent;
    border: 1px solid transparent;
    color: var(--text-muted);
    font-size: 18px;
    line-height: 1;
    padding: 2px 8px;
    border-radius: 4px;
    cursor: pointer;
  }
  .help-close:hover {
    background: var(--bg-elevated);
    color: var(--text);
  }
  .help-list {
    display: grid;
    grid-template-columns: max-content 1fr;
    gap: 8px 14px;
    margin: 0;
    font-size: 13px;
  }
  .help-list dt {
    display: flex;
    align-items: center;
    gap: 4px;
    color: var(--text);
  }
  .help-list dd {
    margin: 0;
    color: var(--text-muted);
    display: flex;
    align-items: center;
  }
  .help-list kbd {
    display: inline-block;
    min-width: 22px;
    padding: 2px 6px;
    border: 1px solid var(--border);
    border-bottom-width: 2px;
    border-radius: 4px;
    background: var(--bg-elevated);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    text-align: center;
    color: var(--text);
  }
  .help-list .help-sep {
    color: var(--text-faint);
    font-size: 11px;
  }
  .help-footnote {
    margin: 14px 0 0;
    font-size: 12px;
    color: var(--text-muted);
  }
</style>
