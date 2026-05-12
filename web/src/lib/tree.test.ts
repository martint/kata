import { describe, expect, test } from 'vitest';

import { buildTree, filterTree, flattenFiles, sortFilesLikeTree } from './tree';
import type { FileChange, Hunk, HunkLine } from './types';

function hunkLine(origin: HunkLine['origin']): HunkLine {
  return { origin, content: 'x\n' };
}

function hunk(adds: number, removes: number): Hunk {
  const lines: HunkLine[] = [];
  for (let i = 0; i < adds; i++) lines.push(hunkLine('added'));
  for (let i = 0; i < removes; i++) lines.push(hunkLine('removed'));
  return { lines };
}

function file(
  path: string,
  status: FileChange['status'] = 'modified',
  adds = 0,
  removes = 0,
): FileChange {
  return {
    path,
    status,
    hunks: adds + removes > 0 ? [hunk(adds, removes)] : [],
    binary: false,
  };
}

describe('buildTree', () => {
  test('groups files under shared folders', () => {
    const root = buildTree([file('a/b/c.txt'), file('a/d.txt')]);
    expect(root.children.length).toBe(1);
    expect(root.children[0].name).toBe('a');
    expect(root.children[0].children.length).toBe(2);
  });

  test('collapses single-child folder chains', () => {
    const root = buildTree([file('a/b/c/d.txt')]);
    expect(root.children.length).toBe(1);
    expect(root.children[0].name).toBe('a/b/c');
    expect(root.children[0].children.length).toBe(1);
    expect(root.children[0].children[0].file?.path).toBe('a/b/c/d.txt');
  });

  test('does not collapse when folder has a file child', () => {
    const root = buildTree([file('a/b.txt'), file('a/c.txt')]);
    expect(root.children[0].name).toBe('a');
  });

  test('sorts folders before files alphabetically within each group', () => {
    const root = buildTree([
      file('zfile.txt'),
      file('a-folder/x.txt'),
      file('mfile.txt'),
      file('b-folder/y.txt'),
    ]);
    expect(root.children.map((c) => c.name)).toEqual([
      'a-folder',
      'b-folder',
      'mfile.txt',
      'zfile.txt',
    ]);
  });

  test('rolls up line counts to folders', () => {
    const root = buildTree([file('a/b.txt', 'modified', 3, 1), file('a/c.txt', 'modified', 2, 4)]);
    expect(root.children[0].added).toBe(5);
    expect(root.children[0].removed).toBe(5);
  });
});

describe('flattenFiles', () => {
  test('emits files in DFS / display order', () => {
    const root = buildTree([file('x.txt'), file('a/b.txt'), file('a/c.txt')]);
    expect(flattenFiles(root).map((f) => f.path)).toEqual([
      'a/b.txt',
      'a/c.txt',
      'x.txt',
    ]);
  });
});

describe('sortFilesLikeTree', () => {
  test('returns files in tree-display order regardless of input order', () => {
    const files = [file('x.txt'), file('a/b.txt'), file('a/c.txt')];
    expect(sortFilesLikeTree(files).map((f) => f.path)).toEqual([
      'a/b.txt',
      'a/c.txt',
      'x.txt',
    ]);
  });
});

describe('filterTree', () => {
  test('empty query returns the input tree unchanged', () => {
    const root = buildTree([file('a.txt'), file('b.txt')]);
    expect(filterTree(root, '').children.length).toBe(2);
    expect(filterTree(root, '   ').children.length).toBe(2);
  });

  test('hides files that do not match', () => {
    const root = buildTree([file('foo/bar.txt'), file('foo/baz.txt')]);
    const filtered = filterTree(root, 'baz');
    expect(flattenFiles(filtered).map((f) => f.path)).toEqual(['foo/baz.txt']);
  });

  test('keeps ancestor folders when a descendant matches', () => {
    const root = buildTree([file('deep/folder/needle.rs'), file('other/x.txt')]);
    const filtered = filterTree(root, 'needle');
    expect(filtered.children.map((c) => c.name)).toEqual(['deep/folder']);
  });

  test('matches case-insensitively against the full path', () => {
    const root = buildTree([file('Foo/Bar.txt')]);
    expect(flattenFiles(filterTree(root, 'foo')).length).toBe(1);
    expect(flattenFiles(filterTree(root, 'BAR')).length).toBe(1);
  });

  test('returns empty children when nothing matches', () => {
    const root = buildTree([file('a.txt'), file('b.txt')]);
    const filtered = filterTree(root, 'zzz');
    expect(filtered.children.length).toBe(0);
  });

  test('re-rolls counts to only visible files', () => {
    const root = buildTree([
      file('a/keep.txt', 'modified', 3, 1),
      file('a/drop.txt', 'modified', 10, 10),
    ]);
    const filtered = filterTree(root, 'keep');
    expect(filtered.children[0].added).toBe(3);
    expect(filtered.children[0].removed).toBe(1);
  });
});
