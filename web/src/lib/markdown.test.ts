import { describe, expect, test } from 'vitest';

import { renderMarkdown } from './markdown';

describe('renderMarkdown', () => {
  test('returns empty string for empty or whitespace input', () => {
    expect(renderMarkdown('')).toBe('');
    expect(renderMarkdown('   \n  ')).toBe('');
  });

  test('wraps plain text in a paragraph', () => {
    const html = renderMarkdown('hello world');
    expect(html).toContain('<p>hello world</p>');
  });

  test('renders fenced code blocks', () => {
    const html = renderMarkdown('```\nlet x = 1;\n```');
    expect(html).toContain('<pre>');
    expect(html).toContain('<code');
    expect(html).toContain('let x = 1;');
  });

  test('renders bullet lists', () => {
    const html = renderMarkdown('- one\n- two');
    expect(html).toContain('<ul>');
    expect(html).toContain('<li>one</li>');
    expect(html).toContain('<li>two</li>');
  });

  test('GFM single newlines become <br>', () => {
    const html = renderMarkdown('line one\nline two');
    expect(html).toContain('<br>');
  });

  test('strips <script> tags', () => {
    const html = renderMarkdown('hi <script>alert(1)</script> bye');
    expect(html).not.toContain('<script');
    expect(html).not.toContain('alert(1)');
  });

  test('strips inline event handlers', () => {
    const html = renderMarkdown('<a href="#" onclick="evil()">click</a>');
    expect(html).not.toContain('onclick');
  });

  test('drops javascript: URLs', () => {
    const html = renderMarkdown('[click](javascript:alert(1))');
    expect(html.toLowerCase()).not.toContain('javascript:');
  });
});
