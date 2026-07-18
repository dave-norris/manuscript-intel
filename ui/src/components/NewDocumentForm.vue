<script setup lang="ts">
import { computed, inject, ref, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { storiesKey, settingsKey } from '../injectionKeys';
import { manuscriptActPaths } from '../composables/useSettings';

const props = defineProps<{
  /** Relative path under the story folder (e.g. from a folder + click). */
  initialLocation?: string;
}>();

const emit = defineEmits<{
  (e: 'created', path: string, title: string): void;
  (e: 'cancel'): void;
}>();

const storiesCtx = inject(storiesKey)!;
const settingsCtx = inject(settingsKey)!;

interface DocTypeOption {
  id: string;
  label: string;
  path: string;
}

const docTypes = computed<DocTypeOption[]>(() => {
  const s = settingsCtx.folderStructure.value;
  const ms = s.manuscript || 'Manuscript';
  const types: DocTypeOption[] = manuscriptActPaths(ms).map((path, i) => ({
    id: `act-${i + 1}`,
    label: `Chapter (${path.split('/').pop()})`,
    path,
  }));
  types.push(
    { id: 'bible', label: 'Bible', path: s.bible || 'Bible' },
    { id: 'characters', label: 'Character', path: s.characters || 'Characters' },
    { id: 'locations', label: 'Location', path: s.locations || 'Locations' },
  );
  for (const extra of s.extra || []) {
    const path = extra.trim();
    if (!path) continue;
    const label = path.split(/[/\\]/).filter(Boolean).pop() || path;
    types.push({ id: `extra:${path}`, label, path });
  }
  return types;
});

const name = ref('');
const docTypeId = ref('act-1');
const location = ref('Manuscript/Act-1');
const error = ref('');
const saving = ref(false);
/** When true, changing type does not overwrite a folder-picked location. */
const locationFromFolder = ref(false);

const selectedType = computed(() =>
  docTypes.value.find(t => t.id === docTypeId.value) || docTypes.value[0]
);

const namePlaceholder = computed(() => {
  const id = selectedType.value?.id || '';
  if (id.startsWith('act-')) return 'Chapter 1';
  switch (id) {
    case 'bible': return 'World Rules';
    case 'characters': return 'Protagonist';
    case 'locations': return 'Primary Setting';
    default: return 'Untitled';
  }
});

function matchTypeForLocation(loc: string): string {
  const norm = loc.replace(/\\/g, '/').replace(/^\/+|\/+$/g, '');
  let best = docTypes.value[0]?.id || 'act-1';
  let bestLen = -1;
  for (const t of docTypes.value) {
    const p = t.path.replace(/\\/g, '/');
    if (norm === p || norm.startsWith(p + '/')) {
      if (p.length > bestLen) {
        best = t.id;
        bestLen = p.length;
      }
    }
  }
  return best;
}

function applyInitialLocation(loc: string | undefined): void {
  name.value = '';
  error.value = '';
  saving.value = false;
  if (loc && loc.trim()) {
    location.value = loc.trim().replace(/\\/g, '/');
    docTypeId.value = matchTypeForLocation(location.value);
    locationFromFolder.value = true;
  } else {
    locationFromFolder.value = false;
    docTypeId.value = 'act-1';
    location.value = selectedType.value?.path || 'Manuscript/Act-1';
  }
}

watch(() => props.initialLocation, (loc) => {
  applyInitialLocation(loc);
}, { immediate: true });

watch(docTypes, (types) => {
  if (!types.some(t => t.id === docTypeId.value) && types[0]) {
    docTypeId.value = types[0].id;
  }
});

function onSelectType(id: string): void {
  docTypeId.value = id;
  const t = docTypes.value.find(x => x.id === id);
  if (t && !locationFromFolder.value) {
    location.value = t.path;
  } else if (t && locationFromFolder.value) {
    // Keep folder path; type is only a hint
  }
}

function onLocationEdit(): void {
  locationFromFolder.value = true;
}

async function onCreate(): Promise<void> {
  const folder = storiesCtx.activeFolder.value;
  if (!folder) {
    error.value = 'Select a story first.';
    return;
  }
  const trimName = name.value.trim();
  const trimLoc = location.value.trim();
  if (!trimName) { error.value = 'Please enter a document name.'; return; }
  if (!trimLoc) { error.value = 'Please choose a location.'; return; }

  error.value = '';
  saving.value = true;
  try {
    const result = await invoke<{ path: string; title: string }>('create_story_document', {
      request: {
        story_folder: folder,
        name: trimName,
        location: trimLoc,
      },
    });
    emit('created', result.path, result.title);
  } catch (e) {
    error.value = String(e);
  } finally {
    saving.value = false;
  }
}

function onCancel(): void {
  emit('cancel');
}
</script>

<template>
  <div class="panel new-doc-panel">
    <h2 class="panel-title">New Document</h2>

    <div class="form-group">
      <label>Name</label>
      <input v-model="name" type="text" :placeholder="namePlaceholder" @keydown.enter="onCreate" />
    </div>

    <div class="form-group">
      <label>Type</label>
      <div class="type-options">
        <label
          v-for="t in docTypes"
          :key="t.id"
          class="type-option"
          :class="{ active: docTypeId === t.id }"
          @click.prevent="onSelectType(t.id)"
        >
          <input :checked="docTypeId === t.id" type="radio" :value="t.id" tabindex="-1" />
          <span class="type-option-title">{{ t.label }}</span>
          <span class="type-option-path">{{ t.path }}</span>
        </label>
      </div>
    </div>

    <div class="form-group">
      <label>
        Location
        <span class="form-hint"> — relative to the story folder (editable)</span>
      </label>
      <input
        v-model="location"
        type="text"
        placeholder="Manuscript"
        class="mono"
        @input="onLocationEdit"
      />
    </div>

    <div v-if="error" class="form-error">{{ error }}</div>

    <div class="form-actions">
      <button class="btn" :disabled="saving" @click="onCreate">Create</button>
      <button class="btn btn-secondary" @click="onCancel">Cancel</button>
    </div>
  </div>
</template>

<style scoped>
.new-doc-panel {
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

.form-group input[type="text"] {
  background: var(--surface2);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--text);
  font-size: 13px;
  padding: 8px 10px;
  width: 100%;
  user-select: text;
}

.form-group input.mono {
  font-family: var(--mono);
  font-size: 12px;
}

.type-options {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.type-option {
  display: flex;
  flex-direction: column;
  gap: 1px;
  padding: 8px 12px 8px 32px;
  border: 1px solid var(--border);
  border-radius: var(--radius);
  background: var(--surface2);
  cursor: pointer;
  position: relative;
  text-transform: none;
  letter-spacing: 0;
  color: var(--text);
}

.type-option.active {
  border-color: var(--accent);
  background: color-mix(in srgb, var(--accent) 10%, var(--surface2));
}

.type-option input[type="radio"] {
  position: absolute;
  left: 10px;
  top: 10px;
  margin: 0;
  pointer-events: none;
}

.type-option-title {
  font-size: 13px;
  font-weight: 600;
}

.type-option-path {
  font-size: 11px;
  color: var(--text-muted);
  font-family: var(--mono);
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

.btn-secondary {
  background: var(--surface2);
  border: 1px solid var(--border);
  color: var(--text-muted);
}

.btn-secondary:hover {
  color: var(--text);
  border-color: var(--accent);
}
</style>
