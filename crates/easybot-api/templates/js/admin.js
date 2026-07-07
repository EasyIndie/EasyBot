// SECURITY: Use sessionStorage instead of localStorage.
// sessionStorage is cleared when the tab/browser closes, limiting
// exposure of the API key to the current browser session.
const LS_KEY = 'easybot_api_key';
let apiKey = sessionStorage.getItem(LS_KEY) || '';
// key display removed — dev key changes on restart, use API Key tab for permanent keys

function setKey(k) { apiKey = k; sessionStorage.setItem(LS_KEY, k); }
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
  const data = await res.json();
  if (!res.ok) throw new Error(data.error?.message || data.message || res.statusText);
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

// ─── 公共渲染工具 ──────────────────────────────

// 统一消息行渲染
const MSG_TYPE_LABELS = {
  Text: '文本', Image: '图片', Audio: '音频', Video: '视频', File: '文件',
  Sticker: '贴纸', Animation: 'GIF', RichText: '富文本', Interactive: '卡片',
  Share: '分享', Location: '位置', Contact: '名片', Link: '链接',
  System: '系统', Unknown: '其他',
};
function msgTypeLabel(raw_data) {
  const t = raw_data?.msg_type;
  return t ? (MSG_TYPE_LABELS[t] || t) : '文本';
}

// 消息类型 → badge CSS 类名映射
const MSG_TYPE_BADGE = {
  Text: 'badge-type-text', Image: 'badge-type-image', Audio: 'badge-type-audio',
  Video: 'badge-type-video', File: 'badge-type-file', Sticker: 'badge-type-sticker',
  Animation: 'badge-type-anim', RichText: 'badge-type-rich', Interactive: 'badge-type-card',
  Share: 'badge-type-share', Location: 'badge-type-loc', Contact: 'badge-type-contact',
  Link: 'badge-type-link', System: 'badge-type-system', Unknown: 'badge-type-unknown',
};
function msgTypeBadgeClass(raw_data) {
  const t = raw_data?.msg_type;
  return MSG_TYPE_BADGE[t] || 'badge-type-text';
}

function renderMessageRow(m) {
  const tr = document.createElement('tr');
  tr.style.cursor = 'pointer';
  const role = m.role || 'User';
  const typeLabel = msgTypeLabel(m.raw_data);
  tr.innerHTML = `<td style="font-size:11px;color:var(--text-muted);white-space:nowrap">${new Date(m.timestamp).toLocaleTimeString()}</td>
    <td><span class="badge ${platformBadgeClass(m.platform)}">${m.platform}</span></td>
    <td style="font-size:12px">${m.chat_id}</td>
    <td><span class="badge ${msgRoleBadgeClass(role)}">${role}</span></td>
    <td><span class="badge ${msgTypeBadgeClass(m.raw_data)}">${typeLabel}</span></td>
    <td style="font-size:12px;color:var(--text-muted);max-width:300px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap">${(m.text || '').substring(0, 80)}</td>
    <td><button class="btn btn-sm btn-reply" data-platform="${m.platform}" data-chat-id="${m.chat_id}" title="回复该会话">回复</button></td>`;
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

// 统一状态徽章 class 计算（返回修饰类名，配合 "badge" 基类使用）
function statusBadgeClass(status, connected) {
  if (connected) return 'badge-green';
  if (status === 'Failed') return 'badge-red';
  if (status === 'Connecting' || status === 'Starting' || status === 'Disconnecting' || status === 'Stopping') return 'badge-blue';
  if (status === 'Reconnecting') return 'badge-yellow';
  return 'badge-gray';
}

// 平台 → badge class
function platformBadgeClass(p) { return 'badge-platform-' + p; }

// 会话类型 → badge class
function chatTypeBadgeClass(t) { return 'badge-chattype-' + t; }

// 消息角色 → badge class
function msgRoleBadgeClass(r) {
  const map = { 'User': 'badge-role-User', 'Assistant': 'badge-role-Assistant' };
  return map[r] || 'badge-gray';
}

// 统一进度条 HTML（百分比，标签）
function renderProgressBar(percent, label) {
  const c = percent < 60 ? 'fill-green' : percent < 80 ? 'fill-yellow' : 'fill-red';
  return `<div class="progress-bar"><div class="fill ${c}" style="width:${percent}%"></div></div><span style="font-size:13px">${label || percent.toFixed(1) + '%'}</span>`;
}

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
// ESC 关闭 + 点击遮罩关闭
document.addEventListener('keydown', e => { if (e.key === 'Escape') { closeCreateDialog(); closeModal(); } });
document.getElementById('detail-modal').addEventListener('click', e => { if (e.target === e.currentTarget) closeModal(); });


// ─── Overview Tab ──────────────────────────────
let uptimeBase = 0;     // 服务端 uptime（秒），上次刷新时获取
let uptimeRef = 0;      // 本地时间戳（ms），与 uptimeBase 对应

function formatUptime(s) {
  const u = Math.floor(s);
  return u < 60 ? u + 's' : u < 3600 ? Math.floor(u/60) + 'm ' + (u%60) + 's' : Math.floor(u/3600) + 'h ' + Math.floor((u%3600)/60) + 'm';
}

// 每次刷新 stats 时更新基准值
function updateUptimeBase(serverUptime) {
  uptimeBase = serverUptime;
  uptimeRef = Date.now();
  const el = document.getElementById('ov-uptime');
  if (el) el.textContent = formatUptime(serverUptime);
}

// 客户端走秒（1s 更新一次，无 API 请求）
function tickUptime() {
  const el = document.getElementById('ov-uptime');
  if (!el || !uptimeRef) return;
  const now = Date.now();
  const elapsed = (now - uptimeRef) / 1000;
  el.textContent = formatUptime(uptimeBase + elapsed);
}

// 首次加载（带 loading 动画）
async function loadOverview() {
  const loading = document.getElementById('overview-loading');
  const content = document.getElementById('overview-content');
  try {
    loading.style.display = 'block';
    content.style.display = 'none';
    await refreshOverviewStats();
    await refreshSystemInfo();
    loading.style.display = 'none';
    content.style.display = 'block';
    loadMetrics();
  } catch (e) {
    loading.innerHTML = '加载失败: ' + escapeHtml(e.message);
  }
}

// 事件驱动：仅刷新统计（适配器数、会话数等）
async function refreshOverviewStats() {
  try {
    const health = await api('/api/v1/health');
    if (!health) return;
    updateUptimeBase(health.uptime);
    document.getElementById('ov-stats').innerHTML = `
      <div class="stat"><div class="val">${health.version}</div><div class="lbl">版本</div></div>
      <div class="stat"><div class="val" id="ov-uptime">${formatUptime(health.uptime)}</div><div class="lbl">运行时间</div></div>
      <div class="stat"><div class="val">${health.adapters.connected}/${health.adapters.total}</div><div class="lbl">适配器</div></div>
      <div class="stat"><div class="val">${health.sessions.active}</div><div class="lbl">会话</div></div>
    `;
  } catch (e) { /* 静默忽略 */ }
}

// 轮询：系统信息（CPU/内存/磁盘）无事件推送，30s 一次
async function refreshSystemInfo() {
  if (!apiKey) return;
  try {
    const sys = await api('/api/v1/system').catch(() => null);
    if (!sys) return;
    const pct = v => renderProgressBar(v);
    document.getElementById('ov-system').innerHTML = `
      <div class="card"><h3>🖥 OS</h3><p>${sys.os.name} ${sys.os.version}</p><p style="font-size:12px;color:var(--text-muted)">${sys.os.hostname} · ${sys.os.kernel || ''}</p></div>
      <div class="card"><h3>🧠 CPU</h3><p>${sys.cpu.brand} · ${sys.cpu.cores}核</p><p>使用率 ${pct(sys.cpu.usage)}</p>${sys.cpu.load_avg_1 ? `<p style="font-size:12px;color:var(--text-muted)">负载: ${sys.cpu.load_avg_1.toFixed(2)} / ${sys.cpu.load_avg_5.toFixed(2)} / ${sys.cpu.load_avg_15.toFixed(2)}</p>` : '<p style="font-size:12px;color:var(--text-faint)">负载: N/A (Windows)</p>'}</div>
      <div class="card"><h3>💾 内存</h3><p>${sys.memory.used_gb.toFixed(1)} GB / ${sys.memory.total_gb.toFixed(1)} GB</p>${pct(sys.memory.percent)}</div>
      <div class="card"><h3>📀 磁盘</h3><p>${sys.disk.used_gb.toFixed(1)} GB / ${sys.disk.total_gb.toFixed(1)} GB</p>${pct(sys.disk.percent)}</div>
    `;
  } catch (e) { /* 静默忽略 */ }
}
setInterval(() => { if (apiKey && document.getElementById('tab-overview').classList.contains('active')) refreshSystemInfo(); }, 5000);
// 客户端走秒，运行时间实时更新
setInterval(() => { if (document.getElementById('tab-overview').classList.contains('active')) tickUptime(); }, 1000);
// 指标 10s 自动刷新（仅 Overview 激活时）
setInterval(() => { if (apiKey && document.getElementById('tab-metrics').classList.contains('active')) loadMetrics(true); }, 10000);

// ─── Metrics (可视化 + 原始数据切换) ──────────
let metricsRawText = '';
let metricsView = 'visual'; // 'visual' | 'raw'

// 解析 Prometheus text/plain 格式，返回 { metricKey: { name, labels, value } }
function parsePrometheus(text) {
  const out = {};
  const lines = text.split('\n');
  for (const line of lines) {
    if (!line || line.startsWith('#')) continue;
    const m = line.match(/^([a-zA-Z_][a-zA-Z0-9_:]*)(?:\{([^}]*)\})?\s+(-?[0-9]+(?:\.[0-9]+)?(?:e[+-]?[0-9]+)?)/);
    if (!m) continue;
    const name = m[1];
    const labelsStr = m[2] || '';
    const value = parseFloat(m[3]);
    const labels = {};
    if (labelsStr) {
      labelsStr.split(',').forEach(pair => {
        const kv = pair.match(/(\w+)="([^"]*)"/);
        if (kv) labels[kv[1]] = kv[2];
      });
    }
    const key = name + (labelsStr ? '{' + labelsStr + '}' : '');
    out[key] = { name, labels, value };
  }
  return out;
}

function mbar(pct, color) {
  return `<div class="mbar" style="flex:1"><div class="fill mbar-fill-${color}" style="width:${Math.min(pct,100)}%"></div></div>`;
}

function renderMetricsVisual(parsed) {
  const values = Object.values(parsed);
  let httpTotal = 0, wsConn = 0;
  let msgInbound = 0, msgOutbound = 0;
  let adaptersConnected = 0, adaptersTotal = 0;
  const httpByPath = {};
  const msgByPlatform = {};
  const adapterList = [];

  for (const v of values) {
    if (v.name === 'http_requests_total') {
      httpTotal += v.value;
      const key = (v.labels.method||'') + ' ' + (v.labels.path||'');
      const status = v.labels.status || '';
      if (!httpByPath[key]) httpByPath[key] = { method: v.labels.method || '', path: v.labels.path || '', ok: 0, err: 0, total: 0 };
      httpByPath[key].total += v.value;
      if (status.startsWith('2') || status.startsWith('3') || status === '101') httpByPath[key].ok += v.value;
      else httpByPath[key].err += v.value;
    }
    if (v.name === 'active_websocket_connections') wsConn = v.value;
    if (v.name === 'messages_inbound_total') {
      msgInbound += v.value; const p = v.labels.platform||'unknown';
      if (!msgByPlatform[p]) msgByPlatform[p] = { inbound:0, outbound:0 };
      msgByPlatform[p].inbound += v.value;
    }
    if (v.name === 'messages_outbound_total') {
      msgOutbound += v.value; const p = v.labels.platform||'unknown';
      if (!msgByPlatform[p]) msgByPlatform[p] = { inbound:0, outbound:0 };
      msgByPlatform[p].outbound += v.value;
    }
    if (v.name === 'adapter_status') {
      adaptersTotal++;
      if (v.value > 0) adaptersConnected++;
      adapterList.push({ platform: v.labels.platform||'unknown', connected: v.value > 0 });
    }
  }

  document.getElementById('metrics-cards').innerHTML = `
    <div class="stat"><div class="val">${httpTotal.toFixed(0)}</div><div class="lbl">HTTP 请求总量</div></div>
    <div class="stat"><div class="val">${wsConn.toFixed(0)}</div><div class="lbl">WebSocket 连接</div></div>
    <div class="stat"><div class="val" style="font-size:20px">${msgInbound.toFixed(0)}<span style="font-size:12px;color:var(--success)"> ↓</span> ${msgOutbound.toFixed(0)}<span style="font-size:12px;color:var(--accent)"> ↑</span></div><div class="lbl">入站 / 出站消息</div></div>
    <div class="stat"><div class="val">${adaptersConnected}/${adaptersTotal}</div><div class="lbl">适配器在线</div></div>
  `;

  let detail = '';

  // HTTP 明细
  const httpEntries = Object.entries(httpByPath).sort((a,b) => b[1].total - a[1].total);
  if (httpEntries.length) {
    const maxHttp = Math.max(...httpEntries.map(e => e[1].total), 1);
    detail += '<div class="card" style="padding:12px 16px"><h3 style="font-size:14px;margin-bottom:8px">🌐 HTTP 请求明细</h3><div style="font-size:12px">';
    for (const [, h] of httpEntries) {
      const w = (h.total/maxHttp*100).toFixed(0);
      const c = h.err>0 && h.err/h.total>0.1 ? 'red' : 'blue';
      detail += `<div style="display:flex;align-items:center;gap:8px;margin-bottom:4px">
        <span style="width:60px;flex-shrink:0;color:var(--text-muted)">${h.method}</span>
        <span style="flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--text-primary)">${h.path}</span>
        ${mbar(w,c)}<span style="width:50px;text-align:right;font-variant-numeric:tabular-nums">${h.total.toFixed(0)}</span>
        ${h.err>0 ? `<span style="width:40px;text-align:right;color:var(--danger);font-size:11px">${h.err.toFixed(0)} err</span>` : '<span style="width:40px"></span>'}</div>`;
    }
    detail += '</div></div>';
  }

  // 消息按平台
  const msgEntries = Object.entries(msgByPlatform).sort((a,b) => (b[1].inbound+b[1].outbound)-(a[1].inbound+a[1].outbound));
  if (msgEntries.length) {
    const maxMsg = Math.max(...msgEntries.map(e => e[1].inbound+e[1].outbound), 1);
    detail += '<div class="card" style="padding:12px 16px"><h3 style="font-size:14px;margin-bottom:8px">💬 消息按平台统计</h3><div style="font-size:12px">';
    for (const [plat, m] of msgEntries) {
      const t = m.inbound + m.outbound;
      detail += `<div style="display:flex;align-items:center;gap:8px;margin-bottom:6px">
        <span style="width:70px;flex-shrink:0;color:var(--text-primary)">${plat}</span>
        ${mbar((t/maxMsg*100).toFixed(0),'green')}
        <span style="width:70px;text-align:right">${t.toFixed(0)}</span>
        <span style="color:var(--success);font-size:11px">↓${m.inbound.toFixed(0)}</span>
        <span style="color:var(--accent);font-size:11px">↑${m.outbound.toFixed(0)}</span></div>`;
    }
    detail += '</div></div>';
  }

  // 适配器状态
  if (adapterList.length) {
    detail += '<div class="card" style="padding:12px 16px"><h3 style="font-size:14px;margin-bottom:8px">🔌 适配器状态</h3><div style="display:flex;gap:8px;flex-wrap:wrap">';
    for (const a of adapterList) {
      detail += `<span style="display:inline-flex;align-items:center;gap:4px;padding:4px 10px;background:var(--bg-tertiary);border-radius:6px;border:1px solid var(--border-muted);font-size:13px">${a.connected?'🟢':'🔴'} ${a.platform} <span style="color:var(--text-muted);font-size:11px">${a.connected?'在线':'离线'}</span></span>`;
    }
    detail += '</div></div>';
  }

  // 请求平均耗时
  const durCounts = values.filter(v => v.name === 'http_request_duration_seconds_count');
  const durSums = values.filter(v => v.name === 'http_request_duration_seconds_sum');
  if (durCounts.length && durSums.length) {
    const durByPath = {};
    for (const v of durCounts) { const k = (v.labels.method||'')+' '+(v.labels.path||''); if(!durByPath[k])durByPath[k]={count:0,sum:0}; durByPath[k].count = v.value; }
    for (const v of durSums) { const k = (v.labels.method||'')+' '+(v.labels.path||''); if(!durByPath[k])durByPath[k]={count:0,sum:0}; durByPath[k].sum = v.value; }
    const durEntries = Object.entries(durByPath).filter(e=>e[1].count>0).sort((a,b)=>b[1].sum/b[1].count - a[1].sum/a[1].count);
    if (durEntries.length) {
      const maxAvg = Math.max(...durEntries.map(e=>e[1].sum/e[1].count), 0.001);
      detail += '<div class="card" style="padding:12px 16px"><h3 style="font-size:14px;margin-bottom:8px">⏱ 请求平均耗时</h3><div style="font-size:12px">';
      for (const [key, d] of durEntries) {
        const avg = d.sum/d.count, p = key.split(' ');
        const c = avg<0.1?'green':avg<0.5?'yellow':'red';
        detail += `<div style="display:flex;align-items:center;gap:8px;margin-bottom:4px">
          <span style="width:50px;flex-shrink:0;color:var(--text-muted)">${p[0]||''}</span>
          <span style="flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--text-primary)">${p.slice(1).join(' ')||'/'}</span>
          ${mbar((avg/maxAvg*100).toFixed(0),c)}
          <span style="width:60px;text-align:right;font-variant-numeric:tabular-nums">${(avg*1000).toFixed(1)}ms</span></div>`;
      }
      detail += '</div></div>';
    }
  }

  document.getElementById('metrics-detail').innerHTML = detail;
}

async function loadMetrics(isRefresh) {
  const loading = document.getElementById('metrics-loading');
  const contentArea = document.getElementById('metrics-content-area');
  const visual = document.getElementById('metrics-visual');
  const pre = document.getElementById('metrics-content');
  const status = document.getElementById('metrics-status');
  const err = document.getElementById('metrics-error');
  try {
    if (!isRefresh) {
      loading.style.display = 'block';
      contentArea.style.display = 'none';
    }
    err.style.display = 'none';
    status.textContent = isRefresh ? '' : '加载中...';
    const res = await fetch('/api/v1/metrics', { headers: { 'Authorization': `Bearer ${apiKey}` } });
    if (!res.ok) throw new Error(await res.text());
    const text = await res.text();
    metricsRawText = text;
    pre.textContent = text;
    const parsed = parsePrometheus(text);
    renderMetricsVisual(parsed);
    if (!isRefresh) {
      loading.style.display = 'none';
      contentArea.style.display = 'block';
    }
    visual.style.display = metricsView === 'visual' ? 'block' : 'none';
    pre.style.display = metricsView === 'visual' ? 'none' : 'block';
    status.textContent = `共 ${Object.keys(parsed).length} 条指标数据`;
  } catch (e) {
    if (!isRefresh) {
      loading.innerHTML = '加载失败: ' + escapeHtml(e.message);
      visual.style.display = 'none';
      pre.style.display = 'none';
    }
    err.textContent = '加载失败: ' + escapeHtml(e.message);
    err.style.display = 'block';
    status.textContent = '';
  }
}

// 切换可视化 / 原始数据视图
document.getElementById('metrics-toggle-view').addEventListener('click', () => {
  const btn = document.getElementById('metrics-toggle-view');
  const visual = document.getElementById('metrics-visual');
  const pre = document.getElementById('metrics-content');
  if (metricsView === 'visual') {
    metricsView = 'raw';
    btn.textContent = '📊 可视化';
    visual.style.display = 'none';
    pre.style.display = 'block';
  } else {
    metricsView = 'visual';
    btn.textContent = '📋 原始数据';
    visual.style.display = 'block';
    pre.style.display = 'none';
  }
});


// ─── Logs Tab ──────────────────────────────────
let logPollTimer = null;
let logSince = 0;
let logPaused = false;
let logLevel = '';
let logSearchText = '';
let logGeneration = 0;    // 递增计数器，过滤变化时丢弃过期响应

function startLogPolling() {
  if (logPollTimer) return;
  logPollTimer = setInterval(pollLogs, 1000);
  pollLogs();
}
function stopLogPolling() { if (logPollTimer) { clearInterval(logPollTimer); logPollTimer = null; } }

async function pollLogs() {
  if (logPaused) return;
  const gen = ++logGeneration;    // 标记当前请求 Generation，丢弃过期响应
  try {
    const params = new URLSearchParams({ since: logSince, limit: '100' });
    if (logLevel) params.set('level', logLevel);
    if (logSearchText) params.set('search', logSearchText);
    const data = await api('/api/v1/logs?' + params.toString());

    // 如果 generation 已变化（过滤条件在请求期间被修改），丢弃过期响应
    if (gen !== logGeneration) return;

    const list = document.getElementById('log-list');
    const container = document.getElementById('log-container');
    const autoScroll = container.scrollHeight - container.scrollTop - container.clientHeight < 100;
    const frag = document.createDocumentFragment();
    for (const e of data.entries) {
      if (e.timestamp > logSince) logSince = e.timestamp;
      const t = new Date(e.timestamp).toLocaleTimeString();
      const div = document.createElement('div');
      div.className = 'log-entry log-' + e.level;
      div.innerHTML = `<span style="color:var(--text-faint)">${t}</span> [<strong>${escapeHtml(e.level)}</strong>] <span style="color:var(--text-muted)">${escapeHtml(e.target)}</span> ${escapeHtml(e.message)}`;
      frag.appendChild(div);
    }
    list.appendChild(frag);
    // Trim DOM if too many
    while (list.children.length > 2000) list.removeChild(list.firstChild);
    if (autoScroll) container.scrollTop = container.scrollHeight;
  } catch (e) { /* ignore polling errors */ }
}

document.querySelectorAll('#log-level-chips .chip').forEach(c => c.addEventListener('click', () => {
  document.querySelectorAll('#log-level-chips .chip').forEach(x => x.classList.remove('active'));
  c.classList.add('active');
  logLevel = c.dataset.level;
  document.getElementById('log-list').innerHTML = '';
  logSince = 0;
  pollLogs();
}));

document.getElementById('log-search').addEventListener('input', e => {
  logSearchText = e.target.value;
  document.getElementById('log-list').innerHTML = '';
  logSince = 0;
  pollLogs();
});

document.getElementById('log-pause-btn').addEventListener('click', () => {
  logPaused = !logPaused;
  document.getElementById('log-pause-btn').textContent = logPaused ? '▶ 继续' : '⏸ 暂停';
});
document.getElementById('log-clear-btn').addEventListener('click', () => {
  document.getElementById('log-list').innerHTML = '';
  logSince = 0;
});


// ─── Adapters Tab ──────────────────────────────
// 存储每个 adapter 的轮询 timeout ID，防止切换 tab 后继续轮询
let adapterPollTimers = {};

async function loadAdapters() {
  const loading = document.getElementById('adapters-loading');
  const content = document.getElementById('adapters-content');
  try {
    loading.style.display = 'block';
    content.style.display = 'none';
    const data = await api('/api/v1/adapters');
    const icons = { telegram: '✈️', discord: '🎮', feishu: '📘', qq: '🐧', wechat: '💬' };
    content.innerHTML = '<div class="grid-2">' + data.adapters.map(a => {
      // 如果有正在轮询中的状态，优先显示轮询状态
      const pollState = adapterPollTimers[a.platform] ? adapterPollTimers[a.platform].displayState : null;
      const displayStatus = pollState || a.status;
      const statusClass = statusBadgeClass(displayStatus, a.connected);
      const icon = icons[a.platform] || '🔌';
      return `<div class="card" id="adapter-card-${a.platform}">
        <div style="display:flex;justify-content:space-between;align-items:center">
          <h3>${icon} ${a.display_name} <span class="badge ${statusClass}" id="adapter-badge-${a.platform}">${displayStatus}</span></h3>
          <div id="adapter-buttons-${a.platform}">
            <button class="btn btn-sm btn-primary" onclick="adapterAction('${a.platform}','start')" ${a.connected || pollState ? 'disabled':''}>启动</button>
            <button class="btn btn-sm btn-danger" onclick="adapterAction('${a.platform}','stop')" ${!a.connected || pollState ? 'disabled':''}>停止</button>
          </div>
        </div>
      </div>`;
    }).join('') + '</div>';
    loading.style.display = 'none';
    content.style.display = 'block';
  } catch (e) {
    loading.innerHTML = '加载失败: ' + escapeHtml(e.message);
  }
}

// 更新单个 adapter 卡片的 badge 和按钮状态（不重新渲染整个列表）
function updateAdapterCard(platform, status, connected, polling) {
  const badge = document.getElementById(`adapter-badge-${platform}`);
  if (badge) {
    badge.className = `badge ${statusBadgeClass(status, connected)}`;
    badge.textContent = status;
  }
  // 更新按钮状态
  const btnDiv = document.getElementById(`adapter-buttons-${platform}`);
  if (btnDiv) {
    const [startBtn, stopBtn] = btnDiv.querySelectorAll('button');
    if (startBtn) startBtn.disabled = connected || polling;
    if (stopBtn) stopBtn.disabled = !connected || polling;
  }
}

// 等待适配器状态稳定的终止状态（Connected 或 Failed）
// 返回最终的 adapter status string
async function waitForStableStatus(platform, targetConnected, timeoutMs = 15000) {
  const pollInterval = 500;
  const startTime = Date.now();

  while (Date.now() - startTime < timeoutMs) {
    await new Promise(r => setTimeout(r, pollInterval));
    try {
      const resp = await api(`/api/v1/adapters/${platform}/status`);
      if (resp && resp.state) {
        const state = resp.state;
        const connected = resp.connected || false;
        // 终止状态：启动目标为 connected=true，停止目标为 connected=false+非过渡状态
        if (targetConnected && connected) {
          return { status: state, connected: true };
        }
        if (!targetConnected && !connected && !['Connecting', 'Starting', 'Disconnecting', 'Stopping'].includes(state)) {
          return { status: state, connected: false };
        }
        // 失败的终止状态
        if (state === 'Failed') {
          return { status: state, connected: false };
        }
      }
    } catch (_) {
      // 轮询请求可能被中断（切换 tab 等），忽略继续
    }
  }
  // 超时：返回当前状态
  try {
    const resp = await api(`/api/v1/adapters/${platform}/status`);
    return { status: resp?.state || 'Unknown', connected: resp?.connected || false };
  } catch (_) {
    return { status: 'Timeout', connected: false };
  }
}

async function adapterAction(platform, action) {
  const isStart = action === 'start';
  const btnAction = isStart ? '启动' : '停止';
  const pendingLabel = isStart ? '启动中...' : '停止中...';

  // 如果已有轮询在进行，忽略本次点击
  if (adapterPollTimers[platform]) return;

  try {
    // 乐观更新：立即禁用按钮并显示过渡状态
    const pollingState = { displayState: pendingLabel, timer: null };
    adapterPollTimers[platform] = pollingState;

    // 禁用按钮，防止重复点击
    const buttons = document.querySelectorAll(`[onclick*="'${platform}','${action}'"]`);
    buttons.forEach(b => b.disabled = true);
    // 立即更新 badge 为过渡状态
    updateAdapterCard(platform, pendingLabel, false, true);

    // 发起启动/停止请求
    const data = await api('/api/v1/adapters/' + platform + '/' + action, { method: 'POST' });
    if (!data.ok) {
      throw new Error(data.error || `${btnAction}失败`);
    }

    // 轮询等待实际状态稳定（启动 → Connected，停止 → Disconnected/Failed）
    const result = await waitForStableStatus(platform, isStart);

    // 清除轮询状态
    delete adapterPollTimers[platform];

    // 更新卡片显示最终状态
    updateAdapterCard(platform, result.status, result.connected, false);
    showToast(`${platform} ${btnAction}成功`, 'success');

    // 如果 Overview 激活则刷新统计数据（适配器数、会话数）
    { const _oa = document.getElementById('tab-overview')?.classList.contains('active'); if (_oa) refreshOverviewStats(); }

    // 如果不稳定（超时仍没达到目标状态），弹提示但不阻塞
    if ((isStart && !result.connected && result.status !== 'Connected')
        || (!isStart && result.connected)) {
      // 部分成功：后端接受了请求，但状态未完全达到预期
      console.warn(`${platform} ${btnAction} 操作已接受但状态未稳定: ${result.status}`);
    }

  } catch (e) {
    // 清除轮询状态
    delete adapterPollTimers[platform];
    // 重新加载让按钮状态恢复
    loadAdapters();
    showToast(btnAction + '失败: ' + e.message, 'error');
  }
}


// ─── Config Tab ──────────────────────────────
let configData = null;
let configEditMode = false;

async function loadConfig() {
  const loading = document.getElementById('config-loading');
  const view = document.getElementById('config-view');
  try {
    loading.style.display = 'block';
    view.style.display = 'none';
    configData = await api('/api/v1/config');
    view.textContent = JSON.stringify(configData, null, 2);
    loading.style.display = 'none';
    view.style.display = 'block';
    if (!configEditMode) document.getElementById('config-editor').value = JSON.stringify(configData, null, 2);
  } catch (e) {
    loading.innerHTML = '加载失败: ' + escapeHtml(e.message);
  }
}

document.getElementById('config-refresh').addEventListener('click', () => { configEditMode = false; document.getElementById('config-editor').style.display = 'none'; document.getElementById('config-save-btn').style.display = 'none'; document.getElementById('config-cancel-btn').style.display = 'none'; document.getElementById('config-edit-btn').style.display = 'inline-block'; document.getElementById('config-view').style.display = 'block'; loadConfig(); });
document.getElementById('config-edit-btn').addEventListener('click', () => {
  configEditMode = true;
  document.getElementById('config-view').style.display = 'none';
  document.getElementById('config-editor').style.display = 'block';
  document.getElementById('config-edit-btn').style.display = 'none';
  document.getElementById('config-save-btn').style.display = 'inline-block';
  document.getElementById('config-cancel-btn').style.display = 'inline-block';
  document.getElementById('config-editor').value = JSON.stringify(configData, null, 2);
});
document.getElementById('config-cancel-btn').addEventListener('click', () => {
  configEditMode = false;
  document.getElementById('config-editor').style.display = 'none';
  document.getElementById('config-save-btn').style.display = 'none';
  document.getElementById('config-cancel-btn').style.display = 'none';
  document.getElementById('config-edit-btn').style.display = 'inline-block';
  document.getElementById('config-view').style.display = 'block';
});
document.getElementById('config-save-btn').addEventListener('click', async () => {
  const msg = document.getElementById('config-msg');
  const editor = document.getElementById('config-editor');
  const raw = editor.value;
  editor.style.borderColor = '';
  try {
    JSON.parse(raw);
  } catch (e) {
    const posMatch = e.message.match(/position\s+(\d+)/);
    let hint = '';
    if (posMatch) {
      const pos = parseInt(posMatch[1]);
      const before = raw.substring(0, pos);
      const line = (before.match(/\n/g) || []).length + 1;
      const col = pos - before.lastIndexOf('\n');
      hint = ` (第 ${line} 行第 ${col} 列)`;
      editor.style.borderColor = 'var(--danger)';
      editor.focus();
      editor.setSelectionRange(pos, pos);
      editor.scrollTop = editor.scrollHeight * (line / (raw.split('\n').length || 1));
    }
    msg.innerHTML = '<span class="error-msg" style="display:inline-block;background:#471a1a;border:1px solid #f851494d;border-radius:6px;padding:6px 10px">❌ JSON 格式错误' + escapeHtml(hint) + '<br><span style="font-size:11px;color:#f85149cc">' + escapeHtml(e.message) + '</span></span>';
    return;
  }
  editor.style.borderColor = '';
  try {
    await api('/api/v1/config', { method: 'PUT', body: JSON.parse(document.getElementById('config-editor').value) });
    msg.innerHTML = '<span class="success-msg">✅ 配置已更新</span>';
    showToast('配置已更新', 'success');
    configEditMode = false;
    document.getElementById('config-editor').style.display = 'none';
    document.getElementById('config-save-btn').style.display = 'none';
    document.getElementById('config-cancel-btn').style.display = 'none';
    document.getElementById('config-edit-btn').style.display = 'inline-block';
    loadConfig();
  } catch (e) {
    msg.innerHTML = '<span class="error-msg">❌ 保存失败: ' + escapeHtml(e.message) + '</span>';
  }
});


// ─── Sessions Tab ──────────────────────────────

// 构造会话显示名称
// 优先 chat_name（群名/频道名），其次 user_name（发送者昵称），最后根据 chat_type 构造回退
function getDisplayName(s) {
  if (s.source?.chat_name) return s.source.chat_name;
  if (s.source?.user_name) return s.source.user_name;
  const labels = { 'Dm': '用户', 'Group': '群组', 'Channel': '频道', 'Thread': '话题' };
  const label = labels[s.source?.chat_type] || '聊天';
  return label + ' (' + s.chat_id + ')';
}

// 复制 session key 到剪贴板，带视觉反馈
function copyKey(el, key) {
  navigator.clipboard.writeText(key).then(() => {
    const orig = el.textContent;
    el.textContent = '✓ 已复制';
    el.style.color = 'var(--success, #22c55e)';
    setTimeout(() => {
      el.textContent = orig;
      el.style.color = '';
    }, 1200);
  }).catch(() => {});
}

// 渲染单个 session 行（供初始渲染和增量更新复用）
function renderSessionRow(s) {
  const tr = document.createElement('tr');
  tr.setAttribute('data-session-key', s.key);
  tr.innerHTML = `<td>
    <div style="font-weight:600;color:var(--text-primary)">${getDisplayName(s)}</div>
    <div style="margin-top:5px;font-size:11px;color:var(--text-faint);font-family:var(--font-mono);cursor:pointer" onclick="copyKey(this,'${s.key}')" title="点击复制">${s.key}</div>
  </td>
    <td><span class="badge ${platformBadgeClass(s.platform)}">${s.platform}</span></td>
    <td><span class="badge ${chatTypeBadgeClass(s.source?.chat_type)}">${s.source?.chat_type || '-'}</span></td>
    <td style="max-width:260px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:12px;color:var(--text-secondary)" title="${(s.last_message || '').replace(/"/g, '&quot;')}">${s.last_message || '-'}</td>
    <td style="font-size:12px;color:var(--text-muted)">${new Date(s.created_at).toLocaleString()}</td>
    <td><button class="btn btn-sm btn-danger" onclick="deleteSession('${s.key}')">删除</button></td>`;
  return tr;
}

// 渲染完整的 sessions 表格骨架 + 所有行
function renderSessionsTable(sessions) {
  const content = document.getElementById('sessions-content');
  const wrapper = document.createElement('div');
  wrapper.className = 'table-wrapper';
  const table = document.createElement('table');
  table.innerHTML = '<thead><tr><th>名称</th><th>平台</th><th>类型</th><th>最近消息</th><th>创建时间</th><th>操作</th></tr></thead>';
  const tbody = document.createElement('tbody');
  sessions.forEach(s => tbody.appendChild(renderSessionRow(s)));
  table.appendChild(tbody);
  wrapper.appendChild(table);
  content.innerHTML = '';
  content.appendChild(wrapper);
}

// 增量更新 sessions 表格：只增删变化行，保持现有行不动（避免闪烁）
function updateSessionsTable(sessions) {
  const content = document.getElementById('sessions-content');
  const tbody = content.querySelector('tbody');
  if (!tbody) { renderSessionsTable(sessions); return; }

  const newKeys = new Set(sessions.map(s => s.key));
  const existingRows = tbody.querySelectorAll('tr[data-session-key]');
  const existingKeys = new Set();

  // 移除不再存在的行
  existingRows.forEach(row => {
    const key = row.getAttribute('data-session-key');
    if (!newKeys.has(key)) {
      row.remove();
    } else {
      existingKeys.add(key);
    }
  });

  // 添加新行
  sessions.forEach(s => {
    if (!existingKeys.has(s.key)) {
      tbody.appendChild(renderSessionRow(s));
    }
  });
}

async function loadSessions(isRefresh) {
  const loading = document.getElementById('sessions-loading');
  const content = document.getElementById('sessions-content');
  try {
    if (!isRefresh) {
      loading.style.display = 'block';
      content.style.display = 'none';
    }
    const data = await api('/api/v1/sessions');
    if (!data.sessions || !data.sessions.length) {
      content.innerHTML = '<div class="card"><p style="color:var(--text-muted)">暂无活跃会话</p></div>';
    } else if (isRefresh) {
      updateSessionsTable(data.sessions);
    } else {
      renderSessionsTable(data.sessions);
    }
    if (!isRefresh) {
      loading.style.display = 'none';
      content.style.display = 'block';
    }
  } catch (e) {
    if (!isRefresh) {
      loading.innerHTML = '加载失败: ' + escapeHtml(e.message);
    }
  }
}

async function deleteSession(key) {
  if (!confirm('确定删除会话 ' + key + ' ？')) return;
  try {
    await api('/api/v1/sessions/' + encodeURIComponent(key), { method: 'DELETE' });
    // 直接从 DOM 移除对应行，无需全量刷新
    const row = document.querySelector(`tr[data-session-key="${CSS.escape(key)}"]`);
    if (row) row.remove();
    // 如果表格为空，显示空状态
    const tbody = document.querySelector('#sessions-content tbody');
    if (tbody && !tbody.querySelector('tr[data-session-key]')) {
      document.getElementById('sessions-content').innerHTML = '<div class="card"><p style="color:var(--text-muted)">暂无活跃会话</p></div>';
    }
  } catch (e) {
    showToast('删除失败: ' + e.message, 'error');
  }
}


// ─── Messages Tab ──────────────────────────────
let msgCursor = null;
let msgPlatform = '';
// 已加载的消息 ID 集合，防止事件重复追加
const loadedMsgIds = new Set();

// 增量追加（入站消息）：直接从 WebSocket 事件数据渲染，避免与 MessagePersister 缓冲写入竞争
function prependNewMessagesFromEvent(msg) {
  const data = msg.data;
  if (!data || !data.id) return;
  // StoredMessage 的 id 格式为 "inbound:<platform>:<msg.id>"，需匹配 loadedMsgIds 中的格式
  const storedId = 'inbound:' + data.platform + ':' + data.id;
  if (loadedMsgIds.has(storedId)) return;
  loadedMsgIds.add(storedId);
  const tbody = document.getElementById('msg-list');
  const tr = renderMessageRow({
    timestamp: data.timestamp,
    platform: data.platform,
    chat_id: data.chat_id,
    text: data.text,
    role: 'User',
    raw_data: data,
  });
  tbody.insertBefore(tr, tbody.firstChild);
}

// 增量追加（出站消息）：通过 API 获取（出站消息已同步持久化，无竞争条件）
async function prependNewMessages() {
  try {
    const params = new URLSearchParams({ limit: '5' });
    if (msgPlatform) params.set('platform', msgPlatform);
    const data = await api('/api/v1/messages?' + params.toString());
    if (!data.messages?.length) return;
    const tbody = document.getElementById('msg-list');
    // 从后往前遍历（API 返回最前的是最新的），跳过已存在的 ID
    for (let i = data.messages.length - 1; i >= 0; i--) {
      const m = data.messages[i];
      if (loadedMsgIds.has(m.id)) continue;
      loadedMsgIds.add(m.id);
      const tr = renderMessageRow(m);
      tbody.insertBefore(tr, tbody.firstChild);
    }
    // 更新 cursor 为最新的消息时间戳
    if (data.messages.length) msgCursor = data.messages[data.messages.length - 1].timestamp;
  } catch (_) { /* 静默 */ }
}

document.getElementById('msg-send-btn').addEventListener('click', async () => {
  const btn = document.getElementById('msg-send-btn');
  const target = document.getElementById('msg-target').value.trim();
  const text = document.getElementById('msg-text').value.trim();
  const parseMode = document.getElementById('msg-parse-mode').value;
  const result = document.getElementById('msg-send-result');
  if (!target || !text) { result.innerHTML = '<span class="error-msg">请输入 Target 和 Text</span>'; return; }
  // 防连点：禁用按钮并显示加载状态
  btn.disabled = true;
  btn.textContent = '发送中...';
  result.innerHTML = '<span style="color:var(--text-muted)">⏳ 正在发送...</span>';
  try {
    const data = await api('/api/v1/messages/send', { method: 'POST', body: { target, text, parseMode: parseMode || null } });
    result.innerHTML = '<span class="success-msg">✅ 已发送 (id: ' + data.messageId + ', status: ' + data.status + ')</span>';
    showToast('消息已发送', 'success');
    document.getElementById('msg-text').value = '';
    prependNewMessages();
  } catch (e) {
    result.innerHTML = '<span class="error-msg">❌ 发送失败: ' + escapeHtml(e.message) + '</span>';
    showToast('发送失败: ' + e.message, 'error');
  } finally {
    btn.disabled = false;
    btn.textContent = '发送';
  }
});
// Ctrl+Enter to send
document.getElementById('msg-text').addEventListener('keydown', e => { if (e.key === 'Enter' && (e.ctrlKey || e.metaKey)) document.getElementById('msg-send-btn').click(); });

document.getElementById('msg-platform-filter').addEventListener('change', () => {
  msgPlatform = document.getElementById('msg-platform-filter').value;
  msgCursor = null;
  loadedMsgIds.clear();
  loadMessages();
});
document.getElementById('msg-refresh').addEventListener('click', () => { msgCursor = null; loadedMsgIds.clear(); loadMessages(); });
document.getElementById('msg-load-more').addEventListener('click', () => { loadMessages(true); });

async function loadMessages(append = false) {
  const loading = document.getElementById('messages-loading');
  const content = document.getElementById('messages-content');

  // 非追加模式：重置分页游标和去重集合（避免 prependNewMessages 等事件处理器
  // 设置的游标导致初始加载查询旧数据甚至空列表）
  if (!append) {
    msgCursor = null;
    loadedMsgIds.clear();
  }

  // 使用 AbortController 管理请求生命周期，切换标签页时取消未完成请求
  const signal = getTabController('messages');

  try {
    if (!append) { loading.style.display = 'block'; content.style.display = 'none'; }
    const params = new URLSearchParams({ limit: '20' });
    if (msgPlatform) params.set('platform', msgPlatform);
    if (msgCursor) params.set('before', msgCursor);
    const data = await api('/api/v1/messages?' + params.toString(), { signal });
    const tbody = document.getElementById('msg-list');
    if (!append) tbody.innerHTML = '';
    for (const m of data.messages) {
      if (m.id) loadedMsgIds.add(m.id);
      tbody.appendChild(renderMessageRow(m));
    }
    document.getElementById('msg-load-more').style.display = data.has_more ? 'inline-block' : 'none';
    if (data.messages.length) msgCursor = data.messages[data.messages.length - 1].timestamp;
    if (!append) { loading.style.display = 'none'; content.style.display = 'block'; }
  } catch (e) {
    // 忽略 AbortError（标签页切换导致的取消），避免显示错误信息
    if (e.name === 'AbortError') return;
    if (!append) loading.innerHTML = '加载失败: ' + escapeHtml(e.message);
  }
}


// ─── API Key 管理 Tab ──────────────────────────

// 所有可用事件类型（与后端 event_types::all() 一致）
// 按类别分组展示
const EVENT_TYPE_GROUPS = [
  {
    title: "消息事件",
    items: ["message.inbound", "message.sent", "message.failed"],
  },
  {
    title: "适配器事件",
    items: [
      "adapter.connected",
      "adapter.disconnected",
      "adapter.error",
      "adapter.reconnecting",
      "adapter.reconnected",
      "adapter.reconnect_failed",
    ],
  },
  {
    title: "回调事件",
    items: ["callback.received"],
  },
  {
    title: "系统事件",
    items: ["gateway.started", "gateway.stopping", "config.changed"],
  },
];

// 扁平列表，供模板预设和程序逻辑使用
const ALL_EVENT_TYPES = EVENT_TYPE_GROUPS.flatMap(g => g.items);

// 类别颜色映射（用于 checkbox-grid 分组标题圆点）
const EVENT_GROUP_COLORS = {
  "消息事件": "#58a6ff",
  "适配器事件": "#3fb950",
  "回调事件": "#d29922",
  "系统事件": "#bc8cff",
};
const PERM_GROUP_COLORS = {
  "全部权限": "#d29922",
  "消息": "#58a6ff",
  "适配器": "#3fb950",
  "配置": "#bc8cff",
  "会话": "#56d4dd",
  "其他": "#6e7681",
};

// 所有可用权限（与后端 Permission 枚举一致）
const ALL_PERMISSIONS = [
  "*", "messagesread", "messagessend", "adaptersread",
  "adaptersmanage", "configread", "configwrite",
  "sessionsread", "sessionsmanage", "websocketconnect", "apikeysmanage",
];

// 创建模板
const KEY_TEMPLATES = [
  {
    name: "客服机器人",
    icon: "📨",
    desc: "自动回复机器人，只接收用户消息",
    permissions: ["messagessend"],
    event_filters: ["message.inbound"],
  },
  {
    name: "监控告警",
    icon: "🔔",
    desc: "监控连接状态，异常时告警",
    permissions: ["adaptersread"],
    event_filters: ["adapter.disconnected", "adapter.error", "adapter.reconnecting"],
  },
  {
    name: "消息日志",
    icon: "📋",
    desc: "消息发送记录归档",
    permissions: ["messagesread"],
    event_filters: ["message.sent", "message.failed"],
  },
  {
    name: "会话跟踪",
    icon: "👤",
    desc: "追踪完整对话流程",
    permissions: ["messagesread", "messagessend", "sessionsread"],
    event_filters: ["message.inbound", "message.sent", "callback.received"],
  },
  {
    name: "全功能",
    icon: "🚀",
    desc: "管理后台、开发调试（同 dev key）",
    permissions: ["*"],
    event_filters: [],
  },
  {
    name: "自定义",
    icon: "✏️",
    desc: "自由组合需要的事件类型和权限",
    permissions: [],
    event_filters: [],
  },
];

// 侧滑调试面板状态
let debugWs = null;
let debugLog = [];
const MAX_DEBUG_LOG = 200;

async function loadApiKeys() {
  const loading = document.getElementById('apikeys-loading');
  const content = document.getElementById('apikeys-content');
  const isFirstLoad = content.style.display === 'none';
  if (isFirstLoad) {
    loading.style.display = 'block';
    content.style.display = 'none';
  }

  try {
    const keys = await api('/api/v1/api-keys');

    let html = '<div style="display:flex;gap:8px;margin-bottom:12px">';
    html += '<button class="btn btn-primary" id="apikey-create-btn">➕ 创建 API Key</button>';
    html += '</div>';

    if (!keys || !keys.length) {
      html += '<div class="card"><p style="color:var(--text-muted)">暂无 API Key</p></div>';
    } else {
      html += '<div class="table-wrapper"><table><thead><tr>' +
        '<th>名称</th><th>Key</th><th>权限</th><th>事件过滤</th><th>状态</th><th>创建时间</th><th>操作</th>' +
        '</tr></thead><tbody>';
      for (const k of keys.filter(k => k.name !== 'dev')) {
        const masked = k.prefix ? k.prefix + '****' : '****';
        const statusHtml = k.revoked
          ? '<span class="badge badge-red">已吊销</span>'
          : '<span class="badge badge-green">正常</span>';
        const permHtml = k.permissions.includes('*')
          ? '<span class="badge badge-blue">全部</span>'
          : k.permissions.map(p => '<span class="badge badge-gray" style="margin:1px">' + p + '</span>').join('');
        const filterHtml = !k.event_filters || !k.event_filters.length
          ? '<span class="badge badge-blue">全部事件</span>'
          : k.event_filters.map(ef => '<span class="badge badge-gray" style="margin:1px">' + ef + '</span>').join('');
        const created = new Date(k.created_at).toLocaleString();
        const debugBtn = k.revoked
          ? '<button class="btn btn-sm" disabled>调试</button>'
          : `<button class="btn btn-sm" onclick="openDebugPanel('${k.id}','${k.name}','${masked}','${k.event_filters.join(',')}')">🔍 调试</button>`;
        const revokeBtn = k.revoked
          ? `<button class="btn btn-sm btn-danger" onclick="deleteApiKey('${k.id}','${k.name}')">删除</button>`
          : `<button class="btn btn-sm btn-danger" onclick="revokeApiKey('${k.id}','${k.name}')">吊销</button>`;
        html += `<tr>
          <td style="white-space:nowrap"><strong>${k.name}</strong></td>
          <td style="font-family:monospace;font-size:12px">${masked}</td>
          <td style="font-size:12px">${permHtml}</td>
          <td style="font-size:12px">${filterHtml}</td>
          <td>${statusHtml}</td>
          <td style="font-size:12px;color:var(--text-muted)">${created}</td>
          <td style="white-space:nowrap">${debugBtn} ${revokeBtn}</td>
        </tr>`;
      }
      html += '</tbody></table></div>';
    }

    // 调试面板容器（初始隐藏）
    html += '<div class="debug-panel" id="debug-panel" style="display:none"></div>';

    content.innerHTML = html;
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

// ─── 创建对话框（单页布局，模板 + 配置同一页面） ──

function buildEventFilterHtml() {
  let html = '<div class="cg-all"><label><input type="checkbox" id="ef-all" onchange="toggleAllEventFilters()"><span>📡 全部事件</span></label></div>';
  for (const group of EVENT_TYPE_GROUPS) {
    const dotColor = EVENT_GROUP_COLORS[group.title] || '#6e7681';
    let itemsHtml = '';
    for (const et of group.items) {
      itemsHtml += `<label><input type="checkbox" class="ef-item" value="${et}" onchange="onEventFilterChange()"><span>${et}</span></label>`;
    }
    html += `<div class="cg-group"><div class="cg-group-title"><span class="cg-dot" style="background:${dotColor}"></span>${group.title}</div><div class="cg-items">${itemsHtml}</div></div>`;
  }
  return html;
}

function buildPermHtml() {
  const permGroups = [
    { title: '全部权限', items: ['*'] },
    { title: '消息', items: ['messagesread', 'messagessend'] },
    { title: '适配器', items: ['adaptersread', 'adaptersmanage'] },
    { title: '配置', items: ['configread', 'configwrite'] },
    { title: '会话', items: ['sessionsread', 'sessionsmanage'] },
    { title: '其他', items: ['websocketconnect', 'apikeysmanage'] },
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
  const overlay = document.createElement('div');
  overlay.className = 'modal-overlay';
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
          <div class="filter-perm-labels">
            <label>事件类型过滤（不选=全部）</label>
            <label>权限（选 <code>*</code> = 全部）</label>
          </div>
          <div class="filter-perm-row">
            <div id="create-event-filters" class="checkbox-grid">${buildEventFilterHtml()}</div>
            <div id="create-permissions" class="checkbox-grid">${buildPermHtml()}</div>
          </div>
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

  // 应用事件过滤预设
  const efAll = document.getElementById('ef-all');
  if (tpl.event_filters.length === 0) {
    if (efAll) efAll.checked = true;
    document.querySelectorAll('.ef-item').forEach(cb => cb.checked = false);
  } else {
    if (efAll) efAll.checked = false;
    document.querySelectorAll('.ef-item').forEach(cb => {
      cb.checked = tpl.event_filters.includes(cb.value);
    });
  }

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

function toggleAllEventFilters() {
  const allChecked = document.getElementById('ef-all').checked;
  document.querySelectorAll('.ef-item').forEach(cb => cb.checked = false);
}

function onEventFilterChange() {
  const anyChecked = document.querySelectorAll('.ef-item:checked').length > 0;
  document.getElementById('ef-all').checked = !anyChecked;
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

function openDebugWithNewKey() {
  if (lastCreatedKey) {
    sessionStorage.setItem('easybot_debug_key', lastCreatedKey);
  }
  closeCreateDialog();
  // 切换到 API Key tab 并自动打开调试面板
  if (currentTab !== 'apikeys') {
    switchTab('apikeys');
  } else {
    loadApiKeys();
  }
  showToast('Key 已保存，在调试面板中粘贴使用', 'info');
}

let lastCreatedKey = '';

async function submitCreateKey() {
  const name = document.getElementById('create-key-name').value.trim();
  if (!name) {
    document.getElementById('create-key-name').style.borderColor = 'var(--danger)';
    showToast('请输入名称', 'error');
    return;
  }
  document.getElementById('create-key-name').style.borderColor = '';

  const allChecked = document.getElementById('ef-all').checked;
  let event_filters;
  if (allChecked) {
    event_filters = [];
  } else {
    event_filters = [...document.querySelectorAll('.ef-item:checked')].map(cb => cb.value);
  }

  const permissions = [...document.querySelectorAll('.perm-item:checked')].map(cb => cb.value);
  if (!permissions.length) {
    showToast('请至少选择一个权限', 'error');
    return;
  }

  const btn = document.getElementById('create-key-submit');
  btn.disabled = true;
  btn.textContent = '创建中...';

  try {
    const result = await api('/api/v1/api-keys', {
      method: 'POST',
      body: { name, permissions, event_filters },
    });
    lastCreatedKey = result.key || '';
    // 保存到 sessionStorage，供调试面板自动填充
    if (lastCreatedKey) {
      sessionStorage.setItem('easybot_debug_key', lastCreatedKey);
    }
    document.getElementById('create-form-area').style.display = 'none';
    document.getElementById('create-result').style.display = 'block';
    document.getElementById('create-result-key').textContent = lastCreatedKey;
  } catch (e) {
    showToast('创建失败: ' + e.message, 'error');
    btn.disabled = false;
    btn.textContent = '✅ 创建';
  }
}

function copyResultKey() {
  if (!lastCreatedKey) return;
  navigator.clipboard.writeText(lastCreatedKey).catch(() => {});
  showToast('密钥已复制到剪贴板', 'success');
}

function closeCreateDialog() {
  const overlay = document.querySelector('.modal-overlay:not(#detail-modal)');
  if (!overlay) return;
  overlay.remove();
  document.body.style.overflow = '';
  loadApiKeys(); // 退出时自动刷新列表
}

function closeCreateDialogAndRefresh() {
  closeCreateDialog();
}

// ─── 吊销 Key ──────────────────────────────────

async function revokeApiKey(id, name) {
  const isDev = name === 'dev';
  const msg = isDev
    ? `⚠️ 这是主管理 Key（${name}），确认吊销？此操作不可撤销！`
    : `确定吊销 Key [${name}]？此操作不可撤销！`;
  if (!confirm(msg)) return;
  try {
    await api(`/api/v1/api-keys/${id}`, { method: 'DELETE' });
    showToast(`Key [${name}] 已吊销`, 'success');
    loadApiKeys();
  } catch (e) {
    showToast('吊销失败: ' + e.message, 'error');
  }
}

async function deleteApiKey(id, name) {
  if (!confirm(`确定永久删除 Key [${name}]？此操作不可撤销！`)) return;
  try {
    await api(`/api/v1/api-keys/${id}/purge`, { method: 'DELETE' });
    showToast(`Key [${name}] 已永久删除`, 'success');
    loadApiKeys();
  } catch (e) {
    showToast('删除失败: ' + e.message, 'error');
  }
}

// ─── 调试面板 ──────────────────────────────────

function openDebugPanel(id, name, masked, eventFiltersStr) {
  const panel = document.getElementById('debug-panel');
  if (!panel) return;

  const eventFilters = eventFiltersStr ? eventFiltersStr.split(',') : [];

  // 尝试从之前创建 Key 的结果中填充 Key（仅本次会话有效），
  // 如果没有则自动填入当前主页面的 API Key
  const savedTestKey = sessionStorage.getItem('easybot_debug_key') || apiKey || '';

  panel.style.display = 'block';
  panel.innerHTML = `
    <div class="dbg-header">
      <div>
        <h3>🔍 调试: ${name}</h3>
        <div class="dbg-meta">${eventFilters.length ? '事件过滤: ' + eventFilters.join(', ') : '全部事件'}</div>
      </div>
      <button class="modal-close" onclick="closeDebugPanel()">&times;</button>
    </div>
    <div style="padding:8px 16px;border-bottom:1px solid var(--border-muted)">
      <label class="dbg-key-label">输入要测试的 API Key（独立连接，不影响主页面）</label>
      <input type="text" class="dbg-key-input" id="debug-key-input" value="${savedTestKey}" placeholder="粘贴 API Key 进行测试...">
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

  // 重置调试状态
  debugLog = [];
  debugWs = null;
}

function closeDebugPanel() {
  debugDisconnect();
  const panel = document.getElementById('debug-panel');
  if (panel) panel.style.display = 'none';
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
  else if (type === 'system') typeColor = 'var(--primary)';
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

function escapeHtml(str) {
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}


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
  apikeys:   { load: loadApiKeys,         refresh: loadApiKeys,        cleanup: closeDebugPanel },
};

function switchTab(name) {
  // 清理旧标签页
  if (tabRegistry[currentTab]?.cleanup) tabRegistry[currentTab].cleanup();
  // 取消旧标签页的未完成请求
  tabControllers[currentTab]?.abort();
  // 更新 active 状态
  document.querySelectorAll('.tab-btn').forEach(b => {
    const active = b.dataset.tab === name;
    b.classList.toggle('active', active);
    b.setAttribute('aria-selected', String(active));
  });
  document.querySelectorAll('.tab-content').forEach(c => c.classList.toggle('active', c.id === 'tab-' + name));
  sessionStorage.setItem('easybot_admin_tab', name);
  currentTab = name;
  // 移动端：滚动激活标签到可视区
  const activeBtn = document.querySelector('.tab-btn.active');
  if (activeBtn) activeBtn.scrollIntoView({ behavior: 'smooth', inline: 'center', block: 'nearest' });
  // 加载新标签页
  if (tabRegistry[name]?.load) tabRegistry[name].load();
}
document.querySelectorAll('.tab-btn').forEach(b => b.addEventListener('click', () => switchTab(b.dataset.tab)));

// 登录后恢复上次 tab
function restoreTab() {
  const saved = sessionStorage.getItem('easybot_admin_tab');
  if (saved && saved !== 'overview' && saved !== 'metrics') switchTab(saved); else if (saved === 'metrics') switchTab('metrics');
  else loadOverview();
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
  // Adapter 事件 → 刷新 Overview + 直接更新单个卡片（避免全量重渲染闪烁）
  if (t.startsWith('adapter.')) {
    if (currentTab === 'overview') refreshOverviewStats();
    if (currentTab === 'adapters') {
      const platform = msg.data?.platform;
      const statusMap = {
        'adapter.connected': { connected: true, status: 'Connected' },
        'adapter.reconnected': { connected: true, status: 'Connected' },
        'adapter.disconnected': { connected: false, status: 'Disconnected' },
        'adapter.error': { connected: false, status: 'Failed' },
        'adapter.reconnecting': { connected: false, status: 'Reconnecting' },
        'adapter.reconnect_failed': { connected: false, status: 'Failed' },
      };
      const mapped = statusMap[t];
      if (platform && mapped) {
        updateAdapterCard(platform, mapped.status, mapped.connected, false);
      } else {
        tabRegistry.adapters.refresh();
      }
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

document.getElementById('logout-btn').addEventListener('click', () => {
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

