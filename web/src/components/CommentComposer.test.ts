//! Component tests for `CommentComposer`. Covers what the composer
//! does with its `target` prop (line / file / commit / review and the
//! edit-existing-draft branch) and the keyboard shortcuts that drive
//! submit/cancel, since those are easy to break and hard to spot
//! without exercising the rendered form.

import { render, screen, fireEvent } from '@testing-library/svelte';
import { describe, expect, test, vi } from 'vitest';
import CommentComposer from './CommentComposer.svelte';
import type { ComposerTarget, DraftCommentInput, Flag } from '../lib/types';

const anchorIds = { change: 'ch1', commit: 'co1' };

function renderComposer(props: {
  target: ComposerTarget;
  oncancel?: () => void;
  onsubmit?: (input: DraftCommentInput) => Promise<void>;
  saving?: boolean;
}) {
  return render(CommentComposer, {
    props: {
      anchorIds,
      saving: false,
      oncancel: () => {},
      onsubmit: async () => {},
      ...props,
    },
  });
}

describe('CommentComposer', () => {
  test('seeds the flag to must-do for a new line comment', () => {
    renderComposer({
      target: {
        kind: 'line',
        file: 'a.txt',
        side: 'tip',
        startLine: 1,
        endLine: 1,
      },
    });
    const mustDo = screen.getByLabelText('Must do') as HTMLInputElement;
    expect(mustDo.checked).toBe(true);
  });

  test('seeds flag + body from the existing draft when editing', () => {
    const editing = {
      commentId: 'c1',
      body: 'previously written',
      flag: 'question' as Flag,
    };
    renderComposer({
      target: {
        kind: 'line',
        file: 'a.txt',
        side: 'tip',
        startLine: 5,
        endLine: 5,
        editing,
      },
    });
    const question = screen.getByLabelText('Question') as HTMLInputElement;
    expect(question.checked).toBe(true);
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    expect(textarea.value).toBe('previously written');
  });

  test('submits a line comment with file / side / lines payload', async () => {
    const onsubmit = vi.fn().mockResolvedValue(undefined);
    renderComposer({
      target: {
        kind: 'line',
        file: 'a.txt',
        side: 'tip',
        startLine: 5,
        endLine: 7,
      },
      onsubmit,
    });
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'looks off' } });
    // The form's onsubmit handler is wired to the form itself, not a
    // button, so submit by dispatching `submit` on the form element.
    const form = textarea.closest('form')!;
    await fireEvent.submit(form);
    expect(onsubmit).toHaveBeenCalledWith({
      anchor_change_id: 'ch1',
      anchor_commit_id: 'co1',
      flag: 'must-do',
      body: 'looks off',
      file: 'a.txt',
      side: 'tip',
      lines: { start: 5, end: 7 },
    });
  });

  test('submits a file-level comment without lines / side', async () => {
    const onsubmit = vi.fn().mockResolvedValue(undefined);
    renderComposer({
      target: { kind: 'file', file: 'a.txt' },
      onsubmit,
    });
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'whole file' } });
    await fireEvent.submit(textarea.closest('form')!);
    expect(onsubmit).toHaveBeenCalledWith({
      anchor_change_id: 'ch1',
      anchor_commit_id: 'co1',
      flag: 'must-do',
      body: 'whole file',
      file: 'a.txt',
    });
  });

  test('marks a review-wide comment with review_wide: true', async () => {
    const onsubmit = vi.fn().mockResolvedValue(undefined);
    renderComposer({
      target: { kind: 'review' },
      onsubmit,
    });
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'overall' } });
    await fireEvent.submit(textarea.closest('form')!);
    expect(onsubmit).toHaveBeenCalledWith(
      expect.objectContaining({ review_wide: true, body: 'overall' }),
    );
  });

  test('Escape on the textarea cancels the composer', async () => {
    const oncancel = vi.fn();
    renderComposer({
      target: { kind: 'review' },
      oncancel,
    });
    const textarea = screen.getByRole('textbox');
    await fireEvent.keyDown(textarea, { key: 'Escape' });
    expect(oncancel).toHaveBeenCalledTimes(1);
  });

  test('Cmd/Ctrl+Enter submits without clicking the save button', async () => {
    const onsubmit = vi.fn().mockResolvedValue(undefined);
    renderComposer({
      target: { kind: 'review' },
      onsubmit,
    });
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'hi' } });
    await fireEvent.keyDown(textarea, { key: 'Enter', metaKey: true });
    expect(onsubmit).toHaveBeenCalledTimes(1);
  });

  test('refuses to submit while saving is true', async () => {
    const onsubmit = vi.fn().mockResolvedValue(undefined);
    renderComposer({
      target: { kind: 'review' },
      onsubmit,
      saving: true,
    });
    const textarea = screen.getByRole('textbox');
    await fireEvent.input(textarea, { target: { value: 'hi' } });
    await fireEvent.submit(textarea.closest('form')!);
    expect(onsubmit).not.toHaveBeenCalled();
  });

  test('button label flips to "Save changes" when editing an existing draft', () => {
    renderComposer({
      target: {
        kind: 'file',
        file: 'a.txt',
        editing: { commentId: 'c1', body: 'b', flag: 'must-do' },
      },
    });
    expect(screen.getByRole('button', { name: 'Save changes' })).toBeTruthy();
  });

  test('header carries flags and tabs on the same row, no "commenting on" heading', () => {
    // Layout was previously: heading + spacer + tabs on one row,
    // flags on a second row. The composer is now narrow enough
    // that wrapping the heading squeezed the tabs and made the
    // user's eye jump across the whole form to find each control;
    // the redesign drops the heading and pulls flags into the
    // same header row as the tabs.
    const { container } = renderComposer({
      target: {
        kind: 'line',
        file: 'a/very/long/path/to/a/file.svelte',
        side: 'tip',
        startLine: 123,
        endLine: 145,
      },
    });
    const header = container.querySelector('.composer > header');
    expect(header).toBeTruthy();
    // Both controls live inside the same header element.
    expect(header!.querySelector('.flags')).toBeTruthy();
    expect(header!.querySelector('.tabs')).toBeTruthy();
    // No leftover heading text. The old layout rendered something
    // like "commenting on a/very/long/path…:123-145 (tip)".
    expect(header!.textContent).not.toMatch(/commenting on/i);
    expect(header!.textContent).not.toMatch(/editing draft/i);
  });
});
