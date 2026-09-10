import { beforeEach, describe, expect, it, vi } from "vitest";
import { invoke } from "@tauri-apps/api/core";
import { renderMemoryView } from "../ui/ambient-memory";
import { renderRoutinesView } from "../ui/ambient-routines";
import { renderHistoryView } from "../ui/ambient-history";

vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn() }));
const mockedInvoke = vi.mocked(invoke);

const memory = {
  id: "memory-1",
  category: "preference" as const,
  content: "Quiet places",
  created_at: "2026-09-08T00:00:00Z",
  importance: 4,
  revision: 1,
};

const routine = {
  filename: "sunday-space",
  schedule: "Sunday at 09:00 AM",
  cron: "0 9 * * SUN",
  session: "agent:sunday-space",
  persona: null,
  max_tool_calls: 3,
  max_runs_per_day: 1,
  paused: false,
  prompt: "Leave an hour open for a walk.",
  prompt_preview: "Leave an hour open for a walk.",
  revision: "revision-1",
};

async function settle() {
  await Promise.resolve();
  await Promise.resolve();
}

describe("ambient capability views", () => {
  beforeEach(() => {
    document.body.innerHTML = '<div id="host"></div>';
    mockedInvoke.mockReset();
  });

  it("keeps memory status and Undo in one reserved row and waits for persisted correction", async () => {
    mockedInvoke.mockResolvedValueOnce([memory]);
    const host = document.querySelector<HTMLElement>("#host")!;
    renderMemoryView(host);
    await settle();

    const status = host.querySelector<HTMLElement>(".ambient-capability-status")!;
    expect(status.querySelector("button")?.hidden).toBe(true);
    host.querySelector<HTMLButtonElement>("article button")!.click();
    const textarea = host.querySelector<HTMLTextAreaElement>("textarea")!;
    textarea.value = "Slow, quiet places";
    let resolveCorrection!: (value: unknown) => void;
    mockedInvoke.mockReturnValueOnce(new Promise((resolve) => (resolveCorrection = resolve)));
    host.querySelector<HTMLButtonElement>('button[type="submit"], button[data-primary="true"]')!.click();
    expect(status.textContent).not.toContain("saved");

    resolveCorrection({
      memory: { ...memory, content: "Slow, quiet places", revision: 2 },
      undo_token: { kind: "correction" },
    });
    await settle();
    expect(status.textContent).toContain("Correction saved");
    expect(status.querySelector("button")?.hidden).toBe(false);
    expect(host.textContent).toContain("Slow, quiet places");
  });

  it("shows a real routine pause only after IPC confirms it", async () => {
    mockedInvoke.mockResolvedValueOnce([routine]);
    const host = document.querySelector<HTMLElement>("#host")!;
    renderRoutinesView(host);
    await settle();
    const details = host.querySelector<HTMLDetailsElement>(".ambient-routine-details")!;
    expect(details.open).toBe(false);
    expect(details.querySelector("summary")?.textContent).toContain("Sunday at 09:00 AM");
    expect(details.querySelector("summary")?.textContent).not.toContain(routine.prompt);
    expect(details.textContent).toContain(routine.prompt);
    const pause = [...host.querySelectorAll("button")].find((button) => button.textContent === "Pause")!;
    let resolvePause!: (value: unknown) => void;
    mockedInvoke.mockReturnValueOnce(new Promise((resolve) => (resolvePause = resolve)));
    pause.click();
    expect(host.textContent).toContain("active");
    resolvePause({ ...routine, paused: true, revision: "revision-2" });
    await settle();
    expect(host.textContent).toContain("paused");
    expect(host.textContent).toContain("already running was not cancelled");
  });

  it("creates routines through the structured host input contract", async () => {
    mockedInvoke.mockResolvedValueOnce([]);
    const host = document.querySelector<HTMLElement>("#host")!;
    renderRoutinesView(host);
    await settle();
    [...host.querySelectorAll("button")]
      .find((button) => button.textContent === "New routine")!
      .click();
    host.querySelector<HTMLInputElement>('[name="name"]')!.value = "morning-review";
    host.querySelector<HTMLInputElement>('[name="schedule"]')!.value = "0 9 * * MON";
    host.querySelector<HTMLInputElement>('[name="session"]')!.value = "agent:morning-review";
    host.querySelector<HTMLTextAreaElement>('[name="prompt"]')!.value = "Review the week.";
    mockedInvoke.mockResolvedValueOnce({ ...routine, filename: "morning-review" });
    host.querySelector<HTMLFormElement>("form")!.dispatchEvent(
      new Event("submit", { bubbles: true, cancelable: true }),
    );
    await settle();
    expect(mockedInvoke).toHaveBeenLastCalledWith("create_heartbeat", {
      name: "morning-review",
      expectedRevision: undefined,
      input: {
        schedule: "0 9 * * MON",
        session: "agent:morning-review",
        persona: null,
        max_tool_calls: 5,
        max_runs_per_day: 10,
        prompt: "Review the week.",
      },
    });
  });

  it("filters history and delegates safe session switching to the shell", async () => {
    vi.useFakeTimers();
    mockedInvoke.mockResolvedValueOnce(
      JSON.stringify([
        { session_id: "one", title: "Trip", summary: "A slower weekend", created_at: "", updated_at: "" },
        { session_id: "two", title: "Garden", summary: "Tomatoes", created_at: "", updated_at: "" },
      ]),
    );
    const load = vi.fn().mockResolvedValue(undefined);
    const remove = vi.fn().mockResolvedValue(undefined);
    const host = document.querySelector<HTMLElement>("#host")!;
    renderHistoryView(host, load, remove);
    await settle();
    const search = host.querySelector<HTMLInputElement>('input[type="search"]')!;
    search.value = "garden";
    search.dispatchEvent(new Event("input"));
    expect(host.textContent).not.toContain("Trip");
    mockedInvoke.mockResolvedValueOnce(
      JSON.stringify([
        { session_id: "two", title: "Garden", summary: "Tomatoes", date: "" },
      ]),
    );
    await vi.advanceTimersByTimeAsync(180);
    expect(mockedInvoke).toHaveBeenLastCalledWith("get_recent_sessions", {
      limit: 50,
      query: "garden",
    });
    host.querySelector<HTMLButtonElement>(".ambient-capability-session")!.click();
    await settle();
    expect(load).toHaveBeenCalledWith("two");
    vi.useRealTimers();
  });

  it("renders backend errors without fabricated success", async () => {
    mockedInvoke.mockRejectedValueOnce("database unavailable");
    const host = document.querySelector<HTMLElement>("#host")!;
    renderMemoryView(host);
    await settle();
    expect(host.textContent).toContain("could not be loaded");
    expect(host.textContent).toContain("database unavailable");
    expect(host.querySelector("[role='status']")?.textContent).not.toContain("Correction saved");
  });
});
