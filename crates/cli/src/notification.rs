use std::time::Duration;

use anyhow::{Context, bail};
use reqwest::Url;
use rs12306_storage::Database;

const FEISHU_WEBHOOK_KEY: &str = "notification.feishu.webhook";
const FEISHU_ENABLED_KEY: &str = "notification.feishu.enabled";
const MAX_MESSAGE_CHARS: usize = 4000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NotificationTypeStatus {
    pub notification_type: &'static str,
    pub configured: bool,
    pub enabled: bool,
    pub configuration: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryResult {
    pub notification_type: &'static str,
    pub error: Option<String>,
}

pub fn notification_types(database: &Database) -> anyhow::Result<Vec<NotificationTypeStatus>> {
    let webhook = database.get_setting(FEISHU_WEBHOOK_KEY)?;
    let configured = webhook.as_deref().is_some_and(|value| !value.is_empty());
    let enabled =
        configured && database.get_setting(FEISHU_ENABLED_KEY)?.as_deref() == Some("true");
    Ok(vec![NotificationTypeStatus {
        notification_type: "feishu",
        configured,
        enabled,
        configuration: webhook
            .as_deref()
            .map(masked_webhook)
            .unwrap_or_else(|| "-".to_string()),
    }])
}

pub fn configure_feishu(database: &Database, webhook: &str) -> anyhow::Result<()> {
    validate_feishu_webhook(webhook)?;
    database.set_setting(FEISHU_WEBHOOK_KEY, webhook.trim())?;
    database.set_setting(FEISHU_ENABLED_KEY, "true")?;
    Ok(())
}

pub fn enable_feishu(database: &Database, enabled: bool) -> anyhow::Result<()> {
    configured_feishu_webhook(database)?;
    database.set_setting(FEISHU_ENABLED_KEY, if enabled { "true" } else { "false" })?;
    Ok(())
}

pub fn remove_feishu(database: &Database) -> anyhow::Result<()> {
    database.delete_setting(FEISHU_WEBHOOK_KEY)?;
    database.delete_setting(FEISHU_ENABLED_KEY)?;
    Ok(())
}

pub async fn test_feishu(database: &Database, message: &str) -> anyhow::Result<()> {
    let webhook = configured_feishu_webhook(database)?;
    send_feishu_with_retry(&webhook, message)
        .await
        .map_err(anyhow::Error::msg)
}

pub async fn send_enabled(database: &Database, message: &str) -> Vec<DeliveryResult> {
    let status = match notification_types(database) {
        Ok(status) => status,
        Err(error) => {
            return vec![DeliveryResult {
                notification_type: "feishu",
                error: Some(format!(
                    "failed to read notification configuration: {error}"
                )),
            }];
        }
    };
    if !status[0].enabled {
        return Vec::new();
    }
    let webhook = match configured_feishu_webhook(database) {
        Ok(webhook) => webhook,
        Err(error) => {
            return vec![DeliveryResult {
                notification_type: "feishu",
                error: Some(error.to_string()),
            }];
        }
    };
    vec![DeliveryResult {
        notification_type: "feishu",
        error: send_feishu_with_retry(&webhook, message).await.err(),
    }]
}

fn configured_feishu_webhook(database: &Database) -> anyhow::Result<String> {
    database
        .get_setting(FEISHU_WEBHOOK_KEY)?
        .filter(|value| !value.trim().is_empty())
        .context("feishu notification is not configured; run `12306-rs notify set feishu`")
}

fn validate_feishu_webhook(webhook: &str) -> anyhow::Result<()> {
    let url = Url::parse(webhook.trim()).context("invalid feishu webhook URL")?;
    if url.scheme() != "https"
        || url.host_str() != Some("open.feishu.cn")
        || !url.path().starts_with("/open-apis/bot/v2/hook/")
        || url.path().trim_end_matches('/') == "/open-apis/bot/v2/hook"
    {
        bail!("webhook must be a Feishu bot v2 URL under https://open.feishu.cn");
    }
    Ok(())
}

fn masked_webhook(webhook: &str) -> String {
    let Ok(url) = Url::parse(webhook) else {
        return "invalid webhook".to_string();
    };
    let token = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or_default();
    let tail: String = token
        .chars()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!(
        "{}://{}/open-apis/bot/v2/hook/****{}",
        url.scheme(),
        url.host_str().unwrap_or_default(),
        tail
    )
}

async fn send_feishu_with_retry(webhook: &str, message: &str) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
        .build()
        .map_err(|error| error.to_string())?;
    let mut last_error = String::new();
    for attempt in 0..=2 {
        match send_feishu_once(&client, webhook, message).await {
            Ok(()) => return Ok(()),
            Err(error) => last_error = error,
        }
        if attempt < 2 {
            tokio::time::sleep(Duration::from_secs(attempt + 1)).await;
        }
    }
    Err(last_error)
}

async fn send_feishu_once(
    client: &reqwest::Client,
    webhook: &str,
    message: &str,
) -> Result<(), String> {
    let response = client
        .post(webhook)
        .json(&serde_json::json!({
            "msg_type": "text",
            "content": { "text": truncate(message, MAX_MESSAGE_CHARS) }
        }))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    let status = response.status();
    let body = response.text().await.map_err(|error| error.to_string())?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {}", truncate(&body, 300)));
    }
    parse_feishu_response(&body)
}

fn parse_feishu_response(body: &str) -> Result<(), String> {
    let payload: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| format!("non-JSON response: {}", truncate(body, 300)))?;
    let code = payload["code"]
        .as_i64()
        .or_else(|| payload["StatusCode"].as_i64())
        .unwrap_or(-1);
    if code == 0 {
        return Ok(());
    }
    let message = payload["msg"]
        .as_str()
        .or_else(|| payload["StatusMessage"].as_str())
        .unwrap_or("unknown error");
    Err(format!("Feishu error {code}: {message}"))
}

pub fn truncate(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_feishu_v2_webhooks() {
        assert!(
            validate_feishu_webhook("https://open.feishu.cn/open-apis/bot/v2/hook/12345678-1234")
                .is_ok()
        );
        assert!(validate_feishu_webhook("https://example.com/hook").is_err());
    }

    #[test]
    fn parses_modern_and_legacy_success_responses() {
        assert!(parse_feishu_response(r#"{"code":0,"msg":"success"}"#).is_ok());
        assert!(parse_feishu_response(r#"{"StatusCode":0}"#).is_ok());
        assert!(parse_feishu_response(r#"{"code":19024,"msg":"Key Words Not Found"}"#).is_err());
    }

    #[test]
    fn truncates_unicode_by_character() {
        assert_eq!(truncate("上海虹桥", 2), "上海...");
        assert_eq!(truncate("上海", 2), "上海");
    }

    #[test]
    fn stores_one_global_feishu_configuration() {
        let database = Database::open_in_memory().unwrap();
        assert_eq!(
            notification_types(&database).unwrap(),
            vec![NotificationTypeStatus {
                notification_type: "feishu",
                configured: false,
                enabled: false,
                configuration: "-".to_string(),
            }]
        );

        configure_feishu(
            &database,
            "https://open.feishu.cn/open-apis/bot/v2/hook/12345678-1234",
        )
        .unwrap();
        let status = notification_types(&database).unwrap();
        assert!(status[0].enabled);
        assert_eq!(
            status[0].configuration,
            "https://open.feishu.cn/open-apis/bot/v2/hook/****678-1234"
        );

        enable_feishu(&database, false).unwrap();
        assert!(!notification_types(&database).unwrap()[0].enabled);

        remove_feishu(&database).unwrap();
        assert!(!notification_types(&database).unwrap()[0].configured);
    }
}
