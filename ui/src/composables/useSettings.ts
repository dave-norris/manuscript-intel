import { ref, computed } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import type { ModelInfo, ModelsResult } from '../types';

// ── AI function model assignments ─────────────────────────────────────────────
// Each AI function can have its own model. Empty means "use the default model."

export interface ModelAssignments {
  default:       string;  // Fallback for any function without a specific model
  summaries:     string;  // Chapter summaries (extraction)
  genre:         string;  // Genre analysis & ranking
  keywords:      string;  // Keywords, search terms, BISAC
  continuity:    string;  // Continuity checker (fact extraction + judgment)
  showDontTell:  string;  // Show Don't Tell analysis
  prose:         string;  // Creative suggestions / rewrites
}

export interface FolderStructure {
  /** Chapter files — analysis reads only here */
  manuscript: string;
  /** Story bible docs */
  bible: string;
  /** Character docs */
  characters: string;
  /** Extra scaffold-only folders (app does not use these) */
  extra: string[];
}

const DEFAULT_FOLDER_STRUCTURE: FolderStructure = {
  manuscript: 'Manuscript',
  bible: 'Bible',
  characters: 'Characters',
  extra: ['Publishing/Cover', 'Research'],
};

function cloneStructure(s: FolderStructure): FolderStructure {
  return {
    manuscript: s.manuscript || DEFAULT_FOLDER_STRUCTURE.manuscript,
    bible: s.bible || DEFAULT_FOLDER_STRUCTURE.bible,
    characters: s.characters || DEFAULT_FOLDER_STRUCTURE.characters,
    extra: Array.isArray(s.extra) ? [...s.extra] : [...DEFAULT_FOLDER_STRUCTURE.extra],
  };
}

function loadAssignments(): ModelAssignments {
  const stored = localStorage.getItem('modelAssignments');
  const defaults: ModelAssignments = {
    default: '', summaries: '', genre: '', keywords: '', continuity: '', showDontTell: '', prose: ''
  };
  if (stored) {
    try { return { ...defaults, ...JSON.parse(stored) }; } catch { /* use defaults */ }
  }
  // Migrate from old settings
  const oldModel = localStorage.getItem('model') || '';
  const oldProse = localStorage.getItem('proseModel') || '';
  if (oldModel || oldProse) {
    defaults.default = oldModel;
    defaults.prose = oldProse;
  }
  return defaults;
}

const provider = ref(localStorage.getItem('provider') || 'tokenmix');
const apiKey = ref(localStorage.getItem('apiKey') || '');
const modelAssignments = ref<ModelAssignments>(loadAssignments());
const canopyApiKey = ref(localStorage.getItem('canopyApiKey') || '');
const dataforseoLogin = ref(localStorage.getItem('dataforseoLogin') || '');
const dataforseoPassword = ref(localStorage.getItem('dataforseoPassword') || '');
const models = ref<ModelInfo[]>(loadModelsFromStorage());
const folderStructure = ref<FolderStructure>(cloneStructure(DEFAULT_FOLDER_STRUCTURE));

function loadModelsFromStorage(): ModelInfo[] {
  const stored = localStorage.getItem('cachedModels');
  if (stored) {
    try { return JSON.parse(stored); } catch { /* ignore */ }
  }
  return [];
}

// ── Convenience getters ───────────────────────────────────────────────────────

/** Resolve the model for a given function. Falls back to default if unset. */
function modelFor(fn: keyof ModelAssignments): string {
  return modelAssignments.value[fn] || modelAssignments.value.default;
}

// Legacy compatibility: 'model' returns default, 'proseModel' returns prose
const model = computed(() => modelAssignments.value.default);
const proseModel = computed(() => modelAssignments.value.prose || modelAssignments.value.default);

// ── Actions ──────────────────────────────────────────────────────────────────

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
      localStorage.setItem('cachedModels', JSON.stringify(result.models));
      return { success: true, error: '' };
    }
    return { success: false, error: result.error || 'No models returned.' };
  } catch (e) {
    return { success: false, error: 'Error: ' + String(e) };
  }
}

async function loadFolderStructure(): Promise<void> {
  try {
    const result = await invoke<FolderStructure>('get_folder_structure');
    folderStructure.value = cloneStructure(result);
  } catch {
    folderStructure.value = cloneStructure(DEFAULT_FOLDER_STRUCTURE);
  }
}

function addFolderEntry(): void {
  folderStructure.value.extra.push('');
}

function removeFolderEntry(index: number): void {
  folderStructure.value.extra.splice(index, 1);
}

async function saveSettings(): Promise<void> {
  localStorage.setItem('provider', provider.value);
  localStorage.setItem('apiKey', apiKey.value.trim());
  localStorage.setItem('modelAssignments', JSON.stringify(modelAssignments.value));
  // Keep legacy keys for backward compat
  localStorage.setItem('model', modelAssignments.value.default);
  localStorage.setItem('proseModel', modelAssignments.value.prose);
  localStorage.setItem('canopyApiKey', canopyApiKey.value.trim());
  localStorage.setItem('dataforseoLogin', dataforseoLogin.value.trim());
  localStorage.setItem('dataforseoPassword', dataforseoPassword.value.trim());

  const saved = await invoke<FolderStructure>('save_folder_structure', {
    structure: folderStructure.value,
  });
  folderStructure.value = cloneStructure(saved);
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
    proseModel,
    modelAssignments,
    modelFor,
    canopyApiKey,
    dataforseoLogin,
    dataforseoPassword,
    models,
    folderStructure,
    fetchModels,
    loadFolderStructure,
    addFolderEntry,
    removeFolderEntry,
    saveSettings,
    testCanopy,
    testDataforseo,
  };
}
