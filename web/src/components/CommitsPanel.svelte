<script lang="ts">
  import type { CommitInfo } from '../lib/types';
  import { renderMarkdown } from '../lib/markdown';

  interface Props {
    commits: CommitInfo[];
    selectedChangeId: string | null;
    onselect: (changeId: string | null) => void;
  }
  const { commits, selectedChangeId, onselect }: Props = $props();

  let collapsed = $state(false);
  /** Commit-ids whose body is expanded. Body = lines after the first. */
  let expanded: Set<string> = $state(new Set());

  function short(id: string): string {
    return id.length > 12 ? id.slice(0, 12) : id;
  }

  function formatDate(iso: string): string {
    const d = new Date(iso);
    if (isNaN(d.getTime())) return iso;
    return d.toLocaleString();
  }

  /** Description minus the first line. Useful for "more details" toggles. */
  function bodyAfterFirstLine(description: string): string {
    const i = description.indexOf('\n');
    if (i < 0) return '';
    return description.slice(i + 1).replace(/\s+$/, '');
  }

  function toggleExpanded(commitId: string) {
    const next = new Set(expanded);
    if (next.has(commitId)) next.delete(commitId);
    else next.add(commitId);
    expanded = next;
  }
</script>

<section class="commits">
  <header>
    <button
      class="toggle"
      aria-label={collapsed ? 'expand' : 'collapse'}
      onclick={() => (collapsed = !collapsed)}
    >
      {collapsed ? '▸' : '▾'}
    </button>
    <h3>Commits ({commits.length})</h3>
    {#if selectedChangeId !== null}
      <button class="show-all" onclick={() => onselect(null)}>Show all commits</button>
    {/if}
  </header>
  {#if !collapsed}
    {#if commits.length === 0}
      <p class="muted">The revset is empty.</p>
    {:else}
      <ul class="commit-list">
        <li class="commit {selectedChangeId === null ? 'selected' : ''}">
          <button class="row-button" onclick={() => onselect(null)}>
            <span class="all-label">All commits</span>
            <span class="meta">show the combined diff for the whole review</span>
          </button>
        </li>
        {#each commits as c (c.commit_id)}
          {@const body = bodyAfterFirstLine(c.description)}
          {@const isExpanded = expanded.has(c.commit_id)}
          <li class="commit {selectedChangeId === c.change_id ? 'selected' : ''}">
            <div class="row">
              {#if body}
                <button
                  class="expand"
                  aria-label={isExpanded ? 'collapse description' : 'expand description'}
                  aria-expanded={isExpanded}
                  title={isExpanded ? 'Collapse' : 'Show full description'}
                  onclick={() => toggleExpanded(c.commit_id)}
                >
                  {isExpanded ? '▾' : '▸'}
                </button>
              {:else}
                <span class="expand placeholder" aria-hidden="true"></span>
              {/if}
              <button class="row-button" onclick={() => onselect(c.change_id)}>
                <div class="row1">
                  <code class="change">{short(c.change_id)}</code>
                  <code class="commit">{short(c.commit_id)}</code>
                  <span class="desc">{c.description_first_line || '(no description)'}</span>
                </div>
                <div class="row2">
                  <span class="meta">{c.author_email}</span>
                  <span class="meta">·</span>
                  <span class="meta" title={c.author_timestamp}>{formatDate(c.author_timestamp)}</span>
                </div>
              </button>
            </div>
            {#if isExpanded && body}
              <div class="body markdown">{@html renderMarkdown(body)}</div>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  {/if}
</section>

<style>
  .commits {
    border: 1px solid var(--border);
    border-radius: 6px;
    margin: 16px 0;
    overflow: hidden;
  }

  .commits header {
    display: flex;
    align-items: baseline;
    gap: 8px;
    padding: 8px 12px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
  }

  .commits header h3 {
    margin: 0;
  }

  .toggle {
    background: transparent;
    border: none;
    padding: 0;
    font-size: 16px;
    line-height: 1;
    color: var(--text-muted);
    cursor: pointer;
    width: 20px;
  }

  .commit-list {
    list-style: none;
    margin: 0;
    padding: 0;
  }

  .commit {
    border-bottom: 1px solid var(--bg-elevated);
  }

  .commit:last-child {
    border-bottom: none;
  }

  .commit.selected {
    background: var(--link-bg);
  }

  .row-button {
    width: 100%;
    background: transparent;
    border: none;
    border-radius: 0;
    padding: 8px 12px;
    text-align: left;
    cursor: pointer;
    display: flex;
    flex-direction: column;
    gap: 2px;
    /* Allow text selection within the row so users can copy a change id
     * / commit id / title. A real click still selects the commit because
     * mousedown+mouseup at the same point doesn't trigger a selection. */
    user-select: text;
  }

  .row-button:hover {
    background: var(--bg-panel);
  }

  .commit.selected .row-button:hover {
    background: var(--link-bg); filter: brightness(0.9);
  }

  .all-label {
    font-weight: 500;
  }

  .show-all {
    margin-left: auto;
    font-size: 12px;
    padding: 2px 8px;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: 4px;
    color: var(--link);
    cursor: pointer;
  }

  .row1 {
    display: flex;
    align-items: baseline;
    gap: 10px;
  }

  .row1 .change,
  .row1 .commit {
    color: var(--text-muted);
    background: var(--bg-elevated);
    padding: 1px 5px;
    border-radius: 3px;
    font-size: 11px;
  }

  .row1 .desc {
    font-weight: 500;
  }

  .row2 {
    display: flex;
    gap: 6px;
    font-size: 12px;
    color: var(--text-muted);
  }

  .row {
    display: flex;
    align-items: stretch;
  }

  .expand {
    flex: 0 0 auto;
    width: 28px;
    padding: 0;
    border: none;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 16px;
    line-height: 1;
    display: inline-flex;
    align-items: center;
    justify-content: center;
  }

  .expand:hover {
    color: var(--text);
    background: var(--bg-panel);
  }

  .commit.selected .expand:hover {
    background: var(--link-bg); filter: brightness(0.9);
  }

  .expand.placeholder {
    cursor: default;
    pointer-events: none;
  }

  .row .row-button {
    flex: 1;
    min-width: 0;
  }

  .body {
    /* Negative top to tuck the description directly under the subject — the
     * .row-button's own bottom padding (8px) is already the visual gap. */
    margin: -4px 0 0;
    padding: 0 12px 12px 36px;
    background: transparent;
    font-size: 13px;
    color: var(--text);
  }

  /* Tighten markdown defaults so paragraphs/lists don't add their own
   * vertical padding inside the description. */
  .body :global(p),
  .body :global(ul),
  .body :global(ol),
  .body :global(pre) {
    margin: 4px 0;
  }
  .body :global(p:first-child),
  .body :global(ul:first-child),
  .body :global(ol:first-child),
  .body :global(pre:first-child) {
    margin-top: 0;
  }
  .body :global(p:last-child),
  .body :global(ul:last-child),
  .body :global(ol:last-child),
  .body :global(pre:last-child) {
    margin-bottom: 0;
  }
</style>
