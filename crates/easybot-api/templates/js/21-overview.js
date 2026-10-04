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
// 适配器卡片"下次重试"倒计时走秒（仅 Adapters 激活时）
setInterval(() => { if (document.getElementById('tab-adapters').classList.contains('active')) tickAdapterCountdown(); }, 1000);

// ─── Metrics (可视化 + 原始数据切换) ──────────
let metricsRawText = '';
let metricsView = 'visual'; // 'visual' | 'raw'

// （parsePrometheus / mbar 见 utils.js）

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
        <span style="width:60px;flex-shrink:0;color:var(--text-muted)">${escapeHtml(h.method)}</span>
        <span style="flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--text-primary)">${escapeHtml(h.path)}</span>
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
        <span style="width:70px;flex-shrink:0;color:var(--text-primary)">${escapeHtml(plat)}</span>
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
      detail += `<span style="display:inline-flex;align-items:center;gap:4px;padding:4px 10px;background:var(--bg-tertiary);border-radius:6px;border:1px solid var(--border-muted);font-size:13px">${a.connected?'🟢':'🔴'} ${escapeHtml(String(a.platform || ''))} <span style="color:var(--text-muted);font-size:11px">${a.connected?'在线':'离线'}</span></span>`;
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
          <span style="width:50px;flex-shrink:0;color:var(--text-muted)">${escapeHtml(p[0]||'')}</span>
          <span style="flex:1;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;color:var(--text-primary)">${escapeHtml(p.slice(1).join(' ')||'/')}</span>
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


