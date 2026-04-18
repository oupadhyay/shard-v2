import { describe, it, expect, beforeEach, vi } from 'vitest';
import {
  detectUnrenderedLatex,
  preprocessMarkdown,
  clearKatexErrors,
  getKatexErrors,
  hasKatexErrors,
  __setKatexErrorsForTesting,
  md,
} from '../ui/markdown';
import hljs from 'highlight.js';

describe('Markdown Utilities', () => {
  describe('detectUnrenderedLatex', () => {
    it('should return no errors for plain text', () => {
      expect(detectUnrenderedLatex('Hello world')).toEqual([]);
    });

    it('should return no errors for correctly rendered inline math', () => {
      expect(detectUnrenderedLatex('This is $E=mc^2$ inline.')).toEqual([]);
    });

    it('should return no errors for correctly rendered display math', () => {
      expect(detectUnrenderedLatex('$$\nE=mc^2\n$$')).toEqual([]);
    });

    it('should detect unbalanced $$ delimiters', () => {
      const errors = detectUnrenderedLatex('$$ unbalanced display');
      expect(errors).toContain('Unbalanced $$: missing opening or closing delimiter for display math');
    });

    // Removed unbalanced single $ test as it causes false positives with currency

    it('should detect unrendered LaTeX commands outside delimiters', () => {
      const errors = detectUnrenderedLatex('Check this: \\frac{1}{2}');
      expect(errors.length).toBeGreaterThan(0);
      expect(errors[0]).toContain('Unrendered LaTeX');
      expect(errors[0]).toContain('frac');
    });

    it('should not flag LaTeX commands inside delimiters', () => {
      expect(detectUnrenderedLatex('Correct: $\\frac{1}{2}$')).toEqual([]);
      expect(detectUnrenderedLatex('$$\\sqrt{x}$$')).toEqual([]);
    });

    it('should provide context for unrendered commands', () => {
      const text = 'Some text before \\alpha and some after';
      const errors = detectUnrenderedLatex(text);
      expect(errors[0]).toContain('before \\alpha and some');
    });

    it('should detect multiple types of errors', () => {
      const text = '$$ unbalanced and \\sum_{i=1}^n';
      const errors = detectUnrenderedLatex(text);
      // Detects: unbalanced $$ and unrendered \sum command
      expect(errors.length).toBeGreaterThanOrEqual(1);
      expect(errors.some(e => e.includes('Unbalanced $$'))).toBe(true);
    });
  });

  describe('preprocessMarkdown', () => {
    it('should add blank line BEFORE $$ if missing', () => {
      const input = 'Some text\n$$\nmath\n$$';
      const output = preprocessMarkdown(input);
      expect(output).toBe('Some text\n\n$$\n\nmath\n\n$$');
    });

    it('should add blank line AFTER $$ if missing', () => {
      const input = '$$\nmath\n$$\nSome text';
      const output = preprocessMarkdown(input);
      expect(output).toBe('$$\n\nmath\n\n$$\n\nSome text');
    });

    it('should not add blank lines if they already exist', () => {
      const input = 'Some text\n\n$$\n\nmath\n\n$$\n\nSome text';
      const output = preprocessMarkdown(input);
      expect(output).toBe('Some text\n\n$$\n\nmath\n\n$$\n\nSome text');
    });

    it('should clean up excessive newlines (3+ to 2)', () => {
      const input = 'Line 1\n\n\n\nLine 2';
      const output = preprocessMarkdown(input);
      expect(output).toBe('Line 1\n\nLine 2');
    });

    it('should remove trailing spaces on lines', () => {
      const input = 'Line 1   \nLine 2 \t';
      const output = preprocessMarkdown(input);
      expect(output).toBe('Line 1\nLine 2');
    });

    it('should handle complex cases combined', () => {
      const input = 'Text\n$$\nmath\n$$\n\n\nMore text  ';
      const output = preprocessMarkdown(input);
      // 1. Add blank line before $$ -> Text\n\n$$
      // 2. Insert blank line after opening $$ so math is on its own line:
      //    Text\n\n$$\n\nmath
      // 3. Insert blank line after closing $$ -> Text\n\n$$\n\nmath\n\n$$
      // 4. Clean up 3+ newlines to 2 and remove trailing spaces:
      //    Text\n\n$$\n\nmath\n\n$$\n\nMore text
      expect(output).toBe('Text\n\n$$\n\nmath\n\n$$\n\nMore text');
    });
  });

  describe('KaTeX Error Tracking', () => {
    beforeEach(() => {
      clearKatexErrors();
    });

    it('should initially have no errors', () => {
      expect(hasKatexErrors()).toBe(false);
      expect(getKatexErrors()).toEqual([]);
    });

    it('should have error tracking functions available', () => {
      // KaTeX error callback integration is tested manually in dev environment
      // since jsdom doesn't fully execute KaTeX parsing. This verifies the API exists.
      expect(typeof getKatexErrors).toBe('function');
      expect(typeof hasKatexErrors).toBe('function');
      expect(typeof clearKatexErrors).toBe('function');
    });

    it('should clear errors', () => {
      // Set up errors manually to verify clearing works
      // (In production, KaTeX would populate these via errorCallback)
      __setKatexErrorsForTesting(['Error 1', 'Error 2']);
      expect(hasKatexErrors()).toBe(true);
      expect(getKatexErrors()).toEqual(['Error 1', 'Error 2']);

      clearKatexErrors();

      expect(hasKatexErrors()).toBe(false);
      expect(getKatexErrors()).toEqual([]);
    });

    it('should provide error retrieval functions', () => {
      clearKatexErrors();
      const errors = getKatexErrors();
      expect(Array.isArray(errors)).toBe(true);
      expect(errors).toHaveLength(0);
    });
  });

  describe('Syntax Highlighting', () => {
    it('should highlight code when language is supported', () => {
      const code = 'const x = 1;';
      const lang = 'javascript';
      const result = md.render(`\`\`\`${lang}\n${code}\n\`\`\``);

      expect(result).toContain('hljs-keyword');
      expect(result).toContain('<pre class="hljs">');
    });

    it('should fallback to escaped text when language is not supported', () => {
      const code = 'some code';
      const lang = 'nonexistent-lang';
      const result = md.render(`\`\`\`${lang}\n${code}\n\`\`\``);

      expect(result).toContain('<pre class="hljs"><code>some code');
      expect(result).not.toContain('hljs-');
    });

    it('should escape HTML in fallback mode', () => {
      const code = '<script>alert(1)</script>';
      const lang = 'nonexistent-lang';
      const result = md.render(`\`\`\`${lang}\n${code}\n\`\`\``);

      expect(result).toContain('&lt;script&gt;alert(1)&lt;/script&gt;');
    });

    it('should handle highlighting errors gracefully', () => {
      const code = 'const x = 1;';
      const lang = 'javascript';

      // Mock highlight.js to throw an error
      const spy = vi.spyOn(hljs, 'highlight').mockImplementationOnce(() => {
        throw new Error('Highlighting failed');
      });
      const warnSpy = vi.spyOn(console, 'warn').mockImplementation(() => {});

      const result = md.render(`\`\`\`${lang}\n${code}\n\`\`\``);

      expect(result).toContain('<pre class="hljs"><code>const x = 1;');
      expect(warnSpy).toHaveBeenCalledWith('[Markdown-it highlight] error:', expect.any(Error));

      spy.mockRestore();
      warnSpy.mockRestore();
    });
  });
});
