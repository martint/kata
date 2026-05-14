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
  let mode = $state<'edit' | 'preview'>('edit');
  let textareaEl: HTMLTextAreaElement | undefined = $state();

  /** Rendered HTML for the preview tab. Only computed when in preview
   *  mode — re-rendering markdown on every keystroke would otherwise
   *  show up in profiles on big summaries. */
  const renderedPreview = $derived(
    mode === 'preview' ? renderMarkdown(draft) : '',
  );

  function startEdit() {
    draft = summary ?? '';
    mode = 'edit';
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
      <span style="flex: 1"></span>
      <div class="tabs" role="tablist">
        <button
          type="button"
          class="tab {mode === 'edit' ? 'active' : ''}"
          role="tab"
          aria-selected={mode === 'edit'}
          onclick={() => (mode = 'edit')}>Write</button
        >
        <button
          type="button"
          class="tab {mode === 'preview' ? 'active' : ''}"
          role="tab"
          aria-selected={mode === 'preview'}
          onclick={() => (mode = 'preview')}>Preview</button
        >
      </div>
    </header>
    {#if mode === 'edit'}
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
    {:else}
      <div class="preview markdown">
        {#if draft.trim().length > 0}
          {@html renderedPreview}
        {:else}
          <em class="muted">Nothing to preview.</em>
        {/if}
      </div>
    {/if}
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

  /* Write/Preview pill — same shape as the comment composer's so the
   * markdown affordances feel like one consistent control across the
   * app. */
  .tabs {
    display: flex;
    border: 1px solid var(--border);
    border-radius: 4px;
    overflow: hidden;
  }

  .tab {
    background: transparent;
    border: none;
    padding: 2px 10px;
    font-size: 12px;
    cursor: pointer;
    color: var(--text-muted);
  }

  .tab.active {
    background: var(--link);
    color: var(--on-accent);
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

  /* Preview pane mirrors the textarea's footprint so toggling between
   * Write and Preview doesn't jank the page height. */
  .preview {
    min-height: 100px;
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg);
  }

  .preview :global(p:first-child) {
    margin-top: 0;
  }

  .preview :global(p:last-child) {
    margin-bottom: 0;
  }

  .preview :global(pre) {
    background: var(--bg-panel);
    padding: 8px;
    border-radius: 4px;
    overflow-x: auto;
  }

  .preview :global(code) {
    background: var(--bg-panel);
    padding: 1px 4px;
    border-radius: 3px;
    font-family: ui-monospace, monospace;
    font-size: 12px;
  }

  .preview :global(pre code) {
    background: transparent;
    padding: 0;
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
