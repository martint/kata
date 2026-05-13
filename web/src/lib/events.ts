//! Singleton EventSource that fans out server-sent events to multiple
//! subscribers. The connection opens lazily when the first subscriber
//! arrives and closes when the last leaves.

import type { ReviewId, SessionId } from './types';

export type ServerEvent =
  | { kind: 'review-created'; repo: string; review_id: ReviewId }
  | { kind: 'review-updated'; repo: string; review_id: ReviewId }
  /** Background watcher detected that the branch has moved relative to
   *  the latest patchset — the UI surfaces a Refresh button in response. */
  | { kind: 'review-branch-moved'; repo: string; review_id: ReviewId }
  | {
      kind: 'session-published';
      repo: string;
      review_id: ReviewId;
      session_id: SessionId;
    }
  | {
      kind: 'session-discarded';
      repo: string;
      review_id: ReviewId;
      session_id: SessionId;
    };

type Listener = (event: ServerEvent) => void;

let source: EventSource | null = null;
const listeners = new Set<Listener>();

function ensureConnected(): void {
  if (source) return;
  source = new EventSource('/api/events');
  source.onmessage = (msg) => {
    let event: ServerEvent;
    try {
      event = JSON.parse(msg.data) as ServerEvent;
    } catch {
      return;
    }
    for (const listener of listeners) {
      try {
        listener(event);
      } catch (e) {
        // eslint-disable-next-line no-console
        console.error('event listener threw', e);
      }
    }
  };
  source.onerror = () => {
    // Browser will auto-reconnect with backoff; nothing to do here.
  };
}

export function subscribe(listener: Listener): () => void {
  listeners.add(listener);
  ensureConnected();
  return () => {
    listeners.delete(listener);
    if (listeners.size === 0 && source) {
      source.close();
      source = null;
    }
  };
}
