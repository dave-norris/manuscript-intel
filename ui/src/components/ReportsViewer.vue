<script setup lang="ts">
import { inject, ref, computed, onMounted, onUnmounted } from 'vue';
import type { Ref, ComputedRef } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { renderReport } from '../reportRenderer';
import { useSettings } from '../composables/useSettings';
import type { Story, ReportEnvelope, SidebarReportGroup } from '../types';

// ── Injections ────────────────────────────────────────────────────────────────

const reportsCtx = inject<{
  sidebarGroups: Ref<SidebarReportGroup[]>;
  currentReport: Ref<ReportEnvelope | null>;
  loadSidebarReports: (folder: string, platform: string) => Promise<void>;
  deleteReport: (id: number) => Promise<void>;
  closeReport: () => void;
}>('reports')!;

const storiesCtx = inject<{
  activeStory: ComputedRef<Story | null>;
  activeFolder: ComputedRef<string>;
}>('stories')!;

const platformCtx = inject<{ platform: Ref<'kdp' | 'wide' | 'craft'> }>('platform')!;

const showPanel = inject<(name: string) => void>('showPanel')!;

const settings = useSettings();

// ── Local state ───────────────────────────────────────────────────────────────

const copyLabel = ref('Copy');
const activeSuggestion = ref('');
const loadingSuggestion = ref(false);
const suggestionError = ref('');

// ── Computed ──────────────────────────────────────────────────────────────────

const report = computed(() => reportsCtx.currentReport.value);

const renderedHtml = computed(() => {
  if (!report.value) return '';
  const storyName = storiesCtx.activeStory.value?.name || '';
  return renderReport(report.value, storyName);
});

const reportTitle = computed(() => {
  if (!report.value) return '';
  const ts = new Date(report.value.generated_at).toLocaleString(undefined, {
    month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit'
  });
  return `${report.value.label} — ${ts}`;
});

const isContinuityReport = computed(() => {
  if (!report.value || report.value.format !== 'json') return false;
  try {
    const data = JSON.parse(report.value.content);
    return data.schema === 'continuity_v1';
  } catch { return false; }
});

const isShowDontTellReport = computed(() => {
  if (!report.value || report.value.format !== 'json') return false;
  try {
    const data = JSON.parse(report.value.content);
    return data.schema === 'show_dont_tell_v1';
  } catch { return false; }
});

const showSuggestionPanel = computed(() =>
  (isContinuityReport.value || isShowDontTellReport.value) && (activeSuggestion.value || loadingSuggestion.value || suggestionError.value)
);

// ── Handlers ──────────────────────────────────────────────────────────────────

async function onCopy(): Promise<void> {
  if (!report.value) return;
  await writeText(report.value.content);
  copyLabel.value = 'Copied!';
  setTimeout(() => { copyLabel.value = 'Copy'; }, 1500);
}

async function onDelete(): Promise<void> {
  if (!report.value) return;
  if (!confirm('Delete this report version? This cannot be undone.')) return;
  try {
    await reportsCtx.deleteReport(report.value.id);
    const folder = storiesCtx.activeFolder.value;
    if (folder) await reportsCtx.loadSidebarReports(folder, platformCtx.platform.value);
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

function closeSuggestion(): void {
  activeSuggestion.value = '';
  suggestionError.value = '';
}

async function onSuggestFix(findingIndex: number): Promise<void> {
  if (!report.value || report.value.format !== 'json') return;

  const data = JSON.parse(report.value.content);
  const findings: any[] = data.findings || [];
  const finding = findings[findingIndex];
  if (!finding) return;

  const proseModel = settings.proseModel.value || settings.model.value;
  if (!proseModel) {
    suggestionError.value = 'No model selected. Set a model in Settings.';
    return;
  }

  activeSuggestion.value = '';
  suggestionError.value = '';
  loadingSuggestion.value = true;

  try {
    const result = await invoke<{ success: boolean; suggestions: string; error: string }>('suggest_continuity_fix', {
      request: {
        provider: settings.provider.value,
        api_key: settings.apiKey.value,
        model: proseModel,
        entity: finding.entity,
        attribute: finding.attribute,
        explanation: finding.explanation,
        occurrences: (finding.occurrences || []).map((o: any) => ({
          story_name: o.story_name || '',
          file: o.file || '',
          chapter_title: o.chapter_title || '',
          value: o.value || '',
          snippet: o.snippet || '',
        })),
      }
    });
    if (result.success) {
      activeSuggestion.value = result.suggestions;
    } else {
      suggestionError.value = result.error || 'Unknown error';
    }
  } catch (e) {
    suggestionError.value = String(e);
  } finally {
    loadingSuggestion.value = false;
  }
}

async function onSuggestSdtFix(chapterIndex: number, violationIndex: number): Promise<void> {
  if (!report.value || report.value.format !== 'json') return;

  const data = JSON.parse(report.value.content);
  const chapters: any[] = data.chapters || [];
  const chapter = chapters[chapterIndex];
  if (!chapter) return;
  const violations: any[] = chapter.violations || [];
  const violation = violations[violationIndex];
  if (!violation) return;

  const proseModel = settings.proseModel.value || settings.model.value;
  if (!proseModel) {
    suggestionError.value = 'No model selected. Set a model in Settings.';
    return;
  }

  activeSuggestion.value = '';
  suggestionError.value = '';
  loadingSuggestion.value = true;

  try {
    const result = await invoke<{ success: boolean; suggestions: string; error: string }>('suggest_sdt_fix', {
      request: {
        provider: settings.provider.value,
        api_key: settings.apiKey.value,
        model: proseModel,
        telling_text: violation.telling_text,
        context: violation.context,
        why: violation.why,
        chapter_title: chapter.title || chapter.file || '',
      }
    });
    if (result.success) {
      activeSuggestion.value = result.suggestions;
    } else {
      suggestionError.value = result.error || 'Unknown error';
    }
  } catch (e) {
    suggestionError.value = String(e);
  } finally {
    loadingSuggestion.value = false;
  }
}

// ── Click delegation for "Suggest fix" links ──────────────────────────────────

const contentRef = ref<HTMLElement | null>(null);

function onContentClick(e: MouseEvent): void {
  const target = e.target as HTMLElement;
  if (target.classList.contains('suggest-fix-link')) {
    e.preventDefault();
    const idx = parseInt(target.dataset.findingIndex || '', 10);
    if (!isNaN(idx)) onSuggestFix(idx);
  }
  if (target.classList.contains('suggest-sdt-fix-link')) {
    e.preventDefault();
    const chIdx = parseInt(target.dataset.chapterIndex || '', 10);
    const vIdx = parseInt(target.dataset.violationIndex || '', 10);
    if (!isNaN(chIdx) && !isNaN(vIdx)) onSuggestSdtFix(chIdx, vIdx);
  }
}

onMounted(() => {
  contentRef.value?.addEventListener('click', onContentClick);
});

onUnmounted(() => {
  contentRef.value?.removeEventListener('click', onContentClick);
});

// ── Suggestion formatting ─────────────────────────────────────────────────────

function formatSuggestion(text: string): string {
  if (!text) return '';
  return text
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/^### (.+)$/gm, '<h4>$1</h4>')
    .replace(/^## (.+)$/gm, '<h3>$1</h3>')
    .replace(/^# (.+)$/gm, '<h2>$1</h2>')
    .replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
    .replace(/\*(.+?)\*/g, '<em>$1</em>')
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/```([\s\S]*?)```/g, '<pre>$1</pre>')
    .replace(/^- (.+)$/gm, '<li>$1</li>')
    .replace(/(<li>.*<\/li>\n?)+/g, (m) => `<ul>${m}</ul>`)
    .replace(/\n{2,}/g, '</p><p>')
    .replace(/^/, '<p>').replace(/$/, '</p>')
    .replace(/<p><\/p>/g, '')
    .replace(/<p>(<h[2-4]>)/g, '$1')
    .replace(/(<\/h[2-4]>)<\/p>/g, '$1')
    .replace(/<p>(<ul>)/g, '$1')
    .replace(/(<\/ul>)<\/p>/g, '$1')
    .replace(/<p>(<pre>)/g, '$1')
    .replace(/(<\/pre>)<\/p>/g, '$1');
}
</script>

<template>
  <div class="reports-viewer">
    <div class="reports-viewer-header">
      <span class="reports-viewer-title">{{ reportTitle }}</span>
      <div class="reports-viewer-actions">
        <button class="btn btn-sm" @click="onCopy">{{ copyLabel }}</button>
        <button class="btn btn-sm btn-danger" @click="onDelete">Delete</button>
        <button class="btn-close" @click="onClose">&times;</button>
      </div>
    </div>
    <div class="reports-viewer-body" :class="{ 'split': showSuggestionPanel }">
      <div class="reports-viewer-content" ref="contentRef" v-html="renderedHtml"></div>
      <div v-if="showSuggestionPanel" class="suggestion-panel">
        <div class="suggestion-panel-header">
          <span class="suggestion-panel-title">Suggested Fixes</span>
          <button class="btn-close" @click="closeSuggestion">&times;</button>
        </div>
        <div v-if="loadingSuggestion" class="suggestion-loading">Generating suggestions...</div>
        <div v-else-if="suggestionError" class="suggestion-error">{{ suggestionError }}</div>
        <div v-else class="suggestion-content" v-html="formatSuggestion(activeSuggestion)"></div>
      </div>
    </div>
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

.reports-viewer-body {
  flex: 1;
  overflow: hidden;
  display: flex;
}

.reports-viewer-body.split .reports-viewer-content {
  flex: 0 0 60%;
  border-right: 1px solid var(--border);
  padding-right: 16px;
}

.reports-viewer-body:not(.split) .reports-viewer-content {
  flex: 1;
}

.reports-viewer-content {
  overflow-y: auto;
  user-select: text;
}

/* Suggestion panel */
.suggestion-panel {
  flex: 0 0 40%;
  display: flex;
  flex-direction: column;
  padding-left: 16px;
  overflow: hidden;
}

.suggestion-panel-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding-bottom: 8px;
  border-bottom: 1px solid var(--border);
  margin-bottom: 10px;
  flex-shrink: 0;
}

.suggestion-panel-title {
  flex: 1;
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
}

.suggestion-loading {
  color: var(--text-muted);
  font-style: italic;
  padding: 12px 0;
}

.suggestion-error {
  color: #e74c3c;
  font-size: 12px;
  padding: 12px 0;
}

.suggestion-content {
  flex: 1;
  overflow-y: auto;
  font-size: 13px;
  line-height: 1.6;
  color: var(--text);
}

.suggestion-content :deep(pre) {
  background: var(--surface2);
  padding: 10px;
  border-radius: var(--radius);
  overflow-x: auto;
  font-size: 12px;
}

.suggestion-content :deep(code) {
  background: var(--surface2);
  padding: 1px 4px;
  border-radius: 3px;
  font-size: 12px;
}

/* Buttons */
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
</style>
