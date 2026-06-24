import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { readDir, readTextFile } from '@tauri-apps/plugin-fs';

document.addEventListener('contextmenu', (e) => {
  const tag = e.target.tagName;
  if (!['INPUT', 'TEXTAREA', 'SELECT'].includes(tag) && !e.target.isContentEditable) {
    e.preventDefault();
  }
});

// ── State ─────────────────────────────────────────────────────────────────────
let currentMarkdown      = '';
let currentCsvMarkdown   = '';
let currentGenreMarkdown = '';
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

// ── Settings ──────────────────────────────────────────────────────────────────
function loadSettings() {
  document.getElementById('api-key').value      = localStorage.getItem('apiKey') || '';
  document.getElementById('model-select').value = localStorage.getItem('model')  || 'claude-sonnet-4-6';
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

// ── Rocket status ─────────────────────────────────────────────────────────────
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
  } catch { statusLabel.textContent = 'Status unknown'; }
}

btnLaunch.addEventListener('click', async () => {
  btnLaunch.disabled = true;
  btnLaunch.textContent = 'Launching…';
  appendLog('Launching Publisher Rocket...');
  const result = await invoke('launch_rocket');
  if (result.success) { appendLog('✓ Publisher Rocket is ready.'); refreshStatus(); }
  else { appendLog('✗ ' + result.error); btnLaunch.disabled = false; btnLaunch.textContent = 'Retry'; }
});

setInterval(refreshStatus, 5000);
refreshStatus();

// ── Navigation ────────────────────────────────────────────────────────────────
document.querySelectorAll('.nav-item').forEach(btn => {
  btn.addEventListener('click', () => {
    const target = btn.dataset.panel;
    document.querySelectorAll('.nav-item').forEach(b => b.classList.toggle('active', b === btn));
    document.querySelectorAll('.panel').forEach(p => { p.classList.toggle('visible', p.id === target + '-panel'); });
  });
});

// ── Tab switching ─────────────────────────────────────────────────────────────
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

setupTabs('[data-tab="log"],[data-tab="markdown"]',                                  { log: 'log-pane', markdown: 'markdown-pane' });
setupTabs('[data-tab="csv-log"],[data-tab="csv-markdown"]',                          { 'csv-log': 'csv-log-pane', 'csv-markdown': 'csv-markdown-pane' });
setupTabs('[data-tab="genre-log"],[data-tab="genre-preview"],[data-tab="genre-raw"]',{ 'genre-log': 'genre-log-pane', 'genre-preview': 'genre-preview-pane', 'genre-raw': 'genre-raw-pane' });
setupTabs('[data-tab="finder-log"],[data-tab="finder-markdown"]',                    { 'finder-log': 'finder-log-pane', 'finder-markdown': 'finder-markdown-pane' });

// ── Genre Analyzer ─────────────────────────────────────────────────────────
const savedFolder = localStorage.getItem('manuscriptFolder');
if (savedFolder) document.getElementById('manuscript-folder').value = savedFolder;

document.getElementById('manuscript-folder').addEventListener('change', (e) => {
  const folder = e.target.value.trim();
  localStorage.setItem('manuscriptFolder', folder);
  refreshAnalysisState(folder);
});

document.getElementById('btn-pick-folder').addEventListener('click', async () => {
  try {
    const path = await invoke('pick_manuscript_folder');
    if (path) {
      document.getElementById('manuscript-folder').value = path;
      localStorage.setItem('manuscriptFolder', path);
      refreshAnalysisState(path);
    }
  } catch (e) { if (!String(e).includes('No folder')) appendGenreLog('✗ ' + String(e)); }
});

function getGenreFolder() { return document.getElementById('manuscript-folder').value.trim(); }

function setGenreReport(markdown) {
  currentGenreMarkdown = markdown;
  document.getElementById('genre-raw-output').textContent = markdown;
  document.getElementById('genre-preview-content').innerHTML =
    typeof marked !== 'undefined' ? marked.parse(markdown) : '<pre>' + markdown + '</pre>';
  showGenreTab('genre-preview');
}

function disableGenreButtons(disabled) {
  ['btn-run-everything','btn-gen-summaries','btn-run-genre','btn-full-analysis',
   'btn-optimize-keywords','btn-gen-pr-keywords','btn-analyze-competition'].forEach(id => {
    const el = document.getElementById(id); if (el) el.disabled = disabled;
  });
  document.getElementById('btn-stop').style.display = disabled ? 'block' : 'none';
  if (!disabled) { const f = getGenreFolder(); if (f) refreshAnalysisState(f); }
}

async function refreshAnalysisState(folder) {
  if (!folder) {
    ['btn-run-everything','btn-run-genre','btn-full-analysis',
     'btn-optimize-keywords','btn-gen-pr-keywords','btn-analyze-competition'].forEach(id => {
      const el = document.getElementById(id); if (el) el.disabled = true;
    });
    document.getElementById('btn-gen-summaries').disabled = false;
    updateButtonLabels(null);
    return;
  }
  try {
    const s = await invoke('check_analysis_state', { folder });
    document.getElementById('btn-run-everything').disabled      = s.summary_count === 0;
    document.getElementById('btn-analyze-competition').disabled = !s.has_pr_keywords;
    document.getElementById('btn-gen-summaries').disabled       = false;
    document.getElementById('btn-run-genre').disabled           = s.summary_count === 0;
    document.getElementById('btn-full-analysis').disabled       = !s.has_genre_data;
    document.getElementById('btn-optimize-keywords').disabled   = !s.has_full_report;
    document.getElementById('btn-gen-pr-keywords').disabled     = !s.has_full_report;
    updateButtonLabels(s);
  } catch (e) { console.error('check_analysis_state:', e); }
}

function updateButtonLabels(state) {
  [
    ['btn-run-everything',     '\u25b6 Run Analysis',         state?.has_pr_keywords  ? ' \u2713' : ''],
    ['btn-gen-summaries',      'Summaries',                   state?.summary_count > 0 ? ` (${state?.summary_count})` : ''],
    ['btn-run-genre',          'Analyze',                     state?.has_genre_data   ? ' \u2713' : ''],
    ['btn-full-analysis',      'Full Analysis',               state?.has_full_report  ? ' \u2713' : ''],
    ['btn-optimize-keywords',  'KDP Keywords',                state?.has_keywords     ? ' \u2713' : ''],
    ['btn-gen-pr-keywords',    'PR Keywords',                 state?.has_pr_keywords  ? ' \u2713' : ''],
    ['btn-analyze-competition','\u25b6 Analyze Competition',  state?.has_competition  ? ' \u2713' : ''],
  ].forEach(([id, base, suffix]) => {
    const btn = document.getElementById(id);
    if (btn) btn.textContent = base + suffix;
  });
}

// ► Run Analysis — chains everything except folder pick and chapter summaries
document.getElementById('btn-run-everything').addEventListener('click', async () => {
  const folder = getGenreFolder();
  if (!folder) { appendGenreLog('✗ Please select a manuscript folder first.'); return; }
  const { apiKey, model } = getSettings();
  if (!apiKey) { appendGenreLog('✗ No API key set. Go to Settings.'); return; }
  appendGenreLog(`Running full analysis for: ${folder}`);
  appendGenreLog('Genre → KDP keywords → PR search terms...');
  showGenreTab('genre-log');
  disableGenreButtons(true);
  try {
    const result = await invoke('run_everything', { request: { folder, api_key: apiKey, model } });
    if (result.success) {
      setGenreReport(result.report);
      appendGenreLog('✓ Analysis complete. Run ► Analyze Competition next.');
    } else { appendGenreLog('✗ ' + result.error); }
  } catch (e) { appendGenreLog('✗ ' + String(e)); }
  finally { disableGenreButtons(false); }
});

// Generate Summaries only
document.getElementById('btn-gen-summaries').addEventListener('click', async () => {
  const folder = getGenreFolder();
  if (!folder) { appendGenreLog('✗ Please select a manuscript folder first.'); return; }
  const { apiKey, model } = getSettings();
  if (!apiKey) { appendGenreLog('✗ No API key set. Go to Settings.'); return; }
  appendGenreLog(`Generating summaries for: ${folder}`);
  showGenreTab('genre-log');
  disableGenreButtons(true);
  try {
    const result = await invoke('generate_summaries', { request: { folder, api_key: apiKey, model } });
    appendGenreLog(result.success ? '✓ ' + result.report : '✗ ' + result.error);
  } catch (e) { appendGenreLog('✗ ' + String(e)); }
  finally { disableGenreButtons(false); }
});

// Analyze — genre + KDP, no PR
document.getElementById('btn-run-genre').addEventListener('click', async () => {
  const folder = getGenreFolder();
  if (!folder) { appendGenreLog('✗ Please select a manuscript folder first.'); return; }
  const { apiKey, model } = getSettings();
  if (!apiKey) { appendGenreLog('✗ No API key set. Go to Settings.'); return; }
  appendGenreLog(`Running genre analysis for: ${folder}`);
  showGenreTab('genre-log');
  disableGenreButtons(true);
  try {
    const result = await invoke('analyze_genre', { request: { folder, api_key: apiKey, model } });
    if (result.success) { setGenreReport(result.report); appendGenreLog('✓ Genre analysis complete.'); }
    else { appendGenreLog('✗ ' + result.error); }
  } catch (e) { appendGenreLog('✗ ' + String(e)); }
  finally { disableGenreButtons(false); }
});

// Full Analysis — summaries + genre + PR Category Search
document.getElementById('btn-full-analysis').addEventListener('click', async () => {
  const folder = getGenreFolder();
  if (!folder) { appendGenreLog('✗ Please select a manuscript folder first.'); return; }
  const { apiKey, model } = getSettings();
  if (!apiKey) { appendGenreLog('✗ No API key set. Go to Settings.'); return; }
  appendGenreLog(`Full analysis for: ${folder}`);
  showGenreTab('genre-log');
  disableGenreButtons(true);
  try {
    const result = await invoke('run_full_analysis', { request: { folder, api_key: apiKey, model } });
    if (result.success) { setGenreReport(result.report); appendGenreLog('✓ Full analysis complete.'); }
    else { appendGenreLog('✗ ' + result.error); }
  } catch (e) { appendGenreLog('✗ ' + String(e)); }
  finally { disableGenreButtons(false); }
});

// Optimize KDP Keywords
document.getElementById('btn-optimize-keywords').addEventListener('click', async () => {
  const folder = getGenreFolder();
  if (!folder) { appendGenreLog('✗ Please select a manuscript folder first.'); return; }
  const { apiKey, model } = getSettings();
  if (!apiKey) { appendGenreLog('✗ No API key set. Go to Settings.'); return; }
  appendGenreLog('Optimizing KDP keywords...');
  showGenreTab('genre-log');
  disableGenreButtons(true);
  try {
    const result = await invoke('optimize_keywords', { request: { folder, api_key: apiKey, model } });
    if (result.success) { setGenreReport(result.report); appendGenreLog('✓ KDP keywords complete.'); }
    else { appendGenreLog('✗ ' + result.error); }
  } catch (e) { appendGenreLog('✗ ' + String(e)); }
  finally { disableGenreButtons(false); }
});

// Generate PR Keywords
document.getElementById('btn-gen-pr-keywords').addEventListener('click', async () => {
  const folder = getGenreFolder();
  if (!folder) { appendGenreLog('✗ Please select a manuscript folder first.'); return; }
  const { apiKey, model } = getSettings();
  if (!apiKey) { appendGenreLog('✗ No API key set. Go to Settings.'); return; }
  appendGenreLog('Generating PR search terms...');
  showGenreTab('genre-log');
  disableGenreButtons(true);
  try {
    const result = await invoke('generate_pr_keywords', { request: { folder, api_key: apiKey, model } });
    if (result.success) { setGenreReport(result.report); appendGenreLog('✓ PR keywords generated.'); }
    else { appendGenreLog('✗ ' + result.error); }
  } catch (e) { appendGenreLog('✗ ' + String(e)); }
  finally { disableGenreButtons(false); }
});

// Stop
document.getElementById('btn-stop').addEventListener('click', async () => {
  appendGenreLog('Stopping after current step...');
  await invoke('cancel_operation');
});

// Show store selector on hover for competition button
document.getElementById('btn-analyze-competition').addEventListener('mouseenter', () => {
  document.getElementById('competition-store-row').style.display = 'flex';
});

// ► Analyze Competition
document.getElementById('btn-analyze-competition').addEventListener('click', async () => {
  const folder = getGenreFolder();
  if (!folder) { appendGenreLog('✗ Please select a manuscript folder first.'); return; }
  const { apiKey, model } = getSettings();
  if (!apiKey) { appendGenreLog('✗ No API key set. Go to Settings.'); return; }
  const store = document.querySelector('input[name="comp-store"]:checked')?.value || 'Kindle';
  appendGenreLog(`Analyzing competition [${store}] via Publisher Rocket CSV exports...`);
  appendGenreLog('This may take several minutes.');
  showGenreTab('genre-log');
  disableGenreButtons(true);
  try {
    const result = await invoke('analyze_competition', { request: { folder, api_key: apiKey, model, store } });
    if (result.success) { setGenreReport(result.report); appendGenreLog('✓ Competition analysis complete.'); }
    else { appendGenreLog('✗ ' + result.error); }
  } catch (e) { appendGenreLog('✗ ' + String(e)); }
  finally { disableGenreButtons(false); }
});

// Copy genre report
document.getElementById('btn-copy-genre').addEventListener('click', async () => {
  if (!currentGenreMarkdown) return;
  await writeText(currentGenreMarkdown);
  const btn = document.getElementById('btn-copy-genre');
  btn.textContent = 'Copied!';
  setTimeout(() => { btn.textContent = 'Copy markdown'; }, 1500);
});

listen('genre:log', (event) => { appendGenreLog(event.payload); });

function appendGenreLog(msg) {
  const el = document.getElementById('genre-log-output');
  el.textContent += (el.textContent ? '\n' : '') + msg;
  el.scrollTop = el.scrollHeight;
}

function showGenreTab(name) {
  document.querySelectorAll('[data-tab="genre-log"],[data-tab="genre-preview"],[data-tab="genre-raw"]')
    .forEach(t => t.classList.toggle('active', t.dataset.tab === name));
  document.getElementById('genre-log-pane').classList.toggle('hidden',     name !== 'genre-log');
  document.getElementById('genre-preview-pane').classList.toggle('hidden', name !== 'genre-preview');
  document.getElementById('genre-raw-pane').classList.toggle('hidden',     name !== 'genre-raw');
}

// ── Category Finder ───────────────────────────────────────────────────────────
let currentFinderMarkdown = '';

document.getElementById('btn-run-finder').addEventListener('click', async () => {
  const genre  = document.getElementById('genre-description').value.trim();
  if (!genre) return;
  const store  = document.querySelector('input[name="finder-store"]:checked')?.value  || 'Kindle';
  const filter = document.querySelector('input[name="finder-filter"]:checked')?.value || 'Selectable Excluding Ghosts';
  const { apiKey, model } = getSettings();
  if (!apiKey) { appendFinderLog('✗ No API key set.'); showFinderTab('finder-log'); return; }
  appendFinderLog(`Finding categories for: "${genre}"`);
  showFinderTab('finder-log');
  const btn = document.getElementById('btn-run-finder');
  btn.disabled = true;
  try {
    const result = await invoke('find_categories', { request: { genre, store, filter, api_key: apiKey, model } });
    if (result.success) {
      currentFinderMarkdown = result.markdown;
      document.getElementById('finder-markdown-output').textContent = currentFinderMarkdown;
      showFinderTab('finder-markdown');
    } else { appendFinderLog('✗ ' + result.error); }
  } catch (e) { appendFinderLog('✗ ' + String(e)); }
  finally { btn.disabled = false; }
});

document.getElementById('btn-copy-finder').addEventListener('click', async () => {
  if (!currentFinderMarkdown) return;
  await writeText(currentFinderMarkdown);
  const btn = document.getElementById('btn-copy-finder');
  btn.textContent = 'Copied!';
  setTimeout(() => { btn.textContent = 'Copy results'; }, 1500);
});

listen('cdp:log', (event) => {
  const finderVisible = document.getElementById('finder-panel').classList.contains('visible');
  if (finderVisible) appendFinderLog(event.payload);
  appendLog(event.payload);
  appendCsvLog(event.payload);
});

function appendFinderLog(msg) {
  const el = document.getElementById('finder-log-output');
  el.textContent += (el.textContent ? '\n' : '') + msg;
  el.scrollTop = el.scrollHeight;
}

function showFinderTab(name) {
  document.querySelectorAll('[data-tab="finder-log"],[data-tab="finder-markdown"]')
    .forEach(t => t.classList.toggle('active', t.dataset.tab === name));
  document.getElementById('finder-log-pane').classList.toggle('hidden', name !== 'finder-log');
  document.getElementById('finder-markdown-pane').classList.toggle('hidden', name !== 'finder-markdown');
}

// ── Category Analyzer ─────────────────────────────────────────────────────────
document.getElementById('btn-run-category').addEventListener('click', async () => {
  const raw   = document.getElementById('category-paths').value.trim();
  if (!raw) return;
  const paths = raw.split('\n').map(l => l.trim()).filter(Boolean);
  const store  = document.querySelector('input[name="store"]:checked')?.value  || 'Kindle';
  const filter = document.querySelector('input[name="filter"]:checked')?.value || 'Selectable Excluding Ghosts';
  appendLog(`Running Category Analyzer for ${paths.length} path(s) [${store}]...`);
  showTab('log');
  const btn = document.getElementById('btn-run-category');
  btn.disabled = true;
  try {
    const result = await invoke('analyze_categories', { request: { paths, store, filter } });
    if (result.success) { currentMarkdown = result.markdown; mdOutput.textContent = currentMarkdown; showTab('markdown'); }
    else { appendLog('✗ ' + result.error); }
  } catch (e) { appendLog('✗ ' + String(e)); }
  finally { btn.disabled = false; }
});

// ── CSV Analyzer ──────────────────────────────────────────────────────────────
const csvDrop     = document.getElementById('csv-drop');
const csvFilename = document.getElementById('csv-filename');

csvDrop.addEventListener('dragover',  e => { e.preventDefault(); csvDrop.classList.add('drag-over'); });
csvDrop.addEventListener('dragleave', () => csvDrop.classList.remove('drag-over'));
csvDrop.addEventListener('drop', e => { e.preventDefault(); csvDrop.classList.remove('drag-over'); const file = e.dataTransfer.files[0]; if (file) loadCsvFile(file); });
csvDrop.addEventListener('click', () => { const input = document.createElement('input'); input.type = 'file'; input.accept = '.csv'; input.onchange = () => { if (input.files[0]) loadCsvFile(input.files[0]); }; input.click(); });

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
  if (!apiKey) { appendCsvLog('✗ No API key set.'); showCsvTab('csv-log'); return; }
  appendCsvLog(`Running CSV Analyzer for: ${csvKeyword}...`);
  showCsvTab('csv-log');
  const btn = document.getElementById('btn-run-csv');
  btn.disabled = true;
  try {
    const result = await invoke('analyze_csv', { request: { keyword: csvKeyword, csv_content: csvContent, api_key: apiKey, model } });
    if (result.success) { currentCsvMarkdown = result.markdown; csvMdOut.textContent = currentCsvMarkdown; showCsvTab('csv-markdown'); }
    else { appendCsvLog('✗ ' + result.error); }
  } catch (e) { appendCsvLog('✗ ' + String(e)); }
  finally { btn.disabled = false; }
});

// ── Copy buttons ──────────────────────────────────────────────────────────────
document.getElementById('btn-copy').addEventListener('click', async () => {
  if (!currentMarkdown) return;
  await writeText(currentMarkdown);
  const btn = document.getElementById('btn-copy'); btn.textContent = 'Copied!';
  setTimeout(() => { btn.textContent = 'Copy markdown'; }, 1500);
});

document.getElementById('btn-copy-csv').addEventListener('click', async () => {
  if (!currentCsvMarkdown) return;
  await writeText(currentCsvMarkdown);
  const btn = document.getElementById('btn-copy-csv'); btn.textContent = 'Copied!';
  setTimeout(() => { btn.textContent = 'Copy markdown'; }, 1500);
});

// ── Helpers ───────────────────────────────────────────────────────────────────
function appendLog(msg) { logOutput.textContent += (logOutput.textContent ? '\n' : '') + msg; logOutput.scrollTop = logOutput.scrollHeight; }
function appendCsvLog(msg) { csvLogOut.textContent += (csvLogOut.textContent ? '\n' : '') + msg; csvLogOut.scrollTop = csvLogOut.scrollHeight; }

function showTab(name) {
  document.querySelectorAll('[data-tab="log"],[data-tab="markdown"]').forEach(t => t.classList.toggle('active', t.dataset.tab === name));
  document.getElementById('log-pane').classList.toggle('hidden',      name !== 'log');
  document.getElementById('markdown-pane').classList.toggle('hidden', name !== 'markdown');
}

function showCsvTab(name) {
  document.querySelectorAll('[data-tab="csv-log"],[data-tab="csv-markdown"]').forEach(t => t.classList.toggle('active', t.dataset.tab === name));
  document.getElementById('csv-log-pane').classList.toggle('hidden',      name !== 'csv-log');
  document.getElementById('csv-markdown-pane').classList.toggle('hidden', name !== 'csv-markdown');
}

function titleCase(str) { return str.toLowerCase().split(' ').map(w => w.charAt(0).toUpperCase() + w.slice(1)).join(' '); }

// ── Clear log buttons ─────────────────────────────────────────────────────────
[['btn-clear-genre-log','genre-log-output'],['btn-clear-log','log-output'],
 ['btn-clear-csv-log','csv-log-output'],['btn-clear-finder-log','finder-log-output'],
].forEach(([btnId, outputId]) => {
  document.getElementById(btnId).addEventListener('click', () => { document.getElementById(outputId).textContent = ''; });
});

// ── Reports ──────────────────────────────────────────────────────────────────

const REPORT_LABELS = {
  'genre-analysis.md':    { label: 'Genre Analysis',       order: 1 },
  'full-report.md':       { label: 'Full Report',           order: 2 },
  'kdp-keywords.md':      { label: 'KDP Keywords',          order: 3 },
  'competition-report.md':{ label: 'Competition Analysis',  order: 4 },
};

async function loadReportsList() {
  const folder = getGenreFolder();
  const note   = document.getElementById('reports-folder-note');
  const list   = document.getElementById('reports-list');

  list.innerHTML = '';
  showReportsList();

  if (!folder) {
    note.textContent = 'No manuscript folder selected. Go to Analyzer and pick a folder.';
    note.style.display = 'block';
    return;
  }

  const analysisPath = folder + '/_analysis';
  note.style.display = 'none';

  try {
    const entries = await readDir(analysisPath);
    const mdFiles = entries
      .filter(e => e.name && e.name.endsWith('.md'))
      .map(e => e.name)
      .sort((a, b) => {
        const oa = REPORT_LABELS[a]?.order ?? 99;
        const ob = REPORT_LABELS[b]?.order ?? 99;
        return oa - ob;
      });

    if (mdFiles.length === 0) {
      note.textContent = 'No reports yet. Run the Analyzer to generate reports.';
      note.style.display = 'block';
      return;
    }

    mdFiles.forEach(filename => {
      const info  = REPORT_LABELS[filename];
      const label = info?.label ?? filename.replace('.md', '').replace(/-/g, ' ').replace(/\b\w/g, c => c.toUpperCase());

      const item = document.createElement('div');
      item.className = 'report-item';
      item.innerHTML = `
        <div>
          <div class="report-item-name">${label}</div>
          <div class="report-item-meta">${filename}</div>
        </div>
        <span class="report-item-arrow">›</span>
      `;
      item.addEventListener('click', () => openReport(analysisPath + '/' + filename, label));
      list.appendChild(item);
    });

  } catch (e) {
    note.textContent = 'No reports folder found. Run the Analyzer first.';
    note.style.display = 'block';
  }
}

async function openReport(path, label) {
  try {
    const content = await readTextFile(path);
    document.getElementById('reports-viewer-title').textContent = label;
    document.getElementById('reports-viewer-content').innerHTML =
      typeof marked !== 'undefined' ? marked.parse(content) : '<pre>' + content + '</pre>';
    document.getElementById('reports-viewer').dataset.content = content;
    showReportsViewer();
  } catch (e) {
    alert('Could not read report: ' + String(e));
  }
}

function showReportsList() {
  document.getElementById('reports-list').classList.remove('hidden');
  document.getElementById('reports-folder-note').classList.remove('hidden');
  document.getElementById('reports-viewer').classList.add('hidden');
}

function showReportsViewer() {
  document.getElementById('reports-list').classList.add('hidden');
  document.getElementById('reports-folder-note').classList.add('hidden');
  document.getElementById('reports-viewer').classList.remove('hidden');
}

document.getElementById('btn-reports-back').addEventListener('click', showReportsList);

document.getElementById('btn-reports-copy').addEventListener('click', async () => {
  const content = document.getElementById('reports-viewer').dataset.content || '';
  if (!content) return;
  await writeText(content);
  const btn = document.getElementById('btn-reports-copy');
  btn.textContent = 'Copied!';
  setTimeout(() => { btn.textContent = 'Copy'; }, 1500);
});

// Reload reports list whenever Reports panel becomes active
const _origNavHandler = document.querySelectorAll('.nav-item');
document.querySelectorAll('.nav-item').forEach(btn => {
  if (btn.dataset.panel === 'reports') {
    btn.addEventListener('click', loadReportsList);
  }
});

// Also reload after any analysis completes
const _origDisable = disableGenreButtons;
function disableGenreButtonsWrapped(disabled) {
  _origDisable(disabled);
  if (!disabled) {
    // Refresh reports list in background if reports panel is active
    const reportsVisible = document.getElementById('reports-panel').classList.contains('visible');
    if (reportsVisible) loadReportsList();
  }
}

// ── Init ──────────────────────────────────────────────────────────────────────
loadSettings();
const _initFolder = getGenreFolder();
if (_initFolder) refreshAnalysisState(_initFolder);
else refreshAnalysisState('');
