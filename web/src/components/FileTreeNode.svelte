<script lang="ts">
  import type { TreeNode } from '../lib/tree';
  import Self from './FileTreeNode.svelte';

  interface Props {
    node: TreeNode;
    depth: number;
    onselect: (path: string) => void;
    /** Path of the file currently being viewed. The matching leaf
     *  highlights so the tree stays oriented as the reader
     *  scrolls. */
    activePath?: string | null;
  }
  const { node, depth, onselect, activePath }: Props = $props();

  let collapsed = $state(false);

  function statusChar(s: string): string {
    return s.charAt(0).toUpperCase();
  }
</script>

<li>
  {#if node.file}
    <button
      class="leaf"
      class:active={activePath === node.file.path}
      style:padding-left="{8 + depth * 14}px"
      onclick={() => onselect(node.file!.path)}
    >
      <span class="status status-{node.file.status}">{statusChar(node.file.status)}</span>
      <span class="name">{node.name}</span>
      <span class="stats">
        <span class="adds">+{node.added}</span>
        <span class="removes">-{node.removed}</span>
      </span>
    </button>
  {:else}
    <button class="folder" style:padding-left="{8 + depth * 14}px" onclick={() => (collapsed = !collapsed)}>
      <span class="caret">{collapsed ? '▸' : '▾'}</span>
      <span class="name folder-name">{node.name}/</span>
      <span class="stats">
        <span class="adds">+{node.added}</span>
        <span class="removes">-{node.removed}</span>
      </span>
    </button>
    {#if !collapsed}
      <ul>
        {#each node.children as child (child.fullPath)}
          <Self
            node={child}
            depth={depth + 1}
            {onselect}
            {activePath}
          />
        {/each}
      </ul>
    {/if}
  {/if}
</li>

<style>
  li {
    list-style: none;
  }

  ul {
    list-style: none;
    padding: 0;
    margin: 0;
  }

  button {
    width: 100%;
    background: transparent;
    border: none;
    border-radius: 0;
    text-align: left;
    cursor: pointer;
    display: flex;
    align-items: center;
    gap: 6px;
    padding: 2px 8px;
    font: inherit;
    font-size: 12.5px;
  }

  button:hover {
    background: var(--bg-panel);
  }

  /* Highlight the leaf for the file currently being viewed so the
   * reader can see where they are in the change list as they scroll
   * through long diffs. */
  .leaf.active {
    background: var(--link-bg);
    color: var(--link);
  }

  .leaf.active:hover {
    background: var(--link-bg);
  }

  .folder-name {
    color: var(--text-muted);
    font-weight: 500;
  }

  .name {
    flex: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .caret {
    width: 12px;
    display: inline-block;
    color: var(--text-muted);
  }

  .status {
    display: inline-block;
    width: 16px;
    text-align: center;
    font-weight: 600;
    font-size: 10px;
    border-radius: 3px;
    padding: 0 0;
    font-family: ui-monospace, monospace;
  }

  .status-added {
    background: var(--success-bg);
    color: var(--success-text);
  }
  .status-deleted {
    background: var(--error-bg);
    color: var(--error-text);
  }
  .status-modified {
    background: var(--warn-bg);
    color: var(--warn-text);
  }
  .status-renamed {
    background: var(--link-bg);
    color: var(--link);
  }

  .stats {
    display: flex;
    gap: 6px;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 11px;
    color: var(--text-muted);
  }

  .adds {
    color: var(--success-text);
  }
  .removes {
    color: var(--error-text);
  }
</style>
