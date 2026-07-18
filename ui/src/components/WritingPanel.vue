<script setup lang="ts">
import { ref, watch, computed, inject } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import ChapterEditor from './ChapterEditor.vue';
import AiChat from './AiChat.vue';

const props = defineProps<{
  filePath: string;
  chapterTitle: string;
  storyFolder: string;
}>();

const openNewDocumentForm = inject<() => void>('openNewDocumentForm')!;
const closeWritingDocument = inject<() => void>('closeWritingDocument')!;
const bumpFileTree = inject<() => void>('bumpFileTree')!;

const editorRef = ref<InstanceType<typeof ChapterEditor> | null>(null);
const chapterText = ref('');
const deleting = ref(false);

const selectedText = computed(() => editorRef.value?.selectedText || '');
const pinnedCount = computed(() => editorRef.value?.pinnedCount || 0);

function pinSelection(): void {
  editorRef.value?.pinSelection();
}

function clearPins(): void {
  editorRef.value?.clearPins();
}

watch(() => props.filePath, async (fp) => {
  if (!fp) { chapterText.value = ''; return; }
  try {
    chapterText.value = await invoke<string>('read_chapter', { filePath: fp });
  } catch {
    chapterText.value = '';
  }
}, { immediate: true });

function onEditorSaved(): void {
  if (props.filePath) {
    invoke<string>('read_chapter', { filePath: props.filePath })
      .then(text => { chapterText.value = text; })
      .catch(() => {});
  }
}

async function onClose(): Promise<void> {
  await editorRef.value?.saveNow();
  closeWritingDocument();
}

async function onDelete(): Promise<void> {
  if (!props.filePath || !props.storyFolder) return;
  const name = props.chapterTitle || 'this document';
  if (!confirm(`Delete “${name}”? This cannot be undone.`)) return;

  deleting.value = true;
  try {
    await invoke<void>('delete_story_document', {
      storyFolder: props.storyFolder,
      filePath: props.filePath,
    });
    bumpFileTree();
    closeWritingDocument();
  } catch (e) {
    alert('Could not delete: ' + String(e));
  } finally {
    deleting.value = false;
  }
}
</script>

<template>
  <div class="writing-panel">
    <div v-if="!filePath" class="writing-empty">
      <p class="writing-empty-title">No document open</p>
      <p class="writing-empty-hint">Create a chapter, bible page, or other document to start writing.</p>
      <button class="btn" @click="openNewDocumentForm">New document</button>
    </div>
    <template v-else>
      <div class="writing-header">
        <span class="writing-chapter-title">{{ chapterTitle }}</span>
        <div class="writing-header-actions">
          <button type="button" class="hdr-btn" @click="onClose">Close</button>
          <button type="button" class="hdr-btn danger" :disabled="deleting" @click="onDelete">Delete</button>
        </div>
      </div>
      <div class="writing-body">
        <div class="writing-editor">
          <ChapterEditor
            ref="editorRef"
            :file-path="filePath"
            @saved="onEditorSaved"
          />
        </div>
        <div class="writing-chat">
          <AiChat
            :chapter-text="chapterText"
            :chapter-title="chapterTitle"
            :story-folder="storyFolder"
            :selected-text="selectedText"
            :pinned-count="pinnedCount"
            @pin="pinSelection"
            @clear-pins="clearPins"
          />
        </div>
      </div>
    </template>
  </div>
</template>

<style scoped>
.writing-panel {
  display: flex;
  flex-direction: column;
  height: 100%;
}

.writing-empty {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 8px;
  height: 100%;
  color: var(--text-muted);
  padding: 24px;
  text-align: center;
}

.writing-empty-title {
  font-size: 15px;
  font-weight: 600;
  color: var(--text);
  margin: 0;
}

.writing-empty-hint {
  font-size: 13px;
  margin: 0 0 8px;
  max-width: 280px;
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

.writing-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 12px;
  padding: 10px 16px;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}

.writing-chapter-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.writing-header-actions {
  display: flex;
  gap: 6px;
  flex-shrink: 0;
}

.hdr-btn {
  background: var(--surface2);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--text-muted);
  font-size: 12px;
  padding: 5px 10px;
  cursor: pointer;
}

.hdr-btn:hover {
  color: var(--text);
  border-color: var(--accent);
}

.hdr-btn.danger:hover {
  color: var(--danger);
  border-color: var(--danger);
}

.hdr-btn:disabled {
  opacity: 0.5;
  cursor: not-allowed;
}

.writing-body {
  flex: 1;
  display: flex;
  overflow: hidden;
}

.writing-editor {
  flex: 1;
  overflow: hidden;
}

.writing-chat {
  flex: 0 0 320px;
  overflow: hidden;
}
</style>
