<script lang="ts">
  import { renderMarkdown } from '../lib/markdown';

  interface Props {
    summary: string | undefined;
    /** True when the current viewer is the review's `created_by`. Hides
     *  the Edit / Add affordances for everyone else. */
    editable: boolean;
    saving: boolean;
    onsave: (next: string | null) => Promise<void>;
  }
  const { summary, editable, saving, onsave }: Props = $props();

  let editing = $state(false);
  let draft = $state('');
  let textareaEl: HTMLTextAreaElement | undefined = $state();

  function startEdit() {
    draft = summary ?? '';
    editing = true;
  }

  async function save() {
    const trimmed = draft.trim();
    await onsave(trimmed.length === 0 ? null : trimmed);
    editing = false;
  }

  function cancel() {
    editing = false;
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      cancel();
    } else if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
      e.preventDefault();
      void save();
    }
  }

  $effect(() => {
    if (editing && textareaEl) {
      requestAnimationFrame(() => textareaEl?.focus({ preventScroll: true }));
    }
  });
</script>

{#if editing}
  <section class="summary editing">
    <header>
      <span class="muted">Editing review summary (markdown)</span>
    </header>
    <textarea
      bind:this={textareaEl}
      bind:value={draft}
      placeholder="A short description of the change. Leave empty to clear. (⌘+Enter to save, Esc to cancel)"
      rows="6"
      onkeydown={onKeydown}
      disabled={saving}
      spellcheck="false"
      autocomplete="off"
    ></textarea>
    <footer>
      <button type="button" onclick={cancel} disabled={saving}>Cancel</button>
      <button type="button" class="primary" onclick={save} disabled={saving}>
        {saving ? 'Saving…' : 'Save summary'}
      </button>
    </footer>
  </section>
{:else if summary}
  <section class="summary">
    <div class="body markdown">{@html renderMarkdown(summary)}</div>
    {#if editable}
      <button
        type="button"
        class="edit-btn"
        title="Edit summary"
        onclick={startEdit}
      >
        Edit
      </button>
    {/if}
  </section>
{:else if editable}
  <section class="summary empty">
    <button type="button" class="add-btn" onclick={startEdit}>
      + Add summary
    </button>
  </section>
{/if}

<style>
  .summary {
    margin: 12px 0 16px;
    padding: 12px 14px;
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 6px;
    position: relative;
  }

  .summary.editing,
  .summary.empty {
    background: var(--bg);
  }

  .summary header {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
    margin-bottom: 8px;
  }

  textarea {
    width: 100%;
    box-sizing: border-box;
    font: inherit;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    resize: vertical;
    min-height: 100px;
  }

  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 8px;
  }

  .edit-btn {
    position: absolute;
    top: 8px;
    right: 8px;
    background: transparent;
    border: 1px solid var(--border);
    color: var(--link);
    border-radius: 4px;
    padding: 2px 8px;
    font-size: 12px;
    cursor: pointer;
  }

  .edit-btn:hover {
    background: var(--bg);
  }

  .add-btn {
    background: transparent;
    border: 1px dashed var(--border);
    color: var(--link);
    border-radius: 4px;
    padding: 6px 12px;
    cursor: pointer;
    font-size: 13px;
  }

  .add-btn:hover {
    background: var(--bg-panel);
  }

  .body :global(p:first-child) {
    margin-top: 0;
  }

  .body :global(p:last-child) {
    margin-bottom: 0;
  }

  .body :global(pre) {
    background: var(--bg);
    padding: 8px;
    border-radius: 4px;
    overflow-x: auto;
  }

  .body :global(code) {
    background: var(--bg);
    padding: 1px 4px;
    border-radius: 3px;
    font-family: ui-monospace, monospace;
    font-size: 12px;
  }

  .body :global(pre code) {
    background: transparent;
    padding: 0;
  }
</style>
