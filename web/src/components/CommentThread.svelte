<script lang="ts">
  import { getContext } from 'svelte';
  import { copyText } from '../lib/clipboard';
  import { renderMarkdown } from '../lib/markdown';
  import { hasUnreadReplies as hasUnreadRepliesShared, isThreadFolded, resolutionFor } from '../lib/resolution';
  import { preserveScrollAnchor } from '../lib/scrollAnchor';
  import type { FoldStore } from '../lib/foldStore';
  import type { SearchMatch } from '../lib/search';

  /** See HunkLines.svelte — reactive accessors for the in-app
   *  search state. Optional because tests render CommentThread
   *  standalone without the ReviewViewer wrapper. */
  interface SearchContext {
    matches: () => readonly SearchMatch[];
    currentMatch: () => SearchMatch | null;
  }
  const searchCtx = getContext<SearchContext | undefined>('kata-search');

  /** Does any match's `comment_id` equal this comment? Used to
   *  tint the whole `<li>` so the reader's eye lands on the
   *  comments that contain the query. */
  function searchHasComment(commentId: string): boolean {
    if (!searchCtx) return false;
    for (const m of searchCtx.matches()) {
      if (m.kind === 'comment' && m.comment_id === commentId) return true;
    }
    return false;
  }

  function searchIsCurrentComment(commentId: string): boolean {
    if (!searchCtx) return false;
    const cur = searchCtx.currentMatch();
    return cur != null && cur.kind === 'comment' && cur.comment_id === commentId;
  }
  import Chevron from './Chevron.svelte';
  import type {
    AnchorView,
    CommentView,
    DraftResponseInput,
    ResolutionAction,
    ResponseView,
  } from '../lib/types';
  import ResponseComposer from './ResponseComposer.svelte';

  interface Props {
    comments: CommentView[];
    responses: ResponseView[];
    saving: boolean;
    /** The patchset the page is currently displaying. Used to decide
     *  whether the per-comment "added in PS N" badge is the current
     *  view (rendered as a plain badge) or a different round
     *  (rendered as a clickable jump). Optional — call sites that
     *  don't know the active patchset omit it. */
    currentPatchset?: number;
    onreply: (input: DraftResponseInput) => Promise<void>;
    onstatus: (commentId: string, action: ResolutionAction) => Promise<void>;
    ondelete: (comment: CommentView) => Promise<void>;
    /** Discard a draft response (reply or resolution-marker) before
     *  the session is published. Optional — call sites that don't
     *  want the affordance (none currently) can omit it. */
    ondeleteresponse?: (response: ResponseView) => Promise<void>;
    onedit: (comment: CommentView) => void;
    /** Switch the viewer to patchset `n`, optionally landing on
     *  comment `commentId` after the switch completes. Threaded down
     *  so a clicked "added in PS N" badge can jump to the patchset
     *  the comment was originally written against AND scroll to the
     *  comment itself in that view. Optional. */
    onselectpatchset?: (n: number, commentId?: string) => void;
    /** When the user clicks Edit on a draft, the parent opens a
     *  composer pre-filled with that draft's body — and passes the
     *  comment's id here so we hide it from the thread. Without this
     *  the original draft would still render above the composer,
     *  which reads like two separate things when in fact one is being
     *  rewritten into the other. */
    editingCommentId?: string | null;
    /** Wall-clock timestamp the viewer last opened this review at, or
     *  `null` on their first ever open. Threads with at least one
     *  response newer than this (and not authored by the viewer) get
     *  flagged as having unread replies. */
    lastVisitAt?: string | null;
    /** Currently signed-in author. A response by this author against
     *  their own comment doesn't count as "unread to themselves." */
    viewer?: string;
    /** Default fold state to use when the user hasn't picked one for
     *  a thread. Mirrors the view-mode default — `true` in Compact,
     *  `false` in Full. Resolution layers on top (a resolved thread
     *  folds even in Full); see `defaultFoldedForThread`. */
    defaultThreadsCollapsed?: boolean;
    /** When `false` (the default), the per-thread fold chevron in
     *  each comment header is hidden — the assumption is that the
     *  group has only one thread and the bulk gutter marker / section
     *  toggle already covers it; a second control would duplicate the
     *  affordance. Parents pass `true` when the group has 2+ items
     *  so the chevron earns its keep by letting the user hide just
     *  one thread within the group. */
    showFold?: boolean;
    /** Path of the file these comments belong to, when applicable.
     *  Threaded down so opening a reply composer can ask ReviewViewer
     *  to keep the owning FileSlot mounted while the user is typing —
     *  otherwise scrolling away would virtualise the slot, destroy
     *  the in-progress reply, and lose whatever the user had typed.
     *  Omitted by call sites where this isn't relevant (commit-level
     *  threads in CommitsPanel, which aren't FileSlot-virtualised). */
    filePath?: string | null;
  }
  const {
    comments,
    responses,
    saving,
    currentPatchset,
    onreply,
    onstatus,
    ondelete,
    ondeleteresponse,
    onedit,
    onselectpatchset,
    editingCommentId = null,
    lastVisitAt = null,
    viewer = '',
    defaultThreadsCollapsed = false,
    showFold = false,
    filePath = null,
  }: Props = $props();

  // Coordination with FileSlot virtualisation — see ReviewViewer.
  // The reply lifecycle (start / cancel / submit) below updates this
  // set so the owning slot stays mounted while a draft reply is
  // still open. Optional context: standalone test renders may not
  // provide it.
  const filesWithReplyInProgress = getContext<Set<string> | undefined>(
    'kata-files-with-reply',
  );

  const visibleComments = $derived(
    editingCommentId
      ? comments.filter((c) => c.comment_id !== editingCommentId)
      : comments,
  );

  let replyingTo: string | null = $state(null);

  function anchorLabel(a: AnchorView): string | null {
    switch (a.kind) {
      case 'valid':
        return null;
      case 'moved':
        return `moved to ${a.new_lines.start}-${a.new_lines.end}`;
      case 'drifted':
        return `drifted (${Math.round(a.similarity * 100)}% similar)`;
      case 'outdated':
        return 'outdated';
    }
  }

  function actionLabel(a: ResolutionAction): string {
    switch (a) {
      case 'comment':
        return 'replied';
      case 'resolve':
        return 'resolved';
      case 'unresolve':
        return 'reopened';
      case 'wont-fix':
        return "marked won't fix";
    }
  }

  function responsesFor(commentId: string): ResponseView[] {
    return responses
      .filter((r) => r.in_reply_to === commentId)
      .slice()
      .sort((a, b) => a.created_at.localeCompare(b.created_at));
  }

  // Per-session acknowledgement set — see ReviewViewer. Comment ids
  // here are treated as "user has seen the unread reply" so the
  // 'new replies' badge dismisses and the resolved-collapse stops
  // being overridden, letting an explicit fold actually hide the
  // thread. The local wrapper just adapts the imported helper to
  // this component's prop closures.
  const acknowledgedUnread = getContext<Set<string> | undefined>(
    'kata-acknowledged-unread',
  );

  /** Does this comment have at least one response that landed after
   *  the viewer's last open of the review (and that the viewer didn't
   *  author themselves)? Drives the 'new replies' badge and overrides
   *  the resolved-collapse so unread threads stay expanded even after
   *  the responder marked them done. */
  function hasUnreadReplies(commentId: string): boolean {
    return hasUnreadRepliesShared(
      commentId,
      responses,
      lastVisitAt,
      viewer,
      acknowledgedUnread,
    );
  }

  async function copyToClipboard(text: string) {
    await copyText(text);
  }

  /** Build a same-origin permalink that includes the review's current
   *  pathname/search (so the patchset stays the same) and a `#c-<id>`
   *  hash that ReviewViewer scrolls to on load and on `hashchange`. */
  function permalinkFor(commentId: string): string {
    const u = new URL(window.location.href);
    u.hash = `c-${encodeURIComponent(commentId)}`;
    return u.toString();
  }

  /** Open the reply composer for `commentId`. Also pins the owning
   *  FileSlot in place (via `filesWithReplyInProgress`) so the user
   *  can scroll the page without losing the in-progress reply. */
  function startReply(commentId: string) {
    replyingTo = commentId;
    if (filePath) filesWithReplyInProgress?.add(filePath);
  }

  /** Close the reply composer without submitting. Releases the
   *  FileSlot pin if there are no other reply composers open in
   *  this same file. (There can't be — `replyingTo` is one id at a
   *  time per CommentThread instance — but the asymmetric add /
   *  delete is the right shape regardless.) */
  function cancelReply() {
    replyingTo = null;
    if (filePath) filesWithReplyInProgress?.delete(filePath);
  }

  async function submitReply(input: DraftResponseInput) {
    await onreply(input);
    replyingTo = null;
    if (filePath) filesWithReplyInProgress?.delete(filePath);
  }

  /** Per-thread fold lookup. The store remembers the user's explicit
   *  choice (true = folded, false = expanded); absent means follow
   *  the resolution-aware default — folded if the thread is
   *  resolved/won't-fix or the view mode is Compact, otherwise
   *  expanded. The shared `foldVersion` (set up by ReviewViewer)
   *  triggers re-renders across every consumer after any fold
   *  change — without it, this component's toggle wouldn't wake
   *  the gutter-marker aggregate in HunkLines / SBS. */
  const sharedFoldStore = getContext<FoldStore | undefined>('kata-fold-store');
  const foldVersionCtx = getContext<{ read: () => number; bump: () => void } | undefined>(
    'kata-fold-version',
  );
  // In production ReviewViewer always provides both context values,
  // but tests render CommentThread standalone — and a chevron click
  // that writes nowhere reads as broken. Fall back to an in-memory
  // store + a local version counter when the context is missing so
  // the chevron always functions, just without cross-component
  // sync + persistence (irrelevant in test harnesses).
  const localFallback = new Map<string, boolean>();
  const foldStore: FoldStore =
    sharedFoldStore ??
    ({
      get: (_kind, id) => localFallback.get(id),
      set: (_kind, id, v) => {
        localFallback.set(id, v);
      },
      ids: () => Array.from(localFallback.keys()),
      prune: () => {},
    } as FoldStore);
  let localFoldVersion = $state(0);
  function isFolded(commentId: string): boolean {
    void localFoldVersion;
    foldVersionCtx?.read();
    return isThreadFolded(commentId, responses, foldStore, defaultThreadsCollapsed);
  }
  function toggleFold(commentId: string) {
    // Wrap the fold flip in `preserveScrollAnchor` so a chevron
    // click on a thread above (or below) the viewport doesn't
    // shift what the user is currently reading. The helper
    // captures the topmost visible element's screen-Y, flushes
    // the state change through Svelte's tick, then re-aligns
    // scroll so that element lands at the same Y again. See
    // `lib/scrollAnchor.ts`.
    void preserveScrollAnchor(() => {
      const next = !isFolded(commentId);
      foldStore.set('comment', commentId, next);
      // Explicit fold action acknowledges the unread-replies
      // force-expand for this thread so the chevron click
      // actually hides it. See ReviewViewer's
      // `acknowledgedUnread`.
      acknowledgedUnread?.add(commentId);
      localFoldVersion++;
      foldVersionCtx?.bump();
    });
  }
</script>

<ul class="thread">
  {#each visibleComments as c (c.comment_id)}
    {@const label = anchorLabel(c.anchor)}
    {@const state = resolutionFor(c.comment_id, responses)}
    {@const replies = responsesFor(c.comment_id)}
    {@const unread = hasUnreadReplies(c.comment_id)}
    <!-- Per-thread fold lives in `foldStore`. Two presentations:
         in gutter contexts (HunkLines / SBS) the parent pre-filters
         folded threads out, so this component only ever sees
         expanded ones and `collapsed` stays false. In orphan /
         file-level / comments-only contexts the parent doesn't
         filter, so folded threads render header-only — the
         in-header chevron is the way back. Unread replies always
         force-expand so a fresh response can't hide behind a fold
         the resolver set. -->
    {@const collapsed = isFolded(c.comment_id) && !unread}
    {@const hasSearchMatch = searchHasComment(c.comment_id)}
    {@const isCurrentSearchMatch = searchIsCurrentComment(c.comment_id)}
    <li
      class="comment {c.draft ? 'draft' : ''} {c.anchor.kind === 'outdated'
        ? 'outdated'
        : ''} {collapsed ? 'collapsed' : ''} {unread ? 'unread' : ''} {hasSearchMatch
        ? 'has-search-match'
        : ''} {isCurrentSearchMatch ? 'is-current-search-match' : ''}"
      data-comment-id={c.comment_id}
    >
      <header>
        <!-- Per-thread fold toggle, only shown when the group has
             2+ items so a single-thread group doesn't display a
             redundant second affordance next to its gutter marker. -->
        {#if showFold}
          <button
            type="button"
            class="fold-toggle"
            aria-expanded={!collapsed}
            title={collapsed ? 'Expand this thread' : 'Fold this thread'}
            onclick={() => toggleFold(c.comment_id)}
            data-tour="thread-fold"
          ><Chevron dir={collapsed ? 'right' : 'down'} size={10} filled /></button>
        {/if}
        <strong>{c.author}</strong>
        <!-- Flag chip suppressed when it equals the default
             (`must-do`): most comments are must-do, so showing it
             on every row is noise. Suggestion / question still
             render their chip. -->
        {#if c.flag !== 'must-do'}
          <span class="flag flag-{c.flag}">{c.flag}</span>
        {/if}
        <!-- No explicit `draft` chip: the `.comment.draft` row tag
             already styles the whole row with the attention border
             + background, which reads as "draft" at a glance. -->
        {#if label}<span class="badge anchor-{c.anchor.kind}">{label}</span>{/if}
        <!-- Resolution chip suppressed while the row is collapsed
             (the collapse itself IS the visual signal that the
             thread is resolved/wont-fix). Expanded resolved rows
             keep the chip so the reader can see what state they
             clicked into. -->
        {#if state !== 'open' && !collapsed}
          <span class="badge resolution-{state}">{state}</span>
        {/if}
        {#if unread}
          <span class="badge new-replies" title="New replies since your last visit">new replies</span>
        {/if}
        <!-- "Added in PS N" jump-button: appears only when the
             comment came from a different patchset than the one
             currently displayed, so the common case (comment on the
             active patchset) stays uncluttered. Clicking switches the
             viewer to that patchset so the user can read the comment
             against the diff it was originally written against. -->
        {#if currentPatchset !== undefined && c.patchset !== currentPatchset && onselectpatchset}
          <button
            type="button"
            class="badge ps-jump"
            title="Jump to this comment in PS{c.patchset}"
            onclick={() => onselectpatchset(c.patchset, c.comment_id)}
          >added in PS{c.patchset}</button>
        {/if}
        <span class="time">{new Date(c.created_at).toLocaleString()}</span>
        <button
          type="button"
          class="copy-button"
          title="Copy permalink"
          onclick={() => copyToClipboard(permalinkFor(c.comment_id))}>🔗</button
        >
        {#if c.body.trim().length > 0}
          <button
            type="button"
            class="copy-button"
            title="Copy markdown source"
            onclick={() => copyToClipboard(c.body)}>⧉</button
          >
        {/if}
      </header>
      {#if !collapsed}
      <div class="body markdown">
        {#if c.body.trim().length > 0}
          {@html renderMarkdown(c.body)}
        {:else}
          <em class="muted">(no message)</em>
        {/if}
      </div>
      <!-- For outdated comments, surface the lines the reviewer
           was originally commenting on. The "added in PS N" jump-
           button in the header is still the way to read the
           comment in its full original context — but that
           navigates away from the current view, and reviewers
           often want a quick peek of "what was this about?"
           without losing their place. Default-folded so the
           excerpt doesn't dominate the comment for users who
           don't need it. -->
      {#if c.anchor.kind === 'outdated' && c.anchor.original_content.trim().length > 0}
        <details class="outdated-excerpt">
          <summary>Originally commented on</summary>
          <pre>{c.anchor.original_content.replace(/\n$/, '')}</pre>
        </details>
      {/if}
      {#if replies.length > 0}
        <ul class="replies">
          {#each replies as r (r.response_id)}
            <li class="reply {r.draft ? 'draft' : ''}">
              <header>
                <strong>{r.author}</strong>
                <span class="action">{actionLabel(r.action)}</span>
                <!-- No explicit `draft` chip: the `.reply.draft`
                     row tag now carries the same attention styling
                     the comment row does, which reads as "draft"
                     without a separate badge. -->
                <span class="time">{new Date(r.created_at).toLocaleString()}</span>
                {#if r.body.trim().length > 0}
                  <button
                    type="button"
                    class="copy-button"
                    title="Copy markdown source"
                    onclick={() => copyToClipboard(r.body)}>⧉</button
                  >
                {/if}
                <!-- Delete affordance for drafts (own session). Covers
                     both body-bearing replies and the empty-body
                     resolution-markers (`resolve` / `wont-fix` /
                     `unresolve`) — a misclicked status flip is at
                     least as common as a typo'd reply, so the same
                     undo path serves both. Published responses can't
                     be deleted, hence the `r.draft` gate. -->
                {#if r.draft && ondeleteresponse}
                  <button
                    type="button"
                    class="action-button destructive reply-delete"
                    disabled={saving}
                    onclick={() => ondeleteresponse?.(r)}
                  >
                    Delete
                  </button>
                {/if}
              </header>
              {#if r.body.trim().length > 0}
                <div class="markdown">{@html renderMarkdown(r.body)}</div>
              {/if}
            </li>
          {/each}
        </ul>
      {/if}
      <footer class="actions">
        {#if replyingTo === c.comment_id}
          <ResponseComposer
            commentId={c.comment_id}
            {saving}
            oncancel={cancelReply}
            onsubmit={submitReply}
          />
        {:else}
          <button
            type="button"
            class="action-button"
            onclick={() => startReply(c.comment_id)}
          >
            Reply
          </button>
          {#if !c.draft}
            {#if state === 'open'}
              <button
                type="button"
                class="action-button"
                disabled={saving}
                onclick={() => onstatus(c.comment_id, 'resolve')}
              >
                Resolve
              </button>
              <button
                type="button"
                class="action-button"
                disabled={saving}
                onclick={() => onstatus(c.comment_id, 'wont-fix')}
              >
                Won't fix
              </button>
            {:else}
              <button
                type="button"
                class="action-button"
                disabled={saving}
                onclick={() => onstatus(c.comment_id, 'unresolve')}
              >
                Reopen
              </button>
            {/if}
          {/if}
          {#if c.draft}
            <button
              type="button"
              class="action-button"
              disabled={saving}
              onclick={() => onedit(c)}
            >
              Edit
            </button>
            <button
              type="button"
              class="action-button destructive"
              disabled={saving}
              onclick={() => ondelete(c)}
            >
              Delete
            </button>
          {/if}
        {/if}
      </footer>
      {/if}
    </li>
  {/each}
</ul>

<style>
  .thread {
    list-style: none;
    margin: 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .comment {
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px 12px;
    font-family: ui-sans-serif, system-ui, sans-serif;
    font-size: 13px;
  }

  .comment.draft {
    border-color: var(--attention-border);
    background: var(--attention-bg);
  }

  .comment.outdated {
    opacity: 0.85;
    border-style: dashed;
  }

  /* Search-match tint on the whole comment. Comment bodies are
   * rendered markdown (entity-escaped, possibly with inline tags)
   * so the per-character `<mark>` injection that works on diff
   * lines doesn't translate; tinting the wrapper conveys "this
   * comment matches" while leaving the body itself readable. The
   * active-match variant gets a sharper outline so prev/next can
   * land somewhere visible. */
  .comment.has-search-match {
    background: rgba(255, 220, 0, 0.18);
  }
  .comment.is-current-search-match {
    box-shadow: inset 3px 0 0 rgba(255, 150, 0, 0.9);
    background: rgba(255, 220, 0, 0.28);
  }

  /* Default-folded peek at the lines the comment was originally
   * commenting on. Visually muted so it stays subordinate to the
   * comment body — the reader sees it as "extra context, click to
   * peek" rather than another thing to read. The `<pre>` keeps the
   * indentation the reviewer was pointing at; horizontal scroll
   * is allowed so a long line doesn't push the comment box wider
   * than its container. */
  .outdated-excerpt {
    margin: 8px 0 0;
    border: 1px dashed var(--border);
    border-radius: 4px;
    background: var(--bg-panel);
    font-size: 12px;
  }
  .outdated-excerpt summary {
    padding: 4px 8px;
    color: var(--text-muted);
    cursor: pointer;
    user-select: none;
  }
  .outdated-excerpt summary:hover {
    color: var(--text);
  }
  .outdated-excerpt pre {
    margin: 0;
    padding: 6px 10px;
    border-top: 1px dashed var(--border);
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 12px;
    white-space: pre;
    overflow-x: auto;
    color: var(--text);
  }

  /* Outline a thread with new replies so the reader's eye lands on
   * it ahead of the surrounding done-and-folded threads. The left
   * accent is wider than the regular border so it still reads at a
   * glance after the user scrolls past it. */
  .comment.unread {
    border-color: var(--link);
    box-shadow: inset 3px 0 0 var(--link);
  }

  /* Resolved / won't-fix threads collapse to just their header to
   * stop "done" comments from filling the page. The fold-toggle
   * chevron at the start of the header expands them on demand.
   *
   * Padding deliberately matches the expanded state — overriding it
   * smaller while collapsed used to jolt the header down a few
   * pixels on expand, which felt buggy. The header just sits a
   * little lower in the box when collapsed (no body or footer
   * below it). */
  .comment.collapsed {
    opacity: 0.7;
  }

  /* Per-thread fold chevron — same filled-triangle Chevron the
   * gutter marker uses, so an orphan-section reader who can't see
   * a hunk gutter still recognises the affordance. The button is
   * sized to fit the 10px Chevron with breathing room and pins
   * itself slightly higher than the surrounding header text so the
   * triangle's tip sits on the author's baseline. */
  .fold-toggle {
    background: transparent;
    border: none;
    cursor: pointer;
    color: var(--link);
    padding: 0 2px;
    margin-right: 2px;
    display: inline-flex;
    align-items: center;
    justify-content: center;
    align-self: center;
  }

  .fold-toggle:hover {
    color: var(--link-hover, var(--link));
    filter: brightness(1.2);
  }

  .comment header,
  .reply header {
    display: flex;
    align-items: baseline;
    gap: 8px;
    margin-bottom: 6px;
  }

  .comment .time,
  .reply .time {
    color: var(--text-faint);
    font-size: 11px;
    margin-left: auto;
  }

  .markdown :global(p:first-child) {
    margin-top: 0;
  }

  .markdown :global(p:last-child) {
    margin-bottom: 0;
  }

  .markdown :global(p) {
    margin: 6px 0;
    line-height: 1.5;
  }

  .markdown :global(pre) {
    background: var(--bg-panel);
    padding: 8px;
    border-radius: 4px;
    overflow-x: auto;
    margin: 6px 0;
  }

  .markdown :global(code) {
    background: var(--bg-elevated);
    padding: 1px 4px;
    border-radius: 3px;
    font-family: ui-monospace, monospace;
    font-size: 12px;
  }

  .markdown :global(pre code) {
    padding: 0;
    background: transparent;
  }

  .markdown :global(ul),
  .markdown :global(ol) {
    margin: 6px 0;
    padding-left: 24px;
  }

  .markdown :global(blockquote) {
    margin: 6px 0;
    padding-left: 12px;
    border-left: 3px solid var(--border);
    color: var(--text-muted);
  }

  .copy-button {
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 3px;
    padding: 0 4px;
    font-size: 11px;
    color: var(--text-muted);
    cursor: pointer;
    margin-left: 4px;
  }

  .copy-button:hover {
    background: var(--bg-panel);
    color: var(--link);
  }

  .badge {
    font-size: 11px;
    padding: 1px 6px;
    border-radius: 9999px;
    background: var(--bg-elevated);
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .badge.draft {
    background: var(--attention-bg);
    color: var(--warn-text);
  }

  .badge.anchor-moved,
  .badge.anchor-drifted {
    background: var(--link-bg);
    color: var(--link);
  }

  .badge.anchor-outdated {
    background: var(--error-bg);
    color: var(--error-text);
  }

  .badge.resolution-resolved {
    background: var(--success-bg);
    color: var(--success-text);
  }

  .badge.resolution-wont-fix {
    background: var(--bg-elevated);
    color: var(--text-muted);
  }

  .badge.new-replies {
    background: var(--link-bg);
    color: var(--link);
    border: 1px solid var(--link);
  }

  /* "Added in PS N" jump-button. Rendered only when the comment came
   * from a patchset other than the one currently displayed; clicking
   * switches the viewer to that round. */
  button.badge.ps-jump {
    background: var(--link-bg);
    color: var(--link);
    border: 1px solid transparent;
    font-family: ui-sans-serif, system-ui, sans-serif;
    font-weight: 500;
    cursor: pointer;
  }

  button.badge.ps-jump:hover {
    border-color: var(--link);
  }

  .replies {
    list-style: none;
    margin: 8px 0 0;
    padding: 0 0 0 12px;
    border-left: 2px solid var(--bg-elevated);
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .reply {
    font-size: 12.5px;
  }

  .reply.draft {
    /* The "draft" chip used to carry this signal — now the row
     * styling does. Background + left accent are enough to read as
     * "draft" without screaming. */
    background: var(--attention-bg);
    border-left: 3px solid var(--attention-border);
    padding-left: 8px;
    margin-left: -8px;
    border-radius: 4px;
  }

  .reply .action {
    color: var(--text-muted);
    font-style: italic;
  }

  .actions {
    margin-top: 8px;
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .action-button {
    font-size: 12px;
    padding: 2px 8px;
    background: transparent;
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--link);
    cursor: pointer;
  }

  .action-button:hover {
    background: var(--link-bg);
  }

  .action-button.destructive {
    color: var(--error-text);
  }

  .action-button.destructive:hover {
    background: var(--error-bg);
  }

  .action-button:disabled {
    opacity: 0.5;
    cursor: default;
  }
</style>
