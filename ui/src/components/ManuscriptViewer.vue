<script setup lang="ts">
import { ref, computed, watch, nextTick } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../composables/useSettings';
import type { Finding } from '../types';

// ── Props ─────────────────────────────────────────────────────────────────────

const props = defineProps<{
  findings: Finding[];
  startIndex: number;
  storyFolder: string;
}>();

const emit = defineEmits<{
  (e: 'close'): void;
}>();

const settings = useSettings();

// ── State ─────────────────────────────────────────────────────────────────────

const currentIndex = ref(props.startIndex);
const chapterContent = ref('');
const loadingChapter = ref(false);
const suggestion = ref('');
const loadingSuggestion = ref(false);
const suggestionError = ref('');
const applied = ref(false);
const applyText = ref('');

// ── Computed ──────────────────────────────────────────────────────────────────

const finding = computed(() => props.findings[currentIndex.value]);
const totalFindings = computed(() => props.findings.length);

const renderedChapter = computed(() => {
  if (!chapterContent.value || !finding.value) return escHtml(chapterContent.value);

  const text = chapterContent.value;
  const target = finding.value.tellingText;
  const idx = text.indexOf(target);

  if (idx < 0) return escHtml(text);

  const before = text.substring(0, idx);
  const match = text.substring(idx, idx + target.length);
  const after = text.substring(idx + target.length);

  return escHtml(before)
    + `<mark id="highlight-target" class="mv-highlight">${escHtml(match)}</mark>`
    + escHtml(after);
});

// ── Load chapter ──────────────────────────────────────────────────────────────

async function loadChapter(): Promise<void> {
  if (!finding.value) return;
  loadingChapter.value = true;
  suggestion.value = '';
  suggestionError.value = '';
  applied.value = false;
  applyText.value = '';

  try {
    chapterContent.value = await invoke<string>('read_chapter', { filePath: finding.value.filePath });
  } catch (e) {
    chapterContent.value = 'Error loading chapter: ' + String(e);
  } finally {
    loadingChapter.value = false;
    await nextTick();
    scrollToHighlight();
  }
}

function scrollToHighlight(): void {
  const el = document.getElementById('highlight-target');
  if (el) el.scrollIntoView({ behavior: 'smooth', block: 'center' });
}

// ── Generate suggestion ───────────────────────────────────────────────────────

async function onSuggestFix(): Promise<void> {
  if (!finding.value) return;

  const proseModel = settings.modelFor('prose');
  if (!proseModel) {
    suggestionError.value = 'No model set. Go to Settings.';
    return;
  }

  suggestion.value = '';
  suggestionError.value = '';
  loadingSuggestion.value = true;

  try {
    if (finding.value.reportType === 'show_dont_tell') {
      const result = await invoke<{ success: boolean; suggestions: string; error: string }>('suggest_sdt_fix', {
        request: {
          provider: settings.provider.value,
          api_key: settings.apiKey.value,
          model: proseModel,
          telling_text: finding.value.tellingText,
          context: finding.value.context,
          why: finding.value.why,
          chapter_title: finding.value.chapterTitle,
        }
      });
      if (result.success) suggestion.value = result.suggestions;
      else suggestionError.value = result.error;
    } else if (finding.value.reportType === 'continuity') {
      const result = await invoke<{ success: boolean; suggestions: string; error: string }>('suggest_continuity_fix', {
        request: {
          provider: settings.provider.value,
          api_key: settings.apiKey.value,
          model: proseModel,
          entity: finding.value.entity || '',
          attribute: finding.value.attribute || '',
          explanation: finding.value.explanation || '',
          occurrences: finding.value.occurrences || [],
        }
      });
      if (result.success) suggestion.value = result.suggestions;
      else suggestionError.value = result.error;
    }
  } catch (e) {
    suggestionError.value = String(e);
  } finally {
    loadingSuggestion.value = false;
  }
}

// ── Apply fix ─────────────────────────────────────────────────────────────────

async function onApply(newText: string): Promise<void> {
  if (!finding.value) return;
  try {
    const updated = await invoke<string>('write_manuscript_fix', {
      filePath: finding.value.filePath,
      oldText: finding.value.tellingText,
      newText,
    });
    chapterContent.value = updated;
    applied.value = true;
    await nextTick();
    scrollToHighlight();
  } catch (e) {
    suggestionError.value = 'Apply failed: ' + String(e);
  }
}

// ── Navigation ────────────────────────────────────────────────────────────────

function onPrev(): void {
  if (currentIndex.value > 0) {
    currentIndex.value--;
  }
}

function onNext(): void {
  if (currentIndex.value < totalFindings.value - 1) {
    currentIndex.value++;
  }
}

function onClose(): void {
  emit('close');
}

// ── Watch index changes to reload chapter ─────────────────────────────────────

watch(currentIndex, () => loadChapter(), { immediate: true });

// ── Helpers ───────────────────────────────────────────────────────────────────

function escHtml(s: string): string {
  return s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
}

function formatSuggestion(text: string): string {
  if (!text) return '';
  return text
    .replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;')
    .replace(/^### (.+)$/gm, '<h4>$1</h4>')
    .replace(/^## (.+)$/gm, '<h3>$1</h3>')
    .replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
    .replace(/\*(.+?)\*/g, '<em>$1</em>')
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    .replace(/```([\s\S]*?)```/g, '<pre>$1</pre>')
    .replace(/^- (.+)$/gm, '<li>$1</li>')
    .replace(/(<li>.*<\/li>\n?)+/g, (m) => `<ul>${m}</ul>`)
    .replace(/\n{2,}/g, '</p><p>')
    .replace(/^/, '<p>').replace(/$/, '</p>')
    .replace(/<p><\/p>/g, '')
    .replace(/<p>(<h[3-4]>)/g, '$1')
    .replace(/(<\/h[3-4]>)<\/p>/g, '$1')
    .replace(/<p>(<ul>)/g, '$1')
    .replace(/(<\/ul>)<\/p>/g, '$1')
    .replace(/<p>(<pre>)/g, '$1')
    .replace(/(<\/pre>)<\/p>/g, '$1');
}
</script>

<template>
  <div class="manuscript-viewer">
    <!-- Header -->
    <div class="mv-header">
      <div class="mv-header-left">
        <button class="mv-back" @click="onClose">&larr; Back to report</button>
        <span class="mv-chapter-title">{{ finding?.chapterTitle || '' }}</span>
      </div>
      <div class="mv-nav">
        <span class="mv-nav-label">{{ currentIndex + 1 }} of {{ totalFindings }}</span>
        <button class="mv-nav-btn" :disabled="currentIndex === 0" @click="onPrev">&larr; Prev</button>
        <button class="mv-nav-btn" :disabled="currentIndex === totalFindings - 1" @click="onNext">Next &rarr;</button>
      </div>
    </div>

    <!-- Body: manuscript + suggestion panel -->
    <div class="mv-body">
      <!-- Left: chapter prose -->
      <div class="mv-chapter">
        <div v-if="loadingChapter" class="mv-loading">Loading chapter...</div>
        <div v-else class="mv-prose" v-html="renderedChapter"></div>
      </div>

      <!-- Right: suggestion panel -->
      <div class="mv-suggestion-panel">
        <div class="mv-finding-info">
          <div class="mv-finding-severity" :class="'sev-' + (finding?.severity || 'minor')">{{ finding?.severity }}</div>
          <div class="mv-finding-why">{{ finding?.why }}</div>
        </div>

        <div v-if="!suggestion && !loadingSuggestion && !applied" class="mv-suggest-action">
          <button class="btn" @click="onSuggestFix" :disabled="loadingSuggestion">Suggest Fix</button>
        </div>

        <div v-if="loadingSuggestion" class="mv-loading">Generating suggestion...</div>

        <div v-if="suggestionError" class="mv-error">{{ suggestionError }}</div>

        <div v-if="suggestion && !applied" class="mv-suggestion-content">
          <div class="mv-suggestion-text" v-html="formatSuggestion(suggestion)"></div>
          <div class="mv-apply-section">
            <label class="mv-apply-label">Replace with:</label>
            <textarea v-model="applyText" class="mv-apply-input" rows="3" placeholder="Paste or type the replacement text here"></textarea>
            <div class="mv-apply-actions">
              <button class="btn" @click="onApply(applyText)" :disabled="!applyText.trim()">Apply</button>
              <button class="mv-skip-btn" @click="onNext">Skip</button>
            </div>
          </div>
        </div>

        <div v-if="applied" class="mv-applied">✓ Applied</div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.manuscript-viewer {
  display: flex;
  flex-direction: column;
  height: 100%;
}

/* Header */
.mv-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 10px 16px;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}

.mv-header-left {
  display: flex;
  align-items: center;
  gap: 12px;
}

.mv-back {
  background: none;
  border: none;
  color: var(--accent);
  font-size: 13px;
  cursor: pointer;
  padding: 4px 8px;
  border-radius: var(--radius);
}

.mv-back:hover {
  background: var(--surface2);
}

.mv-chapter-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
}

.mv-nav {
  display: flex;
  align-items: center;
  gap: 8px;
}

.mv-nav-label {
  font-size: 12px;
  color: var(--text-muted);
}

.mv-nav-btn {
  background: var(--surface2);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--text);
  font-size: 12px;
  padding: 4px 10px;
  cursor: pointer;
}

.mv-nav-btn:hover:not(:disabled) {
  border-color: var(--accent);
  color: var(--accent);
}

.mv-nav-btn:disabled {
  opacity: 0.4;
  cursor: not-allowed;
}

/* Body */
.mv-body {
  flex: 1;
  display: flex;
  overflow: hidden;
}

.mv-chapter {
  flex: 0 0 60%;
  overflow-y: auto;
  padding: 20px 24px;
  border-right: 1px solid var(--border);
}

.mv-prose {
  font-size: 14px;
  line-height: 1.8;
  color: var(--text);
  white-space: pre-wrap;
  word-wrap: break-word;
}

.mv-prose :deep(.mv-highlight) {
  background: rgba(231, 76, 60, 0.12);
  color: #e74c3c;
  font-weight: 600;
  padding: 1px 3px;
  border-radius: 2px;
}

/* Suggestion panel */
.mv-suggestion-panel {
  flex: 0 0 40%;
  overflow-y: auto;
  padding: 16px 20px;
  display: flex;
  flex-direction: column;
  gap: 12px;
}

.mv-finding-info {
  padding: 10px;
  background: var(--surface);
  border: 1px solid var(--border);
  border-radius: var(--radius);
}

.mv-finding-severity {
  font-size: 10px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  margin-bottom: 4px;
}

.mv-finding-severity.sev-minor { color: #7a7a7a; }
.mv-finding-severity.sev-moderate { color: #e0a020; }
.mv-finding-severity.sev-major { color: #e74c3c; }

.mv-finding-why {
  font-size: 13px;
  color: var(--text);
  line-height: 1.5;
}

.mv-suggest-action {
  padding: 8px 0;
}

.mv-loading {
  color: var(--text-muted);
  font-style: italic;
  font-size: 13px;
  padding: 8px 0;
}

.mv-error {
  color: #e74c3c;
  font-size: 12px;
}

.mv-suggestion-content {
  flex: 1;
}

.mv-suggestion-text {
  font-size: 13px;
  line-height: 1.6;
  color: var(--text);
}

.mv-suggestion-text :deep(pre) {
  background: var(--surface2);
  padding: 10px;
  border-radius: var(--radius);
  overflow-x: auto;
  font-size: 12px;
}

.mv-suggestion-text :deep(code) {
  background: var(--surface2);
  padding: 1px 4px;
  border-radius: 3px;
  font-size: 12px;
}

.mv-applied {
  color: #27ae60;
  font-weight: 600;
  font-size: 13px;
}

.mv-apply-section {
  margin-top: 12px;
  padding-top: 12px;
  border-top: 1px solid var(--border);
}

.mv-apply-label {
  font-size: 11px;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--text-muted);
  margin-bottom: 4px;
  display: block;
}

.mv-apply-input {
  width: 100%;
  background: var(--surface2);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--text);
  font-size: 13px;
  line-height: 1.5;
  padding: 8px 10px;
  resize: vertical;
  font-family: inherit;
}

.mv-apply-actions {
  display: flex;
  gap: 8px;
  margin-top: 8px;
  align-items: center;
}

.mv-skip-btn {
  background: none;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--text-muted);
  font-size: 12px;
  padding: 6px 12px;
  cursor: pointer;
}

.mv-skip-btn:hover {
  border-color: var(--accent);
  color: var(--text);
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

.btn:hover { background: var(--accent-dim); }
.btn:disabled { background: var(--surface2); color: var(--text-muted); cursor: not-allowed; }
</style>
