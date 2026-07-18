<script setup lang="ts">
import { ref, watch, computed } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import ChapterEditor from './ChapterEditor.vue';
import AiChat from './AiChat.vue';

const props = defineProps<{
  filePath: string;
  chapterTitle: string;
  storyFolder: string;
}>();

const editorRef = ref<InstanceType<typeof ChapterEditor> | null>(null);
const chapterText = ref('');

const selectedText = computed(() => editorRef.value?.selectedText || '');
const pinnedCount = computed(() => editorRef.value?.pinnedCount || 0);

function pinSelection(): void {
  editorRef.value?.pinSelection();
}

function clearPins(): void {
  editorRef.value?.clearPins();
}

// Load chapter text for AI context when file changes
watch(() => props.filePath, async (fp) => {
  if (!fp) { chapterText.value = ''; return; }
  try {
    chapterText.value = await invoke<string>('read_chapter', { filePath: fp });
  } catch {
    chapterText.value = '';
  }
}, { immediate: true });

function onEditorSaved(): void {
  // Refresh the text for AI context after save
  if (props.filePath) {
    invoke<string>('read_chapter', { filePath: props.filePath })
      .then(text => { chapterText.value = text; })
      .catch(() => {});
  }
}
</script>

<template>
  <div class="writing-panel">
    <div v-if="!filePath" class="writing-empty">
      <p>Select a chapter from the file tree to start writing.</p>
    </div>
    <template v-else>
      <div class="writing-header">
        <span class="writing-chapter-title">{{ chapterTitle }}</span>
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
  align-items: center;
  justify-content: center;
  height: 100%;
  color: var(--text-muted);
  font-size: 14px;
}

.writing-header {
  padding: 10px 16px;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}

.writing-chapter-title {
  font-size: 13px;
  font-weight: 600;
  color: var(--text);
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
