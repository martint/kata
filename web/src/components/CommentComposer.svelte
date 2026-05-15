<script lang="ts">
  import { renderMarkdown } from '../lib/markdown';
  import type { ComposerTarget, DraftCommentInput, Flag } from '../lib/types';

  interface Props {
    target: ComposerTarget;
    anchorIds: { change: string; commit: string };
    saving: boolean;
    oncancel: () => void;
    onsubmit: (input: DraftCommentInput) => Promise<void>;
  }
  const { target, anchorIds, saving, oncancel, onsubmit }: Props = $props();

  // Seed from `target.editing` so the composer opens with the draft's
  // current body/flag when re-entering. `$state` ignores its initial
  // value after first render, so this is one-shot — subsequent edits
  // happen in the composer's own state.
  // svelte-ignore state_referenced_locally
  let flag: Flag = $state(target.editing?.flag ?? 'other');
  // svelte-ignore state_referenced_locally
  let body: string = $state(target.editing?.body ?? '');
  let mode = $state<'edit' | 'preview'>('edit');
  let textareaEl: HTMLTextAreaElement | undefined = $state();

  /** Auto-focus on mount. With the FileSlot virtualization in place, only
   *  a handful of file-diffs are live in the DOM when this fires, so
   *  Firefox's per-mount textarea cost is small. The sibling-textarea
   *  warmup that used to wrap this call (and the trick that fixed the
   *  ~1.5s lag) is no longer needed. */
  $effect(() => {
    if (textareaEl && mode === 'edit') {
      requestAnimationFrame(() =>
        textareaEl?.focus({ preventScroll: true }),
      );
    }
  });

  const renderedPreview = $derived(mode === 'preview' ? renderMarkdown(body) : '');

  function submit(e: Event) {
    e.preventDefault();
    if (saving) return;
    const base: DraftCommentInput = {
      anchor_change_id: anchorIds.change,
      anchor_commit_id: anchorIds.commit,
      flag,
      body,
    };
    let input: DraftCommentInput;
    if (target.kind === 'line') {
      input = {
        ...base,
        file: target.file,
        side: target.side,
        lines: { start: target.startLine, end: target.endLine },
      };
    } else if (target.kind === 'file') {
      input = { ...base, file: target.file };
    } else if (target.kind === 'review') {
      input = { ...base, review_wide: true };
    } else {
      // kind: 'commit' — file/lines/side all null, review_wide stays false.
      input = base;
    }
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
    const verb = target.editing ? 'editing draft on' : 'commenting on';
    if (target.kind === 'line') {
      const range =
        target.startLine === target.endLine
          ? `${target.startLine}`
          : `${target.startLine}-${target.endLine}`;
      return `${verb} ${target.file}:${range} (${target.side})`;
    }
    if (target.kind === 'file') {
      return `${verb} ${target.file}`;
    }
    if (target.kind === 'commit') {
      const short = target.change_id.slice(0, 12);
      return `${verb} commit ${short}`;
    }
    return target.editing
      ? 'editing draft on the whole review'
      : 'commenting on the whole review';
  });
</script>

<form class="composer" onsubmit={submit}>
  <header>
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
  <div class="flags">
    <label><input type="radio" bind:group={flag} value="must-do" /> Must do</label>
    <label><input type="radio" bind:group={flag} value="suggestion" /> Suggestion</label>
    <label><input type="radio" bind:group={flag} value="other" /> Other</label>
  </div>
  {#if mode === 'edit'}
    <textarea
      bind:this={textareaEl}
      bind:value={body}
      placeholder="Write a comment in Markdown… (⌘+Enter to submit, Esc to cancel)"
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
    <button type="submit" class="primary" disabled={saving}>
      {saving
        ? 'Saving…'
        : target.editing
          ? 'Save changes'
          : 'Save draft'}
    </button>
  </footer>
</form>

<style>
  .composer {
    display: flex;
    flex-direction: column;
    gap: 8px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 10px 12px;
    font-family: ui-sans-serif, system-ui, sans-serif;
    font-size: 13px;
  }

  .composer header {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }

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

  .flags {
    display: flex;
    gap: 12px;
    font-size: 12px;
  }

  .flags label {
    display: flex;
    align-items: center;
    gap: 4px;
    cursor: pointer;
  }

  textarea {
    font: inherit;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    padding: 8px;
    border: 1px solid var(--border);
    border-radius: 6px;
    resize: vertical;
    min-height: 80px;
  }

  .preview {
    border: 1px solid var(--border);
    border-radius: 6px;
    padding: 8px 12px;
    min-height: 80px;
    background: var(--bg-panel);
  }

  .preview :global(p:first-child) {
    margin-top: 0;
  }

  .preview :global(p:last-child) {
    margin-bottom: 0;
  }

  .preview :global(pre) {
    background: var(--bg-elevated);
    padding: 8px;
    border-radius: 4px;
    overflow-x: auto;
  }

  .preview :global(code) {
    background: var(--bg-elevated);
    padding: 1px 4px;
    border-radius: 3px;
    font-family: ui-monospace, monospace;
    font-size: 12px;
  }

  .preview :global(pre code) {
    padding: 0;
    background: transparent;
  }

  footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
  }
</style>
