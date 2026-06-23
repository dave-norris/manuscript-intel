import { CheckRocketStatus, LaunchRocket, RunCategoryAnalyzer, AnalyzeCSV } from '../wailsjs/go/main/App.js';
import { EventsOn } from '../wailsjs/runtime/runtime.js';

// ── State ─────────────────────────────────────────────────────────────────────
let currentPanel = 'category';
let csvContent   = '';
let csvKeyword   = '';
let logLines     = [];
let markdownOut  = '';

// ── DOM refs ──────────────────────────────────────────────────────────────────
const statusDot   = document.getElementById('status-dot');
const statusLabel = document.getElementById('status-label');
const btnLaunch   = document.getElementById('btn-launch');
const logOutput   = document.getElementById('log-output');
const mdOutput    = document.getElementById('markdown-output');
const btnCopy     = document.getElementById('btn-copy');

// ── CDP log events from Go ────────────────────────────────────────────────────
EventsOn('cdp:log', (msg) => {
  logLines.push(msg);
  if (logLines.length > 500) logLines.shift();
  logOutput.textContent = logLines.join('\n');
  logOutput.scrollTop = logOutput.scrollHeight;
});

// ── Rocket status ─────────────────────────────────────────────────────────────
async function refreshStatus() {
  try {
    const s = await CheckRocketStatus();
    if (s.cdpEnabled) {
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
  } catch (e) {
    statusLabel.textContent = 'Status unknown';
  }
}

btnLaunch.addEventListener('click', async () => {
  btnLaunch.disabled = true;
  btnLaunch.textContent = 'Launching…';
  appendLog('Launching Publisher Rocket...');
  const result = await LaunchRocket();
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
    document.querySelectorAll('.panel').forEach(p => p.classList.toggle('visible', p.id === target + '-panel'));
    currentPanel = target;
  });
});

// ── Output tabs ───────────────────────────────────────────────────────────────
document.querySelectorAll('.output-tab').forEach(tab => {
  tab.addEventListener('click', () => {
    const target = tab.dataset.tab;
    document.querySelectorAll('.output-tab').forEach(t => t.classList.toggle('active', t === tab));
    document.getElementById('log-pane').style.display    = target === 'log'      ? 'block' : 'none';
    document.getElementById('markdown-pane').style.display = target === 'markdown' ? 'block' : 'none';
  });
});

// ── Category Analyzer ─────────────────────────────────────────────────────────
document.getElementById('btn-run-category').addEventListener('click', async () => {
  const raw = document.getElementById('category-paths').value.trim();
  if (!raw) return;

  const paths = raw.split('\n').map(l => l.trim()).filter(Boolean);
  appendLog(`Running Category Analyzer for ${paths.length} path(s)...`);
  showTab('log');

  const result = await RunCategoryAnalyzer(paths);
  if (result.success) {
    markdownOut = result.markdown;
    mdOutput.textContent = markdownOut;
    appendLog('✓ Category analysis complete.');
    showTab('markdown');
  } else {
    appendLog('✗ ' + result.error);
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
  if (!file) return;
  loadCSVFile(file);
});
csvDrop.addEventListener('click', () => {
  const input = document.createElement('input');
  input.type = 'file';
  input.accept = '.csv';
  input.onchange = () => { if (input.files[0]) loadCSVFile(input.files[0]); };
  input.click();
});

function loadCSVFile(file) {
  const reader = new FileReader();
  reader.onload = e => {
    csvContent = e.target.result;
    // Extract keyword from filename: "COMPETITION ANALYZER - EBOOK <KEYWORD> YYYY-MM-DD …"
    const m = file.name.match(/^COMPETITION ANALYZER - EBOOK (.+?) \d{4}/i);
    csvKeyword = m ? titleCase(m[1]) : file.name.replace(/\.csv$/i, '');
    csvFilename.textContent = `Loaded: ${file.name}`;
    csvFilename.style.display = 'block';
    csvDrop.textContent = 'Drop a different CSV or click to browse';
  };
  reader.readAsText(file);
}

document.getElementById('btn-run-csv').addEventListener('click', async () => {
  if (!csvContent) { appendLog('No CSV loaded.'); return; }
  appendLog(`Running CSV Analyzer for keyword: ${csvKeyword}...`);
  showTab('log');

  const result = await AnalyzeCSV(csvKeyword, csvContent);
  if (result.success) {
    markdownOut = result.markdown;
    mdOutput.textContent = markdownOut;
    appendLog('✓ CSV analysis complete.');
    showTab('markdown');
  } else {
    appendLog('✗ ' + result.error);
  }
});

// ── Copy markdown ─────────────────────────────────────────────────────────────
btnCopy.addEventListener('click', () => {
  if (!markdownOut) return;
  navigator.clipboard.writeText(markdownOut).then(() => {
    btnCopy.textContent = 'Copied!';
    setTimeout(() => { btnCopy.textContent = 'Copy markdown'; }, 1500);
  });
});

// ── Helpers ───────────────────────────────────────────────────────────────────
function appendLog(msg) {
  logLines.push(msg);
  logOutput.textContent = logLines.join('\n');
  logOutput.scrollTop = logOutput.scrollHeight;
}

function showTab(name) {
  document.querySelectorAll('.output-tab').forEach(t => t.classList.toggle('active', t.dataset.tab === name));
  document.getElementById('log-pane').style.display      = name === 'log'      ? 'block' : 'none';
  document.getElementById('markdown-pane').style.display = name === 'markdown' ? 'block' : 'none';
}

function titleCase(str) {
  return str.toLowerCase().split(' ').map(w => w.charAt(0).toUpperCase() + w.slice(1)).join(' ');
}
