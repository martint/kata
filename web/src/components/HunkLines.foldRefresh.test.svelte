<script lang="ts">
  //! Tiny test host: provides the `kata-fold-store`,
  //! `kata-fold-version`, and `kata-acknowledged-unread` contexts
  //! the production HunkLines reads from `ReviewViewer`. Lets a
  //! caller hand in a pre-seeded foldStore + responses + a
  //! `lastVisitAt` baseline so we can reproduce the post-SSE-refresh
  //! "click does nothing" scenario.
  import { setContext } from 'svelte';
  import { SvelteMap, SvelteSet } from 'svelte/reactivity';
  import { createFoldStore, type FoldStore } from '../lib/foldStore';
  import HunkLines from './HunkLines.svelte';
  import type {
    CommentView,
    RegularHunk,
    ResponseView,
  } from '../lib/types';

  interface Props {
    hunk: RegularHunk;
    comments: CommentView[];
    responses?: ResponseView[];
    lastVisitAt?: string | null;
    viewer?: string;
    /** Pre-seed the foldStore so tests can simulate a thread the
     *  user previously folded. Each entry calls `set('comment', id,
     *  value)` on the store before the component mounts. */
    seedFolds?: Record<string, boolean>;
  }
  const {
    hunk,
    comments,
    responses = [],
    lastVisitAt = null,
    viewer = '',
    seedFolds = {},
  }: Props = $props();

  const foldStore: FoldStore = createFoldStore('repo', 1);
  // Seed once at mount; the closure reading the initial value is
  // exactly what we want — this is test-only setup, not reactive
  // mid-life updates.
  // svelte-ignore state_referenced_locally
  for (const [id, value] of Object.entries(seedFolds)) {
    foldStore.set('comment', id, value);
  }
  setContext<FoldStore>('kata-fold-store', foldStore);
  let foldVersion = $state(0);
  setContext<{ read: () => number; bump: () => void }>(
    'kata-fold-version',
    {
      read: () => foldVersion,
      bump: () => {
        foldVersion++;
      },
    },
  );
  const acknowledgedUnread = new SvelteSet<string>();
  setContext<SvelteSet<string>>(
    'kata-acknowledged-unread',
    acknowledgedUnread,
  );

  const noop = () => Promise.resolve();
  const noopSync = () => {};
  const highlights = {
    base: new SvelteMap<number, string>(),
    tip: new SvelteMap<number, string>(),
  };
</script>

<HunkLines
  {hunk}
  filePath="a.txt"
  {comments}
  {responses}
  {lastVisitAt}
  {viewer}
  currentPatchset={1}
  composing={null}
  saving={false}
  {highlights}
  showComments={true}
  commentsWriteable={true}
  onstartcompose={noopSync}
  onreply={noop}
  onstatus={noop}
  ondelete={noop}
  onedit={noopSync}
  onselectpatchset={noopSync}
/>
