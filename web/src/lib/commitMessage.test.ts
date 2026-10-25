import { describe, expect, test } from 'vitest';
import { commitBody } from './commitMessage';

describe('commitBody', () => {
  test('is empty for a subject-only description', () => {
    expect(commitBody('Fix the thing')).toBe('');
    expect(commitBody('Fix the thing\n')).toBe('');
    expect(commitBody('Fix the thing\n\n  \n')).toBe('');
  });

  test('drops the blank lines after the subject and trailing whitespace', () => {
    expect(commitBody('Subject\n\n\nFirst line\nsecond line\n\n')).toBe(
      'First line\nsecond line',
    );
  });

  test('keeps indentation, hard wraps and blank lines inside the body', () => {
    expect(commitBody('Subject\n\n    indented code\nwrapped at\n72 columns\n\n- item')).toBe(
      '    indented code\nwrapped at\n72 columns\n\n- item',
    );
  });
});
