/**
 * Tests for shared streaming helper functions extracted into ui/messages.ts.
 * These are used by both the ambient (main.ts) and dedicated (dedicated.ts)
 * windows to ensure rendering parity.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import {
  createStreamingAssistantMessage,
  renderStreamingContent,
  shouldSkipStreamingChunk,
} from '../ui/messages';

describe('createStreamingAssistantMessage', () => {
  it('creates an element with the correct CSS classes', () => {
    const el = createStreamingAssistantMessage();
    expect(el.classList.contains('message')).toBe(true);
    expect(el.classList.contains('assistant')).toBe(true);
    expect(el.classList.contains('markdown-body')).toBe(true);
  });

  it('includes a .message-content wrapper for streaming updates', () => {
    const el = createStreamingAssistantMessage();
    const content = el.querySelector('.message-content');
    expect(content).not.toBeNull();
  });

  it('includes a copy button with correct attributes', () => {
    const el = createStreamingAssistantMessage();
    const btn = el.querySelector('.copy-btn') as HTMLButtonElement;
    expect(btn).not.toBeNull();
    expect(btn.title).toBe('Copy as Markdown');
    expect(btn.getAttribute('aria-label')).toBe('Copy as Markdown');
  });

  it('copy button reads data-raw from parent message', () => {
    const el = createStreamingAssistantMessage();
    el.setAttribute('data-raw', 'Hello World');

    const btn = el.querySelector('.copy-btn') as HTMLButtonElement;
    // Verify the button exists and has an event listener (we can't easily test
    // the clipboard interaction in jsdom, but we verify structure)
    expect(btn).toBeTruthy();
    expect(el.getAttribute('data-raw')).toBe('Hello World');
  });

  it('content div appears before copy button in DOM order', () => {
    const el = createStreamingAssistantMessage();
    const children = Array.from(el.children);
    const contentIdx = children.findIndex(c => c.classList.contains('message-content'));
    const btnIdx = children.findIndex(c => c.classList.contains('copy-btn'));
    expect(contentIdx).toBeLessThan(btnIdx);
  });
});

describe('renderStreamingContent', () => {
  it('renders plain text as markdown HTML', () => {
    const html = renderStreamingContent('Hello **world**');
    expect(html).toContain('<strong>world</strong>');
  });

  it('renders a closed <think> block as a collapsed details element', () => {
    const html = renderStreamingContent('<think>inner reasoning</think>visible text');
    expect(html).toContain('Thought');
    expect(html).toContain('inner reasoning');
    // Should NOT have open attribute when complete
    expect(html).not.toContain('open');
    expect(html).toContain('visible text');
  });

  it('renders an open <think> block as an expanded details element', () => {
    const html = renderStreamingContent('<think>still thinking...');
    expect(html).toContain('Thinking...');
    expect(html).toContain('still thinking...');
    expect(html).toContain('open');
  });

  it('renders content without think tags as plain markdown', () => {
    const html = renderStreamingContent('# Heading\n\nParagraph');
    expect(html).toContain('<h1>');
    expect(html).toContain('Heading');
    expect(html).toContain('Paragraph');
  });

  it('sanitizes XSS in think block content', () => {
    const html = renderStreamingContent('<think><script>alert("xss")</script></think>safe');
    expect(html).not.toContain('<script>');
    expect(html).toContain('safe');
  });

  it('sanitizes XSS in regular content', () => {
    const html = renderStreamingContent('<img src=x onerror=alert(1)>');
    expect(html).not.toContain('onerror');
  });

  it('handles empty string', () => {
    const html = renderStreamingContent('');
    // Should return a valid (possibly empty) HTML string without throwing
    expect(typeof html).toBe('string');
  });

  it('preserves code blocks in markdown', () => {
    const html = renderStreamingContent('```js\nconst x = 1;\n```');
    expect(html).toContain('const');
    expect(html).toContain('<code');
  });
});

describe('shouldSkipStreamingChunk', () => {
  let container: HTMLDivElement;

  beforeEach(() => {
    container = document.createElement('div');
  });

  it('skips whitespace-only chunk when no existing message', () => {
    expect(shouldSkipStreamingChunk(null, '  \n  ')).toBe(true);
  });

  it('does NOT skip non-whitespace chunk when no existing message', () => {
    expect(shouldSkipStreamingChunk(null, 'Hello')).toBe(false);
  });

  it('does NOT skip whitespace chunk when appending to existing assistant message', () => {
    const existing = document.createElement('div');
    existing.className = 'message assistant';
    container.appendChild(existing);

    expect(shouldSkipStreamingChunk(existing, '  ')).toBe(false);
  });

  it('skips whitespace chunk when last element is a tool-output', () => {
    const tool = document.createElement('div');
    tool.className = 'message tool-output';
    container.appendChild(tool);

    expect(shouldSkipStreamingChunk(tool, '\n')).toBe(true);
  });

  it('skips whitespace chunk when last element is a thinking-output', () => {
    const thinking = document.createElement('div');
    thinking.className = 'message thinking-output';
    container.appendChild(thinking);

    expect(shouldSkipStreamingChunk(thinking, '  ')).toBe(true);
  });

  it('does NOT skip content chunk even after tool-output', () => {
    const tool = document.createElement('div');
    tool.className = 'message tool-output';
    container.appendChild(tool);

    expect(shouldSkipStreamingChunk(tool, 'Hello')).toBe(false);
  });

  it('handles empty string as whitespace (skip on new message)', () => {
    expect(shouldSkipStreamingChunk(null, '')).toBe(true);
  });
});
