/**
 * Message rendering utilities for Shard chat
 */
import DOMPurify from "dompurify";
import { md, preprocessMarkdown } from "./markdown";
import { COPY_ICON, CHECK_ICON, CROSS_ICON } from "./icons";
import type { ImageAttachment, ProactiveMessage } from "../types";
import { invoke } from "@tauri-apps/api/core";
import { logger } from "./utils";

// Track the current web search container for grouping
let currentWebSearchContainer: HTMLElement | null = null;

/**
 * Attaches a smart click listener to a details element to collapse it
 * when clicking the body, while respecting text selection and double clicks.
 */
function attachSmartCollapseListener(details: HTMLDetailsElement) {
  let isSelecting = false;
  let clickTimeout: ReturnType<typeof setTimeout> | null = null;

  details.addEventListener("mousedown", () => {
    isSelecting = false;
  });

  details.addEventListener("mousemove", (e) => {
    if (e.buttons === 1) { // Left mouse button is pressed
      isSelecting = true;
    }
  });

  details.addEventListener("click", (e) => {
    const target = e.target as Element;
    // Let native behavior handle summary clicks
    if (target.closest("summary")) return;

    // Don't close if clicking interactive elements
    if (target.closest("a") || target.closest("button") || target.closest("input")) return;

    if (details.hasAttribute("open")) {
      const selection = window.getSelection();
      const hasSelection = selection && selection.toString().trim().length > 0;

      // Don't collapse if dragging to select or text is actively selected
      if (isSelecting || hasSelection) return;

      if ((e as MouseEvent).detail > 1) {
        // It's a double/triple click (text selection). Cancel any pending collapse.
        if (clickTimeout) {
          clearTimeout(clickTimeout);
          clickTimeout = null;
        }
        return;
      }

      // Defer closing slightly to allow a double-click to register and abort this.
      clickTimeout = setTimeout(() => {
        // Re-check selection just in case they managed to select something super fast
        const delayedSelection = window.getSelection();
        if (delayedSelection && delayedSelection.toString().trim().length > 0) return;

        details.removeAttribute("open");
      }, 200); // 200ms is standard double-click threshold
      e.preventDefault();
    }
  });
}

/**
 * Create a thinking/reasoning block element
 */
export function createThinkingElement(content: string, isComplete: boolean = true): HTMLElement {
  const thinkingMsg = document.createElement("div");
  thinkingMsg.className = "message thinking-output";
  thinkingMsg.setAttribute('data-complete', isComplete ? 'true' : 'false');
  thinkingMsg.setAttribute("data-thinking", content);

  updateThinkingElement(thinkingMsg, content, isComplete);
  return thinkingMsg;
}

/**
 * Update an existing thinking block with new content
 */
export function updateThinkingElement(el: HTMLElement, content: string, isComplete: boolean) {
  el.setAttribute('data-complete', isComplete ? 'true' : 'false');
  el.setAttribute("data-thinking", content);

  const trimmedThinking = content.trimEnd();
  const summaryText = isComplete ? "Thought" : "Thinking...";
  const openAttr = isComplete ? "" : "open";

  let details = el.querySelector("details");
  if (!details) {
    el.innerHTML = `
      <details class="thought-block" ${openAttr}>
        <summary>${summaryText}</summary>
        <div class="thought-content markdown-body">${DOMPurify.sanitize(md.render(preprocessMarkdown(trimmedThinking)))}</div>
      </details>
    `;
    details = el.querySelector("details");
    if (details) attachSmartCollapseListener(details);
  } else {
    // Update summary and open state
    const summary = details.querySelector("summary");
    if (summary) summary.innerHTML = summaryText;
    if (!isComplete) details.setAttribute("open", "");
    else details.removeAttribute("open");
  }

  // Update content
  const contentDiv = details?.querySelector(".thought-content");
  if (contentDiv) {
    contentDiv.innerHTML = DOMPurify.sanitize(md.render(preprocessMarkdown(trimmedThinking)));
  }
}

/**
 * Get or create the web search container for grouping web searches
 */
export function getOrCreateWebSearchContainer(chatArea: HTMLElement | DocumentFragment): HTMLElement {
  // If we already have an active container, return it
  if (currentWebSearchContainer && chatArea.contains(currentWebSearchContainer)) {
    return currentWebSearchContainer;
  }

  // Create new web search container
  const container = document.createElement("div");
  container.className = "message web-search-container";

  container.innerHTML = `
    <details class="web-search-accordion" open>
      <summary class="web-search-summary">
        <span class="web-search-icon">🔍</span>
        <span class="web-search-title">Web Search</span>
        <span class="web-search-count"></span>
      </summary>
      <div class="web-search-queries"></div>
    </details>
  `;

  currentWebSearchContainer = container;
  return container;
}

/**
 * Reset the web search container (call when assistant message starts)
 */
export function resetWebSearchContainer(): void {
  currentWebSearchContainer = null;
}

/**
 * Check if a tool is a web search tool
 */
export function isWebSearchTool(name: string): boolean {
  return name === "web_search";
}

/**
 * Create a tool call accordion element
 */
export function createToolCallElement(name: string, argsStr: string, id?: string, isOpen: boolean = false): HTMLElement {
  const toolDiv = document.createElement("div");
  toolDiv.className = "message tool-output";
  toolDiv.setAttribute("data-tool-name", name);
  if (id) toolDiv.setAttribute("data-tool-id", id);

  let argsObj: Record<string, unknown> = {};
  try {
    argsObj = JSON.parse(argsStr);
  } catch (e) {
    argsObj = { raw: argsStr };
  }

  let argsPretty = JSON.stringify(argsObj, null, 2);
  let summaryArgs = Object.entries(argsObj)
    .map(([k, v]) => `${md.utils.escapeHtml(k)}="${md.utils.escapeHtml(String(v))}"`)
    .join(" ");

  if (Object.keys(argsObj).length === 0) {
    argsPretty = "[No arguments supplied]";
    summaryArgs = "";
  }

  const openAttr = isOpen ? "open" : "";

  toolDiv.innerHTML = `
    <details ${openAttr}>
      <summary>
        <span class="tool-icon">🛠️</span>
        <span class="tool-name">Tool: ${md.utils.escapeHtml(name)}</span>
        <span class="tool-summary-args">${summaryArgs}</span>
      </summary>
      <div class="tool-args">${md.utils.escapeHtml(argsPretty)}</div>
      <div class="tool-result" style="display: none;">
        <div class="tool-result-label">Result:</div>
        <div class="tool-result-content"></div>
      </div>
    </details>
  `;

  const details = toolDiv.querySelector("details");
  if (details) attachSmartCollapseListener(details);

  return toolDiv;
}

/**
 * Create a web search query element (simpler than regular tool call)
 */
export function createWebSearchQueryElement(query: string, id?: string): HTMLElement {
  const queryDiv = document.createElement("div");
  queryDiv.className = "web-search-query";
  queryDiv.setAttribute("data-tool-name", "web_search");
  if (id) queryDiv.setAttribute("data-tool-id", id);

  queryDiv.innerHTML = `
    <details>
      <summary>
        <span class="query-text">"${md.utils.escapeHtml(query || 'Legacy Search')}"</span>
      </summary>
      <div class="tool-result" style="display: none;">
        <div class="tool-result-content markdown-body"></div>
      </div>
    </details>
  `;

  const details = queryDiv.querySelector("details");
  if (details) attachSmartCollapseListener(details);

  return queryDiv;
}

/**
 * Update the web search count in the container
 */
export function updateWebSearchCount(container: HTMLElement): void {
  const queriesContainer = container.querySelector(".web-search-queries");
  const countSpan = container.querySelector(".web-search-count");
  if (queriesContainer && countSpan) {
    const count = queriesContainer.children.length;
    countSpan.textContent = count > 1 ? `(${count} queries)` : "";
  }
}

/**
 * Update a tool call element with its result
 */
export function updateToolResult(toolElement: Element, result: string) {
  const resultSection = toolElement.querySelector('.tool-result') as HTMLElement;
  const resultContent = toolElement.querySelector('.tool-result-content');
  const toolName = toolElement.getAttribute('data-tool-name');

  if (resultSection && resultContent) {
    if (toolName === 'get_weather' || toolName === 'get_stock_price' || toolName === 'web_search') {
      try {
        const data = JSON.parse(result);
        let html = '';
        if (toolName === 'get_weather') html = renderWeatherWidget(data);
        else if (toolName === 'get_stock_price') html = renderStockWidget(data);
        else if (toolName === 'web_search') html = renderWebSearchWidget(data);

        resultContent.innerHTML = DOMPurify.sanitize(html);
      } catch (e) {
    // Fallback to plain text if JSON parsing fails
        resultContent.textContent = result;
      }
    } else {
      resultContent.textContent = result;
    }
    resultSection.style.display = 'block';
  }
}

/**
 * Weather widget renderer
 */
function renderWeatherWidget(data: any): string {
  if (!data.current) return md.utils.escapeHtml(JSON.stringify(data));
  const escape = md.utils.escapeHtml;

  let html = `<div class="weather-widget">`;
  html += `<div class="weather-header">`;
  html += `<div class="weather-location">${escape(data.location)}</div>`;
  html += `<div class="weather-current">${Math.round(data.current.temperature)}${escape(data.current.unit)}</div>`;
  html += `</div>`;

  if (data.forecast && Array.isArray(data.forecast)) {
    html += `<div class="weather-forecast">`;
    for (const day of data.forecast) {
      const date = new Date(day.date).toLocaleDateString(undefined, { weekday: 'short' });
      const emoji = getWeatherEmoji(day.weather_code);
      html += `<div class="weather-day">
                 <div class="weather-date">${date}</div>
                 <div class="weather-icon">${emoji}</div>
                 <div class="weather-temps">
                   <span class="weather-max">${Math.round(day.max_temp)}°</span>
                   <span class="weather-min">${Math.round(day.min_temp)}°</span>
                 </div>
               </div>`;
    }
    html += `</div>`;
  }
  html += `</div>`;
  return html;
}

/**
 * Stock widget renderer
 */
function renderStockWidget(data: any): string {
  if (!data.symbol) return md.utils.escapeHtml(JSON.stringify(data));
  const escape = md.utils.escapeHtml;
  const isUp = data.percent_change >= 0;
  const sign = isUp ? '+' : '';
  const colorClass = isUp ? 'stock-up' : 'stock-down';

  let svg = '';
  if (data.history && data.history.length > 1) {
    const prices = data.history.map((h: any) => h.close);
    const min = Math.min(...prices);
    const max = Math.max(...prices);
    const range = max - min || 1;
    const width = 100;
    const height = 30;

    const pts = prices.map((p: number, i: number) => {
      const x = (i / (prices.length - 1)) * width;
      const y = height - ((p - min) / range) * height;
      return `${x},${y}`;
    }).join(' ');

    // Create gradient fill underneath the sparkline
    const fillPts = `${pts} ${width},${height} 0,${height}`;

    // Smooth the stroke color a bit for darker neon pop
    const strokeColor = isUp ? '#4ade80' : '#f87171';
    const gradientId = `stock-grad-${Math.random().toString(36).substring(2, 9)}`;
    const stopColor = isUp ? 'rgba(74, 222, 128, 0.2)' : 'rgba(248, 113, 113, 0.2)';

    svg = `
      <svg viewBox="-2 -2 104 38" class="stock-sparkline" preserveAspectRatio="none">
        <defs>
          <linearGradient id="${gradientId}" x1="0" y1="0" x2="0" y2="1">
            <stop offset="0%" stop-color="${stopColor}" />
            <stop offset="100%" stop-color="transparent" />
          </linearGradient>
        </defs>
        <polygon fill="url(#${gradientId})" points="${fillPts}" />
        <polyline fill="none" class="stock-sparkline-path" stroke="${strokeColor}" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" points="${pts}" />
      </svg>
    `;
  }

  return `
    <div class="stock-widget">
      <div class="stock-header">
        <div class="stock-header-main">
          <span class="stock-symbol">${escape(data.symbol)}</span>
          <span class="stock-price">$${data.current_price.toFixed(2)}</span>
        </div>
        <div class="stock-header-sub">
          <span class="stock-change ${colorClass}">${sign}${data.percent_change.toFixed(2)}%</span>
          <span class="stock-range-label">1M History</span>
        </div>
      </div>
      ${svg ? `<div class="stock-graph-container">${svg}</div>` : ''}
    </div>
  `;
}

/**
 * Web Search widget renderer
 */
function renderWebSearchWidget(results: any[]): string {
  if (!Array.isArray(results)) return md.utils.escapeHtml(JSON.stringify(results));
  const escape = md.utils.escapeHtml;

  if (results.length === 0) {
    return `<div class="web-search-empty">No results found.</div>`;
  }

  let html = `<ul class="web-search-results">`;
  for (const r of results) {
    let urlHostname = r.url;
    try {
      urlHostname = new URL(r.url).hostname;
    } catch (e) { }

    html += `
      <li class="web-search-item">
        <a href="${escape(r.url)}" target="_blank" class="web-search-link">
          <div class="web-search-site">${escape(urlHostname)}</div>
          <div class="web-search-title">${escape(r.title)}</div>
        </a>
        <div class="web-search-snippet">${escape(r.snippet)}</div>
      </li>
    `;
  }
  html += `</ul>`;
  return html;
}

function getWeatherEmoji(code: number): string {
  if (code === 0) return '☀️';
  if (code === 1 || code === 2) return '⛅';
  if (code === 3) return '☁️';
  if (code >= 45 && code <= 48) return '🌫️';
  if (code >= 51 && code <= 67) return '🌧️';
  if (code >= 71 && code <= 77) return '❄️';
  if (code >= 80 && code <= 82) return '🌦️';
  if (code >= 85 && code <= 86) return '🌨️';
  if (code >= 95 && code <= 99) return '⛈️';
  return '❓';
}

/**
 * Copy text to clipboard and show feedback on button
 */
function copyToClipboard(text: string, button: HTMLElement) {
  navigator.clipboard.writeText(text).then(() => {
    // Show success feedback
    const originalHTML = button.innerHTML;
    button.innerHTML = CHECK_ICON;
    button.classList.add("copied");

    setTimeout(() => {
      button.innerHTML = originalHTML;
      button.classList.remove("copied");
    }, 1500);
  }).catch((err) => {
    logger.error("Failed to copy:", err);
  });
}

/**
 * Add a message to the chat area
 */
export function addMessage(
  chatArea: HTMLElement | DocumentFragment,
  role: "user" | "assistant" | "cron",
  content: string,
  images?: ImageAttachment[]
) {
  const msgDiv = document.createElement("div");
  const isCron = role === "cron";
  msgDiv.className = `message ${isCron ? "user cron-message" : role}`;

  // Render all images if present
  if (images && images.length > 0) {
    const imgContainer = document.createElement("div");
    imgContainer.className = "message-image-container";
    images.forEach((image, idx) => {
      const img = document.createElement("img");
      img.src = `data:${image.mimeType};base64,${image.base64}`;
      img.className = "message-image";
      img.alt = `Attached image ${idx + 1}`;
      imgContainer.appendChild(img);
    });
    msgDiv.appendChild(imgContainer);
  }

  const textDiv = document.createElement("div");
  textDiv.className = "message-content";

  let rawContent = content || "";

  if (role === "assistant") {
    // Render Markdown with preprocessing for KaTeX
    const rawHtml = md.render(preprocessMarkdown(content));
    textDiv.innerHTML = DOMPurify.sanitize(rawHtml);
    textDiv.classList.add("markdown-body");
  } else {
    // User messages: also render with markdown
    let textContent = content || "";

    // Check if content is JSON (Gemini format) and extract text
    try {
      if (content && content.trim().startsWith("{") && content.includes("parts")) {
        const parsed = JSON.parse(content);
        if (parsed.parts && Array.isArray(parsed.parts)) {
          const textPart = parsed.parts.find((p: { text?: string }) => p.text);
          if (textPart) {
            textContent = textPart.text;
            rawContent = textContent; // Use extracted text as raw content
          }
        }
      }
    } catch (e) {
      // Keep original content if parsing fails
    }

    // Render markdown for user messages too with preprocessing for KaTeX
    const rawHtml = md.render(preprocessMarkdown(textContent));
    if (isCron) {
      textDiv.innerHTML = `<span class="cron-label">🤖 Scheduled Task</span> ` + DOMPurify.sanitize(rawHtml);
    } else {
      textDiv.innerHTML = DOMPurify.sanitize(rawHtml);
    }
    textDiv.classList.add("markdown-body");
  }

  // Store raw markdown for copy functionality
  msgDiv.setAttribute("data-raw", rawContent);

  msgDiv.appendChild(textDiv);

  // Add copy button
  const copyBtn = document.createElement("button");
  copyBtn.className = "copy-btn";
  copyBtn.title = "Copy as Markdown";
  copyBtn.setAttribute("aria-label", "Copy as Markdown");
  copyBtn.innerHTML = COPY_ICON;
  copyBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    const raw = msgDiv.getAttribute("data-raw") || "";
    copyToClipboard(raw, copyBtn);
  });
  msgDiv.appendChild(copyBtn);

  chatArea.appendChild(msgDiv);
  if (chatArea instanceof HTMLElement) {
    chatArea.scrollTop = chatArea.scrollHeight;
  }
}

/**
 * Creates a new assistant message element for streaming, with content wrapper
 * and copy button. Used by both ambient and dedicated windows.
 */
export function createStreamingAssistantMessage(): HTMLElement {
  const msgDiv = document.createElement("div");
  msgDiv.className = "message assistant markdown-body";

  const contentDiv = document.createElement("div");
  contentDiv.className = "message-content";
  msgDiv.appendChild(contentDiv);

  const copyBtn = document.createElement("button");
  copyBtn.className = "copy-btn";
  copyBtn.title = "Copy as Markdown";
  copyBtn.setAttribute("aria-label", "Copy as Markdown");
  copyBtn.innerHTML = COPY_ICON;
  copyBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    const raw = msgDiv.getAttribute("data-raw") || "";
    copyToClipboard(raw, copyBtn);
  });
  msgDiv.appendChild(copyBtn);

  return msgDiv;
}

/**
 * Renders accumulated streaming text into sanitized HTML, handling inline
 * <think> tags from models like DeepSeek. Returns HTML ready for innerHTML.
 */
export function renderStreamingContent(rawText: string): string {
  if (rawText.includes("<think>")) {
    const openThink = rawText.indexOf("<think>");
    const closeThink = rawText.indexOf("</think>");

    if (closeThink !== -1 && closeThink > openThink) {
      const thought = rawText.substring(openThink + 7, closeThink);
      const rest = rawText.substring(closeThink + 8);
      return `
        <details class="thought-block">
          <summary>Thought</summary>
          <div class="thought-content">${DOMPurify.sanitize(thought)}</div>
        </details>
        ${DOMPurify.sanitize(md.render(preprocessMarkdown(rest)))}
      `;
    } else {
      const thought = rawText.substring(openThink + 7);
      return `
        <details class="thought-block" open>
          <summary>Thinking...</summary>
          <div class="thought-content">${DOMPurify.sanitize(thought)}</div>
        </details>
      `;
    }
  }

  return DOMPurify.sanitize(md.render(preprocessMarkdown(rawText)));
}

/**
 * Determines if an incoming streaming chunk should be skipped because it
 * would create an empty assistant bubble from leading whitespace.
 */
export function shouldSkipStreamingChunk(lastElement: Element | null, chunk: string): boolean {
  const isNewMessage = !lastElement ||
    !lastElement.classList.contains("assistant") ||
    lastElement.classList.contains("tool-output") ||
    lastElement.classList.contains("thinking-output");

  return isNewMessage && chunk.trim().length === 0;
}

/**
 * Render a proactive message with optional approve/reject buttons
 */
export function addProactiveMessage(chatArea: HTMLElement | DocumentFragment, msg: ProactiveMessage) {
  const msgDiv = document.createElement("div");
  msgDiv.className = "message proactive-message";
  msgDiv.setAttribute("data-id", msg.id);

  const headerDiv = document.createElement("div");
  headerDiv.className = "proactive-header";

  const titleContainer = document.createElement("div");
  titleContainer.style.display = "flex";
  titleContainer.style.alignItems = "center";
  titleContainer.style.gap = "8px";
  
  const icon = msg.needs_approval ? "⚡" : "🤖";
  const title = msg.needs_approval ? "Proactive Action Required" : "Scheduled Task";
  titleContainer.innerHTML = `<span>${icon}</span><span style="letter-spacing: 0.5px; text-transform: uppercase; font-size: 11px;">${title}</span>`;
  
  headerDiv.appendChild(titleContainer);

  if (!msg.needs_approval && !msg.reviewed_at) {
    const dismissBtn = document.createElement("button");
    dismissBtn.className = "proactive-dismiss-btn";
    dismissBtn.title = "Mark as read";
    dismissBtn.setAttribute("aria-label", "Mark as read");
    dismissBtn.innerHTML = CHECK_ICON;

    dismissBtn.addEventListener("click", async () => {
      try {
        await invoke("review_proactive_message", { messageId: msg.id });
        msgDiv.style.transition = "opacity 0.2s ease, transform 0.2s ease";
        msgDiv.style.opacity = "0";
        msgDiv.style.transform = "scale(0.98)";
        setTimeout(() => msgDiv.remove(), 200);
      } catch (e) {
        logger.error("Failed to mark as read:", e);
      }
    });
    headerDiv.appendChild(dismissBtn);
  }

  msgDiv.appendChild(headerDiv);

  const contentDiv = document.createElement("div");
  contentDiv.className = "proactive-content markdown-body";
  contentDiv.innerHTML = DOMPurify.sanitize(md.render(preprocessMarkdown(msg.content)));
  msgDiv.appendChild(contentDiv);

  // If there's a draft payload, show it as a tool call block
  if (msg.draft_payload) {
    let fnName = "Unknown Action";
    let formattedArgs = msg.draft_payload;
    try {
      const parsed = JSON.parse(msg.draft_payload);
      if (parsed.name) fnName = parsed.name;
      if (parsed.arguments) formattedArgs = JSON.stringify(parsed.arguments, null, 2);
    } catch(e) {}

    const draftDiv = document.createElement("div");
    draftDiv.className = "tool-output";
    draftDiv.innerHTML = `
      <details>
        <summary>
          <span class="tool-icon">🛠️</span>
          <span class="tool-name">Draft Action: ${md.utils.escapeHtml(fnName)}</span>
        </summary>
        <div class="tool-args">${md.utils.escapeHtml(formattedArgs)}</div>
      </details>
    `;
    msgDiv.appendChild(draftDiv);
  }

  // Handle Approve / Reject Actions based on state
  if (msg.needs_approval && !msg.reviewed_at) {
    const actionsDiv = document.createElement("div");
    actionsDiv.className = "proactive-actions";

    const approveBtn = document.createElement("button");
    approveBtn.className = "proactive-btn approve";
    approveBtn.innerHTML = `${CHECK_ICON} Approve`;
    
    const rejectBtn = document.createElement("button");
    rejectBtn.className = "proactive-btn reject";
    rejectBtn.innerHTML = `${CROSS_ICON} Reject`;

    approveBtn.addEventListener("click", async () => {
      approveBtn.disabled = true;
      rejectBtn.disabled = true;
      try {
        await invoke("approve_draft", { messageId: msg.id });
        actionsDiv.innerHTML = `<div class="proactive-status">${CHECK_ICON} Approved</div>`;
      } catch (e) {
        logger.error("Failed to approve:", e);
        approveBtn.disabled = false;
        rejectBtn.disabled = false;
      }
    });

    rejectBtn.addEventListener("click", async () => {
      approveBtn.disabled = true;
      rejectBtn.disabled = true;
      try {
        await invoke("reject_draft", { messageId: msg.id });
        actionsDiv.innerHTML = `<div class="proactive-status" style="color: #f87171;">${CROSS_ICON} Rejected</div>`;
      } catch (e) {
        logger.error("Failed to reject:", e);
        approveBtn.disabled = false;
        rejectBtn.disabled = false;
      }
    });

    actionsDiv.appendChild(approveBtn);
    actionsDiv.appendChild(rejectBtn);
    msgDiv.appendChild(actionsDiv);
  } else if (msg.reviewed_at) {
    // Already reviewed
    const actionsDiv = document.createElement("div");
    actionsDiv.className = "proactive-actions";
    if (msg.approved === true) {
      actionsDiv.innerHTML = `<div class="proactive-status">${CHECK_ICON} Approved on ${new Date(msg.reviewed_at).toLocaleString()}</div>`;
    } else if (msg.approved === false) {
      actionsDiv.innerHTML = `<div class="proactive-status" style="color: #f87171;">${CROSS_ICON} Rejected on ${new Date(msg.reviewed_at).toLocaleString()}</div>`;
    } else {
      actionsDiv.innerHTML = `<div class="proactive-status">Dismissed</div>`;
    }
    msgDiv.appendChild(actionsDiv);
  }

  chatArea.appendChild(msgDiv);
  if (chatArea instanceof HTMLElement) {
    chatArea.scrollTop = chatArea.scrollHeight;
  }
}

