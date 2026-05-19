<script lang="ts">
  import type { FileChange } from '../lib/types';
  import Chevron from './Chevron.svelte';
  import FileTreeNode from './FileTreeNode.svelte';
  import type { TreeNode } from '../lib/tree';
  import { buildTree, filterTree } from '../lib/tree';

  interface Props {
    files: FileChange[];
    onselect: (path: string) => void;
    /** Path of the file the reader is currently scrolled to. Used
     *  to highlight the matching row so the tree stays oriented to
     *  the page as the reader scrolls past long diffs. `null` means
     *  no file is in view (e.g. scrolled above the first slot). */
    activePath?: string | null;
    /** 1-based index of the active file (0 when none in view). Drives
     *  the position indicator next to the prev/next buttons. */
    navPosition?: number;
    /** Total files navigable via prev/next. Buttons hide when 0. */
    navTotal?: number;
    onprev?: () => void;
    onnext?: () => void;
  }
  const {
    files,
    onselect,
    activePath,
    navPosition = 0,
    navTotal = 0,
    onprev,
    onnext,
  }: Props = $props();

  let query = $state('');

  const fullRoot: TreeNode = $derived(buildTree(files));
  const root: TreeNode = $derived(filterTree(fullRoot, query));
  const matchCount = $derived.by(() => {
    function count(node: TreeNode): number {
      if (node.file) return 1;
      return node.children.reduce((acc, c) => acc + count(c), 0);
    }
    return count(root);
  });
</script>

<nav class="file-tree" aria-label="Changed files" data-tour="file-tree">
  <header>
    <h3>Files ({files.length})</h3>
    {#if navTotal > 0 && onprev && onnext}
      <span class="file-nav">
        <button type="button" title="Previous file" onclick={onprev}>
          <Chevron dir="left" />
        </button>
        <span class="position">{navPosition || '-'}/{navTotal}</span>
        <button type="button" title="Next file" onclick={onnext}>
          <Chevron dir="right" />
        </button>
      </span>
    {/if}
    <span class="totals">
      <span class="adds">+{fullRoot.added}</span>
      <span class="removes">-{fullRoot.removed}</span>
    </span>
  </header>
  <div class="search">
    <input
      type="text"
      bind:value={query}
      placeholder="Filter files…"
      aria-label="Filter files"
    />
    {#if query.trim().length > 0}
      <button
        type="button"
        class="clear"
        title="Clear filter"
        onclick={() => (query = '')}>×</button
      >
    {/if}
  </div>
  <div class="tree-list">
    {#if files.length === 0}
      <p class="muted empty">No files changed.</p>
    {:else if matchCount === 0}
      <p class="muted empty">No files match.</p>
    {:else}
      <ul>
        {#each root.children as child (child.fullPath)}
          <FileTreeNode
            node={child}
            depth={0}
            {onselect}
            {activePath}
          />
        {/each}
      </ul>
    {/if}
  </div>
</nav>

<style>
  .file-tree {
    font-size: 12.5px;
    /* Flex column so the file list area can scroll while the header and
     * search box stay pinned at the top of the tree pane. */
    display: flex;
    flex-direction: column;
    flex: 1;
    min-height: 0;
  }

  .file-tree header {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 12px;
    background: var(--bg-panel);
    border-bottom: 1px solid var(--border);
  }

  .file-tree header h3 {
    margin: 0;
    flex: 1;
  }

  .tree-list {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 4px 0;
  }

  .totals {
    display: flex;
    gap: 6px;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
  }

  .file-nav {
    display: inline-flex;
    align-items: center;
    gap: 2px;
    font-size: 11px;
  }

  .file-nav button {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    background: transparent;
    border: 1px solid transparent;
    color: var(--text);
    padding: 2px 4px;
    line-height: 1;
    cursor: pointer;
    border-radius: 3px;
  }

  .file-nav button:hover {
    background: var(--bg-hover);
    border-color: var(--border);
  }

  .file-nav .position {
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    color: var(--text-muted);
    min-width: 28px;
    text-align: center;
  }

  .adds {
    color: var(--success-text);
  }

  .removes {
    color: var(--error-text);
  }

  .search {
    position: relative;
    padding: 4px 8px;
  }

  .search input {
    width: 100%;
    box-sizing: border-box;
    padding: 4px 22px 4px 8px;
    font-size: 12px;
  }

  .clear {
    position: absolute;
    right: 12px;
    top: 50%;
    transform: translateY(-50%);
    width: 18px;
    height: 18px;
    line-height: 14px;
    padding: 0;
    background: transparent;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 14px;
  }

  .clear:hover {
    color: var(--text);
  }

  ul {
    list-style: none;
    padding: 0;
    margin: 0;
  }

  .empty {
    padding: 4px 8px;
  }
</style>
