// utils.js 纯函数单元测试
// 加载方式：用 jsdom 的 window.eval 以非严格模式执行 utils.js，
// 使 function 声明成为 window 的全局函数，再挂到 globalThis 供测试调用。
import { describe, it, expect, beforeAll } from 'vitest';
import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { JSDOM } from 'jsdom';

const code = readFileSync(fileURLToPath(new URL('../js/00-utils.js', import.meta.url)), 'utf8');

beforeAll(() => {
  const dom = new JSDOM('<!doctype html><html><body></body></html>', { runScripts: 'outside-only' });
  dom.window.eval(code);
  for (const name of [
    'escapeHtml', 'msgTypeLabel', 'msgTypeBadgeClass', 'statusBadgeClass',
    'platformBadgeClass', 'chatTypeBadgeClass', 'msgRoleBadgeClass',
    'truncateText', 'fmtDuration', 'adapterRetryText', 'renderProgressBar',
    'parsePrometheus', 'mbar',
  ]) {
    globalThis[name] = dom.window[name];
  }
});

describe('escapeHtml', () => {
  it('转义 & < >', () => {
    expect(escapeHtml('&')).toBe('&amp;');
    expect(escapeHtml('<')).toBe('&lt;');
    expect(escapeHtml('>')).toBe('&gt;');
  });
  it('转义标签注入', () => {
    expect(escapeHtml('<img src=x onerror=alert(1)>')).toBe('&lt;img src=x onerror=alert(1)&gt;');
  });
  it('普通文本不变', () => {
    expect(escapeHtml('hello world')).toBe('hello world');
  });
});

describe('msgTypeLabel / msgTypeBadgeClass', () => {
  it('已知类型返回中文标签', () => {
    expect(msgTypeLabel({ msg_type: 'Text' })).toBe('文本');
    expect(msgTypeLabel({ msg_type: 'Image' })).toBe('图片');
    expect(msgTypeLabel({ msg_type: 'Sticker' })).toBe('贴纸');
  });
  it('未知类型回退原文', () => {
    expect(msgTypeLabel({ msg_type: 'Weird' })).toBe('Weird');
  });
  it('缺失 msg_type 回退"文本"', () => {
    expect(msgTypeLabel({})).toBe('文本');
    expect(msgTypeLabel(null)).toBe('文本');
  });
  it('badge class 映射', () => {
    expect(msgTypeBadgeClass({ msg_type: 'Image' })).toBe('badge-type-image');
    expect(msgTypeBadgeClass({})).toBe('badge-type-text');
  });
});

describe('statusBadgeClass', () => {
  it('已连接且健康 → green', () => {
    expect(statusBadgeClass('Connected', true, 'Healthy', false)).toBe('badge-green');
  });
  it('已连接但传输不健康 → yellow', () => {
    expect(statusBadgeClass('Connected', true, 'Degraded', false)).toBe('badge-yellow');
    expect(statusBadgeClass('Connected', true, 'Down', false)).toBe('badge-yellow');
  });
  it('Failed 永久 → red，非永久 → yellow', () => {
    expect(statusBadgeClass('Failed', false, null, true)).toBe('badge-red');
    expect(statusBadgeClass('Failed', false, null, false)).toBe('badge-yellow');
  });
  it('过渡态 → blue', () => {
    expect(statusBadgeClass('Connecting', false, null, false)).toBe('badge-blue');
    expect(statusBadgeClass('Starting', false, null, false)).toBe('badge-blue');
    expect(statusBadgeClass('Stopping', false, null, false)).toBe('badge-blue');
  });
  it('Reconnecting → yellow', () => {
    expect(statusBadgeClass('Reconnecting', false, null, false)).toBe('badge-yellow');
  });
  it('其余 → gray', () => {
    expect(statusBadgeClass('Stopped', false, null, false)).toBe('badge-gray');
  });
});

describe('badge class 工具', () => {
  it('platform / chatType / role', () => {
    expect(platformBadgeClass('telegram')).toBe('badge-platform-telegram');
    expect(chatTypeBadgeClass('Dm')).toBe('badge-chattype-Dm');
    expect(msgRoleBadgeClass('User')).toBe('badge-role-User');
    expect(msgRoleBadgeClass('Assistant')).toBe('badge-role-Assistant');
    expect(msgRoleBadgeClass('Other')).toBe('badge-gray');
  });
});

describe('fmtDuration', () => {
  it('秒级（含向上取整到 1s）', () => {
    expect(fmtDuration(0)).toBe('1s');
    expect(fmtDuration(999)).toBe('1s');
    expect(fmtDuration(59000)).toBe('59s');
  });
  it('分钟级', () => {
    expect(fmtDuration(60000)).toBe('1m');
    expect(fmtDuration(61000)).toBe('1m1s');
    expect(fmtDuration(90000)).toBe('1m30s');
    expect(fmtDuration(3600000)).toBe('60m');
  });
});

describe('truncateText', () => {
  it('超长截断并加省略号', () => {
    expect(truncateText('abcdef', 4)).toBe('abc…');
  });
  it('未超长原样返回', () => {
    expect(truncateText('ab', 4)).toBe('ab');
  });
});

describe('adapterRetryText', () => {
  it('永久失败 → 已停用', () => {
    expect(adapterRetryText({ status: 'Failed', permanent_failure: true })).toContain('已停用');
  });
  it('瞬时失败含重试次数', () => {
    expect(adapterRetryText({ status: 'Failed', retry_attempt: 3 })).toContain('第 3 次');
  });
  it('非 Failed 且无错误 → 空串', () => {
    expect(adapterRetryText({ status: 'Connected' })).toBe('');
  });
});

describe('mbar / renderProgressBar', () => {
  it('mbar 生成颜色类与宽度', () => {
    expect(mbar(50, 'blue')).toContain('mbar-fill-blue');
    expect(mbar(50, 'blue')).toContain('width:50%');
  });
  it('mbar 上限钳制 100%', () => {
    expect(mbar(150, 'red')).toContain('width:100%');
  });
  it('renderProgressBar 阈值配色', () => {
    expect(renderProgressBar(30)).toContain('fill-green');
    expect(renderProgressBar(70)).toContain('fill-yellow');
    expect(renderProgressBar(90)).toContain('fill-red');
  });
});

describe('parsePrometheus', () => {
  it('解析普通指标', () => {
    const out = parsePrometheus('http_requests_total 42');
    expect(out['http_requests_total'].value).toBe(42);
    expect(out['http_requests_total'].name).toBe('http_requests_total');
  });
  it('解析标签', () => {
    const out = parsePrometheus('http_requests_total{method="GET",status="200"} 10');
    const entry = Object.values(out)[0];
    expect(entry.labels.method).toBe('GET');
    expect(entry.labels.status).toBe('200');
    expect(entry.value).toBe(10);
  });
  it('忽略注释行与空行', () => {
    const out = parsePrometheus('# HELP x\n# TYPE x counter\n\nx 1');
    expect(Object.keys(out).length).toBe(1);
  });
  it('支持科学计数法与负数', () => {
    const out = parsePrometheus('a 1.5e3\nb -5');
    expect(out['a'].value).toBe(1500);
    expect(out['b'].value).toBe(-5);
  });
  it('忽略非法行', () => {
    const out = parsePrometheus('not a metric line at all');
    expect(Object.keys(out).length).toBe(0);
  });
});
