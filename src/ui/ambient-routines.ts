import { invoke } from "@tauri-apps/api/core";
import "./ambient-capabilities.css";

export interface RoutineStatus {
  filename: string;
  schedule: string;
  cron: string;
  session: string;
  persona: string | null;
  max_tool_calls: number;
  max_runs_per_day: number | null;
  paused: boolean;
  prompt: string;
  prompt_preview: string;
  revision: string;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function field(labelText: string, value: string, name: string, multiline = false) {
  const label = document.createElement("label");
  label.textContent = labelText;
  const control = multiline ? document.createElement("textarea") : document.createElement("input");
  control.name = name;
  control.value = value;
  if (control instanceof HTMLTextAreaElement) control.rows = 3;
  label.append(control);
  return { label, control };
}

export function renderRoutinesView(host: HTMLElement): () => void {
  const events = new AbortController();
  let routines: RoutineStatus[] = [];

  host.classList.add("ambient-capability");
  host.replaceChildren();
  const status = document.createElement("p");
  status.className = "ambient-capability-status";
  status.setAttribute("role", "status");
  const list = document.createElement("div");
  list.className = "ambient-capability-list";
  list.setAttribute("aria-busy", "true");
  list.textContent = "Loading routines…";
  const createButton = document.createElement("button");
  createButton.type = "button";
  createButton.textContent = "New routine";
  const note = document.createElement("p");
  note.className = "ambient-capability-note";
  note.textContent =
    "Pause prevents future runs and survives restart. A run already started may still finish.";
  host.append(status, list, createButton, note);

  const setStatus = (message: string, error = false) => {
    status.textContent = message;
    status.dataset.tone = error ? "error" : "quiet";
  };

  const renderForm = (routine?: RoutineStatus) => {
    list.replaceChildren();
    createButton.hidden = true;
    const form = document.createElement("form");
    form.className = "ambient-capability-fields";
    const name = field(
      "Name (lowercase letters, numbers, hyphens)",
      routine?.filename ?? "",
      "name",
    );
    const schedule = field("Cron schedule", routine?.cron ?? "0 9 * * MON", "schedule");
    const prompt = field("What Shard should do", routine?.prompt ?? "", "prompt", true);
    const session = field(
      "Session namespace",
      routine?.session ?? "",
      "session",
    );
    if (name.control instanceof HTMLInputElement) {
      name.control.pattern = "[a-z][a-z0-9-]{1,40}";
    }
    if (!routine) {
      name.control.addEventListener(
        "input",
        () => {
          const slug = name.control.value
            .toLocaleLowerCase()
            .replace(/[^a-z0-9-]/g, "-")
            .replace(/^-+|-+$/g, "");
          session.control.value = slug ? `agent:${slug}` : "";
        },
        { signal: events.signal },
      );
    }
    if (routine) name.control.disabled = true;
    const actions = document.createElement("div");
    actions.className = "ambient-capability-actions";
    const save = document.createElement("button");
    save.type = "submit";
    save.dataset.primary = "true";
    save.textContent = routine ? "Save routine" : "Create routine";
    const cancel = document.createElement("button");
    cancel.type = "button";
    cancel.textContent = "Cancel";
    actions.append(save, cancel);
    form.append(name.label, schedule.label, prompt.label, session.label, actions);
    list.append(form);
    name.control.focus();

    cancel.addEventListener("click", () => renderList(), { signal: events.signal });
    form.addEventListener(
      "submit",
      async (event) => {
        event.preventDefault();
        const values = {
          name: name.control.value.trim(),
          schedule: schedule.control.value.trim(),
          prompt: prompt.control.value.trim(),
          session: session.control.value.trim(),
        };
        if (Object.values(values).some((value) => !value)) {
          setStatus("Name, schedule, session, and prompt are required.", true);
          return;
        }
        save.disabled = true;
        try {
          const command = routine ? "update_heartbeat" : "create_heartbeat";
          const updated = await invoke<RoutineStatus>(command, {
            name: values.name,
            expectedRevision: routine?.revision,
            input: {
              schedule: values.schedule,
              session: values.session,
              persona: routine?.persona ?? null,
              max_tool_calls: routine?.max_tool_calls ?? 5,
              max_runs_per_day: routine?.max_runs_per_day ?? 10,
              prompt: values.prompt,
            },
          });
          if (routine) {
            routines = routines.map((item) =>
              item.filename === updated.filename ? updated : item,
            );
          } else {
            routines.push(updated);
          }
          setStatus(routine ? "Routine saved." : "Routine created.");
          renderList();
        } catch (error) {
          setStatus(`Routine was not saved: ${errorMessage(error)}`, true);
          save.disabled = false;
        }
      },
      { signal: events.signal },
    );
  };

  const renderList = () => {
    list.replaceChildren();
    list.setAttribute("aria-busy", "false");
    createButton.hidden = false;
    if (routines.length === 0) {
      const empty = document.createElement("p");
      empty.className = "ambient-capability-empty";
      empty.textContent = "No routines configured.";
      list.append(empty);
      return;
    }

    for (const routine of routines) {
      const row = document.createElement("article");
      row.className = "ambient-capability-row";
      row.dataset.routineName = routine.filename;
      const meta = document.createElement("p");
      meta.className = "ambient-capability-meta";
      meta.textContent = `${routine.paused ? "paused" : "active"} · ${routine.paused ? routine.cron : routine.schedule}`;
      const title = document.createElement("strong");
      title.textContent = routine.filename;
      const details = document.createElement("details");
      details.className = "ambient-routine-details";
      const summary = document.createElement("summary");
      summary.append(title, meta);
      const copy = document.createElement("p");
      copy.className = "ambient-capability-copy";
      copy.textContent = routine.prompt;
      details.append(summary, copy);
      const actions = document.createElement("div");
      actions.className = "ambient-capability-actions";
      const pause = document.createElement("button");
      pause.type = "button";
      pause.textContent = routine.paused ? "Resume" : "Pause";
      const edit = document.createElement("button");
      edit.type = "button";
      edit.textContent = "Edit";
      const remove = document.createElement("button");
      remove.type = "button";
      remove.dataset.danger = "true";
      remove.textContent = "Delete";
      actions.append(pause, edit, remove);
      row.append(details, actions);
      list.append(row);

      pause.addEventListener(
        "click",
        async () => {
          pause.disabled = true;
          try {
            const updated = await invoke<RoutineStatus>("set_heartbeat_paused", {
              name: routine.filename,
              expectedRevision: routine.revision,
              paused: !routine.paused,
            });
            routines = routines.map((item) =>
              item.filename === updated.filename ? updated : item,
            );
            setStatus(
              updated.paused
                ? "Routine paused. Work already running was not cancelled."
                : "Routine resumed.",
            );
            renderList();
          } catch (error) {
            setStatus(`Routine status was not changed: ${errorMessage(error)}`, true);
            pause.disabled = false;
          }
        },
        { signal: events.signal },
      );
      edit.addEventListener("click", () => renderForm(routine), { signal: events.signal });
      remove.addEventListener(
        "click",
        async () => {
          if (remove.dataset.confirm !== "true") {
            remove.dataset.confirm = "true";
            remove.textContent = "Confirm delete";
            return;
          }
          remove.disabled = true;
          try {
            await invoke("delete_heartbeat", {
              name: routine.filename,
              expectedRevision: routine.revision,
            });
            routines = routines.filter((item) => item.filename !== routine.filename);
            setStatus("Routine deleted. Work already running was not cancelled.");
            renderList();
          } catch (error) {
            setStatus(`Routine was not deleted: ${errorMessage(error)}`, true);
            remove.disabled = false;
          }
        },
        { signal: events.signal },
      );
    }
  };

  createButton.addEventListener("click", () => renderForm(), { signal: events.signal });
  void invoke<RoutineStatus[]>("get_heartbeat_status")
    .then((result) => {
      routines = result;
      renderList();
    })
    .catch((error) => {
      list.setAttribute("aria-busy", "false");
      list.textContent = "Routines could not be loaded.";
      setStatus(errorMessage(error), true);
    });

  return () => {
    events.abort();
    host.classList.remove("ambient-capability");
  };
}
