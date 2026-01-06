/**
 * Message rendering utilities for Shard chat
 */
import DOMPurify from "dompurify";
import { md } from "./markdown";
import { COPY_ICON, CHECK_ICON } from "./icons";
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
    console.error("Failed to copy:", err);
  });
}

/**
 * Add a message to the chat area
 */
export function addMessage(
  chatArea: HTMLElement,
  role: "user" | "assistant",
  content: string,
  images?: ImageAttachment[]
) {
  const msgDiv = document.createElement("div");
  msgDiv.className = `message ${role}`;

  // Render all images if present
  if (images && images.length > 0) {
    const imgContainer = document.createElement("div");
    imgContainer.className = "message-image-container";
    for (const image of images) {
      const img = document.createElement("img");
      img.src = `data:${image.mimeType};base64,${image.base64}`;
      img.className = "message-image";
      imgContainer.appendChild(img);
    }
    msgDiv.appendChild(imgContainer);
  }

  const textDiv = document.createElement("div");
  textDiv.className = "message-content";

  let rawContent = content || "";

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
            rawContent = textContent; // Use extracted text as raw content
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

  // Store raw markdown for copy functionality
  msgDiv.setAttribute("data-raw", rawContent);

  msgDiv.appendChild(textDiv);

  // Add copy button
  const copyBtn = document.createElement("button");
  copyBtn.className = "copy-btn";
  copyBtn.title = "Copy as Markdown";
  copyBtn.innerHTML = COPY_ICON;
  copyBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    const raw = msgDiv.getAttribute("data-raw") || "";
    copyToClipboard(raw, copyBtn);
  });
  msgDiv.appendChild(copyBtn);

  chatArea.appendChild(msgDiv);
  chatArea.scrollTop = chatArea.scrollHeight;
}

