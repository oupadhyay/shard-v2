import { invoke } from "@tauri-apps/api/core";
import "./ambient-capabilities.css";

type MemoryCategory = "preference" | "project" | "interaction" | "fact";

export interface SavedMemory {
  id: string;
  category: MemoryCategory;
  content: string;
  created_at: string;
  importance: number;
  revision: number;
}

type UndoToken = Record<string, unknown>;

interface MemoryMutationResult {
  memory: SavedMemory | null;
  undo_token: UndoToken;
}

function errorMessage(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function renderMemoryView(host: HTMLElement): () => void {
  const events = new AbortController();
  let memories: SavedMemory[] = [];
  let undoToken: UndoToken | null = null;

  host.classList.add("ambient-capability");
  host.replaceChildren();
  const intro = document.createElement("p");
  intro.className = "ambient-capability-intro";
  intro.textContent =
    "Explicit memories Shard carries into future conversations. Inferred observations and conversation history are separate.";
  const status = document.createElement("div");
  status.className = "ambient-capability-status";
  status.setAttribute("role", "status");
  const statusText = document.createElement("span");
  const undo = document.createElement("button");
  undo.type = "button";
  undo.textContent = "Undo";
  undo.hidden = true;
  status.append(statusText, undo);
  const list = document.createElement("div");
  list.className = "ambient-capability-list";
  list.setAttribute("aria-busy", "true");
  list.textContent = "Loading saved memories…";
  const note = document.createElement("p");
  note.className = "ambient-capability-note";
  note.textContent = "Forgetting a saved memory does not delete the conversation it came from.";
  host.append(intro, status, list, note);

  const setStatus = (message: string, tone: "quiet" | "error" = "quiet") => {
    statusText.textContent = message;
    status.dataset.tone = tone;
    undo.hidden = !undoToken;
  };

  const renderList = () => {
    list.replaceChildren();
    list.setAttribute("aria-busy", "false");
    if (memories.length === 0) {
      const empty = document.createElement("p");
      empty.className = "ambient-capability-empty";
      empty.textContent = "No explicit saved memories yet.";
      list.append(empty);
      return;
    }

    for (const memory of memories) {
      const row = document.createElement("article");
      row.className = "ambient-capability-row";
      row.dataset.memoryId = memory.id;
      const meta = document.createElement("p");
      meta.className = "ambient-capability-meta";
      meta.textContent = `${memory.category} · importance ${memory.importance}`;
      const copy = document.createElement("p");
      copy.className = "ambient-capability-copy";
      copy.textContent = memory.content;
      const actions = document.createElement("div");
      actions.className = "ambient-capability-actions";
      const edit = document.createElement("button");
      edit.type = "button";
      edit.textContent = "Correct";
      const forget = document.createElement("button");
      forget.type = "button";
      forget.dataset.danger = "true";
      forget.textContent = "Forget";
      actions.append(edit, forget);
      row.append(meta, copy, actions);
      list.append(row);

      edit.addEventListener(
        "click",
        () => {
          const editor = document.createElement("div");
          editor.className = "ambient-capability-editor";
          const label = document.createElement("label");
          label.textContent = "Correct this memory";
          const textarea = document.createElement("textarea");
          textarea.rows = 3;
          textarea.value = memory.content;
          label.append(textarea);
          const editorActions = document.createElement("div");
          editorActions.className = "ambient-capability-actions";
          const save = document.createElement("button");
          save.type = "button";
          save.dataset.primary = "true";
          save.textContent = "Save correction";
          const cancel = document.createElement("button");
          cancel.type = "button";
          cancel.textContent = "Cancel";
          editorActions.append(save, cancel);
          editor.append(label, editorActions);
          copy.replaceWith(editor);
          actions.hidden = true;
          textarea.focus();

          cancel.addEventListener("click", () => renderList(), { signal: events.signal });
          save.addEventListener(
            "click",
            async () => {
              const content = textarea.value.trim();
              if (!content) {
                setStatus("A saved memory cannot be empty.", "error");
                return;
              }
              save.disabled = true;
              try {
                const result = await invoke<MemoryMutationResult>("correct_saved_memory", {
                  id: memory.id,
                  expectedRevision: memory.revision,
                  content,
                });
                if (!result.memory) throw new Error("The saved memory was not returned.");
                memories = memories.map((item) =>
                  item.id === result.memory!.id ? result.memory! : item,
                );
                undoToken = result.undo_token;
                setStatus("Correction saved. Original conversation unchanged.");
                renderList();
              } catch (error) {
                setStatus(`Could not save correction: ${errorMessage(error)}`, "error");
                save.disabled = false;
              }
            },
            { signal: events.signal },
          );
        },
        { signal: events.signal },
      );

      forget.addEventListener(
        "click",
        async () => {
          if (forget.dataset.confirm !== "true") {
            forget.dataset.confirm = "true";
            forget.textContent = "Confirm forget";
            return;
          }
          forget.disabled = true;
          try {
            const result = await invoke<MemoryMutationResult>("forget_saved_memory", {
              id: memory.id,
              expectedRevision: memory.revision,
            });
            memories = memories.filter((item) => item.id !== memory.id);
            undoToken = result.undo_token;
            setStatus("Saved memory forgotten. Conversation history unchanged.");
            renderList();
          } catch (error) {
            setStatus(`Could not forget memory: ${errorMessage(error)}`, "error");
            forget.disabled = false;
          }
        },
        { signal: events.signal },
      );
    }
  };

  undo.addEventListener(
    "click",
    async () => {
      if (!undoToken) return;
      undo.disabled = true;
      try {
        const restored = await invoke<SavedMemory>("undo_saved_memory", {
          undoToken,
        });
        const index = memories.findIndex((memory) => memory.id === restored.id);
        if (index >= 0) memories[index] = restored;
        else memories.unshift(restored);
        undoToken = null;
        setStatus("Undo saved.");
        renderList();
      } catch (error) {
        undoToken = null;
        setStatus(`Undo unavailable: ${errorMessage(error)}`, "error");
      } finally {
        undo.disabled = false;
      }
    },
    { signal: events.signal },
  );

  void invoke<SavedMemory[]>("list_saved_memories")
    .then((result) => {
      memories = result;
      renderList();
    })
    .catch((error) => {
      list.setAttribute("aria-busy", "false");
      list.textContent = "Saved memories could not be loaded.";
      setStatus(errorMessage(error), "error");
    });

  return () => {
    events.abort();
    host.classList.remove("ambient-capability");
  };
}
