import { describe, expect, test } from 'vitest';

import { searchReview } from './search';
import type {
  AnnotationView,
  CommentView,
  FileChange,
  Hunk,
  RegularHunk,
} from './types';

function regular(lines: RegularHunk['lines']): Hunk {
  return {
    kind: 'regular',
    base_range: undefined,
    tip_range: undefined,
    lines,
  };
}

function file(
  path: string,
  hunks?: Hunk[],
  overrides: Partial<FileChange> = {},
): FileChange {
  return {
    path,
    status: 'modified',
    hunks,
    binary: false,
    added: 0,
    removed: 0,
    ...overrides,
  };
}

function comment(
  id: string,
  body: string,
  file: string | null = null,
  line: number | null = null,
): CommentView {
  return {
    schema_version: 1,
    comment_id: id,
    session_id: 'sess-1',
    review_id: 'rev-1',
    author: 'a@example.com',
    created_at: '2026-05-20T00:00:00Z',
    patchset: 1,
    anchor_change_id: 'ch1',
    anchor_commit_id: 'co1',
    file: file ?? undefined,
    side: file ? 'tip' : undefined,
    lines: line != null ? { start: line, end: line } : undefined,
    flag: 'must-do',
    body,
    anchor: { kind: 'valid' },
    draft: false,
  } as CommentView;
}

function annotation(
  id: string,
  body: string,
  file: string | null = null,
  line: number | null = null,
): AnnotationView {
  return {
    schema_version: 1,
    annotation_id: id,
    review_id: 'rev-1',
    author: 'a@example.com',
    created_at: '2026-05-20T00:00:00Z',
    updated_at: '2026-05-20T00:00:00Z',
    patchset: 1,
    anchor_change_id: 'ch1',
    anchor_commit_id: 'co1',
    file: file ?? undefined,
    side: file ? 'tip' : undefined,
    lines: line != null ? { start: line, end: line } : undefined,
    body,
    anchor: { kind: 'valid' },
  } as AnnotationView;
}

describe('searchReview', () => {
  test('empty / whitespace query returns no matches', () => {
    const src = {
      files: [
        file('a.txt', [
          regular([
            { origin: 'context', base_line: 1, tip_line: 1, content: 'foo\n' },
          ]),
        ]),
      ],
      comments: [comment('c1', 'foo')],
      annotations: [],
    };
    expect(searchReview('', src)).toEqual([]);
    expect(searchReview('   ', src).length).toBe(0); // whitespace query: spaces don't naturally match, so result is empty
  });

  test('matches a diff line; preserves the line number and content', () => {
    const src = {
      files: [
        file('greeter.ts', [
          regular([
            {
              origin: 'context',
              base_line: 1,
              tip_line: 1,
              content: 'function greet(name: string) {\n',
            },
          ]),
        ]),
      ],
      comments: [],
      annotations: [],
    };
    const m = searchReview('greet', src);
    expect(m.length).toBe(1);
    expect(m[0]).toMatchObject({
      kind: 'line',
      file: 'greeter.ts',
      side: 'tip',
      line: 1,
      matchStart: 9,
      matchEnd: 14,
    });
  });

  test('case-insensitive substring match', () => {
    const src = {
      files: [
        file('a.ts', [
          regular([
            { origin: 'context', base_line: 1, tip_line: 1, content: 'FooBar\n' },
          ]),
        ]),
      ],
      comments: [],
      annotations: [],
    };
    expect(searchReview('foobar', src).length).toBe(1);
    expect(searchReview('OoB', src).length).toBe(1);
    expect(searchReview('xyz', src).length).toBe(0);
  });

  test('emits one match per occurrence on a single line', () => {
    const src = {
      files: [
        file('a.ts', [
          regular([
            { origin: 'context', base_line: 1, tip_line: 1, content: 'foo foo foo\n' },
          ]),
        ]),
      ],
      comments: [],
      annotations: [],
    };
    const m = searchReview('foo', src);
    expect(m.length).toBe(3);
    expect(m.map((x) => x.kind === 'line' ? x.matchStart : -1)).toEqual([0, 4, 8]);
  });

  test('added lines emit `tip` side; removed lines emit `base`', () => {
    const src = {
      files: [
        file('a.ts', [
          regular([
            { origin: 'added', tip_line: 5, content: 'add foo\n' },
            { origin: 'removed', base_line: 4, content: 'rem foo\n' },
          ]),
        ]),
      ],
      comments: [],
      annotations: [],
    };
    const m = searchReview('foo', src);
    expect(m.length).toBe(2);
    expect(m[0]).toMatchObject({ kind: 'line', side: 'tip', line: 5 });
    expect(m[1]).toMatchObject({ kind: 'line', side: 'base', line: 4 });
  });

  test('skips conflict hunks (out of v1 scope)', () => {
    const src = {
      files: [
        file('c.ts', [
          {
            kind: 'conflict',
            sides: [
              { label: 'Base', lines: ['needle line\n'] },
              { label: 'Side 1', lines: ['needle here\n'] },
            ],
          } as Hunk,
        ]),
      ],
      comments: [],
      annotations: [],
    };
    expect(searchReview('needle', src)).toEqual([]);
  });

  test('skips files whose hunks are not yet loaded', () => {
    const src = {
      files: [file('lazy.ts', undefined)], // hunks: undefined
      comments: [],
      annotations: [],
    };
    expect(searchReview('anything', src)).toEqual([]);
  });

  test('matches comment + annotation bodies', () => {
    const src = {
      files: [],
      comments: [comment('c1', 'please check the foo'), comment('c2', 'nothing here')],
      annotations: [annotation('n1', 'design note: foo is intentional')],
    };
    const m = searchReview('foo', src);
    expect(m.length).toBe(2);
    expect(m[0]).toMatchObject({ kind: 'comment', comment_id: 'c1' });
    expect(m[1]).toMatchObject({ kind: 'annotation', annotation_id: 'n1' });
  });

  test('orders matches: per-file diff first, then that file\'s comments/annotations, then review-wide', () => {
    const src = {
      files: [
        file('a.ts', [
          regular([{ origin: 'context', base_line: 1, tip_line: 1, content: 'foo a\n' }]),
        ]),
        file('b.ts', [
          regular([{ origin: 'context', base_line: 1, tip_line: 1, content: 'foo b\n' }]),
        ]),
      ],
      comments: [
        comment('c-review', 'foo: review-wide note', null, null),
        comment('c-on-a', 'foo on a', 'a.ts', 3),
        comment('c-on-b', 'foo on b', 'b.ts', 2),
      ],
      annotations: [],
    };
    const m = searchReview('foo', src);
    expect(m.length).toBe(5);
    expect(m[0]).toMatchObject({ kind: 'line', file: 'a.ts' });
    expect(m[1]).toMatchObject({ kind: 'comment', comment_id: 'c-on-a' });
    expect(m[2]).toMatchObject({ kind: 'line', file: 'b.ts' });
    expect(m[3]).toMatchObject({ kind: 'comment', comment_id: 'c-on-b' });
    expect(m[4]).toMatchObject({ kind: 'comment', comment_id: 'c-review' });
  });

  test('comments on the same file sort by line number, not insertion order', () => {
    const src = {
      files: [file('a.ts', [])],
      comments: [
        comment('c-late', 'foo', 'a.ts', 50),
        comment('c-early', 'foo', 'a.ts', 5),
      ],
      annotations: [],
    };
    const m = searchReview('foo', src);
    expect(m.map((x) => x.kind === 'comment' ? x.comment_id : '')).toEqual([
      'c-early',
      'c-late',
    ]);
  });
});
