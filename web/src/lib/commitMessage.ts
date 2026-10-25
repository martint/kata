//! Splits a commit description into the parts the UI shows.

/**
 * The body of a commit description: everything after the subject line,
 * without the blank lines that separate it from the subject and without
 * trailing whitespace. Indentation and hard wraps inside the body stay
 * as written. Empty when the description is a subject only.
 */
export function commitBody(description: string): string {
  const i = description.indexOf('\n');
  if (i < 0) return '';
  return description
    .slice(i + 1)
    .replace(/^(?:[ \t]*\n)+/, '')
    .trimEnd();
}
