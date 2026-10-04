// ─── API Key 管理 Tab ──────────────────────────

const PERM_GROUP_COLORS = {
  "全部权限": "#d29922",
  "消息": "#58a6ff",
  "适配器": "#3fb950",
  "配置": "#bc8cff",
  "会话": "#56d4dd",
  "插件": "#f2cc60",
  "其他": "#6e7681",
};

// 所有可用权限（与后端 Permission 枚举一致）
const ALL_PERMISSIONS = [
  "*", "messagesread", "messagessend", "adaptersread",
  "adaptersmanage", "configread", "configwrite",
  "sessionsread", "sessionsmanage", "websocketconnect", "apikeysmanage",
  "pluginsread", "pluginsmanage",
];

// 创建模板
const KEY_TEMPLATES = [
  {
    name: "客服机器人",
    icon: "📨",
    desc: "自动回复机器人，只接收用户消息",
    permissions: ["messagessend", "websocketconnect"],
  },
  {
    name: "监控告警",
    icon: "🔔",
    desc: "监控连接状态，异常时告警",
    permissions: ["adaptersread"],
  },
  {
    name: "消息日志",
    icon: "📋",
    desc: "消息发送记录归档",
    permissions: ["messagesread"],
  },
  {
    name: "会话跟踪",
    icon: "👤",
    desc: "追踪完整对话流程",
    permissions: ["messagesread", "messagessend", "sessionsread"],
  },
  {
    name: "全功能",
    icon: "🚀",
    desc: "业务 API 的全权限开发调试",
    permissions: ["*"],
  },
  {
    name: "自定义",
    icon: "✏️",
    desc: "自由组合需要的权限",
    permissions: [],
  },
];

// 侧滑调试面板状态
let debugWs = null;
let debugLog = [];
const MAX_DEBUG_LOG = 200;
const TARGET_ACTIONS = ['inbound:read', 'messages:read', 'messages:send', 'sessions:read', 'sessions:manage'];
// Raw keys are only available when created and remain memory-only for this page lifecycle.
const sessionRawApiKeys = new Map();

function targetGrantActions(grants, platform, chatId) {
  return [...new Set((grants || [])
    .filter(grant => (grant.platform === '*' || grant.platform === platform)
      && (grant.chat_id === '*' || grant.chat_id === chatId))
    .flatMap(grant => Array.isArray(grant.actions) ? grant.actions : []))];
}

async function loadTargetCatalog(subjectId) {
  const [sessionResponse, grants] = await Promise.all([
    api('/api/v1/sessions'),
    subjectId
      ? api(`/api/v1/subjects/${encodeURIComponent(subjectId)}/target-grants`)
      : Promise.resolve([]),
  ]);
  const sessions = Array.isArray(sessionResponse.sessions) ? sessionResponse.sessions : [];
  const targets = [...new Map(sessions
    .filter(session => session.platform && session.chat_id)
    .map(session => [`${session.platform}:${session.chat_id}`, session]))
    .values()]
    .map(session => {
      const key = `${session.platform}:${session.chat_id}`;
      const sessionName = session.custom_name ? String(session.custom_name) : (session.source?.chat_name ? String(session.source.chat_name) : '');
      const actions = targetGrantActions(grants, session.platform, session.chat_id);
      return {
        key,
        platform: session.platform,
        chatId: session.chat_id,
        sessionName,
        label: sessionName ? `${key} · ${sessionName}` : key,
        searchText: `${key} ${sessionName} ${actions.join(' ')}`.toLowerCase(),
        actions,
        granted: actions.length > 0,
      };
    })
    .sort((left, right) => left.key.localeCompare(right.key));
  return { targets, grants };
}

function renderTargetPickerOptions(targets, options = {}) {
  const {
    inputType = 'checkbox',
    inputClass = 'target-picker-input',
    inputName = '',
    disableGranted = false,
    showGrantActions = false,
    emptyText = '当前没有可用的活跃 Target',
  } = options;
  if (!targets.length) return `<div class="target-picker-empty">${escapeHtml(emptyText)}</div>`;
  return targets.map(target => {
    const disabled = disableGranted && target.granted;
    const status = target.granted
      ? (showGrantActions ? `已配置 ${target.actions.join(', ')}` : '已授权')
      : (showGrantActions ? '未配置授权' : '');
    return `<label class="target-picker-option${disabled ? ' is-disabled' : ''}" data-search="${escapeHtml(target.searchText)}">
      <input type="${inputType}" class="${escapeHtml(inputClass)}"${inputName ? ` name="${escapeHtml(inputName)}"` : ''} data-platform="${escapeHtml(target.platform)}" data-chat-id="${escapeHtml(target.chatId)}" data-grant-actions="${escapeHtml(target.actions.join(','))}"${disabled ? ' disabled' : ''}>
      <span class="target-picker-copy">
        <span class="target-picker-label">${escapeHtml(target.label)}</span>
        ${status ? `<span class="target-picker-status">${escapeHtml(status)}</span>` : ''}
      </span>
    </label>`;
  }).join('');
}

function bindTargetPicker(searchInput, list, onSelectionChange) {
  if (!searchInput || !list) return;
  searchInput.oninput = () => {
    const query = searchInput.value.trim().toLowerCase();
    list.querySelectorAll('.target-picker-option').forEach(option => {
      option.hidden = Boolean(query) && !option.dataset.search.includes(query);
    });
  };
  if (onSelectionChange) list.onchange = onSelectionChange;
}

function selectedTargetPickerValues(list, inputClass) {
  if (!list) return [];
  return [...list.querySelectorAll(`.${inputClass}:checked`)].map(input => ({
    platform: input.dataset.platform,
    chat_id: input.dataset.chatId,
    actions: input.dataset.grantActions ? input.dataset.grantActions.split(',') : [],
  }));
}

async function loadApiKeys() {
  const loading = document.getElementById('apikeys-loading');
  const content = document.getElementById('apikeys-content');
  const isFirstLoad = content.style.display === 'none';
  if (isFirstLoad) {
    loading.style.display = 'block';
    content.style.display = 'none';
  }

  try {
    const keys = await api('/api/v1/api-keys?limit=1000&offset=0');

    let html = '<div style="display:flex;gap:8px;margin-bottom:12px">';
    html += '<button class="btn btn-primary" id="apikey-create-btn">➕ 创建 API Key</button>';
    html += '</div>';

    if (!keys || !keys.length) {
      html += '<div class="card"><p style="color:var(--text-muted)">暂无 API Key</p></div>';
    } else {
      html += '<div class="table-wrapper"><table><thead><tr>' +
        '<th>名称</th><th>Key</th><th>权限</th><th>分钟配额</th><th>状态</th><th>创建时间</th><th>操作</th>' +
        '</tr></thead><tbody>';
      for (const k of keys) {
        const masked = String(k.prefix ? k.prefix + '****' : '****');
        const keyId = escapeHtml(String(k.id || ''));
        const keyName = escapeHtml(String(k.name || ''));
        const statusHtml = k.revoked
          ? '<span class="badge badge-red">已吊销</span>'
          : '<span class="badge badge-green">正常</span>';
        const permHtml = k.permissions.includes('*')
          ? '<span class="badge badge-blue">全部</span>'
          : k.permissions.map(p => '<span class="badge badge-gray" style="margin:1px">' + escapeHtml(String(p)) + '</span>').join('');
        const created = new Date(k.created_at).toLocaleString();
        const debugBtn = k.revoked
          ? '<button class="btn btn-sm" disabled>调试</button>'
          : `<button class="btn btn-sm api-key-action" data-action="debug" data-key-id="${keyId}">🔍 调试</button>`;
        const rotateBtn = k.revoked
          ? '<button class="btn btn-sm" disabled>轮换</button>'
          : `<button class="btn btn-sm api-key-action" data-action="rotate" data-key-id="${keyId}">🔄 轮换</button>`;
        const revokeBtn = k.revoked
          ? `<button class="btn btn-sm btn-danger api-key-action" data-action="delete" data-key-id="${keyId}">删除</button>`
          : `<button class="btn btn-sm btn-danger api-key-action" data-action="revoke" data-key-id="${keyId}">吊销</button>`;
        const grantsBtn = `<button class="btn btn-sm api-key-action" data-action="grants" data-key-id="${keyId}">🛡️ Target 授权</button>`;
        html += `<tr>
          <td style="white-space:nowrap"><strong>${keyName}</strong></td>
          <td style="font-family:monospace;font-size:12px">${escapeHtml(masked)}</td>
          <td style="font-size:12px">${permHtml}</td>
          <td style="font-size:12px">${k.requests_per_minute ?? '无限制'}</td>
          <td>${statusHtml}</td>
          <td style="font-size:12px;color:var(--text-muted)">${created}</td>
          <td style="white-space:nowrap">${grantsBtn} ${debugBtn} ${rotateBtn} ${revokeBtn}</td>
        </tr>`;
      }
      html += '</tbody></table></div>';
    }

    // 调试面板容器（初始隐藏）
    html += '<div class="debug-panel" id="debug-panel" style="display:none"></div>';

    content.innerHTML = html;
    content.querySelectorAll('.api-key-action').forEach(button => {
      button.addEventListener('click', () => {
        const key = keys.find(item => String(item.id) === button.dataset.keyId);
        if (!key) return;
        if (button.dataset.action === 'grants') {
          showTargetGrantsDialog(key.subject_id, key.name);
        } else if (button.dataset.action === 'debug') {
          openDebugPanel(key.id, key.name, key.prefix ? key.prefix + '****' : '****', key.subject_id, key.permissions);
        } else if (button.dataset.action === 'rotate') {
          showRotateKeyDialog(key);
        } else if (button.dataset.action === 'delete') {
          deleteApiKey(key.id, key.name, button);
        } else {
          revokeApiKey(key.id, key.name, button);
        }
      });
    });
    loading.style.display = 'none';
    content.style.display = 'block';

    // 绑定创建按钮
    document.getElementById('apikey-create-btn').addEventListener('click', showCreateDialog);

  } catch (e) {
    if (isFirstLoad) {
      loading.innerHTML = '加载失败: ' + escapeHtml(e.message);
    } else {
      content.innerHTML = '<div class="error-msg" style="padding:12px">刷新失败: ' + escapeHtml(e.message) + '</div>';
    }
  }
}

function showRotateKeyDialog(key) {
  const modalKey = `rotate-api-key:${key.id}`;
  if (!beginModal(modalKey)) return;
  const overlay = document.createElement('div');
  overlay.className = 'modal-overlay';
  overlay.dataset.modalKey = modalKey;
  overlay.style.display = 'flex';
  const defaultExpiry = new Date(Date.now() + 365 * 24 * 60 * 60 * 1000);
  defaultExpiry.setMinutes(defaultExpiry.getMinutes() - defaultExpiry.getTimezoneOffset());
  overlay.innerHTML = `
    <div class="modal-card" style="max-width:520px">
      <div class="modal-header"><h3>🔄 轮换 API Key</h3><button class="modal-close">&times;</button></div>
      <div style="padding:16px">
        <p>将轮换 <strong>${escapeHtml(key.name)}</strong>（${escapeHtml(key.prefix || '')}****）。新 Key 继承相同 Subject、接口权限、配额和 Target 授权。</p>
        <label for="rotate-key-expiry">新 Key 过期时间</label>
        <input id="rotate-key-expiry" type="datetime-local" value="${defaultExpiry.toISOString().slice(0, 16)}" style="width:100%;margin:6px 0 12px">
        <button class="btn btn-primary" id="rotate-key-submit">确认轮换</button>
        <div id="rotate-key-result" style="margin-top:12px"></div>
      </div>
    </div>`;
  document.body.appendChild(overlay);
  const close = () => { overlay.remove(); finishModal(modalKey); };
  overlay.querySelector('.modal-close').onclick = close;
  overlay.querySelector('#rotate-key-submit').onclick = async event => {
    const button = event.currentTarget;
    const actionKey = `api-key-rotate:${key.id}`;
    if (!beginAction(actionKey, button, '轮换中...')) return;
    try {
      const value = overlay.querySelector('#rotate-key-expiry').value;
      const expiresAt = new Date(value).getTime();
      if (!Number.isFinite(expiresAt)) throw new Error('请选择有效的过期时间');
      const replacement = await api(`/api/v1/api-keys/${encodeURIComponent(key.id)}/rotate`, {
        method: 'POST',
        body: { expires_at: expiresAt },
      });
      if (replacement.id && replacement.key) sessionRawApiKeys.set(String(replacement.id), replacement.key);
      overlay.querySelector('#rotate-key-result').innerHTML = `
        <div class="success-msg">✅ 轮换完成。旧 Key 已吊销，新 Key 仅显示一次：</div>
        <code style="display:block;word-break:break-all;padding:10px;margin:8px 0;background:var(--bg-tertiary);border-radius:6px">${escapeHtml(replacement.key)}</code>
        <button class="btn btn-sm" id="rotate-key-copy">复制新 Key</button>`;
      overlay.querySelector('#rotate-key-copy').onclick = () => {
        navigator.clipboard.writeText(replacement.key).catch(() => {});
        showToast('新 Key 已复制', 'success');
      };
      button.style.display = 'none';
      overlay.querySelector('#rotate-key-expiry').disabled = true;
      await loadApiKeys();
    } catch (error) {
      overlay.querySelector('#rotate-key-result').innerHTML = `<div class="error-msg">❌ ${escapeHtml(error.message)}</div>`;
      endAction(actionKey, button);
    }
  };
}

async function showTargetGrantsDialog(subjectId, keyName) {
  const modalKey = `target-grants:${subjectId}`;
  if (!beginModal(modalKey)) return;
  try {
    const { targets: availableTargets, grants } = await loadTargetCatalog(subjectId);
    const grantedTargets = new Set(availableTargets.filter(target => target.granted).map(target => target.key));
    const targetOptions = renderTargetPickerOptions(availableTargets, {
      inputClass: 'grant-target-checkbox',
      disableGranted: true,
      emptyText: '当前没有可选的活跃 Target，请先建立会话。',
    });
    const overlay = document.createElement('div');
    overlay.className = 'modal-overlay target-grants-modal';
    overlay.dataset.modalKey = modalKey;
    overlay.style.display = 'flex';
    const rows = renderTargetGrantRows(grants);
    overlay.innerHTML = `
      <div class="modal-card target-grants-card">
        <div class="modal-header"><h3>🛡️ Target 授权</h3><button class="modal-close">&times;</button></div>
        <div class="target-grants-body">
          <p style="color:var(--text-muted);font-size:13px">授权属于 Subject <code>${escapeHtml(subjectId)}</code>，不属于 API Key；轮换 Key 不会改变这些授权。</p>
          <section class="target-grants-section">
            <h4>已授权 Target</h4>
            <div class="table-wrapper target-grants-current"><table><thead><tr><th>Platform</th><th>Chat ID</th><th>Actions</th><th>操作</th></tr></thead><tbody>${rows}</tbody></table></div>
          </section>
          <section class="target-grants-section">
            <h4>添加授权</h4>
            <label for="grant-target-search">从现有活跃 Target 中选择（可多选）</label>
            <input id="grant-target-search" type="search" placeholder="搜索平台、Chat ID 或会话名称...">
            <div id="grant-target-options" class="target-picker-list">
            ${targetOptions}
            </div>
            <div class="target-picker-help">仅展示当前已建立的活跃会话；已授权 Target 会被标记并禁用。</div>
            <div class="target-action-list">
              ${TARGET_ACTIONS.map(action => `<label><input type="checkbox" class="target-action" value="${action}" checked> ${action}</label>`).join('')}
            </div>
            <button class="btn btn-primary" id="target-grant-create">添加授权</button>
          </section>
        </div>
      </div>`;
    document.body.appendChild(overlay);
    overlay.querySelector('.modal-close').onclick = () => overlay.remove();
    bindTargetPicker(
      overlay.querySelector('#grant-target-search'),
      overlay.querySelector('#grant-target-options'),
    );
    overlay.querySelector('#target-grant-create').onclick = async event => {
      const button = event.currentTarget;
      const actionKey = `target-grant-create:${subjectId}`;
      if (!beginAction(actionKey, button, '添加中...')) return;
      try {
        const targets = selectedTargetPickerValues(
          overlay.querySelector('#grant-target-options'),
          'grant-target-checkbox',
        );
        const actions = [...overlay.querySelectorAll('.target-action:checked')].map(input => input.value);
        const uniqueTargets = [...new Map(targets
          .filter(target => target.platform && target.chat_id)
          .map(target => [`${target.platform}:${target.chat_id}`, target]))
          .values()];
        const newTargets = uniqueTargets.filter(target => !grantedTargets.has(`${target.platform}:${target.chat_id}`));
        if (!newTargets.length || !actions.length) {
          endAction(actionKey, button);
          return showToast('请选择至少一个新 Target，并选择 action', 'error');
        }
        for (const target of newTargets) {
          await api(`/api/v1/subjects/${encodeURIComponent(subjectId)}/target-grants`, {method:'POST', body:{...target, actions}});
        }
        endAction(actionKey, button);
        showToast(`${keyName} 已添加 ${newTargets.length} 个 Target 授权`, 'success');
        await refreshTargetGrantsDialog(overlay, subjectId, keyName);
      } catch (e) {
        endAction(actionKey, button);
        showToast('创建授权失败: ' + e.message, 'error');
      }
    };
    bindTargetGrantDeleteButtons(overlay, subjectId, keyName);
  } catch (e) {
    showToast('加载 Target 授权失败: ' + e.message, 'error');
  } finally {
    finishModal(modalKey);
  }
}

function renderTargetGrantRows(grants) {
  return grants.length ? grants.map(grant => `
    <tr>
      <td>${escapeHtml(grant.platform)}</td>
      <td><code>${escapeHtml(grant.chat_id)}</code></td>
      <td>${grant.actions.map(action => `<span class="badge badge-gray">${escapeHtml(action)}</span>`).join(' ')}</td>
      <td><button class="btn btn-sm btn-danger target-grant-delete" data-id="${escapeHtml(grant.id)}">删除</button></td>
    </tr>`).join('') : '<tr><td colspan="4" style="color:var(--text-muted)">当前 Subject 没有 Target 授权，不会接收或访问任何会话数据。</td></tr>';
}

function bindTargetGrantDeleteButtons(overlay, subjectId, keyName) {
  overlay.querySelectorAll('.target-grant-delete').forEach(button => button.onclick = async () => {
    const actionKey = `target-grant-delete:${subjectId}:${button.dataset.id}`;
    if (!beginAction(actionKey, button, '删除中...')) return;
    if (!(await confirmDialog('删除后对应客户端将立即失去访问权限，确认继续？', { title: '删除 Target 授权', danger: true, confirmText: '删除' }))) {
      endAction(actionKey, button);
      return;
    }
    try {
      await api(`/api/v1/subjects/${encodeURIComponent(subjectId)}/target-grants/${encodeURIComponent(button.dataset.id)}`, {method:'DELETE'});
      endAction(actionKey, button);
      await refreshTargetGrantsDialog(overlay, subjectId, keyName);
    } catch (e) {
      endAction(actionKey, button);
      showToast('删除授权失败: ' + e.message, 'error');
    }
  });
}

async function refreshTargetGrantsDialog(overlay, subjectId, keyName) {
  if (!overlay?.isConnected) return;
  const { targets, grants } = await loadTargetCatalog(subjectId);
  const tbody = overlay.querySelector('.target-grants-current tbody');
  const list = overlay.querySelector('#grant-target-options');
  const search = overlay.querySelector('#grant-target-search');
  if (!tbody || !list || !search) return;
  tbody.innerHTML = renderTargetGrantRows(grants);
  list.innerHTML = renderTargetPickerOptions(targets, {
    inputClass: 'grant-target-checkbox',
    disableGranted: true,
    emptyText: '当前没有可选的活跃 Target，请先建立会话。',
  });
  search.value = '';
  bindTargetPicker(search, list);
  bindTargetGrantDeleteButtons(overlay, subjectId, keyName);
}

