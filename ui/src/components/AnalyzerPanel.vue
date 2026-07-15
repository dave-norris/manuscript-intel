<script setup lang="ts">
import { inject, ref, computed, watch } from 'vue';
import type { Ref, ComputedRef } from 'vue';
import type { Story, AnalysisState } from '../types';
import LogStream from './LogStream.vue';

// ── Report definitions ────────────────────────────────────────────────────────

interface ReportDef {
  id: string;
  label: string;
  description: string;
  platforms: string[];
}

const REPORT_DEFS: ReportDef[] = [
  { id: 'chapter_summaries', label: 'Chapter Summaries', description: 'Extract genre signals from each chapter of the manuscript.', platforms: ['kdp', 'wide'] },
  { id: 'genre_analysis', label: 'Genre Analysis', description: 'Industry genre classification, KDP paths, comps, and reader demographic.', platforms: ['kdp', 'wide'] },
  { id: 'genre_ranking', label: 'Genre Ranking', description: 'Score the manuscript against all known genres independently.', platforms: ['kdp', 'wide'] },
  { id: 'kdp_categories', label: 'KDP Categories', description: 'Find the best-fit Amazon categories with discoverability stats.', platforms: ['kdp'] },
  { id: 'kdp_keywords', label: 'KDP Keywords', description: 'Optimize the 7 keyword strings for KDP discoverability.', platforms: ['kdp'] },
  { id: 'bisac_classification', label: 'BISAC Classification', description: 'Select BISAC subject codes for KDP Print and Ingram distribution.', platforms: ['kdp', 'wide'] },
  { id: 'mi_search_terms', label: 'Search Terms', description: 'Generate competition search phrases for market analysis.', platforms: ['kdp'] },
  { id: 'discovery_keywords', label: 'Discovery Keywords', description: 'Keywords optimized for Apple Books, Kobo, Google Play, and SEO.', platforms: ['wide'] },
];

const REPORT_DEPS: Record<string, string[]> = {
  'chapter_summaries': [],
  'genre_analysis': ['chapter_summaries'],
  'genre_ranking': ['chapter_summaries', 'genre_analysis'],
  'kdp_categories': ['chapter_summaries', 'genre_analysis', 'genre_ranking'],
  'kdp_keywords': ['chapter_summaries', 'genre_analysis', 'genre_ranking'],
  'bisac_classification': ['chapter_summaries', 'genre_analysis'],
  'mi_search_terms': ['chapter_summaries', 'genre_analysis'],
  'discovery_keywords': ['chapter_summaries', 'genre_analysis'],
};

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

const selected = ref<string[]>([]);
const forceResummarize = ref(false);
const hasRun = ref(false);

// ── Computed ──────────────────────────────────────────────────────────────────

/** Map report IDs to their "exists" status based on analysisState */
const existsMap = computed(() => {
  const state = analysisCtx.analysisState.value;
  const map: Record<string, boolean> = {
    chapter_summaries: false,
    genre_analysis: false,
    genre_ranking: false,
    kdp_categories: false,
    kdp_keywords: false,
    bisac_classification: false,
    mi_search_terms: false,
    discovery_keywords: false,
  };
  if (!state) return map;
  map.chapter_summaries = state.summary_count > 0;
  map.genre_analysis = state.has_genre_data;
  map.genre_ranking = state.has_genre_ranking;
  map.kdp_categories = state.has_categories;
  map.kdp_keywords = state.has_keywords;
  map.bisac_classification = state.has_bisac;
  map.mi_search_terms = state.has_search_terms;
  map.discovery_keywords = state.has_discovery_keywords;
  return map;
});

/** Reports filtered to the current platform */
const visibleReports = computed(() => {
  const plat = platformCtx.platform.value;
  return REPORT_DEFS
    .filter(r => r.platforms.includes(plat))
    .map(r => ({
      ...r,
      exists: existsMap.value[r.id] ?? false,
    }));
});

/** Whether the Get Reports button should be disabled */
const getReportsDisabled = computed(() => {
  return analysisCtx.isWorking.value
    || !storiesCtx.activeFolder.value
    || selected.value.length === 0;
});

// ── Dependency logic ──────────────────────────────────────────────────────────

/** Get all transitive dependencies for a report */
function getAllDeps(id: string): string[] {
  const deps = new Set<string>();
  const queue = [...(REPORT_DEPS[id] ?? [])];
  while (queue.length > 0) {
    const dep = queue.pop()!;
    if (!deps.has(dep)) {
      deps.add(dep);
      queue.push(...(REPORT_DEPS[dep] ?? []));
    }
  }
  return [...deps];
}

/** Get all reports that transitively depend on a given report */
function getDependents(id: string): string[] {
  const dependents: string[] = [];
  for (const [reportId, deps] of Object.entries(REPORT_DEPS)) {
    if (getAllDeps(reportId).includes(id) || deps.includes(id)) {
      dependents.push(reportId);
    }
  }
  return dependents;
}

function onCheckChange(id: string): void {

  const sel = new Set(selected.value);

  if (sel.has(id)) {
    // Checked — add all transitive deps
    const deps = getAllDeps(id);
    for (const dep of deps) {
      sel.add(dep);
    }
  } else {
    // Unchecked — remove all dependents (cascade up)
    const dependents = getDependents(id);
    for (const dep of dependents) {
      sel.delete(dep);
    }
  }

  selected.value = [...sel];
}

// Reset selection when platform changes
watch(() => platformCtx.platform.value, () => {
  selected.value = [];
});

// ── Handlers ──────────────────────────────────────────────────────────────────

function onGetReports(): void {
  const folder = storiesCtx.activeFolder.value;
  hasRun.value = true;
  analysisCtx.runAnalyze(folder, forceResummarize.value, platformCtx.platform.value);
}

function onMarketIntel(): void {
  const folder = storiesCtx.activeFolder.value;
  hasRun.value = true;
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

    <!-- Actions (top) -->
    <div class="analyzer-actions">
      <button
        class="btn"
        :disabled="getReportsDisabled"
        @click="onGetReports"
      >Get Reports</button>

      <button
        v-if="platformCtx.isKdp.value"
        class="btn btn-secondary"
        title="Run market intelligence via Canopy API"
        :disabled="analysisCtx.isWorking.value || !analysisCtx.analysisState.value?.has_search_terms"
        @click="onMarketIntel"
      >Market Intel</button>

      <button
        v-if="analysisCtx.isWorking.value"
        class="btn btn-stop"
        @click="onStop"
      >Stop</button>

      <label class="force-resummarize-label">
        <input v-model="forceResummarize" type="checkbox" />
        Force re-summarize
      </label>
    </div>

    <!-- Report cards -->
    <div class="report-cards">
      <div
        v-for="report in visibleReports"
        :key="report.id"
        class="report-card"
      >
        <label class="report-card-check">
          <input
            type="checkbox"
            v-model="selected"
            :value="report.id"
            @change="onCheckChange(report.id)"
          />
        </label>
        <div class="report-card-content">
          <div class="report-card-label">{{ report.label }}</div>
          <div class="report-card-desc">{{ report.description }}</div>
          <div v-if="report.exists" class="report-card-exists">✓ exists</div>
        </div>
      </div>
    </div>

    <!-- Activity indicator -->
    <div v-if="hasRun && analysisCtx.isWorking.value" class="activity-indicator">
      <div class="spinner"></div>
      <span class="activity-text">Working...</span>
    </div>

    <!-- Log output (only shown after first run) -->
    <LogStream v-if="hasRun" />
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

/* ── Platform tabs ─────────────────────────────────────────────────────────── */

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

/* ── Report cards ──────────────────────────────────────────────────────────── */

.report-cards {
  flex: 1;
  overflow-y: auto;
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 8px;
  margin-bottom: 14px;
  padding-right: 4px;
  align-content: start;
}

.report-card {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 10px 12px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  transition: border-color 0.15s, opacity 0.15s;
}

.report-card:hover {
  border-color: var(--accent);
}

.report-card.dimmed {
  opacity: 0.55;
}

.report-card.dimmed:hover {
  border-color: var(--border);
}

.report-card-check {
  display: flex;
  align-items: center;
  padding-top: 2px;
  cursor: pointer;
}

.report-card-check input[type="checkbox"] {
  accent-color: var(--accent);
  width: 15px;
  height: 15px;
  cursor: pointer;
}

.report-card.dimmed .report-card-check input[type="checkbox"] {
  cursor: not-allowed;
}

.report-card-content {
  display: flex;
  flex-direction: column;
  gap: 2px;
  min-width: 0;
}

.report-card-label {
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
}

.report-card-desc {
  font-size: 12px;
  color: var(--text-muted);
  line-height: 1.4;
}

.report-card-exists {
  font-size: 11px;
  color: var(--accent);
  font-weight: 500;
  margin-top: 2px;
}

/* ── Actions ───────────────────────────────────────────────────────────────── */

.analyzer-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  align-items: center;
  margin-bottom: 8px;
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

.btn-stop {
  background: var(--danger);
  color: white;
  font-size: 12px;
  padding: 9px 12px;
  border-radius: var(--radius);
  border: none;
  cursor: pointer;
  white-space: nowrap;
}

.btn-stop:hover {
  background: #a04050;
}

.force-resummarize-label {
  font-size: 12px;
  color: var(--text-muted);
  display: flex;
  align-items: center;
  gap: 4px;
  white-space: nowrap;
  cursor: pointer;
  margin-left: auto;
}

.force-resummarize-label input[type="checkbox"] {
  accent-color: var(--accent);
}

/* ── Activity indicator ────────────────────────────────────────────────────── */

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
