import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { AnalysisState, GenreResult, LogLine } from '../types';

const analysisState = ref<AnalysisState | null>(null);
const isWorking = ref(false);
const logLines = ref<LogLine[]>([]);

function classifyLogLine(msg: string): LogLine {
  const trimmed = msg.trimStart();

  if (trimmed.startsWith('✓')) {
    return { type: 'log-success', icon: '✓', text: trimmed.slice(1).trim() };
  }
  if (trimmed.startsWith('✗')) {
    return { type: 'log-error', icon: '✗', text: trimmed.slice(1).trim() };
  }
  if (trimmed.startsWith('⚠')) {
    return { type: 'log-warn', icon: '⚠', text: trimmed.slice(1).trim() };
  }
  if (trimmed.startsWith('→')) {
    return { type: 'log-item', icon: '→', text: trimmed.slice(1).trim() };
  }
  if (/^(Step \d|Phase \d|Running |Analyzing |Mining |Syncing )/i.test(trimmed)) {
    return { type: 'log-step', icon: '', text: trimmed };
  }
  if (/complete\.?$|complete —|done\.?$/i.test(trimmed)) {
    return { type: 'log-done', icon: '', text: trimmed };
  }
  if (msg.startsWith('    ') || msg.startsWith('\t\t')) {
    return { type: 'log-detail', icon: '', text: trimmed };
  }
  return { type: 'log-info', icon: '', text: trimmed };
}

function appendLog(msg: string): void {
  logLines.value.push(classifyLogLine(msg));
}

function clearLog(): void {
  logLines.value = [];
}

async function refreshState(folder: string): Promise<void> {
  if (!folder) {
    analysisState.value = null;
    return;
  }
  try {
    const state = await invoke<AnalysisState>('check_analysis_state', { folder });
    analysisState.value = state;
  } catch (e) {
    console.error('check_analysis_state:', e);
    analysisState.value = null;
  }
}

function getSettings() {
  return {
    provider: localStorage.getItem('provider') || 'tokenmix',
    apiKey: localStorage.getItem('apiKey') || '',
    model: localStorage.getItem('model') || '',
    canopyApiKey: localStorage.getItem('canopyApiKey') || '',
  };
}

async function runAnalyze(folder: string, forceResummarize: boolean): Promise<void> {
  if (!folder) { appendLog('✗ No story selected.'); return; }
  const { provider, apiKey, model, canopyApiKey } = getSettings();
  if (!apiKey) { appendLog('✗ No API key set. Go to Settings.'); return; }
  if (!model) { appendLog('✗ No model selected. Go to Settings and fetch models.'); return; }

  appendLog(`Running full analysis pipeline... [${provider}: ${model}]${forceResummarize ? ' (force re-summarize)' : ''}`);
  isWorking.value = true;
  try {
    const result = await invoke<GenreResult>('analyze_story', {
      request: { folder, api_key: apiKey, model, provider, force_resummarize: forceResummarize, canopy_api_key: canopyApiKey },
    });
    if (result.success) {
      appendLog('✓ Analysis complete. View reports in the sidebar.');
    } else {
      appendLog('✗ ' + result.error);
    }
  } catch (e) {
    appendLog('✗ ' + String(e));
  } finally {
    isWorking.value = false;
  }
}

async function runCompetition(folder: string, store: string): Promise<void> {
  if (!folder) { appendLog('✗ No story selected.'); return; }
  const { provider, apiKey, model, canopyApiKey } = getSettings();
  if (!apiKey) { appendLog('✗ No API key set. Go to Settings.'); return; }
  if (!model) { appendLog('✗ No model selected. Go to Settings.'); return; }
  if (!canopyApiKey) { appendLog('✗ No Canopy API key set. Go to Settings.'); return; }

  appendLog(`Analyzing competition [${store}] via Canopy API... [${provider}: ${model}]`);
  isWorking.value = true;
  try {
    const result = await invoke<GenreResult>('analyze_competition_canopy', {
      request: { folder, api_key: apiKey, model, store, provider, canopy_api_key: canopyApiKey },
    });
    if (result.success) {
      appendLog('✓ Competition analysis complete. View reports in the sidebar.');
    } else {
      appendLog('✗ ' + result.error);
    }
  } catch (e) {
    appendLog('✗ ' + String(e));
  } finally {
    isWorking.value = false;
  }
}

async function runMineReviews(folder: string): Promise<void> {
  if (!folder) { appendLog('✗ No story selected.'); return; }
  const { provider, apiKey, model, canopyApiKey } = getSettings();
  if (!canopyApiKey) { appendLog('✗ No Canopy API key. Go to Settings.'); return; }
  if (!apiKey) { appendLog('✗ No AI API key. Go to Settings.'); return; }

  appendLog('Mining competitor reviews...');
  isWorking.value = true;
  try {
    const result = await invoke<{ success: boolean; error: string }>('mine_competitor_reviews', {
      request: { folder, canopy_api_key: canopyApiKey, api_key: apiKey, model, provider },
    });
    if (result.success) {
      appendLog('✓ Review mining complete. View in sidebar.');
    } else {
      appendLog('✗ ' + result.error);
    }
  } catch (e) {
    appendLog('✗ ' + String(e));
  } finally {
    isWorking.value = false;
  }
}

async function runAuthorAnalysis(folder: string): Promise<void> {
  if (!folder) { appendLog('✗ No story selected.'); return; }
  const { provider, apiKey, model, canopyApiKey } = getSettings();
  if (!canopyApiKey) { appendLog('✗ No Canopy API key. Go to Settings.'); return; }
  if (!apiKey) { appendLog('✗ No AI API key. Go to Settings.'); return; }

  appendLog('Analyzing competitor authors...');
  isWorking.value = true;
  try {
    const result = await invoke<{ success: boolean; error: string }>('analyze_comp_authors', {
      request: { folder, canopy_api_key: canopyApiKey, api_key: apiKey, model, provider },
    });
    if (result.success) {
      appendLog('✓ Author analysis complete. View in sidebar.');
    } else {
      appendLog('✗ ' + result.error);
    }
  } catch (e) {
    appendLog('✗ ' + String(e));
  } finally {
    isWorking.value = false;
  }
}

async function cancelOperation(): Promise<void> {
  appendLog('Stopping after current step...');
  await invoke('cancel_operation');
}

// Set up Tauri event listeners (runs once at module load)
listen<string>('genre:log', (event) => { appendLog(event.payload); });
listen<string>('cdp:log', (event) => { appendLog(event.payload); });

export function useAnalysis() {
  return {
    analysisState,
    isWorking,
    logLines,
    refreshState,
    runAnalyze,
    runCompetition,
    runMineReviews,
    runAuthorAnalysis,
    cancelOperation,
    clearLog,
    appendLog,
  };
}
