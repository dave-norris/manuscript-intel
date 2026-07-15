<script setup lang="ts">
import { inject, ref } from 'vue';
import type { Ref, ComputedRef } from 'vue';
import type { Story, AnalysisState } from '../types';
import LogStream from './LogStream.vue';

// ── Injections ────────────────────────────────────────────────────────────────

const storiesCtx = inject<{
  activeStory: ComputedRef<Story | null>;
  activeFolder: ComputedRef<string>;
}>('stories')!;

const analysisCtx = inject<{
  analysisState: Ref<AnalysisState | null>;
  isWorking: Ref<boolean>;
  runAnalyze: (folder: string, forceResummarize: boolean, platform: string) => Promise<void>;
  runMarketIntel: (folder: string) => Promise<void>;
  cancelOperation: () => Promise<void>;
}>('analysis')!;

const platformCtx = inject<{
  platform: Ref<'kdp' | 'wide'>;
  isKdp: ComputedRef<boolean>;
  setPlatform: (p: 'kdp' | 'wide') => void;
}>('platform')!;

// ── Local state ───────────────────────────────────────────────────────────────

const forceResummarize = ref(false);

// ── Handlers ──────────────────────────────────────────────────────────────────

function onAnalyze(): void {
  const folder = storiesCtx.activeFolder.value;
  analysisCtx.runAnalyze(folder, forceResummarize.value, platformCtx.platform.value);
}

function onMarketIntel(): void {
  const folder = storiesCtx.activeFolder.value;
  analysisCtx.runMarketIntel(folder);
}

function onStop(): void {
  analysisCtx.cancelOperation();
}
</script>

<template>
  <div class="panel analyzer-panel">
    <h2 class="panel-title">Analyzer</h2>
    <p class="panel-desc">
      {{ storiesCtx.activeStory.value ? `Story: ${storiesCtx.activeStory.value.name}` : 'Select or create a story to begin.' }}
    </p>

    <!-- Platform tabs -->
    <div class="platform-tabs">
      <button
        class="platform-tab"
        :class="{ active: platformCtx.platform.value === 'kdp' }"
        @click="platformCtx.setPlatform('kdp')"
      >KDP</button>
      <button
        class="platform-tab"
        :class="{ active: platformCtx.platform.value === 'wide' }"
        @click="platformCtx.setPlatform('wide')"
      >Wide</button>
    </div>

    <!-- Action buttons -->
    <div class="analyzer-buttons">
      <button
        class="btn"
        title="Run full analysis pipeline: chapters, genres, categories, keywords, BISAC"
        :disabled="analysisCtx.isWorking.value || !storiesCtx.activeFolder.value"
        @click="onAnalyze"
      >Get Reports</button>

      <button
        v-if="platformCtx.isKdp.value"
        class="btn btn-secondary"
        title="Run market intelligence via Canopy API: competition analysis, review mining, and author catalog analysis. Requires Canopy API key."
        :disabled="analysisCtx.isWorking.value || !analysisCtx.analysisState.value?.has_search_terms"
        @click="onMarketIntel"
      >Market Intel</button>

      <button
        v-if="analysisCtx.isWorking.value"
        class="btn btn-action-stop"
        title="Stop after current step"
        @click="onStop"
      >Stop</button>
    </div>

    <!-- Force re-summarize -->
    <div class="analyzer-options">
      <label class="force-resummarize-label">
        <input v-model="forceResummarize" type="checkbox" />
        Force re-summarize
      </label>
    </div>

    <!-- Activity indicator -->
    <div v-if="analysisCtx.isWorking.value" class="activity-indicator">
      <div class="spinner"></div>
      <span class="activity-text">Working...</span>
    </div>

    <!-- Log output -->
    <LogStream />
  </div>
</template>

<style scoped>
.analyzer-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
  padding: 20px;
  overflow: hidden;
}

.panel-title {
  font-size: 16px;
  font-weight: 700;
  margin-bottom: 10px;
}

.panel-desc {
  color: var(--text-muted);
  margin-bottom: 14px;
  font-size: 13px;
  line-height: 1.5;
}

.platform-tabs {
  display: flex;
  gap: 0;
  margin-bottom: 14px;
  border-bottom: 2px solid var(--border);
}

.platform-tab {
  background: none;
  border: none;
  color: var(--text-muted);
  font-size: 13px;
  font-weight: 600;
  padding: 8px 16px;
  cursor: pointer;
  border-bottom: 2px solid transparent;
  margin-bottom: -2px;
  transition: color 0.15s, border-color 0.15s;
}

.platform-tab:hover {
  color: var(--text);
}

.platform-tab.active {
  color: var(--accent);
  border-bottom-color: var(--accent);
}

.analyzer-buttons {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 8px;
  align-items: center;
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
}

.btn:hover {
  background: var(--accent-dim);
}

.btn:disabled {
  background: var(--surface2);
  color: var(--text-muted);
  cursor: not-allowed;
}

.btn-secondary {
  background: var(--surface2);
  border: 1px solid var(--border);
  color: var(--text-muted);
}

.btn-secondary:hover:not(:disabled) {
  color: var(--text);
  border-color: var(--accent);
}

.btn-action-stop {
  background: var(--danger);
  color: white;
  font-size: 12px;
  padding: 9px 12px;
  border-radius: var(--radius);
  border: none;
  cursor: pointer;
  white-space: nowrap;
}

.btn-action-stop:hover {
  background: #a04050;
}

.analyzer-options {
  margin-bottom: 8px;
}

.force-resummarize-label {
  font-size: 12px;
  color: var(--text-muted);
  display: flex;
  align-items: center;
  gap: 4px;
  white-space: nowrap;
  cursor: pointer;
}

.force-resummarize-label input[type="checkbox"] {
  accent-color: var(--accent);
}

.activity-indicator {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 10px;
  padding: 6px 12px;
  background: rgba(232, 97, 44, 0.06);
  border: 1px solid rgba(232, 97, 44, 0.15);
  border-radius: var(--radius);
  font-size: 12px;
  color: var(--accent);
}

.spinner {
  width: 14px;
  height: 14px;
  border: 2px solid rgba(232, 97, 44, 0.3);
  border-top-color: var(--accent);
  border-radius: 50%;
  animation: spin 0.8s linear infinite;
}

@keyframes spin {
  to { transform: rotate(360deg); }
}

.activity-text {
  font-weight: 500;
}
</style>
