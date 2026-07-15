<script setup lang="ts">
import { inject, computed, ref } from 'vue';
import type { Ref, ComputedRef } from 'vue';
import type { Story, DocMeta, ReportEnvelope } from '../types';

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
  currentReport: Ref<ReportEnvelope | null>;
  loadReports: (folder: string) => Promise<void>;
  openReport: (id: number) => Promise<ReportEnvelope>;
  deleteReport: (id: number) => Promise<void>;
  closeReport: () => void;
}>('reports')!;

const platformCtx = inject<{
  platform: Ref<'kdp' | 'wide' | 'craft'>;
  KDP_REPORT_TYPES: Set<string>;
  WIDE_REPORT_TYPES: Set<string>;
  CRAFT_REPORT_TYPES: Set<string>;
}>('platform')!;

const showPanel = inject<(name: string) => void>('showPanel')!;

// ── Emits ─────────────────────────────────────────────────────────────────────

const emit = defineEmits<{
  (e: 'open-story-form', story: Story | null): void;
}>();

// ── Report type definitions ───────────────────────────────────────────────────

const ALL_REPORT_TYPES: { docType: string; label: string; description: string }[] = [
  { docType: 'analysis', label: 'Full Analysis', description: 'Combined analysis: categories, BISAC, keywords, and positioning.' },
  { docType: 'genres_and_categories', label: 'Find Genres & Categories', description: 'Genre ranking with KDP category matching.' },
  { docType: 'genre_analysis', label: 'Genre Analysis', description: 'Industry genre classification, KDP paths, comps, and reader demographic.' },
  { docType: 'full_report', label: 'Full Report', description: 'Genre analysis with competition status.' },
  { docType: 'kdp_keywords', label: 'KDP Keywords', description: 'The 7 keyword strings optimized for KDP discoverability.' },
  { docType: 'mi_search_terms', label: 'Search Terms', description: 'Short search phrases used for competition analysis.' },
  { docType: 'competition_report', label: 'Competition Analysis', description: 'Market landscape: how competitive, who dominates.' },
  { docType: 'category_finder', label: 'Category Finder', description: 'Category matching results with discoverability scores.' },
  { docType: 'genre_ranking', label: 'Genre Ranking', description: 'Each genre scored independently against the manuscript.' },
  { docType: 'bisac_classification', label: 'BISAC Classification', description: 'BISAC subject codes for KDP Print and Ingram.' },
  { docType: 'review_mining', label: 'Reader Review Intelligence', description: 'Reader insights from competitor reviews.' },
  { docType: 'author_analysis', label: 'Competitor Author Analysis', description: 'Competitor pricing, release cadence, series.' },
  { docType: 'chapter_summaries', label: 'Chapter Summaries', description: 'Genre signal extraction from each chapter.' },
  { docType: 'discovery_keywords', label: 'Discovery Keywords', description: 'Keywords for non-Amazon platforms.' },
  { docType: 'keyword_search', label: 'Keyword Search Results', description: 'Amazon keyword volume and competition data.' },
  { docType: 'activity_log', label: 'Activity Log', description: 'Log output from the last analysis run.' },
  { docType: 'zeigarnik_analysis', label: 'Zeigarnik Effect', description: 'Analyzes open loops and unresolved tension to maintain reader engagement.' },
  { docType: 'continuity_check', label: 'Continuity Check', description: 'AI-assisted scan for contradicted facts — within a manuscript or across a series.' },
];

// ── Expand/collapse state ─────────────────────────────────────────────────────

const expanded = ref<string | null>(null);

function toggleExpand(docType: string): void {
  expanded.value = expanded.value === docType ? null : docType;
}

// ── Computed: visible report types with version counts ────────────────────────

interface VisibleReportType {
  docType: string;
  label: string;
  description: string;
  count: number;
  versions: DocMeta[];
}

const visibleTypes = computed<VisibleReportType[]>(() => {
  const p = platformCtx.platform.value;
  const allowedTypes = p === 'kdp' ? platformCtx.KDP_REPORT_TYPES
    : p === 'wide' ? platformCtx.WIDE_REPORT_TYPES
    : platformCtx.CRAFT_REPORT_TYPES;

  const docs = reportsCtx.reports.value;

  // Group docs by doc_type
  const versionsByType = new Map<string, DocMeta[]>();
  for (const doc of docs) {
    if (!versionsByType.has(doc.doc_type)) {
      versionsByType.set(doc.doc_type, []);
    }
    versionsByType.get(doc.doc_type)!.push(doc);
  }

  // Sort each group by generated_at descending (newest first)
  for (const versions of versionsByType.values()) {
    versions.sort((a, b) => new Date(b.generated_at).getTime() - new Date(a.generated_at).getTime());
  }

  return ALL_REPORT_TYPES
    .filter(t => allowedTypes.has(t.docType))
    .map(t => {
      const versions = versionsByType.get(t.docType) || [];
      return {
        docType: t.docType,
        label: t.label,
        description: t.description,
        count: versions.length,
        versions,
      };
    });
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

async function onVersionClick(id: number): Promise<void> {
  await reportsCtx.openReport(id);
  showPanel('reports');
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

    <!-- Reports section -->
    <div class="reports-section">
      <div class="sidebar-section-header">Reports</div>
      <div v-if="!storiesCtx.activeFolder.value" class="sidebar-hint">
        Select a story to see reports.
      </div>
      <template v-else>
        <div
          v-for="type in visibleTypes"
          :key="type.docType"
          class="report-type"
        >
          <div
            class="report-type-header"
            :title="type.description"
            @click="toggleExpand(type.docType)"
          >
            <span class="report-type-label">
              {{ type.label }}
            </span>
            <span class="report-count">{{ type.count }}</span>
          </div>

          <!-- Expanded: show versions -->
          <div
            v-if="expanded === type.docType && type.versions.length > 0"
            class="report-versions"
          >
            <div
              v-for="version in type.versions"
              :key="version.id"
              class="report-version-item"
              @click="onVersionClick(version.id)"
            >
              {{ formatTimestamp(version.generated_at) }}
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
  padding: 4px 10px;
  font-size: 11px;
  color: var(--text-muted);
  cursor: pointer;
  border-radius: var(--radius);
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
</style>
