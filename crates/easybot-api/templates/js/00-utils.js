// EasyBot 前端纯函数工具集（无顶层 DOM 访问，可被 vitest 单独加载测试）
// 由 build.rs 在 admin.js 之前内联到 gen/admin.html；function 声明会被提升，故 admin.js 可直接调用。

// ─── 消息类型标签 ──────────────────────────────
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

// ─── 状态 / 徽章 class 计算 ────────────────────
// 统一状态徽章 class 计算（返回修饰类名，配合 "badge" 基类使用）
// permanent: 是否永久停用（凭据拒绝等）。用于区分 "Failed" 的两种语义：
//   永久 → 红色（已停用，需人工介入）；瞬时 → 黄色（自动重连中，会自动恢复）
function statusBadgeClass(status, connected, health, permanent) {
  if (connected) {
    // 适配器已连接但传输层不健康 → 警告色
    if (health === 'Degraded' || health === 'Down') return 'badge-yellow';
    return 'badge-green';
  }
  if (status === 'Failed') return permanent ? 'badge-red' : 'badge-yellow';
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

// ─── 文本 / 格式化工具 ─────────────────────────
// 截断长文本（用于失败原因副标题）
function truncateText(s, max) {
  return s.length > max ? s.slice(0, max - 1) + '…' : s;
}

// 毫秒 → 人类可读时长（如 "1m30s"）
function fmtDuration(ms) {
  const s = Math.max(1, Math.round(ms / 1000));
  if (s < 60) return s + 's';
  const m = Math.floor(s / 60);
  return m + 'm' + (s % 60 ? `${s % 60}s` : '');
}

// 生成适配器卡片副标题的"重试/失败原因"文本（无信息时返回 ''）
function adapterRetryText(a) {
  const permanent = a.permanent_failure || false;
  const parts = [];
  if (a.status === 'Failed') {
    if (permanent) {
      parts.push('已停用（不再自动重试）');
    } else {
      const label = a.retry_attempt > 0 ? `自动重连中 (第 ${a.retry_attempt} 次)` : '自动重连中';
      parts.push(label);
    }
  }
  if (a.last_error) {
    parts.push(truncateText(String(a.last_error), 60));
  }
  return parts.join(' · ');
}

// 统一进度条 HTML（百分比，标签）
function renderProgressBar(percent, label) {
  const c = percent < 60 ? 'fill-green' : percent < 80 ? 'fill-yellow' : 'fill-red';
  return `<div class="progress-bar"><div class="fill ${c}" style="width:${percent}%"></div></div><span style="font-size:13px">${label || percent.toFixed(1) + '%'}</span>`;
}

// ─── Prometheus 指标工具 ───────────────────────
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

// ─── HTML 转义 ─────────────────────────────────
function escapeHtml(str) {
  const div = document.createElement('div');
  div.textContent = str;
  return div.innerHTML;
}
