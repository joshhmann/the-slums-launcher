import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';

const $ = (s) => document.querySelector(s);
const $$ = (s) => document.querySelectorAll(s);

let config = {};
let clientStatus = null;
let addonList = [];
let installedAddons = [];
let addonPage = 1;
const PAGE_SIZE = 10;
let selectedAddon = null;

// ── DOM refs ────────────────────────────────────

const el = {
  appName: $('#app-name'),
  tagline: $('#tagline'),
  stateNotInstalled: $('#state-not-installed'),
  stateDownloading: $('#state-downloading'),
  stateReady: $('#state-ready'),
  progressPhase: $('#dl-phase'),
  progressFill: $('#dl-fill'),
  progressStatus: $('#dl-status'),
  progressDetail: $('#dl-detail'),
  statSize: $('#stat-size'),
  statServer: $('#stat-server'),
  statAddons: $('#stat-addons'),
  playServer: $('#play-server'),
  addonGrid: $('#addon-grid'),
  addonPager: $('#addon-pager'),
  pageInfo: $('#page-info'),
  addonSearch: $('#addon-search'),
  addonSort: $('#addon-sort'),
  addonExpansion: $('#addon-expansion'),
  installedCount: $('#installed-count'),
  modal: $('#modal'),
  modalTitle: $('#modal-title'),
  modalAuthor: $('#modal-author'),
  modalDesc: $('#modal-desc'),
  modalImage: $('#modal-image'),
  modalAction: $('#modal-action'),
  toast: $('#toast'),
};

// ── Toast ───────────────────────────────────────

let toastTimer;
function showToast(msg, type = 'info') {
  clearTimeout(toastTimer);
  el.toast.textContent = msg;
  el.toast.className = 'toast ' + type;
  el.toast.classList.remove('hidden');
  toastTimer = setTimeout(() => el.toast.classList.add('hidden'), 4000);
}

// ── Init ────────────────────────────────────────

async function init() {
  try {
    config = await invoke('get_config');
    el.appName.textContent = config.app_name || 'The Slums';
    el.tagline.textContent = config.app_tagline || 'AzerothCore WotLK 3.3.5a';
    el.playServer.textContent = config.server_address || '';
    el.statServer.textContent = config.server_address || '';
    updateClientState();
  } catch (e) { console.error('Init failed:', e); }
}

// ── Client State Machine ────────────────────────

async function updateClientState() {
  try {
    clientStatus = await invoke('get_client_status');
  } catch (e) {
    clientStatus = null;
  }

  el.stateNotInstalled.classList.add('hidden');
  el.stateDownloading.classList.add('hidden');
  el.stateReady.classList.add('hidden');

  const s = clientStatus;
  if (!s || s.phase === 'not_installed') {
    el.stateNotInstalled.classList.remove('hidden');
  } else if (s.phase === 'needs_update') {
    el.stateReady.classList.remove('hidden');
    showReadyState();
  } else {
    el.stateReady.classList.remove('hidden');
    showReadyState();
  }
}

function showReadyState() {
  const size = clientStatus?.installed_size || 0;
  el.statSize.textContent = size > 0 ? formatSize(size) : '—';
  el.statAddons.textContent = installedAddons.length > 0 ? `${installedAddons.length} installed` : '—';
  $('#btn-check-update').classList.remove('hidden');
}

function formatSize(bytes) {
  if (bytes > 1_073_741_824) return (bytes / 1_073_741_824).toFixed(1) + ' GB';
  if (bytes > 1_048_576) return (bytes / 1_048_576).toFixed(1) + ' MB';
  return (bytes / 1024).toFixed(1) + ' KB';
}

// ── Install Button ──────────────────────────────

$('#btn-install')?.addEventListener('click', async () => {
  el.stateNotInstalled.classList.add('hidden');
  el.stateDownloading.classList.remove('hidden');
  el.progressPhase.textContent = 'Preparing...';
  el.progressFill.style.width = '0%';
  el.progressStatus.textContent = '';
  el.progressDetail.textContent = '';

  try {
    const path = await invoke('download_client');
    showToast('Client installed!', 'success');
    updateClientState();
  } catch (e) {
    showToast('Download failed: ' + e, 'error');
    el.stateDownloading.classList.add('hidden');
    el.stateNotInstalled.classList.remove('hidden');
  }
});

// ── Check for Updates ───────────────────────────

$('#btn-check-update')?.addEventListener('click', async () => {
  el.stateReady.classList.add('hidden');
  el.stateDownloading.classList.remove('hidden');
  el.progressPhase.textContent = 'Checking for updates...';
  el.progressFill.style.width = '0%';
  el.progressStatus.textContent = '';

  try {
    const path = await invoke('download_client');
    showToast('Client up to date!', 'success');
    updateClientState();
  } catch (e) {
    showToast('Update failed: ' + e, 'error');
    updateClientState();
  }
});

// ── Play Button ─────────────────────────────────

$('#btn-play')?.addEventListener('click', async () => {
  try {
    await invoke('launch_game');
    showToast('Game launched!', 'success');
  } catch (e) {
    showToast('Failed: ' + e, 'error');
  }
});

// ── Clear Cache ─────────────────────────────────

$('#btn-clear-cache')?.addEventListener('click', async () => {
  try {
    await invoke('clear_cache');
    showToast('Cache cleared', 'success');
  } catch (e) {
    showToast('Failed: ' + e, 'error');
  }
});

// ── Register Button ─────────────────────────────

$('#btn-register')?.addEventListener('click', async () => {
  if (!invoke || !config.account_url) return;
  await invoke('open_url', { url: config.account_url });
});

// ── Download Progress ───────────────────────────

listen('client-progress', (event) => {
    const p = event.payload;
    el.stateNotInstalled.classList.add('hidden');
    el.stateReady.classList.add('hidden');
    el.stateDownloading.classList.remove('hidden');

    const pct = p.total > 0 ? Math.round((p.downloaded / p.total) * 100) : 0;
    el.progressFill.style.width = pct + '%';
    el.progressStatus.textContent = p.message;
    el.progressDetail.textContent = p.speed || '';

    if (p.phase === 'syncing' || p.phase === 'downloading') {
      el.progressPhase.textContent = pct + '% Complete';
    } else {
      el.progressPhase.textContent = p.message;
    }

    if (p.phase === 'complete') {
      setTimeout(() => updateClientState(), 1000);
    }
  });

// ── Addons ──────────────────────────────────────

refreshAddonCount();

async function loadAddons() {
  try {
    addonList = await invoke('fetch_addon_list');
    installedAddons = await invoke('list_installed_addons');
    refreshAddonCount();
    renderAddonGrid();
  } catch (e) {
    el.addonGrid.innerHTML = '<div class="loading">Failed to load addons</div>';
  }
}

function refreshAddonCount() {
  el.installedCount.textContent = installedAddons.length;
  if (clientStatus) {
    el.statAddons.textContent = installedAddons.length > 0 ? `${installedAddons.length} installed` : '—';
  }
}

function renderAddonGrid() {
  const activeTab = getActiveAddonTab();
  if (activeTab === 'installed') return renderInstalled();
  return renderBrowse();
}

function renderInstalled() {
  if (installedAddons.length === 0) {
    el.addonGrid.innerHTML = '<div class="loading">No addons installed yet. Switch to Browse tab to find addons.</div>';
    el.addonPager.classList.add('hidden');
    return;
  }
  el.addonPager.classList.add('hidden');
  let html = '';
  for (const addon of installedAddons) {
    html += cardHtml(addon, false, true);
  }
  el.addonGrid.innerHTML = html;
  bindCardClicks();
}

function renderBrowse() {
  const query = el.addonSearch.value.toLowerCase();
  const sort = el.addonSort.value;
  const expansion = el.addonExpansion.value;

  let filtered = addonList.filter(a => {
    if (!hasDownloadFor(a, expansion)) return false;
    const t = (a.title || a.name || '').toLowerCase();
    const d = (a.description || '').toLowerCase();
    const u = (a.author || '').toLowerCase();
    return t.includes(query) || d.includes(query) || u.includes(query);
  });

  if (sort === 'popular') {
    filtered.sort((a, b) => (b.downloadCount || 0) - (a.downloadCount || 0));
  } else if (sort === 'newest') {
    filtered.sort((a, b) => (b.uploadDate || '').localeCompare(a.uploadDate || ''));
  } else if (sort === 'az') {
    filtered.sort((a, b) => (a.title || '').localeCompare(b.title || ''));
  } else if (sort === 'za') {
    filtered.sort((a, b) => (b.title || '').localeCompare(a.title || ''));
  }

  const totalPages = Math.max(1, Math.ceil(filtered.length / PAGE_SIZE));
  if (addonPage > totalPages) addonPage = totalPages;
  const start = (addonPage - 1) * PAGE_SIZE;
  const page = filtered.slice(start, start + PAGE_SIZE);

  el.pageInfo.textContent = `Page ${addonPage} of ${totalPages}`;
  el.addonPager.classList.toggle('hidden', filtered.length <= PAGE_SIZE);

  if (filtered.length === 0) {
    el.addonGrid.innerHTML = '<div class="loading">No addons found</div>';
    return;
  }

  let html = '';
  for (const addon of page) {
    html += cardHtml(addon, true, isAddonInstalled(addon));
  }
  el.addonGrid.innerHTML = html;
  bindCardClicks();
}

function cardHtml(addon, isBrowse, isInstalled) {
  const name = addon.title || addon.name || 'Unknown';
  const desc = addon.description || '';
  const author = addon.author || '';
  const img = addon.image || '';
  const initial = name.charAt(0).toUpperCase();

  return `
    <div class="addon-card" data-addon='${escAttr(JSON.stringify(addon))}'>
      <div class="addon-card-header">
        <div class="addon-thumb">${img ? `<img src="${escHtml(img)}" onerror="this.style.display='none';this.parentElement.textContent='${initial}'"/>` : initial}</div>
        <div class="addon-info">
          <div class="addon-title">${escHtml(name)}</div>
          <div class="addon-author">${escHtml(author)}</div>
          <div class="addon-desc">${escHtml(desc)}</div>
        </div>
      </div>
      <div class="addon-card-footer">
        ${isInstalled
          ? '<span class="addon-badge installed">Installed</span>'
          : isBrowse
            ? '<button class="btn-primary btn-sm install-btn" data-action="install">Install</button>'
            : '<button class="btn-danger btn-sm remove-btn" data-action="remove">Uninstall</button>'}
      </div>
    </div>
  `;
}

function bindCardClicks() {
  el.addonGrid.querySelectorAll('.addon-card').forEach(card => {
    card.addEventListener('click', (e) => {
      if (e.target.closest('button')) return;
      selectedAddon = JSON.parse(card.dataset.addon);
      openModal(selectedAddon);
    });
  });

  el.addonGrid.querySelectorAll('.install-btn').forEach(b => {
    b.addEventListener('click', async (e) => {
      e.stopPropagation();
      const addon = JSON.parse(e.target.closest('.addon-card').dataset.addon);
      await doInstall(addon);
    });
  });

  el.addonGrid.querySelectorAll('.remove-btn').forEach(b => {
    b.addEventListener('click', async (e) => {
      e.stopPropagation();
      const addon = JSON.parse(e.target.closest('.addon-card').dataset.addon);
      await doRemove(addon);
    });
  });
}

function hasDownloadFor(addon, expansion) {
  if (addon.download_url || addon.downloadUrl) return true;
  if (addon.downloads) {
    if (expansion === 'wotlk' && addon.downloads.wotlk) return true;
    if (expansion === 'tbc' && addon.downloads.tbc) return true;
    if (expansion === 'classic' && addon.downloads.classic) return true;
  }
  return false;
}

function isAddonInstalled(addon) {
  const title = (addon.title || addon.name || '').toLowerCase();
  return installedAddons.some(a =>
    a.name.toLowerCase() === createFolderName(title).toLowerCase() ||
    a.title.toLowerCase() === title
  );
}

function createFolderName(name) {
  return name.replace(/[^a-zA-Z0-9_-]/g, '').replace(/\s+/g, '_');
}

async function doInstall(addon) {
  const name = addon.title || addon.name || 'Unknown';
  showToast('Installing ' + name + '...', 'info');
  try {
    await invoke('install_addon', { addon, expansion: el.addonExpansion.value });
    installedAddons = await invoke('list_installed_addons');
    refreshAddonCount();
    renderAddonGrid();
    closeModal();
    showToast('Installed!', 'success');
  } catch (e) {
    showToast('Failed: ' + e, 'error');
  }
}

async function doRemove(addon) {
  const name = addon.title || addon.name || 'Unknown';
  const folderName = createFolderName(name);
  showToast('Removing ' + name + '...', 'info');
  try {
    await invoke('delete_addon', { name: folderName });
    installedAddons = await invoke('list_installed_addons');
    refreshAddonCount();
    renderAddonGrid();
    closeModal();
    showToast('Removed!', 'success');
  } catch (e) {
    showToast('Failed: ' + e, 'error');
  }
}

// ── Modal ───────────────────────────────────────

function openModal(addon) {
  const name = addon.title || addon.name || 'Unknown';
  el.modalTitle.textContent = name;
  el.modalAuthor.textContent = addon.author || 'Unknown author';
  el.modalDesc.textContent = addon.description || 'No description available.';

  el.modalImage.innerHTML = '';
  if (addon.image) {
    el.modalImage.innerHTML = `<img src="${escHtml(addon.image)}" onerror="this.style.display='none';this.parentElement.innerHTML='<div class=modal-image-placeholder>${name.charAt(0).toUpperCase()}</div>'"/>`;
  } else {
    el.modalImage.innerHTML = `<div class="modal-image-placeholder">${name.charAt(0).toUpperCase()}</div>`;
  }

  const installed = isAddonInstalled(addon);
  const activeTab = getActiveAddonTab();

  el.modalAction.textContent = '';
  el.modalAction.onclick = null;
  el.modalAction.className = 'btn-primary';

  if (activeTab === 'browse') {
    if (installed) {
      el.modalAction.style.display = 'none';
    } else {
      el.modalAction.style.display = '';
      el.modalAction.textContent = 'Install';
      el.modalAction.onclick = () => doInstall(addon);
    }
  } else {
    el.modalAction.style.display = '';
    el.modalAction.textContent = 'Uninstall';
    el.modalAction.className = 'btn-danger';
    el.modalAction.onclick = () => doRemove(addon);
  }

  el.modal.classList.remove('hidden');
}

function closeModal() {
  el.modal.classList.add('hidden');
  selectedAddon = null;
}

$('.modal-close')?.addEventListener('click', closeModal);
el.modal?.addEventListener('click', (e) => {
  if (e.target === el.modal) closeModal();
});

// ── Addon Sub-tabs ──────────────────────────────

function getActiveAddonTab() {
  const active = document.querySelector('.addon-tab-btn.active');
  return active ? active.dataset.addonTab : 'browse';
}

document.querySelectorAll('.addon-tab-btn').forEach(btn => {
  btn.addEventListener('click', () => {
    document.querySelectorAll('.addon-tab-btn').forEach(b => b.classList.remove('active'));
    btn.classList.add('active');
    addonPage = 1;
    const actions = $('#browse-actions');
    if (btn.dataset.addonTab === 'installed') {
      if (actions) actions.style.display = 'none';
    } else {
      if (actions) actions.style.display = '';
    }
    renderAddonGrid();
  });
});

// ── Addon Search / Sort / Expansion ─────────────

el.addonSearch?.addEventListener('input', () => { addonPage = 1; renderBrowse(); });
el.addonSort?.addEventListener('change', () => { addonPage = 1; renderBrowse(); });
el.addonExpansion?.addEventListener('change', () => { addonPage = 1; renderBrowse(); });

// ── Pagination ──────────────────────────────────

$('#page-prev')?.addEventListener('click', () => { if (addonPage > 1) { addonPage--; renderBrowse(); } });
$('#page-next')?.addEventListener('click', () => { addonPage++; renderBrowse(); });

// ── Main Tabs ───────────────────────────────────

$$('.tab-btn').forEach(btn => {
  btn.addEventListener('click', () => {
    $$('.tab-btn').forEach(b => b.classList.remove('active'));
    $$('.tab-content').forEach(t => t.classList.remove('active'));
    btn.classList.add('active');
    const tab = document.getElementById('tab-' + btn.dataset.tab);
    if (tab) tab.classList.add('active');
    if (btn.dataset.tab === 'addons') loadAddons();
    if (btn.dataset.tab === 'game') updateClientState();
  });
});

// ── Helpers ─────────────────────────────────────

function escHtml(str) {
  const div = document.createElement('div');
  div.textContent = str || '';
  return div.innerHTML;
}

function escAttr(str) {
  return (str || '').replace(/"/g, '&quot;').replace(/'/g, '&#39;');
}

// ── Start ───────────────────────────────────────

init();
loadAddons();
