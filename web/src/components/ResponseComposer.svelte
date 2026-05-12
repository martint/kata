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
    rows="3"
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
    gap: 6px;
    margin-top: 8px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px 10px;
    font-size: 12.5px;
  }

  textarea {
    font: inherit;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    padding: 6px;
    border: 1px solid var(--border);
    border-radius: 4px;
    resize: vertical;
    min-height: 60px;
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
