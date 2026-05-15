<script lang="ts">
  import { copyText } from '../lib/clipboard';
  import { renderMarkdown } from '../lib/markdown';
  import { resolutionFor } from '../lib/resolution';
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
  }
  const {
    comments,
    responses,
    saving,
    currentPatchset,
    onreply,
    onstatus,
    ondelete,
    onedit,
    onselectpatchset,
    editingCommentId = null,
    lastVisitAt = null,
    viewer = '',
  }: Props = $props();

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

  /** Does this comment have at least one response that landed after
   *  the viewer's last open of the review (and that the viewer didn't
   *  author themselves)? Drives the 'new replies' badge and overrides
   *  the resolved-collapse so unread threads stay expanded even after
   *  the responder marked them done. */
  function hasUnreadReplies(commentId: string): boolean {
    if (!lastVisitAt) return false;
    return responses.some(
      (r) =>
        r.in_reply_to === commentId &&
        !r.draft &&
        r.author !== viewer &&
        r.created_at > lastVisitAt,
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

  async function submitReply(input: DraftResponseInput) {
    await onreply(input);
    replyingTo = null;
  }

  /** Comment IDs the user has explicitly unfolded. Resolved /
   *  won't-fix comments are collapsed by default — they're "done
   *  with it" threads that just clutter the view otherwise — but
   *  the user can click the header to expand them and re-read the
   *  body or replies. Tracking only the explicit overrides keeps
   *  the resolution-status as the source of truth: a comment
   *  flipping back to open via a new response collapses naturally. */
  let expanded: Set<string> = $state(new Set());
  function toggleExpanded(id: string) {
    const next = new Set(expanded);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    expanded = next;
  }
</script>

<ul class="thread">
  {#each visibleComments as c (c.comment_id)}
    {@const label = anchorLabel(c.anchor)}
    {@const state = resolutionFor(c.comment_id, responses)}
    {@const replies = responsesFor(c.comment_id)}
    {@const unread = hasUnreadReplies(c.comment_id)}
    <!-- A resolved thread normally collapses (it's "done"), but if it
         has replies the viewer hasn't seen yet we keep it expanded so
         the body and the new response are immediately visible —
         otherwise the agent's claim of "resolved" would hide its own
         answer behind a fold. -->
    {@const collapsed =
      state !== 'open' && !expanded.has(c.comment_id) && !unread}
    <li
      class="comment {c.draft ? 'draft' : ''} {c.anchor.kind === 'outdated'
        ? 'outdated'
        : ''} {collapsed ? 'collapsed' : ''} {unread ? 'unread' : ''}"
      data-comment-id={c.comment_id}
    >
      <header>
        <!-- Header is the collapse handle for resolved / won't-fix
             comments: click anywhere on it (other than the existing
             buttons) to toggle the body + replies + actions. -->
        {#if state !== 'open'}
          <button
            type="button"
            class="fold-toggle"
            aria-expanded={!collapsed}
            title={collapsed ? 'Expand' : 'Collapse'}
            onclick={() => toggleExpanded(c.comment_id)}
          >{collapsed ? '▸' : '▾'}</button>
        {/if}
        <strong>{c.author}</strong>
        <span class="flag flag-{c.flag}">{c.flag}</span>
        {#if c.draft}<span class="badge draft">draft</span>{/if}
        {#if label}<span class="badge anchor-{c.anchor.kind}">{label}</span>{/if}
        {#if state !== 'open'}
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
      {#if c.anchor.kind === 'outdated'}
        <!-- Open by default for outdated comments: the orphan-threads
             block in FileDiff has no inline diff to anchor against, so
             surfacing the original lines is the only way the reader
             can tell what the comment was about. -->
        <details class="original" open>
          <summary>
            Original lines from PS{c.patchset}
            {#if c.lines}(lines {c.lines.start}–{c.lines.end}){/if}
          </summary>
          <pre>{c.anchor.original_content}</pre>
        </details>
      {/if}
      {#if replies.length > 0}
        <ul class="replies">
          {#each replies as r (r.response_id)}
            <li class="reply {r.draft ? 'draft' : ''}">
              <header>
                <strong>{r.author}</strong>
                <span class="action">{actionLabel(r.action)}</span>
                {#if r.draft}<span class="badge draft">draft</span>{/if}
                <span class="time">{new Date(r.created_at).toLocaleString()}</span>
                {#if r.body.trim().length > 0}
                  <button
                    type="button"
                    class="copy-button"
                    title="Copy markdown source"
                    onclick={() => copyToClipboard(r.body)}>⧉</button
                  >
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
            oncancel={() => (replyingTo = null)}
            onsubmit={submitReply}
          />
        {:else}
          <button
            type="button"
            class="action-button"
            onclick={() => (replyingTo = c.comment_id)}
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

  .fold-toggle {
    background: transparent;
    border: none;
    cursor: pointer;
    color: var(--text-muted);
    font-size: 11px;
    padding: 0 2px;
    margin-right: 2px;
  }

  .fold-toggle:hover {
    color: var(--link);
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

  .original {
    margin-top: 6px;
    font-size: 12px;
    background: var(--bg-panel);
    border-radius: 4px;
    padding: 4px 8px;
  }

  .original pre {
    margin: 4px 0 0;
    font-size: 11px;
    white-space: pre-wrap;
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
    color: var(--text-muted);
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
