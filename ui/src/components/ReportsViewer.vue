<script setup lang="ts">
import { inject, ref, computed } from 'vue';
import type { Ref, ComputedRef } from 'vue';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { renderReport } from '../reportRenderer';
import type { Story, DocMeta, SavedReportMeta, ReportEnvelope } from '../types';

// ── Injections ────────────────────────────────────────────────────────────────

const reportsCtx = inject<{
  reports: Ref<DocMeta[]>;
  savedReports: Ref<SavedReportMeta[]>;
  currentReport: Ref<ReportEnvelope | null>;
  loadReports: (folder: string) => Promise<void>;
  saveVersion: (folder: string, docType: string) => Promise<SavedReportMeta>;
  deleteVersion: (id: number) => Promise<void>;
  closeReport: () => void;
}>('reports')!;

const storiesCtx = inject<{
  activeStory: ComputedRef<Story | null>;
  activeFolder: ComputedRef<string>;
}>('stories')!;

const showPanel = inject<(name: string) => void>('showPanel')!;

// ── Local state ───────────────────────────────────────────────────────────────

const copyLabel = ref('Copy');
const saveLabel = ref('Save');
const saveDisabled = ref(false);

// Track which type we're viewing — needed for save/delete
const viewingDocType = ref('');
const viewingSavedId = ref<number | null>(null);

// ── Computed ──────────────────────────────────────────────────────────────────

const report = computed(() => reportsCtx.currentReport.value);

const renderedHtml = computed(() => {
  if (!report.value) return '';
  const storyName = storiesCtx.activeStory.value?.name || '';
  return renderReport(report.value, storyName);
});

const reportTitle = computed(() => report.value?.label || '');

const isSavedVersion = computed(() => viewingSavedId.value !== null);

// Detect whether this is a current report (can Save) or saved (can Delete)
// We watch the currentReport to update tracking
import { watch } from 'vue';

watch(report, (envelope) => {
  if (!envelope) {
    viewingDocType.value = '';
    viewingSavedId.value = null;
    return;
  }
  // Determine if it's a saved version by checking savedReports
  const savedMatch = reportsCtx.savedReports.value.find(
    s => s.doc_type === envelope.doc_type && s.label === envelope.label
  );
  // If the envelope matches a doc from the current reports list, it's "current"
  const currentMatch = reportsCtx.reports.value.find(
    d => d.doc_type === envelope.doc_type
  );

  if (currentMatch && currentMatch.label === envelope.label) {
    viewingDocType.value = envelope.doc_type;
    viewingSavedId.value = null;
  } else if (savedMatch) {
    viewingDocType.value = '';
    viewingSavedId.value = savedMatch.id;
  } else {
    // Fallback: treat as current
    viewingDocType.value = envelope.doc_type;
    viewingSavedId.value = null;
  }
});

// ── Handlers ──────────────────────────────────────────────────────────────────

async function onCopy(): Promise<void> {
  if (!report.value) return;
  await writeText(report.value.content);
  copyLabel.value = 'Copied!';
  setTimeout(() => { copyLabel.value = 'Copy'; }, 1500);
}

async function onSave(): Promise<void> {
  const folder = storiesCtx.activeFolder.value;
  if (!folder || !viewingDocType.value) return;
  saveDisabled.value = true;
  saveLabel.value = 'Saving...';
  try {
    const meta = await reportsCtx.saveVersion(folder, viewingDocType.value);
    saveLabel.value = `Saved as ${meta.label}`;
    await reportsCtx.loadReports(folder);
    setTimeout(() => { saveLabel.value = 'Save'; saveDisabled.value = false; }, 2000);
  } catch (e) {
    saveLabel.value = 'Error';
    setTimeout(() => { saveLabel.value = 'Save'; saveDisabled.value = false; }, 2000);
  }
}

async function onDelete(): Promise<void> {
  if (viewingSavedId.value === null) return;
  if (!confirm('Delete this saved report version? This cannot be undone.')) return;
  try {
    await reportsCtx.deleteVersion(viewingSavedId.value);
    const folder = storiesCtx.activeFolder.value;
    if (folder) await reportsCtx.loadReports(folder);
    reportsCtx.closeReport();
    showPanel('analyzer');
  } catch (e) {
    alert('Could not delete: ' + String(e));
  }
}

function onClose(): void {
  reportsCtx.closeReport();
  showPanel('analyzer');
}
</script>

<template>
  <div class="reports-viewer">
    <div class="reports-viewer-header">
      <span class="reports-viewer-title">{{ reportTitle }}</span>
      <div class="reports-viewer-actions">
        <button
          v-if="!isSavedVersion"
          class="btn btn-sm"
          :disabled="saveDisabled"
          @click="onSave"
        >{{ saveLabel }}</button>
        <button class="btn btn-sm" @click="onCopy">{{ copyLabel }}</button>
        <button
          v-if="isSavedVersion"
          class="btn btn-sm btn-danger"
          @click="onDelete"
        >Delete</button>
        <button class="btn-close" @click="onClose">&times;</button>
      </div>
    </div>
    <div class="reports-viewer-content" v-html="renderedHtml"></div>
  </div>
</template>

<style scoped>
.reports-viewer {
  display: flex;
  flex-direction: column;
  height: 100%;
}

.reports-viewer-header {
  display: flex;
  align-items: center;
  gap: 10px;
  padding-bottom: 10px;
  border-bottom: 1px solid var(--border);
  margin-bottom: 12px;
  flex-shrink: 0;
}

.reports-viewer-title {
  flex: 1;
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.reports-viewer-actions {
  display: flex;
  gap: 6px;
  align-items: center;
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

.btn:hover {
  background: var(--accent-dim);
}

.btn:disabled {
  background: var(--surface2);
  color: var(--text-muted);
  cursor: not-allowed;
}

.btn-sm {
  padding: 6px 12px;
  font-size: 12px;
  white-space: nowrap;
}

.btn-danger {
  background: #c0392b;
  color: #fff;
}

.btn-danger:hover {
  background: #a93226;
}

.btn-close {
  background: none;
  border: none;
  color: var(--text-muted);
  font-size: 20px;
  line-height: 1;
  cursor: pointer;
  padding: 2px 6px;
  border-radius: 4px;
  margin-left: 4px;
}

.btn-close:hover {
  color: var(--text);
  background: var(--surface2);
}

.reports-viewer-content {
  flex: 1;
  overflow-y: auto;
  user-select: text;
}
</style>
