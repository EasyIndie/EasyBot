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
  if (!beginAction('message-send', btn, '发送中...')) return;
  result.innerHTML = '<span style="color:var(--text-muted)">⏳ 正在发送...</span>';
  try {
    const data = await api('/api/v1/messages/send', { method: 'POST', body: { target, text, parseMode: parseMode || null } });
    result.innerHTML = '<span class="success-msg">✅ 已发送 (id: ' + escapeHtml(String(data.messageId || '')) + ', status: ' + escapeHtml(String(data.status || '')) + ')</span>';
    showToast('消息已发送', 'success');
    document.getElementById('msg-text').value = '';
    prependNewMessages();
  } catch (e) {
    result.innerHTML = '<span class="error-msg">❌ 发送失败: ' + escapeHtml(e.message) + '</span>';
    showToast('发送失败: ' + e.message, 'error');
  } finally {
    endAction('message-send', btn);
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


