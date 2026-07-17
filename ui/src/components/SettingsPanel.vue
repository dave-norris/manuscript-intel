<script setup lang="ts">
import { inject, ref } from 'vue';
import type { Ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { ModelInfo, WinningCatImportResult, StaleCleanupResult } from '../types';

const settingsCtx = inject<{
  provider: Ref<string>;
  apiKey: Ref<string>;
  modelAssignments: Ref<{ default: string; summaries: string; genre: string; keywords: string; continuity: string; showDontTell: string; prose: string }>;
  canopyApiKey: Ref<string>;
  dataforseoLogin: Ref<string>;
  dataforseoPassword: Ref<string>;
  models: Ref<ModelInfo[]>;
  fetchModels: () => Promise<{ success: boolean; error: string }>;
  saveSettings: () => void;
  testCanopy: () => Promise<{ success: boolean; error: string }>;
  testDataforseo: () => Promise<{ success: boolean; error: string }>;
}>('settings')!;

const showPanel = inject<(name: string) => void>('showPanel')!;

const savedMsg = ref('');
const modelFetchStatus = ref('');
const canopyTestStatus = ref('');
const dataforseoTestStatus = ref('');
const winningcatStatus = ref('');
const staleStatus = ref('');
const showStaleRow = ref(false);
const importDisabled = ref(false);
let lastImportedAt = '';

function modelLabel(m: ModelInfo): string {
  let label = m.id;
  if (m.owned_by) label += ` (${m.owned_by})`;
  if (m.input_price != null && m.output_price != null) {
    label += ` — $${m.input_price}/$${m.output_price} per 1M tokens`;
  }
  return label;
}

async function onFetchModels(): Promise<void> {
  modelFetchStatus.value = 'Fetching models...';
  const result = await settingsCtx.fetchModels();
  if (result.success) {
    modelFetchStatus.value = `${settingsCtx.models.value.length} models loaded.`;
  } else {
    modelFetchStatus.value = result.error;
  }
}

function onSave(): void {
  settingsCtx.saveSettings();
  savedMsg.value = '✓ Saved';
  setTimeout(() => { savedMsg.value = ''; }, 1500);
  showPanel('analyzer');
}

async function onTestCanopy(): Promise<void> {
  canopyTestStatus.value = 'Testing...';
  const result = await settingsCtx.testCanopy();
  canopyTestStatus.value = result.success ? '✓ Connected' : '✗ ' + result.error;
}

async function onTestDataforseo(): Promise<void> {
  dataforseoTestStatus.value = 'Testing...';
  const result = await settingsCtx.testDataforseo();
  dataforseoTestStatus.value = result.success ? '✓ Connected' : '✗ ' + result.error;
}

async function onImportWinningCat(): Promise<void> {
  winningcatStatus.value = 'Select the CSV file...';
  importDisabled.value = true;
  showStaleRow.value = false;
  try {
    const result = await invoke<WinningCatImportResult>('import_winningcat_csv');
    if (result.success) {
      winningcatStatus.value = `✓ Imported ${result.imported} categories. Skipped ${result.skipped_other_department} (other department), ${result.skipped_unparseable} (unparseable).`;
      lastImportedAt = result.imported_at;
      if (result.stale_count > 0) {
        showStaleRow.value = true;
        const word = result.stale_count === 1 ? 'y was' : 'ies were';
        staleStatus.value = `${result.stale_count} categor${word} in the catalog from a previous import but missing from this one — possibly retired or renamed by Amazon.`;
      }
    } else {
      winningcatStatus.value = result.error || 'Import failed.';
    }
  } catch (e) {
    winningcatStatus.value = 'Error: ' + String(e);
  } finally {
    importDisabled.value = false;
  }
}

async function onRemoveStale(): Promise<void> {
  if (!lastImportedAt) return;
  if (!confirm('Remove these stale categories from the catalog? This only affects reference data — no story data is touched.')) return;
  try {
    const result = await invoke<StaleCleanupResult>('remove_stale_kdp_categories', { since: lastImportedAt });
    if (result.success) {
      const word = result.removed === 1 ? 'y' : 'ies';
      staleStatus.value = `✓ Removed ${result.removed} stale categor${word}.`;
      showStaleRow.value = false;
    } else {
      staleStatus.value = result.error || 'Cleanup failed.';
    }
  } catch (e) {
    staleStatus.value = 'Error: ' + String(e);
  }
}
</script>

<template>
  <div class="panel settings-panel">
    <h2 class="panel-title">Settings</h2>

    <div class="settings-form">
      <!-- Provider -->
      <label>Provider</label>
      <div class="provider-options">
        <label class="provider-option">
          <input type="radio" v-model="settingsCtx.provider.value" value="claude" />
          Claude
        </label>
        <label class="provider-option">
          <input type="radio" v-model="settingsCtx.provider.value" value="tokenmix" />
          TokenMix
        </label>
      </div>

      <!-- API Key -->
      <label>API Key</label>
      <input
        type="password"
        v-model="settingsCtx.apiKey.value"
        placeholder="Enter your API key"
      />

      <!-- Model -->
      <label>
        Default Model
        <span class="model-hint">Fetch models first, then assign each function below.</span>
      </label>
      <div class="model-row">
        <select v-model="settingsCtx.modelAssignments.value.default">
          <option v-if="settingsCtx.models.value.length === 0" value="" disabled>
            No models loaded
          </option>
          <option
            v-for="m in settingsCtx.models.value"
            :key="m.id"
            :value="m.id"
          >{{ modelLabel(m) }}</option>
        </select>
        <button class="btn btn-sm" @click="onFetchModels">Fetch Models</button>
      </div>
      <div class="model-fetch-status">{{ modelFetchStatus }}</div>

      <!-- Per-function model assignments -->
      <div v-if="settingsCtx.models.value.length > 0" class="model-assignments">
        <div class="model-assign-header">Model per function</div>

        <div class="model-assign-row">
          <div class="model-assign-label">
            <strong>Chapter Summaries</strong>
            <span class="model-recommend">Fast model. Structured extraction — accuracy matters more than creativity. A smaller, cheaper model works well.</span>
          </div>
          <select v-model="settingsCtx.modelAssignments.value.summaries">
            <option value="">(Use default)</option>
            <option v-for="m in settingsCtx.models.value" :key="m.id" :value="m.id">{{ m.id }}</option>
          </select>
        </div>

        <div class="model-assign-row">
          <div class="model-assign-label">
            <strong>Genre Analysis</strong>
            <span class="model-recommend">Classification task. Needs broad book-market knowledge. Mid-tier model is sufficient.</span>
          </div>
          <select v-model="settingsCtx.modelAssignments.value.genre">
            <option value="">(Use default)</option>
            <option v-for="m in settingsCtx.models.value" :key="m.id" :value="m.id">{{ m.id }}</option>
          </select>
        </div>

        <div class="model-assign-row">
          <div class="model-assign-label">
            <strong>Keywords &amp; Categories</strong>
            <span class="model-recommend">Short structured output. Fast model works — speed over depth.</span>
          </div>
          <select v-model="settingsCtx.modelAssignments.value.keywords">
            <option value="">(Use default)</option>
            <option v-for="m in settingsCtx.models.value" :key="m.id" :value="m.id">{{ m.id }}</option>
          </select>
        </div>

        <div class="model-assign-row">
          <div class="model-assign-label">
            <strong>Continuity Check</strong>
            <span class="model-recommend">Needs reasoning ability to spot contradictions across chapters. Use a capable model (e.g. GPT-4o, Claude Sonnet).</span>
          </div>
          <select v-model="settingsCtx.modelAssignments.value.continuity">
            <option value="">(Use default)</option>
            <option v-for="m in settingsCtx.models.value" :key="m.id" :value="m.id">{{ m.id }}</option>
          </select>
        </div>

        <div class="model-assign-row">
          <div class="model-assign-label">
            <strong>Show Don't Tell</strong>
            <span class="model-recommend">Literary judgment — needs to understand prose craft. Use a strong model (e.g. Claude Sonnet, GPT-4o).</span>
          </div>
          <select v-model="settingsCtx.modelAssignments.value.showDontTell">
            <option value="">(Use default)</option>
            <option v-for="m in settingsCtx.models.value" :key="m.id" :value="m.id">{{ m.id }}</option>
          </select>
        </div>

        <div class="model-assign-row">
          <div class="model-assign-label">
            <strong>Prose Suggestions</strong>
            <span class="model-recommend">Creative rewriting. Use the highest-quality model you have — this writes prose the author will paste into their manuscript.</span>
          </div>
          <select v-model="settingsCtx.modelAssignments.value.prose">
            <option value="">(Use default)</option>
            <option v-for="m in settingsCtx.models.value" :key="m.id" :value="m.id">{{ m.id }}</option>
          </select>
        </div>
      </div>

      <!-- Save -->
      <button class="btn" @click="onSave">Save Settings</button>
      <div class="settings-saved">{{ savedMsg }}</div>
    </div>

    <!-- Canopy section -->
    <div class="settings-section-divider"></div>
    <h3 class="section-title">Canopy API</h3>
    <div class="settings-form">
      <label>Canopy API Key</label>
      <input
        type="password"
        v-model="settingsCtx.canopyApiKey.value"
        placeholder="Enter Canopy API key"
      />
      <button class="btn btn-sm" @click="onTestCanopy">Test Connection</button>
      <div class="canopy-test-status">{{ canopyTestStatus }}</div>
    </div>

    <!-- DataForSEO section -->
    <div class="settings-section-divider"></div>
    <h3 class="section-title">DataForSEO</h3>
    <div class="settings-form">
      <p class="panel-desc">Used for keyword search volume data (Amazon + Google). Get credentials at <strong>app.dataforseo.com</strong>.</p>
      <label>Login (email)</label>
      <input
        type="text"
        v-model="settingsCtx.dataforseoLogin.value"
        placeholder="your@email.com"
      />
      <label>Password</label>
      <input
        type="password"
        v-model="settingsCtx.dataforseoPassword.value"
        placeholder="DataForSEO API password"
      />
      <button class="btn btn-sm" @click="onTestDataforseo">Test Connection</button>
      <div class="canopy-test-status">{{ dataforseoTestStatus }}</div>
    </div>

    <!-- WinningCat section -->
    <div class="settings-section-divider"></div>
    <h3 class="section-title">WinningCat Import</h3>
    <div class="settings-form">
      <p class="panel-desc">Import the WinningCat category catalog CSV to enable category matching.</p>
      <button class="btn" :disabled="importDisabled" @click="onImportWinningCat">Import CSV</button>
      <div class="winningcat-status">{{ winningcatStatus }}</div>
      <div v-if="showStaleRow" class="stale-row">
        <div class="stale-status">{{ staleStatus }}</div>
        <button class="btn btn-sm btn-danger" @click="onRemoveStale">Remove Stale</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.settings-panel {
  padding: 20px;
  overflow-y: auto;
  max-width: 560px;
}

.panel-title {
  font-size: 16px;
  font-weight: 700;
  margin-bottom: 16px;
}

.section-title {
  font-size: 14px;
  font-weight: 600;
  margin-bottom: 10px;
  color: var(--text);
}

.settings-section-divider {
  border-top: 1px solid var(--border);
  margin: 20px 0;
}

.settings-form {
  display: flex;
  flex-direction: column;
  gap: 10px;
  max-width: 480px;
}

.settings-form label {
  font-size: 12px;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.06em;
}

.settings-form input,
.settings-form select {
  background: var(--surface2);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--text);
  font-size: 13px;
  padding: 8px 10px;
  width: 100%;
  user-select: text;
}

.settings-form select option {
  background: var(--surface2);
}

.provider-options {
  display: flex;
  gap: 16px;
}

.provider-option {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  color: var(--text);
  text-transform: none;
  letter-spacing: 0;
  cursor: pointer;
}

.provider-option input[type="radio"] {
  width: auto;
  accent-color: var(--accent);
}

.model-row {
  display: flex;
  gap: 8px;
  align-items: center;
}

.model-row select {
  flex: 1;
}

.model-hint {
  display: block;
  font-size: 11px;
  color: var(--text-muted);
  font-weight: 400;
  text-transform: none;
  letter-spacing: 0;
  margin-top: 2px;
}

.model-fetch-status,
.canopy-test-status,
.winningcat-status {
  font-size: 12px;
  color: var(--text-muted);
  min-height: 16px;
}

.model-assignments {
  margin-top: 12px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 12px;
}

.model-assign-header {
  font-size: 12px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--text-muted);
  margin-bottom: 10px;
}

.model-assign-row {
  display: flex;
  flex-direction: column;
  gap: 4px;
  padding: 8px 0;
  border-bottom: 1px solid var(--border);
}

.model-assign-row:last-child {
  border-bottom: none;
  padding-bottom: 0;
}

.model-assign-label strong {
  font-size: 13px;
  color: var(--text);
}

.model-recommend {
  display: block;
  font-size: 11px;
  color: var(--text-muted);
  line-height: 1.4;
  margin-top: 2px;
  font-style: italic;
}

.model-assign-row select {
  background: var(--surface2);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--text);
  font-size: 12px;
  padding: 5px 8px;
}

.settings-saved {
  font-size: 12px;
  color: var(--success);
  min-height: 18px;
}

.panel-desc {
  color: var(--text-muted);
  font-size: 13px;
  line-height: 1.5;
}

.btn {
  background: var(--accent);
  border: none;
  border-radius: var(--radius);
  color: #fff;
  cursor: pointer;
  font-size: 13px;
  font-weight: 600;
  padding: 9px 18px;
  transition: background 0.15s;
  align-self: flex-start;
}

.btn:hover { background: var(--accent-dim); }
.btn:disabled { background: var(--surface2); color: var(--text-muted); cursor: not-allowed; }

.btn-sm {
  padding: 6px 12px;
  font-size: 12px;
  white-space: nowrap;
}

.btn-danger {
  background: #c0392b;
  color: #fff;
}
.btn-danger:hover { background: #a93226; }

.stale-row {
  display: flex;
  flex-direction: column;
  gap: 8px;
  padding: 10px;
  background: var(--surface2);
  border: 1px solid var(--border);
  border-radius: var(--radius);
}

.stale-status {
  font-size: 12px;
  color: var(--text-muted);
}
</style>
