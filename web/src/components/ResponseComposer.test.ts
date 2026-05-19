//! Component tests for `ResponseComposer`. The form is small but the
//! submit-label switching, the action dropdown's payload effect, and
//! the keyboard shortcuts are all places future refactors could
//! break silently.

import { fireEvent, render, screen } from '@testing-library/svelte';
import { describe, expect, test, vi } from 'vitest';
import ResponseComposer from './ResponseComposer.svelte';
import type { DraftResponseInput } from '../lib/types';

function renderComposer(props: {
  oncancel?: () => void;
  onsubmit?: (input: DraftResponseInput) => Promise<void>;
  saving?: boolean;
}) {
  return render(ResponseComposer, {
    props: {
      commentId: 'c1',
      saving: false,
      oncancel: () => {},
      onsubmit: async () => {},
      ...props,
    },
  });
}

describe('ResponseComposer', () => {
  test('defaults to a "Reply" submission and shows the matching label', () => {
    renderComposer({});
    expect(screen.getByRole('button', { name: 'Reply' })).toBeTruthy();
  });

  test('submits with the comment id, current action, and body', async () => {
    const onsubmit = vi.fn().mockResolvedValue(undefined);
    renderComposer({ onsubmit });
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'thanks' } });
    await fireEvent.submit(textarea.closest('form')!);
    expect(onsubmit).toHaveBeenCalledWith({
      in_reply_to: 'c1',
      action: 'comment',
      body: 'thanks',
    });
  });

  test('routes the chosen action through to the submit payload', async () => {
    const onsubmit = vi.fn().mockResolvedValue(undefined);
    renderComposer({ onsubmit });
    const select = screen.getByRole('combobox') as HTMLSelectElement;
    await fireEvent.change(select, { target: { value: 'resolve' } });
    const textarea = screen.getByRole('textbox') as HTMLTextAreaElement;
    await fireEvent.input(textarea, { target: { value: 'addressed' } });
    await fireEvent.submit(textarea.closest('form')!);
    expect(onsubmit).toHaveBeenCalledWith({
      in_reply_to: 'c1',
      action: 'resolve',
      body: 'addressed',
    });
    // The button label re-derives from the action.
    expect(screen.getByRole('button', { name: 'Resolve' })).toBeTruthy();
  });

  test('Escape cancels and Cmd/Ctrl+Enter submits', async () => {
    const oncancel = vi.fn();
    const onsubmit = vi.fn().mockResolvedValue(undefined);
    renderComposer({ oncancel, onsubmit });
    const textarea = screen.getByRole('textbox');
    await fireEvent.keyDown(textarea, { key: 'Escape' });
    expect(oncancel).toHaveBeenCalledTimes(1);
    await fireEvent.input(textarea, { target: { value: 'go' } });
    await fireEvent.keyDown(textarea, { key: 'Enter', metaKey: true });
    expect(onsubmit).toHaveBeenCalledTimes(1);
  });

  test('refuses to submit while saving and shows "Saving…"', async () => {
    const onsubmit = vi.fn().mockResolvedValue(undefined);
    renderComposer({ onsubmit, saving: true });
    expect(screen.getByRole('button', { name: 'Saving…' })).toBeTruthy();
    const textarea = screen.getByRole('textbox');
    await fireEvent.submit(textarea.closest('form')!);
    expect(onsubmit).not.toHaveBeenCalled();
  });
});
