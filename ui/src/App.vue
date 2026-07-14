<script setup lang="ts">
import { ref, watch, onMounted, provide } from 'vue';
import { useStories } from './composables/useStories';
import { useAnalysis } from './composables/useAnalysis';
import { usePlatform } from './composables/usePlatform';
import { useSettings } from './composables/useSettings';
import { useReports } from './composables/useReports';

// ── Composables ───────────────────────────────────────────────────────────────

const storiesCtx = useStories();
const analysisCtx = useAnalysis();
const platformCtx = usePlatform();
const settingsCtx = useSettings();
const reportsCtx = useReports();

provide('stories', storiesCtx);
provide('analysis', analysisCtx);
provide('platform', platformCtx);
provide('settings', settingsCtx);
provide('reports', reportsCtx);

// ── Panel state ───────────────────────────────────────────────────────────────

type Panel = 'analyzer' | 'reports' | 'settings' | 'story-form';
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
    <!-- TitleBar -->
    <header id="titlebar" data-tauri-drag-region>
      <span class="titlebar-label">Manuscript Intel</span>
    </header>

    <!-- Sidebar -->
    <aside id="sidebar">
      <!-- TODO: SidebarComponent -->
    </aside>

    <!-- Main content area -->
    <main id="main">
      <!-- TODO: AnalyzerPanel -->
      <div v-if="activePanel === 'analyzer'">
        <!-- AnalyzerPanel component goes here -->
      </div>

      <!-- TODO: ReportsViewer -->
      <div v-if="activePanel === 'reports'">
        <!-- ReportsViewer component goes here -->
      </div>

      <!-- TODO: SettingsPanel -->
      <div v-if="activePanel === 'settings'">
        <!-- SettingsPanel component goes here -->
      </div>

      <!-- TODO: StoryForm -->
      <div v-if="activePanel === 'story-form'">
        <!-- StoryForm component goes here -->
      </div>
    </main>
  </div>
</template>

<style scoped>
#app-root {
  display: grid;
  grid-template-rows: 32px 1fr;
  grid-template-columns: 220px 1fr;
  grid-template-areas:
    "titlebar titlebar"
    "sidebar main";
  height: 100vh;
  overflow: hidden;
}

#titlebar {
  grid-area: titlebar;
  display: flex;
  align-items: center;
  padding: 0 12px;
  user-select: none;
  -webkit-user-select: none;
  background: var(--bg-titlebar, #1a1a2e);
}

.titlebar-label {
  font-size: 12px;
  font-weight: 600;
  color: var(--text-muted, #aaa);
}

#sidebar {
  grid-area: sidebar;
  overflow-y: auto;
  background: var(--bg-sidebar, #16162a);
  border-right: 1px solid var(--border, #2a2a4a);
}

#main {
  grid-area: main;
  overflow-y: auto;
  padding: 20px;
  background: var(--bg-main, #1e1e3a);
}
</style>
