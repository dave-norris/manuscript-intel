<script setup lang="ts">
import { ref, watch, onMounted, provide } from 'vue';
import { useStories } from './composables/useStories';
import { useAnalysis } from './composables/useAnalysis';
import { usePlatform } from './composables/usePlatform';
import { useSettings } from './composables/useSettings';
import { useReports } from './composables/useReports';
import { useSeries } from './composables/useSeries';
import type { Story } from './types';

import TitleBar from './components/TitleBar.vue';
import Sidebar from './components/Sidebar.vue';
import AnalyzerPanel from './components/AnalyzerPanel.vue';
import ReportsViewer from './components/ReportsViewer.vue';
import SettingsPanel from './components/SettingsPanel.vue';
import StoryForm from './components/StoryForm.vue';
import SeriesPanel from './components/SeriesPanel.vue';

// ── Composables ───────────────────────────────────────────────────────────────

const storiesCtx = useStories();
const analysisCtx = useAnalysis();
const platformCtx = usePlatform();
const settingsCtx = useSettings();
const reportsCtx = useReports();
const seriesCtx = useSeries();

provide('stories', storiesCtx);
provide('analysis', analysisCtx);
provide('platform', platformCtx);
provide('settings', settingsCtx);
provide('reports', reportsCtx);
provide('series', seriesCtx);

// ── Panel state ───────────────────────────────────────────────────────────────

type Panel = 'analyzer' | 'reports' | 'settings' | 'story-form' | 'series';
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

provide('showPanel', showPanel);

// ── Story form state ──────────────────────────────────────────────────────────

const editingStory = ref<Story | null>(null);

function openStoryForm(story: Story | null): void {
  editingStory.value = story;
  showPanel('story-form');
}

// ── Watchers ──────────────────────────────────────────────────────────────────

// When active story changes, refresh analysis state and reports
watch(() => storiesCtx.activeStoryId.value, (id) => {
  if (id && storiesCtx.activeFolder.value) {
    analysisCtx.refreshState(storiesCtx.activeFolder.value);
    reportsCtx.loadReports(storiesCtx.activeFolder.value);
  } else {
    analysisCtx.refreshState('');
    reportsCtx.loadReports('');
  }
});

// When platform changes, reload reports list
watch(() => platformCtx.platform.value, () => {
  if (storiesCtx.activeFolder.value) {
    reportsCtx.loadReports(storiesCtx.activeFolder.value);
  }
});

// When analysis finishes, refresh state and reports
watch(() => analysisCtx.isWorking.value, (working, wasWorking) => {
  if (wasWorking && !working && storiesCtx.activeFolder.value) {
    analysisCtx.refreshState(storiesCtx.activeFolder.value);
    reportsCtx.loadReports(storiesCtx.activeFolder.value);
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
  storiesCtx.loadStories();
});
</script>

<template>
  <div id="app-root">
    <TitleBar />
    <Sidebar @open-story-form="openStoryForm" />
    <main id="main">
      <AnalyzerPanel v-if="activePanel === 'analyzer'" />
      <ReportsViewer v-if="activePanel === 'reports'" />
      <SettingsPanel v-if="activePanel === 'settings'" />
      <StoryForm v-if="activePanel === 'story-form'" :story="editingStory" />
      <SeriesPanel v-if="activePanel === 'series'" />
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
