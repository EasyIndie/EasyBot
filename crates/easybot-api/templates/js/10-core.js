const LS_KEY = 'easybot_api_key';
// Privileged credentials remain in memory only. Reload requires a new login.
let apiKey = '';
// Admin authentication uses a short-lived, memory-only session returned by password login.

function setKey(k) { apiKey = k; }
function clearKey() { apiKey = ''; sessionStorage.removeItem(LS_KEY); }


// ─── API 请求包装 ──────────────────────────────
function showLogin() {
  // 已显示登录框时不再重复重置（防止轮询 401 打断输入）
  const overlay = document.getElementById('login-overlay');
  if (overlay.style.display === 'flex') return;
  clearKey();
  sessionStorage.removeItem('easybot_admin_tab');
  disconnectWebSocket();
  stopLogPolling();
  overlay.style.display = 'flex';
  document.getElementById('logout-btn').style.display = 'none';
  document.getElementById('login-password').value = '';
  document.getElementById('login-error').style.display = 'none';
}

async function api(path, opts = {}) {
  const { method = 'GET', body, signal } = opts;
  const headers = { 'Authorization': `Bearer ${apiKey}` };
  if (body) headers['Content-Type'] = 'application/json';
  const res = await fetch(path, { method, headers, body: body ? JSON.stringify(body) : undefined, signal });
  if (res.status === 401 && !path.includes('/admin/login')) {
    showLogin();
    throw new Error('未授权，请重新登录');
  }
  const text = await res.text();
  let data = null;
  try { data = text ? JSON.parse(text) : null; } catch (_) { data = { message: text }; }
  if (!res.ok) {
    const err = new Error(data?.error?.message || data?.message || res.statusText);
    err.status = res.status;
    throw err;
  }
  return data;
}

// AbortController 管理：切换标签页时取消未完成的请求
const tabControllers = {};
function getTabController(name) {
  tabControllers[name]?.abort();
  tabControllers[name] = new AbortController();
  return tabControllers[name].signal;
}

// 简单请求缓存（TTL 毫秒）
const requestCache = new Map();
function cachedApi(path, opts = {}, ttlMs = 30000) {
  const key = path + JSON.stringify(opts);
  const now = Date.now();
  const cached = requestCache.get(key);
  if (cached && now - cached.time < ttlMs) return Promise.resolve(cached.data);
  const promise = api(path, opts).then(data => {
    requestCache.set(key, { data, time: now });
    return data;
  });
  return promise;
}

// 统一防重复提交与动态弹窗管理。
// 读操作可以并发，写操作和弹窗创建必须按业务键串行。
const pendingActions = new Set();
const openingModals = new Set();

function beginAction(key, button, busyText) {
  if (pendingActions.has(key)) return false;
  pendingActions.add(key);
  if (button) {
    button.disabled = true;
    if (busyText) {
      button.dataset.actionOriginalText = button.textContent;
      button.textContent = busyText;
    }
  }
  return true;
}

function endAction(key, button) {
  pendingActions.delete(key);
  if (button) {
    button.disabled = false;
    if (button.dataset.actionOriginalText) {
      button.textContent = button.dataset.actionOriginalText;
      delete button.dataset.actionOriginalText;
    }
  }
}

function beginModal(key) {
  if (openingModals.has(key) || document.querySelector(`[data-modal-key="${CSS.escape(key)}"]`)) return false;
  openingModals.add(key);
  return true;
}

function finishModal(key) {
  openingModals.delete(key);
}

// ─── 公共渲染工具 ──────────────────────────────

// 统一消息行渲染（纯函数 msgTypeLabel / *BadgeClass / escapeHtml 见 utils.js）

function renderMessageRow(m) {
  const tr = document.createElement('tr');
  tr.style.cursor = 'pointer';
  const role = m.role || 'User';
  const typeLabel = msgTypeLabel(m.raw_data);
  tr.innerHTML = `<td style="font-size:11px;color:var(--text-muted);white-space:nowrap">${new Date(m.timestamp).toLocaleTimeString()}</td>
    <td><span class="badge ${platformBadgeClass(m.platform)}">${escapeHtml(String(m.platform || ''))}</span></td>
    <td style="font-size:12px">${escapeHtml(String(m.chat_id || ''))}</td>
    <td><span class="badge ${msgRoleBadgeClass(role)}">${escapeHtml(String(role))}</span></td>
    <td><span class="badge ${msgTypeBadgeClass(m.raw_data)}">${escapeHtml(String(typeLabel))}</span></td>
    <td style="font-size:12px;color:var(--text-muted);max-width:300px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${escapeHtml(String(m.text || '').substring(0, 80))}</td>
    <td><button class="btn btn-sm btn-reply" title="回复该会话">回复</button></td>`;
  tr.querySelector('.btn-reply').addEventListener('click', e => {
    e.stopPropagation();
    const target = m.platform + ':' + m.chat_id;
    document.getElementById('msg-target').value = target;
    const textarea = document.getElementById('msg-text');
    textarea.focus();
    textarea.scrollIntoView({ behavior: 'smooth', block: 'center' });
    showToast('已填充回复目标: ' + target, 'info');
  });
  tr.addEventListener('click', () => showDetailModal('消息详情', m));
  return tr;
}

// （状态徽章 / 文本格式化纯函数见 utils.js）

// 每秒更新适配器卡片的"下次重试倒计时"（基于渲染时记录的 data-retry-until 时间戳）
function tickAdapterCountdown() {
  document.querySelectorAll('[data-adapter-countdown]').forEach(el => {
    const until = Number(el.dataset.retryUntil || 0);
    if (!until) return;
    const remain = until - Date.now();
    if (remain <= 0) {
      el.textContent = '即将重试…';
      return;
    }
    el.textContent = `下次 ${fmtDuration(remain)} 后重试`;
  });
}

// （平台/会话类型/角色徽章 class、进度条 HTML 见 utils.js）

// ─── Toast 通知 ──────────────────────────────
function showToast(message, type = 'info') {
  const container = document.getElementById('toast-container');
  const toast = document.createElement('div');
  toast.className = `toast toast-${type}`;
  toast.textContent = message;
  container.appendChild(toast);
  setTimeout(() => {
    toast.classList.add('removing');
    toast.addEventListener('animationend', () => toast.remove());
  }, 3000);
}

// ─── Modal 详情弹窗 ───────────────────────────
function showDetailModal(title, data) {
  document.getElementById('modal-title').textContent = title;
  document.getElementById('modal-body').textContent = typeof data === 'string' ? data : JSON.stringify(data, null, 2);
  document.getElementById('detail-modal').style.display = 'flex';
  document.body.style.overflow = 'hidden';
}
function closeModal() {
  document.getElementById('detail-modal').style.display = 'none';
  document.body.style.overflow = '';
}

// ─── 通用确认 / 输入对话框（替代原生 confirm/prompt，可样式化可测试）───
// 返回值：confirm 模式 → Promise<boolean>；prompt 模式 → Promise<string|null>（取消为 null）
function openDialog({ title, message, inputValue, confirmText = '确定', danger = false }) {
  return new Promise(resolve => {
    const overlay = document.createElement('div');
    overlay.className = 'modal-overlay';
    overlay.dataset.modalKey = 'dialog';
    overlay.style.display = 'flex';
    const isPrompt = inputValue !== undefined;
    const inputHtml = isPrompt
      ? `<input type="text" id="dialog-input" value="${escapeHtml(inputValue)}" style="width:100%;margin-top:12px">`
      : '';
    overlay.innerHTML = `
      <div class="modal-card" style="max-width:440px">
        <div class="modal-header"><h3>${escapeHtml(title)}</h3></div>
        <div style="padding:16px 20px">
          <div style="font-size:13px;line-height:1.7;color:var(--text-secondary);white-space:pre-wrap">${escapeHtml(message || '')}</div>
          ${inputHtml}
        </div>
        <div style="display:flex;justify-content:flex-end;gap:8px;padding:12px 20px;border-top:1px solid var(--border-muted)">
          <button class="btn" data-dialog="cancel">取消</button>
          <button class="btn ${danger ? 'btn-danger' : 'btn-primary'}" data-dialog="ok">${escapeHtml(confirmText)}</button>
        </div>
      </div>`;
    document.body.appendChild(overlay);
    const input = overlay.querySelector('#dialog-input');
    const okBtn = overlay.querySelector('[data-dialog="ok"]');
    const finish = value => {
      overlay.remove();
      document.removeEventListener('keydown', onKey);
      resolve(value);
    };
    const ok = () => finish(isPrompt ? (input ? input.value : '') : true);
    const cancel = () => finish(isPrompt ? null : false);
    const onKey = e => {
      if (e.key === 'Escape') cancel();
      else if (e.key === 'Enter' && document.activeElement !== overlay.querySelector('[data-dialog="cancel"]')) ok();
    };
    overlay.querySelector('[data-dialog="cancel"]').onclick = cancel;
    okBtn.onclick = ok;
    overlay.addEventListener('click', e => { if (e.target === overlay) cancel(); });
    document.addEventListener('keydown', onKey);
    (input || okBtn).focus();
    if (input) input.select();
  });
}

function confirmDialog(message, opts = {}) {
  return openDialog({ title: opts.title || '确认操作', message, confirmText: opts.confirmText, danger: opts.danger });
}

function promptDialog(title, initialValue = '', opts = {}) {
  return openDialog({ title, message: opts.message || '', inputValue: initialValue, confirmText: opts.confirmText });
}

// ESC 关闭 + 点击遮罩关闭
document.addEventListener('keydown', e => { if (e.key === 'Escape') { closeCreateDialog(); closeModal(); } });
document.getElementById('detail-modal').addEventListener('click', e => { if (e.target === e.currentTarget) closeModal(); });


