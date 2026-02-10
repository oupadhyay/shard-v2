import { describe, it, expect, vi } from 'vitest';
import { addMessage } from '../ui/messages';

describe('Performance & Fragment Support', () => {
  it('should append to DocumentFragment without triggering scroll', () => {
    const fragment = document.createDocumentFragment();

    // In our code, we check if (chatArea instanceof HTMLElement).
    // DocumentFragment is NOT an instance of HTMLElement.

    addMessage(fragment, 'user', 'Hello world');

    expect(fragment.children.length).toBe(1);
    const msg = fragment.children[0] as HTMLElement;
    expect(msg.classList.contains('message')).toBe(true);
    expect(msg.classList.contains('user')).toBe(true);
  });

  it('should append to HTMLElement and trigger scroll', () => {
    const container = document.createElement('div');
    // Mock scrollTop/scrollHeight
    Object.defineProperty(container, 'scrollHeight', { value: 100, configurable: true });
    const scrollSetter = vi.fn();
    Object.defineProperty(container, 'scrollTop', { set: scrollSetter, configurable: true });

    addMessage(container, 'assistant', 'Response');

    expect(container.children.length).toBe(1);
    expect(scrollSetter).toHaveBeenCalledWith(100);
  });
});
