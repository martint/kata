//! Markdown → sanitized HTML.
//!
//! We use `marked` for parsing and `DOMPurify` for sanitization since
//! comments can come from AI agents and arbitrary contributors.

import DOMPurify from 'dompurify';
import { marked } from 'marked';

marked.setOptions({
  breaks: true, // GitHub-style: single newlines become <br>
  gfm: true,
});

export function renderMarkdown(source: string): string {
  if (!source.trim()) return '';
  const raw = marked.parse(source, { async: false }) as string;
  return DOMPurify.sanitize(raw);
}
