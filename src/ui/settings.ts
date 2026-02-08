/**
 * Settings modal UI component
 */

export const SETTINGS_MODAL_HTML = `
  <div class="settings-content">
    <!-- Tab Navigation -->
    <div class="settings-tabs">
      <button class="settings-tab active" data-tab="api-keys">API Keys</button>
      <button class="settings-tab" data-tab="models">Models</button>
      <button class="settings-tab" data-tab="capabilities">Capabilities</button>
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
