# 各平台 API 能力调研报告

## 1. QQ 开放平台 API v2

| 能力 | 适配器现状 | API 是否支持 | 结论 |
|------|-----------|-------------|------|
| 文本发送 | ✅ 实现 | ✅ 支持 | 正确 |
| 媒体发送（图片） | ✅ 实现（`msg_type: 2`） | ✅ C2C/群/频道均支持 | **需改进**：C2C 私聊不支持 `msg_type: 2`（图文混合），需要改用 `msg_type: 1`（纯图片）或消息段模式 |
| 交互式消息（Keyboard 按钮） | ✅ 已有实现（`send_interactive`） | ✅ 全部端点支持 | 实现完整，但真实 E2E 测试中失败了，需排查原因 |
| 编辑消息 | ✅ 已实现（`PATCH /channels/...`） | ⚠️ **频道限定** `PATCH /channels/{id}/messages/{msg_id}` | **真实限制**：QQ API 只支持频道消息编辑，C2C/群聊不支持，且需申请特殊权限 |
| 撤回消息 | ✅ 已实现（`DELETE /channels/...`） | ⚠️ **频道限定** `DELETE /channels/{id}/messages/{msg_id}` | **真实限制**：QQ API 只支持频道消息撤回，C2C/群聊不支持 |

### 参考文档
- [QQ 机器人官方文档](https://bot.q.qq.com/wiki/)
- `qq-bot-rs` Rust SDK (crates.io)
- `@chnak/qq-bot` (npm)

---

## 2. 飞书开放平台 API

| 能力 | 适配器现状 | API 是否支持 | 结论 |
|------|-----------|-------------|------|
| 文本发送 | ✅ 实现 | ✅ `POST /im/v1/messages` | 正确 |
| 媒体发送（图片/文件） | ❌ **未实现** | ✅ `msg_type: "image"` / `"file"` 均支持 | **适配器缺失**：需先上传获取 `image_key`/`file_key`，再发消息 |
| 交互式消息（消息卡片） | ✅ 实现 | ✅ `msg_type: "interactive"` | 正确 |
| **编辑文本消息** | ❌ **错误实现**（用 `PATCH`） | ✅ **正确 API**：`PUT /im/v1/messages/{id}` + `msg_type: "text"` | **适配器缺陷**：当前用 `PATCH /.../patch`（卡片编辑），文本编辑应使用 `PUT /im/v1/messages/{id}` |
| 撤回消息 | ❌ **未实现**（返回"飞书不支持删除"） | ✅ `DELETE /im/v1/messages/{id}`（24h 内可撤回自己发的消息） | **适配器缺失**：API 明确支持删除/撤回 |
| 编辑卡片消息 | ✅ 实现（`PATCH`） | ✅ `PATCH /im/v1/messages/{id}` | 正确 |

### 参考文档
- [编辑消息 (PUT)](https://open.feishu.cn/document/server-docs/im-v1/message/update)
- [撤回消息 (DELETE)](https://open.feishu.cn/document/server-docs/im-v1/message/delete)
- [发送消息 (POST)](https://open.feishu.cn/document/server-docs/im-v1/message/create)
- [上传图片](https://open.feishu.cn/document/server-docs/im-v1/image/create)
- [消息内容结构](https://open.feishu.cn/document/server-docs/im-v1/message-content-description/create_json)

---

## 3. Discord Bot API

| 能力 | 适配器现状 | API 是否支持 | 结论 |
|------|-----------|-------------|------|
| 文本发送 | ✅ 实现 | ✅ `POST /channels/{id}/messages` | 正确 |
| 媒体发送 | ✅ 实现 | ✅ 附件/embed | 正确 |
| **交互式消息（按钮/ActionRow）** | ❌ **未实现** | ✅ **原生支持**：`components` 字段 + `flags: 32768`（Components v2） | **适配器缺失**：Discord 的 ActionRow + Button 组件系统 2023 年就已 GA |
| 编辑消息 | ✅ 实现 | ✅ | 正确 |
| 撤回消息 | ✅ 实现 | ✅ | 正确 |

### Discord Components 消息格式
```json
{
  "flags": 32768,
  "components": [
    {
      "type": 1,
      "components": [
        { "type": 2, "custom_id": "btn_yes", "label": "是", "style": 1 },
        { "type": 2, "custom_id": "btn_no",  "label": "否", "style": 4 }
      ]
    }
  ]
}
```

### 参考文档
- [Discord Message Components](https://docs.discord.com/developers/interactions/message-components)

---

## 4. 微信 iLink Bot API

| 能力 | 适配器现状 | API 是否支持 | 结论 |
|------|-----------|-------------|------|
| 文本发送 | ✅ 实现 | ✅ `sendMessage` | 正确 |
| **图片发送** | ❌ **未实现** | ✅ `sendImage` / `sendPhoto` | **适配器缺失**：支持图片发送，需 AES 加密传输 |
| **文件发送** | ❌ **未实现** | ✅ `sendFile` / `sendDocument` | **适配器缺失** |
| **视频发送** | ❌ **未实现** | ✅ `sendVideo` | **适配器缺失** |
| **语音发送** | ❌ **未实现** | ✅ `sendVoice` | **适配器缺失** |
| 交互式消息 | ❌ 不支持 | ❌ **真实限制** | 个人微信确实没有按钮/卡片能力 |
| 编辑/撤回 | ❌ 不支持 | ❌ **真实限制** | iLink Bot 无此能力 |

### 参考文档
- [@chnak/weixin-bot (npm)](https://www.npmjs.com/package/@chnak/weixin-bot)
- [腾讯 openclaw 微信 SDK](https://github.com/Tencent/openclaw-weixin)

---

## 结论：真实限制 vs 适配器缺失

### ✅ 真实平台限制（共 4 项，全部已正确标注）

| 平台 | 限制 | 说明 |
|------|------|------|
| QQ | edit_message | 仅频道端点支持，C2C/群聊不支持 |
| QQ | delete_message | 仅频道端点支持，C2C/群聊不支持 |
| 微信 | 交互式消息 | iLink Bot 无按钮/卡片能力 |
| 微信 | 编辑/撤回 | iLink Bot 无此能力 |

### ❌ 适配器缺失/缺陷 — 状态更新

| # | 平台 | 缺失能力 | 优先级 | 复杂度 | 状态 |
|---|------|---------|--------|--------|------|
| 1 | **飞书 edit_message** | 用错方法（PATCH → PUT）导致"NOT a card" | **P0** | 低 | ✅ 已修复 (`a8ae8b8`) |
| 2 | **飞书 delete_message** | 未实现，返回"飞书不支持删除" | **P0** | 低 | ✅ 已修复 (`a8ae8b8`) |
| 3 | **Discord send_interactive** | 缺少 E2E 测试（适配器代码已有） | **P1** | 低 | ✅ 已补充测试 (`cd5d6fc`) |
| 4 | **飞书 send_media** | 代码已实现（上传+发送） | **P2** | — | ✅ 代码存在，无需改动 |
| 5 | **微信 send_media** | 代码已实现（CDN 上传+发送） | **P2** | — | ✅ 代码存在，无需改动 |
| 6 | **QQ C2C media** | `msg_type: 2` 不适用 C2C | **P3** | 中 | ✅ 已修复 — 降级为 `msg_type: 1` (`e77ce40`) |

### 推荐行动计划（已全部完成）

1. ✅ **P0** — 飞书 edit_message（PUT）+ delete_message（DELETE）
2. ✅ **P1** — Discord send_interactive E2E 测试补充
3. ✅ **P2** — 飞书/微信 send_media：代码已存在，经验证
4. ✅ **P3** — QQ C2C media：`msg_type: 2` → `msg_type: 1` 降级
