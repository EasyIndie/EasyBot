// ─── Tab 切换 ──────────────────────────────────
// ─── 标签页注册表 ──────────────────────────────
let currentTab = 'overview';

const tabRegistry = {
  overview:  { load: loadOverview,        refresh: () => { refreshOverviewStats(); refreshSystemInfo(); }, cleanup: null },
  metrics:   { load: loadMetrics,         refresh: () => loadMetrics(true), cleanup: null },
  logs:      { load: startLogPolling,     refresh: null,               cleanup: stopLogPolling },
  adapters:  { load: loadAdapters,        refresh: loadAdapters,        cleanup: () => { adapterPollTimers = {}; } },
  config:    { load: loadConfig,          refresh: loadConfig,          cleanup: null },
  sessions:  { load: loadSessions,        refresh: () => loadSessions(true), cleanup: null },
  messages:  { load: loadMessages,        refresh: loadMessages,        cleanup: null },
  plugins:   { load: loadPlugins,         refresh: loadPlugins,        cleanup: null },
  apikeys:   { load: loadApiKeys,         refresh: loadApiKeys,        cleanup: closeDebugPanel },
};

// 从 URL hash 解析 tab 名（如 #/logs → "logs"），非法值返回 null
function tabFromHash() {
  const m = location.hash.match(/^#\/?([a-z]+)$/);
  return m && tabRegistry[m[1]] ? m[1] : null;
}

// fromHash=true 时表示由 hashchange 触发，避免再次写 hash 造成循环
function switchTab(name, fromHash) {
  if (!tabRegistry[name]) name = 'overview';
  // 清理旧标签页
  if (tabRegistry[currentTab]?.cleanup) tabRegistry[currentTab].cleanup();
  // 取消旧标签页的未完成请求
  tabControllers[currentTab]?.abort();
  // 更新 active 状态
  document.querySelectorAll('.tab-btn').forEach(b => {
    const active = b.dataset.tab === name;
    b.classList.toggle('active', active);
    b.setAttribute('aria-selected', String(active));
    b.tabIndex = active ? 0 : -1;   // roving tabindex（ARIA tabs 约定）
  });
  document.querySelectorAll('.tab-content').forEach(c => c.classList.toggle('active', c.id === 'tab-' + name));
  sessionStorage.setItem('easybot_admin_tab', name);
  currentTab = name;
  // 同步 URL hash（深链，可刷新/收藏/前进后退）
  if (!fromHash && tabFromHash() !== name) location.hash = '#/' + name;
  // 移动端：滚动激活标签到可视区
  const activeBtn = document.querySelector('.tab-btn.active');
  if (activeBtn) activeBtn.scrollIntoView({ behavior: 'smooth', inline: 'center', block: 'nearest' });
  // 加载新标签页
  if (tabRegistry[name]?.load) tabRegistry[name].load();
}
document.querySelectorAll('.tab-btn').forEach(b => b.addEventListener('click', () => switchTab(b.dataset.tab)));

// 浏览器前进/后退或手动改 hash 时同步
window.addEventListener('hashchange', () => {
  const name = tabFromHash();
  if (name && name !== currentTab) switchTab(name, true);
});

// 登录后恢复 tab：优先 URL hash，其次 sessionStorage，最后 overview
function restoreTab() {
  const fromHash = tabFromHash();
  const saved = sessionStorage.getItem('easybot_admin_tab');
  switchTab(fromHash || (saved && tabRegistry[saved] ? saved : 'overview'));
}

// 键盘导航：← → 方向键切换标签页
document.getElementById('tabs-bar').addEventListener('keydown', (e) => {
  if (e.key === 'ArrowLeft' || e.key === 'ArrowRight') {
    e.preventDefault();
    const tabs = [...document.querySelectorAll('.tab-btn')];
    const idx = tabs.findIndex(b => b.classList.contains('active'));
    const next = e.key === 'ArrowRight' ? (idx + 1) % tabs.length : (idx - 1 + tabs.length) % tabs.length;
    tabs[next]?.focus();
    switchTab(tabs[next]?.dataset.tab);
  }
});
// ─── WebSocket 事件驱动 ────────────────────────
let ws = null;
let wsReconnectTimer = null;
let wsReconnectDelay = 1; // 指数退避起始秒数

function wsStatus(color, label) {
  let el = document.getElementById('ws-status');
  if (!el) {
    el = document.createElement('span');
    el.id = 'ws-status';
    el.title = 'WebSocket 状态';
    document.querySelector('.header .right')?.prepend(el);
  }
  el.style.cssText = `display:inline-flex;align-items:center;gap:4px;font-size:11px;color:${color};margin-right:8px`;
  el.innerHTML = `<span style="width:8px;height:8px;border-radius:50%;background:${color};display:inline-block"></span>${label}`;
}

function connectWebSocket() {
  disconnectWebSocket();
  if (!apiKey) { console.log('[WS] No API key, skipping'); return; }
  wsStatus('var(--text-muted)', 'connecting');
  try {
    const proto = location.protocol === 'https:' ? 'wss:' : 'ws:';
    const url = proto + '//' + location.host + '/api/v1/ws';
    console.log('[WS] Connecting to', url);
    ws = new WebSocket(url);
    ws.onopen = () => {
      console.log('[WS] Connected, sending auth');
      ws.send(JSON.stringify({ token: apiKey }));
    };
    ws.onmessage = (e) => {
      try {
        const msg = JSON.parse(e.data);
        if (msg.type === 'auth_ok') {
          console.log('[WS] Authenticated successfully');
          wsReconnectDelay = 1; // 连接成功时重置指数退避
          wsStatus('var(--success)', 'connected');
          return;
        }
        if (msg.type === 'auth_failed') {
          console.log('[WS] Auth failed — key invalid');
          showLogin();
          return;
        }
        if (msg.type === 'ping') {
          ws.send(JSON.stringify({ type: 'pong' }));
          return;
        }
        if (msg.type !== 'event') {
          console.log('[WS] Non-event msg:', msg.type);
          return;
        }
        console.log('[WS] Event received:', msg.event, msg.data);
        handleGatewayEvent(msg);
      } catch (err) {
        console.error('[WS] Parse/handle error:', err, e.data);
      }
    };
    ws.onerror = (err) => {
      console.error('[WS] Connection error', err);
      wsStatus('var(--danger)', 'error');
    };
    ws.onclose = (ev) => {
      console.log('[WS] Closed code=' + ev.code + ' reason=' + ev.reason);
      wsStatus('var(--text-muted)', 'disconnected');
      if (apiKey) {
        const delay = wsReconnectDelay * 1000;
        console.log('[WS] Reconnecting in ' + wsReconnectDelay + 's...');
        wsReconnectDelay = Math.min(wsReconnectDelay * 2, 30);
        wsReconnectTimer = setTimeout(connectWebSocket, delay);
      }
    };
  } catch (err) {
    console.error('[WS] Creation failed:', err);
    wsStatus('var(--danger)', 'error');
  }
}

function disconnectWebSocket() {
  if (wsReconnectTimer) { clearTimeout(wsReconnectTimer); wsReconnectTimer = null; }
  if (ws) { ws.onclose = null; ws.close(); ws = null; }
  wsReconnectDelay = 1; // 重置指数退避
  console.log('[WS] Disconnected');
  wsStatus('var(--text-muted)', 'disconnected');
}

function handleGatewayEvent(msg) {
  const t = msg.event || '';
  console.log('[EVENT]', t, {currentTab});
  // Adapter 事件 → 刷新 Overview + 重新拉取列表。
  // 事件 payload 本身只携带 platform/health，不携带永久/瞬时分类与重试进度；
  // 重新拉取列表可让卡片正确区分"自动重连中"与"已永久停用"并显示重试信息。
  if (t.startsWith('adapter.')) {
    if (currentTab === 'overview') refreshOverviewStats();
    if (currentTab === 'adapters') {
      tabRegistry.adapters.refresh();
    }
  }
  // 入站消息事件 → 直接渲染（避免与 MessagePersister 缓冲写入竞争）
  if (t === 'message.inbound') {
    if (currentTab === 'overview') refreshOverviewStats();
    if (currentTab === 'sessions') tabRegistry.sessions.refresh();
    if (currentTab === 'messages') prependNewMessagesFromEvent(msg);
  }
  // 出站/失败/回调事件 → 通过 API 获取（已同步持久化，无竞争条件）
  if (t === 'message.sent' || t === 'message.failed' || t === 'callback.received') {
    if (currentTab === 'overview') refreshOverviewStats();
    if (currentTab === 'sessions') tabRegistry.sessions.refresh();
    if (currentTab === 'messages') prependNewMessages();
  }
  // 配置变更 / Gateway 事件 → 刷新对应标签页
  if (t === 'config.changed' && currentTab === 'config') tabRegistry.config.refresh();
  if ((t === 'gateway.started' || t === 'gateway.stopping') && currentTab === 'overview') refreshOverviewStats();
}


// ─── 登录 ──────────────────────────────────────
function initAuth() {
  if (apiKey) {
    // 验证已有 key
    api('/api/v1/adapters').then(() => {
      document.getElementById('login-overlay').style.display = 'none';
      document.getElementById('logout-btn').style.display = 'block';
      restoreTab();
      connectWebSocket();
    }).catch(() => {
      showLogin();
    });
  } else {
    showLogin();
  }
}

document.getElementById('login-form').addEventListener('submit', async (e) => {
  e.preventDefault();
  const password = document.getElementById('login-password').value;
  if (!password) return;
  const btn = document.getElementById('login-btn');
  const err = document.getElementById('login-error');
  err.style.display = 'none';
  err.className = 'login-error-msg';
  btn.disabled = true;
  btn.textContent = '登录中...';
  try {
    const res = await fetch('/admin/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ password }),
    });
    const data = await res.json();
    if (!res.ok) throw new Error(data.error?.message || data.message || '登录失败');
    setKey(data.key);
    document.getElementById('login-overlay').style.display = 'none';
    document.getElementById('logout-btn').style.display = 'block';
    restoreTab();
    connectWebSocket();
  } catch (e) {
    clearKey();
    err.textContent = '登录失败：' + e.message;
    err.style.display = 'block';
    err.classList.add('shake');
    setTimeout(() => err.classList.remove('shake'), 400);
    btn.disabled = false;
    btn.textContent = '登录';
  }
});

document.getElementById('logout-btn').addEventListener('click', async () => {
  try { await api('/api/v1/admin/logout', { method: 'POST' }); } catch (_) { /* local logout still proceeds */ }
  clearKey();
  // Reset tab contents
  document.querySelectorAll('#ov-stats, #adapters-content, #sessions-content').forEach(e => e.innerHTML = '');
  showLogin();
});


// ─── Error monitoring ─────────────────────────
window.onerror = (msg, url, line, col, err) => {
  console.error('[Frontend Error]', msg, `at ${url}:${line}:${col}`, err?.stack || '');
};
window.addEventListener('unhandledrejection', e => {
  console.error('[Unhandled Promise]', e.reason?.message || e.reason, e.reason?.stack || '');
});

// ─── Initialize ────────────────────────────────
document.getElementById('metrics-refresh').addEventListener('click', () => loadMetrics(true));
initAuth();
