<script setup lang="ts">
import { inject, ref, watch } from 'vue';
import type { Ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { Story, StoriesResult } from '../types';

const storiesCtx = inject<{
  stories: Ref<Story[]>;
  activeStoryId: Ref<string | null>;
  setActiveStory: (id: string | null) => void;
  addStory: (name: string, folder: string) => Promise<StoriesResult>;
  updateStory: (id: string, name: string, folder: string) => Promise<StoriesResult>;
  deleteStory: (id: string) => Promise<StoriesResult>;
}>('stories')!;

const showPanel = inject<(name: string) => void>('showPanel')!;

const props = defineProps<{
  story: Story | null;
}>();

const name = ref('');
const folder = ref('');
const error = ref('');
const isEditing = ref(false);
const editId = ref('');

// Populate form when story prop changes
watch(() => props.story, (s) => {
  if (s) {
    name.value = s.name;
    folder.value = s.folder;
    editId.value = s.id;
    isEditing.value = true;
  } else {
    name.value = '';
    folder.value = '';
    editId.value = '';
    isEditing.value = false;
  }
  error.value = '';
}, { immediate: true });

async function onPickFolder(): Promise<void> {
  try {
    const path = await invoke<string>('pick_manuscript_folder');
    if (path) folder.value = path;
  } catch (e) {
    if (!String(e).includes('No folder')) {
      error.value = String(e);
    }
  }
}

async function onSave(): Promise<void> {
  const trimName = name.value.trim();
  const trimFolder = folder.value.trim();

  if (!trimName) { error.value = 'Please enter a story name.'; return; }
  if (!trimFolder) { error.value = 'Please select a folder.'; return; }
  error.value = '';

  let result: StoriesResult;
  if (isEditing.value && editId.value) {
    result = await storiesCtx.updateStory(editId.value, trimName, trimFolder);
  } else {
    result = await storiesCtx.addStory(trimName, trimFolder);
  }

  if (!result.success) {
    error.value = result.error;
    return;
  }

  // Find the saved story and activate it
  const saved = result.stories.find(s => s.name === trimName && s.folder === trimFolder);
  if (saved) storiesCtx.setActiveStory(saved.id);
  showPanel('analyzer');
}

function onCancel(): void {
  showPanel('analyzer');
}

async function onDelete(): Promise<void> {
  if (!editId.value) return;
  if (!confirm('Remove this story from the list? (The folder and files will not be deleted.)')) return;

  const result = await storiesCtx.deleteStory(editId.value);
  if (result.success) {
    showPanel('analyzer');
  } else {
    error.value = result.error;
  }
}
</script>

<template>
  <div class="panel story-form-panel">
    <h2 class="panel-title">{{ isEditing ? 'Edit Story' : 'New Story' }}</h2>

    <div class="form-group">
      <label>Story Name</label>
      <input v-model="name" type="text" placeholder="My Novel" />
    </div>

    <div class="form-group">
      <label>Manuscript Folder</label>
      <div class="folder-row">
        <input v-model="folder" type="text" placeholder="/path/to/manuscript" readonly />
        <button class="btn btn-sm" @click="onPickFolder">Browse</button>
      </div>
    </div>

    <div v-if="error" class="form-error">{{ error }}</div>

    <div class="form-actions">
      <button class="btn" @click="onSave">Save</button>
      <button class="btn btn-secondary" @click="onCancel">Cancel</button>
      <button
        v-if="isEditing"
        class="btn btn-danger"
        @click="onDelete"
      >Delete</button>
    </div>
  </div>
</template>

<style scoped>
.story-form-panel {
  padding: 20px;
  max-width: 480px;
}

.panel-title {
  font-size: 16px;
  font-weight: 700;
  margin-bottom: 16px;
}

.form-group {
  margin-bottom: 14px;
}

.form-group label {
  display: block;
  font-size: 12px;
  color: var(--text-muted);
  text-transform: uppercase;
  letter-spacing: 0.06em;
  margin-bottom: 6px;
}

.form-group input {
  background: var(--surface2);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--text);
  font-size: 13px;
  padding: 8px 10px;
  width: 100%;
  user-select: text;
}

.folder-row {
  display: flex;
  gap: 8px;
  align-items: center;
}

.folder-row input {
  flex: 1;
  font-family: var(--mono);
  font-size: 12px;
}

.form-error {
  color: var(--danger);
  font-size: 12px;
  margin-bottom: 12px;
}

.form-actions {
  display: flex;
  gap: 8px;
  margin-top: 16px;
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

.btn-sm {
  padding: 6px 12px;
  font-size: 12px;
  white-space: nowrap;
  flex-shrink: 0;
}

.btn-secondary {
  background: var(--surface2);
  border: 1px solid var(--border);
  color: var(--text-muted);
}

.btn-secondary:hover {
  color: var(--text);
  border-color: var(--accent);
}

.btn-danger {
  background: #c0392b;
  color: #fff;
}

.btn-danger:hover { background: #a93226; }
</style>
