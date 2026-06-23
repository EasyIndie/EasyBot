//! Discord Bot API 数据类型
//!
//! 定义用于反序列化 Discord REST API 响应的数据结构。

#![allow(dead_code)]

use serde::Deserialize;

// ── REST API 公共类型 ──

/// Discord 用户对象
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct DiscordUser {
    pub id: String,
    pub username: String,
    #[serde(default)]
    pub global_name: Option<String>,
    #[serde(default)]
    pub bot: Option<bool>,
    #[serde(default)]
    pub avatar: Option<String>,
}

/// Discord 频道对象
#[derive(Debug, Deserialize)]
pub(crate) struct DiscordChannel {
    pub id: String,
    #[serde(rename = "type")]
    pub channel_type: u8,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub guild_id: Option<String>,
}

/// Discord 服务器（Guild）对象 — GET /users/@me/guilds 响应
#[derive(Debug, Deserialize)]
pub(crate) struct DiscordGuild {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub owner: Option<bool>,
}

/// 附件对象
#[derive(Debug, Deserialize)]
pub(crate) struct DiscordAttachment {
    pub id: String,
    pub filename: String,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub content_type: Option<String>,
    #[serde(default)]
    pub width: Option<u64>,
    #[serde(default)]
    pub height: Option<u64>,
}

/// Discord 消息对象（REST API 响应）
#[derive(Debug, Deserialize)]
pub(crate) struct DiscordMessage {
    pub id: String,
    pub channel_id: String,
    #[serde(default)]
    pub guild_id: Option<String>,
    pub author: DiscordUser,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub attachments: Vec<DiscordAttachment>,
    pub timestamp: String,
    #[serde(default)]
    pub edited_timestamp: Option<String>,
    #[serde(default)]
    pub mention_everyone: bool,
    #[serde(default)]
    pub tts: bool,
    #[serde(rename = "type")]
    #[serde(default)]
    pub msg_type: u8,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_discord_user_deserialize_with_bot_field() {
        let json =
            r#"{"id":"123","username":"test","global_name":"Test","bot":true,"avatar":null}"#;
        let user: DiscordUser = serde_json::from_str(json).unwrap();
        assert_eq!(user.id, "123");
        assert_eq!(user.bot, Some(true));
        assert_eq!(user.username, "test");
    }

    #[test]
    fn test_discord_user_deserialize_without_bot_field() {
        // 普通用户消息中不包含 bot 字段，应默认 None
        let json = r#"{"id":"456","username":"realuser","global_name":"Real User","avatar":null}"#;
        let user: DiscordUser = serde_json::from_str(json).unwrap();
        assert_eq!(user.id, "456");
        assert_eq!(user.bot, None);
    }

    #[test]
    fn test_discord_user_deserialize_bot_false() {
        let json =
            r#"{"id":"789","username":"human","global_name":null,"bot":false,"avatar":"abc"}"#;
        let user: DiscordUser = serde_json::from_str(json).unwrap();
        assert_eq!(user.bot, Some(false));
    }

    #[test]
    fn test_discord_message_deserialize_without_bot_on_author() {
        // Discord MESSAGE_CREATE 中 author 可能不含 bot 字段
        let json = r#"{
            "id":"msg1",
            "channel_id":"ch1",
            "guild_id":null,
            "author":{"id":"author1","username":"user","global_name":null,"avatar":null},
            "content":"hello",
            "timestamp":"2026-01-01T00:00:00+00:00",
            "edited_timestamp":null,
            "mention_everyone":false,
            "tts":false,
            "type":0
        }"#;
        let msg: DiscordMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.author.id, "author1");
        assert_eq!(msg.author.bot, None);
        assert_eq!(msg.content, Some("hello".to_string()));
    }

    #[test]
    fn test_discord_guild_deserialize() {
        let json = r#"{"id":"guild123","name":"My Server","owner":true}"#;
        let guild: DiscordGuild = serde_json::from_str(json).unwrap();
        assert_eq!(guild.id, "guild123");
        assert_eq!(guild.name, "My Server");
        assert_eq!(guild.owner, Some(true));
    }

    #[test]
    fn test_discord_guild_deserialize_without_owner() {
        let json = r#"{"id":"guild456","name":"Another Server"}"#;
        let guild: DiscordGuild = serde_json::from_str(json).unwrap();
        assert_eq!(guild.id, "guild456");
        assert_eq!(guild.name, "Another Server");
        assert_eq!(guild.owner, None);
    }
}
