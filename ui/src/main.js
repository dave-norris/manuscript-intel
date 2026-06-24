import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';

// Disable the native context menu everywhere except editable fields.
// -webkit-context-menu CSS is not supported in Tauri 2 — this is the correct approach.
document.addEventListener('contextmenu', (e) => {
  const tag = e.target.tagName;
  if (!['INPUT', 'TEXTAREA', 'SELECT'].includes(tag) && !e.target.isContentEditable) {
    e.preventDefault();
  }
});

// ── State ─────────────────────────────────────────────────────────────────────
let currentMarkdown = '';
let currentCsvMarkdown = '';
let csvContent = '';
let csvKeyword = '';

// ── DOM ───────────────────────────────────────────────────────────────────────
const statusDot   = document.getElementById('status-dot');
const statusLabel = document.getElementById('status-label');
const btnLaunch   = document.getElementById('btn-launch');
const logOutput   = document.getElementById('log-output');
const mdOutput    = document.getElementById('markdown-output');
const csvLogOut   = document.getElementById('csv-log-output');
const csvMdOut    = document.getElementById('csv-markdown-output');

// ── Settings persistence (localStorage) ──────────────────────────────────────
function loadSettings() {
  document.getElementById('api-key').value   = localStorage.getItem('apiKey')  || '';
  document.getElementById('model-select').value = localStorage.getItem('model') || 'claude-sonnet-4-6';
}

document.getElementById('btn-save-settings').addEventListener('click', () => {
  localStorage.setItem('apiKey', document.getElementById('api-key').value.trim());
  localStorage.setItem('model',  document.getElementById('model-select').value);
  const saved = document.getElementById('settings-saved');
  saved.textContent = '✓ Saved';
  setTimeout(() => { saved.textContent = ''; }, 2000);
});

function getSettings() {
  return {
    apiKey: localStorage.getItem('apiKey') || '',
    model:  localStorage.getItem('model')  || 'claude-sonnet-4-6',
  };
}

// ── // ── Rocket status ─────────────────────────────────────────────────────────────
async function refreshStatus() {
  try {
    const s = await invoke('check_rocket_status');
    if (s.cdp_enabled) {
      statusDot.className = 'status-dot running';
      statusLabel.textContent = 'Rocket running';
      btnLaunch.textContent = 'Connected';
      btnLaunch.disabled = true;
    } else if (s.running) {
      statusDot.className = 'status-dot error';
      statusLabel.textContent = 'Rocket open (no CDP)';
      btnLaunch.disabled = false;
      btnLaunch.textContent = 'Relaunch with CDP';
    } else {
      statusDot.className = 'status-dot';
      statusLabel.textContent = 'Rocket not running';
      btnLaunch.disabled = false;
      btnLaunch.textContent = 'Launch Rocket';
    }
  } catch {
    statusLabel.textContent = 'Status unknown';
  }
}

btnLaunch.addEventListener('click', async () => {
  btnLaunch.disabled = true;
  btnLaunch.textContent = 'Launching…';
  appendLog('Launching Publisher Rocket...');
  const result = await invoke('launch_rocket');
  if (result.success) {
    appendLog('✓ Publisher Rocket is ready.');
    refreshStatus();
  } else {
    appendLog('✗ ' + result.error);
    btnLaunch.disabled = false;
    btnLaunch.textContent = 'Retry';
  }
});

setInterval(refreshStatus, 5000);
refreshStatus();

// ── Navigation ────────────────────────────────────────────────────────────────
document.querySelectorAll('.nav-item').forEach(btn => {
  btn.addEventListener('click', () => {
    const target = btn.dataset.panel;
    document.querySelectorAll('.nav-item').forEach(b => b.classList.toggle('active', b === btn));
    document.querySelectorAll('.panel').forEach(p => {
      p.classList.toggle('visible', p.id === target + '-panel');
    });
  });
});

// ── Output tab switching ──────────────────────────────────────────────────────
function setupTabs(tabSelector, paneMap) {
  document.querySelectorAll(tabSelector).forEach(tab => {
    tab.addEventListener('click', () => {
      document.querySelectorAll(tabSelector).forEach(t => t.classList.toggle('active', t === tab));
      const target = tab.dataset.tab;
      for (const [key, paneId] of Object.entries(paneMap)) {
        document.getElementById(paneId).classList.toggle('hidden', key !== target);
      }
    });
  });
}

setupTabs('[data-tab="log"],[data-tab="markdown"]', {
  log:      'log-pane',
  markdown: 'markdown-pane',
});

setupTabs('[data-tab="csv-log"],[data-tab="csv-markdown"]', {
  'csv-log':      'csv-log-pane',
  'csv-markdown': 'csv-markdown-pane',
});

// ── Category Finder ─────────────────────────────────────────────────────────
let currentFinderMarkdown = '';

setupTabs('[data-tab="finder-log"],[data-tab="finder-markdown"]', {
  'finder-log':      'finder-log-pane',
  'finder-markdown': 'finder-markdown-pane',
});

document.getElementById('btn-run-finder').addEventListener('click', async () => {
  const genre = document.getElementById('genre-description').value.trim();
  if (!genre) return;

  const store  = document.querySelector('input[name="finder-store"]:checked')?.value  || 'Kindle';
  const filter = document.querySelector('input[name="finder-filter"]:checked')?.value || 'Selectable Excluding Ghosts';
  const { apiKey, model } = getSettings();

  if (!apiKey) {
    appendFinderLog('✗ No API key set. Go to Settings and add your Anthropic API key.');
    showFinderTab('finder-log');
    return;
  }

  appendFinderLog(`Finding categories for: "${genre}"`);
  appendFinderLog(`Store: ${store} / Filter: ${filter}`);
  showFinderTab('finder-log');

  const btn = document.getElementById('btn-run-finder');
  btn.disabled = true;

  try {
    const result = await invoke('find_categories', {
      request: { genre, store, filter, api_key: apiKey, model }
    });
    if (result.success) {
      currentFinderMarkdown = result.markdown;
      document.getElementById('finder-markdown-output').textContent = currentFinderMarkdown;
      showFinderTab('finder-markdown');
    } else {
      appendFinderLog('✗ ' + result.error);
    }
  } catch (e) {
    appendFinderLog('✗ ' + String(e));
  } finally {
    btn.disabled = false;
  }
});

document.getElementById('btn-copy-finder').addEventListener('click', async () => {
  if (!currentFinderMarkdown) return;
  await writeText(currentFinderMarkdown);
  const btn = document.getElementById('btn-copy-finder');
  btn.textContent = 'Copied!';
  setTimeout(() => { btn.textContent = 'Copy results'; }, 1500);
});

function appendFinderLog(msg) {
  const el = document.getElementById('finder-log-output');
  el.textContent += (el.textContent ? '\n' : '') + msg;
  el.scrollTop = el.scrollHeight;
}

function showFinderTab(name) {
  document.querySelectorAll('[data-tab="finder-log"],[data-tab="finder-markdown"]').forEach(t => {
    t.classList.toggle('active', t.dataset.tab === name);
  });
  document.getElementById('finder-log-pane').classList.toggle('hidden', name !== 'finder-log');
  document.getElementById('finder-markdown-pane').classList.toggle('hidden', name !== 'finder-markdown');
}

// Also route cdp:log to finder log when finder panel is active
listen('cdp:log', (event) => {
  const finderVisible = document.getElementById('finder-panel').classList.contains('visible');
  if (finderVisible) appendFinderLog(event.payload);
  appendLog(event.payload);
  appendCsvLog(event.payload);
});

// ── Category Analyzer ─────────────────────────────────────────────────────────
document.getElementById('btn-run-category').addEventListener('click', async () => {
  const raw = document.getElementById('category-paths').value.trim();
  if (!raw) return;

  const paths = raw.split('\n').map(l => l.trim()).filter(Boolean);
  const store = document.querySelector('input[name="store"]:checked')?.value || 'Kindle';
  const filter = document.querySelector('input[name="filter"]:checked')?.value || 'Selectable Excluding Ghosts';
  appendLog(`Running Category Analyzer for ${paths.length} path(s) [${store} / ${filter}]...`);
  showTab('log');

  const btn = document.getElementById('btn-run-category');
  btn.disabled = true;

  try {
    const result = await invoke('analyze_categories', {
      request: { paths, store, filter }
    });
    if (result.success) {
      currentMarkdown = result.markdown;
      mdOutput.textContent = currentMarkdown;
      showTab('markdown');
    } else {
      appendLog('✗ ' + result.error);
    }
  } catch (e) {
    appendLog('✗ ' + String(e));
  } finally {
    btn.disabled = false;
  }
});

// ── CSV Analyzer ──────────────────────────────────────────────────────────────
const csvDrop = document.getElementById('csv-drop');
const csvFilename = document.getElementById('csv-filename');

csvDrop.addEventListener('dragover', e => { e.preventDefault(); csvDrop.classList.add('drag-over'); });
csvDrop.addEventListener('dragleave', () => csvDrop.classList.remove('drag-over'));
csvDrop.addEventListener('drop', e => {
  e.preventDefault();
  csvDrop.classList.remove('drag-over');
  const file = e.dataTransfer.files[0];
  if (file) loadCsvFile(file);
});
csvDrop.addEventListener('click', () => {
  const input = document.createElement('input');
  input.type = 'file';
  input.accept = '.csv';
  input.onchange = () => { if (input.files[0]) loadCsvFile(input.files[0]); };
  input.click();
});

function loadCsvFile(file) {
  const reader = new FileReader();
  reader.onload = e => {
    csvContent = e.target.result;
    const m = file.name.match(/^COMPETITION ANALYZER - EBOOK (.+?) \d{4}/i);
    csvKeyword = m ? titleCase(m[1]) : file.name.replace(/\.csv$/i, '');
    csvFilename.textContent = `Loaded: ${file.name}`;
    csvFilename.style.display = 'block';
    csvDrop.textContent = 'Drop a different CSV or click to browse';
  };
  reader.readAsText(file);
}

document.getElementById('btn-run-csv').addEventListener('click', async () => {
  if (!csvContent) { appendCsvLog('No CSV loaded.'); return; }

  const { apiKey, model } = getSettings();
  if (!apiKey) {
    appendCsvLog('✗ No API key set. Go to Settings and add your Anthropic API key.');
    showCsvTab('csv-log');
    return;
  }

  appendCsvLog(`Running CSV Analyzer for keyword: ${csvKeyword}...`);
  showCsvTab('csv-log');

  const btn = document.getElementById('btn-run-csv');
  btn.disabled = true;

  try {
    const result = await invoke('analyze_csv', {
      request: { keyword: csvKeyword, csv_content: csvContent, api_key: apiKey, model }
    });
    if (result.success) {
      currentCsvMarkdown = result.markdown;
      csvMdOut.textContent = currentCsvMarkdown;
      showCsvTab('csv-markdown');
    } else {
      appendCsvLog('✗ ' + result.error);
    }
  } catch (e) {
    appendCsvLog('✗ ' + String(e));
  } finally {
    btn.disabled = false;
  }
});

// ── Copy buttons ──────────────────────────────────────────────────────────────
document.getElementById('btn-copy').addEventListener('click', async () => {
  if (!currentMarkdown) return;
  await writeText(currentMarkdown);
  const btn = document.getElementById('btn-copy');
  btn.textContent = 'Copied!';
  setTimeout(() => { btn.textContent = 'Copy markdown'; }, 1500);
});

document.getElementById('btn-copy-csv').addEventListener('click', async () => {
  if (!currentCsvMarkdown) return;
  await writeText(currentCsvMarkdown);
  const btn = document.getElementById('btn-copy-csv');
  btn.textContent = 'Copied!';
  setTimeout(() => { btn.textContent = 'Copy markdown'; }, 1500);
});

// ── Helpers ───────────────────────────────────────────────────────────────────
function appendLog(msg) {
  logOutput.textContent += (logOutput.textContent ? '\n' : '') + msg;
  logOutput.scrollTop = logOutput.scrollHeight;
}

function appendCsvLog(msg) {
  csvLogOut.textContent += (csvLogOut.textContent ? '\n' : '') + msg;
  csvLogOut.scrollTop = csvLogOut.scrollHeight;
}

function showTab(name) {
  document.querySelectorAll('[data-tab="log"],[data-tab="markdown"]').forEach(t => {
    t.classList.toggle('active', t.dataset.tab === name);
  });
  document.getElementById('log-pane').classList.toggle('hidden', name !== 'log');
  document.getElementById('markdown-pane').classList.toggle('hidden', name !== 'markdown');
}

function showCsvTab(name) {
  document.querySelectorAll('[data-tab="csv-log"],[data-tab="csv-markdown"]').forEach(t => {
    t.classList.toggle('active', t.dataset.tab === name);
  });
  document.getElementById('csv-log-pane').classList.toggle('hidden', name !== 'csv-log');
  document.getElementById('csv-markdown-pane').classList.toggle('hidden', name !== 'csv-markdown');
}

function titleCase(str) {
  return str.toLowerCase().split(' ').map(w => w.charAt(0).toUpperCase() + w.slice(1)).join(' ');
}

// ── Init ──────────────────────────────────────────────────────────────────────
loadSettings();
