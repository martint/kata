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
      };

  // Synchronously decide the initial screen based on the URL, BEFORE the
  // first render. A permalink (`/r/<repo>/<id>`) immediately enters
  // `loading`; without this the user would see the review list flash up
  // during the (async) whoami + listRepos + openReview round-trip.
  function initialScreen(): Screen {
    const m = location.pathname.match(/^\/r\/([^/]+)\/(.+)$/);
    if (m) {
      return { kind: 'loading', label: decodeURIComponent(m[2]) };
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

  function pathForReview(repo: string, id: string, patchset?: number): string {
    const base = `/r/${encodeURIComponent(repo)}/${encodeURIComponent(id)}`;
    return patchset !== undefined ? `${base}?ps=${patchset}` : base;
  }

  /** Parse `/r/<repo>/<review_id>` (with optional `?ps=N`). Returns null
   *  when the URL is the review list. */
  function parseUrl():
    | { repo: string; id: string; patchset: number | undefined }
    | null {
    const m = location.pathname.match(/^\/r\/([^/]+)\/(.+)$/);
    if (!m) return null;
    const psRaw = new URLSearchParams(location.search).get('ps');
    const psNum = psRaw === null ? Number.NaN : Number(psRaw);
    return {
      repo: decodeURIComponent(m[1]),
      id: decodeURIComponent(m[2]),
      patchset: Number.isFinite(psNum) ? psNum : undefined,
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
    id: string,
    patchset: number | undefined,
  ) {
    loading = true;
    error = null;
    try {
      const view = await api.openReview(targetRepo, id, patchset);
      screen = { kind: 'review', repo: targetRepo, view, initialPatchset: patchset };
    } catch (e) {
      error = (e as Error).message;
      screen = { kind: 'list' };
      await loadList(targetRepo);
    } finally {
      loading = false;
    }
  }

  /** Navigate to a review (called by user click — pushes history). */
  async function openReview(id: string) {
    const path = pathForReview(repo, id);
    if (location.pathname + location.search !== path) {
      history.pushState({}, '', path);
    }
    await showReview(repo, id, undefined);
  }

  /** Called when the viewer changes patchset via the dropdown. Sync the
   *  URL so the link is shareable without pushing a new history entry. */
  function onPatchsetChange(n: number) {
    if (screen.kind !== 'review') return;
    const path = pathForReview(screen.repo, screen.view.manifest.review_id, n);
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
      await showReview(parsed.repo, parsed.id, parsed.patchset);
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

<header class="app">
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
        >‹</button>
        <button
          onclick={commits.next}
          title="Next commit"
          aria-label="Next commit"
        >›</button>
        <span class="position">
          {commits.position === 0 ? 'All' : commits.position}/{commits.total}
        </span>
        <span class="commit-label">{commits.label}</span>
      </div>
    {/if}
  {/if}
  {#if loading}
    <span class="spinner" aria-label="loading"></span>
  {/if}
  <span style="flex: 1"></span>
  {#if toolbar}
    {#if toolbar.nav.total > 0}
      <div class="comment-nav" role="group" aria-label="Comment navigation">
        <button
          onclick={toolbar.nav.prev}
          title="Previous comment"
          aria-label="Previous comment"
        >‹</button>
        <span class="position" aria-live="polite">
          {toolbar.nav.position || '–'}/{toolbar.nav.total}
        </span>
        <button
          onclick={toolbar.nav.next}
          title="Next comment"
          aria-label="Next comment"
        >›</button>
      </div>
    {/if}
    <button
      onclick={toolbar.toggleDiffs}
      title={toolbar.diffsCollapsed ? 'Show file diffs' : 'Hide file diffs, leave only comments'}
    >
      {toolbar.diffsCollapsed ? 'Show diffs' : 'Comments only'}
    </button>
    {#if toolbar.drafts}
      {@const drafts = toolbar.drafts}
      <span class="draft-count">
        <strong>{drafts.count}</strong> <span class="lbl">draft{drafts.count === 1 ? '' : 's'}</span>
      </span>
      <button onclick={drafts.discard} disabled={drafts.saving}>Discard</button>
      <button class="primary" onclick={drafts.publish} disabled={drafts.saving}>
        {drafts.saving ? 'Publishing…' : 'Publish'}
      </button>
    {/if}
  {/if}
  {#if whoami}
    <span class="author">signed in as {whoami.author}</span>
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
      onpatchsetchange={onPatchsetChange}
      ontoolbarchange={(t) => (toolbar = t)}
    />
  {/if}
</main>

