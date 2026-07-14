<script setup lang="ts">
import { inject, ref, watch, nextTick, computed } from 'vue';
import type { Ref } from 'vue';
import type { LogLine } from '../types';

const analysisCtx = inject<{
  logLines: Ref<LogLine[]>;
  clearLog: () => void;
}>('analysis')!;

const paneRef = ref<HTMLElement | null>(null);
const userScrolledUp = ref(false);

function onScroll(): void {
  if (!paneRef.value) return;
  const el = paneRef.value;
  const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 30;
  userScrolledUp.value = !atBottom;
}

watch(
  () => analysisCtx.logLines.value.length,
  async () => {
    if (!userScrolledUp.value) {
      await nextTick();
      if (paneRef.value) {
        paneRef.value.scrollTop = paneRef.value.scrollHeight;
      }
    }
  }
);

function renderText(text: string): string {
  // Escape HTML, then render backtick-wrapped text as inline <code>
  const escaped = text
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;');
  return escaped.replace(/`([^`]+)`/g, '<code>$1</code>');
}

const lines = computed(() => analysisCtx.logLines.value);
</script>

<template>
  <div class="log-stream-container">
    <div class="log-stream-header">
      <span class="output-tab-label">Output</span>
      <button class="clear-log" @click="analysisCtx.clearLog()">Clear</button>
    </div>
    <div ref="paneRef" class="log-stream-pane" @scroll="onScroll">
      <div class="log-stream">
        <div
          v-for="(line, idx) in lines"
          :key="idx"
          class="log-line"
          :class="line.type"
        >
          <span v-if="line.icon" class="log-icon">{{ line.icon }}</span>
          <span class="log-text" v-html="renderText(line.text)"></span>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.log-stream-container {
  flex: 1;
  display: flex;
  flex-direction: column;
  min-height: 0;
  margin-top: 14px;
}

.log-stream-header {
  display: flex;
  align-items: center;
  padding: 4px 0 6px;
}

.output-tab-label {
  font-size: 11px;
  font-weight: 600;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.3px;
}

.clear-log {
  margin-left: auto;
  background: none;
  border: none;
  color: var(--text-muted);
  cursor: pointer;
  font-size: 11px;
  padding: 4px 8px;
  border-radius: 4px;
}

.clear-log:hover {
  color: var(--danger);
  background: rgba(207, 102, 121, 0.1);
}

.log-stream-pane {
  flex: 1;
  background: var(--surface2);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  overflow-y: auto;
  padding: 12px;
  min-height: 0;
}

.log-stream {
  display: flex;
  flex-direction: column;
  gap: 2px;
  user-select: text;
}

.log-line {
  display: flex;
  align-items: baseline;
  gap: 8px;
  font-family: var(--mono);
  font-size: 12px;
  line-height: 1.6;
  padding: 3px 10px;
  border-radius: 4px;
  animation: log-slide-in 0.2s ease-out;
}

@keyframes log-slide-in {
  from { opacity: 0; transform: translateY(4px); }
  to   { opacity: 1; transform: translateY(0); }
}

.log-icon {
  flex-shrink: 0;
  width: 16px;
  text-align: center;
  font-weight: 600;
}

.log-text {
  flex: 1;
  word-break: break-word;
}

.log-text :deep(code) {
  background: rgba(255, 255, 255, 0.06);
  border: 1px solid rgba(255, 255, 255, 0.08);
  border-radius: 3px;
  padding: 1px 5px;
  font-size: 11px;
  color: var(--text);
}

/* Line types */
.log-step {
  color: var(--text);
  font-weight: 600;
  margin-top: 8px;
  padding: 5px 10px;
  background: rgba(232, 97, 44, 0.08);
  border-left: 3px solid var(--accent);
}

.log-success .log-icon { color: var(--success); }
.log-success .log-text { color: var(--success); }
.log-success { background: rgba(76, 175, 125, 0.06); }

.log-error .log-icon { color: var(--danger); }
.log-error .log-text { color: var(--danger); }
.log-error { background: rgba(207, 102, 121, 0.08); }

.log-warn .log-icon { color: #f5a623; }
.log-warn .log-text { color: #dda040; }
.log-warn { background: rgba(245, 166, 35, 0.06); }

.log-item {
  color: var(--text);
  padding-left: 20px;
}
.log-item .log-icon { color: var(--accent); }

.log-detail {
  color: var(--text-muted);
  padding-left: 32px;
  font-size: 11px;
}

.log-info {
  color: var(--text-muted);
}

.log-done {
  color: var(--text);
  font-weight: 500;
  margin-bottom: 4px;
}
</style>
