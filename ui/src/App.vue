<script setup lang="ts">
import { ref, watch, onMounted, provide } from 'vue';
import { useStories } from './composables/useStories';
import { useAnalysis } from './composables/useAnalysis';
import { usePlatform } from './composables/usePlatform';
import { useSettings } from './composables/useSettings';
import { useReports } from './composables/useReports';
import { useSeries } from './composables/useSeries';
import {
  storiesKey, analysisKey, platformKey, settingsKey,
  reportsKey, seriesKey, showPanelKey, openManuscriptEditorKey,
} from './injectionKeys';
import type { Story, Finding, Series } from './types';

import TitleBar from './components/TitleBar.vue';
import Sidebar from './components/Sidebar.vue';
import AnalyzerPanel from './components/AnalyzerPanel.vue';
import ReportsViewer from './components/ReportsViewer.vue';
import SettingsPanel from './components/SettingsPanel.vue';
import StoryForm from './components/StoryForm.vue';
import SeriesForm from './components/SeriesForm.vue';
import NewDocumentForm from './components/NewDocumentForm.vue';
import ManuscriptViewer from './components/ManuscriptViewer.vue';
import WritingPanel from './components/WritingPanel.vue';

// ── Composables ───────────────────────────────────────────────────────────────

const storiesCtx = useStories();
const analysisCtx = useAnalysis();
const platformCtx = usePlatform();
const settingsCtx = useSettings();
const reportsCtx = useReports();
const seriesCtx = useSeries();

provide(storiesKey, storiesCtx);
provide(analysisKey, analysisCtx);
provide(platformKey, platformCtx);
provide(settingsKey, settingsCtx);
provide(reportsKey, reportsCtx);
provide(seriesKey, seriesCtx);

// ── Top-level mode ────────────────────────────────────────────────────────────

type AppMode = 'analyzer' | 'writing';
const appMode = ref<AppMode>('analyzer');

provide('appMode', appMode);
provide('setAppMode', (mode: AppMode) => { appMode.value = mode; });

// ── Panel state (within Analyzer mode) ────────────────────────────────────────

type Panel = 'analyzer' | 'reports' | 'settings' | 'story-form' | 'series' | 'manuscript' | 'new-document';
const activePanel = ref<Panel>('analyzer');
const prevPanel = ref<Panel>('analyzer');
/** Panel to restore after cancelling New Document (works across writing/analyzer). */
const panelBeforeNewDoc = ref<Panel>('analyzer');
const modeBeforeNewDoc = ref<AppMode>('analyzer');

function showPanel(name: Panel): void {
  if (name === 'settings' && activePanel.value === 'settings') {
    activePanel.value = prevPanel.value;
    return;
  }
  if (name === 'settings' && activePanel.value !== 'settings') {
    prevPanel.value = activePanel.value;
  }
  activePanel.value = name;
}

provide(showPanelKey, showPanel as (name: string) => void);

const fileTreeTick = ref(0);
provide('fileTreeTick', fileTreeTick);

// ── Manuscript editor state (for report findings) ─────────────────────────────

const manuscriptFindings = ref<Finding[]>([]);
const manuscriptStartIndex = ref(0);
/** Where to return after closing the manuscript editor. */
const manuscriptReturnPanel = ref<Panel>('analyzer');

function openManuscriptEditor(findings: Finding[], startIndex: number): void {
  manuscriptFindings.value = findings;
  manuscriptStartIndex.value = startIndex;
  // From a report → return to reports; from Files/elsewhere → analyzer.
  // If already in the manuscript editor, keep the existing return target.
  if (activePanel.value === 'reports') {
    manuscriptReturnPanel.value = 'reports';
  } else if (activePanel.value !== 'manuscript') {
    manuscriptReturnPanel.value = 'analyzer';
  }
  activePanel.value = 'manuscript';
}

function closeManuscriptEditor(): void {
  const target = manuscriptReturnPanel.value;
  // Don't land on an empty reports shell (Copy/Delete with no title)
  if (target === 'reports' && !reportsCtx.currentReport.value) {
    activePanel.value = 'analyzer';
  } else {
    activePanel.value = target;
  }
}

provide(openManuscriptEditorKey, openManuscriptEditor);

// ── Writing mode state ────────────────────────────────────────────────────────

const writingFilePath = ref('');
const writingChapterTitle = ref('');
const newDocLocation = ref<string | undefined>(undefined);

function openInWritingMode(filePath: string, title: string): void {
  writingFilePath.value = filePath;
  writingChapterTitle.value = title;
  appMode.value = 'writing';
}

provide('openInWritingMode', openInWritingMode);

function closeWritingDocument(): void {
  writingFilePath.value = '';
  writingChapterTitle.value = '';
}

provide('closeWritingDocument', closeWritingDocument);

function bumpFileTree(): void {
  fileTreeTick.value += 1;
}

provide('bumpFileTree', bumpFileTree);

function openNewDocumentForm(location?: string): void {
  if (!storiesCtx.activeFolder.value) return;
  panelBeforeNewDoc.value = activePanel.value;
  modeBeforeNewDoc.value = appMode.value;
  newDocLocation.value = location;
  activePanel.value = 'new-document';
}

provide('openNewDocumentForm', openNewDocumentForm);

function onDocumentCreated(path: string, title: string): void {
  fileTreeTick.value += 1;
  newDocLocation.value = undefined;
  writingFilePath.value = path;
  writingChapterTitle.value = title;
  appMode.value = 'writing';
  activePanel.value = 'analyzer';
}

function onDocumentFormCancel(): void {
  newDocLocation.value = undefined;
  activePanel.value = panelBeforeNewDoc.value;
  appMode.value = modeBeforeNewDoc.value;
}

// ── Story form state ──────────────────────────────────────────────────────────

const editingStory = ref<Story | null>(null);

function openStoryForm(story: Story | null): void {
  editingStory.value = story;
  showPanel('story-form');
}

// ── Series form state ─────────────────────────────────────────────────────────

const editingSeries = ref<Series | null>(null);

function openSeriesForm(series: Series | null): void {
  editingSeries.value = series;
  showPanel('series');
}

// ── Watchers ──────────────────────────────────────────────────────────────────

watch(() => storiesCtx.activeStoryId.value, (id) => {
  if (id && storiesCtx.activeFolder.value) {
    analysisCtx.refreshState(storiesCtx.activeFolder.value);
    reportsCtx.loadSidebarReports(storiesCtx.activeFolder.value, platformCtx.platform.value);
  } else {
    analysisCtx.refreshState('');
    reportsCtx.loadSidebarReports('', platformCtx.platform.value);
  }
});

watch(() => platformCtx.platform.value, () => {
  if (storiesCtx.activeFolder.value) {
    reportsCtx.loadSidebarReports(storiesCtx.activeFolder.value, platformCtx.platform.value);
  }
});

watch(() => analysisCtx.isWorking.value, (working, wasWorking) => {
  if (wasWorking && !working && storiesCtx.activeFolder.value) {
    analysisCtx.refreshState(storiesCtx.activeFolder.value);
    reportsCtx.loadSidebarReports(storiesCtx.activeFolder.value, platformCtx.platform.value);
  }
});

// ── Context menu prevention ───────────────────────────────────────────────────

onMounted(() => {
  document.addEventListener('contextmenu', (e) => {
    const tag = (e.target as HTMLElement).tagName;
    if (!['INPUT', 'TEXTAREA', 'SELECT'].includes(tag) && !(e.target as HTMLElement).isContentEditable) {
      e.preventDefault();
    }
  });

  settingsCtx.loadFolderStructure();
  storiesCtx.loadStories().then(() => {
    const folder = storiesCtx.activeFolder.value;
    if (folder) {
      analysisCtx.refreshState(folder);
      reportsCtx.loadSidebarReports(folder, platformCtx.platform.value);
    }
  });
  seriesCtx.loadSeries();
});
</script>

<template>
  <div id="app-root">
    <TitleBar />
    <Sidebar @open-story-form="openStoryForm" @open-series-form="openSeriesForm" />
    <main id="main">
      <NewDocumentForm
        v-if="activePanel === 'new-document'"
        :initial-location="newDocLocation"
        @created="onDocumentCreated"
        @cancel="onDocumentFormCancel"
      />

      <!-- Writing mode -->
      <WritingPanel
        v-else-if="appMode === 'writing'"
        :file-path="writingFilePath"
        :chapter-title="writingChapterTitle"
        :story-folder="storiesCtx.activeFolder.value"
      />

      <!-- Analyzer mode panels -->
      <template v-else-if="appMode === 'analyzer'">
        <AnalyzerPanel v-if="activePanel === 'analyzer'" />
        <ReportsViewer v-if="activePanel === 'reports'" />
        <SettingsPanel v-if="activePanel === 'settings'" />
        <StoryForm v-if="activePanel === 'story-form'" :story="editingStory" />
        <SeriesForm v-if="activePanel === 'series'" :series="editingSeries" />
        <ManuscriptViewer
          v-if="activePanel === 'manuscript'"
          :findings="manuscriptFindings"
          :start-index="manuscriptStartIndex"
          :story-folder="storiesCtx.activeFolder.value"
          @close="closeManuscriptEditor"
        />
      </template>
    </main>
  </div>
</template>

<style scoped>
#app-root {
  display: grid;
  grid-template-rows: var(--titlebar-h, 28px) 1fr;
  grid-template-columns: 200px 1fr;
  grid-template-areas:
    "titlebar titlebar"
    "sidebar main";
  height: 100vh;
  overflow: hidden;
}

#main {
  grid-area: main;
  display: flex;
  flex-direction: column;
  overflow: hidden;
}
</style>
