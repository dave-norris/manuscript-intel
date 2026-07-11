import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { readDir, readTextFile } from '@tauri-apps/plugin-fs';

// ── Types ─────────────────────────────────────────────────────────────────────

interface Story {
  id: string;
  name: string;
  folder: string;
  created: string;
}

interface StoriesResult {
  success: boolean;
  stories: Story[];
  error: string;
}

interface GenreResult {
  success: boolean;
  report: string;
  error: string;
}

interface StatusResult {
  running: boolean;
  cdp_enabled: boolean;
  page_id: string;
  error: string;
}

interface LaunchResult {
  success: boolean;
  error: string;
}

interface ModelInfo {
  id: string;
  owned_by: string;
  input_price: number | null;
  output_price: number | null;
}

interface ModelsResult {
  success: boolean;
  models: ModelInfo[];
  error: string;
}

interface AnalysisState {
  has_folder: boolean;
  summary_count: number;
  has_genre_data: boolean;
  has_full_report: boolean;
  has_keywords: boolean;
  has_pr_keywords: boolean;
  has_competition: boolean;
  has_categories: boolean;
}

interface Settings {
  provider: string;
  apiKey: string;
  model: string;
}

declare const marked: { parse(md: string): string } | undefined;

// ── Helpers ───────────────────────────────────────────────────────────────────

function $(id: string): HTMLElement {
  return document.getElementById(id)!;
}

function $input(id: string): HTMLInputElement {
  return document.getElementById(id) as HTMLInputElement;
}

function $select(id: string): HTMLSelectElement {
  return document.getElementById(id) as HTMLSelectElement;
}

function $btn(id: string): HTMLButtonElement {
  return document.getElementById(id) as HTMLButtonElement;
}

// ── Context menu ──────────────────────────────────────────────────────────────

document.addEventListener('contextmenu', (e) => {
  const tag = (e.target as HTMLElement).tagName;
  if (!['INPUT', 'TEXTAREA', 'SELECT'].includes(tag) && !(e.target as HTMLElement).isContentEditable) {
    e.preventDefault();
  }
});

// ── App State ─────────────────────────────────────────────────────────────────

let currentGenreMarkdown = '';
let activeStoryId: string | null = localStorage.getItem('activeStoryId') || null;
let allStories: Story[] = [];

// ── DOM refs ──────────────────────────────────────────────────────────────────

const statusDot = $('status-dot');
const statusLabel = $('status-label');
const btnLaunch = $btn('btn-launch');

// ── Settings ──────────────────────────────────────────────────────────────────

function getSelectedProvider(): string {
  return (document.querySelector('input[name="provider"]:checked') as HTMLInputElement)?.value || 'tokenmix';
}

function loadSettings(): void {
  const provider = localStorage.getItem('provider') || 'tokenmix';
  const radio = document.querySelector(`input[name="provider"][value="${provider}"]`) as HTMLInputElement | null;
  if (radio) radio.checked = true;
  $input('api-key').value = localStorage.getItem('apiKey') || '';
  const savedModel = localStorage.getItem('model') || '';
  const select = $select('model-select');
  if (savedModel && select.querySelector(`option[value="${savedModel}"]`)) {
    select.value = savedModel;
  }
}

$btn('btn-save-settings').addEventListener('click', () => {
  const provider = getSelectedProvider();
  localStorage.setItem('provider', provider);
  localStorage.setItem('apiKey', $input('api-key').value.trim());
  localStorage.setItem('model', $select('model-select').value);
  const saved = $('settings-saved');
  saved.textContent = '\u2713 Saved';
  setTimeout(() => { saved.textContent = ''; }, 1500);
  const prevPanel = localStorage.getItem('prevPanel') || 'analyzer';
  showPanel(prevPanel);
});

$btn('btn-fetch-models').addEventListener('click', async () => {
  const provider = getSelectedProvider();
  const apiKey = $input('api-key').value.trim();
  const status = $('model-fetch-status');
  const select = $select('model-select');

  if (!apiKey) { status.textContent = 'Enter an API key first.'; return; }

  status.textContent = 'Fetching models...';
  select.disabled = true;

  try {
    const result = await invoke<ModelsResult>('list_models', { provider, apiKey });
    if (result.success && result.models.length > 0) {
      select.innerHTML = '';
      result.models.forEach((m) => {
        const opt = document.createElement('option');
        opt.value = m.id;
        let label = m.id;
        if (m.owned_by) label += ` (${m.owned_by})`;
        if (m.input_price != null && m.output_price != null) {
          label += ` — $${m.input_price}/$${m.output_price} per 1M tokens`;
        }
        opt.textContent = label;
        select.appendChild(opt);
      });
      const savedModel = localStorage.getItem('model') || '';
      if (savedModel && select.querySelector(`option[value="${savedModel}"]`)) {
        select.value = savedModel;
      }
      status.textContent = `${result.models.length} models loaded.`;
    } else {
      status.textContent = result.error || 'No models returned.';
    }
  } catch (e) {
    status.textContent = 'Error: ' + String(e);
  } finally {
    select.disabled = false;
  }
});

function getSettings(): Settings {
  return {
    provider: localStorage.getItem('provider') || 'tokenmix',
    apiKey:   localStorage.getItem('apiKey')   || '',
    model:    localStorage.getItem('model')    || '',
  };
}

// ── Active story ──────────────────────────────────────────────────────────────

function getActiveStory(): Story | null {
  return allStories.find(s => s.id === activeStoryId) || null;
}

function getActiveFolder(): string {
  return getActiveStory()?.folder || '';
}

function setActiveStory(id: string | null): void {
  activeStoryId = id;
  localStorage.setItem('activeStoryId', id || '');
  renderStoriesList();
  updateAnalyzerDesc();
  if (id) {
    refreshAnalysisState(getActiveFolder());
  } else {
    disableAllButtons();
  }
  if ($('reports-panel').classList.contains('visible')) loadReportsList();
}

function updateAnalyzerDesc(): void {
  const story = getActiveStory();
  const desc = $('analyzer-desc');
  desc.textContent = story ? `Story: ${story.name}` : 'Select or create a story to begin.';
}

// ── Stories ───────────────────────────────────────────────────────────────────

async function loadStoriesFromDisk(): Promise<void> {
  const result = await invoke<StoriesResult>('list_stories');
  allStories = result.success ? result.stories : [];
  renderStoriesList();
  updateAnalyzerDesc();
  if (activeStoryId && allStories.find(s => s.id === activeStoryId)) {
    refreshAnalysisState(getActiveFolder());
  } else if (activeStoryId) {
    setActiveStory(null);
  } else {
    disableAllButtons();
  }
}

function renderStoriesList(): void {
  const list = $('stories-list');
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
      <button class="story-item-edit" data-id="${story.id}" title="Edit story">\u270E</button>
    `;
    item.addEventListener('click', (e) => {
      if ((e.target as HTMLElement).classList.contains('story-item-edit')) return;
      setActiveStory(story.id);
      showPanel('analyzer');
    });
    item.querySelector('.story-item-edit')!.addEventListener('click', (e) => {
      e.stopPropagation();
      openStoryForm(story);
    });
    list.appendChild(item);
  });
}

// ── Story form ────────────────────────────────────────────────────────────────

function openStoryForm(story: Story | null = null): void {
  $input('story-form-id').value = story?.id || '';
  $input('story-name').value = story?.name || '';
  $input('story-folder').value = story?.folder || '';
  $('story-form-title').textContent = story ? 'Edit Story' : 'New Story';
  ($('btn-story-delete') as HTMLElement).style.display = story ? 'block' : 'none';
  $('story-form-error').textContent = '';
  showPanel('story-form');
}

$btn('btn-new-story').addEventListener('click', () => openStoryForm());

$btn('btn-story-pick-folder').addEventListener('click', async () => {
  try {
    const path = await invoke<string>('pick_manuscript_folder');
    if (path) $input('story-folder').value = path;
  } catch (e) {
    if (!String(e).includes('No folder')) {
      $('story-form-error').textContent = String(e);
    }
  }
});

$btn('btn-story-save').addEventListener('click', async () => {
  const id = $input('story-form-id').value;
  const name = $input('story-name').value.trim();
  const folder = $input('story-folder').value.trim();
  const errEl = $('story-form-error');

  if (!name) { errEl.textContent = 'Please enter a story name.'; return; }
  if (!folder) { errEl.textContent = 'Please select a folder.'; return; }
  errEl.textContent = '';

  let result: StoriesResult;
  if (id) {
    result = await invoke<StoriesResult>('update_story', { request: { id, name, folder } });
  } else {
    result = await invoke<StoriesResult>('add_story', { request: { name, folder } });
  }

  if (!result.success) { errEl.textContent = result.error; return; }

  allStories = result.stories;
  const saved = result.stories.find(s => s.name === name && s.folder === folder);
  if (saved) setActiveStory(saved.id);
  renderStoriesList();
  showPanel('analyzer');
});

$btn('btn-story-cancel').addEventListener('click', () => showPanel('analyzer'));

$btn('btn-story-delete').addEventListener('click', async () => {
  const id = $input('story-form-id').value;
  if (!id) return;
  if (!confirm('Remove this story from the list? (The folder and files will not be deleted.)')) return;

  const result = await invoke<StoriesResult>('delete_story', { id });
  if (result.success) {
    allStories = result.stories;
    if (activeStoryId === id) setActiveStory(null);
    renderStoriesList();
    showPanel('analyzer');
  } else {
    $('story-form-error').textContent = result.error;
  }
});

// ── Panel navigation ──────────────────────────────────────────────────────────

function showPanel(name: string): void {
  const currentlyVisible = document.querySelector('.panel.visible')?.id?.replace('-panel', '');
  if ((name === 'settings' || name === 'reports') && currentlyVisible === name) {
    const prev = localStorage.getItem('prevPanel') || 'analyzer';
    showPanel(prev);
    return;
  }
  if ((name === 'settings' || name === 'reports') && currentlyVisible && currentlyVisible !== name) {
    localStorage.setItem('prevPanel', currentlyVisible);
  }
  document.querySelectorAll('.panel').forEach(p => {
    p.classList.toggle('visible', p.id === name + '-panel');
  });
  document.querySelectorAll<HTMLButtonElement>('.nav-item').forEach(b => {
    b.classList.toggle('active', b.dataset.panel === name);
  });
  if (name === 'reports') loadReportsList();
}

document.querySelectorAll<HTMLButtonElement>('.nav-item').forEach(btn => {
  btn.addEventListener('click', () => showPanel(btn.dataset.panel!));
});

// ── Rocket status ─────────────────────────────────────────────────────────────

async function refreshStatus(): Promise<void> {
  try {
    const s = await invoke<StatusResult>('check_rocket_status');
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
  btnLaunch.disabled = true; btnLaunch.textContent = 'Launching\u2026';
  const result = await invoke<LaunchResult>('launch_rocket');
  if (result.success) { refreshStatus(); }
  else { btnLaunch.disabled = false; btnLaunch.textContent = 'Retry'; }
});

setInterval(refreshStatus, 5000);
refreshStatus();

// ── Tab switching ─────────────────────────────────────────────────────────────

document.querySelectorAll<HTMLButtonElement>('[data-tab="genre-log"],[data-tab="genre-preview"],[data-tab="genre-raw"]')
  .forEach(tab => {
    tab.addEventListener('click', () => {
      document.querySelectorAll<HTMLButtonElement>('[data-tab="genre-log"],[data-tab="genre-preview"],[data-tab="genre-raw"]')
        .forEach(t => t.classList.toggle('active', t === tab));
      const name = tab.dataset.tab!;
      $('genre-log-pane').classList.toggle('hidden', name !== 'genre-log');
      $('genre-preview-pane').classList.toggle('hidden', name !== 'genre-preview');
      $('genre-raw-pane').classList.toggle('hidden', name !== 'genre-raw');
    });
  });

// ── Button state ──────────────────────────────────────────────────────────────

function disableAllButtons(): void {
  ['btn-run-everything','btn-analyze-competition','btn-gen-summaries',
   'btn-run-genre','btn-full-analysis','btn-optimize-keywords',
   'btn-gen-pr-keywords','btn-find-categories'].forEach(id => {
    const el = document.getElementById(id) as HTMLButtonElement | null;
    if (el) el.disabled = true;
  });
  updateStepLabels(null);
}

function disableGenreButtons(disabled: boolean): void {
  ['btn-run-everything','btn-analyze-competition','btn-gen-summaries',
   'btn-run-genre','btn-full-analysis','btn-optimize-keywords',
   'btn-gen-pr-keywords','btn-find-categories'].forEach(id => {
    const el = document.getElementById(id) as HTMLButtonElement | null;
    if (el) el.disabled = disabled;
  });
  ($('btn-stop') as HTMLElement).style.display = disabled ? 'flex' : 'none';
  if (!disabled) {
    const f = getActiveFolder();
    if (f) refreshAnalysisState(f);
    if ($('reports-panel').classList.contains('visible')) loadReportsList();
  }
}

async function refreshAnalysisState(folder: string): Promise<void> {
  if (!folder) { disableAllButtons(); return; }
  try {
    const s = await invoke<AnalysisState>('check_analysis_state', { folder });
    $btn('btn-run-everything').disabled = s.summary_count === 0;
    $btn('btn-analyze-competition').disabled = !s.has_pr_keywords;
    $btn('btn-find-categories').disabled = !s.has_genre_data;
    $btn('btn-gen-summaries').disabled = false;
    $btn('btn-run-genre').disabled = s.summary_count === 0;
    $btn('btn-full-analysis').disabled = !s.has_genre_data;
    $btn('btn-optimize-keywords').disabled = !s.has_full_report;
    $btn('btn-gen-pr-keywords').disabled = !s.has_full_report;
    updateStepLabels(s);
  } catch (e) { console.error('check_analysis_state:', e); }
}

function updateStepLabels(state: AnalysisState | null): void {
  const steps: [string, string, string][] = [
    ['btn-gen-summaries', 'Summaries', state && state.summary_count > 0 ? ` (${state.summary_count})\u2713` : ''],
    ['btn-run-genre', 'Analyze', state?.has_genre_data ? ' \u2713' : ''],
    ['btn-full-analysis', 'Full Analysis', state?.has_full_report ? ' \u2713' : ''],
    ['btn-optimize-keywords', 'KDP Keywords', state?.has_keywords ? ' \u2713' : ''],
    ['btn-gen-pr-keywords', 'PR Keywords', state?.has_pr_keywords ? ' \u2713' : ''],
  ];
  steps.forEach(([id, base, suffix]) => {
    const btn = document.getElementById(id);
    if (btn) btn.textContent = base + suffix;
  });
  const rAll = document.getElementById('btn-run-everything');
  const rComp = document.getElementById('btn-analyze-competition');
  const rCat = document.getElementById('btn-find-categories');
  if (rAll) rAll.textContent = '\u25b6 Run Analysis' + (state?.has_pr_keywords ? ' \u2713' : '');
  if (rCat) rCat.textContent = '\u25b6 Find Categories' + (state?.has_categories ? ' \u2713' : '');
  if (rComp) rComp.textContent = '\u25b6 Analyze Competition' + (state?.has_competition ? ' \u2713' : '');
}

// ── Analyzer buttons ──────────────────────────────────────────────────────────

function setGenreReport(markdown: string): void {
  currentGenreMarkdown = markdown;
  $('genre-raw-output').textContent = markdown;
  $('genre-preview-content').innerHTML =
    typeof marked !== 'undefined' ? marked.parse(markdown) : '<pre>' + markdown + '</pre>';
  document.querySelectorAll<HTMLButtonElement>('[data-tab="genre-log"],[data-tab="genre-preview"],[data-tab="genre-raw"]')
    .forEach(t => t.classList.toggle('active', t.dataset.tab === 'genre-preview'));
  $('genre-log-pane').classList.add('hidden');
  $('genre-preview-pane').classList.remove('hidden');
  $('genre-raw-pane').classList.add('hidden');
}

function appendGenreLog(msg: string): void {
  const el = $('genre-log-output');
  el.textContent += (el.textContent ? '\n' : '') + msg;
  el.scrollTop = el.scrollHeight;
  document.querySelectorAll<HTMLButtonElement>('[data-tab="genre-log"],[data-tab="genre-preview"],[data-tab="genre-raw"]')
    .forEach(t => t.classList.toggle('active', t.dataset.tab === 'genre-log'));
  $('genre-log-pane').classList.remove('hidden');
  $('genre-preview-pane').classList.add('hidden');
  $('genre-raw-pane').classList.add('hidden');
}

async function runGenreCommand(cmdName: string, logMsg: string, successMsg: string): Promise<void> {
  const folder = getActiveFolder();
  if (!folder) { appendGenreLog('\u2717 No story selected.'); return; }
  const { provider, apiKey, model } = getSettings();
  if (!apiKey) { appendGenreLog('\u2717 No API key set. Go to Settings.'); return; }
  if (!model) { appendGenreLog('\u2717 No model selected. Go to Settings and fetch models.'); return; }
  appendGenreLog(`${logMsg} [${provider}: ${model}]`);
  disableGenreButtons(true);
  try {
    const result = await invoke<GenreResult>(cmdName, { request: { folder, api_key: apiKey, model, provider } });
    if (result.success) { setGenreReport(result.report); appendGenreLog(successMsg); }
    else { appendGenreLog('\u2717 ' + result.error); }
  } catch (e) { appendGenreLog('\u2717 ' + String(e)); }
  finally { disableGenreButtons(false); }
}

$btn('btn-run-everything').addEventListener('click', () =>
  runGenreCommand('run_everything', 'Running full analysis...', '\u2713 Analysis complete. Run \u25b6 Analyze Competition next.'));

$btn('btn-gen-summaries').addEventListener('click', () =>
  runGenreCommand('generate_summaries', 'Generating chapter summaries...', '\u2713 Summaries complete.'));

$btn('btn-run-genre').addEventListener('click', () =>
  runGenreCommand('analyze_genre', 'Running genre analysis...', '\u2713 Genre analysis complete.'));

$btn('btn-full-analysis').addEventListener('click', () =>
  runGenreCommand('run_full_analysis', 'Running full analysis...', '\u2713 Full analysis complete.'));

$btn('btn-optimize-keywords').addEventListener('click', () =>
  runGenreCommand('optimize_keywords', 'Optimizing KDP keywords...', '\u2713 KDP keywords complete.'));

$btn('btn-gen-pr-keywords').addEventListener('click', () =>
  runGenreCommand('generate_pr_keywords', 'Generating PR search terms...', '\u2713 PR keywords generated.'));

$btn('btn-stop').addEventListener('click', async () => {
  appendGenreLog('Stopping after current step...');
  await invoke('cancel_operation');
});

// ── Analyze Competition ───────────────────────────────────────────────────────

$btn('btn-analyze-competition').addEventListener('mouseenter', () => {
  ($('competition-store-row') as HTMLElement).style.display = 'flex';
});

$btn('btn-analyze-competition').addEventListener('click', async () => {
  const folder = getActiveFolder();
  if (!folder) { appendGenreLog('\u2717 No story selected.'); return; }
  const { provider, apiKey, model } = getSettings();
  if (!apiKey) { appendGenreLog('\u2717 No API key set. Go to Settings.'); return; }
  if (!model) { appendGenreLog('\u2717 No model selected. Go to Settings.'); return; }
  const store = (document.querySelector('input[name="comp-store"]:checked') as HTMLInputElement)?.value || 'Kindle';
  appendGenreLog(`Analyzing competition [${store}] via Publisher Rocket... [${provider}: ${model}]`);
  appendGenreLog('This may take several minutes.');
  disableGenreButtons(true);
  try {
    const result = await invoke<GenreResult>('analyze_competition', { request: { folder, api_key: apiKey, model, store, provider } });
    if (result.success) { setGenreReport(result.report); appendGenreLog('\u2713 Competition analysis complete.'); }
    else { appendGenreLog('\u2717 ' + result.error); }
  } catch (e) { appendGenreLog('\u2717 ' + String(e)); }
  finally { disableGenreButtons(false); }
});

$btn('btn-find-categories').addEventListener('click', async () => {
  const folder = getActiveFolder();
  if (!folder) { appendGenreLog('No story selected.'); return; }
  const { provider, apiKey, model } = getSettings();
  if (!apiKey) { appendGenreLog('No API key set. Go to Settings.'); return; }
  if (!model) { appendGenreLog('No model selected. Go to Settings.'); return; }
  const store = (document.querySelector('input[name="cat-store"]:checked') as HTMLInputElement)?.value || 'Kindle';
  appendGenreLog(`Finding categories [${store}, Selectable Excluding Ghosts] [${provider}: ${model}]`);
  appendGenreLog('This may take several minutes.');
  disableGenreButtons(true);
  try {
    const result = await invoke<GenreResult>('find_categories_for_story', { request: { folder, api_key: apiKey, model, store, provider } });
    if (result.success) { setGenreReport(result.report); appendGenreLog('Category Finder complete.'); }
    else { appendGenreLog('Error: ' + result.error); }
  } catch (e) { appendGenreLog('Error: ' + String(e)); }
  finally { disableGenreButtons(false); }
});

$btn('btn-copy-genre').addEventListener('click', async () => {
  if (!currentGenreMarkdown) return;
  await writeText(currentGenreMarkdown);
  const btn = $btn('btn-copy-genre');
  btn.textContent = 'Copied!';
  setTimeout(() => { btn.textContent = 'Copy markdown'; }, 1500);
});

$btn('btn-clear-genre-log').addEventListener('click', () => {
  $('genre-log-output').textContent = '';
});

listen<string>('genre:log', (event) => { appendGenreLog(event.payload); });

// ── Reports ───────────────────────────────────────────────────────────────────

const REPORT_LABELS: Record<string, { label: string; order: number }> = {
  'genre-analysis.md':     { label: 'Genre Analysis', order: 1 },
  'full-report.md':        { label: 'Full Report', order: 2 },
  'kdp-keywords.md':       { label: 'KDP Keywords', order: 3 },
  'competition-report.md': { label: 'Competition Analysis', order: 4 },
  'category-finder.md':    { label: 'Category Finder', order: 5 },
};

async function loadReportsList(): Promise<void> {
  const folder = getActiveFolder();
  const note = $('reports-folder-note');
  const list = $('reports-list');
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
      .map(e => e.name!)
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
        <span class="report-item-arrow">\u203A</span>
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

async function openReport(path: string, label: string): Promise<void> {
  try {
    const content = await readTextFile(path);
    $('reports-viewer-title').textContent = label;
    $('reports-viewer-content').innerHTML =
      typeof marked !== 'undefined' ? marked.parse(content) : '<pre>' + content + '</pre>';
    ($('reports-viewer') as HTMLElement).dataset.content = content;
    showReportsViewer();
  } catch (e) { alert('Could not read report: ' + String(e)); }
}

function showReportsList(): void {
  $('reports-list').classList.remove('hidden');
  $('reports-folder-note').classList.remove('hidden');
  $('reports-viewer').classList.add('hidden');
}

function showReportsViewer(): void {
  $('reports-list').classList.add('hidden');
  $('reports-folder-note').classList.add('hidden');
  $('reports-viewer').classList.remove('hidden');
}

$btn('btn-reports-back').addEventListener('click', showReportsList);

$btn('btn-reports-copy').addEventListener('click', async () => {
  const content = ($('reports-viewer') as HTMLElement).dataset.content || '';
  if (!content) return;
  await writeText(content);
  const btn = $btn('btn-reports-copy');
  btn.textContent = 'Copied!';
  setTimeout(() => { btn.textContent = 'Copy'; }, 1500);
});

// ── Init ──────────────────────────────────────────────────────────────────────

loadSettings();
loadStoriesFromDisk();
