import { ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { AnalysisState, GenreResult, LogLine } from '../types';

import { useSettings } from './useSettings';

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
  const s = useSettings();
  return {
    provider: s.provider.value,
    apiKey: s.apiKey.value,
    model: s.model.value,
    canopyApiKey: s.canopyApiKey.value,
    dataforseoLogin: s.dataforseoLogin.value,
    dataforseoPassword: s.dataforseoPassword.value,
  };
}

async function runAnalyze(folder: string, forceResummarize: boolean, platform: string): Promise<void> {
  if (!folder) { appendLog('✗ No story selected.'); return; }
  const { provider, apiKey, model, canopyApiKey, dataforseoLogin, dataforseoPassword } = getSettings();

  clearLog();
  isWorking.value = true;
  const runTime = new Date().toISOString();

  try {
    const result = await invoke<GenreResult>('analyze_story', {
      request: {
        folder, api_key: apiKey, model, provider,
        force_resummarize: forceResummarize,
        canopy_api_key: canopyApiKey,
        platform,
        dataforseo_login: dataforseoLogin,
        dataforseo_password: dataforseoPassword,
        run_time: runTime,
      },
    });
    if (!result.success) {
      appendLog('✗ ' + result.error);
    }
  } catch (e) {
    appendLog('✗ ' + String(e));
  } finally {
    isWorking.value = false;
    saveLog(folder, runTime);
  }
}

export type ContinuityScope = { mode: 'manuscript' } | { mode: 'series'; seriesId: number };

/**
 * Runs the craft pipeline via a single backend command.
 * The backend handles ordering, AI calls, and storage.
 */
async function runCraftAnalysis(folder: string, selected: string[], continuityScope: ContinuityScope): Promise<void> {
  if (!folder) { appendLog('✗ No story selected.'); return; }

  const s = useSettings();
  const { provider, apiKey } = getSettings();
  clearLog();
  isWorking.value = true;

  try {
    const result = await invoke<GenreResult>('run_craft_pipeline', {
      request: {
        folder,
        selected,
        provider,
        api_key: apiKey,
        model: s.modelFor('default'),
        model_summaries: s.modelFor('summaries'),
        model_continuity: s.modelFor('continuity'),
        model_sdt: s.modelFor('showDontTell'),
        continuity_scope: continuityScope.mode,
        series_id: continuityScope.mode === 'series' ? continuityScope.seriesId : 0,
      },
    });
    if (!result.success) {
      appendLog('✗ ' + result.error);
    }
  } catch (e) {
    appendLog('✗ ' + String(e));
  } finally {
    isWorking.value = false;
    saveLog(folder, new Date().toISOString());
  }
}

async function runMarketIntel(folder: string): Promise<void> {
  if (!folder) { appendLog('✗ No story selected.'); return; }
  const { provider, apiKey, model, canopyApiKey } = getSettings();

  clearLog();
  isWorking.value = true;

  try {
    const result = await invoke<GenreResult>('run_market_intel', {
      request: { folder, provider, api_key: apiKey, model, canopy_api_key: canopyApiKey },
    });
    if (!result.success) {
      appendLog('✗ ' + result.error);
    }
  } catch (e) {
    appendLog('✗ ' + String(e));
  } finally {
    isWorking.value = false;
    saveLog(folder, new Date().toISOString());
  }
}

async function cancelOperation(): Promise<void> {
  appendLog('Stopping after current step...');
  await invoke('cancel_operation');
}

async function saveLog(folder: string, timestamp: string): Promise<void> {
  if (!folder || logLines.value.length === 0) return;
  const content = JSON.stringify({
    schema: 'activity_log_v1',
    timestamp,
    lines: logLines.value,
  });
  try {
    await invoke('save_activity_log_cmd', { folder, content, timestamp });
  } catch (e) {
    console.error('Failed to save activity log:', e);
  }
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
    runCraftAnalysis,
    runMarketIntel,
    cancelOperation,
    clearLog,
    appendLog,
  };
}
