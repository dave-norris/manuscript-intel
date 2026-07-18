<script setup lang="ts">
import { ref, nextTick, watch } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { useSettings } from '../composables/useSettings';
import { formatMarkdown } from '../formatMarkdown';

const props = defineProps<{
  chapterText: string;
  chapterTitle: string;
  storyFolder: string;
  selectedText: string;
}>();

const settings = useSettings();

interface Message {
  role: 'user' | 'assistant';
  content: string;
}

const messages = ref<Message[]>([]);
const input = ref('');
const loading = ref(false);
const error = ref('');
const chatPane = ref<HTMLElement | null>(null);
const chatModel = ref(settings.modelFor('prose'));

async function onSend(): Promise<void> {
  const text = input.value.trim();
  if (!text || loading.value) return;

  if (!chatModel.value) {
    error.value = 'No model selected.';
    return;
  }

  error.value = '';
  messages.value.push({ role: 'user', content: text });
  input.value = '';
  loading.value = true;
  await scrollToBottom();

  // If there's selected text, include it as context in the actual message sent to AI
  const messageForAi = props.selectedText
    ? `[Selected text from manuscript: "${props.selectedText}"]\n\n${text}`
    : text;

  // Load bible from folder
  const bible = await loadBible();

  try {
    const result = await invoke<{ success: boolean; reply: string; error: string }>('chat_with_context', {
      request: {
        provider: settings.provider.value,
        api_key: settings.apiKey.value,
        model: chatModel.value,
        message: messageForAi,
        chapter_text: props.chapterText,
        chapter_title: props.chapterTitle,
        bible,
        history: messages.value.slice(0, -1), // exclude the message we just added
      }
    });

    if (result.success) {
      messages.value.push({ role: 'assistant', content: result.reply });
    } else {
      error.value = result.error;
    }
  } catch (e) {
    error.value = String(e);
  } finally {
    loading.value = false;
    await scrollToBottom();
  }
}

async function loadBible(): Promise<string> {
  // The backend will discover it from the folder, but we pass empty
  // and let the chat command handle it via the system prompt.
  // Actually, we can load it here for the frontend to pass.
  // For simplicity, pass empty — the backend system prompt includes bible.
  return '';
}

function onKeydown(e: KeyboardEvent): void {
  if (e.key === 'Enter' && !e.shiftKey) {
    e.preventDefault();
    onSend();
  }
}

async function scrollToBottom(): Promise<void> {
  await nextTick();
  if (chatPane.value) {
    chatPane.value.scrollTop = chatPane.value.scrollHeight;
  }
}

function clearChat(): void {
  messages.value = [];
  error.value = '';
}

// Clear chat when chapter changes
watch(() => props.chapterTitle, () => {
  // Don't clear automatically — the user might want to keep context
});
</script>

<template>
  <div class="ai-chat">
    <div class="chat-header">
      <span class="chat-title">AI Chat</span>
      <select v-model="chatModel" class="chat-model-select">
        <option v-for="m in settings.models.value" :key="m.id" :value="m.id">{{ m.id }}</option>
      </select>
      <button v-if="messages.length > 0" class="chat-clear" @click="clearChat">Clear</button>
    </div>

    <div class="chat-messages" ref="chatPane">
      <div v-if="messages.length === 0" class="chat-empty">
        Ask anything about your chapter — rewrites, brainstorming, continuity, prose feedback.
      </div>
      <div
        v-for="(msg, i) in messages"
        :key="i"
        class="chat-msg"
        :class="msg.role"
      >
        <div v-if="msg.role === 'user'" class="chat-msg-text">{{ msg.content }}</div>
        <div v-else class="chat-msg-text" v-html="formatMarkdown(msg.content)"></div>
      </div>
      <div v-if="loading" class="chat-msg assistant">
        <div class="chat-msg-text chat-loading">Thinking...</div>
      </div>
    </div>

    <div v-if="error" class="chat-error">{{ error }}</div>

    <div v-if="props.selectedText" class="chat-selection-indicator">
      <span class="selection-label">Selected:</span> {{ props.selectedText.length > 60 ? props.selectedText.substring(0, 60) + '...' : props.selectedText }}
    </div>

    <div class="chat-input-row">
      <textarea
        v-model="input"
        class="chat-input"
        rows="2"
        placeholder="Ask about your chapter..."
        @keydown="onKeydown"
        :disabled="loading"
      ></textarea>
      <button class="chat-send" @click="onSend" :disabled="loading || !input.trim()">Send</button>
    </div>
  </div>
</template>

<style scoped>
.ai-chat {
  display: flex;
  flex-direction: column;
  height: 100%;
  border-left: 1px solid var(--border);
}

.chat-header {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 14px;
  border-bottom: 1px solid var(--border);
  flex-shrink: 0;
}

.chat-title {
  font-size: 12px;
  font-weight: 700;
  text-transform: uppercase;
  letter-spacing: 0.05em;
  color: var(--text-muted);
}

.chat-model-select {
  flex: 1;
  background: var(--surface2);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--text);
  font-size: 11px;
  padding: 3px 6px;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
}

.chat-clear {
  background: none;
  border: none;
  color: var(--text-muted);
  font-size: 11px;
  cursor: pointer;
  padding: 2px 6px;
  border-radius: var(--radius);
}

.chat-clear:hover {
  color: var(--text);
  background: var(--surface2);
}

.chat-messages {
  flex: 1;
  overflow-y: auto;
  padding: 12px 14px;
  display: flex;
  flex-direction: column;
  gap: 10px;
}

.chat-empty {
  color: var(--text-muted);
  font-size: 12px;
  font-style: italic;
  padding: 20px 0;
  text-align: center;
}

.chat-msg {
  max-width: 90%;
}

.chat-msg.user {
  align-self: flex-end;
}

.chat-msg.assistant {
  align-self: flex-start;
}

.chat-msg.user .chat-msg-text {
  background: var(--accent);
  color: #fff;
  border-radius: 12px 12px 2px 12px;
  padding: 8px 12px;
  font-size: 13px;
  line-height: 1.5;
  white-space: pre-wrap;
}

.chat-msg.assistant .chat-msg-text {
  background: var(--surface2);
  color: var(--text);
  border-radius: 12px 12px 12px 2px;
  padding: 8px 12px;
  font-size: 13px;
  line-height: 1.6;
}

.chat-msg.assistant .chat-msg-text :deep(p) {
  margin: 0 0 0.5em;
}

.chat-msg.assistant .chat-msg-text :deep(p:last-child) {
  margin-bottom: 0;
}

.chat-msg.assistant .chat-msg-text :deep(pre) {
  background: var(--surface);
  padding: 8px;
  border-radius: var(--radius);
  font-size: 12px;
  overflow-x: auto;
  margin: 6px 0;
}

.chat-msg.assistant .chat-msg-text :deep(code) {
  background: var(--surface);
  padding: 1px 4px;
  border-radius: 3px;
  font-size: 12px;
}

.chat-loading {
  font-style: italic;
  color: var(--text-muted);
}

.chat-error {
  color: #e74c3c;
  font-size: 11px;
  padding: 4px 14px;
  flex-shrink: 0;
}

.chat-selection-indicator {
  font-size: 11px;
  color: var(--accent);
  padding: 4px 14px;
  background: rgba(232, 97, 44, 0.06);
  border-top: 1px solid var(--border);
  flex-shrink: 0;
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}

.selection-label {
  font-weight: 600;
}

.chat-input-row {
  display: flex;
  gap: 8px;
  padding: 10px 14px;
  border-top: 1px solid var(--border);
  flex-shrink: 0;
  align-items: flex-end;
}

.chat-input {
  flex: 1;
  background: var(--surface2);
  border: 1px solid var(--border);
  border-radius: var(--radius);
  color: var(--text);
  font-size: 13px;
  padding: 8px 10px;
  resize: none;
  font-family: inherit;
  line-height: 1.4;
}

.chat-input:focus {
  outline: none;
  border-color: var(--accent);
}

.chat-send {
  background: var(--accent);
  border: none;
  border-radius: var(--radius);
  color: #fff;
  font-size: 12px;
  font-weight: 600;
  padding: 8px 14px;
  cursor: pointer;
  white-space: nowrap;
}

.chat-send:hover { background: var(--accent-dim); }
.chat-send:disabled { background: var(--surface2); color: var(--text-muted); cursor: not-allowed; }
</style>
