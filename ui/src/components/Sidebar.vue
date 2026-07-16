<script setup lang="ts">
import { inject, ref } from 'vue';
import type { Ref, ComputedRef } from 'vue';
import type { Story, ReportEnvelope, Series, SidebarReportGroup } from '../types';

// ── Injections ────────────────────────────────────────────────────────────────

const storiesCtx = inject<{
  stories: Ref<Story[]>;
  activeStoryId: Ref<string | null>;
  activeStory: ComputedRef<Story | null>;
  activeFolder: ComputedRef<string>;
  setActiveStory: (id: string | null) => void;
}>('stories')!;

const reportsCtx = inject<{
  sidebarGroups: Ref<SidebarReportGroup[]>;
  currentReport: Ref<ReportEnvelope | null>;
  loadSidebarReports: (folder: string, platform: string) => Promise<void>;
  openReport: (id: number) => Promise<ReportEnvelope>;
  deleteReport: (id: number) => Promise<void>;
  closeReport: () => void;
}>('reports')!;

const platformCtx = inject<{
  platform: Ref<'kdp' | 'wide' | 'craft'>;
}>('platform')!;

const showPanel = inject<(name: string) => void>('showPanel')!;

const seriesCtx = inject<{
  series: Ref<Series[]>;
  loadSeries: () => Promise<void>;
}>('series')!;

// ── Emits ─────────────────────────────────────────────────────────────────────

const emit = defineEmits<{
  (e: 'open-story-form', story: Story | null): void;
  (e: 'open-series-form', series: Series | null): void;
}>();

// ── Expand/collapse state ─────────────────────────────────────────────────────

const expanded = ref<string | null>(null);

function toggleExpand(docType: string): void {
  expanded.value = expanded.value === docType ? null : docType;
}

// ── Handlers ──────────────────────────────────────────────────────────────────

function onStoryClick(story: Story): void {
  storiesCtx.setActiveStory(story.id);
  showPanel('analyzer');
}

function onEditStory(story: Story): void {
  emit('open-story-form', story);
}

function onNewStory(): void {
  emit('open-story-form', null);
}

function onNewSeries(): void {
  emit('open-series-form', null);
}

function onEditSeries(s: Series): void {
  emit('open-series-form', s);
}

async function onVersionClick(id: number): Promise<void> {
  await reportsCtx.openReport(id);
  showPanel('reports');
}

async function onDeleteVersion(id: number, e: Event): Promise<void> {
  e.stopPropagation();
  if (!confirm('Delete this report? This cannot be undone.')) return;
  try {
    await reportsCtx.deleteReport(id);
    const folder = storiesCtx.activeFolder.value;
    if (folder) await reportsCtx.loadSidebarReports(folder, platformCtx.platform.value);
  } catch (err) {
    alert('Could not delete: ' + String(err));
  }
}

// ── Timestamp formatting ──────────────────────────────────────────────────────

function formatTimestamp(ts: string): string {
  return new Date(ts).toLocaleString(undefined, {
    month: 'short',
    day: 'numeric',
    hour: 'numeric',
    minute: '2-digit',
  });
}
</script>

<template>
  <aside id="sidebar">
    <!-- Analyzer nav -->
    <div class="nav-section">
      <button class="nav-item" @click="showPanel('analyzer')">
        Analyzer
      </button>
    </div>

    <!-- Stories section -->
    <div class="stories-section">
      <div class="nav-label-row">
        <span class="nav-label">Stories</span>
        <button class="btn-new-story" title="New story" @click="onNewStory">+</button>
      </div>
      <div class="stories-list">
        <div
          v-if="storiesCtx.stories.value.length === 0"
          class="sidebar-hint"
        >
          No stories yet. Click + to add one.
        </div>
        <div
          v-for="story in storiesCtx.stories.value"
          :key="story.id"
          class="story-item"
          :class="{ active: story.id === storiesCtx.activeStoryId.value }"
          :title="story.folder"
          @click="onStoryClick(story)"
        >
          <span class="story-item-name">{{ story.name }}</span>
          <button
            class="story-item-edit"
            :title="'Edit story'"
            @click.stop="onEditStory(story)"
          >&#x270E;</button>
        </div>
      </div>
    </div>

    <!-- Series section -->
    <div class="series-section">
      <div class="nav-label-row">
        <span class="nav-label">Series</span>
        <button class="btn-new-story" title="New series" @click="onNewSeries">+</button>
      </div>
      <div class="series-list">
        <div
          v-for="s in seriesCtx.series.value"
          :key="s.id"
          class="story-item"
          @click="onEditSeries(s)"
        >
          <span class="story-item-name">{{ s.name }}</span>
          <span class="series-book-count">{{ s.books.length }}</span>
        </div>
      </div>
    </div>

    <!-- Reports section -->
    <div class="reports-section">
      <div class="sidebar-section-header">Reports</div>
      <div v-if="!storiesCtx.activeFolder.value" class="sidebar-hint">
        Select a story to see reports.
      </div>
      <template v-else>
        <div
          v-for="type in reportsCtx.sidebarGroups.value"
          :key="type.doc_type"
          class="report-type"
        >
          <div
            class="report-type-header"
            :title="type.description"
            @click="toggleExpand(type.doc_type)"
          >
            <span class="report-type-label">
              {{ type.label }}
            </span>
            <span class="report-count">{{ type.count }}</span>
          </div>

          <!-- Expanded: show versions -->
          <div
            v-if="expanded === type.doc_type && type.versions.length > 0"
            class="report-versions"
          >
            <div
              v-for="version in type.versions"
              :key="version.id"
              class="report-version-item"
              @click="onVersionClick(version.id)"
            >
              <span class="version-label">{{ formatTimestamp(version.generated_at) }}</span>
              <button class="version-delete" @click="onDeleteVersion(version.id, $event)" title="Delete this report">&times;</button>
            </div>
          </div>
        </div>
      </template>
    </div>

    <!-- Settings at bottom -->
    <div class="nav-section settings-section">
      <button class="nav-item" @click="showPanel('series')">
        Series
      </button>
      <button class="nav-item" @click="showPanel('settings')">
        Settings
      </button>
    </div>
  </aside>
</template>

<style scoped>
#sidebar {
  grid-area: sidebar;
  background: var(--surface);
  border-right: 1px solid var(--border);
  padding: 12px 0;
  overflow-y: auto;
  display: flex;
  flex-direction: column;
}

.nav-section {
  padding: 0 12px 8px;
}

.settings-section {
  margin-top: auto;
  border-top: 1px solid var(--border);
  padding-top: 12px;
}

.nav-label {
  font-size: 10px;
  text-transform: uppercase;
  letter-spacing: 0.08em;
  color: var(--text-muted);
  padding: 0 4px 0;
}

.nav-label-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 8px 14px 4px;
}

.nav-item {
  display: block;
  width: 100%;
  background: none;
  border: none;
  border-radius: var(--radius);
  color: var(--text-muted);
  cursor: pointer;
  font-size: 13px;
  padding: 7px 10px;
  text-align: left;
  transition: background 0.15s, color 0.15s;
}

.nav-item:hover {
  background: var(--surface2);
  color: var(--text);
}

.btn-new-story {
  background: none;
  border: 1px solid var(--border);
  color: var(--text-muted);
  width: 20px;
  height: 20px;
  border-radius: 4px;
  cursor: pointer;
  font-size: 16px;
  line-height: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0;
}

.btn-new-story:hover {
  color: var(--accent);
  border-color: var(--accent);
}

.stories-section {
  flex: 1;
  overflow-y: auto;
  min-height: 0;
  padding: 0;
}

.stories-list {
  padding: 0 8px 8px;
}

.story-item {
  padding: 8px 10px;
  border-radius: var(--radius);
  cursor: pointer;
  font-size: 13px;
  color: var(--text-muted);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  position: relative;
  display: flex;
  align-items: center;
}

.story-item:hover {
  background: var(--surface2);
  color: var(--text);
}

.story-item.active {
  background: var(--surface2);
  color: var(--text);
  font-weight: 600;
  border-left: 2px solid var(--accent);
  padding-left: 8px;
}

.story-item-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
}

.story-item-edit {
  position: absolute;
  right: 6px;
  top: 50%;
  transform: translateY(-50%);
  font-size: 11px;
  color: var(--text-muted);
  opacity: 0;
  background: none;
  border: none;
  cursor: pointer;
  padding: 2px 4px;
}

.story-item:hover .story-item-edit {
  opacity: 1;
}

.reports-section {
  flex: 1;
  overflow-y: auto;
  min-height: 0;
}

.sidebar-section-header {
  font-size: 10px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.5px;
  color: var(--text-muted);
  padding: 8px 10px 4px;
}

.report-type {
  margin: 0;
}

.report-type-header {
  padding: 6px 10px;
  font-size: 12px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  cursor: pointer;
  border-radius: var(--radius);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.report-type-header:hover {
  background: var(--surface2);
}

.report-type-label {
  overflow: hidden;
  text-overflow: ellipsis;
  color: var(--text);
}

.report-count {
  background: var(--surface2);
  color: var(--text-muted);
  font-size: 10px;
  font-weight: 600;
  padding: 1px 6px;
  border-radius: 8px;
  min-width: 18px;
  text-align: center;
  flex-shrink: 0;
}

.report-versions {
  padding: 2px 0 4px 18px;
}

.report-version-item {
  display: flex;
  align-items: center;
  padding: 4px 10px;
  font-size: 11px;
  color: var(--text-muted);
  cursor: pointer;
  border-radius: var(--radius);
}

.report-version-item .version-label {
  flex: 1;
}

.report-version-item .version-delete {
  display: none;
  background: none;
  border: none;
  color: var(--text-muted);
  font-size: 14px;
  line-height: 1;
  cursor: pointer;
  padding: 0 4px;
  border-radius: 3px;
}

.report-version-item:hover .version-delete {
  display: inline;
}

.report-version-item .version-delete:hover {
  color: #e74c3c;
  background: var(--surface2);
}

.report-version-item:hover {
  background: var(--surface2);
  color: var(--accent);
}

.sidebar-hint {
  padding: 8px 10px;
  font-size: 11px;
  color: var(--text-muted);
}
.series-section {
  padding: 0;
}
.series-list {
  padding: 0 8px 8px;
}
.series-book-count {
  font-size: 10px;
  color: var(--text-muted);
  background: var(--surface2);
  padding: 1px 5px;
  border-radius: 6px;
  margin-left: auto;
}
</style>
