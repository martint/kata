//! Component tests for SelectionPopup. The popup is the visible
//! surface of the selection-driven comment workflow, so the key
//! invariants are: it renders nothing without a selection, it
//! offers all three actions when a selection is present, and each
//! button routes to its own callback.

import { fireEvent, render, screen } from '@testing-library/svelte';
import type { ComponentProps } from 'svelte';
import { describe, expect, test, vi } from 'vitest';
import SelectionPopup from './SelectionPopup.svelte';
import type { DiffSelection } from '../lib/diffSelection';

function selection(over: Partial<DiffSelection> = {}): DiffSelection {
  return {
    side: 'tip',
    startLine: 10,
    endLine: 10,
    startCol: 4,
    endCol: 9,
    multiLine: false,
    rect: { top: 100, left: 50, right: 200, bottom: 120, width: 150, height: 20, x: 50, y: 100, toJSON: () => ({}) } as DOMRect,
    ...over,
  };
}

function renderPopup(props: Partial<ComponentProps<typeof SelectionPopup>> = {}) {
  return render(SelectionPopup, {
    props: {
      selection: selection(),
      oncomment: () => {},
      oncopy: () => {},
      onpermalink: () => {},
      ...props,
    },
  });
}

describe('SelectionPopup', () => {
  test('renders nothing when selection is null', () => {
    const { container } = renderPopup({ selection: null });
    expect(container.querySelector('.selection-popup')).toBeNull();
  });

  test('renders all three action buttons when a selection is present', () => {
    renderPopup();
    expect(screen.getByLabelText('Comment on selection')).toBeTruthy();
    expect(screen.getByLabelText('Copy selected text')).toBeTruthy();
    expect(screen.getByLabelText('Copy permalink')).toBeTruthy();
  });

  test('routes the comment button to oncomment', async () => {
    const oncomment = vi.fn();
    renderPopup({ oncomment });
    await fireEvent.click(screen.getByLabelText('Comment on selection'));
    expect(oncomment).toHaveBeenCalledOnce();
  });

  test('routes the copy button to oncopy', async () => {
    const oncopy = vi.fn();
    renderPopup({ oncopy });
    await fireEvent.click(screen.getByLabelText('Copy selected text'));
    expect(oncopy).toHaveBeenCalledOnce();
  });

  test('routes the permalink button to onpermalink', async () => {
    const onpermalink = vi.fn();
    renderPopup({ onpermalink });
    await fireEvent.click(screen.getByLabelText('Copy permalink'));
    expect(onpermalink).toHaveBeenCalledOnce();
  });

  test('positions itself relative to the selection rect (in document coords)', () => {
    const { container } = renderPopup({
      selection: selection({
        rect: { top: 50, left: 100, right: 220, bottom: 70, width: 120, height: 20, x: 100, y: 50, toJSON: () => ({}) } as DOMRect,
      }),
    });
    // Scroll offsets default to 0 in jsdom, so positions equal the
    // raw rect's bottom + 4 / right + 4 (the popup's "below + right
    // of selection end" placement).
    const popup = container.querySelector('.selection-popup') as HTMLElement;
    expect(popup.style.top).toBe('74px');
    expect(popup.style.left).toBe('224px');
  });

  test('exposes a toolbar role for assistive tech', () => {
    renderPopup();
    expect(screen.getByRole('toolbar', { name: 'Selection actions' })).toBeTruthy();
  });
});
