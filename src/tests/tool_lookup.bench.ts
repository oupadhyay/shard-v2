import { bench, describe, expect } from 'vitest';

/**
 * Benchmark: Tool Call Lookup Optimization
 *
 * Compares Array.from().reverse().find() vs backward for-loop
 * for matching tool call elements by data attribute — the pattern
 * optimized in src/main.ts for loadChatHistory and AGENT_TOOL_RESULT.
 */

function buildToolElements(count: number, targetId: string): Element[] {
  const elements: Element[] = [];
  for (let i = 0; i < count; i++) {
    const el = document.createElement('div');
    el.className = 'tool-output';
    el.setAttribute('data-tool-id', i === count - 2 ? targetId : `tool-${i}`);
    el.setAttribute('data-tool-name', i === count - 2 ? 'web_search' : `tool_${i}`);
    elements.push(el);
  }
  return elements;
}

function buildContainer(elements: Element[]): HTMLElement {
  const container = document.createElement('div');
  for (const el of elements) {
    container.appendChild(el.cloneNode(true));
  }
  return container;
}

// Sink to prevent dead-code elimination
let sink: Element | undefined;

const SIZES = [50, 200, 1000];
const TARGET_ID = 'target-tool-id';

for (const size of SIZES) {
  const elements = buildToolElements(size, TARGET_ID);
  const container = buildContainer(elements);

  describe(`Tool ID lookup (${size} elements)`, () => {
    bench('Array.from().reverse().find()', () => {
      const toolMessages = Array.from(container.querySelectorAll('.tool-output'));
      sink = toolMessages
        .reverse()
        .find((el) => el.getAttribute('data-tool-id') === TARGET_ID);
      expect(sink).toBeDefined();
    });

    bench('backward for loop', () => {
      const toolMessages = container.querySelectorAll('.tool-output');
      for (let i = toolMessages.length - 1; i >= 0; i--) {
        if (toolMessages[i].getAttribute('data-tool-id') === TARGET_ID) {
          sink = toolMessages[i];
          break;
        }
      }
      expect(sink).toBeDefined();
    });
  });

  describe(`Tool name lookup (${size} elements)`, () => {
    bench('Array.from().reverse().find()', () => {
      const toolMessages = Array.from(container.querySelectorAll('.tool-output'));
      sink = toolMessages
        .reverse()
        .find((el) => el.getAttribute('data-tool-name') === 'web_search');
      expect(sink).toBeDefined();
    });

    bench('backward for loop', () => {
      const toolMessages = container.querySelectorAll('.tool-output');
      for (let i = toolMessages.length - 1; i >= 0; i--) {
        if (toolMessages[i].getAttribute('data-tool-name') === 'web_search') {
          sink = toolMessages[i];
          break;
        }
      }
      expect(sink).toBeDefined();
    });
  });
}
