//! Tests for the per-review fold-state persistence layer.

import { beforeEach, describe, expect, test } from 'vitest';
import { createFoldStore } from './foldStore';

beforeEach(() => {
  localStorage.clear();
});

describe('createFoldStore', () => {
  test('get returns undefined for ids the user has never touched', () => {
    const s = createFoldStore('repo', 1);
    expect(s.get('file', 'foo')).toBeUndefined();
    expect(s.get('comment', 'c1')).toBeUndefined();
  });

  test('set then get round-trips the boolean', () => {
    const s = createFoldStore('repo', 1);
    s.set('file', 'foo', true);
    s.set('file', 'bar', false);
    expect(s.get('file', 'foo')).toBe(true);
    expect(s.get('file', 'bar')).toBe(false);
  });

  test('different kinds are independent namespaces', () => {
    const s = createFoldStore('repo', 1);
    s.set('file', 'same-id', true);
    s.set('comment', 'same-id', false);
    expect(s.get('file', 'same-id')).toBe(true);
    expect(s.get('comment', 'same-id')).toBe(false);
  });

  test('different (repo, number) pairs are independent stores', () => {
    const a = createFoldStore('repo-a', 1);
    const b = createFoldStore('repo-b', 1);
    const c = createFoldStore('repo-a', 2);
    a.set('file', 'foo', true);
    expect(b.get('file', 'foo')).toBeUndefined();
    expect(c.get('file', 'foo')).toBeUndefined();
  });

  test('values survive across createFoldStore calls for the same (repo, number)', () => {
    // Page reload: the user re-enters the same review, expects to see
    // their fold state restored.
    const first = createFoldStore('repo', 1);
    first.set('file', 'foo', true);
    first.set('comment', 'c1', true);
    const reopened = createFoldStore('repo', 1);
    expect(reopened.get('file', 'foo')).toBe(true);
    expect(reopened.get('comment', 'c1')).toBe(true);
  });

  test('ids lists everything explicitly set under one kind', () => {
    const s = createFoldStore('repo', 1);
    s.set('file', 'a', true);
    s.set('file', 'b', false);
    s.set('comment', 'c', true);
    expect(s.ids('file').sort()).toEqual(['a', 'b']);
    expect(s.ids('comment')).toEqual(['c']);
    expect(s.ids('commit')).toEqual([]);
  });

  test('prune drops ids not in the keep set', () => {
    const s = createFoldStore('repo', 1);
    s.set('file', 'a', true);
    s.set('file', 'b', true);
    s.set('file', 'c', true);
    s.prune('file', ['a', 'c']);
    expect(s.ids('file').sort()).toEqual(['a', 'c']);
    expect(s.get('file', 'b')).toBeUndefined();
  });

  test('prune only touches the targeted kind', () => {
    const s = createFoldStore('repo', 1);
    s.set('file', 'a', true);
    s.set('comment', 'b', true);
    s.prune('file', []);
    expect(s.get('file', 'a')).toBeUndefined();
    expect(s.get('comment', 'b')).toBe(true);
  });

  test('prune persists across reopens', () => {
    const first = createFoldStore('repo', 1);
    first.set('file', 'a', true);
    first.set('file', 'b', true);
    first.prune('file', ['a']);
    const reopened = createFoldStore('repo', 1);
    expect(reopened.get('file', 'b')).toBeUndefined();
    expect(reopened.get('file', 'a')).toBe(true);
  });

  test('a no-op set does not rewrite storage', () => {
    // Setting the same value twice should skip the serialisation +
    // localStorage write. Verify by spying on setItem.
    const s = createFoldStore('repo', 1);
    s.set('file', 'a', true);
    let writes = 0;
    const origSetItem = Storage.prototype.setItem;
    Storage.prototype.setItem = function (...args) {
      writes++;
      return origSetItem.apply(this, args);
    };
    try {
      s.set('file', 'a', true); // same value — should no-op
      expect(writes).toBe(0);
      s.set('file', 'a', false); // different — should write
      expect(writes).toBe(1);
    } finally {
      Storage.prototype.setItem = origSetItem;
    }
  });

  test('a corrupt stored blob is treated as empty (no throw)', () => {
    localStorage.setItem('kata:fold:repo:1', '{not valid json');
    const s = createFoldStore('repo', 1);
    expect(s.get('file', 'foo')).toBeUndefined();
    s.set('file', 'foo', true);
    expect(s.get('file', 'foo')).toBe(true);
  });

  test('a stored blob with the wrong shape is treated as empty', () => {
    localStorage.setItem('kata:fold:repo:1', '"a string, not an object"');
    const s = createFoldStore('repo', 1);
    expect(s.get('file', 'foo')).toBeUndefined();
  });
});
