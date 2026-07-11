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

// ── App State ─────────────────────────────────────────────────────────────────
let currentGenreMarkdown = '';
let activeStoryId        = localStorage.getItem('activeStoryId') || null;
let allStories           = [];

// ── DOM refs ──────────────────────────────────────────────────────────────────
const statusDot   = document.getElementById('status-dot');
const statusLabel = document.getElementById('status-label');
const btnLaunch   = document.getElementById('btn-launch');

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
  setTimeout(() => { saved.textContent = ''; }, 1500);
  // Close settings — return to previous panel
  const prevPanel = localStorage.getItem('prevPanel') || 'analyzer';
  showPanel(prevPanel);
});

function getSettings() {
  return {
    apiKey: localStorage.getItem('apiKey') || '',
    model:  localStorage.getItem('model')  || 'claude-sonnet-4-6',
  };
}

// ── Active story ──────────────────────────────────────────────────────────────
function getActiveStory() {
  return allStories.find(s => s.id === activeStoryId) || null;
}

function getActiveFolder() {
  return getActiveStory()?.folder || '';
}

function setActiveStory(id) {
  activeStoryId = id;
  localStorage.setItem('activeStoryId', id || '');
  renderStoriesList();
  updateAnalyzerDesc();
  if (id) {
    refreshAnalysisState(getActiveFolder());
  } else {
    disableAllButtons();
  }
  // Refresh reports if that panel is visible
  const reportsVisible = document.getElementById('reports-panel').classList.contains('visible');
  if (reportsVisible) loadReportsList();
}

function updateAnalyzerDesc() {
  const story = getActiveStory();
  const desc  = document.getElementById('analyzer-desc');
  if (story) {
    desc.textContent = `Story: ${story.name}`;
  } else {
    desc.textContent = 'Select or create a story to begin.';
  }
}

// ── Stories ───────────────────────────────────────────────────────────────────
async function loadStoriesFromDisk() {
  const result = await invoke('list_stories');
  allStories = result.success ? result.stories : [];
  renderStoriesList();
  updateAnalyzerDesc();
  // If saved active story still exists, refresh its state
  if (activeStoryId && allStories.find(s => s.id === activeStoryId)) {
    refreshAnalysisState(getActiveFolder());
  } else if (activeStoryId) {
    // Story was deleted — clear selection
    setActiveStory(null);
  } else {
    disableAllButtons();
  }
}

function renderStoriesList() {
  const list = document.getElementById('stories-list');
  list.innerHTML = '';

  if (allStories.length === 0) {
    list.innerHTML = '<div style="padding:8px 10px;font-size:11px;color:var(--text-muted)">No stories yet. Click + to add one.</div>';
    return;
  }

  allStories.forEach(story => {
    const item = document.createElement('div');
    item.className = 'story-item' + (story.id === activeStoryId ? ' active' : '');
    item.title = story.folder;
    item.innerHTML = `
      <span class="story-item-name">${story.name}</span>
      <button class="story-item-edit" data-id="${story.id}" title="Edit story">✎</button>
    `;
    item.addEventListener('click', (e) => {
      if (e.target.classList.contains('story-item-edit')) return;
      setActiveStory(story.id);
      showPanel('analyzer');
    });
    item.querySelector('.story-item-edit').addEventListener('click', (e) => {
      e.stopPropagation();
      openStoryForm(story);
    });
    list.appendChild(item);
  });
}

// ── Story form ────────────────────────────────────────────────────────────────
function openStoryForm(story = null) {
  document.getElementById('story-form-id').value      = story?.id    || '';
  document.getElementById('story-name').value         = story?.name  || '';
  document.getElementById('story-folder').value       = story?.folder|| '';
  document.getElementById('story-form-title').textContent = story ? 'Edit Story' : 'New Story';
  document.getElementById('btn-story-delete').style.display = story ? 'block' : 'none';
  document.getElementById('story-form-error').textContent = '';
  showPanel('story-form');
}

document.getElementById('btn-new-story').addEventListener('click', () => openStoryForm());

document.getElementById('btn-story-pick-folder').addEventListener('click', async () => {
  try {
    const path = await invoke('pick_manuscript_folder');
    if (path) document.getElementById('story-folder').value = path;
  } catch (e) {
    if (!String(e).includes('No folder')) {
      document.getElementById('story-form-error').textContent = String(e);
    }
  }
});

document.getElementById('btn-story-save').addEventListener('click', async () => {
  const id     = document.getElementById('story-form-id').value;
  const name   = document.getElementById('story-name').value.trim();
  const folder = document.getElementById('story-folder').value.trim();
  const errEl  = document.getElementById('story-form-error');

  if (!name)   { errEl.textContent = 'Please enter a story name.'; return; }
  if (!folder) { errEl.textContent = 'Please select a folder.'; return; }

  errEl.textContent = '';

  let result;
  if (id) {
    result = await invoke('update_story', { request: { id, name, folder } });
  } else {
    result = await invoke('add_story', { request: { name, folder } });
  }

  if (!result.success) {
    errEl.textContent = result.error;
    return;
  }

  allStories = result.stories;
  // Select the new/updated story
  const saved = result.stories.find(s => s.name === name && s.folder === folder);
  if (saved) setActiveStory(saved.id);

  renderStoriesList();
  showPanel('analyzer');
});

document.getElementById('btn-story-cancel').addEventListener('click', () => {
  showPanel(activeStoryId ? 'analyzer' : 'analyzer');
});

document.getElementById('btn-story-delete').addEventListener('click', async () => {
  const id = document.getElementById('story-form-id').value;
  if (!id) return;
  if (!confirm('Remove this story from the list? (The folder and files will not be deleted.)')) return;

  const result = await invoke('delete_story', { id });
  if (result.success) {
    allStories = result.stories;
    if (activeStoryId === id) setActiveStory(null);
    renderStoriesList();
    showPanel('analyzer');
  } else {
    document.getElementById('story-form-error').textContent = result.error;
  }
});

// ── Panel navigation ──────────────────────────────────────────────────────────
function showPanel(name) {
  const currentlyVisible = document.querySelector('.panel.visible')?.id?.replace('-panel', '');
  // Settings and Reports are toggles
  if ((name === 'settings' || name === 'reports') && currentlyVisible === name) {
    const prev = localStorage.getItem('prevPanel') || 'analyzer';
    showPanel(prev);
    return;
  }
  // Remember where we came from before opening settings or reports
  if ((name === 'settings' || name === 'reports') && currentlyVisible && currentlyVisible !== name) {
    localStorage.setItem('prevPanel', currentlyVisible);
  }
  document.querySelectorAll('.panel').forEach(p => {
    p.classList.toggle('visible', p.id === name + '-panel');
  });
  document.querySelectorAll('.nav-item').forEach(b => {
    b.classList.toggle('active', b.dataset.panel === name);
  });
  if (name === 'reports') loadReportsList();
}

document.querySelectorAll('.nav-item').forEach(btn => {
  btn.addEventListener('click', () => showPanel(btn.dataset.panel));
});

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
  btnLaunch.disabled = true; btnLaunch.textContent = 'Launching…';
  const result = await invoke('launch_rocket');
  if (result.success) { refreshStatus(); }
  else { btnLaunch.disabled = false; btnLaunch.textContent = 'Retry'; }
});

setInterval(refreshStatus, 5000);
refreshStatus();

// ── Tab switching ─────────────────────────────────────────────────────────────
document.querySelectorAll('[data-tab="genre-log"],[data-tab="genre-preview"],[data-tab="genre-raw"]')
  .forEach(tab => {
    tab.addEventListener('click', () => {
      document.querySelectorAll('[data-tab="genre-log"],[data-tab="genre-preview"],[data-tab="genre-raw"]')
        .forEach(t => t.classList.toggle('active', t === tab));
      const name = tab.dataset.tab;
      document.getElementById('genre-log-pane').classList.toggle('hidden',     name !== 'genre-log');
      document.getElementById('genre-preview-pane').classList.toggle('hidden', name !== 'genre-preview');
      document.getElementById('genre-raw-pane').classList.toggle('hidden',     name !== 'genre-raw');
    });
  });

// ── Button state ──────────────────────────────────────────────────────────────
function disableAllButtons() {
  ['btn-run-everything','btn-analyze-competition','btn-gen-summaries',
   'btn-run-genre','btn-full-analysis','btn-optimize-keywords',
   'btn-gen-pr-keywords'].forEach(id => {
    const el = document.getElementById(id); if (el) el.disabled = true;
  });
  updateStepLabels(null);
}

function disableGenreButtons(disabled) {
  ['btn-run-everything','btn-analyze-competition','btn-gen-summaries',
   'btn-run-genre','btn-full-analysis','btn-optimize-keywords',
   'btn-gen-pr-keywords'].forEach(id => {
    const el = document.getElementById(id); if (el) el.disabled = disabled;
  });
  document.getElementById('btn-stop').style.display = disabled ? 'flex' : 'none';
  if (!disabled) {
    const f = getActiveFolder();
    if (f) refreshAnalysisState(f);
    // Refresh reports if visible
    if (document.getElementById('reports-panel').classList.contains('visible')) loadReportsList();
  }
}

async function refreshAnalysisState(folder) {
  if (!folder) { disableAllButtons(); return; }
  try {
    const s = await invoke('check_analysis_state', { folder });
    document.getElementById('btn-run-everything').disabled      = s.summary_count === 0;
    document.getElementById('btn-analyze-competition').disabled = !s.has_pr_keywords;
    document.getElementById('btn-gen-summaries').disabled       = false;
    document.getElementById('btn-run-genre').disabled           = s.summary_count === 0;
    document.getElementById('btn-full-analysis').disabled       = !s.has_genre_data;
    document.getElementById('btn-optimize-keywords').disabled   = !s.has_full_report;
    document.getElementById('btn-gen-pr-keywords').disabled     = !s.has_full_report;
    updateStepLabels(s);
  } catch (e) { console.error('check_analysis_state:', e); }
}

function updateStepLabels(state) {
  const steps = [
    ['btn-gen-summaries',      'Summaries',     state?.summary_count > 0 ? ` (${state.summary_count})\u2713` : ''],
    ['btn-run-genre',          'Analyze',       state?.has_genre_data   ? ' \u2713' : ''],
    ['btn-full-analysis',      'Full Analysis', state?.has_full_report  ? ' \u2713' : ''],
    ['btn-optimize-keywords',  'KDP Keywords',  state?.has_keywords     ? ' \u2713' : ''],
    ['btn-gen-pr-keywords',    'PR Keywords',   state?.has_pr_keywords  ? ' \u2713' : ''],
  ];
  steps.forEach(([id, base, suffix]) => {
    const btn = document.getElementById(id);
    if (btn) btn.textContent = base + suffix;
  });
  const rAll  = document.getElementById('btn-run-everything');
  const rComp = document.getElementById('btn-analyze-competition');
  if (rAll)  rAll.textContent  = '\u25b6 Run Analysis'        + (state?.has_pr_keywords ? ' \u2713' : '');
  if (rComp) rComp.textContent = '\u25b6 Analyze Competition' + (state?.has_competition ? ' \u2713' : '');
}

// ── Analyzer buttons ──────────────────────────────────────────────────────────
function setGenreReport(markdown) {
  currentGenreMarkdown = markdown;
  document.getElementById('genre-raw-output').textContent = markdown;
  document.getElementById('genre-preview-content').innerHTML =
    typeof marked !== 'undefined' ? marked.parse(markdown) : '<pre>' + markdown + '</pre>';
  // Switch to preview tab
  document.querySelectorAll('[data-tab="genre-log"],[data-tab="genre-preview"],[data-tab="genre-raw"]')
    .forEach(t => t.classList.toggle('active', t.dataset.tab === 'genre-preview'));
  document.getElementById('genre-log-pane').classList.add('hidden');
  document.getElementById('genre-preview-pane').classList.remove('hidden');
  document.getElementById('genre-raw-pane').classList.add('hidden');
}

function appendGenreLog(msg) {
  const el = document.getElementById('genre-log-output');
  el.textContent += (el.textContent ? '\n' : '') + msg;
  el.scrollTop = el.scrollHeight;
  // Switch to log tab
  document.querySelectorAll('[data-tab="genre-log"],[data-tab="genre-preview"],[data-tab="genre-raw"]')
    .forEach(t => t.classList.toggle('active', t.dataset.tab === 'genre-log'));
  document.getElementById('genre-log-pane').classList.remove('hidden');
  document.getElementById('genre-preview-pane').classList.add('hidden');
  document.getElementById('genre-raw-pane').classList.add('hidden');
}

async function runGenreCommand(cmdName, logMsg, successMsg) {
  const folder = getActiveFolder();
  if (!folder) { appendGenreLog('✗ No story selected.'); return; }
  const { apiKey, model } = getSettings();
  if (!apiKey) { appendGenreLog('✗ No API key set. Go to Settings.'); return; }
  appendGenreLog(logMsg);
  disableGenreButtons(true);
  try {
    const result = await invoke(cmdName, { request: { folder, api_key: apiKey, model } });
    if (result.success) { setGenreReport(result.report); appendGenreLog(successMsg); }
    else { appendGenreLog('✗ ' + result.error); }
  } catch (e) { appendGenreLog('✗ ' + String(e)); }
  finally { disableGenreButtons(false); }
}

document.getElementById('btn-run-everything').addEventListener('click', () =>
  runGenreCommand('run_everything', 'Running full analysis...', '✓ Analysis complete. Run ▶ Analyze Competition next.'));

document.getElementById('btn-gen-summaries').addEventListener('click', () =>
  runGenreCommand('generate_summaries', 'Generating chapter summaries...', '✓ Summaries complete.'));

document.getElementById('btn-run-genre').addEventListener('click', () =>
  runGenreCommand('analyze_genre', 'Running genre analysis...', '✓ Genre analysis complete.'));

document.getElementById('btn-full-analysis').addEventListener('click', () =>
  runGenreCommand('run_full_analysis', 'Running full analysis...', '✓ Full analysis complete.'));

document.getElementById('btn-optimize-keywords').addEventListener('click', () =>
  runGenreCommand('optimize_keywords', 'Optimizing KDP keywords...', '✓ KDP keywords complete.'));

document.getElementById('btn-gen-pr-keywords').addEventListener('click', () =>
  runGenreCommand('generate_pr_keywords', 'Generating PR search terms...', '✓ PR keywords generated.'));

document.getElementById('btn-stop').addEventListener('click', async () => {
  appendGenreLog('Stopping after current step...');
  await invoke('cancel_operation');
});

// ── Analyze Competition ───────────────────────────────────────────────────────
document.getElementById('btn-analyze-competition').addEventListener('mouseenter', () => {
  document.getElementById('competition-store-row').style.display = 'flex';
});

document.getElementById('btn-analyze-competition').addEventListener('click', async () => {
  const folder = getActiveFolder();
  if (!folder) { appendGenreLog('✗ No story selected.'); return; }
  const { apiKey, model } = getSettings();
  if (!apiKey) { appendGenreLog('✗ No API key set. Go to Settings.'); return; }
  const store = document.querySelector('input[name="comp-store"]:checked')?.value || 'Kindle';
  appendGenreLog(`Analyzing competition [${store}] via Publisher Rocket...`);
  appendGenreLog('This may take several minutes.');
  disableGenreButtons(true);
  try {
    const result = await invoke('analyze_competition', { request: { folder, api_key: apiKey, model, store } });
    if (result.success) { setGenreReport(result.report); appendGenreLog('✓ Competition analysis complete.'); }
    else { appendGenreLog('✗ ' + result.error); }
  } catch (e) { appendGenreLog('✗ ' + String(e)); }
  finally { disableGenreButtons(false); }
});

document.getElementById('btn-copy-genre').addEventListener('click', async () => {
  if (!currentGenreMarkdown) return;
  await writeText(currentGenreMarkdown);
  const btn = document.getElementById('btn-copy-genre');
  btn.textContent = 'Copied!';
  setTimeout(() => { btn.textContent = 'Copy markdown'; }, 1500);
});

document.getElementById('btn-clear-genre-log').addEventListener('click', () => {
  document.getElementById('genre-log-output').textContent = '';
});

listen('genre:log', (event) => { appendGenreLog(event.payload); });

// ── Reports ───────────────────────────────────────────────────────────────────
const REPORT_LABELS = {
  'genre-analysis.md':    { label: 'Genre Analysis',      order: 1 },
  'full-report.md':       { label: 'Full Report',          order: 2 },
  'kdp-keywords.md':      { label: 'KDP Keywords',         order: 3 },
  'competition-report.md':{ label: 'Competition Analysis', order: 4 },
};

async function loadReportsList() {
  const folder = getActiveFolder();
  const note   = document.getElementById('reports-folder-note');
  const list   = document.getElementById('reports-list');
  list.innerHTML = '';
  showReportsList();

  if (!folder) {
    note.textContent = 'Select a story to see its reports.';
    note.style.display = 'block';
    return;
  }

  const story = getActiveStory();
  note.textContent = story ? `Reports for: ${story.name}` : '';
  note.style.display = story ? 'block' : 'none';

  try {
    const entries = await readDir(folder + '/_analysis');
    const mdFiles = entries
      .filter(e => e.name?.endsWith('.md'))
      .map(e => e.name)
      .sort((a, b) => (REPORT_LABELS[a]?.order ?? 99) - (REPORT_LABELS[b]?.order ?? 99));

    if (mdFiles.length === 0) {
      const empty = document.createElement('p');
      empty.className = 'panel-desc';
      empty.textContent = 'No reports yet. Run the Analyzer to generate reports.';
      list.appendChild(empty);
      return;
    }

    mdFiles.forEach(filename => {
      const label = REPORT_LABELS[filename]?.label ??
        filename.replace('.md', '').replace(/-/g, ' ').replace(/\b\w/g, c => c.toUpperCase());
      const item = document.createElement('div');
      item.className = 'report-item';
      item.innerHTML = `
        <div>
          <div class="report-item-name">${label}</div>
          <div class="report-item-meta">${filename}</div>
        </div>
        <span class="report-item-arrow">›</span>
      `;
      item.addEventListener('click', () => openReport(folder + '/_analysis/' + filename, label));
      list.appendChild(item);
    });
  } catch {
    const p = document.createElement('p');
    p.className = 'panel-desc';
    p.textContent = 'No reports folder found. Run the Analyzer first.';
    list.appendChild(p);
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
  } catch (e) { alert('Could not read report: ' + String(e)); }
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

// ── Init ──────────────────────────────────────────────────────────────────────
loadSettings();
loadStoriesFromDisk();
