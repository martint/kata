//! Moved lines in the unified renderer: rows of a moved block carry
//! the moved classes, the block's first row gets a label row linking
//! to the other end, and moved lines stay out of word-diff pairing.

import { render } from '@testing-library/svelte';
import { tick } from 'svelte';
import { describe, expect, test } from 'vitest';
import Host from './HunkLines.foldRefresh.test.svelte';
import { movedLookup } from '../lib/moved';
import type { RegularHunk } from '../lib/types';

function hunk(): RegularHunk {
  return {
    kind: 'regular',
    base_range: { start: 1, end: 3 },
    tip_range: { start: 1, end: 1 },
    lines: [
      { origin: 'removed', base_line: 1, content: 'value = compute(alpha)\n' },
      { origin: 'removed', base_line: 2, content: 'cleanup\n' },
      { origin: 'added', tip_line: 1, content: 'value = compute(alpha2)\n' },
    ],
  };
}

const movedAt = movedLookup([
  {
    side: 'base',
    lines: { start: 1, end: 1 },
    other: { path: 'b.txt', lines: { start: 5, end: 5 } },
  },
]);

describe('HunkLines moved lines', () => {
  test('marks the moved row and labels its other end', async () => {
    const { container } = render(Host, { props: { hunk: hunk(), comments: [], movedAt } });
    await tick();
    const moved = [...container.querySelectorAll('tr.moved')];
    const classes = moved.map((r) => [...r.classList].filter((c) => !c.startsWith('svelte-')));
    expect(classes).toEqual([['row', 'removed', 'moved', 'moved-from']]);
    const badges = [...container.querySelectorAll<HTMLAnchorElement>('a.moved-badge')];
    expect(badges.map((b) => [b.textContent, b.getAttribute('href')])).toEqual([
      ['Code moved to b.txt line 5', '#L:b.txt:tip:5'],
    ]);
  });

  test('keeps the moved line out of word-diff pairing', async () => {
    const { container } = render(Host, { props: { hunk: hunk(), comments: [], movedAt } });
    await tick();
    expect(container.querySelectorAll('.wd-removed, .wd-added').length).toBe(0);
  });

  test('pairs the same lines for word diff when nothing moved', async () => {
    const { container } = render(Host, { props: { hunk: hunk(), comments: [] } });
    await tick();
    expect(container.querySelectorAll('.wd-added').length).toBeGreaterThan(0);
  });
});
