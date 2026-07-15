<script setup lang="ts">
import { inject, ref, computed, onMounted } from 'vue';
import type { Ref, ComputedRef } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { Story, SeriesRow, ReportEnvelope, DocMeta } from '../types';

// ── Injections ────────────────────────────────────────────────────────────────

const storiesCtx = inject<{
  stories: Ref<Story[]>;
}>('stories')!;

const seriesCtx = inject<{
  series: Ref<SeriesRow[]>;
  activeSeriesBooks: Ref<{ story_folder: string; story_name: string; book_order: number }[]>;
  loadSeries: () => Promise<void>;
  createSeries: (name: string) => Promise<SeriesRow | null>;
  deleteSeries: (seriesId: number) => Promise<void>;
  loadSeriesBooks: (seriesId: number) => Promise<void>;
  addStoryToSeries: (seriesId: number, storyFolder: string, storyName: string, bookOrder: number) => Promise<void>;
  removeStoryFromSeries: (seriesId: number, storyFolder: string) => Promise<void>;
}>('series')!;

const reportsCtx = inject<{
  openReport: (id: number) => Promise<ReportEnvelope>;
}>('reports')!;

const showPanel = inject<(name: string) => void>('showPanel')!;

// ── Local state ───────────────────────────────────────────────────────────────

const newSeriesName = ref('');
const activeSeriesId = ref<number | null>(null);
const addStoryFolder = ref('');
const addBookOrder = ref(1);

// ── Computed ──────────────────────────────────────────────────────────────────

const activeSeries = computed(() => seriesCtx.series.value.find(s => s.id === activeSeriesId.value) || null);

const availableStories = computed(() => {
  const inSeries = new Set(seriesCtx.activeSeriesBooks.value.map(b => b.story_folder));
  return storiesCtx.stories.value.filter(s => !inSeries.has(s.folder));
});

// ── Handlers ──────────────────────────────────────────────────────────────────

async function onCreateSeries(): Promise<void> {
  if (!newSeriesName.value.trim()) return;
  const row = await seriesCtx.createSeries(newSeriesName.value.trim());
  newSeriesName.value = '';
  if (row) onSelectSeries(row.id);
}

async function onDeleteSeries(id: number): Promise<void> {
  if (!confirm('Delete this series? Books stay untouched, but any saved series-wide continuity report is orphaned.')) return;
  await seriesCtx.deleteSeries(id);
  if (activeSeriesId.value === id) activeSeriesId.value = null;
}

async function onSelectSeries(id: number): Promise<void> {
  activeSeriesId.value = id;
  await seriesCtx.loadSeriesBooks(id);
  addBookOrder.value = seriesCtx.activeSeriesBooks.value.length + 1;
}

async function onAddStory(): Promise<void> {
  if (!activeSeriesId.value || !addStoryFolder.value) return;
  const story = storiesCtx.stories.value.find(s => s.folder === addStoryFolder.value);
  if (!story) return;
  await seriesCtx.addStoryToSeries(activeSeriesId.value, story.folder, story.name, addBookOrder.value);
  addStoryFolder.value = '';
  addBookOrder.value = seriesCtx.activeSeriesBooks.value.length + 1;
}

async function onRemoveStory(storyFolder: string): Promise<void> {
  if (!activeSeriesId.value) return;
  await seriesCtx.removeStoryFromSeries(activeSeriesId.value, storyFolder);
}

const loadingReport = ref(false);
const noReportYet = ref(false);

async function onViewContinuityReport(): Promise<void> {
  if (!activeSeriesId.value) return;
  loadingReport.value = true;
  noReportYet.value = false;
  try {
    const docs = await invoke<DocMeta[]>('list_reports_cmd', { folder: `series:${activeSeriesId.value}` });
    const latest = docs.find(d => d.doc_type === 'continuity_check');
    if (!latest) { noReportYet.value = true; return; }
    await reportsCtx.openReport(latest.id);
    showPanel('reports');
  } finally {
    loadingReport.value = false;
  }
}

onMounted(() => seriesCtx.loadSeries());
</script>

<template>
  <div class="panel series-panel">
    <h2 class="panel-title">Series</h2>
    <p class="panel-desc">Group stories into a series, in reading order, so the Continuity Check can compare facts across books &mdash; not just within one manuscript.</p>

    <div class="series-create">
      <input v-model="newSeriesName" type="text" placeholder="New series name" @keyup.enter="onCreateSeries" />
      <button class="btn" :disabled="!newSeriesName.trim()" @click="onCreateSeries">Create</button>
    </div>

    <div class="series-list">
      <div v-if="seriesCtx.series.value.length === 0" class="sidebar-hint">No series yet. Create one above.</div>
      <div
        v-for="s in seriesCtx.series.value"
        :key="s.id"
        class="series-item"
        :class="{ active: s.id === activeSeriesId }"
        @click="onSelectSeries(s.id)"
      >
        <span class="series-item-name">{{ s.name }}</span>
        <span class="series-item-count">{{ s.book_count }} book{{ s.book_count === 1 ? '' : 's' }}</span>
        <button class="series-item-delete" title="Delete series" @click.stop="onDeleteSeries(s.id)">&times;</button>
      </div>
    </div>

    <div v-if="activeSeries" class="series-detail">
      <h3 class="series-detail-title">{{ activeSeries.name }} &mdash; reading order</h3>

      <div v-if="seriesCtx.activeSeriesBooks.value.length === 0" class="sidebar-hint">
        No books added yet.
      </div>
      <ol v-else class="series-books">
        <li v-for="b in seriesCtx.activeSeriesBooks.value" :key="b.story_folder" class="series-book-item">
          <span>{{ b.story_name }}</span>
          <button class="btn-remove" @click="onRemoveStory(b.story_folder)">Remove</button>
        </li>
      </ol>

      <div v-if="availableStories.length" class="series-add-row">
        <select v-model="addStoryFolder">
          <option value="" disabled>Add a story&hellip;</option>
          <option v-for="s in availableStories" :key="s.id" :value="s.folder">{{ s.name }}</option>
        </select>
        <input v-model.number="addBookOrder" type="number" min="1" title="Book order" />
        <button class="btn btn-sm" :disabled="!addStoryFolder" @click="onAddStory">Add</button>
      </div>
      <p v-else class="sidebar-hint">All your stories are already in this series.</p>

      <div class="series-report-row">
        <button class="btn btn-sm btn-secondary" :disabled="loadingReport" @click="onViewContinuityReport">
          View Continuity Report
        </button>
        <span v-if="noReportYet" class="continuity-scope-hint">No series-wide continuity check has been run yet — run one from the Craft tab in the Analyzer.</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
.series-panel {
  padding: 20px;
  overflow-y: auto;
  height: 100%;
}

.panel-title {
  font-size: 16px;
  font-weight: 700;
  margin-bottom: 6px;
}

.panel-desc {
  color: var(--text-muted);
  margin-bottom: 16px;
  font-size: 13px;
  line-height: 1.5;
}

.series-create {
  display: flex;
  gap: 8px;
  margin-bottom: 16px;
}

.series-create input {
  flex: 1;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--text);
  padding: 8px 10px;
  font-size: 13px;
}

.series-list {
  display: flex;
  flex-direction: column;
  gap: 4px;
  margin-bottom: 20px;
}

.series-item {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 10px;
  border-radius: var(--radius);
  cursor: pointer;
  font-size: 13px;
  border: 1px solid transparent;
}

.series-item:hover {
  background: var(--surface2);
}

.series-item.active {
  background: var(--surface2);
  border-color: var(--accent);
}

.series-item-name {
  flex: 1;
  font-weight: 600;
}

.series-item-count {
  color: var(--text-muted);
  font-size: 11px;
}

.series-item-delete {
  background: none;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  font-size: 16px;
  line-height: 1;
  padding: 0 4px;
}

.series-item-delete:hover {
  color: var(--danger, #c0392b);
}

.series-detail {
  border-top: 1px solid var(--border);
  padding-top: 16px;
}

.series-detail-title {
  font-size: 13px;
  font-weight: 700;
  margin-bottom: 10px;
}

.series-books {
  list-style: decimal;
  padding-left: 20px;
  margin-bottom: 14px;
}

.series-book-item {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 4px 0;
  font-size: 13px;
}

.btn-remove {
  background: none;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  font-size: 11px;
  padding: 2px 6px;
}

.btn-remove:hover {
  color: var(--danger, #c0392b);
}

.series-add-row {
  display: flex;
  gap: 8px;
  align-items: center;
}

.series-add-row select {
  flex: 1;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--text);
  padding: 7px 8px;
  font-size: 13px;
}

.series-add-row input {
  width: 56px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--text);
  padding: 7px 8px;
  font-size: 13px;
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

.btn:hover:not(:disabled) {
  background: var(--accent-dim);
}

.btn:disabled {
  background: var(--surface2);
  color: var(--text-muted);
  cursor: not-allowed;
}

.btn-sm {
  padding: 7px 12px;
  font-size: 12px;
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

.series-report-row {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-top: 14px;
  padding-top: 14px;
  border-top: 1px solid var(--border);
}

.continuity-scope-hint {
  color: var(--text-muted);
  font-size: 11px;
}

.sidebar-hint {
  padding: 8px 0;
  font-size: 12px;
  color: var(--text-muted);
}
</style>
