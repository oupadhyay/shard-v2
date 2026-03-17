import type { ModelInfo, HeartbeatStatusInfo } from "../types";
import { invoke } from "@tauri-apps/api/core";
/**
 * Settings modal UI component
 */

export const SETTINGS_MODAL_HTML = `
  <div class="settings-content">
    <!-- Tab Navigation -->
    <div class="settings-tabs">
      <button class="settings-tab active" data-tab="api-keys">Keys</button>
      <button class="settings-tab" data-tab="models">Models</button>
      <button class="settings-tab" data-tab="capabilities">Capabilities</button>
      <button class="settings-tab" data-tab="heartbeats">Heartbeats</button>
    </div>

    <!-- Tab Panels -->
    <div class="settings-panels">
      <!-- API Keys Panel -->
      <div class="settings-panel active" id="panel-api-keys">
        <div class="setting-group">
          <label>Gemini API Key</label>
          <input type="password" id="gemini-key" placeholder="Enter Gemini API Key" />
        </div>
        <div class="setting-group">
          <label>OpenRouter API Key <span class="required-hint">*</span></label>
          <input type="password" id="openrouter-key" placeholder="Enter OpenRouter API Key" />
        </div>
        <div class="setting-group">
          <label>Cerebras API Key</label>
          <input type="password" id="cerebras-key" placeholder="Enter Cerebras API Key" />
        </div>
        <div class="setting-group">
          <label>Groq API Key <span class="required-hint">*</span></label>
          <input type="password" id="groq-key" placeholder="Enter Groq API Key" />
        </div>
        <div class="setting-group">
          <label>Brave Search API Key</label>
          <input type="password" id="brave-key" placeholder="Enter Brave API Key for web search" />
        </div>
      </div>

      <!-- Models Panel -->
      <div class="settings-panel" id="panel-models">
        <div class="setting-group">
          <label>Chat Model</label>
          <select id="model-id">
            <!-- Dynamically populated from backend -->
          </select>
        </div>
        <div class="setting-group">
          <label>Background Job Model</label>
          <select id="background-model-id">
            <!-- Dynamically populated from backend -->
          </select>
        </div>
        <div id="provider-conflict-warning" style="color: #ff8844; font-size: 0.8em; display: none;">
          ⚠ Chat and background models use the same provider. This may cause rate limiting.
        </div>
      </div>

      <!-- Capabilities Panel -->
      <div class="settings-panel" id="panel-capabilities">
        <div class="setting-group checkbox-setting">
          <label>
            <input type="checkbox" id="enable-tools" />
            <span class="checkbox-label">Enable Tools (Search, etc.)</span>
          </label>
        </div>
        <div class="setting-group checkbox-setting">
          <label>
            <input type="checkbox" id="incognito-mode" />
            <span class="checkbox-label">Incognito Mode (No Memories)</span>
          </label>
        </div>
        <div class="setting-group checkbox-setting">
          <label>
            <input type="checkbox" id="enable-screen-context" />
            <span class="checkbox-label">Screen Context (Beta)</span>
          </label>
        </div>
      </div>

      <!-- Heartbeats Panel -->
      <div class="settings-panel" id="panel-heartbeats">
        <div class="setting-group">
          <label>Global Cooldown (seconds)</label>
          <input type="number" id="heartbeat-cooldown" min="0" max="3600" step="10" placeholder="60" />
          <span class="setting-hint">Minimum gap between any two heartbeat runs</span>
        </div>
        <div class="setting-group">
          <label>Active Heartbeats</label>
          <div id="heartbeat-list" class="heartbeat-list">
            <div class="heartbeat-empty">Loading...</div>
          </div>
        </div>
      </div>
    </div>

    <div class="settings-actions">
      <button id="save-settings">Save</button>
      <button id="close-settings">Close</button>
    </div>
  </div>
`;

/**
 * Initialize settings modal logic (tabs, etc.)
 */
export function initSettingsTabs(settingsModal: HTMLElement) {
  const settingsTabs = settingsModal.querySelector(".settings-tabs");
  settingsTabs?.addEventListener("click", (e) => {
    const target = e.target as HTMLElement;
    if (!target.classList.contains("settings-tab")) return;

    const tabId = target.dataset.tab;
    if (!tabId) return;

    // Update active tab
    settingsTabs.querySelectorAll(".settings-tab").forEach((tab) => {
      tab.classList.remove("active");
    });
    target.classList.add("active");

    // Update active panel
    const panels = settingsModal.querySelectorAll(".settings-panel");
    panels.forEach((panel) => {
      panel.classList.remove("active");
    });
    const activePanel = settingsModal.querySelector(`#panel-${tabId}`);
    activePanel?.classList.add("active");
  });
}


/**
 * Helper to populate a dropdown with models grouped by provider
 */
export function populateModelDropdown(
  selectEl: HTMLSelectElement,
  models: ModelInfo[],
  selectedValue: string | null
) {
  // Clear existing options
  selectEl.innerHTML = "";

  // Group models by provider display name
  const providerDisplayNames: Record<string, string> = {
    gemini: "Gemini AI",
    openrouter: "OpenRouter",
    groq: "Groq",
    cerebras: "Cerebras",
  };

  const groups: Record<string, ModelInfo[]> = {};
  for (const model of models) {
    const providerKey = model.provider;
    if (!groups[providerKey]) {
      groups[providerKey] = [];
    }
    groups[providerKey].push(model);
  }

  // Create optgroups in order: gemini, openrouter, groq, cerebras
  const providerOrder = ["gemini", "openrouter", "groq", "cerebras"];
  for (const provider of providerOrder) {
    const modelsInGroup = groups[provider];
    if (!modelsInGroup || modelsInGroup.length === 0) continue;

    const optgroup = document.createElement("optgroup");
    optgroup.label = providerDisplayNames[provider] || provider;

    for (const model of modelsInGroup) {
      const option = document.createElement("option");
      option.value = model.id;
      option.textContent = model.display_name;
      option.dataset.provider = model.provider;
      optgroup.appendChild(option);
    }

    selectEl.appendChild(optgroup);
  }

  // Set selected value if provided and exists
  if (selectedValue) {
    // Check if the value exists in options
    const exists = Array.from(selectEl.options).some(opt => opt.value === selectedValue);
    if (exists) {
      selectEl.value = selectedValue;
    }
  }
}

/**
 * Populate the Heartbeats dashboard panel
 */
export async function populateHeartbeatsPanel(settingsModal: HTMLElement) {
  const listEl = settingsModal.querySelector("#heartbeat-list");
  if (!listEl) return;

  try {
    const heartbeats = await invoke<HeartbeatStatusInfo[]>("get_heartbeat_status");

    if (heartbeats.length === 0) {
      listEl.innerHTML = `<div class="heartbeat-empty">No heartbeats configured.<br><span class="setting-hint">Add .toml files to ~/Library/Application Support/dev.ojasw.shard/heartbeats/</span></div>`;
      return;
    }

    listEl.innerHTML = "";
    for (const hb of heartbeats) {
      const card = document.createElement("div");
      card.className = "heartbeat-card";

      const personaBadge = hb.persona
        ? `<span class="heartbeat-badge persona">${escapeHtml(hb.persona)}</span>`
        : "";
      const capBadge = hb.max_runs_per_day !== null
        ? `<span class="heartbeat-badge cap">${hb.max_runs_per_day}/day</span>`
        : `<span class="heartbeat-badge cap">unlimited</span>`;

      card.innerHTML = `
        <div class="heartbeat-card-header">
          <span class="heartbeat-name">${escapeHtml(hb.filename)}</span>
          <div class="heartbeat-badges">${personaBadge}${capBadge}</div>
        </div>
        <div class="heartbeat-meta">
          <span>⏱ <code>${escapeHtml(hb.schedule)}</code></span>
          <span>📁 <code>${escapeHtml(hb.session)}</code></span>
        </div>
        <div class="heartbeat-prompt">${escapeHtml(hb.prompt_preview)}</div>
      `;
      listEl.appendChild(card);
    }
  } catch (e) {
    listEl.innerHTML = `<div class="heartbeat-empty">Failed to load heartbeats.</div>`;
    console.error("Failed to load heartbeat status:", e);
  }
}

function escapeHtml(str: string): string {
  return str.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}
