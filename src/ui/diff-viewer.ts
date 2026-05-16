/**
 * Streaming multi-file diff viewer.
 *
 * Subscribes to the backend's `file-edited` event (see `events.ts` /
 * `self_files.rs`) and renders each edit into a horizontal-tabbed panel. Each
 * panel shows a colored unified diff that streams in line-by-line so the user
 * watches the change appear, matching the feel of an editor live-applying a
 * patch.
 *
 * Library-free by design (the user explicitly said trees.software is optional)
 * — just native DOM + CSS, ~150 LOC. If multi-file editing graduates beyond
 * a flat tab strip, swap the tab bar for a tree without touching consumers.
 *
 * Mount once per window (ambient overlay + dedicated chat both call
 * `mountDiffViewer`).
 */
import DOMPurify from "dompurify";

export interface EditOutcome {
  path: string;
  abs_path: string;
  before: string;
  after: string;
  unified_diff: string;
  replacements: number;
}

export interface DiffViewerController {
  /** Add a new edit (or replace the existing one for the same path). */
  addOrUpdate(outcome: EditOutcome): void;
  /** Remove all tabs and hide the panel. */
  clear(): void;
  /** Show or hide the whole panel. */
  setVisible(visible: boolean): void;
}

interface TabState {
  outcome: EditOutcome;
  tab: HTMLButtonElement;
  panel: HTMLElement;
}

// Stream ~120 lines/sec; smooth without feeling sluggish.
const STREAM_INTERVAL_MS = 8;

function classifyLine(line: string): "hunk" | "add" | "del" | "ctx" | "meta" {
  if (line.startsWith("@@")) return "hunk";
  if (line.startsWith("+++") || line.startsWith("---")) return "meta";
  if (line.startsWith("+")) return "add";
  if (line.startsWith("-")) return "del";
  return "ctx";
}

function computeStats(diff: string): { adds: number; dels: number } {
  let adds = 0,
    dels = 0;
  for (const raw of diff.split("\n")) {
    if (raw.startsWith("+") && !raw.startsWith("+++")) adds++;
    else if (raw.startsWith("-") && !raw.startsWith("---")) dels++;
  }
  return { adds, dels };
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;");
}

/**
 * Stream the diff lines into `target`, replacing any prior content. Returns
 * a cancellation function so a subsequent stream for the same target can
 * preempt the in-flight animation.
 */
function streamDiffInto(target: HTMLElement, diff: string): () => void {
  target.replaceChildren();
  const lines = diff.split("\n");
  let i = 0;
  let timer: number | null = null;
  const tick = () => {
    // Batch a few lines per frame so very large diffs don't feel sluggish.
    const batchSize = Math.max(1, Math.ceil(lines.length / 200));
    for (let b = 0; b < batchSize && i < lines.length; b++, i++) {
      const cls = classifyLine(lines[i]);
      const div = document.createElement("div");
      div.className = `diff-line diff-${cls}`;
      // textContent preserves whitespace; safer than innerHTML for streaming.
      div.textContent = lines[i] || "\u00A0";
      target.appendChild(div);
    }
    if (i < lines.length) {
      timer = window.setTimeout(tick, STREAM_INTERVAL_MS) as unknown as number;
    } else {
      target.classList.add("diff-stream-done");
    }
  };
  target.classList.remove("diff-stream-done");
  tick();
  return () => {
    if (timer !== null) window.clearTimeout(timer);
  };
}

export function mountDiffViewer(host: HTMLElement): DiffViewerController {
  // Single shared root inside the host. Hidden until the first edit arrives.
  const root = document.createElement("div");
  root.className = "diff-viewer";
  root.hidden = true;
  root.innerHTML = DOMPurify.sanitize(`
    <div class="diff-viewer-header">
      <div class="diff-viewer-title">Recent edits</div>
      <div class="diff-viewer-tabs" role="tablist"></div>
      <button class="diff-viewer-close" type="button" aria-label="Close diff viewer">×</button>
    </div>
    <div class="diff-viewer-body"></div>
  `);
  host.appendChild(root);

  const tabsEl = root.querySelector(".diff-viewer-tabs") as HTMLElement;
  const bodyEl = root.querySelector(".diff-viewer-body") as HTMLElement;
  const closeBtn = root.querySelector(".diff-viewer-close") as HTMLButtonElement;
  const tabs = new Map<string, TabState>();
  // Track in-flight stream cancellations per path.
  const cancellers = new Map<string, () => void>();

  const setActive = (path: string) => {
    tabs.forEach((state, p) => {
      const active = p === path;
      state.tab.classList.toggle("active", active);
      state.tab.setAttribute("aria-selected", String(active));
      state.panel.hidden = !active;
    });
  };

  closeBtn.addEventListener("click", () => {
    root.hidden = true;
  });

  const addOrUpdate = (outcome: EditOutcome) => {
    root.hidden = false;
    const { path } = outcome;
    let state = tabs.get(path);
    if (!state) {
      const tab = document.createElement("button");
      tab.type = "button";
      tab.className = "diff-viewer-tab";
      tab.setAttribute("role", "tab");
      tab.dataset.path = path;
      tab.addEventListener("click", () => setActive(path));

      const panel = document.createElement("div");
      panel.className = "diff-viewer-panel";
      panel.setAttribute("role", "tabpanel");
      panel.dataset.path = path;
      panel.innerHTML = DOMPurify.sanitize(`
        <div class="diff-viewer-pathbar">
          <span class="diff-viewer-path"></span>
          <span class="diff-viewer-abs"></span>
        </div>
        <pre class="diff-viewer-lines"></pre>
      `);

      tabsEl.appendChild(tab);
      bodyEl.appendChild(panel);
      state = { outcome, tab, panel };
      tabs.set(path, state);
    } else {
      state.outcome = outcome;
    }

    // Update tab label + stats.
    const { adds, dels } = computeStats(outcome.unified_diff);
    state.tab.innerHTML = DOMPurify.sanitize(`
      <span class="diff-viewer-tab-path">${escapeHtml(path)}</span>
      <span class="diff-viewer-tab-stats">
        <span class="diff-add-count">+${adds}</span>
        <span class="diff-del-count">-${dels}</span>
      </span>
    `);

    // Update path bar + start streaming the diff.
    const pathEl = state.panel.querySelector(".diff-viewer-path") as HTMLElement;
    const absEl = state.panel.querySelector(".diff-viewer-abs") as HTMLElement;
    const linesEl = state.panel.querySelector(".diff-viewer-lines") as HTMLElement;
    pathEl.textContent = path;
    absEl.textContent = outcome.abs_path;

    // Cancel any in-flight stream for this path before starting a new one.
    cancellers.get(path)?.();
    const cancel = streamDiffInto(linesEl, outcome.unified_diff);
    cancellers.set(path, cancel);

    // Latest edit auto-activates.
    setActive(path);
  };

  const clear = () => {
    cancellers.forEach((c) => c());
    cancellers.clear();
    tabs.clear();
    tabsEl.replaceChildren();
    bodyEl.replaceChildren();
    root.hidden = true;
  };

  const setVisible = (visible: boolean) => {
    root.hidden = !visible;
  };

  return { addOrUpdate, clear, setVisible };
}
