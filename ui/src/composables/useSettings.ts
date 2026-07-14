import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { ModelInfo, ModelsResult } from '../types';

const provider = ref(localStorage.getItem('provider') || 'tokenmix');
const apiKey = ref(localStorage.getItem('apiKey') || '');
const model = ref(localStorage.getItem('model') || '');
const canopyApiKey = ref(localStorage.getItem('canopyApiKey') || '');
const dataforseoLogin = ref(localStorage.getItem('dataforseoLogin') || '');
const dataforseoPassword = ref(localStorage.getItem('dataforseoPassword') || '');
const models = ref<ModelInfo[]>([]);

async function fetchModels(): Promise<{ success: boolean; error: string }> {
  if (!apiKey.value) {
    return { success: false, error: 'Enter an API key first.' };
  }
  try {
    const result = await invoke<ModelsResult>('list_models', {
      provider: provider.value,
      apiKey: apiKey.value,
    });
    if (result.success && result.models.length > 0) {
      models.value = result.models;
      // Restore saved model if still available
      const savedModel = localStorage.getItem('model') || '';
      if (savedModel && models.value.find(m => m.id === savedModel)) {
        model.value = savedModel;
      }
      return { success: true, error: '' };
    }
    return { success: false, error: result.error || 'No models returned.' };
  } catch (e) {
    return { success: false, error: 'Error: ' + String(e) };
  }
}

function saveSettings(): void {
  localStorage.setItem('provider', provider.value);
  localStorage.setItem('apiKey', apiKey.value.trim());
  localStorage.setItem('model', model.value);
  localStorage.setItem('canopyApiKey', canopyApiKey.value.trim());
  localStorage.setItem('dataforseoLogin', dataforseoLogin.value.trim());
  localStorage.setItem('dataforseoPassword', dataforseoPassword.value.trim());
}

async function testCanopy(): Promise<{ success: boolean; error: string }> {
  const key = canopyApiKey.value.trim();
  if (!key) {
    return { success: false, error: 'Enter a key first.' };
  }
  try {
    const result = await invoke<{ success: boolean; error: string }>('test_canopy_connection', { apiKey: key });
    return result;
  } catch (e) {
    return { success: false, error: String(e) };
  }
}

async function testDataforseo(): Promise<{ success: boolean; error: string }> {
  const login = dataforseoLogin.value.trim();
  const password = dataforseoPassword.value.trim();
  if (!login || !password) {
    return { success: false, error: 'Enter login and password first.' };
  }
  try {
    const result = await invoke<{ success: boolean; error: string }>('test_dataforseo_connection', { login, password });
    return result;
  } catch (e) {
    return { success: false, error: String(e) };
  }
}

export function useSettings() {
  return {
    provider,
    apiKey,
    model,
    canopyApiKey,
    dataforseoLogin,
    dataforseoPassword,
    models,
    fetchModels,
    saveSettings,
    testCanopy,
    testDataforseo,
  };
}
