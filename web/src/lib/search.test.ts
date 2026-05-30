import { describe, expect, test } from 'vitest';

import { searchReview, type CommitInfoLite, type SearchSource } from './search';
import type {
  AnnotationView,
  CommentView,
  FileChange,
  Hunk,
  RegularHunk,
  ResponseView,
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

function response(
  id: string,
  inReplyTo: string,
  body: string,
  createdAt = '2026-05-21T00:00:00Z',
): ResponseView {
  return {
    schema_version: 1,
    response_id: id,
    in_reply_to: inReplyTo,
    session_id: 'sess-1',
    author: 'a@example.com',
    created_at: createdAt,
    action: 'comment',
    body,
    draft: false,
  } as ResponseView;
}

function commit(id: string, description: string): CommitInfoLite {
  return { change_id: id as CommitInfoLite['change_id'], description_first_line: description };
}

/** Fill in the search-source defaults so each test only spells out
 *  the bucket it actually exercises. */
function makeSrc(overrides: Partial<SearchSource> = {}): SearchSource {
  return {
    files: [],
    comments: [],
    responses: [],
    annotations: [],
    commits: [],
    reviewName: '',
    reviewSummary: null,
    ...overrides,
  };
}

describe('searchReview', () => {
  test('empty / whitespace query returns no matches', () => {
    const src = makeSrc({
      files: [
        file('a.txt', [
          regular([
            { origin: 'context', base_line: 1, tip_line: 1, content: 'foo\n' },
          ]),
        ]),
      ],
      comments: [comment('c1', 'foo')],
    });
    expect(searchReview('', src)).toEqual([]);
    expect(searchReview('   ', src).length).toBe(0); // whitespace query: spaces don't naturally match, so result is empty
  });

  test('matches a diff line; preserves the line number and content', () => {
    const src = makeSrc({
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
    });
    const m = searchReview('greet', src);
    // 'greet' also matches the file path 'greeter.ts'. File-path
    // match comes first inside the bucket; then the diff-line hit.
    expect(m.length).toBe(2);
    expect(m[0]).toMatchObject({ kind: 'file', file: 'greeter.ts' });
    expect(m[1]).toMatchObject({
      kind: 'line',
      file: 'greeter.ts',
      side: 'tip',
      line: 1,
      matchStart: 9,
      matchEnd: 14,
    });
  });

  test('case-insensitive substring match', () => {
    const src = makeSrc({
      files: [
        file('a.ts', [
          regular([
            { origin: 'context', base_line: 1, tip_line: 1, content: 'FooBar\n' },
          ]),
        ]),
      ],
    });
    expect(searchReview('foobar', src).length).toBe(1);
    expect(searchReview('OoB', src).length).toBe(1);
    expect(searchReview('xyz', src).length).toBe(0);
  });

  test('emits one match per occurrence on a single line', () => {
    const src = makeSrc({
      files: [
        file('a.ts', [
          regular([
            { origin: 'context', base_line: 1, tip_line: 1, content: 'foo foo foo\n' },
          ]),
        ]),
      ],
    });
    const m = searchReview('foo', src);
    expect(m.length).toBe(3);
    expect(m.map((x) => x.kind === 'line' ? x.matchStart : -1)).toEqual([0, 4, 8]);
  });

  test('added lines emit `tip` side; removed lines emit `base`', () => {
    const src = makeSrc({
      files: [
        file('a.ts', [
          regular([
            { origin: 'added', tip_line: 5, content: 'add foo\n' },
            { origin: 'removed', base_line: 4, content: 'rem foo\n' },
          ]),
        ]),
      ],
    });
    const m = searchReview('foo', src);
    expect(m.length).toBe(2);
    expect(m[0]).toMatchObject({ kind: 'line', side: 'tip', line: 5 });
    expect(m[1]).toMatchObject({ kind: 'line', side: 'base', line: 4 });
  });

  test('skips conflict hunks (out of v1 scope)', () => {
    const src = makeSrc({
      files: [
        file('c.ts', [
          {
            kind: 'conflict',
            terms: [
              {
                label: 'Base',
                kind: 'base',
                lines: [
                  {
                    origin: 'context',
                    base_line: 1,
                    tip_line: 1,
                    content: 'needle line\n',
                  },
                ],
              },
              {
                label: 'Side 1',
                kind: 'side',
                lines: [
                  {
                    origin: 'added',
                    base_line: null,
                    tip_line: 1,
                    content: 'needle here\n',
                  },
                ],
              },
            ],
          } as Hunk,
        ]),
      ],
    });
    expect(searchReview('needle', src)).toEqual([]);
  });

  test('skips files whose hunks are not yet loaded', () => {
    const src = makeSrc({
      files: [file('lazy.ts', undefined)], // hunks: undefined
    });
    expect(searchReview('anything', src)).toEqual([]);
  });

  test('matches comment + annotation bodies', () => {
    const src = makeSrc({
      comments: [comment('c1', 'please check the foo'), comment('c2', 'nothing here')],
      annotations: [annotation('n1', 'design note: foo is intentional')],
    });
    const m = searchReview('foo', src);
    expect(m.length).toBe(2);
    expect(m[0]).toMatchObject({ kind: 'comment', comment_id: 'c1' });
    expect(m[1]).toMatchObject({ kind: 'annotation', annotation_id: 'n1' });
  });

  test('orders matches: per-file diff first, then that file\'s comments/annotations, then review-wide', () => {
    const src = makeSrc({
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
    });
    const m = searchReview('foo', src);
    expect(m.length).toBe(5);
    expect(m[0]).toMatchObject({ kind: 'line', file: 'a.ts' });
    expect(m[1]).toMatchObject({ kind: 'comment', comment_id: 'c-on-a' });
    expect(m[2]).toMatchObject({ kind: 'line', file: 'b.ts' });
    expect(m[3]).toMatchObject({ kind: 'comment', comment_id: 'c-on-b' });
    expect(m[4]).toMatchObject({ kind: 'comment', comment_id: 'c-review' });
  });

  test('comments on the same file sort by line number, not insertion order', () => {
    const src = makeSrc({
      files: [file('a.ts', [])],
      comments: [
        comment('c-late', 'foo', 'a.ts', 50),
        comment('c-early', 'foo', 'a.ts', 5),
      ],
    });
    const m = searchReview('foo', src);
    expect(m.map((x) => x.kind === 'comment' ? x.comment_id : '')).toEqual([
      'c-early',
      'c-late',
    ]);
  });

  test('matches file paths — emitted before the file\'s diff lines', () => {
    const src = makeSrc({
      files: [
        file('docs/setup.md', [
          regular([
            { origin: 'context', base_line: 1, tip_line: 1, content: 'unrelated\n' },
          ]),
        ]),
        file('other.ts', [
          regular([
            { origin: 'context', base_line: 1, tip_line: 1, content: 'setup config\n' },
          ]),
        ]),
      ],
    });
    const m = searchReview('setup', src);
    expect(m.length).toBe(2);
    expect(m[0]).toMatchObject({ kind: 'file', file: 'docs/setup.md' });
    expect(m[1]).toMatchObject({ kind: 'line', file: 'other.ts' });
  });

  test('matches response bodies, bucketed under the parent comment\'s file', () => {
    const src = makeSrc({
      files: [
        file('a.ts', [
          regular([{ origin: 'context', base_line: 1, tip_line: 1, content: 'ok\n' }]),
        ]),
      ],
      comments: [comment('c1', 'parent', 'a.ts', 1)],
      responses: [response('r1', 'c1', 'reply mentions foo here')],
    });
    const m = searchReview('foo', src);
    expect(m.length).toBe(1);
    expect(m[0]).toMatchObject({
      kind: 'response',
      response_id: 'r1',
      in_reply_to: 'c1',
      file: 'a.ts',
      line: 1,
    });
  });

  test('drops responses with empty body and responses with no parent', () => {
    const src = makeSrc({
      comments: [comment('c1', 'has foo', null, null)],
      responses: [
        response('r-empty', 'c1', ''), // pure resolution marker — nothing to search
        response('r-orphan', 'c-missing', 'foo orphan'), // no parent in source
      ],
    });
    const m = searchReview('foo', src);
    // Only the parent comment's match — the empty-body and orphaned
    // responses are filtered out.
    expect(m.length).toBe(1);
    expect(m[0]).toMatchObject({ kind: 'comment', comment_id: 'c1' });
  });

  test('matches commit messages — emitted after file buckets, before review-meta', () => {
    const src = makeSrc({
      files: [
        file('a.ts', [
          regular([{ origin: 'context', base_line: 1, tip_line: 1, content: 'foo line\n' }]),
        ]),
      ],
      commits: [
        commit('ch1', 'add foo to greeter'),
        commit('ch2', 'unrelated commit'),
      ],
      reviewName: 'foo review',
    });
    const m = searchReview('foo', src);
    // Order: per-file diff line, then commits bucket, then review-meta.
    expect(m.map((x) => x.kind)).toEqual(['line', 'commit', 'review-meta']);
    expect(m[1]).toMatchObject({ kind: 'commit', change_id: 'ch1' });
  });

  test('matches review name + summary; both flagged as review-meta', () => {
    const src = makeSrc({
      reviewName: 'foo greeter',
      reviewSummary: 'a foo-flavoured refactor',
    });
    const m = searchReview('foo', src);
    expect(m.length).toBe(2);
    expect(m[0]).toMatchObject({ kind: 'review-meta', field: 'name' });
    expect(m[1]).toMatchObject({ kind: 'review-meta', field: 'summary' });
  });
});
