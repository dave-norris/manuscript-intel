<script setup lang="ts">
import { inject, computed } from 'vue';
import type { Ref, ComputedRef } from 'vue';
import type { Story, DocMeta, SavedReportMeta, ReportEnvelope } from '../types';

// ── Injections ────────────────────────────────────────────────────────────────

const storiesCtx = inject<{
  stories: Ref<Story[]>;
  activeStoryId: Ref<string | null>;
  activeStory: ComputedRef<Story | null>;
  activeFolder: ComputedRef<string>;
  setActiveStory: (id: string | null) => void;
}>('stories')!;

const reportsCtx = inject<{
  reports: Ref<DocMeta[]>;
  savedReports: Ref<SavedReportMeta[]>;
  openReport: (folder: string, docType: string) => Promise<ReportEnvelope>;
  openSavedReport: (id: number) => Promise<ReportEnvelope>;
}>('reports')!;

const platformCtx = inject<{
  platform: Ref<'kdp' | 'wide'>;
  KDP_REPORT_TYPES: Set<string>;
  WIDE_REPORT_TYPES: Set<string>;
}>('platform')!;

const showPanel = inject<(name: string) => void>('showPanel')!;

// ── Emits ─────────────────────────────────────────────────────────────────────

const emit = defineEmits<{
  (e: 'open-story-form', story: Story | null): void;
}>();

// ── Report descriptions ───────────────────────────────────────────────────────

const reportDescriptions: Record<string, string> = {
  'analysis': 'Combined analysis: categories, BISAC, keywords, and positioning.',
  'genres_and_categories': 'Genre ranking with KDP category matching for Kindle and Paperback.',
  'genre_analysis': 'Industry genre classification, KDP paths, comps, and reader demographic.',
  'full_report': 'Genre analysis with competition status.',
  'kdp_keywords': 'The 7 keyword strings optimized for KDP discoverability.',
  'mi_search_terms': 'Short search phrases used for competition and review analysis.',
  'competition_report': 'Market landscape: how competitive the niche is, who dominates.',
  'category_finder': 'Category matching results with live discoverability scores.',
  'genre_ranking': 'Each genre scored independently against the manuscript.',
  'bisac_classification': 'BISAC subject codes for KDP Print and Ingram distribution.',
  'review_mining': 'Reader insights extracted from competitor book reviews.',
  'author_analysis': 'Competitor author catalog strategy: pricing, release cadence.',
  'chapter_summaries': 'Genre signal extraction from each chapter of the manuscript.',
  'discovery_keywords': 'Keyword phrases for non-Amazon platforms.',
  'keyword_search': 'Amazon keyword volume and competition data.',
  'mapped_categories': 'Verified KDP category paths with live bestseller stats.',
};

// ── Computed reports list ─────────────────────────────────────────────────────

interface ReportListItem {
  docType: string;
  label: string;
  count: number;
  hasCurrent: boolean;
  firstSavedId: number | null;
  description: string;
}

const reportsList = computed<ReportListItem[]>(() => {
  const docs = reportsCtx.reports.value;
  const saved = reportsCtx.savedReports.value;
  const visibleTypes = platformCtx.platform.value === 'kdp'
    ? platformCtx.KDP_REPORT_TYPES
    : platformCtx.WIDE_REPORT_TYPES;

  const typeMap: Record<string, { doc: DocMeta | null; savedCount: number; firstSavedId: number | null }> = {};

  for (const doc of docs) {
    typeMap[doc.doc_type] = { doc, savedCount: 0, firstSavedId: null };
  }
  for (const s of saved) {
    if (!typeMap[s.doc_type]) {
      typeMap[s.doc_type] = { doc: null, savedCount: 0, firstSavedId: null };
    }
    typeMap[s.doc_type].savedCount++;
    if (typeMap[s.doc_type].firstSavedId === null) {
      typeMap[s.doc_type].firstSavedId = s.id;
    }
  }

  const items: ReportListItem[] = [];
  for (const [docType, info] of Object.entries(typeMap)) {
    if (!visibleTypes.has(docType)) continue;
    const label = info.doc?.label || saved.find(s => s.doc_type === docType)?.label || docType;
    const totalCount = (info.doc ? 1 : 0) + info.savedCount;
    items.push({
      docType,
      label,
      count: totalCount,
      hasCurrent: !!info.doc,
      firstSavedId: info.firstSavedId,
      description: reportDescriptions[docType] || '',
    });
  }
  return items;
});

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

async function onReportClick(item: ReportListItem): Promise<void> {
  const folder = storiesCtx.activeFolder.value;
  if (!folder) return;
  showPanel('reports');
  if (item.hasCurrent) {
    await reportsCtx.openReport(folder, item.docType);
  } else if (item.firstSavedId !== null) {
    await reportsCtx.openSavedReport(item.firstSavedId);
  }
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
          >✎</button>
        </div>
      </div>
    </div>

    <!-- Reports section -->
    <div class="reports-section">
      <div class="sidebar-section-header">Reports</div>
      <div v-if="!storiesCtx.activeFolder.value" class="sidebar-hint">
        Select a story to see reports.
      </div>
      <div v-else-if="reportsList.length === 0" class="sidebar-hint">
        No reports yet. Click Get Reports.
      </div>
      <div
        v-for="item in reportsList"
        :key="item.docType"
        class="sidebar-report-item"
        :title="item.description"
        @click="onReportClick(item)"
      >
        <span class="report-item-label">{{ item.label }}</span>
        <span v-if="item.count > 1" class="report-count">{{ item.count }}</span>
      </div>
    </div>

    <!-- Settings at bottom -->
    <div class="nav-section settings-section">
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

.sidebar-report-item {
  padding: 6px 10px;
  font-size: 12px;
  display: flex;
  align-items: center;
  justify-content: space-between;
  color: var(--text);
  cursor: pointer;
  border-radius: var(--radius);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.sidebar-report-item:hover {
  background: var(--surface2);
  color: var(--accent);
}

.report-item-label {
  overflow: hidden;
  text-overflow: ellipsis;
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

.sidebar-hint {
  padding: 8px 10px;
  font-size: 11px;
  color: var(--text-muted);
}
</style>
