<script setup lang="ts">
import { inject, ref, watch, type Ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { storiesKey, reportsKey, platformKey, showPanelKey, openManuscriptEditorKey, seriesKey } from '../injectionKeys';
import type { Story, Series } from '../types';
import FileTreeNodes, { type FileTreeEntry } from './FileTreeNodes.vue';

// ── Injections ────────────────────────────────────────────────────────────────

const storiesCtx = inject(storiesKey)!;
const reportsCtx = inject(reportsKey)!;
const platformCtx = inject(platformKey)!;
const showPanel = inject(showPanelKey)!;
const openManuscriptEditor = inject(openManuscriptEditorKey)!;
const seriesCtx = inject(seriesKey)!;

const appMode = inject<Ref<'analyzer' | 'writing'>>('appMode')!;
const setAppMode = inject<(mode: 'analyzer' | 'writing') => void>('setAppMode')!;
const openInWritingMode = inject<(filePath: string, title: string) => void>('openInWritingMode')!;
const openNewDocumentForm = inject<(location?: string) => void>('openNewDocumentForm')!;
const fileTreeTick = inject<Ref<number>>('fileTreeTick')!;

// ── Emits ─────────────────────────────────────────────────────────────────────

const emit = defineEmits<{
  (e: 'open-story-form', story: Story | null): void;
  (e: 'open-series-form', series: Series | null): void;
}>();

// ── Sidebar mode toggle ───────────────────────────────────────────────────────

type SidebarMode = 'files' | 'reports';
const sidebarMode = ref<SidebarMode>('files');

// ── File tree state ───────────────────────────────────────────────────────────

const fileTree = ref<FileTreeEntry[]>([]);
const expandedDirs = ref<Set<string>>(new Set());

function relativeLocation(absolutePath: string): string {
  const root = storiesCtx.activeFolder.value.replace(/[/\\]+$/, '');
  const full = absolutePath.replace(/\\/g, '/');
  const base = root.replace(/\\/g, '/');
  if (full === base) return '';
  if (full.startsWith(base + '/')) return full.slice(base.length + 1);
  return absolutePath;
}

async function loadFileTree(): Promise<void> {
  const folder = storiesCtx.activeFolder.value;
  if (!folder) { fileTree.value = []; return; }
  try {
    fileTree.value = await invoke<FileTreeEntry[]>('list_manuscript_files', { folder });
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
  if (entry.is_dir) {
    toggleDir(entry.path);
    return;
  }
  if (appMode.value === 'writing') {
    openInWritingMode(entry.path, entry.name.replace(/\.md$/, ''));
  } else {
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
}

function onAddInFolder(entry: FileTreeEntry): void {
  openNewDocumentForm(relativeLocation(entry.path));
}

function onAddDocument(): void {
  openNewDocumentForm();
}

watch(() => storiesCtx.activeFolder.value, (folder) => {
  if (folder) loadFileTree();
  else fileTree.value = [];
}, { immediate: true });

watch(fileTreeTick, () => {
  if (storiesCtx.activeFolder.value) loadFileTree();
});

watch(sidebarMode, (mode) => {
  if (mode === 'files' && storiesCtx.activeFolder.value) loadFileTree();
});

watch(appMode, (mode) => {
  if (mode === 'writing' && storiesCtx.activeFolder.value) {
    sidebarMode.value = 'files';
    loadFileTree();
  }
});

// If stories load with a saved selection, keep Files visible
watch(() => storiesCtx.stories.value.length, (n) => {
  if (n > 0 && !storiesCtx.activeStoryId.value) {
    storiesCtx.setActiveStory(storiesCtx.stories.value[0].id);
  }
});

// ── Report expand/collapse state ──────────────────────────────────────────────

const expanded = ref<string | null>(null);

function toggleExpand(docType: string): void {
  expanded.value = expanded.value === docType ? null : docType;
}

// ── Handlers ──────────────────────────────────────────────────────────────────

function onStoryClick(story: Story): void {
  storiesCtx.setActiveStory(story.id);
  sidebarMode.value = 'files';
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
    <div class="nav-section mode-tabs">
      <button class="mode-tab" :class="{ active: appMode === 'analyzer' }" @click="setAppMode('analyzer')">Analyzer</button>
      <button class="mode-tab" :class="{ active: appMode === 'writing' }" @click="setAppMode('writing'); sidebarMode = 'files'">Writing</button>
    </div>

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

    <div v-if="!storiesCtx.activeFolder.value" class="sidebar-hint files-hint">
      Select a story to browse files.
    </div>

    <div v-if="storiesCtx.activeFolder.value && appMode === 'analyzer'" class="mode-toggle-row">
      <div class="mode-toggle">
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
      <button
        v-if="sidebarMode === 'files'"
        class="btn-new-story"
        title="New document"
        @click="onAddDocument"
      >+</button>
    </div>

    <div v-if="(sidebarMode === 'files' || appMode === 'writing') && storiesCtx.activeFolder.value" class="files-section">
      <div v-if="appMode === 'writing'" class="nav-label-row files-header">
        <span class="nav-label">Files</span>
        <button class="btn-new-story" title="New document" @click="onAddDocument">+</button>
      </div>
      <div v-if="fileTree.length === 0" class="sidebar-hint">No documents yet. Click + to create one.</div>
      <FileTreeNodes
        v-else
        :entries="fileTree"
        :expanded="expandedDirs"
        @toggle="toggleDir"
        @open="onFileClick"
        @add="onAddInFolder"
      />
    </div>

    <div v-if="sidebarMode === 'reports' && appMode === 'analyzer'" class="reports-section">
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

    <div v-if="appMode === 'analyzer'" class="series-section">
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

.mode-tabs {
  display: flex;
  gap: 0;
  padding: 8px 10px;
}

.mode-tab {
  flex: 1;
  background: none;
  border: none;
  border-bottom: 2px solid transparent;
  color: var(--text-muted);
  font-size: 12px;
  font-weight: 600;
  padding: 6px 0;
  cursor: pointer;
  text-align: center;
  transition: color 0.15s, border-color 0.15s;
}

.mode-tab:hover {
  color: var(--text);
}

.mode-tab.active {
  color: var(--accent);
  border-bottom-color: var(--accent);
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
  flex-shrink: 0;
}

.btn-new-story:hover {
  color: var(--accent);
  border-color: var(--accent);
}

.stories-section {
  padding: 0;
  flex: 0 0 auto;
  max-height: 28vh;
  overflow-y: auto;
  min-height: 0;
}

.stories-list {
  padding: 0 8px 8px;
}

.files-section {
  flex: 1 1 auto;
  overflow-y: auto;
  padding: 0 6px;
  min-height: 120px;
}

.files-header {
  padding: 4px 8px 6px;
}

.series-section {
  padding: 0;
  flex: 0 0 auto;
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

.mode-toggle-row {
  display: flex;
  align-items: center;
  gap: 6px;
  margin: 4px 10px 8px;
}

.mode-toggle {
  display: flex;
  flex: 1;
  min-width: 0;
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

.reports-section {
  flex: 1;
  overflow-y: auto;
  min-height: 0;
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

.files-hint {
  flex: 1;
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
