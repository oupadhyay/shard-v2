import { describe, it, expect, vi } from 'vitest';
import { addMessage } from '../ui/messages';

describe('Performance & Fragment Support', () => {
  it('should append to DocumentFragment without triggering scroll', () => {
    const fragment = document.createDocumentFragment();

    addMessage(fragment, 'user', 'Hello world');

    expect(fragment.children.length).toBe(1);
    const msg = fragment.children[0] as HTMLElement;
    expect(msg.classList.contains('message')).toBe(true);
    expect(msg.classList.contains('user')).toBe(true);
  });

  it('should append to HTMLElement and trigger scroll', () => {
    const container = document.createElement('div');
    Object.defineProperty(container, 'scrollHeight', { value: 100, configurable: true });
    const scrollSetter = vi.fn();
    Object.defineProperty(container, 'scrollTop', { set: scrollSetter, configurable: true });

    addMessage(container, 'assistant', 'Response');

    expect(container.children.length).toBe(1);
    expect(scrollSetter).toHaveBeenCalledWith(100);
  });

  it('should access scrollHeight only once when batching via DocumentFragment', () => {
    const fragment = document.createDocumentFragment();
    const chatArea = document.createElement('div');

    let scrollHeightReads = 0;
    Object.defineProperty(chatArea, 'scrollHeight', {
      get: () => { scrollHeightReads++; return 500; },
      configurable: true,
    });
    const scrollSetter = vi.fn();
    Object.defineProperty(chatArea, 'scrollTop', { set: scrollSetter, configurable: true });

    const messageCount = 20;
    for (let i = 0; i < messageCount; i++) {
      addMessage(fragment, i % 2 === 0 ? 'user' : 'assistant', `Message ${i}`);
    }

    expect(fragment.children.length).toBe(messageCount);
    expect(scrollHeightReads).toBe(0);
    expect(scrollSetter).not.toHaveBeenCalled();

    chatArea.appendChild(fragment);
    chatArea.scrollTop = chatArea.scrollHeight;

    expect(scrollHeightReads).toBe(1);
    expect(scrollSetter).toHaveBeenCalledTimes(1);
  });
});
