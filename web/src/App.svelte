<script lang="ts">
  import { onMount } from 'svelte';
  import { ApiError, api } from './lib/api';
  import { installDiffCopyHandler } from './lib/diffCopy';
  import { subscribe as subscribeEvents } from './lib/events';
  import type {
    RepoSummary,
    ReviewSummary,
    ReviewView,
    WhoAmI,
  } from './lib/types';
  import ActionsMenu from './components/ActionsMenu.svelte';
  import BrowseViewer from './components/BrowseViewer.svelte';
  import Bubble from './components/Bubble.svelte';
  import Chevron from './components/Chevron.svelte';
  import FilterMenu from './components/FilterMenu.svelte';
  import ReviewList from './components/ReviewList.svelte';
  import ReviewSearch from './components/ReviewSearch.svelte';
  import ReviewViewer, { type ReviewToolbarState } from './components/ReviewViewer.svelte';
  import Sheet from './components/Sheet.svelte';
  import DemoOverlay from './demo/DemoOverlay.svelte';

  /** Demo tour gate. `?demo=1` on initial load latches a
   *  `kata:demo:active` flag in localStorage; the overlay then
   *  stays mounted across the navigations the tour itself triggers
   *  (each step's `pushState` would otherwise drop the query param
   *  and unmount us). Skip / Done clears both. */
  const DEMO_ACTIVE_KEY = 'kata:demo:active';
  let showDemo = $state(false);
  function updateDemoGate() {
    if (typeof window === 'undefined') return;
    const fromUrl = new URLSearchParams(window.location.search).get('demo') === '1';
    if (fromUrl) {
      localStorage.setItem(DEMO_ACTIVE_KEY, '1');
    }
    showDemo = localStorage.getItem(DEMO_ACTIVE_KEY) === '1';
  }
  $effect(() => {
    updateDemoGate();
  });


  type Screen =
    | { kind: 'loading'; label: string }
    | { kind: 'list' }
    | {
        kind: 'not-found';
        /** Repo the user asked for — shown in the message and used
         *  for the "back to reviews" CTA. */
        repo: string;
        /** Review number the user asked for. Drives the title. */
        number: number;
        /** Server-provided error string, if any. Surfaced under the
         *  main message so the reader can see exactly what the
         *  backend said ("review #9999 not found", "archived…"). */
        detail: string;
      }
    | {
        kind: 'review';
        repo: string;
        view: ReviewView;
        initialPatchset: number | undefined;
        initialCompareWith: number | undefined;
        initialCommit: string | undefined;
        initialScope: string | undefined;
        debug: boolean;
      }
    | {
        kind: 'browse';
        repo: string;
        /** Pre-selected commit from the URL (`?commit=…`). The
         *  viewer overrides on click. */
        initialCommit: string | undefined;
        /** Pre-selected change from the URL (`?change=…`). The
         *  viewer resolves it to the current commit_id and
         *  canonicalises the URL to `?commit=…`. */
        initialChange: string | undefined;
        /** Pre-opened file path (`?path=…`). When set, the
         *  detail pane shows the file viewer instead of the
         *  commit detail. */
        initialPath: string | undefined;
        /** Revset to start the log on (`?revset=…`). Undefined →
         *  use the server's default. */
        initialRevset: string | undefined;
      };

  // Synchronously decide the initial screen based on the URL, BEFORE the
  // first render. A permalink (`/r/<repo>/<id>`) immediately enters
  // `loading`; without this the user would see the review list flash up
  // during the (async) whoami + listRepos + openReview round-trip.
  function initialScreen(): Screen {
    const m = location.pathname.match(/^\/r\/([^/]+)\/(\d+)$/);
    if (m) {
      return { kind: 'loading', label: `#${m[2]}` };
    }
    const browse = location.pathname.match(/^\/r\/([^/]+)\/browse$/);
    if (browse) {
      const params = new URLSearchParams(location.search);
      return {
        kind: 'browse',
        repo: decodeURIComponent(browse[1]),
        initialCommit: params.get('commit') ?? undefined,
        initialChange: params.get('change') ?? undefined,
        initialPath: params.get('path') ?? undefined,
        initialRevset: params.get('revset') ?? undefined,
      };
    }
    return { kind: 'list' };
  }

  let screen: Screen = $state(initialScreen());
  let repos: RepoSummary[] = $state([]);
  let repo: string = $state('');
  let summaries: ReviewSummary[] | null = $state(null);
  let whoami: WhoAmI | null = $state(null);
  let error: string | null = $state(null);
  let loading: boolean = $state(false);
  /** Bumped every time the browser fires a `popstate` (back / forward).
   *  Threaded into the ReviewViewer `{#key}` block so an external URL
   *  rewind forces a fresh mount even when the URL parses to the same
   *  initial fields the viewer was originally mounted against. Without
   *  this, ReviewViewer's *internal* state can diverge from its mount-
   *  time state (e.g. an "Added in PSx" chip switches the patchset
   *  in-place), and a back to the no-`?ps=` URL would leave the viewer
   *  stuck on the in-app-navigated patchset because the key didn't
   *  change. The counter is the smallest fix that doesn't add a stale
   *  remount on regular in-app navigation. */
  let popstateGen = $state(0);

  /** `pathname + search` (everything but the hash) of the URL the
   *  current `screen` was built against. A `popstate` whose URL still
   *  matches this is a hash-only change: ReviewViewer's in-review
   *  jumps (`#file-`, `#c-`, `#L:`) set `location.hash`, which fires
   *  popstate. That must NOT rebuild `screen` or bump `popstateGen` —
   *  doing so remounts the viewer mid-jump, throwing away the file
   *  tree's scroll position and re-fetching every diff. ReviewViewer's
   *  own `hashchange` listener owns the scroll for those. */
  let syncedNavUrl = '';
  function rememberNavUrl() {
    syncedNavUrl = location.pathname + location.search;
  }
  /** Mirrored from ReviewViewer so its review-level controls (publish /
   *  discard, diff-collapse toggle, etc.) can live in the sticky top bar —
   *  always reachable while scrolling, instead of in a banner inside the
   *  scrolling document. */
  let toolbar: ReviewToolbarState | null = $state.raw(null);

  /** Bound to `<header class="app">` so we can re-publish its rendered
   *  height as `--app-header-h`. The header is one row on the home
   *  screen and two rows on a review page, so the offset that every
   *  sticky file-header / tree-pane uses needs to track it
   *  dynamically. The static fallback in app.css covers the very first
   *  paint before the observer is wired. */
  let headerEl: HTMLElement | undefined = $state();

  /** True on phone-width viewports. The review header is a different
   *  shape there — a compact two-row layout with the filter chips and
   *  the overflow controls tucked into bottom sheets — so the markup
   *  branches on this rather than leaning on CSS reflow alone. */
  let narrowViewport = $state(false);
  $effect(() => {
    const mq = window.matchMedia('(max-width: 640px)');
    narrowViewport = mq.matches;
    const update = (e: MediaQueryListEvent) => (narrowViewport = e.matches);
    mq.addEventListener('change', update);
    return () => mq.removeEventListener('change', update);
  });

  /** Which mobile header sheet is open, if any. The filter sheet
   *  holds the status / severity chips; the "More" sheet holds the
   *  patchset pickers, commit navigation, and scroll-to-top — all
   *  the controls that don't fit the compact two-row phone header. */
  let filterSheetOpen = $state(false);
  let moreSheetOpen = $state(false);

  /** Count of active (off) filter chips, for the ⚑ button's badge. */
  function activeFilterCount(f: NonNullable<ReviewToolbarState['filter']>): number {
    let n = 0;
    for (const on of Object.values(f.status)) if (!on) n++;
    for (const on of Object.values(f.flag)) if (!on) n++;
    return n;
  }

  /** Hide-on-scroll for the sticky header on phones. The two-row
   *  review header eats ~200px of an 812px viewport; sliding it out
   *  of the way while the reader scrolls down into the diff reclaims
   *  that space, and a scroll-up gesture brings it straight back.
   *  Desktop keeps the header permanently pinned. */
  let headerHidden = $state(false);
  $effect(() => {
    const mq = window.matchMedia('(max-width: 640px)');
    let lastY = window.scrollY;
    function onScroll() {
      if (!mq.matches) {
        headerHidden = false;
        return;
      }
      const y = window.scrollY;
      const delta = y - lastY;
      // Always reveal near the top; otherwise react to a deliberate
      // scroll (the >6px guard ignores sub-pixel / momentum jitter).
      if (y < (headerEl?.offsetHeight ?? 48)) {
        headerHidden = false;
      } else if (delta > 6) {
        headerHidden = true;
      } else if (delta < -6) {
        headerHidden = false;
      }
      lastY = y;
    }
    window.addEventListener('scroll', onScroll, { passive: true });
    return () => window.removeEventListener('scroll', onScroll);
  });

  $effect(() => {
    if (!headerEl) return;
    // Publish the header's rendered height as `--app-header-h` for
    // every sticky file-header / tree-pane offset. When the header
    // is hidden the offset collapses to 0 so those stickies ride up
    // to the viewport top instead of leaving a 44px dead band.
    const update = () => {
      document.documentElement.style.setProperty(
        '--app-header-h',
        headerHidden ? '0px' : `${headerEl!.offsetHeight}px`,
      );
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(headerEl);
    return () => ro.disconnect();
  });

  /** Full view state for a review URL. All fields optional — absent
   *  means "use the default" (latest patchset, no compare, no scope,
   *  no pair selected). */
  interface ReviewViewState {
    patchset?: number;
    compareWith?: number | null;
    commit?: string | null;  // compare-mode pair selection
    scope?: string | null;   // non-compare commit-panel scoping
    debug?: boolean;         // `?debug` opt-in; preserved across nav
  }

  function pathForReview(
    repo: string,
    number: number,
    state: ReviewViewState = {},
  ): string {
    const base = `/r/${encodeURIComponent(repo)}/${number}`;
    const parts: string[] = [];
    if (state.patchset !== undefined) parts.push(`ps=${state.patchset}`);
    if (state.compareWith != null) parts.push(`cmp=${state.compareWith}`);
    // `commit` only carries meaning in compare mode (it selects a
    // change-id from the pair list); leave it off otherwise so URLs
    // don't grow stale params.
    if (state.commit != null && state.compareWith != null) {
      parts.push(`commit=${encodeURIComponent(state.commit)}`);
    }
    // `scope` is the non-compare counterpart of `commit`: a change-id
    // scoping the file panel to that one commit. Doesn't compose with
    // compare mode (where the pair-list selection takes its place).
    if (state.scope != null && state.compareWith == null) {
      parts.push(`scope=${encodeURIComponent(state.scope)}`);
    }
    // Preserve `?debug` across in-app navigation so the user doesn't
    // have to re-type it after every patchset/compare/scope change.
    if (state.debug) parts.push('debug');
    return parts.length > 0 ? `${base}?${parts.join('&')}` : base;
  }

  /** Parse `/r/<repo>/<number>` (with optional `?ps=N`, `?cmp=M`,
   *  `?commit=<id>`, `?scope=<id>`, `?debug`). Returns null when the
   *  URL is the review list (or when `<number>` isn't a positive
   *  integer). */
  function parseUrl():
    | {
        repo: string;
        number: number;
        patchset: number | undefined;
        compareWith: number | undefined;
        commit: string | undefined;
        scope: string | undefined;
        debug: boolean;
      }
    | null {
    const m = location.pathname.match(/^\/r\/([^/]+)\/(\d+)$/);
    if (!m) return null;
    const params = new URLSearchParams(location.search);
    const readNum = (key: string): number | undefined => {
      const raw = params.get(key);
      if (raw === null) return undefined;
      const n = Number(raw);
      return Number.isFinite(n) ? n : undefined;
    };
    return {
      repo: decodeURIComponent(m[1]),
      number: Number(m[2]),
      patchset: readNum('ps'),
      compareWith: readNum('cmp'),
      commit: params.get('commit') ?? undefined,
      scope: params.get('scope') ?? undefined,
      // `?debug` (with or without a value) turns on debug
      // affordances — currently the per-file "show jj command"
      // icon. Not surfaced in the UI to enable; users opt in by
      // typing `?debug` (or `?debug=1`) into the URL bar.
      debug: params.has('debug'),
    };
  }

  async function loadList(targetRepo: string) {
    if (!targetRepo) {
      summaries = [];
      return;
    }
    loading = true;
    error = null;
    try {
      summaries = await api.listReviews(targetRepo);
    } catch (e) {
      error = (e as Error).message;
    } finally {
      loading = false;
    }
  }

  async function showReview(
    targetRepo: string,
    number: number,
    patchset: number | undefined,
    compareWith: number | undefined,
    commit: string | undefined,
    scope: string | undefined,
    debug: boolean,
  ) {
    loading = true;
    error = null;
    try {
      const view = await api.openReview(targetRepo, number, patchset, compareWith);
      screen = {
        kind: 'review',
        repo: targetRepo,
        view,
        initialPatchset: patchset,
        initialCompareWith: compareWith,
        initialCommit: commit,
        initialScope: scope,
        debug,
      };
    } catch (e) {
      // 404 → render a dedicated not-found page rather than silently
      // dropping to the review list. The user's URL stays intact in
      // the URL bar so they can correct a typo'd review number, and
      // they get an explicit "this review doesn't exist" message
      // instead of an empty list with their bad URL hovering above.
      // Other errors (network, 5xx) still fall back to the list with
      // an error banner — those tend to be transient and the list is
      // a reasonable safe haven.
      if (e instanceof ApiError && e.status === 404) {
        screen = {
          kind: 'not-found',
          repo: targetRepo,
          number,
          detail: e.detail,
        };
      } else {
        error = (e as Error).message;
        screen = { kind: 'list' };
        await loadList(targetRepo);
      }
    } finally {
      loading = false;
    }
  }

  /** Navigate to a review (called by user click — pushes history). */
  async function openReview(number: number) {
    const path = pathForReview(repo, number);
    if (location.pathname + location.search !== path) {
      history.pushState({}, '', path);
    }
    rememberNavUrl();
    await showReview(repo, number, undefined, undefined, undefined, undefined, false);
  }

  /** Called by BrowseViewer when its internal selection, file, or
   *  revset changes. Updates the URL via replaceState so the
   *  browser history isn't spammed with every commit click (only
   *  the entry point to /browse gets a real history entry). */
  function onBrowseStateChange(state: {
    commit?: string;
    path?: string;
    revset?: string;
  }) {
    if (screen.kind !== 'browse') return;
    const params = new URLSearchParams();
    if (state.commit) params.set('commit', state.commit);
    if (state.path) params.set('path', state.path);
    if (state.revset) params.set('revset', state.revset);
    const qs = params.toString();
    const target = `/r/${encodeURIComponent(screen.repo)}/browse${qs ? `?${qs}` : ''}`;
    if (location.pathname + location.search !== target) {
      history.replaceState({}, '', target);
    }
    rememberNavUrl();
  }

  /** Called by ReviewViewer when any of its view-state fields change
   *  via in-app navigation (patchset selector, compare-with selector,
   *  pair-list click in compare mode, commits-panel scoping click in
   *  non-compare mode). Pushes a new history entry so the browser
   *  back button undoes the last action. The previous behaviour
   *  (replaceState) made back skip past everything the user did
   *  inside the review and jump straight back to where they entered
   *  it — the exact bug this fixes. */
  function onViewChange(state: ReviewViewState) {
    if (screen.kind !== 'review') return;
    const path = pathForReview(
      screen.repo,
      screen.view.manifest.number,
      // Preserve the debug flag — ReviewViewer doesn't know about it
      // (it's a URL-only toggle), and dropping it on every patchset /
      // compare switch would make `?debug` feel broken.
      { ...state, debug: screen.debug },
    );
    if (location.pathname + location.search !== path) {
      history.pushState({}, '', path);
    }
    rememberNavUrl();
  }

  /** Navigate to the per-repo review list (the "home" screen). Used
   *  by the Kata logo in the header; pushes history so the browser
   *  back button still works to return to the review the user came
   *  from. */
  function goHome() {
    if (location.pathname + location.search !== '/') {
      history.pushState({}, '', '/');
    }
    void syncFromUrl();
  }

  async function switchRepo(name: string) {
    if (name === repo) return;
    repo = name;
    await loadList(name);
  }

  /** Reflect the current URL into `screen`. Runs on mount and on popstate. */
  async function syncFromUrl() {
    rememberNavUrl();
    // `/r/<repo>/browse` routes through the browse pane.
    const browseMatch = location.pathname.match(/^\/r\/([^/]+)\/browse$/);
    if (browseMatch) {
      const browseRepo = decodeURIComponent(browseMatch[1]);
      if (!repos.some((r) => r.name === browseRepo)) {
        screen = { kind: 'list' };
        if (repos[0]) await switchRepo(repos[0].name);
        return;
      }
      repo = browseRepo;
      const params = new URLSearchParams(location.search);
      screen = {
        kind: 'browse',
        repo: browseRepo,
        initialCommit: params.get('commit') ?? undefined,
        initialChange: params.get('change') ?? undefined,
        initialPath: params.get('path') ?? undefined,
        initialRevset: params.get('revset') ?? undefined,
      };
      return;
    }
    const parsed = parseUrl();
    if (parsed) {
      // Make sure the named repo is known; fall back to list if not.
      if (!repos.some((r) => r.name === parsed.repo)) {
        screen = { kind: 'list' };
        if (repos[0]) await switchRepo(repos[0].name);
        return;
      }
      repo = parsed.repo;
      await showReview(
        parsed.repo,
        parsed.number,
        parsed.patchset,
        parsed.compareWith,
        parsed.commit,
        parsed.scope,
        parsed.debug,
      );
    } else {
      screen = { kind: 'list' };
      if (!repo && repos[0]) repo = repos[0].name;
      await loadList(repo);
    }
  }

  onMount(() => {
    // Rewrite the clipboard payload when copying inside a diff so
    // the result pastes as plain code, not as the underlying HTML
    // table. Installed once for the app lifetime; the handler is a
    // no-op when the selection isn't inside a diff cell.
    const uninstallDiffCopy = installDiffCopyHandler();

    // Toggle `body.dragging-in-diff` whenever a drag starts inside
    // a `.hunks-wrapper`. Used by `.file-header .path` CSS to
    // re-block selection while the user is mid-drag (so the drag
    // can't spill into the next file's header), while leaving the
    // path selectable on plain clicks. See FileDiff.svelte for
    // the matching CSS rule.
    const onAnyMouseDown = (e: MouseEvent) => {
      const t = e.target as Element | null;
      if (t && t.closest('.hunks-wrapper')) {
        document.body.classList.add('dragging-in-diff');
      }
    };
    const onAnyMouseUp = () => {
      document.body.classList.remove('dragging-in-diff');
    };
    document.addEventListener('mousedown', onAnyMouseDown);
    document.addEventListener('mouseup', onAnyMouseUp);
    const unsubscribe = subscribeEvents((event) => {
      if (
        screen.kind === 'list' &&
        event.repo === repo &&
        (event.kind === 'review-created' ||
          event.kind === 'review-updated' ||
          event.kind === 'review-deleted')
      ) {
        void loadList(repo);
      }
      // If the currently-open review was deleted (from this tab's
      // own request, or another tab's), drop back to the list. The
      // viewer would otherwise sit on a stale manifest until the
      // next manual refresh.
      if (
        screen.kind === 'review' &&
        event.kind === 'review-deleted' &&
        event.repo === repo &&
        event.review_id === screen.view.manifest.review_id
      ) {
        history.replaceState({}, '', '/');
        rememberNavUrl();
        screen = { kind: 'list' };
        void loadList(repo);
      }
    });
    window.addEventListener('popstate', async () => {
      // A popstate whose pathname+search still match what `screen` was
      // built against is a hash-only change — ReviewViewer sets
      // `location.hash` for its `#file-`/`#c-`/`#L:` in-review jumps,
      // and that fires popstate. ReviewViewer's own `hashchange`
      // listener does the scroll; rebuilding `screen` / bumping
      // `popstateGen` here would remount the viewer mid-jump and lose
      // the file tree's scroll position.
      if (location.pathname + location.search === syncedNavUrl) return;
      // Await syncFromUrl so `screen` is rebuilt against the restored
      // URL before we bump the gen counter. If the new URL produces a
      // different `screen.initialPatchset` the {#key} block will have
      // already re-keyed by the time the bump lands — the bump
      // mainly matters when the URL parses to the SAME initial
      // fields (and ReviewViewer's in-app navigation is what
      // diverged in the meantime).
      await syncFromUrl();
      popstateGen++;
      updateDemoGate();
    });
    (async () => {
      try {
        whoami = await api.whoami();
        repos = await api.listRepos();
      } catch (e) {
        // Server-side OIDC mode (or any auth mode whose missing
        // credentials produce a 401) lands here. Bounce the user
        // through the IdP rather than parking them on a generic
        // error: with no cookie, every subsequent API call would
        // 401 anyway, and the SPA has nothing useful to show.
        if (e instanceof ApiError && e.status === 401) {
          const next = encodeURIComponent(location.pathname + location.search);
          location.assign(`/auth/login?next=${next}`);
          return;
        }
        error = (e as Error).message;
      }
      await syncFromUrl();
    })();
    return () => {
      unsubscribe();
      uninstallDiffCopy();
    };
  });
</script>

<!-- Header control pieces, defined once and rendered in either the
     desktop two-row header or the compact phone header (and its
     sheets) below. Snippets live at component scope, not inside
     `<header>`, so the sheet blocks after `</header>` can reach
     them too. -->
{#snippet scrollTopUI()}
    <button
      type="button"
      onclick={() => window.scrollTo({ top: 0, behavior: 'smooth' })}
      title="Scroll to the top of the review"
      aria-label="Scroll to top"
    >↑ <span class="lbl">Top</span></button>
  {/snippet}

  {#snippet commitNavUI()}
    {#if toolbar?.commits}
      {@const commits = toolbar.commits}
      <div
        class="commit-nav"
        role="group"
        aria-label="Commit navigation"
        title={commits.label}
      >
        <button onclick={commits.prev} title="Previous commit" aria-label="Previous commit"
          ><Chevron dir="left" /></button>
        <span class="position">
          {commits.position === 0 ? 'All' : commits.position}/{commits.total}
        </span>
        <button onclick={commits.next} title="Next commit" aria-label="Next commit"
          ><Chevron dir="right" /></button>
        <span class="commit-label">{commits.label}</span>
      </div>
    {/if}
  {/snippet}

  {#snippet searchUI()}
    {#if toolbar?.search}
      {@const s = toolbar.search}
      <ReviewSearch
        open={s.open}
        query={s.query}
        total={s.total}
        position={s.position}
        loading={s.loading}
        onqueryInput={s.onQueryInput}
        onnext={s.onNext}
        onprev={s.onPrev}
        onopen={s.onOpen}
        onclose={s.onClose}
      />
    {/if}
  {/snippet}

  {#snippet draftClusterUI()}
    {#if toolbar?.drafts}
      {@const drafts = toolbar.drafts}
      <div class="action-cluster">
        <div class="draft-nav" role="group" aria-label="Draft navigation">
          {#if drafts.nav}
            {@const nav = drafts.nav}
            <button type="button" onclick={nav.prev} title="Previous draft" aria-label="Previous draft"
              ><Chevron dir="left" /></button>
            <span class="draft-count" aria-live="polite">
              {nav.position || '–'}/<strong>{drafts.count}</strong>
              <span class="lbl">draft{drafts.count === 1 ? '' : 's'}</span>
            </span>
            <button type="button" onclick={nav.next} title="Next draft" aria-label="Next draft"
              ><Chevron dir="right" /></button>
          {:else}
            <span class="draft-count" aria-live="polite">
              <strong>{drafts.count}</strong>
              <span class="lbl">draft{drafts.count === 1 ? '' : 's'}</span>
            </span>
          {/if}
        </div>
        <button onclick={drafts.discard} disabled={drafts.saving}>Discard</button>
        <button class="primary" onclick={drafts.publish} disabled={drafts.saving}>
          {drafts.saving ? 'Publishing…' : 'Publish'}
        </button>
      </div>
    {/if}
  {/snippet}

  {#snippet patchsetUI()}
    {#if toolbar?.patchsets}
      {@const ps = toolbar.patchsets}
      <span class="ps-picker-group" data-tour="patchset-picker">
        <label class="ps-picker">
          <span class="muted">Patchset</span>
          <select
            value={ps.selected}
            onchange={(e) =>
              ps.select(Number((e.currentTarget as HTMLSelectElement).value))}
          >
            {#each ps.options as opt (opt.n)}
              <option value={opt.n}>{opt.label}</option>
            {/each}
          </select>
        </label>
        <label class="ps-picker">
          <span class="muted">compared to</span>
          <select
            value={ps.compareWith ?? ''}
            onchange={(e) => {
              const v = (e.currentTarget as HTMLSelectElement).value;
              ps.selectCompareWith(v === '' ? null : Number(v));
            }}
          >
            <option value="">base</option>
            {#each ps.options as opt (opt.n)}
              {#if opt.n !== ps.selected}
                <option value={opt.n}>PS{opt.n}</option>
              {/if}
            {/each}
          </select>
        </label>
      </span>
    {/if}
  {/snippet}

  {#snippet commentNavUI()}
    {#if toolbar?.comments}
      {@const c = toolbar.comments}
      <div
        class="comment-nav"
        role="group"
        aria-label="Comment navigation"
        data-tour="comment-nav"
      >
        <!-- Bubble glyph marks this as the *comment* navigator — without
             it, it's an unlabelled `</>` pair that reads identically to
             the draft navigator sitting one row below. -->
        <span class="nav-icon" aria-hidden="true"><Bubble size={13} /></span>
        <button type="button" onclick={c.prev} title="Previous comment" aria-label="Previous comment"
          ><Chevron dir="left" /></button>
        <span class="position" aria-live="polite">
          {c.position || '–'}/{c.total}
        </span>
        <button type="button" onclick={c.next} title="Next comment" aria-label="Next comment"
          ><Chevron dir="right" /></button>
      </div>
    {/if}
  {/snippet}

  {#snippet filterChipsUI()}
    {#if toolbar?.filter}
      {@const filter = toolbar.filter}
      <div class="filter-chips" data-tour="filter-chips">
        <span class="label">Status</span>
        <button type="button" class="chip status-draft" class:on={filter.status.draft}
          aria-pressed={filter.status.draft} onclick={() => filter.toggleStatus('draft')}>Draft</button>
        <button type="button" class="chip status-open" class:on={filter.status.open}
          aria-pressed={filter.status.open} onclick={() => filter.toggleStatus('open')}>Open</button>
        <button type="button" class="chip status-resolved" class:on={filter.status.resolved}
          aria-pressed={filter.status.resolved} onclick={() => filter.toggleStatus('resolved')}>Resolved</button>
        <span class="sep" aria-hidden="true"></span>
        <span class="label">Severity</span>
        <button type="button" class="chip flag-must-do" class:on={filter.flag['must-do']}
          aria-pressed={filter.flag['must-do']} onclick={() => filter.toggleFlag('must-do')}>Must do</button>
        <button type="button" class="chip flag-suggestion" class:on={filter.flag.suggestion}
          aria-pressed={filter.flag.suggestion} onclick={() => filter.toggleFlag('suggestion')}>Suggestion</button>
        <button type="button" class="chip flag-question" class:on={filter.flag.question}
          aria-pressed={filter.flag.question} onclick={() => filter.toggleFlag('question')}>Question</button>
      </div>
    {/if}
  {/snippet}

  {#snippet viewToggleUI()}
    {#if toolbar?.view}
      {@const v = toolbar.view}
      <div class="view-toggle" role="radiogroup" aria-label="View mode" data-tour="view-toggle">
        <button type="button" class="seg" class:on={v.mode === 'both'} role="radio"
          aria-checked={v.mode === 'both'} onclick={() => v.set('both')}
          title="Show diffs and comment threads">Both</button>
        <button type="button" class="seg" class:on={v.mode === 'diffs'} role="radio"
          aria-checked={v.mode === 'diffs'} onclick={() => v.set('diffs')}
          title="Show only the diffs, hide comment threads">Diffs</button>
        <button type="button" class="seg" class:on={v.mode === 'comments'} role="radio"
          aria-checked={v.mode === 'comments'} onclick={() => v.set('comments')}
          title="Show only the comments, hide the diffs">Comments</button>
      </div>
    {/if}
  {/snippet}

<header class="app" class:header-hidden={headerHidden} bind:this={headerEl}>
  {#if narrowViewport && screen.kind === 'review' && toolbar}
    <!-- ===== Phone review header: two coherent rows ===== -->
    {@const t = toolbar}
    <!-- Row 1 — identity: where am I + the two ways out (search, More). -->
    <div class="m-row m-identity">
      <button
        class="tree-button"
        type="button"
        onclick={t.tree.toggle}
        aria-label="Toggle file list"
        aria-expanded={!t.tree.collapsed}
      >☰</button>
      <h1>
        <a
          href="/"
          class="home-link"
          onclick={(e) => { e.preventDefault(); goHome(); }}
          aria-label="Kata — back to review list"
        >
          <img class="app-icon" src="/favicon.svg" alt="" width="22" height="22" />
          <span class="m-wordmark">Kata</span>
        </a>
      </h1>
      {#if t.title}
        <span class="m-crumb" aria-hidden="true">›</span>
        <span class="review-title">
          <span class="review-number">#{t.title.number}</span>
          <span class="review-name">{t.title.name}</span>
          {#if t.title.archived}
            <span class="archived-badge" title="Archived — read-only until unarchived">Archived</span>
          {/if}
        </span>
      {/if}
      {#if loading}<span class="spinner" aria-label="loading"></span>{/if}
      <span style="flex: 1"></span>
      <!-- The search wrapper drops to a full-width line of its own
           when expanded (see `.m-search.open`), so the More button
           stays put on row 1 instead of being orphaned below it. -->
      <span class="m-search" class:open={t.search?.open}>
        {@render searchUI()}
      </span>
      <button
        class="m-icon-btn"
        type="button"
        onclick={() => (moreSheetOpen = true)}
        aria-label="More controls"
        aria-haspopup="dialog"
      >⋯</button>
    </div>
    <!-- Row 2 — toolbar: the controls used constantly while reading. -->
    <div class="m-row m-toolbar">
      {@render commentNavUI()}
      {@render viewToggleUI()}
      <span style="flex: 1"></span>
      {#if t.filter}
        {@const fc = activeFilterCount(t.filter)}
        <button
          class="m-filter-btn"
          class:active={fc > 0}
          type="button"
          onclick={() => (filterSheetOpen = true)}
          aria-label="Filter comments"
          aria-haspopup="dialog"
        >
          <span class="m-filter-glyph" aria-hidden="true">⚑</span>
          Filter
          {#if fc > 0}<span class="m-filter-badge">{fc}</span>{/if}
        </button>
      {/if}
    </div>
    {@render draftClusterUI()}
  {:else}
  <!-- Row 1: global app controls. Always present. -->
  <div class="header-row primary">
    {#if toolbar}
      <!-- Phone-only file-tree toggle. Hidden on desktop via CSS. -->
      <button
        class="tree-button"
        type="button"
        onclick={toolbar.tree.toggle}
        aria-label="Toggle file list"
        aria-expanded={!toolbar.tree.collapsed}
      >☰</button>
    {/if}
    <h1>
      <a
        href="/"
        class="home-link"
        onclick={(e) => {
          // Intercept so the in-app router takes the navigation;
          // a full `<a href="/">` reload would discard the SPA
          // state and force a fresh fetch.
          e.preventDefault();
          goHome();
        }}
        aria-label="Kata — back to review list"
      >
        <img class="app-icon" src="/favicon.svg" alt="" width="22" height="22" />
        Kata
      </a>
    </h1>
    {#if screen.kind === 'review'}
      {@render scrollTopUI()}
      {@render commitNavUI()}
    {/if}
    {#if loading}
      <span class="spinner" aria-label="loading"></span>
    {/if}
    <span style="flex: 1"></span>
    <!-- Search lives in row 1 (the always-on top bar) rather than
         row 2: row 2 is already crowded with the review identity
         plus chips + nav + view toggle. Row 1's right side has
         fewer obligations, so the search bar's expanded width fits
         without wrapping. -->
    {@render searchUI()}
    {@render draftClusterUI()}
    {#if whoami}
      <span class="author">signed in as {whoami.author}</span>
    {/if}
  </div>

  <!-- Row 2: review-scoped state (title, comment filter chips, comment
       navigation, comments-only toggle). Renders only on a review
       page once the viewer has reported its toolbar state. Pinning
       these controls in a fixed-at-top row solves the problem the
       previous in-body sticky comment-bar had: clicking `< >`
       repeatedly used to chase the bar around as the page scrolled. -->
  {#if screen.kind === 'review' && toolbar?.title}
    {@const title = toolbar.title}
    <div class="header-row review">
      <span class="review-title">
        <span class="review-number">#{title.number}</span>
        <span class="review-name">{title.name}</span>
        {#if title.archived}
          <span class="archived-badge" title="Archived — read-only until unarchived">
            Archived
          </span>
        {/if}
        {#if toolbar.actions}
          {@const a = toolbar.actions}
          {@const suffix = a.manageable ? '' : ' (creator only)'}
          <ActionsMenu
            label="Review actions"
            items={[
              {
                label: `${a.archived ? 'Unarchive' : 'Archive'}${suffix}`,
                onclick: () => void a.archive(),
                disabled: a.busy || !a.manageable,
              },
              {
                label: `Delete…${suffix}`,
                onclick: () => void a.delete(),
                danger: true,
                disabled: a.busy || !a.manageable,
              },
            ]}
          />
        {/if}
      </span>
      {@render patchsetUI()}
      <!-- Float controls to the right so the title gets breathing
           room from the chips next to it. -->
      <span style="flex: 1"></span>
      <!-- Order is `nav | hint | chips | view-toggle` rather than the
           visual-reading-order opposite so the chip cluster stays
           anchored against the view-toggle at the right edge: when
           the nav or hint disappears (no comments, filter not empty),
           only the elements between the title spacer and the chips
           shift — the chips themselves keep their position. -->
      {@render commentNavUI()}
      {#if toolbar.filter && toolbar.filter.hiddenCount > 0}
        <button
          type="button"
          class="filter-empty-hint"
          onclick={toolbar.filter.reset}
          title="All chips off — click to restore"
        >
          Filter hides {toolbar.filter.hiddenCount}
          {toolbar.filter.hiddenCount === 1 ? 'comment' : 'comments'} — show all
        </button>
      {/if}
      <!-- Two presentations of the same filter cluster, gated by a
           CSS media query at 1280px. Wide widths keep the inline
           chip strip (high-glance discoverability); narrower desktop
           widths fold the six chips into a popover button so the row
           stops wrapping onto two sub-rows. The popover is only
           hidden visually — the data binding is identical, so a
           viewport-width change while filters are active doesn't
           lose state. -->
      <span class="filter-inline">{@render filterChipsUI()}</span>
      {#if toolbar.filter}
        <span class="filter-popover">
          <FilterMenu
            filter={toolbar.filter}
            activeCount={activeFilterCount(toolbar.filter)}
          />
        </span>
      {/if}
      {@render viewToggleUI()}
    </div>
  {/if}
  {/if}
</header>

{#if narrowViewport && filterSheetOpen && toolbar?.filter}
  {@const filter = toolbar.filter}
  <Sheet title="Filter comments" onclose={() => (filterSheetOpen = false)}>
    {@render filterChipsUI()}
    {#if filter.hiddenCount > 0}
      <button
        type="button"
        class="sheet-reset"
        onclick={() => { filter.reset(); filterSheetOpen = false; }}
      >
        Show all — {filter.hiddenCount} hidden
      </button>
    {/if}
  </Sheet>
{/if}

{#if narrowViewport && moreSheetOpen && toolbar}
  {@const t = toolbar}
  <Sheet title="More" onclose={() => (moreSheetOpen = false)}>
    <div class="more-sheet">
      {#if t.patchsets}
        <div class="more-row">
          <span class="more-label">Patchset</span>
          {@render patchsetUI()}
        </div>
      {/if}
      {#if t.commits}
        <div class="more-row">
          <span class="more-label">Commit</span>
          {@render commitNavUI()}
        </div>
      {/if}
      <div class="more-row">
        <span class="more-label">Page</span>
        {@render scrollTopUI()}
      </div>
      {#if t.actions}
        {@const a = t.actions}
        <div class="more-row more-actions">
          <span class="more-label">Review</span>
          <button
            type="button"
            onclick={() => { void a.archive(); moreSheetOpen = false; }}
            disabled={a.busy || !a.manageable}
          >{a.archived ? 'Unarchive' : 'Archive'}</button>
          <button
            type="button"
            class="danger"
            onclick={() => { void a.delete(); moreSheetOpen = false; }}
            disabled={a.busy || !a.manageable}
          >Delete…</button>
        </div>
      {/if}
    </div>
  </Sheet>
{/if}

<main class:wide={screen.kind === 'review'} class:browse-main={screen.kind === 'browse'}>
  {#if error}
    <p class="error">{error}</p>
  {/if}

  {#if screen.kind === 'loading'}
    <p class="loading-permalink">
      <span class="spinner" aria-hidden="true"></span>
      Loading review <code>{screen.label}</code>…
    </p>
  {:else if screen.kind === 'not-found'}
    {@const nf = screen}
    <div class="not-found-page" role="alert">
      <h2>Review #{nf.number} not found</h2>
      <p class="not-found-detail">
        {nf.detail || `No review with number ${nf.number} exists in “${nf.repo}”.`}
      </p>
      <p class="not-found-hint muted">
        The link may have been mistyped, or the review may have been
        deleted. Check the URL, or browse the review list to find it.
      </p>
      <div class="not-found-cta">
        <button
          type="button"
          class="primary"
          onclick={() => { void switchRepo(nf.repo); goHome(); }}
        >Back to reviews</button>
      </div>
    </div>
  {:else if screen.kind === 'list'}
    {@const prefill = new URLSearchParams(location.search).get('prefill_revset') ?? undefined}
    <ReviewList
      {repos}
      {repo}
      summaries={summaries}
      loading={loading}
      createdBy={whoami?.author ?? ''}
      prefillRevset={prefill}
      onchangerepo={switchRepo}
      onopen={openReview}
    />
  {:else if screen.kind === 'browse'}
    {#key `${popstateGen}|${screen.repo}|${screen.initialCommit ?? ''}|${screen.initialChange ?? ''}|${screen.initialPath ?? ''}|${screen.initialRevset ?? ''}`}
      <BrowseViewer
        repo={screen.repo}
        initialCommit={screen.initialCommit ?? null}
        initialChange={screen.initialChange ?? null}
        initialPath={screen.initialPath ?? null}
        initialRevset={screen.initialRevset ?? null}
        onstate={onBrowseStateChange}
      />
    {/key}
  {:else}
    <!-- Key on the URL-state fields so popstate (which rebuilds
         `screen` via `showReview` with new initial* values) actually
         remounts the viewer. ReviewViewer's `current`, `selectedPatchset`,
         `compareWith`, etc. are seeded once at mount; without the
         remount, a back-button navigation would update the URL but
         leave the viewer showing the previous view's data.
         In-app navigation (dropdowns, pair clicks) doesn't change the
         initial* fields — those are only re-assigned by `showReview` —
         so no spurious remounts during normal use.

         `popstateGen` is the extra piece: in-app navigation that
         pushed a new URL (e.g. an "Added in PSx" chip click) leaves
         the initial* fields *unchanged* even though ReviewViewer's
         internal state moved. A subsequent browser back then
         restored the original URL — which parses to the same
         initial* values — and the formula above would not change,
         leaving ReviewViewer stuck on the in-app-navigated state.
         Bumping `popstateGen` on every popstate forces the key to
         change, so external URL rewinds always produce a fresh
         mount even when the parsed state happens to match. -->
    {#key `${popstateGen}|${screen.repo}|${screen.view.manifest.number}|${screen.initialPatchset ?? ''}|${screen.initialCompareWith ?? ''}|${screen.initialCommit ?? ''}|${screen.initialScope ?? ''}|${screen.debug}`}
      <ReviewViewer
        repo={screen.repo}
        view={screen.view}
        viewer={whoami?.author ?? ''}
        initialPatchset={screen.initialPatchset}
        initialCompareWith={screen.initialCompareWith}
        initialCommit={screen.initialCommit}
        initialScope={screen.initialScope}
        debug={screen.debug}
        onviewchange={onViewChange}
        ontoolbarchange={(t) => (toolbar = t)}
      />
    {/key}
  {/if}
</main>

{#if showDemo}
  <DemoOverlay />
{/if}

