<script setup lang="ts">
import { computed, inject, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { storiesKey, showPanelKey, settingsKey } from '../injectionKeys';
import type { Story, StoriesResult } from '../types';

const storiesCtx = inject(storiesKey)!;
const settingsCtx = inject(settingsKey)!;
const showPanel = inject(showPanelKey)!;

const props = defineProps<{
  story: Story | null;
}>();

/** link = existing folders on disk; create = scaffold empty structure from Settings */
type CreateMode = 'link' | 'create';

const name = ref('');
const folder = ref('');
const biblePath = ref('');
const error = ref('');
const isEditing = ref(false);
const editId = ref('');
const createMode = ref<CreateMode>('link');

const structureHint = computed(() => {
  const s = settingsCtx.folderStructure.value;
  const ms = s.manuscript || 'Manuscript';
  return [
    `${ms}/Act-1`, `${ms}/Act-2`, `${ms}/Act-3`,
    s.bible, s.characters, s.locations,
    ...(s.extra || []),
  ]
    .filter(Boolean)
    .join(', ');
});

const folderLabel = computed(() => {
  if (isEditing.value) return 'Story Folder';
  return createMode.value === 'create' ? 'Parent Folder' : 'Story Folder';
});

const folderHint = computed(() => {
  if (isEditing.value) return '';
  if (createMode.value === 'create') {
    return `A folder named after the story will be created here with ${structureHint.value}.`;
  }
  return 'Point at the existing story folder on disk.';
});

const folderPlaceholder = computed(() => {
  if (isEditing.value) return '/path/to/story';
  return createMode.value === 'create' ? '/path/to/parent' : '/path/to/existing/story';
});

watch(() => props.story, (s) => {
  if (s) {
    name.value = s.name;
    folder.value = s.folder;
    biblePath.value = s.bible_path || '';
    editId.value = s.id;
    isEditing.value = true;
  } else {
    name.value = '';
    folder.value = '';
    biblePath.value = '';
    editId.value = '';
    isEditing.value = false;
    createMode.value = 'link';
  }
  error.value = '';
}, { immediate: true });

watch(createMode, () => {
  if (!isEditing.value) {
    folder.value = '';
    error.value = '';
  }
});

async function onPickFolder(): Promise<void> {
  try {
    let title = 'Select Story Folder';
    if (!isEditing.value && createMode.value === 'create') {
      title = 'Select Parent Folder for New Story';
    }
    const path = await invoke<string>('pick_manuscript_folder', { title });
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
  if (!trimFolder) {
    error.value = createMode.value === 'create' && !isEditing.value
      ? 'Please select a parent folder.'
      : 'Please select a story folder.';
    return;
  }
  error.value = '';

  let result: StoriesResult;
  if (isEditing.value && editId.value) {
    result = await storiesCtx.updateStory(editId.value, trimName, trimFolder, biblePath.value.trim());
  } else if (createMode.value === 'create') {
    result = await storiesCtx.initStory(trimName, trimFolder);
  } else {
    result = await storiesCtx.addStory(trimName, trimFolder);
  }

  if (!result.success) {
    error.value = result.error;
    return;
  }

  const saved = isEditing.value && editId.value
    ? result.stories.find(s => s.id === editId.value)
    : [...result.stories].reverse().find(s => s.name === trimName);
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

    <div v-if="!isEditing" class="form-group">
      <label>How to add</label>
      <div class="create-options">
        <label class="create-option" :class="{ active: createMode === 'link' }">
          <input v-model="createMode" type="radio" value="link" />
          <span class="create-option-title">Link existing story</span>
          <span class="create-option-desc">Name it and choose the folder(s) that already exist on disk</span>
        </label>
        <label class="create-option" :class="{ active: createMode === 'create' }">
          <input v-model="createMode" type="radio" value="create" />
          <span class="create-option-title">Create empty story</span>
          <span class="create-option-desc">
            New folder named after the story, using Settings → Folder Structure
            ({{ structureHint }})
          </span>
        </label>
      </div>
    </div>

    <div class="form-group">
      <label>Story Name</label>
      <input v-model="name" type="text" placeholder="My Novel" />
    </div>

    <div class="form-group">
      <label>
        {{ folderLabel }}
        <span v-if="folderHint" class="form-hint"> — {{ folderHint }}</span>
      </label>
      <div class="folder-row">
        <input
          v-model="folder"
          type="text"
          :placeholder="folderPlaceholder"
          readonly
        />
        <button class="btn btn-sm" @click="onPickFolder">Browse</button>
      </div>
    </div>

    <div v-if="isEditing || createMode === 'link'" class="form-group">
      <label>
        Story Bible
        <span class="form-hint">
          (override — leave blank to auto-discover from your configured Bible/Characters folders)
        </span>
      </label>
      <input v-model="biblePath" type="text" placeholder="Auto-detected if present in story folder" />
    </div>

    <div v-if="error" class="form-error">{{ error }}</div>

    <div class="form-actions">
      <button class="btn" @click="onSave">
        {{ !isEditing && createMode === 'create' ? 'Create' : 'Save' }}
      </button>
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

.form-hint {
  text-transform: none;
  letter-spacing: 0;
  font-weight: 400;
  font-size: 11px;
}

.form-group input[type="text"],
.form-group input:not([type]) {
  background: var(--surface2);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--text);
  font-size: 13px;
  padding: 8px 10px;
  width: 100%;
  user-select: text;
}

.create-options {
  display: flex;
  flex-direction: column;
  gap: 8px;
}

.create-option {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 10px 12px 10px 32px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--surface2);
  cursor: pointer;
  position: relative;
  text-transform: none;
  letter-spacing: 0;
  color: var(--text);
}

.create-option.active {
  border-color: var(--accent);
  background: color-mix(in srgb, var(--accent) 10%, var(--surface2));
}

.create-option input[type="radio"] {
  position: absolute;
  left: 10px;
  top: 12px;
  margin: 0;
}

.create-option-title {
  font-size: 13px;
  font-weight: 600;
}

.create-option-desc {
  font-size: 11px;
  color: var(--text-muted);
  font-weight: 400;
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
