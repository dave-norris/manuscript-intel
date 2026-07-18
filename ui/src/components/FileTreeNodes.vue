<script setup lang="ts">
export interface FileTreeEntry {
  name: string;
  path: string;
  is_dir: boolean;
  children: FileTreeEntry[];
}

defineProps<{
  entries: FileTreeEntry[];
  expanded: Set<string>;
}>();

const emit = defineEmits<{
  (e: 'toggle', path: string): void;
  (e: 'open', entry: FileTreeEntry): void;
  (e: 'add', entry: FileTreeEntry, event: Event): void;
}>();

function onAdd(entry: FileTreeEntry, event: Event): void {
  event.stopPropagation();
  emit('add', entry, event);
}
</script>

<template>
  <template v-for="entry in entries" :key="entry.path">
    <div v-if="entry.is_dir" class="file-tree-dir">
      <div class="file-tree-dir-header" @click="emit('toggle', entry.path)">
        <span class="file-tree-arrow">{{ expanded.has(entry.path) ? '▾' : '▸' }}</span>
        <span class="file-tree-dir-name">{{ entry.name }}</span>
        <button
          class="btn-add"
          title="New document in this folder"
          @click="onAdd(entry, $event)"
        >+</button>
      </div>
      <div v-if="expanded.has(entry.path)" class="file-tree-children">
        <FileTreeNodes
          :entries="entry.children"
          :expanded="expanded"
          @toggle="emit('toggle', $event)"
          @open="emit('open', $event)"
          @add="(e, ev) => emit('add', e, ev)"
        />
      </div>
    </div>
    <div
      v-else
      class="file-tree-file"
      @click="emit('open', entry)"
    >{{ entry.name.replace(/\.md$/, '') }}</div>
  </template>
</template>

<style scoped>
.file-tree-dir {
  margin: 0;
}

.file-tree-dir-header {
  display: flex;
  align-items: center;
  gap: 4px;
  padding: 5px 8px;
  font-size: 12px;
  font-weight: 600;
  color: var(--text);
  cursor: pointer;
  border-radius: var(--radius);
}

.file-tree-dir-header:hover {
  background: var(--surface2);
}

.file-tree-arrow {
  font-size: 10px;
  color: var(--text-muted);
  width: 12px;
  text-align: center;
  flex-shrink: 0;
}

.file-tree-dir-name {
  flex: 1;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  min-width: 0;
}

.btn-add {
  background: none;
  border: 1px solid var(--border);
  color: var(--text-muted);
  width: 18px;
  height: 18px;
  border-radius: 4px;
  cursor: pointer;
  font-size: 14px;
  line-height: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0;
  flex-shrink: 0;
}

.btn-add:hover {
  color: var(--accent);
  border-color: var(--accent);
}

.file-tree-children {
  padding-left: 14px;
}

.file-tree-file {
  padding: 4px 8px 4px 12px;
  font-size: 12px;
  color: var(--text-muted);
  cursor: pointer;
  border-radius: var(--radius);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.file-tree-file:hover {
  background: var(--surface2);
  color: var(--text);
}
</style>
