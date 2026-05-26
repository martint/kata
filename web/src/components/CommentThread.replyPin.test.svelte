<script lang="ts">
  //! Tiny test host: provides the `kata-files-with-reply` context
  //! the production CommentThread reads from ReviewViewer, and
  //! forwards everything else through. Lets the test inspect the
  //! set's contents across reply-lifecycle clicks.
  import { setContext } from 'svelte';
  import { SvelteSet } from 'svelte/reactivity';
  import CommentThread from './CommentThread.svelte';
  import type { CommentView, ResponseView } from '../lib/types';

  interface Props {
    comments: CommentView[];
    filePath?: string | null;
    onreply?: (input: import('../lib/types').DraftResponseInput) => Promise<void>;
    /** Bound by the test so it can inspect the shared set. */
    filesWithReplyInProgress: SvelteSet<string>;
  }
  const {
    comments,
    filePath = null,
    onreply = async () => {},
    filesWithReplyInProgress,
  }: Props = $props();

  // Capture once at mount — the test holds the same reference and
  // inspects its mutations after each interaction.
  // svelte-ignore state_referenced_locally
  setContext<SvelteSet<string>>(
    'kata-files-with-reply',
    filesWithReplyInProgress,
  );

  const noop = () => Promise.resolve();
  const noopSync = () => {};
</script>

<CommentThread
  {comments}
  responses={[] as ResponseView[]}
  saving={false}
  {filePath}
  {onreply}
  onstatus={noop}
  ondelete={noop}
  onedit={noopSync}
/>
