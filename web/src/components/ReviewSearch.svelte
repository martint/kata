<script lang="ts">
  //! Header-bar search affordance: collapsed-by-default icon button
  //! that expands into a Cmd+F-style strip (input + counter + prev/
  //! next + close) on click or `/`. The state and the actual search
  //! engine live in `ReviewViewer`; this component is just the UI.

  import { tick } from 'svelte';

  import Chevron from './Chevron.svelte';

  interface Props {
    /** Expanded vs collapsed. When `false`, render only the icon
     *  button. Parent flips this on `/` or icon click. */
    open: boolean;
    /** Current query text. Parent owns the canonical state so the
     *  result list and the input field stay in lockstep. */
    query: string;
    /** Total matches across the review for the active query. */
    total: number;
    /** 1-based index of the currently-focused match, or 0 when no
     *  match is active (no query, empty result list). */
    position: number;
    /** True while the parent is force-loading lazy files into the
     *  search source. Surfaced so the counter can show "loading…"
     *  rather than a stale total. */
    loading?: boolean;
    onqueryInput: (q: string) => void;
    onnext: () => void;
    onprev: () => void;
    onopen: () => void;
    onclose: () => void;
  }
  const {
    open,
    query,
    total,
    position,
    loading = false,
    onqueryInput,
    onnext,
    onprev,
    onopen,
    onclose,
  }: Props = $props();

  let inputEl: HTMLInputElement | null = $state(null);

  /** Auto-focus the input the moment the bar expands so `/` followed
   *  by typing "just works". The `await tick()` makes sure the input
   *  has rendered before we try to focus it. */
  $effect(() => {
    if (open && inputEl) {
      void tick().then(() => inputEl?.focus());
    }
  });

  function onKeyDown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      onclose();
    } else if (e.key === 'Enter') {
      e.preventDefault();
      if (e.shiftKey) onprev();
      else onnext();
    }
  }
</script>

{#if !open}
  <button
    type="button"
    class="search-toggle"
    title="Search the review ( / )"
    aria-label="Search the review"
    data-tour="search"
    onclick={onopen}
  >
    🔍
  </button>
{:else}
  <div class="search-bar" role="search" data-tour="search">
    <input
      bind:this={inputEl}
      type="text"
      class="search-input"
      placeholder="Search diff, files, commits, comments…"
      value={query}
      oninput={(e) => onqueryInput((e.currentTarget as HTMLInputElement).value)}
      onkeydown={onKeyDown}
      aria-label="Search query"
    />
    <span class="search-count" aria-live="polite">
      {#if loading}
        loading…
      {:else if query.length === 0}
        &nbsp;
      {:else if total === 0}
        no matches
      {:else}
        {position}/{total}
      {/if}
    </span>
    <button
      type="button"
      class="search-nav"
      title="Previous match (Shift+Enter)"
      aria-label="Previous match"
      onclick={onprev}
      disabled={total === 0}
    ><Chevron dir="left" /></button>
    <button
      type="button"
      class="search-nav"
      title="Next match (Enter)"
      aria-label="Next match"
      onclick={onnext}
      disabled={total === 0}
    ><Chevron dir="right" /></button>
    <button
      type="button"
      class="search-close"
      title="Close (Esc)"
      aria-label="Close search"
      onclick={onclose}
    >×</button>
  </div>
{/if}

<style>
  .search-toggle {
    background: transparent;
    border: 1px solid transparent;
    border-radius: 4px;
    padding: 2px 8px;
    font-size: 13px;
    cursor: pointer;
    color: var(--text-muted);
  }
  .search-toggle:hover {
    background: var(--bg-elevated);
    color: var(--text);
  }

  .search-bar {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 2px 4px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg);
  }

  .search-input {
    font: inherit;
    font-size: 12px;
    padding: 2px 6px;
    border: none;
    background: transparent;
    color: var(--text);
    width: 220px;
    outline: none;
  }
  .search-input::placeholder {
    color: var(--text-faint);
  }

  .search-count {
    font-size: 11px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
    min-width: 56px;
    text-align: right;
    padding: 0 4px;
  }

  .search-nav {
    background: transparent;
    border: none;
    padding: 2px 4px;
    color: var(--text-muted);
    cursor: pointer;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }
  .search-nav:hover:not(:disabled) {
    color: var(--text);
  }
  .search-nav:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }

  .search-close {
    background: transparent;
    border: none;
    padding: 0 4px;
    font-size: 16px;
    line-height: 1;
    color: var(--text-muted);
    cursor: pointer;
  }
  .search-close:hover {
    color: var(--text);
  }
</style>
