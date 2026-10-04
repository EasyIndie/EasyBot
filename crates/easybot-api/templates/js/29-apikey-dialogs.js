// ─── 创建对话框（单页布局，模板 + 配置同一页面） ──

function buildPermHtml() {
  const permGroups = [
    { title: '全部权限', items: ['*'] },
    { title: '消息', items: ['messagesread', 'messagessend'] },
    { title: '适配器', items: ['adaptersread', 'adaptersmanage'] },
    { title: '配置', items: ['configread', 'configwrite'] },
    { title: '会话', items: ['sessionsread', 'sessionsmanage'] },
    { title: '插件', items: ['pluginsread', 'pluginsmanage'] },
    { title: '其他', items: ['websocketconnect', 'apikeysmanage', 'metricsread', 'auditread'] },
  ];
  let html = '';
  for (const group of permGroups) {
    const dotColor = PERM_GROUP_COLORS[group.title] || '#6e7681';
    let itemsHtml = '';
    for (const p of group.items) {
      const starAttr = p === '*' ? 'onchange="onStarPermissionChange()"' : '';
      itemsHtml += `<label><input type="checkbox" class="perm-item" value="${p}" ${starAttr}><span>${p}</span></label>`;
    }
    html += `<div class="cg-group"><div class="cg-group-title"><span class="cg-dot" style="background:${dotColor}"></span>${group.title}</div><div class="cg-items">${itemsHtml}</div></div>`;
  }
  return html;
}

function showCreateDialog() {
  const modalKey = 'create-api-key';
  if (!beginModal(modalKey)) return;
  const overlay = document.createElement('div');
  overlay.className = 'modal-overlay';
  overlay.dataset.modalKey = modalKey;
  overlay.style.display = 'flex';

  let templateCards = KEY_TEMPLATES.map((t, i) => `
    <div class="template-card" data-idx="${i}" onclick="selectTemplate(${i})">
      <div class="tpl-icon">${t.icon}</div>
      <div class="tpl-name">${t.name}</div>
      <div class="tpl-desc">${t.desc}</div>
    </div>
  `).join('');

  overlay.innerHTML = `
    <div class="modal-card" style="max-width:680px;max-height:90vh;overflow-y:auto">
      <div class="modal-header">
        <h3>🔑 创建 API Key</h3>
        <button class="modal-close" onclick="closeCreateDialog()">&times;</button>
      </div>
      <div style="padding:20px" id="create-form-area">
        <p style="color:var(--text-muted);margin-bottom:8px;font-size:13px">选择场景模板（点击快速填充配置）：</p>
        <div class="template-grid" id="template-list">${templateCards}</div>

        <div class="form-group">
          <label>名称 <span style="color:var(--danger)">*</span></label>
          <input type="text" id="create-key-name" placeholder="例如: 客服机器人">
        </div>

        <div class="form-group">
          <label>每分钟请求配额（留空表示不设置 Key 级配额）</label>
          <input type="number" id="create-key-rpm" min="1" max="1000000" placeholder="例如: 600">
        </div>

        <div class="form-group">
          <label>权限（选 <code>*</code> = 全部）</label>
          <div id="create-permissions" class="checkbox-grid">${buildPermHtml()}</div>
        </div>

        <button class="btn btn-primary" id="create-key-submit" onclick="submitCreateKey()" style="width:100%;margin-top:4px">✅ 创建 API Key</button>
      </div>

      <div id="create-result" style="display:none;padding:20px">
        <div style="text-align:center;padding:8px 0">
          <p style="font-size:32px;margin-bottom:8px">✅</p>
          <p style="color:var(--text-muted);font-size:13px">API Key 创建成功！请立即复制并妥善保管。</p>
        </div>
        <div class="key-result-box">
          <div class="key-warn">⚠️ 密钥只显示一次，关闭后无法再次查看</div>
          <div class="key-value" id="create-result-key"></div>
          <div class="key-actions">
            <button class="btn btn-primary" onclick="copyResultKey()">📋 复制密钥</button>
            <button class="btn" onclick="openDebugWithNewKey()">🔍 调试验证</button>
          </div>
        </div>
        <div style="margin-top:12px;padding:12px;border:1px solid var(--border-muted);border-radius:8px;background:var(--bg-subtle)">
          <div style="font-size:13px;font-weight:600;margin-bottom:4px">下一步：配置 Target 授权</div>
          <div style="color:var(--text-muted);font-size:12px;margin-bottom:10px">API Key 只负责身份认证。当前 Subject 尚未授权任何平台或群组，客户端默认无法读取或发送业务数据。</div>
          <button class="btn btn-primary" onclick="openTargetGrantsForCreatedKey()">🛡️ 配置 Target 授权</button>
        </div>
        <div style="display:flex;gap:8px;margin-top:8px">
          <button class="btn" onclick="resetCreateForm()" style="flex:1">🔄 再创建一个</button>
          <button class="btn" onclick="closeCreateDialogAndRefresh()" style="flex:1">完成</button>
        </div>
      </div>
    </div>
  `;

  document.body.appendChild(overlay);
  document.body.style.overflow = 'hidden';

  // 默认选中"自定义"模板（索引 5）
  selectTemplate(5);
  finishModal(modalKey);
}

// 当前选中的模板索引（-1 = 未选中）
let selectedTemplateIdx = -1;

function selectTemplate(idx) {
  selectedTemplateIdx = idx;
  const tpl = KEY_TEMPLATES[idx];

  // 高亮选中
  document.querySelectorAll('.template-card').forEach((c, i) => {
    c.classList.toggle('selected', i === idx);
  });

  // 隐藏结果区（如果之前创建过），显示表单区
  const resultArea = document.getElementById('create-result');
  const formArea = document.getElementById('create-form-area');
  if (resultArea) resultArea.style.display = 'none';
  if (formArea) formArea.style.display = 'block';

  // 填充名称
  const nameInput = document.getElementById('create-key-name');
  if (nameInput) nameInput.value = tpl.name !== '自定义' ? tpl.name : '';

  // 应用权限预设
  document.querySelectorAll('.perm-item').forEach(cb => {
    cb.checked = tpl.permissions.includes(cb.value);
  });
  onStarPermissionChange(); // 同步 * 的禁用状态

  // 绑定创建提交
  const submitBtn = document.getElementById('create-key-submit');
  if (submitBtn) { submitBtn.onclick = submitCreateKey; submitBtn.disabled = false; submitBtn.textContent = '✅ 创建 API Key'; }
}

function resetCreateForm() {
  document.getElementById('create-result').style.display = 'none';
  document.getElementById('create-form-area').style.display = 'block';
  selectTemplate(5); // 重置为"自定义"
}

function onStarPermissionChange() {
  const starChecked = document.querySelector('.perm-item[value="*"]')?.checked;
  document.querySelectorAll('.perm-item').forEach(cb => {
    if (cb.value !== '*') {
      cb.disabled = starChecked;
      if (starChecked) cb.checked = false;
    }
  });
}

async function openDebugWithNewKey() {
  if (!lastCreatedKeyId || !lastCreatedKey) {
    showToast('当前没有可调试的新 API Key', 'error');
    return;
  }
  closeCreateDialog(false);
  if (currentTab !== 'apikeys') switchTab('apikeys');
  await loadApiKeys();
  openDebugPanel(lastCreatedKeyId, lastCreatedKeyName, lastCreatedKey.slice(0, 8) + '****', lastCreatedSubjectId, lastCreatedPermissions);
}

let lastCreatedKeyId = '';
let lastCreatedKey = '';
let lastCreatedSubjectId = '';
let lastCreatedKeyName = '';
let lastCreatedPermissions = [];

async function submitCreateKey() {
  const name = document.getElementById('create-key-name').value.trim();
  if (!name) {
    document.getElementById('create-key-name').style.borderColor = 'var(--danger)';
    showToast('请输入名称', 'error');
    return;
  }
  document.getElementById('create-key-name').style.borderColor = '';

  const permissions = [...document.querySelectorAll('.perm-item:checked')].map(cb => cb.value);
  if (!permissions.length) {
    showToast('请至少选择一个权限', 'error');
    return;
  }
  const quotaInput = document.getElementById('create-key-rpm').value.trim();
  const requests_per_minute = quotaInput === '' ? null : Number(quotaInput);
  if (requests_per_minute !== null && (!Number.isInteger(requests_per_minute) || requests_per_minute < 1 || requests_per_minute > 1000000)) {
    showToast('每分钟请求配额必须是 1 到 1000000 之间的整数', 'error');
    return;
  }

  const btn = document.getElementById('create-key-submit');
  if (!beginAction('api-key-create', btn, '创建中...')) return;

  try {
    const result = await api('/api/v1/api-keys', {
      method: 'POST',
      body: { name, permissions, requests_per_minute },
    });
    lastCreatedKeyId = result.id || '';
    lastCreatedKey = result.key || '';
    lastCreatedSubjectId = result.subject_id || '';
    lastCreatedKeyName = name;
    lastCreatedPermissions = Array.isArray(result.permissions) ? result.permissions : permissions;
    if (lastCreatedKeyId && lastCreatedKey) sessionRawApiKeys.set(lastCreatedKeyId, lastCreatedKey);
    document.getElementById('create-form-area').style.display = 'none';
    document.getElementById('create-result').style.display = 'block';
    document.getElementById('create-result-key').textContent = lastCreatedKey;
    endAction('api-key-create', btn);
  } catch (e) {
    showToast('创建失败: ' + e.message, 'error');
    endAction('api-key-create', btn);
  }
}

function openTargetGrantsForCreatedKey() {
  if (!lastCreatedSubjectId) {
    showToast('创建响应缺少 subject_id，无法配置 Target 授权', 'error');
    return;
  }
  showTargetGrantsDialog(lastCreatedSubjectId, lastCreatedKeyName || '新 API Key');
}

function copyResultKey() {
  if (!lastCreatedKey) return;
  navigator.clipboard.writeText(lastCreatedKey).catch(() => {});
  showToast('密钥已复制到剪贴板', 'success');
}

function closeCreateDialog(refresh = true) {
  const overlay = document.querySelector('[data-modal-key="create-api-key"]');
  if (!overlay) return;
  overlay.remove();
  document.body.style.overflow = '';
  if (refresh) loadApiKeys(); // 退出时自动刷新列表
}

function closeCreateDialogAndRefresh() {
  closeCreateDialog();
}

// ─── 吊销 Key ──────────────────────────────────

async function revokeApiKey(id, name, button) {
  const actionKey = `api-key-revoke:${id}`;
  if (!beginAction(actionKey, button, '处理中...')) return;
  const isDev = name === 'dev';
  const msg = isDev
    ? `⚠️ 这是主管理 Key（${name}），确认吊销？此操作不可撤销！`
    : `确定吊销 Key [${name}]？此操作不可撤销！`;
  try {
    if (!(await confirmDialog(msg, { title: '吊销 API Key', danger: true, confirmText: '吊销' }))) return;
    await api(`/api/v1/api-keys/${id}`, { method: 'DELETE' });
    sessionRawApiKeys.delete(String(id));
    showToast(`Key [${name}] 已吊销`, 'success');
    loadApiKeys();
  } catch (e) {
    showToast('吊销失败: ' + e.message, 'error');
  } finally {
    endAction(actionKey, button);
  }
}

async function deleteApiKey(id, name, button) {
  const actionKey = `api-key-purge:${id}`;
  if (!beginAction(actionKey, button, '处理中...')) return;
  try {
    if (!(await confirmDialog(`确定永久删除 Key [${name}]？此操作不可撤销！`, { title: '永久删除', danger: true, confirmText: '删除' }))) return;
    await api(`/api/v1/api-keys/${id}/purge`, { method: 'DELETE' });
    sessionRawApiKeys.delete(String(id));
    showToast(`Key [${name}] 已永久删除`, 'success');
    loadApiKeys();
  } catch (e) {
    showToast('删除失败: ' + e.message, 'error');
  } finally {
    endAction(actionKey, button);
  }
}

// ─── 调试面板 ──────────────────────────────────

function openDebugPanel(id, name, masked, subjectId, permissions = []) {
  const panel = document.getElementById('debug-panel');
  if (!panel) return;

  // Raw values are available for keys created in this page lifecycle and for
  // the credential currently authenticating the management page when prefixes match.
  const rememberedKey = sessionRawApiKeys.get(String(id)) || '';
  const selectedPrefix = String(masked || '').replace(/\*+$/, '');
  const currentPageKey = apiKey && selectedPrefix && apiKey.startsWith(selectedPrefix) ? apiKey : '';
  const savedTestKey = rememberedKey || currentPageKey;
  const normalizedPermissions = Array.isArray(permissions) ? permissions : [];
  const permissionLabel = normalizedPermissions.includes('*')
    ? '全部接口权限'
    : (normalizedPermissions.join(', ') || '无接口权限');
  const keyInputHint = rememberedKey
    ? '已自动填入本次页面会话中创建的 Key。'
    : (currentPageKey
      ? '已自动填入当前管理页面正在使用的 Key。'
      : '历史 Key 的明文不会由服务端返回，请粘贴该 Key 的完整值。');

  panel.style.display = 'block';
  panel.innerHTML = `
    <div class="dbg-header">
      <div>
        <h3>🔍 调试: ${escapeHtml(name)}</h3>
        <div class="dbg-meta">${escapeHtml(masked)} · API Key 接口权限：${escapeHtml(permissionLabel)}</div>
      </div>
      <button class="modal-close" onclick="closeDebugPanel()">&times;</button>
    </div>
    <div style="padding:8px 16px;border-bottom:1px solid var(--border-muted)">
      <label class="dbg-key-label">输入要测试的 API Key（独立连接，不影响主页面）</label>
      <input type="password" class="dbg-key-input" id="debug-key-input" value="${escapeHtml(savedTestKey)}" autocomplete="off" placeholder="粘贴完整 API Key 进行测试...">
      <div class="target-picker-help">${keyInputHint}</div>
    </div>
    <div style="padding:12px 16px;border-bottom:1px solid var(--border-muted)">
      <div style="font-size:13px;font-weight:600;margin-bottom:6px">🛡️ Target 授权验证</div>
      <div style="color:var(--text-muted);font-size:12px;margin-bottom:8px">有效权限 = API Key 接口权限 ∩ Subject Target action。验证会分别指出缺失的授权层；不会发送实际消息。</div>
      <input type="search" id="debug-target-search" placeholder="搜索平台、Chat ID 或会话名称..." style="width:100%;font-size:12px;margin-bottom:8px">
      <div id="debug-target-options" class="target-picker-list debug-target-picker">
        <div class="target-picker-empty">正在加载可用 Target...</div>
      </div>
      <div id="debug-target-grant-hint" style="color:var(--text-muted);font-size:12px;margin-top:6px">仅展示管理后台当前可见的活跃会话。</div>
      <button class="btn btn-sm btn-primary" id="debug-target-verify-btn" onclick="debugVerifyTarget()" style="margin-top:8px">🧪 验证所选 Target</button>
      <div id="debug-target-results" style="margin-top:8px"></div>
    </div>
    <div class="dbg-toolbar">
      <button class="btn btn-sm btn-primary" id="debug-connect-btn" onclick="debugConnect()">🔗 连接</button>
      <button class="btn btn-sm" id="debug-disconnect-btn" onclick="debugDisconnect()" disabled>⏹ 断开</button>
      <button class="btn btn-sm" onclick="debugClearLog()">🗑 清空日志</button>
      <span id="debug-status" style="font-size:12px;color:var(--text-muted)">● 已断开</span>
    </div>
    <div style="padding:8px 16px"><input type="text" id="debug-filter" placeholder="筛选事件..." oninput="debugFilterLog()" style="width:100%;font-size:12px"></div>
    <div class="dbg-log" id="debug-log-container">
      <div class="dbg-empty">填入 Key 点击"连接"开始测试</div>
    </div>
  `;

  // 存储当前调试的 key id 和 info
  panel.dataset.keyId = id;
  panel.dataset.keyName = name;
  panel.dataset.subjectId = subjectId || '';
  panel.dataset.permissions = JSON.stringify(normalizedPermissions);

  // 重置调试状态
  debugLog = [];
  debugWs = null;
  loadDebugTargets(subjectId);
}

function closeDebugPanel() {
  debugDisconnect();
  const panel = document.getElementById('debug-panel');
  if (panel) panel.style.display = 'none';
}

async function debugRequest(path, testKey, opts = {}) {
  const { method = 'GET', body } = opts;
  try {
    const headers = { 'Authorization': `Bearer ${testKey}` };
    if (body !== undefined) headers['Content-Type'] = 'application/json';
    const response = await fetch(path, {
      method,
      headers,
      body: body === undefined ? undefined : JSON.stringify(body),
    });
    const text = await response.text();
    let data = null;
    try {
      data = text ? JSON.parse(text) : null;
    } catch (_) {
      data = { message: text };
    }
    return { ok: response.ok, status: response.status, data };
  } catch (error) {
    return { ok: false, status: 0, data: { message: error.message } };
  }
}

function debugResponseMessage(response) {
  return response?.data?.error?.message
    || response?.data?.message
    || response?.data?.error
    || (response?.status ? `HTTP ${response.status}` : '请求失败');
}

function updateDebugTargetHint() {
  const list = document.getElementById('debug-target-options');
  const hint = document.getElementById('debug-target-grant-hint');
  if (!list || !hint) return;
  const target = selectedTargetPickerValues(list, 'debug-target-radio')[0];
  if (!target) {
    hint.textContent = '仅展示管理后台当前可见的活跃会话。';
    return;
  }
  hint.textContent = target.actions.length
    ? `后台配置的 Target action：${target.actions.join(', ')}；验证结果仍以该 API Key 的实际请求为准。`
    : '该 Subject 当前没有匹配的 Target Grant；验证请求应被服务端拒绝。';
}

async function loadDebugTargets(subjectId) {
  const panel = document.getElementById('debug-panel');
  const list = document.getElementById('debug-target-options');
  const search = document.getElementById('debug-target-search');
  if (!panel || !list || !search) return;
  const loadId = String(Date.now());
  panel.dataset.targetLoadId = loadId;
  list.innerHTML = '<div class="target-picker-empty">正在加载可用 Target...</div>';
  try {
    const { targets } = await loadTargetCatalog(subjectId);
    if (panel.dataset.targetLoadId !== loadId) return;
    list.innerHTML = renderTargetPickerOptions(targets, {
      inputType: 'radio',
      inputClass: 'debug-target-radio',
      inputName: 'debug-target',
      showGrantActions: true,
    });
    const firstTarget = list.querySelector('.debug-target-radio');
    if (firstTarget) firstTarget.checked = true;
    updateDebugTargetHint();
    bindTargetPicker(search, list, updateDebugTargetHint);
  } catch (error) {
    if (panel.dataset.targetLoadId !== loadId) return;
    list.innerHTML = '<div class="target-picker-empty">加载 Target 失败</div>';
    const hint = document.getElementById('debug-target-grant-hint');
    if (hint) hint.textContent = `无法加载活跃会话：${error.message}`;
  }
}

function debugResultRow(label, endpoint, state, response, detail) {
  const states = {
    passed: { color: 'var(--success)', icon: '✅', label: '通过' },
    permission: { color: 'var(--warning)', icon: '⚠️', label: '接口权限不足' },
    grant: { color: 'var(--warning)', icon: '⚠️', label: 'Target action 不足' },
    failed: { color: 'var(--danger)', icon: '❌', label: '请求失败' },
  };
  const display = states[state] || states.failed;
  const status = response.status ? `HTTP ${response.status}` : '网络错误';
  return `<div style="display:grid;grid-template-columns:120px 1fr auto;gap:8px;align-items:start;padding:6px 8px;border-bottom:1px solid var(--border-muted);font-size:12px">
    <strong style="color:${display.color}">${display.icon} ${escapeHtml(label)}</strong>
    <span><code>${escapeHtml(endpoint)}</code><br><span style="color:var(--text-muted)">${escapeHtml(detail)}</span></span>
    <span style="color:${display.color};white-space:nowrap">${escapeHtml(display.label)} · ${escapeHtml(status)}</span>
  </div>`;
}

function debugAuthorizationState(permissions, targetActions, requiredPermission, requiredAction, passed) {
  const globalAccess = permissions.includes('*');
  if (!globalAccess && !permissions.includes(requiredPermission)) return 'permission';
  if (!globalAccess && !targetActions.includes(requiredAction)) return 'grant';
  return passed ? 'passed' : 'failed';
}

function debugAuthorizationDetail(permissions, targetActions, requiredPermission, requiredAction, passedDetail, failureDetail) {
  const globalAccess = permissions.includes('*');
  if (!globalAccess && !permissions.includes(requiredPermission)) {
    return `API Key 缺少接口权限 ${requiredPermission}；Target Grant 即使包含 ${requiredAction} 也无法访问。`;
  }
  if (!globalAccess && !targetActions.includes(requiredAction)) {
    return `Subject 对该 Target 缺少 action ${requiredAction}。`;
  }
  return failureDetail || passedDetail;
}

async function debugVerifyTarget() {
  const button = document.getElementById('debug-target-verify-btn');
  const list = document.getElementById('debug-target-options');
  const keyInput = document.getElementById('debug-key-input');
  const results = document.getElementById('debug-target-results');
  if (!button || !list || !keyInput || !results) return;
  if (!beginAction('debug-target-verify', button, '验证中...')) return;
  try {
    const panel = document.getElementById('debug-panel');
    const testKey = keyInput.value.trim();
    const selectedTarget = selectedTargetPickerValues(list, 'debug-target-radio')[0];
    if (!testKey) {
      results.innerHTML = '<div class="error-msg">请先填入要测试的 API Key。</div>';
      return;
    }
    if (!selectedTarget) {
      results.innerHTML = '<div class="error-msg">请选择一个活跃 Target。</div>';
      return;
    }
    const platform = selectedTarget.platform;
    const chatId = selectedTarget.chat_id;
    const target = `${platform}:${chatId}`;
    let permissions = [];
    try {
      permissions = JSON.parse(panel?.dataset.permissions || '[]');
    } catch (_) { /* invalid metadata is treated as no declared permissions */ }
    if (!Array.isArray(permissions)) permissions = [];
    const targetActions = Array.isArray(selectedTarget.actions) ? selectedTarget.actions : [];
    const historyQuery = new URLSearchParams({ platform, chat_id: chatId, limit: '1' });
    const [sessionsResponse, historyResponse, sendProbeResponse] = await Promise.all([
      debugRequest('/api/v1/sessions', testKey),
      debugRequest(`/api/v1/messages?${historyQuery.toString()}`, testKey),
      // send_message 先执行 Target 授权，再校验文本长度；超长文本会在授权成功后以 400 返回，绝不会触达适配器。
      debugRequest('/api/v1/messages/send', testKey, {
        method: 'POST',
        body: { target, text: 'x'.repeat(16385), parse_mode: null },
      }),
    ]);
    const visibleSessions = Array.isArray(sessionsResponse.data?.sessions)
      && sessionsResponse.data.sessions.some(session => session.platform === platform && session.chat_id === chatId);
    const historyPassed = historyResponse.ok;
    const sendMessage = debugResponseMessage(sendProbeResponse);
    const sendPassed = sendProbeResponse.status === 400
      && (sendProbeResponse.data?.error?.code === 'MESSAGE_TOO_LONG' || /message too long/i.test(sendMessage));
    const sessionPassed = sessionsResponse.ok && visibleSessions;
    const sessionActualDetail = sessionsResponse.ok
      ? (visibleSessions ? 'Target 出现在该 Key 可见的会话列表中。' : '请求成功，但返回列表不包含该 Target。')
      : debugResponseMessage(sessionsResponse);
    const sessionDetail = debugAuthorizationDetail(permissions, targetActions, 'sessionsread', 'sessions:read', sessionActualDetail, sessionPassed ? '' : sessionActualDetail);
    const historyDetail = debugAuthorizationDetail(permissions, targetActions, 'messagesread', 'messages:read', '消息历史接口允许按该 Target 查询（即使当前没有消息也算通过）。', historyPassed ? '' : debugResponseMessage(historyResponse));
    const sendDetail = debugAuthorizationDetail(permissions, targetActions, 'messagessend', 'messages:send', '服务端先通过 Target 授权，再命中超长文本校验；未发送真实消息。', sendPassed ? '' : sendMessage);
    results.innerHTML = `
      <div style="border:1px solid var(--border-muted);border-radius:6px;overflow:hidden">
        <div style="padding:8px;background:var(--bg-tertiary);font-size:12px">验证 Target：<code>${escapeHtml(target)}</code></div>
        ${debugResultRow('sessions:read', 'GET /api/v1/sessions', debugAuthorizationState(permissions, targetActions, 'sessionsread', 'sessions:read', sessionPassed), sessionsResponse, sessionDetail)}
        ${debugResultRow('messages:read', 'GET /api/v1/messages', debugAuthorizationState(permissions, targetActions, 'messagesread', 'messages:read', historyPassed), historyResponse, historyDetail)}
        ${debugResultRow('messages:send', 'POST /api/v1/messages/send', debugAuthorizationState(permissions, targetActions, 'messagessend', 'messages:send', sendPassed), sendProbeResponse, sendDetail)}
      </div>`;
    debugAddLog('system', `Target 验证完成：${target}`);
  } finally {
    endAction('debug-target-verify', button);
  }
}

function debugConnect() {
  const panel = document.getElementById('debug-panel');
  if (!panel) return;

  // 从输入框读取要测试的 Key（独立于主管理员的 apiKey）
  const keyInput = document.getElementById('debug-key-input');
  const testKey = keyInput ? keyInput.value.trim() : '';
  if (!testKey) { showToast('请先填入要测试的 API Key', 'error'); return; }

  debugDisconnect();

  const statusEl = document.getElementById('debug-status');
  const connectBtn = document.getElementById('debug-connect-btn');
  const disconnectBtn = document.getElementById('debug-disconnect-btn');
  const logContainer = document.getElementById('debug-log-container');
  if (!statusEl || !connectBtn || !disconnectBtn || !logContainer) return;

  statusEl.textContent = '● 连接中...';
  statusEl.style.color = 'var(--accent)';
  connectBtn.disabled = true;

  try {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = proto + '//' + location.host + '/api/v1/ws';
    debugWs = new WebSocket(url);

    debugWs.onopen = () => {
      debugWs.send(JSON.stringify({ token: testKey }));
    };

    debugWs.onmessage = (e) => {
      try {
        const msg = JSON.parse(e.data);
        if (msg.type === 'auth_ok') {
          statusEl.textContent = '● 已连接';
          statusEl.style.color = 'var(--success)';
          disconnectBtn.disabled = false;
          debugAddLog('system', '认证成功，开始接收事件');
          return;
        }
        if (msg.type === 'auth_failed') {
          statusEl.textContent = '● 认证失败';
          statusEl.style.color = 'var(--danger)';
          debugAddLog('error', '认证失败: API Key 无效');
          debugDisconnect();
          return;
        }
        if (msg.type === 'ping') {
          debugWs.send(JSON.stringify({ type: 'pong' }));
          return;
        }
        // 业务事件
        if (msg.type === 'event') {
          debugAddLog(msg.event, msg.data);
        }
      } catch (err) {
        debugAddLog('error', '解析错误: ' + err.message);
      }
    };

    debugWs.onerror = () => {
      statusEl.textContent = '● 连接错误';
      statusEl.style.color = 'var(--danger)';
      debugAddLog('error', 'WebSocket 连接错误');
      connectBtn.disabled = false;
    };

    debugWs.onclose = () => {
      statusEl.textContent = '● 已断开';
      statusEl.style.color = 'var(--text-muted)';
      connectBtn.disabled = false;
      disconnectBtn.disabled = true;
      debugWs = null;
    };

  } catch (err) {
    statusEl.textContent = '● 创建失败';
    statusEl.style.color = 'var(--danger)';
    connectBtn.disabled = false;
    debugAddLog('error', '创建 WebSocket 失败: ' + err.message);
  }
}

function debugDisconnect() {
  if (debugWs) {
    debugWs.onclose = null;
    debugWs.close();
    debugWs = null;
  }
  const statusEl = document.getElementById('debug-status');
  const connectBtn = document.getElementById('debug-connect-btn');
  const disconnectBtn = document.getElementById('debug-disconnect-btn');
  if (statusEl) { statusEl.textContent = '● 已断开'; statusEl.style.color = 'var(--text-muted)'; }
  if (connectBtn) connectBtn.disabled = false;
  if (disconnectBtn) disconnectBtn.disabled = true;
}

function debugAddLog(type, data) {
  const container = document.getElementById('debug-log-container');
  if (!container) return;

  const time = new Date().toLocaleTimeString();
  let typeColor = 'var(--text-muted)';
  if (type === 'message.inbound' || type === 'message.sent') typeColor = 'var(--success)';
  else if (type === 'message.failed') typeColor = 'var(--danger)';
  else if (type.startsWith('adapter.')) typeColor = 'var(--accent)';
  else if (type === 'system') typeColor = 'var(--accent)';
  else if (type === 'error') typeColor = 'var(--danger)';

  const dataStr = typeof data === 'object' ? JSON.stringify(data) : String(data);

  debugLog.push({ time, type, data: dataStr, typeColor });

  // 限制数量
  if (debugLog.length > MAX_DEBUG_LOG) debugLog.shift();

  // 移除空状态提示
  const emptyMsg = container.querySelector('div[style*="text-align:center"]');
  if (emptyMsg) emptyMsg.remove();

  // 渲染
  debugRenderLog(container);
}

function debugRenderLog(container) {
  const filterText = document.getElementById('debug-filter')?.value?.toLowerCase() || '';
  const filtered = filterText
    ? debugLog.filter(l => l.type.toLowerCase().includes(filterText) || l.data.toLowerCase().includes(filterText))
    : debugLog;

  container.innerHTML = filtered.map(l =>
    `<div class="dbg-log-entry">
      <span class="dbg-time">${l.time}</span>
      <span class="dbg-type" style="color:${l.typeColor}">${l.type}</span>
      <span class="dbg-data">${escapeHtml(l.data)}</span>
    </div>`
  ).join('') || '<div class="dbg-empty">无匹配事件</div>';

  container.scrollTop = container.scrollHeight;
}

function debugFilterLog() {
  const container = document.getElementById('debug-log-container');
  if (container) debugRenderLog(container);
}

function debugClearLog() {
  debugLog = [];
  const container = document.getElementById('debug-log-container');
  if (container) {
    container.innerHTML = '<div class="dbg-empty">日志已清空</div>';
  }
}

// （escapeHtml 见 utils.js）


