import { beforeEach, describe, expect, test } from 'vitest';

import { createFoldStore } from './foldStore';
import {
  defaultFoldedForThread,
  hasUnreadReplies,
  isThreadFolded,
  resolutionFor,
} from './resolution';
import type { ResolutionAction, ResponseView } from './types';

function resp(
  id: string,
  action: ResolutionAction,
  createdAt = '2026-01-01T00:00:00Z',
  inReplyTo = 'c1',
): ResponseView {
  return {
    schema_version: 1,
    response_id: id,
    in_reply_to: inReplyTo,
    session_id: 's1',
    author: 'a@example.com',
    created_at: createdAt,
    action,
    body: '',
    draft: false,
  };
}

describe('resolutionFor', () => {
  test('open when there are no responses', () => {
    expect(resolutionFor('c1', [])).toBe('open');
  });

  test('plain comment responses do not change state', () => {
    expect(resolutionFor('c1', [resp('r1', 'comment')])).toBe('open');
  });

  test('resolve flips to resolved', () => {
    expect(resolutionFor('c1', [resp('r1', 'resolve')])).toBe('resolved');
  });

  test('wont-fix flips to wont-fix', () => {
    expect(resolutionFor('c1', [resp('r1', 'wont-fix')])).toBe('wont-fix');
  });

  test('last status action wins, in created_at order', () => {
    expect(
      resolutionFor('c1', [
        resp('r1', 'resolve', '2026-01-01T00:00:00Z'),
        resp('r2', 'wont-fix', '2026-01-02T00:00:00Z'),
      ]),
    ).toBe('wont-fix');
  });

  test('unresolve reopens regardless of previous status', () => {
    expect(
      resolutionFor('c1', [
        resp('r1', 'wont-fix', '2026-01-01T00:00:00Z'),
        resp('r2', 'unresolve', '2026-01-02T00:00:00Z'),
      ]),
    ).toBe('open');
    expect(
      resolutionFor('c1', [
        resp('r1', 'resolve', '2026-01-01T00:00:00Z'),
        resp('r2', 'unresolve', '2026-01-02T00:00:00Z'),
      ]),
    ).toBe('open');
  });

  test('intervening comment responses do not affect status', () => {
    expect(
      resolutionFor('c1', [
        resp('r1', 'resolve', '2026-01-01T00:00:00Z'),
        resp('r2', 'comment', '2026-01-02T00:00:00Z'),
      ]),
    ).toBe('resolved');
  });

  test('ignores responses targeting a different comment', () => {
    expect(
      resolutionFor('c1', [resp('r1', 'resolve', '2026-01-01T00:00:00Z', 'other')]),
    ).toBe('open');
  });
});

describe('defaultFoldedForThread', () => {
  test('compact view-mode folds every thread regardless of resolution', () => {
    // Compact mode = "comments stay out of the way by default" — so
    // even an open thread should default-fold. Resolution doesn't
    // matter here; the view mode wins.
    expect(defaultFoldedForThread('c1', [], true)).toBe(true);
    expect(
      defaultFoldedForThread('c1', [resp('r1', 'resolve')], true),
    ).toBe(true);
  });

  test('full view-mode keeps open threads expanded', () => {
    expect(defaultFoldedForThread('c1', [], false)).toBe(false);
    expect(
      defaultFoldedForThread('c1', [resp('r1', 'comment')], false),
    ).toBe(false);
  });

  test('full view-mode folds resolved + wont-fix threads', () => {
    // The visual cue that a thread is "done" is that it folds by
    // default — no separate auto-collapse concept any more.
    expect(
      defaultFoldedForThread('c1', [resp('r1', 'resolve')], false),
    ).toBe(true);
    expect(
      defaultFoldedForThread('c1', [resp('r1', 'wont-fix')], false),
    ).toBe(true);
  });
});

describe('isThreadFolded', () => {
  beforeEach(() => localStorage.clear());

  test('returns the stored override when one is present', () => {
    const store = createFoldStore('repo', 1);
    // Stored override beats the default in either direction.
    store.set('comment', 'c1', false);
    expect(isThreadFolded('c1', [], store, true)).toBe(false);
    store.set('comment', 'c1', true);
    expect(isThreadFolded('c1', [], store, false)).toBe(true);
  });

  test('falls back to the resolution-aware default with no override', () => {
    const store = createFoldStore('repo', 1);
    expect(isThreadFolded('c1', [], store, false)).toBe(false);
    expect(
      isThreadFolded('c1', [resp('r1', 'resolve')], store, false),
    ).toBe(true);
  });

  test('works without a foldStore (degrades to the default)', () => {
    expect(isThreadFolded('c1', [], undefined, true)).toBe(true);
    expect(isThreadFolded('c1', [], undefined, false)).toBe(false);
  });
});

describe('hasUnreadReplies', () => {
  function unread(over: Partial<ResponseView> = {}): ResponseView {
    return {
      ...resp('r1', 'comment', '2026-05-16T09:00:00Z'),
      author: 'author@example.com',
      ...over,
    };
  }

  test('flags a reply newer than the last visit, by someone else', () => {
    expect(
      hasUnreadReplies('c1', [unread()], '2026-05-15T20:00:00Z', 'reviewer@example.com'),
    ).toBe(true);
  });

  test("doesn't flag replies the viewer wrote themselves", () => {
    expect(
      hasUnreadReplies(
        'c1',
        [unread({ author: 'reviewer@example.com' })],
        '2026-05-15T20:00:00Z',
        'reviewer@example.com',
      ),
    ).toBe(false);
  });

  test("doesn't flag drafts", () => {
    // Drafts are invisible to other viewers until publish, so they
    // can't be "unread" to anyone but their own author.
    expect(
      hasUnreadReplies(
        'c1',
        [unread({ draft: true })],
        '2026-05-15T20:00:00Z',
        'reviewer@example.com',
      ),
    ).toBe(false);
  });

  test("doesn't flag replies older than the last visit", () => {
    expect(
      hasUnreadReplies(
        'c1',
        [unread({ created_at: '2026-05-14T09:00:00Z' })],
        '2026-05-15T20:00:00Z',
        'reviewer@example.com',
      ),
    ).toBe(false);
  });

  test('returns false on first ever open (no recorded last visit)', () => {
    expect(
      hasUnreadReplies('c1', [unread()], null, 'reviewer@example.com'),
    ).toBe(false);
  });
});
