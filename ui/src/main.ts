import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { writeText } from '@tauri-apps/plugin-clipboard-manager';
import { renderReport } from './reportRenderer';

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
  has_genre_ranking: boolean;
  has_mapped_verified: boolean;
  has_bisac: boolean;
  has_discovery_keywords: boolean;
  has_keyword_search_results: boolean;
}

interface Settings {
  provider: string;
  apiKey: string;
  model: string;
}

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

let activeStoryId: string | null = localStorage.getItem('activeStoryId') || null;
let allStories: Story[] = [];

// ── DOM refs ──────────────────────────────────────────────────────────────────

// ── Settings ──────────────────────────────────────────────────────────────────

function getSelectedProvider(): string {
  return (document.querySelector('input[name="provider"]:checked') as HTMLInputElement)?.value || 'tokenmix';
}

function loadSettings(): void {
  const provider = localStorage.getItem('provider') || 'tokenmix';
  const radio = document.querySelector(`input[name="provider"][value="${provider}"]`) as HTMLInputElement | null;
  if (radio) radio.checked = true;
  $input('api-key').value = localStorage.getItem('apiKey') || '';
  $input('canopy-api-key').value = localStorage.getItem('canopyApiKey') || '';
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
  localStorage.setItem('canopyApiKey', $input('canopy-api-key').value.trim());
  const saved = $('settings-saved');
  saved.textContent = '✓ Saved';
  setTimeout(() => { saved.textContent = ''; }, 1500);
  const prevPanel = localStorage.getItem('prevPanel') || 'analyzer';
  showPanel(prevPanel);
});

$btn('btn-test-canopy').addEventListener('click', async () => {
  const key = $input('canopy-api-key').value.trim();
  const status = $('canopy-test-status');
  if (!key) { status.textContent = 'Enter a key first.'; return; }
  status.textContent = 'Testing...';
  try {
    const result = await invoke<{ success: boolean; error: string }>('test_canopy_connection', { apiKey: key });
    status.textContent = result.success ? '✓ Connected' : '✗ ' + result.error;
  } catch (e) { status.textContent = '✗ ' + String(e); }
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

// ── WinningCat import ──────────────────────────────────────────────────

interface WinningCatImportResult {
  success: boolean;
  imported: number;
  skipped_other_department: number;
  skipped_unparseable: number;
  stale_count: number;
  imported_at: string;
  error: string;
}

interface StaleCleanupResult {
  success: boolean;
  removed: number;
  error: string;
}

let lastImportedAt = '';

$btn('btn-import-winningcat').addEventListener('click', async () => {
  const status = $('winningcat-import-status');
  const btn = $btn('btn-import-winningcat');
  const staleRow = $('winningcat-stale-row');
  status.textContent = 'Select the CSV file…';
  btn.disabled = true;
  staleRow.style.display = 'none';
  try {
    const result = await invoke<WinningCatImportResult>('import_winningcat_csv');
    if (result.success) {
      status.textContent = `✓ Imported ${result.imported} categories. Skipped ${result.skipped_other_department} (other department), ${result.skipped_unparseable} (unparseable).`;
      lastImportedAt = result.imported_at;
      if (result.stale_count > 0) {
        staleRow.style.display = 'block';
        $('winningcat-stale-status').textContent = `${result.stale_count} categor${result.stale_count === 1 ? 'y was' : 'ies were'} in the catalog from a previous import but missing from this one — possibly retired or renamed by Amazon.`;
      }
    } else {
      status.textContent = result.error || 'Import failed.';
    }
  } catch (e) {
    status.textContent = 'Error: ' + String(e);
  } finally {
    btn.disabled = false;
  }
});

$btn('btn-remove-stale-winningcat').addEventListener('click', async () => {
  if (!lastImportedAt) return;
  if (!confirm('Remove these stale categories from the catalog? This only affects reference data — no story data is touched.')) return;
  const status = $('winningcat-stale-status');
  const btn = $btn('btn-remove-stale-winningcat');
  btn.disabled = true;
  try {
    const result = await invoke<StaleCleanupResult>('remove_stale_kdp_categories', { since: lastImportedAt });
    if (result.success) {
      status.textContent = `✓ Removed ${result.removed} stale categor${result.removed === 1 ? 'y' : 'ies'}.`;
      ($('winningcat-stale-row') as HTMLElement).style.display = 'none';
    } else {
      status.textContent = result.error || 'Cleanup failed.';
    }
  } catch (e) {
    status.textContent = 'Error: ' + String(e);
  } finally {
    btn.disabled = false;
  }
});

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
  loadReportsList();
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
    loadReportsList();
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
      <button class="story-item-edit" data-id="${story.id}" title="Edit story">✎</button>
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
  if (name === 'settings' && currentlyVisible === name) {
    const prev = localStorage.getItem('prevPanel') || 'analyzer';
    showPanel(prev);
    return;
  }
  if (name === 'settings' && currentlyVisible && currentlyVisible !== name) {
    localStorage.setItem('prevPanel', currentlyVisible);
  }
  document.querySelectorAll('.panel').forEach(p => {
    p.classList.toggle('visible', p.id === name + '-panel');
  });
  document.querySelectorAll<HTMLButtonElement>('.nav-item').forEach(b => {
    b.classList.toggle('active', b.dataset.panel === name);
  });
}

document.querySelectorAll<HTMLButtonElement>('.nav-item').forEach(btn => {
  btn.addEventListener('click', () => showPanel(btn.dataset.panel!));
});

// ── Button state ──────────────────────────────────────────────────────────────

function disableAllButtons(): void {
  ['btn-analyze', 'btn-analyze-competition'].forEach(id => {
    const el = document.getElementById(id) as HTMLButtonElement | null;
    if (el) el.disabled = true;
  });
}

function disableGenreButtons(disabled: boolean): void {
  ['btn-analyze', 'btn-analyze-competition'].forEach(id => {
    const el = document.getElementById(id) as HTMLButtonElement | null;
    if (el) el.disabled = disabled;
  });
  ($('btn-stop') as HTMLElement).style.display = disabled ? 'flex' : 'none';
  if (!disabled) {
    const f = getActiveFolder();
    if (f) refreshAnalysisState(f);
    loadReportsList();
  }
}

async function refreshAnalysisState(folder: string): Promise<void> {
  if (!folder) { disableAllButtons(); return; }
  try {
    const s = await invoke<AnalysisState>('check_analysis_state', { folder });
    $btn('btn-analyze').disabled = false;
    $btn('btn-analyze-competition').disabled = !s.has_pr_keywords;
    $btn('btn-mine-reviews').disabled = !s.has_pr_keywords;
    $btn('btn-author-analysis').disabled = !s.has_pr_keywords;
  } catch (e) { console.error('check_analysis_state:', e); }
}

// ── Analyzer output ───────────────────────────────────────────────────────────

function appendGenreLog(msg: string): void {
  const el = $('genre-log-output');
  el.textContent += (el.textContent ? '\n' : '') + msg;
  el.scrollTop = el.scrollHeight;
}

// ── Analyze handler ───────────────────────────────────────────────────────────

$btn('btn-analyze').addEventListener('click', async () => {
  const folder = getActiveFolder();
  if (!folder) { appendGenreLog('✗ No story selected.'); return; }
  const { provider, apiKey, model } = getSettings();
  if (!apiKey) { appendGenreLog('✗ No API key set. Go to Settings.'); return; }
  if (!model) { appendGenreLog('✗ No model selected. Go to Settings and fetch models.'); return; }
  const forceResummarize = $input('force-resummarize').checked;
  appendGenreLog(`Running full analysis pipeline... [${provider}: ${model}]${forceResummarize ? ' (force re-summarize)' : ''}`);
  disableGenreButtons(true);
  try {
    const result = await invoke<GenreResult>('analyze_story', { request: { folder, api_key: apiKey, model, provider, force_resummarize: forceResummarize, canopy_api_key: localStorage.getItem('canopyApiKey') || '' } });
    if (result.success) { appendGenreLog('✓ Analysis complete. View reports in the sidebar.'); }
    else { appendGenreLog('✗ ' + result.error); }
  } catch (e) { appendGenreLog('✗ ' + String(e)); }
  finally { disableGenreButtons(false); }
});

// ── Stop handler ──────────────────────────────────────────────────────────────

$btn('btn-stop').addEventListener('click', async () => {
  appendGenreLog('Stopping after current step...');
  await invoke('cancel_operation');
});

// ── Analyze Competition ───────────────────────────────────────────────────────

$btn('btn-analyze-competition').addEventListener('click', async () => {
  const folder = getActiveFolder();
  if (!folder) { appendGenreLog('✗ No story selected.'); return; }
  const { provider, apiKey, model } = getSettings();
  if (!apiKey) { appendGenreLog('✗ No API key set. Go to Settings.'); return; }
  if (!model) { appendGenreLog('✗ No model selected. Go to Settings.'); return; }
  const store = (document.querySelector('input[name="comp-store"]:checked') as HTMLInputElement)?.value || 'Kindle';
  const canopyKey = localStorage.getItem('canopyApiKey') || '';
  if (!canopyKey) { appendGenreLog('✗ No Canopy API key set. Go to Settings.'); return; }
  appendGenreLog(`Analyzing competition [${store}] via Canopy API... [${provider}: ${model}]`);
  disableGenreButtons(true);
  try {
    const result = await invoke<GenreResult>('analyze_competition_canopy', { request: { folder, api_key: apiKey, model, store, provider, canopy_api_key: canopyKey } });
    if (result.success) { appendGenreLog('✓ Competition analysis complete. View reports in the sidebar.'); }
    else { appendGenreLog('✗ ' + result.error); }
  } catch (e) { appendGenreLog('✗ ' + String(e)); }
  finally { disableGenreButtons(false); }
});

// ── Output actions ────────────────────────────────────────────────────────────

$btn('btn-clear-genre-log').addEventListener('click', () => {
  $('genre-log-output').textContent = '';
});

// ── New feature buttons ───────────────────────────────────────────────────────

$btn('btn-mine-reviews').addEventListener('click', async () => {
  const folder = getActiveFolder();
  if (!folder) { appendGenreLog('✗ No story selected.'); return; }
  const { provider, apiKey, model } = getSettings();
  const canopyKey = localStorage.getItem('canopyApiKey') || '';
  if (!canopyKey) { appendGenreLog('✗ No Canopy API key. Go to Settings.'); return; }
  if (!apiKey) { appendGenreLog('✗ No AI API key. Go to Settings.'); return; }
  appendGenreLog('Mining competitor reviews...');
  disableGenreButtons(true);
  try {
    const result = await invoke<{ success: boolean; error: string }>('mine_competitor_reviews', { request: { folder, canopy_api_key: canopyKey, api_key: apiKey, model, provider } });
    if (result.success) { appendGenreLog('✓ Review mining complete. View in sidebar.'); loadReportsList(); }
    else { appendGenreLog('✗ ' + result.error); }
  } catch (e) { appendGenreLog('✗ ' + String(e)); }
  finally { disableGenreButtons(false); }
});

$btn('btn-author-analysis').addEventListener('click', async () => {
  const folder = getActiveFolder();
  if (!folder) { appendGenreLog('✗ No story selected.'); return; }
  const { provider, apiKey, model } = getSettings();
  const canopyKey = localStorage.getItem('canopyApiKey') || '';
  if (!canopyKey) { appendGenreLog('✗ No Canopy API key. Go to Settings.'); return; }
  if (!apiKey) { appendGenreLog('✗ No AI API key. Go to Settings.'); return; }
  appendGenreLog('Analyzing competitor authors...');
  disableGenreButtons(true);
  try {
    const result = await invoke<{ success: boolean; error: string }>('analyze_comp_authors', { request: { folder, canopy_api_key: canopyKey, api_key: apiKey, model, provider } });
    if (result.success) { appendGenreLog('✓ Author analysis complete. View in sidebar.'); loadReportsList(); }
    else { appendGenreLog('✗ ' + result.error); }
  } catch (e) { appendGenreLog('✗ ' + String(e)); }
  finally { disableGenreButtons(false); }
});

$btn('btn-sync-categories').addEventListener('click', async () => {
  const canopyKey = localStorage.getItem('canopyApiKey') || '';
  if (!canopyKey) { appendGenreLog('✗ No Canopy API key. Go to Settings.'); return; }
  appendGenreLog('Syncing categories from Amazon...');
  try {
    const kindle = await invoke<{ success: boolean; imported: number; error: string }>('sync_categories_canopy', { canopyApiKey: canopyKey, store: 'Kindle' });
    if (kindle.success) { appendGenreLog(`  ✓ Kindle: ${kindle.imported} categories synced.`); }
    else { appendGenreLog('  ✗ Kindle: ' + kindle.error); }
    const books = await invoke<{ success: boolean; imported: number; error: string }>('sync_categories_canopy', { canopyApiKey: canopyKey, store: 'Books' });
    if (books.success) { appendGenreLog(`  ✓ Books: ${books.imported} categories synced.`); }
    else { appendGenreLog('  ✗ Books: ' + books.error); }
    appendGenreLog('✓ Category sync complete.');
  } catch (e) { appendGenreLog('✗ ' + String(e)); }
});

// ── Log listeners ─────────────────────────────────────────────────────────────

listen<string>('genre:log', (event) => { appendGenreLog(event.payload); });
listen<string>('cdp:log', (event) => { appendGenreLog(event.payload); });

// ── Reports ───────────────────────────────────────────────────────────────────

interface DocMeta {
  doc_type: string;
  label: string;
  generated_at: string;
}

interface SavedReportMeta {
  id: number;
  doc_type: string;
  version: number;
  label: string;
  saved_at: string;
}

// Track what's currently being viewed so Save/Delete know the context
let viewingDocType = '';
let viewingSavedId: number | null = null;

async function loadReportsList(): Promise<void> {
  const folder = getActiveFolder();
  const list = $('reports-list');
  list.innerHTML = '';

  if (!folder) {
    list.innerHTML = '<div class="sidebar-hint">Select a story to see reports.</div>';
    return;
  }

  try {
    const docs = await invoke<DocMeta[]>('list_reports_cmd', { folder });
    const saved = await invoke<SavedReportMeta[]>('list_saved_reports_cmd', { folder });

    if (docs.length === 0 && saved.length === 0) {
      list.innerHTML = '<div class="sidebar-hint">No reports yet.</div>';
      return;
    }

    // Current reports
    if (docs.length > 0) {
      const header = document.createElement('div');
      header.className = 'sidebar-section-header';
      header.textContent = 'Current';
      list.appendChild(header);

      docs.forEach(doc => {
        const item = document.createElement('div');
        item.className = 'sidebar-report-item';
        item.textContent = doc.label;
        item.title = new Date(doc.generated_at).toLocaleString();
        item.addEventListener('click', () => openReport(folder, doc.doc_type, doc.label));
        list.appendChild(item);
      });
    }

    // Saved versions
    if (saved.length > 0) {
      const header = document.createElement('div');
      header.className = 'sidebar-section-header';
      header.textContent = 'Saved';
      list.appendChild(header);

      saved.forEach(s => {
        const item = document.createElement('div');
        item.className = 'sidebar-report-item';
        item.textContent = s.label;
        item.title = `Saved ${new Date(s.saved_at).toLocaleString()}`;
        item.addEventListener('click', () => openSavedReport(s.id, s.label));
        list.appendChild(item);
      });
    }
  } catch (e) {
    list.innerHTML = `<div class="sidebar-hint">Error: ${String(e)}</div>`;
  }
}

interface ReportEnvelope {
  doc_type: string;
  label: string;
  format: string;
  content: string;
  generated_at: string;
}

async function openReport(folder: string, docType: string, label: string): Promise<void> {
  try {
    const envelope = await invoke<ReportEnvelope>('get_report_cmd', { folder, docType });
    const storyName = getActiveStory()?.name || '';
    viewingDocType = docType;
    viewingSavedId = null;
    $('reports-viewer-title').textContent = label;
    $('reports-viewer-content').innerHTML = renderReport(envelope, storyName);
    ($('reports-viewer') as HTMLElement).dataset.content = envelope.content;
    $btn('btn-reports-save').classList.remove('hidden');
    $btn('btn-reports-delete').classList.add('hidden');
    showPanel('reports');
  } catch (e) { alert('Could not read report: ' + String(e)); }
}

async function openSavedReport(id: number, label: string): Promise<void> {
  try {
    const envelope = await invoke<ReportEnvelope>('get_saved_report_cmd', { id });
    const storyName = getActiveStory()?.name || '';
    viewingDocType = '';
    viewingSavedId = id;
    $('reports-viewer-title').textContent = label;
    $('reports-viewer-content').innerHTML = renderReport(envelope, storyName);
    ($('reports-viewer') as HTMLElement).dataset.content = envelope.content;
    $btn('btn-reports-save').classList.add('hidden');
    $btn('btn-reports-delete').classList.remove('hidden');
    showPanel('reports');
  } catch (e) { alert('Could not read saved report: ' + String(e)); }
}

$btn('btn-reports-copy').addEventListener('click', async () => {
  const content = ($('reports-viewer') as HTMLElement).dataset.content || '';
  if (!content) return;
  await writeText(content);
  const btn = $btn('btn-reports-copy');
  btn.textContent = 'Copied!';
  setTimeout(() => { btn.textContent = 'Copy'; }, 1500);
});

$btn('btn-reports-save').addEventListener('click', async () => {
  const folder = getActiveFolder();
  if (!folder || !viewingDocType) return;
  const btn = $btn('btn-reports-save');
  btn.disabled = true;
  btn.textContent = 'Saving…';
  try {
    const meta = await invoke<SavedReportMeta>('save_report_version_cmd', { folder, docType: viewingDocType });
    btn.textContent = `Saved as ${meta.label}`;
    loadReportsList(); // Refresh sidebar list
    setTimeout(() => { btn.textContent = 'Save'; btn.disabled = false; }, 2000);
  } catch (e) {
    btn.textContent = 'Error';
    setTimeout(() => { btn.textContent = 'Save'; btn.disabled = false; }, 2000);
    alert('Could not save report: ' + String(e));
  }
});

$btn('btn-reports-delete').addEventListener('click', async () => {
  if (viewingSavedId === null) return;
  if (!confirm('Delete this saved report version? This cannot be undone.')) return;
  const btn = $btn('btn-reports-delete');
  btn.disabled = true;
  try {
    await invoke<void>('delete_saved_report_cmd', { id: viewingSavedId });
    loadReportsList();
    showPanel('analyzer');
  } catch (e) {
    alert('Could not delete: ' + String(e));
  } finally {
    btn.disabled = false;
  }
});

// ── Init ──────────────────────────────────────────────────────────────────────

loadSettings();
loadStoriesFromDisk();
