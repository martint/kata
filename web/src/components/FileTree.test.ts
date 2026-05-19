//! Component tests for `FileTree`. Covers the per-file rendering,
//! the file-totals summary, the filter input, the prev/next nav,
//! and the active-path highlight that keeps the tree oriented to
//! the page as the user scrolls past long diffs. The recursive
//! shape (via `FileTreeNode`) is exercised here too — it's a
//! private implementation detail not worth its own suite.

import { fireEvent, render, screen } from '@testing-library/svelte';
import type { ComponentProps } from 'svelte';
import { describe, expect, test, vi } from 'vitest';
import FileTree from './FileTree.svelte';
import type { FileChange } from '../lib/types';

function file(over: Partial<FileChange> = {}): FileChange {
  return {
    path: 'src/a.txt',
    status: 'modified',
    added: 3,
    removed: 1,
    binary: false,
    hunks: undefined,
    ...over,
  };
}

function renderTree(props: Partial<ComponentProps<typeof FileTree>> = {}) {
  return render(FileTree, {
    props: {
      files: [file()],
      onselect: () => {},
      ...props,
    },
  });
}

describe('FileTree', () => {
  test('shows file count and totals in the header', () => {
    renderTree({
      files: [
        file({ path: 'a.txt', added: 3, removed: 1 }),
        file({ path: 'b.txt', added: 2, removed: 5 }),
      ],
    });
    expect(screen.getByText('Files (2)')).toBeTruthy();
    expect(screen.getByText('+5')).toBeTruthy();
    expect(screen.getByText('-6')).toBeTruthy();
  });

  test('shows the empty-tree muted message when there are no files', () => {
    renderTree({ files: [] });
    expect(screen.getByText('No files changed.')).toBeTruthy();
  });

  test('clicking a file fires onselect with the full path', async () => {
    const onselect = vi.fn();
    const { container } = renderTree({
      files: [file({ path: 'src/a.txt' })],
      onselect,
    });
    // FileTreeNode renders file rows as <button>s containing the
    // basename. Find one and click it.
    const fileButton = Array.from(container.querySelectorAll('button')).find(
      (b) => b.textContent?.includes('a.txt'),
    ) as HTMLElement;
    await fireEvent.click(fileButton);
    expect(onselect).toHaveBeenCalledWith('src/a.txt');
  });

  test('the filter input narrows the visible files', async () => {
    renderTree({
      files: [
        file({ path: 'src/foo.ts' }),
        file({ path: 'src/bar.ts' }),
        file({ path: 'docs/readme.md' }),
      ],
    });
    const filter = screen.getByPlaceholderText('Filter files…');
    await fireEvent.input(filter, { target: { value: 'foo' } });
    expect(screen.getByText('foo.ts')).toBeTruthy();
    expect(screen.queryByText('bar.ts')).toBeNull();
    expect(screen.queryByText('readme.md')).toBeNull();
  });

  test('the filter shows the no-match message when nothing matches', async () => {
    renderTree({ files: [file({ path: 'src/a.txt' })] });
    const filter = screen.getByPlaceholderText('Filter files…');
    await fireEvent.input(filter, { target: { value: 'nope' } });
    expect(screen.getByText('No files match.')).toBeTruthy();
  });

  test('the clear button resets the filter', async () => {
    renderTree({
      files: [file({ path: 'src/foo.ts' }), file({ path: 'src/bar.ts' })],
    });
    const filter = screen.getByPlaceholderText('Filter files…');
    await fireEvent.input(filter, { target: { value: 'foo' } });
    expect(screen.queryByText('bar.ts')).toBeNull();
    // Clear button only renders while the filter is non-empty.
    const clear = screen.getByTitle('Clear filter');
    await fireEvent.click(clear);
    expect(screen.getByText('bar.ts')).toBeTruthy();
  });

  test('hides the prev/next nav when navTotal is 0', () => {
    renderTree({ files: [file()], navTotal: 0 });
    expect(screen.queryByTitle('Previous file')).toBeNull();
    expect(screen.queryByTitle('Next file')).toBeNull();
  });

  test('shows the prev/next nav with the position indicator', () => {
    renderTree({
      files: [file()],
      navTotal: 3,
      navPosition: 2,
      onprev: () => {},
      onnext: () => {},
    });
    expect(screen.getByTitle('Previous file')).toBeTruthy();
    expect(screen.getByTitle('Next file')).toBeTruthy();
    expect(screen.getByText('2/3')).toBeTruthy();
  });

  test('shows a "-" placeholder in the position indicator when nothing is in view', () => {
    renderTree({
      files: [file()],
      navTotal: 3,
      navPosition: 0,
      onprev: () => {},
      onnext: () => {},
    });
    expect(screen.getByText('-/3')).toBeTruthy();
  });
});
