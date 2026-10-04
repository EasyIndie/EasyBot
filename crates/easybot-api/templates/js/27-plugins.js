// ─── Plugins Tab ──────────────────────────────

// 加载已安装插件 + 市场目录（任一失败视为整体失败：未启用特性时两者均 400）
async function loadPlugins() {
  const loading = document.getElementById('plugins-loading');
  const content = document.getElementById('plugins-content');
  const signal = getTabController('plugins');
  try {
    loading.style.display = 'block';
    content.style.display = 'none';
    const [installed, catalog] = await Promise.all([
      api('/api/v1/plugins', { signal }),
      api('/api/v1/plugins/catalog', { signal }),
    ]);
    renderPlugins(installed.plugins || [], catalog.plugins || []);
    loading.style.display = 'none';
    content.style.display = 'block';
  } catch (e) {
    if (e.name === 'AbortError') return;
    if (e.message.includes('Plugin system is not enabled')) {
      loading.innerHTML = '<div class="card" style="color:var(--text-muted);text-align:center;padding:24px">🧩 插件系统未启用<br><span style="font-size:12px">当前构建未开启 <code>plugin-system</code> 特性</span></div>';
    } else {
      loading.innerHTML = '加载失败: ' + escapeHtml(e.message);
    }
  }
}

// 已安装插件行
function renderInstalledRow(p) {
  let stateBadge;
  if (p.load_error) stateBadge = `<span class="badge badge-red" title="${escapeHtml(String(p.load_error))}">加载失败</span>`;
  else if (!p.enabled) stateBadge = '<span class="badge badge-gray">已禁用</span>';
  else if (p.platform) stateBadge = '<span class="badge badge-green">已加载</span>';
  else stateBadge = '<span class="badge badge-yellow">未加载</span>';
  let sigBadge = '<span class="badge badge-gray">未签名</span>';
  if (p.signed) {
    sigBadge = p.signature_valid
      ? '<span class="badge badge-green" title="ed25519 签名校验通过（只证作者与完整性，不证安全）">✓ 已验证</span>'
      : '<span class="badge badge-red" title="签名校验失败">✗ 验签失败</span>';
  }
  const name = escapeHtml(String(p.name || ''));
  const display = escapeHtml(String(p.display_name || p.name || ''));
  const loadError = p.load_error ? `<div style="font-size:11px;color:var(--danger);margin-top:3px;max-width:320px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap" title="${escapeHtml(String(p.load_error))}">${escapeHtml(String(p.load_error))}</div>` : '';
  return `<tr data-plugin="${name}">
    <td><div style="font-weight:600;color:var(--text-primary)">${display}</div>
      <div style="font-size:11px;color:var(--text-faint);font-family:var(--font-mono)">${name}</div>${loadError}</td>
    <td style="font-size:12px;color:var(--text-secondary)">${escapeHtml(String(p.version || ''))}</td>
    <td style="font-size:12px;color:var(--text-secondary)">${escapeHtml(String(p.publisher || '-'))}</td>
    <td>${sigBadge}</td>
    <td>${stateBadge}</td>
    <td style="white-space:nowrap">
      <button class="btn btn-sm plugin-action" data-action="${p.enabled ? 'disable' : 'enable'}" data-name="${name}">${p.enabled ? '禁用' : '启用'}</button>
      <button class="btn btn-sm btn-danger plugin-action" data-action="uninstall" data-name="${name}">卸载</button>
    </td>
  </tr>`;
}

// 市场目录行
function renderCatalogRow(c) {
  const verified = c.verified
    ? '<span class="badge badge-blue" title="官方验证（只证身份，不证安全）">✓ 官方</span>'
    : '<span class="badge badge-gray">社区</span>';
  const qualified = c.publisher ? `${c.publisher}/${c.name}` : c.name;
  return `<tr data-catalog-name="${escapeHtml(qualified)}">
    <td><div style="font-weight:600;color:var(--text-primary)">${escapeHtml(String(c.display_name || c.name || ''))}</div>
      <div style="font-size:11px;color:var(--text-faint);font-family:var(--font-mono)">${escapeHtml(String(c.name || ''))}</div></td>
    <td style="font-size:12px">${escapeHtml(String(c.publisher || '-'))} ${verified}</td>
    <td style="font-size:12px;color:var(--text-secondary);max-width:340px">${escapeHtml(String(c.description || '-'))}</td>
    <td style="font-size:11px">${(c.tags || []).map(t => `<span class="badge badge-gray">${escapeHtml(String(t))}</span>`).join(' ') || '-'}</td>
    <td><button class="btn btn-sm btn-primary plugin-action" data-action="install" data-name="${escapeHtml(qualified)}">安装</button></td>
  </tr>`;
}

function renderPlugins(installed, catalog) {
  const content = document.getElementById('plugins-content');
  content.innerHTML = `
    <div class="card" style="margin-bottom:16px">
      <h3 style="margin:0 0 12px">📦 已安装插件</h3>
      <div class="table-wrapper"><table>
        <thead><tr><th>插件</th><th>版本</th><th>发布者</th><th>签名</th><th>状态</th><th style="width:150px">操作</th></tr></thead>
        <tbody>${installed.length ? installed.map(renderInstalledRow).join('') : '<tr><td colspan="6" style="text-align:center;color:var(--text-muted)">暂无已安装插件</td></tr>'}</tbody>
      </table></div>
    </div>
    <div class="card">
      <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:12px;gap:8px;flex-wrap:wrap">
        <h3 style="margin:0">🛒 插件市场</h3>
        <div style="display:flex;gap:6px;flex-wrap:wrap">
          <input type="text" id="plugin-install-name" placeholder="按名称安装 (publisher/name)" style="width:200px" title="也可输入发布者限定名，如 easybot/slack">
          <button class="btn btn-sm btn-primary" id="plugin-install-btn">安装</button>
        </div>
      </div>
      <div style="display:flex;gap:6px;margin-bottom:12px">
        <input type="text" id="plugin-search" placeholder="搜索市场插件 (名称/描述/标签)..." style="flex:1">
        <button class="btn btn-sm" id="plugin-search-btn">🔍 搜索</button>
        <button class="btn btn-sm" id="plugin-refresh">🔄 刷新</button>
      </div>
      <div class="table-wrapper"><table>
        <thead><tr><th>插件</th><th>发布者</th><th>描述</th><th>标签</th><th style="width:80px">操作</th></tr></thead>
        <tbody id="catalog-rows">${catalog.length ? catalog.map(renderCatalogRow).join('') : '<tr><td colspan="5" style="text-align:center;color:var(--text-muted)">市场中暂无插件</td></tr>'}</tbody>
      </table></div>
    </div>`;
  document.getElementById('plugin-refresh').addEventListener('click', loadPlugins);
  document.getElementById('plugin-search-btn').addEventListener('click', searchCatalog);
  document.getElementById('plugin-search').addEventListener('keydown', e => { if (e.key === 'Enter') searchCatalog(); });
  document.getElementById('plugin-install-btn').addEventListener('click', () => installByName());
  document.getElementById('plugin-install-name').addEventListener('keydown', e => { if (e.key === 'Enter') installByName(); });
  bindPluginActions();
}

function bindPluginActions() {
  document.querySelectorAll('#tab-plugins .plugin-action').forEach(button => {
    button.addEventListener('click', () => {
      if (button.dataset.action === 'install') doInstall(button.dataset.name, button);
      else pluginAction(button.dataset.action, button.dataset.name, button);
    });
  });
}

// 信任发布者确认弹窗（对齐 VS Code 语义：本次信任仅用于本次安装，不写 .trust）
function showTrustConfirm(name, publisher, onDone) {
  const modalKey = 'plugin-trust';
  if (!beginModal(modalKey)) return;
  const overlay = document.createElement('div');
  overlay.className = 'modal-overlay';
  overlay.dataset.modalKey = modalKey;
  overlay.innerHTML = `
    <div class="modal-card" style="max-width:480px">
      <div class="modal-header"><h3>🛡️ 信任发布者</h3><button class="modal-close" title="取消">&times;</button></div>
      <div style="padding:16px;font-size:13px;line-height:1.8;color:var(--text-secondary)">
        <p>插件 <strong>${escapeHtml(name)}</strong> 的发布者 <strong>${escapeHtml(publisher || '未知')}</strong> 不在受信任列表。</p>
        <p>⚠️ 签名只能证明<b>作者与完整性</b>，<b>不能证明代码安全</b>。插件以宿主权限运行，无沙箱隔离。</p>
        <p style="color:var(--text-muted)">确认后本次安装继续；不会自动加入长期信任（如需长期信任，请使用 CLI <code>plugin trust</code>）。</p>
      </div>
      <div style="display:flex;justify-content:flex-end;gap:8px;padding:12px 16px;border-top:1px solid var(--border-muted)">
        <button class="btn" id="trust-cancel-btn">取消</button>
        <button class="btn btn-primary" id="trust-install-btn">信任并安装</button>
      </div>
    </div>`;
  document.body.appendChild(overlay);
  const close = () => { overlay.remove(); finishModal(modalKey); };
  overlay.querySelector('.modal-close').onclick = () => { close(); onDone(false); };
  overlay.querySelector('#trust-cancel-btn').onclick = () => { close(); onDone(false); };
  overlay.querySelector('#trust-install-btn').onclick = () => { close(); onDone(true); };
}

async function doInstall(name, button) {
  const key = `plugin:install:${name}`;
  if (!beginAction(key, button, '安装中...')) return;
  try {
    let resp = await api('/api/v1/plugins/install', { method: 'POST', body: { name, channel: 'stable', trust: false } });
    if (resp.needs_trust_confirmation) {
      const ok = await new Promise(resolve => showTrustConfirm(resp.name || name, resp.publisher, resolve));
      if (!ok) { showToast('已取消安装', 'info'); return; }
      resp = await api('/api/v1/plugins/install', { method: 'POST', body: { name, channel: 'stable', trust: true } });
    }
    showToast(resp.upgraded ? `已升级 ${resp.name} v${resp.version}` : `已安装 ${resp.name} v${resp.version}`, 'success');
  } catch (e) {
    showToast(e.message, 'error');
  } finally {
    endAction(key, button);
    loadPlugins();
  }
}

function installByName() {
  const name = document.getElementById('plugin-install-name').value.trim();
  if (!name) { showToast('请输入插件名', 'error'); return; }
  doInstall(name, document.getElementById('plugin-install-btn'));
}

async function searchCatalog() {
  const q = document.getElementById('plugin-search').value.trim();
  const rows = document.getElementById('catalog-rows');
  rows.innerHTML = '<tr><td colspan="5" style="text-align:center;color:var(--text-muted)">搜索中...</td></tr>';
  try {
    const data = await api('/api/v1/plugins/catalog?query=' + encodeURIComponent(q));
    rows.innerHTML = data.plugins.length
      ? data.plugins.map(renderCatalogRow).join('')
      : '<tr><td colspan="5" style="text-align:center;color:var(--text-muted)">无匹配插件</td></tr>';
    bindPluginActions();
  } catch (e) {
    rows.innerHTML = `<tr><td colspan="5" style="text-align:center;color:var(--danger)">${escapeHtml(e.message)}</td></tr>`;
  }
}

async function pluginAction(action, name, button) {
  const key = `plugin:${action}:${name}`;
  if (!beginAction(key, button, '处理中...')) return;
  try {
    if (action === 'uninstall') {
      if (!(await confirmDialog(`确定卸载插件 ${name} ？\n\n卸载后该插件的会话/消息数据由 TTL 保留策略清理，不会立即删除。`, { title: '卸载插件', danger: true, confirmText: '卸载' }))) return;
      await api(`/api/v1/plugins/${encodeURIComponent(name)}`, { method: 'DELETE' });
      showToast(`已卸载 ${name}`, 'success');
    } else if (action === 'enable') {
      await api(`/api/v1/plugins/${encodeURIComponent(name)}/enable`, { method: 'POST' });
      showToast(`已启用 ${name}（下次启动生效）`, 'success');
    } else if (action === 'disable') {
      await api(`/api/v1/plugins/${encodeURIComponent(name)}/disable`, { method: 'POST' });
      showToast(`已禁用 ${name}`, 'success');
    }
    loadPlugins();
  } catch (e) {
    showToast(e.message, 'error');
  } finally {
    endAction(key, button);
  }
}


