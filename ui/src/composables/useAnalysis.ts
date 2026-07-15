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

async function runAnalyze(folder: string, forceResummarize: boolean, platform: string): Promise<void> {
  if (!folder) { appendLog('✗ No story selected.'); return; }
  const { provider, apiKey, model, canopyApiKey } = getSettings();
  if (!apiKey) { appendLog('✗ No API key set. Go to Settings.'); return; }
  if (!model) { appendLog('✗ No model selected. Go to Settings and fetch models.'); return; }

  const pipelineLabel = platform === 'wide' ? 'Wide distribution' : 'KDP';
  appendLog(`Running ${pipelineLabel} analysis pipeline... [${provider}: ${model}]${forceResummarize ? ' (force re-summarize)' : ''}`);
  isWorking.value = true;
  try {
    const result = await invoke<GenreResult>('analyze_story', {
      request: { folder, api_key: apiKey, model, provider, force_resummarize: forceResummarize, canopy_api_key: canopyApiKey, platform, dataforseo_login: localStorage.getItem('dataforseoLogin') || '', dataforseo_password: localStorage.getItem('dataforseoPassword') || '' },
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

async function runMarketIntel(folder: string): Promise<void> {
  if (!folder) { appendLog('✗ No story selected.'); return; }
  const { provider, apiKey, model, canopyApiKey } = getSettings();
  if (!canopyApiKey) { appendLog('✗ No Canopy API key set. Go to Settings.'); return; }
  if (!apiKey) { appendLog('✗ No AI API key set. Go to Settings.'); return; }
  if (!model) { appendLog('✗ No model selected. Go to Settings.'); return; }

  appendLog('Running Market Intel (Canopy API)...');
  appendLog('  → Competition analysis');
  appendLog('  → Review mining');
  appendLog('  → Author catalog analysis');
  isWorking.value = true;

  // Competition
  try {
    const compResult = await invoke<GenreResult>('analyze_competition_canopy', {
      request: { folder, api_key: apiKey, model, store: 'Kindle', provider, canopy_api_key: canopyApiKey },
    });
    if (compResult.success) { appendLog('✓ Competition analysis complete.'); }
    else { appendLog('✗ Competition: ' + compResult.error); }
  } catch (e) { appendLog('✗ Competition: ' + String(e)); }

  // Reviews
  try {
    const revResult = await invoke<{ success: boolean; error: string }>('mine_competitor_reviews', {
      request: { folder, canopy_api_key: canopyApiKey, api_key: apiKey, model, provider },
    });
    if (revResult.success) { appendLog('✓ Review mining complete.'); }
    else { appendLog('✗ Reviews: ' + revResult.error); }
  } catch (e) { appendLog('✗ Reviews: ' + String(e)); }

  // Authors
  try {
    const authResult = await invoke<{ success: boolean; error: string }>('analyze_comp_authors', {
      request: { folder, canopy_api_key: canopyApiKey, api_key: apiKey, model, provider },
    });
    if (authResult.success) { appendLog('✓ Author analysis complete.'); }
    else { appendLog('✗ Authors: ' + authResult.error); }
  } catch (e) { appendLog('✗ Authors: ' + String(e)); }

  appendLog('✓ Market Intel complete. View reports in the sidebar.');
  isWorking.value = false;
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
    runMarketIntel,
    cancelOperation,
    clearLog,
    appendLog,
  };
}
