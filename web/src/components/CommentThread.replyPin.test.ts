//! Regression test for: a reply composer that's mid-draft must
//! survive the FileSlot virtualising away. CommentThread reports
//! the file path through the `kata-files-with-reply` context on
//! reply start / cancel / submit so ReviewViewer can pin the
//! owning FileSlot via `forceRender` while the user is typing.
//!
//! Without this contract the local `replyingTo` state and the
//! textarea content die the moment the slot scrolls out of view —
//! the user comes back to find the composer gone and the draft
//! lost.

import { fireEvent, render, within } from '@testing-library/svelte';
import { tick } from 'svelte';
import { SvelteSet } from 'svelte/reactivity';
import { describe, expect, test } from 'vitest';
import Host from './CommentThread.replyPin.test.svelte';
import type { CommentView } from '../lib/types';

function comment(over: Partial<CommentView> = {}): CommentView {
  return {
    schema_version: 1,
    comment_id: 'c1',
    session_id: 's1',
    review_id: 'r1',
    author: 'reviewer@example.com',
    created_at: '2026-05-15T10:00:00Z',
    patchset: 1,
    anchor_change_id: 'ch1',
    anchor_commit_id: 'co1',
    file: 'a.txt',
    side: 'tip',
    lines: { start: 1, end: 1 },
    flag: 'must-do',
    body: 'Please address.',
    anchor: { kind: 'valid' },
    draft: false,
    ...over,
  };
}

describe('CommentThread reply-pin contract', () => {
  test(
    'opening a reply registers the file path; cancelling removes it',
    async () => {
      const filesWithReplyInProgress = new SvelteSet<string>();
      const { container } = render(Host, {
        props: {
          comments: [comment()],
          filePath: 'a.txt',
          filesWithReplyInProgress,
        },
      });
      await tick();

      // Find and click "Reply".
      const replyBtn = within(container)
        .getByRole('button', { name: 'Reply' });
      expect(filesWithReplyInProgress.has('a.txt')).toBe(false);
      await fireEvent.click(replyBtn);
      await tick();
      expect(filesWithReplyInProgress.has('a.txt')).toBe(true);

      // Cancel the composer.
      const cancelBtn = within(container)
        .getByRole('button', { name: 'Cancel' });
      await fireEvent.click(cancelBtn);
      await tick();
      expect(filesWithReplyInProgress.has('a.txt')).toBe(false);
    },
  );

  test(
    'submitting a reply removes the file path from the set',
    async () => {
      const filesWithReplyInProgress = new SvelteSet<string>();
      // Holder so the type-narrower doesn't pin `submitted` to
      // `null` — the reassignment happens inside an async callback
      // that TS doesn't connect to the outer flow.
      const submitted: { body?: string } = {};
      const { container } = render(Host, {
        props: {
          comments: [comment()],
          filePath: 'a.txt',
          filesWithReplyInProgress,
          onreply: async (input) => {
            submitted.body = input.body;
          },
        },
      });
      await tick();

      await fireEvent.click(
        within(container).getByRole('button', { name: 'Reply' }),
      );
      await tick();
      expect(filesWithReplyInProgress.has('a.txt')).toBe(true);

      // Fill the body and click the composer's primary submit. We
      // query by `type="submit"` since the submit button's label
      // ("Reply") collides with the per-comment "Reply" button that
      // opened the composer.
      const textarea = container.querySelector('textarea');
      expect(textarea).not.toBeNull();
      await fireEvent.input(textarea!, {
        target: { value: 'thanks for the catch' },
      });
      await tick();

      const submit = container.querySelector(
        'button[type="submit"]',
      ) as HTMLButtonElement | null;
      expect(submit).not.toBeNull();
      await fireEvent.click(submit!);
      // Wait for the async onreply to resolve and the cleanup to
      // run inside submitReply.
      await tick();
      await tick();
      expect(submitted.body).toBe('thanks for the catch');
      expect(filesWithReplyInProgress.has('a.txt')).toBe(false);
    },
  );

  test(
    'no filePath means no context mutation (CommitsPanel call site)',
    async () => {
      const filesWithReplyInProgress = new SvelteSet<string>();
      const { container } = render(Host, {
        props: {
          comments: [comment()],
          filePath: null,
          filesWithReplyInProgress,
        },
      });
      await tick();

      await fireEvent.click(
        within(container).getByRole('button', { name: 'Reply' }),
      );
      await tick();
      // Commit-level reply: nothing should be added to the set
      // (the slot virtualisation concern doesn't apply).
      expect(filesWithReplyInProgress.size).toBe(0);
    },
  );
});
