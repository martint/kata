// The changed files as the web UI's sidebar shows them: folders before files, alphabetical, with
// single-child folder chains merged ("main/java/io/trino/").

function folder(name, path) {
  return { type: "folder", name, path, folders: new Map(), files: [], added: 0, removed: 0 };
}

function compress(node) {
  for (const [key, child] of node.folders) {
    compress(child);
    if (child.files.length === 0 && child.folders.size === 1) {
      const [only] = child.folders.values();
      only.name = `${child.name}${only.name}`;
      node.folders.set(key, only);
    }
  }
}

export function buildTree(files) {
  const root = folder("", "");
  for (const file of files) {
    const parts = file.path.split("/");
    const name = parts.pop();
    let node = root;
    let path = "";
    for (const part of parts) {
      path = `${path}${part}/`;
      if (!node.folders.has(part)) node.folders.set(part, folder(`${part}/`, path));
      node = node.folders.get(part);
      node.added += file.added;
      node.removed += file.removed;
    }
    node.files.push({ type: "file", name, file });
  }
  compress(root);
  return root;
}

const byName = (a, b) => (a.name < b.name ? -1 : a.name > b.name ? 1 : 0);

// Depth-first, the order the diff stacks files in.
export function orderedFiles(root) {
  const out = [];
  const walk = (node) => {
    for (const child of [...node.folders.values()].sort(byName)) walk(child);
    for (const leaf of [...node.files].sort(byName)) out.push(leaf.file);
  };
  walk(root);
  return out;
}

// Sidebar rows, skipping the insides of collapsed folders.
export function treeRows(root, collapsed) {
  const out = [];
  const walk = (node, depth) => {
    for (const child of [...node.folders.values()].sort(byName)) {
      out.push({ type: "folder", node: child, depth });
      if (!collapsed.has(child.path)) walk(child, depth + 1);
    }
    for (const leaf of [...node.files].sort(byName)) out.push({ type: "file", name: leaf.name, file: leaf.file, depth });
  };
  walk(root, 0);
  return out;
}
