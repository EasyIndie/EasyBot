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
  const saveButton = document.getElementById('config-save-btn');
  if (!beginAction('config-save', saveButton, '保存中...')) return;
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
    endAction('config-save', saveButton);
    return;
  }
  editor.style.borderColor = '';
  try {
    await api('/api/v1/config', { method: 'PUT', body: JSON.parse(document.getElementById('config-editor').value) });
    msg.innerHTML = '<span class="success-msg">✅ 配置已更新（重启后生效）</span>';
    showToast('配置已更新（重启后生效）', 'success');
    configEditMode = false;
    document.getElementById('config-editor').style.display = 'none';
    document.getElementById('config-save-btn').style.display = 'none';
    document.getElementById('config-cancel-btn').style.display = 'none';
    document.getElementById('config-edit-btn').style.display = 'inline-block';
    loadConfig();
  } catch (e) {
    if (e.status === 409) {
      // 后端设计如此：运行时不可热更新配置，需审阅后重启。这不是失败。
      msg.innerHTML = '<span class="success-msg">ℹ️ 运行时不可热更新配置：请将上述内容写入 <code>gateway.yaml</code> / <code>gateway.local.yaml</code>，重启 EasyBot 后生效。</span>';
      showToast('配置变更需审阅后重启生效', 'info');
    } else {
      msg.innerHTML = '<span class="error-msg">❌ 保存失败: ' + escapeHtml(e.message) + '</span>';
    }
  } finally {
    endAction('config-save', saveButton);
  }
});


