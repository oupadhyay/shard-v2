/**
 * Markdown parser configuration for Shard
 */
import MarkdownIt from "markdown-it";
import mk from "@vscode/markdown-it-katex";
import hljs from "highlight.js";
import "highlight.js/styles/github-dark.css";

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

md.use(mk);
