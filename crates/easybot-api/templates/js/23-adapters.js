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
      const permanent = a.permanent_failure || false;
      // 如果 Connected 但传输层不健康，显示 Degraded 而非 Connected
      let displayStatus = pollState || a.status;
      if (!pollState && a.status === 'Connected' && (a.health === 'Degraded' || a.health === 'Down')) {
        displayStatus = 'Degraded';
      }
      // Failed 且非永久停用 → 自动重连中（黄色徽章，区别于永久失败的红色）
      if (!pollState && a.status === 'Failed' && !permanent) {
        displayStatus = '自动重连中';
      }
      const statusClass = statusBadgeClass(a.status, a.connected, a.health, permanent);
      const icon = icons[a.platform] || '🔌';
      const platform = escapeHtml(String(a.platform || ''));
      // 健康状态副标题（默认隐藏，通过 WebSocket 事件更新时显示）
      const healthLabel = a.health === 'Degraded' ? '传输异常' : a.health === 'Down' ? '传输断开' : '';
      const healthDisplay = healthLabel ? 'block' : 'none';
      const healthSubtitle = `<div data-adapter-health="${platform}" style="font-size:12px;color:var(--text-muted);margin-top:2px;display:${healthDisplay}">${escapeHtml(healthLabel)}</div>`;
      // 重试/失败原因副标题（永久停用 / 自动重连进度 / 最近错误）
      const retryText = adapterRetryText(a);
      const retrySubtitle = retryText
        ? `<div data-adapter-retry="${platform}" style="font-size:12px;color:var(--warning);margin-top:2px">${escapeHtml(retryText)}</div>`
        : `<div data-adapter-retry="${platform}" style="display:none"></div>`;
      // 下次重试倒计时（仅瞬时失败且有排定重试时显示，每秒本地走时）
      const hasCountdown = a.status === 'Failed' && !permanent && typeof a.next_retry_in_ms === 'number';
      const countdownSubtitle = hasCountdown
        ? `<div data-adapter-countdown="${platform}" data-retry-until="${Date.now() + a.next_retry_in_ms}" style="font-size:12px;color:var(--warning);margin-top:2px">下次 ${fmtDuration(a.next_retry_in_ms)} 后重试</div>`
        : `<div data-adapter-countdown="${platform}" style="display:none"></div>`;
      return `<div class="card" data-adapter-card="${platform}">
        <div style="display:flex;justify-content:space-between;align-items:center">
          <div>
            <h3>${icon} ${escapeHtml(String(a.display_name || ''))} <span class="badge ${statusClass}" data-adapter-badge="${platform}">${escapeHtml(String(displayStatus || ''))}</span></h3>
            ${healthSubtitle}
            ${retrySubtitle}
            ${countdownSubtitle}
          </div>
          <div data-adapter-buttons="${platform}">
            <button class="btn btn-sm btn-primary adapter-action" data-platform="${platform}" data-action="start" ${a.connected || pollState ? 'disabled':''}>启动</button>
            <button class="btn btn-sm btn-danger adapter-action" data-platform="${platform}" data-action="stop" ${!a.connected || pollState ? 'disabled':''}>停止</button>
          </div>
        </div>
      </div>`;
    }).join('') + '</div>';
    content.querySelectorAll('.adapter-action').forEach(button => {
      button.addEventListener('click', () => adapterAction(button.dataset.platform, button.dataset.action));
    });
    loading.style.display = 'none';
    content.style.display = 'block';
  } catch (e) {
    loading.innerHTML = '加载失败: ' + escapeHtml(e.message);
  }
}

// 更新单个 adapter 卡片的 badge 和按钮状态（不重新渲染整个列表）
// health: 传输层健康状态（"Healthy" / "Degraded" / "Down" / null），null 表示不覆盖
// permanent: 是否永久停用（默认 false）
function updateAdapterCard(platform, status, connected, polling, health, permanent) {
  const selector = CSS.escape(String(platform));
  const badge = document.querySelector(`[data-adapter-badge="${selector}"]`);
  if (badge) {
    badge.className = `badge ${statusBadgeClass(status, connected, health, permanent)}`;
    // 如果 Connected 但传输不健康，显示 Degraded
    let displayStatus = status;
    if (status === 'Connected' && (health === 'Degraded' || health === 'Down')) {
      displayStatus = 'Degraded';
    }
    badge.textContent = displayStatus;
  }
  // 更新按钮状态
  const btnDiv = document.querySelector(`[data-adapter-buttons="${selector}"]`);
  if (btnDiv) {
    const [startBtn, stopBtn] = btnDiv.querySelectorAll('button');
    if (startBtn) startBtn.disabled = connected || polling;
    if (stopBtn) stopBtn.disabled = !connected || polling;
  }
  // 更新健康状态副标题
  const healthDiv = document.querySelector(`[data-adapter-health="${selector}"]`);
  if (healthDiv) {
    if (health && health !== 'Healthy') {
      const healthLabel = health === 'Degraded' ? '传输异常' : '传输断开';
      healthDiv.textContent = healthLabel;
      healthDiv.style.display = 'block';
    } else {
      healthDiv.style.display = 'none';
    }
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
          return { status: state, connected: true, health: resp.health || null };
        }
        if (!targetConnected && !connected && !['Connecting', 'Starting', 'Disconnecting', 'Stopping'].includes(state)) {
          return { status: state, connected: false, health: resp.health || null };
        }
        // 失败的终止状态
        if (state === 'Failed') {
          return { status: state, connected: false, health: resp.health || null };
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
    updateAdapterCard(platform, result.status, result.connected, false, result.health);
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


