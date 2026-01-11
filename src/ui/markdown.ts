/**
 * Markdown parser configuration for Shard
 *
 * Includes KaTeX error tracking for auto-retry mechanism.
 */
import MarkdownIt from "markdown-it";
import mk from "@vscode/markdown-it-katex";
import hljs from "highlight.js";
import "highlight.js/styles/github-dark.css";

// ============================================================================
// KaTeX Error Tracking
// ============================================================================
// Track KaTeX parse errors during rendering for auto-retry mechanism.
// Errors are collected per render call and can be retrieved after rendering.

let katexErrors: string[] = [];

/** Clear KaTeX errors before a new render */
export function clearKatexErrors(): void {
  katexErrors = [];
}

/** Get KaTeX errors from last render */
export function getKatexErrors(): string[] {
  return [...katexErrors];
}

/** Check if last render had KaTeX errors */
export function hasKatexErrors(): boolean {
  return katexErrors.length > 0;
}

// Common LaTeX commands that indicate unrendered math
const LATEX_COMMANDS = [
  '\\frac', '\\dfrac', '\\tfrac', '\\sqrt', '\\sum', '\\prod', '\\int',
  '\\lim', '\\sin', '\\cos', '\\tan', '\\log', '\\ln', '\\exp',
  '\\alpha', '\\beta', '\\gamma', '\\delta', '\\theta', '\\pi', '\\tau',
  '\\infty', '\\partial', '\\nabla', '\\cdot', '\\times', '\\div',
  '\\leq', '\\geq', '\\neq', '\\approx', '\\equiv', '\\sim', '\\propto',
  '\\begin{', '\\end{', '\\left', '\\right', '\\top', '\\bot',
  '\\tilde', '\\hat', '\\bar', '\\vec', '\\dot', '\\ddot', // Accents
  '\\text{', '\\mathrm{', '\\mathbf{', '\\mathit{', // Text/font
  '\\quad', '\\qquad', '\\,', '\\;', '\\!', // Spacing
  '^{', '_{', // Common subscript/superscript patterns
];

/**
 * Detect unrendered LaTeX in text (commands outside $ delimiters)
 * Returns array of detected LaTeX fragments for retry hint
 */
export function detectUnrenderedLatex(text: string): string[] {
  const errors: string[] = [];

  // Check for unbalanced $$ delimiters (odd count = missing opening or closing)
  const displayDelimiters = (text.match(/\$\$/g) || []).length;
  if (displayDelimiters % 2 !== 0) {
    errors.push('Unbalanced $$: missing opening or closing delimiter for display math');
  }

  // Check for unbalanced single $ (trickier - need to exclude $$)
  const textWithoutDisplay = text.replace(/\$\$/g, '');
  const inlineDelimiters = (textWithoutDisplay.match(/\$/g) || []).length;
  if (inlineDelimiters % 2 !== 0) {
    errors.push('Unbalanced $: missing opening or closing delimiter for inline math');
  }

  // Remove content inside $ delimiters (properly rendered math)
  const textWithoutMath = text
    .replace(/\$\$[\s\S]*?\$\$/g, '')  // Remove display math (multiline)
    .replace(/\$[^$\n]+\$/g, '');       // Remove inline math (single line)

  // Check for LaTeX commands in remaining text
  for (const cmd of LATEX_COMMANDS) {
    if (textWithoutMath.includes(cmd)) {
      // Find the context around the command
      const idx = textWithoutMath.indexOf(cmd);
      const start = Math.max(0, idx - 10);
      const end = Math.min(textWithoutMath.length, idx + cmd.length + 20);
      const context = textWithoutMath.slice(start, end).trim();
      errors.push(`Unrendered LaTeX: "${context}..." - use $...$ delimiters`);
      break; // One example is enough for the retry hint
    }
  }

  return errors;
}

// Initialize Markdown parser with syntax highlighting
export const md: MarkdownIt = new MarkdownIt({
  html: true,
  linkify: true,
  typographer: true,
  highlight: function (str, lang) {
    if (lang && hljs.getLanguage(lang)) {
      try {
        return '<pre class="hljs"><code>' +
               hljs.highlight(str, { language: lang, ignoreIllegals: true }).value +
               '</code></pre>';
      } catch (__) {}
    }

    return '<pre class="hljs"><code>' + md.utils.escapeHtml(str) + '</code></pre>';
  }
});

// Configure KaTeX with error tracking
md.use(mk, {
  throwOnError: false,  // Don't throw, render error instead
  errorColor: '#cc0000',
  // Custom error callback to track errors
  errorCallback: (msg: string, err: Error) => {
    const errorMsg = `${msg}: ${err.message}`;
    katexErrors.push(errorMsg);
    console.warn('[KaTeX Error]', errorMsg);
  }
});

/**
 * Preprocess markdown to fix common LaTeX formatting issues
 * that break KaTeX rendering.
 *
 * The markdown-it-katex plugin requires display math ($$) to be on its own
 * paragraph with blank lines before and after.
 */
export function preprocessMarkdown(text: string): string {
  return text
    // Ensure blank line BEFORE $$ (if not already at start or after blank line)
    .replace(/([^\n])\n*(\$\$)/g, '$1\n\n$2')
    // Ensure blank line AFTER $$ (if not already before another $$ or end)
    .replace(/(\$\$)\n*([^\n$])/g, '$1\n\n$2')
    // Clean up excessive newlines (more than 2 consecutive)
    .replace(/\n{3,}/g, '\n\n')
    // Remove trailing spaces on lines
    .replace(/[ \t]+$/gm, '');
}
