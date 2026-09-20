export interface AmbientViewOptions {
  title: string;
  render: (host: HTMLElement) => void | (() => void);
}

let cleanup: (() => void) | undefined;

function elements() {
  return {
    view: document.getElementById("ambient-view") as HTMLElement | null,
    title: document.getElementById("ambient-view-title") as HTMLElement | null,
    content: document.getElementById("ambient-view-content") as HTMLElement | null,
    close: document.getElementById("ambient-view-close") as HTMLButtonElement | null,
  };
}

/** Mount one connected capability view without replacing the conversation or composer. */
export function mountAmbientView({ title, render }: AmbientViewOptions): void {
  closeAmbientView(false);
  const parts = elements();
  if (!parts.view || !parts.title || !parts.content) {
    throw new Error("Ambient view host is unavailable");
  }

  parts.title.textContent = title;
  parts.content.replaceChildren();
  cleanup = render(parts.content) || undefined;
  parts.view.hidden = false;
  parts.view.closest(".ambient-surface")?.classList.add("has-ambient-view");
  parts.close?.focus();
}

/** Close the mounted view and return to the unchanged composer. */
export function closeAmbientView(restoreFocus = true): boolean {
  const parts = elements();
  if (!parts.view || parts.view.hidden) return false;

  cleanup?.();
  cleanup = undefined;
  parts.content?.replaceChildren();
  parts.view.hidden = true;
  parts.view.closest(".ambient-surface")?.classList.remove("has-ambient-view");
  if (restoreFocus) {
    (document.getElementById("input-field") as HTMLTextAreaElement | null)?.focus();
  }
  return true;
}

export function isAmbientViewOpen(): boolean {
  const view = elements().view;
  return Boolean(view && !view.hidden);
}
