<script lang="ts">
  import type { DraftResponseInput, ResolutionAction } from '../lib/types';

  interface Props {
    commentId: string;
    saving: boolean;
    oncancel: () => void;
    onsubmit: (input: DraftResponseInput) => Promise<void>;
  }
  const { commentId, saving, oncancel, onsubmit }: Props = $props();

  let action: ResolutionAction = $state('comment');
  let body: string = $state('');
  let textareaEl: HTMLTextAreaElement | undefined = $state();

  $effect(() => {
    if (textareaEl) textareaEl.focus();
  });

  function submit(e: Event) {
    e.preventDefault();
    if (saving) return;
    onsubmit({ in_reply_to: commentId, action, body });
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      oncancel();
    } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      submit(e);
    }
  }

  const submitLabel = $derived.by(() => {
    if (saving) return 'Saving…';
    switch (action) {
      case 'comment':
        return 'Reply';
      case 'resolve':
        return 'Resolve';
      case 'unresolve':
        return 'Reopen';
      case 'wont-fix':
        return "Won't fix";
    }
  });
</script>

<form class="composer" onsubmit={submit}>
  <header>
    <select bind:value={action} disabled={saving}>
      <option value="comment">Reply</option>
      <option value="resolve">Resolve</option>
      <option value="unresolve">Reopen</option>
      <option value="wont-fix">Won't fix</option>
    </select>
  </header>
  <textarea
    bind:this={textareaEl}
    bind:value={body}
    placeholder="Optional message… (⌘+Enter to submit, Esc to cancel)"
    rows="5"
    onkeydown={onKeydown}
    disabled={saving}
  ></textarea>
  <footer>
    <button type="button" onclick={oncancel} disabled={saving}>Cancel</button>
    <button type="submit" class="primary" disabled={saving}>{submitLabel}</button>
  </footer>
</form>

<style>
  .composer {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 8px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 10px 12px;
    font-size: 13px;
    /* Live inside the parent comment's `.actions` flex row alongside
     * other buttons — without a basis the form sizes to content (the
     * select header) and produces a tiny composer. `100%` forces a
     * wrap to its own row and fills the comment's inner width. */
    flex: 1 1 100%;
    box-sizing: border-box;
  }

  textarea {
    font: inherit;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    resize: vertical;
    min-height: 100px;
    /* Default textarea width is ~20em; let it fill the composer's
     * content box instead. */
    width: 100%;
    box-sizing: border-box;
  }

  footer {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
  }

  header select {
    font: inherit;
  }
</style>
