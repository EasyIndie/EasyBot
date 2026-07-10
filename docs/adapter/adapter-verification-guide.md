# 适配器验证指南

本目录存放各平台适配器的验证/测试文档。每个适配器一个独立文件，供测试和验收时参考。

## 文件列表

| 文件 | 适配器 | 状态 |
|------|--------|------|
| [telegram.md](telegram.md) | Telegram | ✅ 已验证 |
| [discord.md](discord.md) | Discord | ✅ 已验证 |
| [qq.md](qq.md) | QQ | ✅ 已验证（鉴权升级 + 群聊/私聊/频道全场景双向收发 + Interactive + ChatList） |
| [feishu.md](feishu.md) | 飞书 | ✅ 已验证（REST API + WebSocket 事件订阅双向） |
| [wechat.md](wechat.md) | 个人微信 | ✅ 已验证（iLink Bot API 长轮询双向） |

> 另见：本目录下的 [适配器架构全面评审报告](adapter-performance-review.md)，包含所有适配器的代码审查结果和优化建议。

## 验证方法速览

所有适配器通用的验证层次：

1. **纯单元测试** —— `cargo test -p easybot-adapter-<name> -- <pure_unit_test>`  — 离线、快速
2. **全部单元测试** —— `cargo test -p easybot-adapter-<name>` — 有网络即可
3. **端到端验证** —— 获取真实平台凭证、启动服务、发送消息
4. **适配器管理 API** —— `/api/v1/adapters/{platform}/start|stop|status`
5. **消息 API** —— `/api/v1/messages/send` 发送并确认接收

## 新建适配器验证文档模板

复制以下结构创建新适配器的验证文档：

```markdown
# <Platform> 适配器验证指南

验证范围：`easybot-adapter-<name>` crate。

## 当前测试现状

| 测试名 | 类型 | 依赖网络 | 需要真实凭证 |
|--------|------|---------|-------------|
| `test_xxx` | 单元测试 | ❌ | ❌ |

## 验证方法

### 1. 纯离线单元测试

```bash
cargo test -p easybot-adapter-<name> -- <test_name> --exact
```

### 2. 全部单元测试

```bash
cargo test -p easybot-adapter-<name>
```

### 3. 端到端验证——真实平台

#### 3.1 获取凭证

...

#### 3.2 配置

...

#### 3.3 启动并验证

...

### 4. 适配器管理 API

...

## 关键实现细节

| 属性 | 值 |
|------|-----|
| 连接方式 | ... |
| 支持的能力 | ... |

## 后续改进建议

- [ ] ...
```
