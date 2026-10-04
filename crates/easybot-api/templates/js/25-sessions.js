// ─── Sessions Tab ──────────────────────────────

// 构造会话显示名称
// 优先用户自定义名，其次 chat_name（群名/频道名）、user_name（发送者昵称），最后根据 chat_type 构造回退
function getDisplayName(s) {
  if (s.custom_name) return s.custom_name;
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
  const key = escapeHtml(String(s.key || ''));
  tr.innerHTML = `<td>
    <div style="font-weight:600;color:var(--text-primary)">${escapeHtml(String(getDisplayName(s) || ''))}</div>
    <div class="session-key-copy" style="margin-top:5px;font-size:11px;color:var(--text-faint);font-family:var(--font-mono);cursor:pointer" title="点击复制">${key}</div>
  </td>
    <td><span class="badge ${platformBadgeClass(s.platform)}">${escapeHtml(String(s.platform || ''))}</span></td>
    <td><span class="badge ${chatTypeBadgeClass(s.source?.chat_type)}">${escapeHtml(String(s.source?.chat_type || '-'))}</span></td>
    <td style="max-width:260px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;font-size:12px;color:var(--text-secondary)" title="${escapeHtml(String(s.last_message || ''))}">${escapeHtml(String(s.last_message || '-'))}</td>
    <td style="font-size:12px;color:var(--text-muted)">${new Date(s.created_at).toLocaleString()}</td>
    <td>
      <button class="btn btn-sm session-rename">改名</button>
      <button class="btn btn-sm btn-danger session-delete">删除</button>
    </td>`;
  tr.querySelector('.session-key-copy').addEventListener('click', event => copyKey(event.currentTarget, s.key));
  tr.querySelector('.session-rename').addEventListener('click', event => renameSession(s, event.currentTarget));
  tr.querySelector('.session-delete').addEventListener('click', event => deleteSession(s.key, event.currentTarget));
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

async function deleteSession(key, button) {
  const actionKey = `session-delete:${key}`;
  if (!beginAction(actionKey, button, '删除中...')) return;
  if (!(await confirmDialog('确定删除会话 ' + key + ' ？'))) {
    endAction(actionKey, button);
    return;
  }
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
  } finally {
    endAction(actionKey, button);
  }
}

// 会话改名：非空自定义名覆盖展示；清空则撤销自定义名，回退自动推导链
async function renameSession(session, button) {
  const key = session.key;
  const actionKey = `session-rename:${key}`;
  if (!beginAction(actionKey, button, '保存中...')) return;
  const input = await promptDialog('会话改名', session.custom_name || '', { message: '留空并确定 = 恢复自动名称' });
  if (input === null) { endAction(actionKey, button); return; }
  const trimmed = input.trim();
  try {
    const data = await api('/api/v1/sessions/' + encodeURIComponent(key), {
      method: 'PUT',
      body: { custom_name: trimmed }
    });
    // 用返回的完整会话重建该行（自动名回退/自定义名都正确显示）
    const row = document.querySelector(`tr[data-session-key="${CSS.escape(key)}"]`);
    if (row && data.session) {
      row.replaceWith(renderSessionRow(data.session));
    }
    showToast(trimmed ? '已更新会话名称' : '已恢复自动名称', 'success');
  } catch (e) {
    showToast('改名失败: ' + e.message, 'error');
  } finally {
    endAction(actionKey, button);
  }
}


