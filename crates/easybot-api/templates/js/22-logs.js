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


