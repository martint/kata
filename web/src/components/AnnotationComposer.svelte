<script lang="ts">
  //! Composer for author annotations. Mirrors CommentComposer's
  //! markdown-write-then-submit shape but with three intentional cuts:
  //! no flag selector (annotations have no severity), no "draft"
  //! framing in the submit button (annotations publish immediately),
  //! and no review-wide / commit-only target kinds (only line and
  //! file-level — the use case is "extra context for this code").

  import { renderMarkdown } from '../lib/markdown';
  import type { AnnotationInput, Side } from '../lib/types';

  /** What region of code the composer is annotating. `editing` carries
   *  the existing annotation id + body when we're updating instead of
   *  creating. */
  export type AnnotationComposerTarget = (
    | { kind: 'line'; file: string; side: Side; startLine: number; endLine: number }
    | { kind: 'file'; file: string }
  ) & {
    editing?: { annotationId: string; body: string };
  };

  interface Props {
    target: AnnotationComposerTarget;
    anchorIds: { change: string; commit: string };
    saving: boolean;
    oncancel: () => void;
    onsubmit: (input: AnnotationInput) => Promise<void>;
  }
  const { target, anchorIds, saving, oncancel, onsubmit }: Props = $props();

  // Seed from `target.editing` when re-entering an existing annotation.
  // svelte-ignore state_referenced_locally
  let body: string = $state(target.editing?.body ?? '');
  let mode = $state<'edit' | 'preview'>('edit');
  let textareaEl: HTMLTextAreaElement | undefined = $state();

  $effect(() => {
    if (textareaEl && mode === 'edit') {
      requestAnimationFrame(() => textareaEl?.focus({ preventScroll: true }));
    }
  });

  const renderedPreview = $derived(mode === 'preview' ? renderMarkdown(body) : '');

  function submit(e: Event) {
    e.preventDefault();
    if (saving) return;
    const base: AnnotationInput = {
      anchor_change_id: anchorIds.change,
      anchor_commit_id: anchorIds.commit,
      body,
    };
    const input: AnnotationInput =
      target.kind === 'line'
        ? {
            ...base,
            file: target.file,
            side: target.side,
            lines: { start: target.startLine, end: target.endLine },
          }
        : { ...base, file: target.file };
    onsubmit(input);
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

  const heading = $derived.by(() => {
    const verb = target.editing ? 'editing note on' : 'note on';
    if (target.kind === 'line') {
      const range =
        target.startLine === target.endLine
          ? `${target.startLine}`
          : `${target.startLine}-${target.endLine}`;
      return `${verb} ${target.file}:${range} (${target.side})`;
    }
    return `${verb} ${target.file}`;
  });
</script>

<form class="composer" onsubmit={submit}>
  <header>
    <span class="badge">Note</span>
    <span class="muted">{heading}</span>
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
      bind:value={body}
      placeholder="Add author context in Markdown… (⌘+Enter to save, Esc to cancel)"
      rows="4"
      onkeydown={onKeydown}
      disabled={saving}
      spellcheck="false"
      autocomplete="off"
      autocapitalize="off"
    ></textarea>
  {:else}
    <div class="preview">
      {#if renderedPreview}
        {@html renderedPreview}
      {:else}
        <em class="muted">Nothing to preview.</em>
      {/if}
    </div>
  {/if}
  <footer>
    <button type="button" onclick={oncancel} disabled={saving}>Cancel</button>
    <button type="submit" class="primary" disabled={saving || body.trim().length === 0}>
      {saving ? 'Saving…' : target.editing ? 'Save changes' : 'Add note'}
    </button>
  </footer>
</form>

<style>
  .composer {
    display: flex;
    flex-direction: column;
    gap: 8px;
    background: var(--attention-bg);
    border: 1px solid var(--attention-border);
    border-radius: 6px;
    padding: 10px 12px;
    font-family: ui-sans-serif, system-ui, sans-serif;
    font-size: 13px;
    /* Same amber accent stripe as AnnotationBubble so the reader sees
     * "this is the author's note channel, not the comment channel". */
    box-shadow: inset 3px 0 0 var(--attention-border);
  }

  .composer header {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }

  .badge {
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    font-size: 10px;
    background: var(--attention-border);
    color: var(--bg);
    padding: 1px 6px;
    border-radius: 3px;
  }

  .tabs {
    display: flex;
    border: 1px solid var(--attention-border);
    border-radius: 4px;
    overflow: hidden;
  }

  .tab {
    background: transparent;
    border: none;
    padding: 2px 10px;
    font-size: 12px;
    cursor: pointer;
    color: var(--attention-text);
  }

  .tab.active {
    background: var(--attention-border);
    color: var(--bg);
  }

  textarea {
    font: inherit;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    padding: 8px;
    border: 1px solid var(--attention-border);
    border-radius: 6px;
    resize: vertical;
    min-height: 80px;
    background: var(--bg);
  }

  .preview {
    border: 1px solid var(--attention-border);
    border-radius: 6px;
    padding: 8px 12px;
    min-height: 80px;
    background: var(--bg);
  }

  .preview :global(p:first-child) {
    margin-top: 0;
  }
  .preview :global(p:last-child) {
    margin-bottom: 0;
  }

  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
