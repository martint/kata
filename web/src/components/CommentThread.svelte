<script lang="ts">
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
    onreply: (input: DraftResponseInput) => Promise<void>;
    onstatus: (commentId: string, action: ResolutionAction) => Promise<void>;
    ondelete: (comment: CommentView) => Promise<void>;
  }
  const { comments, responses, saving, onreply, onstatus, ondelete }: Props =
    $props();

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

  async function copyToClipboard(text: string) {
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      // Surface as a UI hint? For now, fail silently — most browsers grant
      // clipboard write to same-origin pages.
    }
  }

  async function submitReply(input: DraftResponseInput) {
    await onreply(input);
    replyingTo = null;
  }
</script>

<ul class="thread">
  {#each comments as c (c.comment_id)}
    {@const label = anchorLabel(c.anchor)}
    {@const state = resolutionFor(c.comment_id, responses)}
    {@const replies = responsesFor(c.comment_id)}
    <li
      class="comment {c.draft ? 'draft' : ''} {c.anchor.kind === 'outdated'
        ? 'outdated'
        : ''}"
      data-comment-id={c.comment_id}
    >
      <header>
        <strong>{c.author}</strong>
        <span class="flag flag-{c.flag}">{c.flag}</span>
        {#if c.draft}<span class="badge draft">draft</span>{/if}
        {#if label}<span class="badge anchor-{c.anchor.kind}">{label}</span>{/if}
        {#if state !== 'open'}
          <span class="badge resolution-{state}">{state}</span>
        {/if}
        <span class="time">{new Date(c.created_at).toLocaleString()}</span>
        {#if c.body.trim().length > 0}
          <button
            type="button"
            class="copy-button"
            title="Copy markdown source"
            onclick={() => copyToClipboard(c.body)}>⧉</button
          >
        {/if}
      </header>
      <div class="body markdown">
        {#if c.body.trim().length > 0}
          {@html renderMarkdown(c.body)}
        {:else}
          <em class="muted">(no message)</em>
        {/if}
      </div>
      {#if c.anchor.kind === 'outdated'}
        <details class="original">
          <summary>Original lines (from commit when comment was made)</summary>
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
              class="action-button destructive"
              disabled={saving}
              onclick={() => ondelete(c)}
            >
              Delete
            </button>
          {/if}
        {/if}
      </footer>
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
