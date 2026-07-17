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
import type { Story } from './types';

import TitleBar from './components/TitleBar.vue';
import Sidebar from './components/Sidebar.vue';
import AnalyzerPanel from './components/AnalyzerPanel.vue';
import ReportsViewer from './components/ReportsViewer.vue';
import SettingsPanel from './components/SettingsPanel.vue';
import StoryForm from './components/StoryForm.vue';
import SeriesForm from './components/SeriesForm.vue';
import ManuscriptViewer from './components/ManuscriptViewer.vue';
import type { Finding } from './types';

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

// ── Panel state ───────────────────────────────────────────────────────────────

type Panel = 'analyzer' | 'reports' | 'settings' | 'story-form' | 'series' | 'manuscript';
const activePanel = ref<Panel>('analyzer');
const prevPanel = ref<Panel>('analyzer');

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

// ── Manuscript editor state ───────────────────────────────────────────────────

const manuscriptFindings = ref<Finding[]>([]);
const manuscriptStartIndex = ref(0);

function openManuscriptEditor(findings: Finding[], startIndex: number): void {
  manuscriptFindings.value = findings;
  manuscriptStartIndex.value = startIndex;
  activePanel.value = 'manuscript';
}

provide(openManuscriptEditorKey, openManuscriptEditor);

// ── Story form state ──────────────────────────────────────────────────────────

const editingStory = ref<Story | null>(null);

function openStoryForm(story: Story | null): void {
  editingStory.value = story;
  showPanel('story-form');
}

// ── Series form state ─────────────────────────────────────────────────────────

import type { Series } from './types';
const editingSeries = ref<Series | null>(null);

function openSeriesForm(series: Series | null): void {
  editingSeries.value = series;
  showPanel('series');
}

// ── Watchers ──────────────────────────────────────────────────────────────────

// When active story changes, refresh analysis state and reports
watch(() => storiesCtx.activeStoryId.value, (id) => {
  if (id && storiesCtx.activeFolder.value) {
    analysisCtx.refreshState(storiesCtx.activeFolder.value);
    reportsCtx.loadSidebarReports(storiesCtx.activeFolder.value, platformCtx.platform.value);
  } else {
    analysisCtx.refreshState('');
    reportsCtx.loadSidebarReports('', platformCtx.platform.value);
  }
});

// When platform changes, reload sidebar (backend filters by platform)
watch(() => platformCtx.platform.value, () => {
  if (storiesCtx.activeFolder.value) {
    reportsCtx.loadSidebarReports(storiesCtx.activeFolder.value, platformCtx.platform.value);
  }
});

// When analysis finishes, refresh state and reports
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

  // Load initial data
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
        @close="showPanel('reports')"
      />
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
