<script setup lang="ts">
import { inject, ref, computed, onMounted, onUnmounted } from 'vue';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { renderReport } from '../reportRenderer';
import { storiesKey, reportsKey, platformKey, showPanelKey, openManuscriptEditorKey } from '../injectionKeys';
import type { Finding } from '../types';

// ── Injections ────────────────────────────────────────────────────────────────

const reportsCtx = inject(reportsKey)!;
const storiesCtx = inject(storiesKey)!;
const platformCtx = inject(platformKey)!;
const showPanel = inject(showPanelKey)!;
const openManuscriptEditor = inject(openManuscriptEditorKey)!;

// ── Local state ───────────────────────────────────────────────────────────────

const copyLabel = ref('Copy');

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

// ── Open manuscript editor for a finding ──────────────────────────────────────

function openEditorForSdt(chapterIndex: number, violationIndex: number): void {
  if (!report.value || report.value.format !== 'json') return;
  const folder = storiesCtx.activeFolder.value;
  if (!folder) return;

  const data = JSON.parse(report.value.content);
  const chapters: any[] = data.chapters || [];

  // Build findings array from all SDT violations
  const findings: Finding[] = [];
  let targetIdx = 0;

  chapters.forEach((ch: any, chIdx: number) => {
    const violations: any[] = ch.violations || [];
    violations.forEach((v: any, vIdx: number) => {
      if (chIdx === chapterIndex && vIdx === violationIndex) {
        targetIdx = findings.length;
      }
      findings.push({
        filePath: folder + '/' + (ch.file || ''),
        chapterTitle: ch.title || ch.file || '',
        tellingText: v.telling_text || '',
        context: v.context || '',
        why: v.why || '',
        severity: v.severity || 'minor',
        reportType: 'show_dont_tell',
      });
    });
  });

  if (findings.length > 0) {
    openManuscriptEditor(findings, targetIdx);
  }
}

function openEditorForContinuity(findingIndex: number): void {
  if (!report.value || report.value.format !== 'json') return;
  const folder = storiesCtx.activeFolder.value;
  if (!folder) return;

  const data = JSON.parse(report.value.content);
  const reportFindings: any[] = data.findings || [];

  // Build findings array from continuity findings
  const findings: Finding[] = [];

  reportFindings.forEach((f: any) => {
    const occs: any[] = f.occurrences || [];
    // Use the first occurrence's file as the chapter to open
    const firstOcc = occs[0] || {};
    findings.push({
      filePath: folder + '/' + (firstOcc.file || ''),
      chapterTitle: firstOcc.chapter_title || firstOcc.file || '',
      tellingText: firstOcc.snippet || '',
      context: '',
      why: f.explanation || '',
      severity: f.verdict === 'contradiction' ? 'major' : 'moderate',
      reportType: 'continuity',
      entity: f.entity,
      attribute: f.attribute,
      explanation: f.explanation,
      occurrences: occs.map((o: any) => ({
        story_name: o.story_name || '',
        file: o.file || '',
        chapter_title: o.chapter_title || '',
        value: o.value || '',
        snippet: o.snippet || '',
      })),
    });
  });

  if (findings.length > 0) {
    openManuscriptEditor(findings, findingIndex);
  }
}

// ── Click delegation for "Suggest fix" links ──────────────────────────────────

const contentRef = ref<HTMLElement | null>(null);

function onContentClick(e: MouseEvent): void {
  const target = e.target as HTMLElement;
  if (target.classList.contains('suggest-fix-link')) {
    e.preventDefault();
    const idx = parseInt(target.dataset.findingIndex || '', 10);
    if (!isNaN(idx)) openEditorForContinuity(idx);
  }
  if (target.classList.contains('suggest-sdt-fix-link')) {
    e.preventDefault();
    const chIdx = parseInt(target.dataset.chapterIndex || '', 10);
    const vIdx = parseInt(target.dataset.violationIndex || '', 10);
    if (!isNaN(chIdx) && !isNaN(vIdx)) openEditorForSdt(chIdx, vIdx);
  }
}

onMounted(() => {
  contentRef.value?.addEventListener('click', onContentClick);
});

onUnmounted(() => {
  contentRef.value?.removeEventListener('click', onContentClick);
});
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
    <div class="reports-viewer-content" ref="contentRef" v-html="renderedHtml"></div>
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

.reports-viewer-content {
  flex: 1;
  overflow-y: auto;
  user-select: text;
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

.btn:hover { background: var(--accent-dim); }
.btn:disabled { background: var(--surface2); color: var(--text-muted); cursor: not-allowed; }

.btn-sm {
  padding: 6px 12px;
  font-size: 12px;
  white-space: nowrap;
}

.btn-danger {
  background: #c0392b;
  color: #fff;
}

.btn-danger:hover { background: #a93226; }

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
