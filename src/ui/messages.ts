/**
 * Message rendering utilities for Shard chat
 */
import DOMPurify from "dompurify";
import { md } from "./markdown";
import type { ImageAttachment } from "../types";

/**
 * Create a thinking/reasoning block element
 */
export function createThinkingElement(content: string, isComplete: boolean = true): HTMLElement {
  const thinkingMsg = document.createElement("div");
  thinkingMsg.className = "message thinking-output";
  thinkingMsg.setAttribute('data-complete', isComplete ? 'true' : 'false');
  thinkingMsg.setAttribute("data-thinking", content);

  const trimmedThinking = content.trimEnd();
  const summaryText = isComplete ? "Thought" : "Thinking...";
  const openAttr = isComplete ? "" : "open";

  thinkingMsg.innerHTML = `
    <details class="thought-block" ${openAttr}>
      <summary>${summaryText}</summary>
      <div class="thought-content markdown-body">${DOMPurify.sanitize(md.render(trimmedThinking))}</div>
    </details>
  `;
  return thinkingMsg;
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

  const argsPretty = JSON.stringify(argsObj, null, 2);
  const summaryArgs = Object.entries(argsObj)
    .map(([k, v]) => `${k}="${v}"`)
    .join(" ");

  const openAttr = isOpen ? "open" : "";

  toolDiv.innerHTML = `
    <details ${openAttr}>
      <summary>
        <span class="tool-icon">🛠️</span>
        <span class="tool-name">Tool: ${name}</span>
        <span class="tool-summary-args">${summaryArgs}</span>
      </summary>
      <div class="tool-args">${argsPretty}</div>
      <div class="tool-result" style="display: none;">
        <div class="tool-result-label">Result:</div>
        <div class="tool-result-content"></div>
      </div>
    </details>
  `;
  return toolDiv;
}

/**
 * Update a tool call element with its result
 */
export function updateToolResult(toolElement: Element, result: string) {
  const resultSection = toolElement.querySelector('.tool-result') as HTMLElement;
  const resultContent = toolElement.querySelector('.tool-result-content');
  if (resultSection && resultContent) {
    resultContent.textContent = result;
    resultSection.style.display = 'block';
  }
}

/**
 * Add a message to the chat area
 */
export function addMessage(
  chatArea: HTMLElement,
  role: "user" | "assistant",
  content: string,
  image?: ImageAttachment
) {
  const msgDiv = document.createElement("div");
  msgDiv.className = `message ${role}`;

  // Render Image if present
  if (image) {
    const imgContainer = document.createElement("div");
    imgContainer.className = "message-image-container";
    const img = document.createElement("img");
    img.src = `data:${image.mimeType};base64,${image.base64}`;
    img.className = "message-image";
    imgContainer.appendChild(img);
    msgDiv.appendChild(imgContainer);
  }

  const textDiv = document.createElement("div");
  textDiv.className = "message-content";

  if (role === "assistant") {
    // Render Markdown
    const rawHtml = md.render(content);
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
          }
        }
      }
    } catch (e) {
      // Keep original content if parsing fails
    }

    // Render markdown for user messages too
    const rawHtml = md.render(textContent);
    textDiv.innerHTML = DOMPurify.sanitize(rawHtml);
    textDiv.classList.add("markdown-body");
  }

  msgDiv.appendChild(textDiv);
  chatArea.appendChild(msgDiv);
  chatArea.scrollTop = chatArea.scrollHeight;
}
