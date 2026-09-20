import { beforeEach, describe, expect, it, vi } from "vitest";
import { closeAmbientView, isAmbientViewOpen, mountAmbientView } from "../ui/ambient-view";

describe("ambient capability view host", () => {
  beforeEach(() => {
    document.body.innerHTML = `
      <section class="ambient-surface">
        <section id="ambient-view" hidden>
          <button id="ambient-view-close"></button>
          <h2 id="ambient-view-title"></h2>
          <div id="ambient-view-content"></div>
        </section>
        <textarea id="input-field"></textarea>
      </section>`;
  });

  it("mounts one view without replacing the composer", () => {
    mountAmbientView({
      title: "Memory",
      render: (host) => {
        host.textContent = "Connected memory";
      },
    });

    expect(isAmbientViewOpen()).toBe(true);
    expect(document.getElementById("ambient-view-title")?.textContent).toBe("Memory");
    expect(document.getElementById("ambient-view-content")?.textContent).toBe("Connected memory");
    expect(document.getElementById("input-field")).not.toBeNull();
    expect(document.querySelector(".ambient-surface")?.classList.contains("has-ambient-view")).toBe(true);
  });

  it("cleans up the previous view and returns focus to the composer", () => {
    const cleanup = vi.fn();
    mountAmbientView({ title: "Routines", render: () => cleanup });

    expect(closeAmbientView()).toBe(true);
    expect(cleanup).toHaveBeenCalledOnce();
    expect(isAmbientViewOpen()).toBe(false);
    expect(document.activeElement?.id).toBe("input-field");
    expect(closeAmbientView()).toBe(false);
  });

  it("cleans up before mounting a replacement", () => {
    const cleanup = vi.fn();
    mountAmbientView({ title: "Memory", render: () => cleanup });
    mountAmbientView({ title: "Routines", render: (host) => { host.textContent = "Routine"; } });

    expect(cleanup).toHaveBeenCalledOnce();
    expect(document.getElementById("ambient-view-title")?.textContent).toBe("Routines");
  });
});
