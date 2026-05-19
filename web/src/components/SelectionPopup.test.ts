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
      anchorX: 100,
      anchorY: 200,
      oncomment: () => {},
      oncopy: () => {},
      ...props,
    },
  });
}

describe('SelectionPopup', () => {
  test('renders nothing when selection is null', () => {
    const { container } = renderPopup({ selection: null });
    expect(container.querySelector('.selection-popup')).toBeNull();
  });

  test('renders both action buttons when a selection is present', () => {
    renderPopup();
    expect(screen.getByLabelText('Comment on selection')).toBeTruthy();
    expect(screen.getByLabelText('Copy selected text')).toBeTruthy();
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

  test('positions itself just below+right of the anchor (mouseup pointer)', () => {
    // Popup uses `position: fixed`, so positions equal the anchor
    // coords offset by GAP (6 px). Window dimensions in jsdom are
    // large enough that no flip-to-above/left logic kicks in.
    const { container } = renderPopup({ anchorX: 100, anchorY: 200 });
    const popup = container.querySelector('.selection-popup') as HTMLElement;
    expect(popup.style.top).toBe('206px');
    expect(popup.style.left).toBe('106px');
  });

  test('flips above the anchor when the default placement would overflow the viewport bottom', () => {
    // jsdom's innerHeight defaults to 768. An anchor near the
    // bottom must flip the popup above the pointer.
    const { container } = renderPopup({ anchorX: 100, anchorY: 760 });
    const popup = container.querySelector('.selection-popup') as HTMLElement;
    // 760 - 6 - 32 (POPUP_H) = 722. Clamped >= 0.
    expect(popup.style.top).toBe('722px');
  });

  test('flips left of the anchor when the default placement would overflow the viewport right', () => {
    // jsdom's innerWidth defaults to 1024. An anchor near the right
    // edge must flip the popup to the left of the pointer.
    const { container } = renderPopup({ anchorX: 1020, anchorY: 200 });
    const popup = container.querySelector('.selection-popup') as HTMLElement;
    // 1020 - 6 - 62 (POPUP_W) = 952. Clamped >= 4.
    expect(popup.style.left).toBe('952px');
  });

  test('exposes a toolbar role for assistive tech', () => {
    renderPopup();
    expect(screen.getByRole('toolbar', { name: 'Selection actions' })).toBeTruthy();
  });
});
