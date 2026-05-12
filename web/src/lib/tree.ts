import type { FileChange } from './types';

export interface TreeNode {
  name: string;
  fullPath: string;
  children: TreeNode[];
  file: FileChange | null;
  added: number;
  removed: number;
}

function countChanges(file: FileChange): { added: number; removed: number } {
  if (!file.hunks) return { added: 0, removed: 0 };
  let added = 0;
  let removed = 0;
  for (const h of file.hunks) {
    for (const l of h.lines) {
      if (l.origin === 'added') added++;
      else if (l.origin === 'removed') removed++;
    }
  }
  return { added, removed };
}

function rollup(node: TreeNode): { added: number; removed: number } {
  if (node.file) {
    const c = countChanges(node.file);
    node.added = c.added;
    node.removed = c.removed;
    return c;
  }
  let added = 0;
  let removed = 0;
  for (const child of node.children) {
    const c = rollup(child);
    added += c.added;
    removed += c.removed;
  }
  node.added = added;
  node.removed = removed;
  return { added, removed };
}

function sortRecursive(node: TreeNode): void {
  // Folders first, then files, alphabetical within each.
  node.children.sort((a, b) => {
    const af = a.file ? 1 : 0;
    const bf = b.file ? 1 : 0;
    if (af !== bf) return af - bf;
    return a.name.localeCompare(b.name);
  });
  for (const c of node.children) sortRecursive(c);
}

/** Collapse chains where a folder's sole child is another folder:
 *  `a / b / file.txt` renders as `a/b` containing `file.txt`. Stops as soon
 *  as a folder has more than one child or its child is a file. */
function collapseFolderChains(node: TreeNode): void {
  for (let i = 0; i < node.children.length; i++) {
    let child = node.children[i];
    while (
      child.file === null &&
      child.children.length === 1 &&
      child.children[0].file === null
    ) {
      const inner = child.children[0];
      child = {
        name: `${child.name}/${inner.name}`,
        fullPath: inner.fullPath,
        children: inner.children,
        file: null,
        added: child.added,
        removed: child.removed,
      };
    }
    node.children[i] = child;
    collapseFolderChains(child);
  }
}

/** Return a new tree containing only files whose path matches `query`
 *  (case-insensitive substring), plus their ancestor folders. Counts are
 *  re-rolled up over the visible subset. Empty query → full tree clone. */
export function filterTree(root: TreeNode, query: string): TreeNode {
  const q = query.trim().toLowerCase();
  if (!q) return root;
  const filtered = filter(root, q);
  if (filtered) {
    rollup(filtered);
    return filtered;
  }
  // Nothing matched — return an empty root so the UI can show "no results".
  return { ...root, children: [], added: 0, removed: 0 };
}

function filter(node: TreeNode, q: string): TreeNode | null {
  if (node.file) {
    return node.file.path.toLowerCase().includes(q) ? { ...node } : null;
  }
  const kept: TreeNode[] = [];
  for (const child of node.children) {
    const f = filter(child, q);
    if (f) kept.push(f);
  }
  if (kept.length === 0 && node.name !== '') return null;
  return { ...node, children: kept };
}

/** Walk a tree in display (DFS, folder-first then alphabetical) order and
 *  emit the files in that order. */
export function flattenFiles(root: TreeNode): FileChange[] {
  const result: FileChange[] = [];
  function visit(node: TreeNode): void {
    if (node.file) {
      result.push(node.file);
    } else {
      for (const c of node.children) visit(c);
    }
  }
  visit(root);
  return result;
}

/** Reorder a flat file list to match how those files appear in the tree
 *  sidebar (top-down traversal). */
export function sortFilesLikeTree(files: FileChange[]): FileChange[] {
  return flattenFiles(buildTree(files));
}

/** Build a directory tree from the flat file list. The root node has empty
 *  name and fullPath; iterate its `children` for the top-level entries. */
export function buildTree(files: FileChange[]): TreeNode {
  const root: TreeNode = {
    name: '',
    fullPath: '',
    children: [],
    file: null,
    added: 0,
    removed: 0,
  };
  for (const file of files) {
    const parts = file.path.split('/');
    let node = root;
    for (let i = 0; i < parts.length; i++) {
      const part = parts[i];
      const isLeaf = i === parts.length - 1;
      let child = node.children.find((c) => c.name === part);
      if (!child) {
        child = {
          name: part,
          fullPath: parts.slice(0, i + 1).join('/'),
          children: [],
          file: isLeaf ? file : null,
          added: 0,
          removed: 0,
        };
        node.children.push(child);
      }
      node = child;
    }
  }
  rollup(root);
  sortRecursive(root);
  collapseFolderChains(root);
  return root;
}
