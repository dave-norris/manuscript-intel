<script setup lang="ts">
import { inject, ref, computed, watch, onMounted } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { ContinuityScope } from '../composables/useAnalysis';
import { useSettings } from '../composables/useSettings';
import { storiesKey, analysisKey, seriesKey, platformKey } from '../injectionKeys';
import LogStream from './LogStream.vue';
import { useReportTypes } from '../composables/useReportTypes';

// ── Injections ────────────────────────────────────────────────────────────────

const storiesCtx = inject(storiesKey)!;
const analysisCtx = inject(analysisKey)!;
const seriesCtx = inject(seriesKey)!;
const platformCtx = inject(platformKey)!;
const settings = useSettings();

// ── Report types from DB ──────────────────────────────────────────────────────

const { reportTypes, loadReportTypes, getDependants } = useReportTypes();
onMounted(() => {
  loadReportTypes();
  fetchCostEstimates();
});

// ── Local state ───────────────────────────────────────────────────────────────

const selected = ref<string[]>([]);
const forceResummarize = ref(false);
const hasRun = ref(false);
const continuityScopeMode = ref<'manuscript' | 'series'>('manuscript');
const continuitySeriesId = ref<number | null>(null);

// ── Computed ──────────────────────────────────────────────────────────────────

const existsMap = computed(() => {
  const state = analysisCtx.analysisState.value;
  if (!state) return {} as Record<string, boolean>;
  return {
    chapter_summaries: state.summary_count > 0,
    genre_analysis: state.has_genre_data,
    genre_ranking: state.has_genre_ranking,
    kdp_categories: state.has_categories,
    kdp_keywords: state.has_keywords,
    bisac_classification: state.has_bisac,
    mi_search_terms: state.has_search_terms,
    discovery_keywords: state.has_discovery_keywords,
    analysis: state.has_full_report,
    keyword_search: state.has_keyword_search_results,
    competition_report: state.has_competition,
    review_mining: false,
    author_analysis: false,
    activity_log: false,
    zeigarnik_analysis: state.has_zeigarnik,
    continuity_check: state.has_continuity_check,
    show_dont_tell: state.has_show_dont_tell,
    ai_isms: state.has_ai_isms,
  } as Record<string, boolean>;
});

const visibleReports = computed(() => {
  const plat = platformCtx.platform.value;
  return reportTypes.value
    .filter(r => r.platforms.includes(plat))
    .map(r => ({
      ...r,
      exists: existsMap.value[r.id] ?? false,
    }));
});

const getReportsDisabled = computed(() => {
  return analysisCtx.isWorking.value
    || !storiesCtx.activeFolder.value
    || selected.value.length === 0;
});

// ── Checkbox logic ────────────────────────────────────────────────────────────

function toggleReport(id: string): void {
  const sel = new Set(selected.value);
  const dependants = getDependants(id);

  if (sel.has(id)) {
    // Unchecking: remove this and its dependants
    sel.delete(id);
    for (const dep of dependants) {
      sel.delete(dep);
    }
  } else {
    // Checking: add this and its dependants
    sel.add(id);
    for (const dep of dependants) {
      sel.add(dep);
    }
  }

  selected.value = [...sel];
}

// Reset selection when platform changes
watch(() => platformCtx.platform.value, () => {
  selected.value = [];
});

// ── Cost estimation ───────────────────────────────────────────────────────────

const costEstimates = ref<Record<string, number>>({});

const totalEstimatedCost = computed(() => {
  let total = 0;
  for (const id of selected.value) {
    total += costEstimates.value[id] || 0;
  }
  return total;
});

function formatCost(cost: number): string {
  if (cost === 0) return 'Free';
  if (cost < 0.01) return '<$0.01';
  return `~$${cost.toFixed(2)}`;
}

async function fetchCostEstimates(): Promise<void> {
  const folder = storiesCtx.activeFolder.value;
  if (!folder || visibleReports.value.length === 0 || settings.models.value.length === 0) {
    costEstimates.value = {};
    return;
  }

  // Build model prices for each visible report
  const modelPrices = visibleReports.value.map(r => {
    const fnKey = reportToModelFn(r.id);
    const modelId = settings.modelFor(fnKey);
    const modelInfo = settings.models.value.find(m => m.id === modelId);
    return {
      report_id: r.id,
      input_price: modelInfo?.input_price ?? 0,
      output_price: modelInfo?.output_price ?? 0,
    };
  });

  try {
    const result = await invoke<{ success: boolean; estimates: { report_id: string; estimated_cost: number }[] }>('estimate_report_costs', {
      request: { folder, model_prices: modelPrices },
    });
    if (result.success) {
      const obj: Record<string, number> = {};
      for (const est of result.estimates) {
        obj[est.report_id] = est.estimated_cost;
      }
      costEstimates.value = obj;
    }
  } catch (e) {
    console.error('estimate_report_costs:', e);
  }
}

/** Map report_id to the modelFor() function key (from report_types.model_slot). */
function reportToModelFn(reportId: string): 'default' | 'summaries' | 'genre' | 'keywords' | 'continuity' | 'showDontTell' | 'aiIsms' | 'prose' {
  const slot = reportTypes.value.find(r => r.id === reportId)?.model_slot;
  switch (slot) {
    case 'summaries':
    case 'genre':
    case 'keywords':
    case 'continuity':
    case 'showDontTell':
    case 'aiIsms':
    case 'prose':
      return slot;
    default:
      return 'default';
  }
}

// Refresh estimates when folder changes, models are loaded, or report types load
watch(() => storiesCtx.activeFolder.value, () => fetchCostEstimates());
watch(() => settings.models.value, () => fetchCostEstimates());
watch(() => reportTypes.value, () => fetchCostEstimates());

// ── Handlers ──────────────────────────────────────────────────────────────────

function onGetReports(): void {
  const folder = storiesCtx.activeFolder.value;
  hasRun.value = true;
  if (platformCtx.platform.value === 'craft') {
    const scope: ContinuityScope = continuityScopeMode.value === 'series' && continuitySeriesId.value != null
      ? { mode: 'series', seriesId: continuitySeriesId.value }
      : { mode: 'manuscript' };
    analysisCtx.runCraftAnalysis(folder, selected.value, scope);
  } else {
    analysisCtx.runAnalyze(folder, forceResummarize.value, platformCtx.platform.value);
  }
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
      <button
        class="platform-tab"
        :class="{ active: platformCtx.platform.value === 'craft' }"
        @click="platformCtx.setPlatform('craft')"
      >Craft</button>
    </div>

    <!-- Actions (top) -->
    <div class="analyzer-actions">
      <button
        class="btn"
        :disabled="getReportsDisabled"
        @click="onGetReports"
      >Get Reports</button>

      <span v-if="selected.length > 0 && totalEstimatedCost > 0" class="cost-total">
        {{ formatCost(totalEstimatedCost) }}
      </span>

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

      <label v-if="platformCtx.platform.value !== 'craft'" class="force-resummarize-label">
        <input v-model="forceResummarize" type="checkbox" />
        Force re-summarize
      </label>
    </div>

    <!-- Continuity Check scope (only relevant when that report is selected) -->
    <div v-if="platformCtx.platform.value === 'craft' && selected.includes('continuity_check')" class="continuity-scope-row">
      <span class="continuity-scope-label">Continuity Check scope:</span>
      <label class="scope-radio">
        <input v-model="continuityScopeMode" type="radio" value="manuscript" />
        This manuscript
      </label>
      <label class="scope-radio">
        <input v-model="continuityScopeMode" type="radio" value="series" :disabled="seriesCtx.series.value.length === 0" />
        Series
      </label>
      <select
        v-if="continuityScopeMode === 'series'"
        v-model="continuitySeriesId"
        class="continuity-series-select"
      >
        <option :value="null" disabled>Choose a series…</option>
        <option v-for="s in seriesCtx.series.value" :key="s.id" :value="s.id">{{ s.name }} ({{ s.books.length }} books)</option>
      </select>
      <span v-if="seriesCtx.series.value.length === 0" class="continuity-scope-hint">No series yet — create one in the Series panel.</span>
    </div>

    <!-- Report cards -->
    <div class="report-cards">
      <div
        v-for="report in visibleReports"
        :key="report.id"
        class="report-card"
      >
        <div class="report-card-check">
          <input
            type="checkbox"
            :checked="selected.includes(report.id)"
            @input="toggleReport(report.id)"
          />
        </div>
        <div class="report-card-content">
          <div class="report-card-label">{{ report.label }}</div>
          <div class="report-card-desc">{{ report.description }}</div>
          <div class="report-card-meta">
            <span v-if="report.exists" class="report-card-exists">✓ exists</span>
            <span v-if="costEstimates[report.id] != null" class="report-card-cost">{{ formatCost(costEstimates[report.id]) }}</span>
          </div>
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

.report-card-meta {
  display: flex;
  gap: 10px;
  align-items: center;
  margin-top: 2px;
}

.report-card-cost {
  font-size: 11px;
  color: var(--text-muted);
  font-weight: 500;
}

.cost-total {
  font-size: 12px;
  color: var(--text-muted);
  font-weight: 600;
  padding: 4px 10px;
  background: var(--surface2);
  border-radius: var(--radius);
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

/* ── Continuity scope row ────────────────────────────────────────────────── */

.continuity-scope-row {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 10px;
  padding: 8px 12px;
  margin-bottom: 10px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  font-size: 12px;
}

.continuity-scope-label {
  color: var(--text-muted);
  font-weight: 600;
  white-space: nowrap;
}

.scope-radio {
  display: flex;
  align-items: center;
  gap: 4px;
  color: var(--text);
  cursor: pointer;
  white-space: nowrap;
}

.scope-radio input[type="radio"] {
  accent-color: var(--accent);
}

.continuity-series-select {
  background: var(--surface2);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--text);
  padding: 5px 8px;
  font-size: 12px;
}

.continuity-scope-hint {
  color: var(--text-muted);
  font-size: 11px;
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
