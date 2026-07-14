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
  runAnalyze: (folder: string, forceResummarize: boolean) => Promise<void>;
  runCompetition: (folder: string, store: string) => Promise<void>;
  runMineReviews: (folder: string) => Promise<void>;
  runAuthorAnalysis: (folder: string) => Promise<void>;
  cancelOperation: () => Promise<void>;
}>('analysis')!;

const platformCtx = inject<{
  platform: Ref<'kdp' | 'wide'>;
  isKdp: ComputedRef<boolean>;
  setPlatform: (p: 'kdp' | 'wide') => void;
}>('platform')!;

// ── Local state ───────────────────────────────────────────────────────────────

const forceResummarize = ref(false);
const compStore = ref<'Kindle' | 'Audible'>('Kindle');

// ── Handlers ──────────────────────────────────────────────────────────────────

function onAnalyze(): void {
  const folder = storiesCtx.activeFolder.value;
  analysisCtx.runAnalyze(folder, forceResummarize.value);
}

function onCompetition(): void {
  const folder = storiesCtx.activeFolder.value;
  analysisCtx.runCompetition(folder, compStore.value);
}

function onMineReviews(): void {
  const folder = storiesCtx.activeFolder.value;
  analysisCtx.runMineReviews(folder);
}

function onAuthorAnalysis(): void {
  const folder = storiesCtx.activeFolder.value;
  analysisCtx.runAuthorAnalysis(folder);
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
        class="btn btn-run-all"
        title="Run full analysis pipeline: chapters, genres, categories, keywords, BISAC"
        :disabled="analysisCtx.isWorking.value || !storiesCtx.activeFolder.value"
        @click="onAnalyze"
      >Get Reports</button>

      <div v-if="platformCtx.isKdp.value" class="competition-wrapper">
        <button
          class="btn btn-run-all-comp"
          title="Analyze competition using Canopy API data"
          :disabled="analysisCtx.isWorking.value || !analysisCtx.analysisState.value?.has_search_terms"
          @click="onCompetition"
        >Analyze Competition</button>
        <div class="store-selector">
          <label class="store-option">
            <input v-model="compStore" type="radio" name="comp-store" value="Kindle" />
            Kindle
          </label>
          <label class="store-option">
            <input v-model="compStore" type="radio" name="comp-store" value="Audible" />
            Audible
          </label>
        </div>
      </div>

      <button
        v-if="platformCtx.isKdp.value"
        class="btn btn-secondary"
        title="Mine competitor reviews for reader language insights"
        :disabled="analysisCtx.isWorking.value || !analysisCtx.analysisState.value?.has_search_terms"
        @click="onMineReviews"
      >Mine Reviews</button>

      <button
        v-if="platformCtx.isKdp.value"
        class="btn btn-secondary"
        title="Analyze competitor author catalogs for strategy insights"
        :disabled="analysisCtx.isWorking.value || !analysisCtx.analysisState.value?.has_search_terms"
        @click="onAuthorAnalysis"
      >Author Analysis</button>

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

.btn-run-all {
  font-size: 14px;
  padding: 10px 16px;
  font-weight: 700;
}

.btn-run-all:hover {
  background: #c87820;
}

.btn-run-all-comp {
  font-size: 14px;
  padding: 10px 16px;
  font-weight: 700;
  background: var(--accent);
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

.competition-wrapper {
  position: relative;
}

.competition-wrapper .store-selector {
  display: none;
  position: absolute;
  top: 100%;
  left: 0;
  z-index: 10;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  padding: 8px 12px;
  margin-top: 4px;
  white-space: nowrap;
}

.competition-wrapper:hover .store-selector,
.competition-wrapper:focus-within .store-selector {
  display: flex;
  gap: 8px;
}

.store-option {
  display: flex;
  align-items: center;
  gap: 6px;
  padding: 5px 12px;
  border: 1px solid var(--border);
  border-radius: 20px;
  cursor: pointer;
  font-size: 13px;
  color: var(--text-muted);
  transition: border-color 0.15s, color 0.15s, background 0.15s;
  user-select: none;
}

.store-option:hover {
  border-color: var(--accent);
  color: var(--text);
}

.store-option input[type="radio"] {
  display: none;
}

.store-option:has(input:checked) {
  background: var(--accent);
  border-color: var(--accent);
  color: #fff;
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
