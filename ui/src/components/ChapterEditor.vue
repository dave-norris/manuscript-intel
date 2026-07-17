<script setup lang="ts">
import { ref, watch, onBeforeUnmount, nextTick } from 'vue';
import { useEditor, EditorContent } from '@tiptap/vue-3';
import StarterKit from '@tiptap/starter-kit';
import Highlight from '@tiptap/extension-highlight';
import { invoke } from '@tauri-apps/api/core';

// ── Props ─────────────────────────────────────────────────────────────────────

const props = defineProps<{
  filePath: string;
  highlightText?: string;
}>();

const emit = defineEmits<{
  (e: 'saved'): void;
}>();

// ── State ─────────────────────────────────────────────────────────────────────

const saving = ref(false);
const saveStatus = ref('');
const content = ref('');
let saveTimer: ReturnType<typeof setTimeout> | null = null;

// ── Editor setup ──────────────────────────────────────────────────────────────

const editor = useEditor({
  extensions: [
    StarterKit.configure({
      heading: { levels: [1, 2, 3] },
    }),
    Highlight.configure({ multicolor: true }),
  ],
  content: '',
  editorProps: {
    attributes: {
      class: 'chapter-editor-content',
    },
  },
  onUpdate: () => {
    scheduleSave();
  },
});

// ── Load file ─────────────────────────────────────────────────────────────────

async function loadFile(filePath: string): Promise<void> {
  if (!filePath) return;
  try {
    const text = await invoke<string>('read_chapter', { filePath });
    content.value = text;
    // Convert markdown to HTML for Tiptap
    const html = markdownToEditorHtml(text);
    editor.value?.commands.setContent(html);
    await nextTick();
    if (props.highlightText) {
      highlightTarget(props.highlightText);
    }
  } catch (e) {
    editor.value?.commands.setContent(`<p style="color:red">Error loading: ${e}</p>`);
  }
}

// ── Save to disk ──────────────────────────────────────────────────────────────

function scheduleSave(): void {
  if (saveTimer) clearTimeout(saveTimer);
  saveTimer = setTimeout(() => saveNow(), 2000);
}

async function saveNow(): Promise<void> {
  if (!editor.value || !props.filePath) return;
  saving.value = true;
  try {
    // Get plain text from editor (preserving markdown structure)
    const text = editorHtmlToMarkdown(editor.value.getHTML());
    await invoke<string>('write_manuscript_fix', {
      filePath: props.filePath,
      oldText: content.value,
      newText: text,
    });
    content.value = text;
    saveStatus.value = 'Saved';
    emit('saved');
    setTimeout(() => { saveStatus.value = ''; }, 1500);
  } catch (e) {
    saveStatus.value = 'Save failed: ' + String(e);
  } finally {
    saving.value = false;
  }
}

// ── Highlight a text range ────────────────────────────────────────────────────

function highlightTarget(text: string): void {
  if (!editor.value || !text) return;

  const doc = editor.value.state.doc;
  const fullText = doc.textContent;
  const idx = fullText.indexOf(text);
  if (idx < 0) return;

  // Find the position in the ProseMirror document
  let pos = 0;
  let from = 0;
  let to = 0;
  doc.descendants((node, nodePos) => {
    if (from > 0) return false; // already found
    if (node.isText) {
      const nodeText = node.text || '';
      const relIdx = fullText.indexOf(text, pos) - pos;
      if (relIdx >= 0 && relIdx < nodeText.length) {
        from = nodePos + relIdx;
        to = from + text.length;
        return false;
      }
      pos += nodeText.length;
    }
    return true;
  });

  // Simpler approach: scan character positions
  if (from === 0) {
    let charCount = 0;
    doc.descendants((node, nodePos) => {
      if (from > 0) return false;
      if (node.isText) {
        const nodeText = node.text || '';
        const localIdx = nodeText.indexOf(text.substring(0, Math.min(20, text.length)));
        if (localIdx >= 0) {
          from = nodePos + localIdx;
          to = from + text.length;
          return false;
        }
        charCount += nodeText.length;
      }
      return true;
    });
  }

  if (from > 0 && to > from) {
    editor.value.chain()
      .setTextSelection({ from, to })
      .setHighlight()
      .run();

    // Scroll into view
    setTimeout(() => {
      const el = document.querySelector('.chapter-editor-content mark');
      if (el) el.scrollIntoView({ behavior: 'smooth', block: 'center' });
    }, 100);
  }
}

// ── Public: insert text at cursor (called from parent) ────────────────────────

function insertAtCursor(text: string): void {
  if (!editor.value) return;
  editor.value.chain().focus().insertContent(text).run();
  scheduleSave();
}

// ── Public: replace highlighted/selected text ─────────────────────────────────

function replaceSelection(text: string): void {
  if (!editor.value) return;
  const { from, to } = editor.value.state.selection;
  if (from === to) {
    // No selection — insert at cursor
    insertAtCursor(text);
  } else {
    editor.value.chain().focus().deleteSelection().insertContent(text).run();
    scheduleSave();
  }
}

// Expose methods to parent
defineExpose({ insertAtCursor, replaceSelection, highlightTarget, saveNow });

// ── Watch file path changes ───────────────────────────────────────────────────

watch(() => props.filePath, (fp) => { if (fp) loadFile(fp); }, { immediate: true });

watch(() => props.highlightText, (text) => {
  if (text && editor.value) highlightTarget(text);
});

// ── Cleanup ───────────────────────────────────────────────────────────────────

onBeforeUnmount(() => {
  if (saveTimer) clearTimeout(saveTimer);
});

// ── Markdown <-> HTML conversion (simple, prose-focused) ──────────────────────

function markdownToEditorHtml(md: string): string {
  return md
    .replace(/^### (.+)$/gm, '<h3>$1</h3>')
    .replace(/^## (.+)$/gm, '<h2>$1</h2>')
    .replace(/^# (.+)$/gm, '<h1>$1</h1>')
    .replace(/\*\*(.+?)\*\*/g, '<strong>$1</strong>')
    .replace(/\*(.+?)\*/g, '<em>$1</em>')
    .replace(/^---$/gm, '<hr>')
    .split(/\n{2,}/)
    .map(block => {
      if (block.startsWith('<h') || block.startsWith('<hr')) return block;
      return `<p>${block.replace(/\n/g, '<br>')}</p>`;
    })
    .join('');
}

function editorHtmlToMarkdown(html: string): string {
  return html
    .replace(/<h1>(.*?)<\/h1>/g, '# $1\n\n')
    .replace(/<h2>(.*?)<\/h2>/g, '## $1\n\n')
    .replace(/<h3>(.*?)<\/h3>/g, '### $1\n\n')
    .replace(/<strong>(.*?)<\/strong>/g, '**$1**')
    .replace(/<em>(.*?)<\/em>/g, '*$1*')
    .replace(/<mark[^>]*>(.*?)<\/mark>/g, '$1')
    .replace(/<hr\s*\/?>/g, '---\n\n')
    .replace(/<br\s*\/?>/g, '\n')
    .replace(/<\/p>\s*<p>/g, '\n\n')
    .replace(/<\/?p>/g, '')
    .replace(/<[^>]+>/g, '')
    .replace(/&amp;/g, '&')
    .replace(/&lt;/g, '<')
    .replace(/&gt;/g, '>')
    .replace(/&quot;/g, '"')
    .replace(/&#39;/g, "'")
    .replace(/\n{3,}/g, '\n\n')
    .trim() + '\n';
}
</script>

<template>
  <div class="chapter-editor">
    <div class="editor-status" v-if="saving || saveStatus">
      <span v-if="saving" class="status-saving">Saving...</span>
      <span v-else class="status-saved">{{ saveStatus }}</span>
    </div>
    <EditorContent :editor="editor" class="editor-wrapper" />
  </div>
</template>

<style scoped>
.chapter-editor {
  display: flex;
  flex-direction: column;
  height: 100%;
  position: relative;
}

.editor-status {
  position: absolute;
  top: 8px;
  right: 12px;
  font-size: 11px;
  z-index: 10;
}

.status-saving {
  color: var(--text-muted);
  font-style: italic;
}

.status-saved {
  color: #27ae60;
}

.editor-wrapper {
  flex: 1;
  overflow-y: auto;
}

.editor-wrapper :deep(.chapter-editor-content) {
  outline: none;
  padding: 20px 24px;
  font-size: 14px;
  line-height: 1.8;
  color: var(--text);
  min-height: 100%;
}

.editor-wrapper :deep(.chapter-editor-content p) {
  margin: 0 0 1em;
}

.editor-wrapper :deep(.chapter-editor-content h1),
.editor-wrapper :deep(.chapter-editor-content h2),
.editor-wrapper :deep(.chapter-editor-content h3) {
  margin: 1.2em 0 0.5em;
  font-weight: 700;
  color: var(--text);
}

.editor-wrapper :deep(.chapter-editor-content h1) { font-size: 1.4em; }
.editor-wrapper :deep(.chapter-editor-content h2) { font-size: 1.2em; }
.editor-wrapper :deep(.chapter-editor-content h3) { font-size: 1.05em; }

.editor-wrapper :deep(.chapter-editor-content mark) {
  background: rgba(231, 76, 60, 0.15);
  color: #e74c3c;
  font-weight: 600;
  padding: 1px 2px;
  border-radius: 2px;
}

.editor-wrapper :deep(.chapter-editor-content hr) {
  border: none;
  border-top: 1px solid var(--border);
  margin: 1.5em 0;
}
</style>
