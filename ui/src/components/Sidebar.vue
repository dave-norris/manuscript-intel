<script setup lang="ts">
import { inject, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { storiesKey, reportsKey, platformKey, showPanelKey, openManuscriptEditorKey, seriesKey } from '../injectionKeys';
import type { Story, Series } from '../types';

// ── Injections ────────────────────────────────────────────────────────────────

const storiesCtx = inject(storiesKey)!;
const reportsCtx = inject(reportsKey)!;
const platformCtx = inject(platformKey)!;
const showPanel = inject(showPanelKey)!;
const openManuscriptEditor = inject(openManuscriptEditorKey)!;
const seriesCtx = inject(seriesKey)!;

// ── Emits ─────────────────────────────────────────────────────────────────────

const emit = defineEmits<{
  (e: 'open-story-form', story: Story | null): void;
  (e: 'open-series-form', series: Series | null): void;
}>();

// ── Sidebar mode toggle ───────────────────────────────────────────────────────

type SidebarMode = 'files' | 'reports';
const sidebarMode = ref<SidebarMode>('reports');

// ── File tree state ───────────────────────────────────────────────────────────

interface FileTreeEntry {
  name: string;
  path: string;
  is_dir: boolean;
  children: FileTreeEntry[];
}

const fileTree = ref<FileTreeEntry[]>([]);
const expandedDirs = ref<Set<string>>(new Set());

async function loadFileTree(): Promise<void> {
  const folder = storiesCtx.activeFolder.value;
  if (!folder) { fileTree.value = []; return; }
  try {
    fileTree.value = await invoke<FileTreeEntry[]>('list_manuscript_files', { folder });
    // Auto-expand all directories
    const expand = new Set<string>();
    function walk(entries: FileTreeEntry[]) {
      for (const e of entries) {
        if (e.is_dir) { expand.add(e.path); walk(e.children); }
      }
    }
    walk(fileTree.value);
    expandedDirs.value = expand;
  } catch (e) {
    console.error('list_manuscript_files:', e);
    fileTree.value = [];
  }
}

function toggleDir(path: string): void {
  const s = new Set(expandedDirs.value);
  if (s.has(path)) s.delete(path); else s.add(path);
  expandedDirs.value = s;
}

function onFileClick(entry: FileTreeEntry): void {
  // Open chapter in ManuscriptViewer read mode (empty findings array)
  openManuscriptEditor([{
    filePath: entry.path,
    chapterTitle: entry.name.replace(/\.md$/, ''),
    tellingText: '',
    context: '',
    why: '',
    severity: '',
    reportType: 'show_dont_tell',
  }], 0);
}

// Reload file tree when story changes
watch(() => storiesCtx.activeFolder.value, () => {
  if (sidebarMode.value === 'files') loadFileTree();
});

// Load file tree when switching to files mode
watch(sidebarMode, (mode) => {
  if (mode === 'files' && storiesCtx.activeFolder.value) loadFileTree();
});

// ── Report expand/collapse state ──────────────────────────────────────────────

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

function formatTimestamp(ts: string): string {
  return new Date(ts).toLocaleString(undefined, {
    month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit',
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

    <!-- Files / Reports toggle -->
    <div v-if="storiesCtx.activeFolder.value" class="mode-toggle">
      <button
        class="mode-btn"
        :class="{ active: sidebarMode === 'files' }"
        @click="sidebarMode = 'files'"
      >Files</button>
      <button
        class="mode-btn"
        :class="{ active: sidebarMode === 'reports' }"
        @click="sidebarMode = 'reports'"
      >Reports</button>
    </div>

    <!-- Files mode: manuscript tree -->
    <div v-if="sidebarMode === 'files' && storiesCtx.activeFolder.value" class="files-section">
      <div v-if="fileTree.length === 0" class="sidebar-hint">No chapters found.</div>
      <template v-for="entry in fileTree" :key="entry.path">
        <div v-if="entry.is_dir" class="file-tree-dir">
          <div class="file-tree-dir-header" @click="toggleDir(entry.path)">
            <span class="file-tree-arrow">{{ expandedDirs.has(entry.path) ? '▾' : '▸' }}</span>
            <span class="file-tree-dir-name">{{ entry.name }}</span>
          </div>
          <div v-if="expandedDirs.has(entry.path)" class="file-tree-children">
            <div
              v-for="child in entry.children"
              :key="child.path"
              class="file-tree-file"
              @click="onFileClick(child)"
            >{{ child.name.replace(/\.md$/, '') }}</div>
          </div>
        </div>
        <div v-else class="file-tree-file" @click="onFileClick(entry)">
          {{ entry.name.replace(/\.md$/, '') }}
        </div>
      </template>
    </div>

    <!-- Reports mode -->
    <div v-if="sidebarMode === 'reports'" class="reports-section">
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
            <span class="report-type-label">{{ type.label }}</span>
            <span class="report-count">{{ type.count }}</span>
          </div>

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

/* ── Mode toggle ───────────────────────────────────────────────────────────── */

.mode-toggle {
  display: flex;
  margin: 4px 10px 8px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  overflow: hidden;
}

.mode-btn {
  flex: 1;
  background: none;
  border: none;
  color: var(--text-muted);
  font-size: 11px;
  font-weight: 600;
  padding: 5px 0;
  cursor: pointer;
  transition: background 0.15s, color 0.15s;
}

.mode-btn:first-child {
  border-right: 1px solid var(--border);
}

.mode-btn.active {
  background: var(--accent);
  color: #fff;
}

.mode-btn:not(.active):hover {
  background: var(--surface2);
  color: var(--text);
}

/* ── File tree ─────────────────────────────────────────────────────────────── */

.files-section {
  flex: 1;
  overflow-y: auto;
  padding: 0 6px;
}

.file-tree-dir {
  margin: 0;
}

.file-tree-dir-header {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 5px 8px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text);
  cursor: pointer;
  border-radius: var(--radius);
}

.file-tree-dir-header:hover {
  background: var(--surface2);
}

.file-tree-arrow {
  font-size: 10px;
  color: var(--text-muted);
  width: 12px;
  text-align: center;
}

.file-tree-dir-name {
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.file-tree-children {
  padding-left: 16px;
}

.file-tree-file {
  padding: 4px 8px 4px 12px;
  font-size: 12px;
  color: var(--text-muted);
  cursor: pointer;
  border-radius: var(--radius);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.file-tree-file:hover {
  background: var(--surface2);
  color: var(--text);
}

/* ── Reports section ───────────────────────────────────────────────────────── */

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
