import { describe, expect, test } from 'vitest';

import { resolutionFor } from './resolution';
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
