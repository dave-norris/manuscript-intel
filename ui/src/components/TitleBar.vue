<script setup lang="ts">
</script>

<template>
  <header id="titlebar" data-tauri-drag-region>
    <span class="titlebar-label" data-tauri-drag-region>Manuscript Intel</span>
  </header>
</template>

<style scoped>
#titlebar {
  grid-area: titlebar;
  background: var(--surface);
  border-bottom: 1px solid var(--border);
  display: flex;
  align-items: center;
  padding: 0 12px 0 80px;
  gap: 12px;
  user-select: none;
  -webkit-user-select: none;
}

.titlebar-label {
  font-size: 12px;
  font-weight: 600;
  letter-spacing: 0.02em;
  color: var(--text-muted);
  pointer-events: none;
}
</style>
