<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from './lib/api';
  import { subscribe as subscribeEvents } from './lib/events';
  import type {
    RepoSummary,
    ReviewSummary,
    ReviewView,
    WhoAmI,
  } from './lib/types';
  import Chevron from './components/Chevron.svelte';
  import ReviewList from './components/ReviewList.svelte';
  import ReviewViewer, { type ReviewToolbarState } from './components/ReviewViewer.svelte';


  type Screen =
    | { kind: 'loading'; label: string }
    | { kind: 'list' }
    | {
        kind: 'review';
        repo: string;
        view: ReviewView;
        initialPatchset: number | undefined;
        initialCompareWith: number | undefined;
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
    return { kind: 'list' };
  }

  let screen: Screen = $state(initialScreen());
  let repos: RepoSummary[] = $state([]);
  let repo: string = $state('');
  let summaries: ReviewSummary[] | null = $state(null);
  let whoami: WhoAmI | null = $state(null);
  let error: string | null = $state(null);
  let loading: boolean = $state(false);
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
  $effect(() => {
    if (!headerEl) return;
    const update = () => {
      document.documentElement.style.setProperty(
        '--app-header-h',
        `${headerEl!.offsetHeight}px`,
      );
    };
    update();
    const ro = new ResizeObserver(update);
    ro.observe(headerEl);
    return () => ro.disconnect();
  });

  function pathForReview(
    repo: string,
    number: number,
    patchset?: number,
    compareWith?: number | null,
  ): string {
    const base = `/r/${encodeURIComponent(repo)}/${number}`;
    const parts: string[] = [];
    if (patchset !== undefined) parts.push(`ps=${patchset}`);
    if (compareWith != null) parts.push(`cmp=${compareWith}`);
    return parts.length > 0 ? `${base}?${parts.join('&')}` : base;
  }

  /** Parse `/r/<repo>/<number>` (with optional `?ps=N`, `?cmp=M`).
   *  Returns null when the URL is the review list (or when `<number>`
   *  isn't a positive integer — those URLs are treated as not-a-review). */
  function parseUrl():
    | {
        repo: string;
        number: number;
        patchset: number | undefined;
        compareWith: number | undefined;
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
      };
    } catch (e) {
      error = (e as Error).message;
      screen = { kind: 'list' };
      await loadList(targetRepo);
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
    await showReview(repo, number, undefined, undefined);
  }

  /** Called when the viewer changes patchset or compare target via the
   *  dropdowns. Sync the URL so the link is shareable without pushing
   *  a new history entry. */
  function onViewChange(n: number, compare: number | null) {
    if (screen.kind !== 'review') return;
    const path = pathForReview(
      screen.repo,
      screen.view.manifest.number,
      n,
      compare,
    );
    if (location.pathname + location.search !== path) {
      history.replaceState({}, '', path);
    }
  }

  /** App-level back button: just defer to the browser. */
  function back() {
    history.back();
  }

  async function switchRepo(name: string) {
    if (name === repo) return;
    repo = name;
    await loadList(name);
  }

  /** Reflect the current URL into `screen`. Runs on mount and on popstate. */
  async function syncFromUrl() {
    const parsed = parseUrl();
    if (parsed) {
      // Make sure the named repo is known; fall back to list if not.
      if (!repos.some((r) => r.name === parsed.repo)) {
        screen = { kind: 'list' };
        if (repos[0]) await switchRepo(repos[0].name);
        return;
      }
      repo = parsed.repo;
      await showReview(parsed.repo, parsed.number, parsed.patchset, parsed.compareWith);
    } else {
      screen = { kind: 'list' };
      if (!repo && repos[0]) repo = repos[0].name;
      await loadList(repo);
    }
  }

  onMount(() => {
    const unsubscribe = subscribeEvents((event) => {
      if (
        screen.kind === 'list' &&
        event.repo === repo &&
        (event.kind === 'review-created' || event.kind === 'review-updated')
      ) {
        void loadList(repo);
      }
    });
    window.addEventListener('popstate', () => {
      void syncFromUrl();
    });
    (async () => {
      try {
        whoami = await api.whoami();
        repos = await api.listRepos();
      } catch (e) {
        error = (e as Error).message;
      }
      await syncFromUrl();
    })();
    return unsubscribe;
  });
</script>

<header class="app" bind:this={headerEl}>
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
      <img class="app-icon" src="/favicon.svg" alt="" width="22" height="22" />
      Kata
    </h1>
    {#if screen.kind === 'review'}
      <button onclick={back} aria-label="Back to review list">← <span class="lbl">Back</span></button>
      <button
        onclick={() => window.scrollTo({ top: 0, behavior: 'smooth' })}
        title="Scroll to the top of the review"
        aria-label="Scroll to top"
      >↑ <span class="lbl">Top</span></button>
      {#if toolbar?.commits}
        {@const commits = toolbar.commits}
        <div
          class="commit-nav"
          role="group"
          aria-label="Commit navigation"
          title={commits.label}
        >
          <button
            onclick={commits.prev}
            title="Previous commit"
            aria-label="Previous commit"
          ><Chevron dir="left" /></button>
          <span class="position">
            {commits.position === 0 ? 'All' : commits.position}/{commits.total}
          </span>
          <button
            onclick={commits.next}
            title="Next commit"
            aria-label="Next commit"
          ><Chevron dir="right" /></button>
          <span class="commit-label">{commits.label}</span>
        </div>
      {/if}
    {/if}
    {#if loading}
      <span class="spinner" aria-label="loading"></span>
    {/if}
    <span style="flex: 1"></span>
    {#if toolbar}
      {#if toolbar.drafts}
        {@const drafts = toolbar.drafts}
        <div class="draft-nav" role="group" aria-label="Draft navigation">
          <button
            type="button"
            onclick={drafts.prev}
            title="Previous draft"
            aria-label="Previous draft"
          ><Chevron dir="left" /></button>
          <span class="draft-count" aria-live="polite">
            {drafts.position || '–'}/<strong>{drafts.count}</strong>
            <span class="lbl">draft{drafts.count === 1 ? '' : 's'}</span>
          </span>
          <button
            type="button"
            onclick={drafts.next}
            title="Next draft"
            aria-label="Next draft"
          ><Chevron dir="right" /></button>
        </div>
        <button onclick={drafts.discard} disabled={drafts.saving}>Discard</button>
        <button class="primary" onclick={drafts.publish} disabled={drafts.saving}>
          {drafts.saving ? 'Publishing…' : 'Publish'}
        </button>
      {/if}
    {/if}
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
      </span>
      {#if toolbar.patchsets}
        {@const ps = toolbar.patchsets}
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
      {/if}
      <!-- Float controls to the right so the title gets breathing
           room from the chips next to it. -->
      <span style="flex: 1"></span>
      <!-- Order is `nav | hint | chips | diffs-toggle` rather than the
           visual-reading-order opposite so the chip cluster stays
           anchored against the diffs-toggle at the right edge: when
           the nav or hint disappears (no comments, filter not empty),
           only the elements between the title spacer and the chips
           shift — the chips themselves keep their position. -->
      {#if toolbar.comments}
        {@const c = toolbar.comments}
        <div class="comment-nav" role="group" aria-label="Comment navigation">
          <button
            type="button"
            onclick={c.prev}
            title="Previous comment"
            aria-label="Previous comment"
          ><Chevron dir="left" /></button>
          <span class="position" aria-live="polite">
            {c.position || '–'}/{c.total}
          </span>
          <button
            type="button"
            onclick={c.next}
            title="Next comment"
            aria-label="Next comment"
          ><Chevron dir="right" /></button>
        </div>
      {/if}
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
      {#if toolbar.filter}
        {@const filter = toolbar.filter}
        <div class="filter-chips">
          <span class="label">Status</span>
          <button
            type="button"
            class="chip status-draft"
            class:on={filter.status.draft}
            aria-pressed={filter.status.draft}
            onclick={() => filter.toggleStatus('draft')}
          >Draft</button>
          <button
            type="button"
            class="chip status-open"
            class:on={filter.status.open}
            aria-pressed={filter.status.open}
            onclick={() => filter.toggleStatus('open')}
          >Open</button>
          <button
            type="button"
            class="chip status-resolved"
            class:on={filter.status.resolved}
            aria-pressed={filter.status.resolved}
            onclick={() => filter.toggleStatus('resolved')}
          >Resolved</button>
          <span class="sep" aria-hidden="true"></span>
          <span class="label">Severity</span>
          <button
            type="button"
            class="chip flag-must-do"
            class:on={filter.flag['must-do']}
            aria-pressed={filter.flag['must-do']}
            onclick={() => filter.toggleFlag('must-do')}
          >Must do</button>
          <button
            type="button"
            class="chip flag-suggestion"
            class:on={filter.flag.suggestion}
            aria-pressed={filter.flag.suggestion}
            onclick={() => filter.toggleFlag('suggestion')}
          >Suggestion</button>
          <button
            type="button"
            class="chip flag-other"
            class:on={filter.flag.other}
            aria-pressed={filter.flag.other}
            onclick={() => filter.toggleFlag('other')}
          >Other</button>
        </div>
      {/if}
      {#if toolbar.diffs}
        {@const d = toolbar.diffs}
        <button
          type="button"
          class="diffs-toggle"
          onclick={d.toggle}
          title={d.collapsed ? 'Show file diffs' : 'Hide file diffs, leave only comments'}
        >
          {d.collapsed ? 'Show diffs' : 'Comments only'}
        </button>
      {/if}
    </div>
  {/if}
</header>

<main class={screen.kind === 'review' ? 'wide' : ''}>
  {#if error}
    <p class="error">{error}</p>
  {/if}

  {#if screen.kind === 'loading'}
    <p class="loading-permalink">
      <span class="spinner" aria-hidden="true"></span>
      Loading review <code>{screen.label}</code>…
    </p>
  {:else if screen.kind === 'list'}
    <ReviewList
      {repos}
      {repo}
      summaries={summaries}
      loading={loading}
      createdBy={whoami?.author ?? ''}
      onchangerepo={switchRepo}
      onopen={openReview}
    />
  {:else}
    <ReviewViewer
      repo={screen.repo}
      view={screen.view}
      viewer={whoami?.author ?? ''}
      initialPatchset={screen.initialPatchset}
      initialCompareWith={screen.initialCompareWith}
      onviewchange={onViewChange}
      ontoolbarchange={(t) => (toolbar = t)}
    />
  {/if}
</main>

