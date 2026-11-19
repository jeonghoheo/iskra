use serde::{Deserialize, Serialize};
use std::time::Duration;

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/chat/completions";
pub const DEFAULT_MODEL: &str = "deepseek-chat";
pub const MODEL_TIMEOUT_SECS: u64 = 60;

#[derive(Clone, Debug)]
pub struct Config {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
}

impl Config {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(|key| std::env::var(key).ok())
    }

    pub fn from_lookup<F>(lookup: F) -> Result<Self, String>
    where
        F: Fn(&str) -> Option<String>,
    {
        let api_key = lookup("LLM_API_KEY")
            .and_then(non_empty)
            .or_else(|| lookup("DEEPSEEK_API_KEY").and_then(non_empty))
            .ok_or_else(|| {
                "모델 API 키가 없습니다: LLM_API_KEY 또는 DEEPSEEK_API_KEY를 설정하세요".to_string()
            })?;

        let base_url = lookup("LLM_BASE_URL")
            .and_then(non_empty)
            .unwrap_or_else(|| DEFAULT_BASE_URL.to_string());
        let model = lookup("LLM_MODEL")
            .and_then(non_empty)
            .unwrap_or_else(|| DEFAULT_MODEL.to_string());

        Ok(Self {
            api_key,
            base_url,
            model,
        })
    }
}

fn non_empty(value: String) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(content: &str) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn user(content: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
        }
    }

    pub fn tool(tool_call_id: &str, content: &str) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub call_type: String,
    pub function: FunctionCall,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

pub struct ChatReply {
    pub message: Message,
    pub usage: Option<Usage>,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    tools: serde_json::Value,
    tool_choice: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
    usage: Option<Usage>,
}

#[derive(Deserialize)]
struct Choice {
    message: Message,
}

pub struct Brain {
    client: reqwest::Client,
    config: Config,
}

impl Brain {
    pub fn new(config: Config) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
        }
    }

    pub fn model(&self) -> &str {
        &self.config.model
    }

    pub async fn chat(&self, messages: &[Message]) -> Result<ChatReply, String> {
        let request = ChatRequest {
            model: &self.config.model,
            messages,
            tools: crate::tools::schemas(),
            tool_choice: "auto",
        };

        let exchange = async {
            let response = self
                .client
                .post(&self.config.base_url)
                .bearer_auth(&self.config.api_key)
                .json(&request)
                .send()
                .await?;
            let status = response.status();
            let body = response.text().await?;
            Ok::<_, reqwest::Error>((status, body))
        };

        let (status, body) =
            tokio::time::timeout(Duration::from_secs(MODEL_TIMEOUT_SECS), exchange)
                .await
                .map_err(|_| format!("모델 호출 타임아웃({MODEL_TIMEOUT_SECS}초)"))?
                .map_err(|e| format!("모델 호출 실패: {e}"))?;

        if !status.is_success() {
            return Err(format!(
                "모델 API 오류 {status}: {}",
                truncate_chars(&body, 300)
            ));
        }

        let parsed: ChatResponse =
            serde_json::from_str(&body).map_err(|e| format!("응답 JSON 파싱 실패: {e}"))?;
        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| "모델 응답에 choices가 없습니다".to_string())?;

        Ok(ChatReply {
            message: choice.message,
            usage: parsed.usage,
        })
    }
}

fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_string()
    } else {
        let mut out: String = text.chars().take(limit).collect();
        out.push('…');
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn lookup(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| (key.to_string(), value.to_string()))
            .collect();
        move |key| map.get(key).cloned()
    }

    #[test]
    fn defaults_apply_with_deepseek_key() {
        let config = Config::from_lookup(lookup(&[("DEEPSEEK_API_KEY", "ds-key")])).unwrap();
        assert_eq!(config.api_key, "ds-key");
        assert_eq!(config.base_url, DEFAULT_BASE_URL);
        assert_eq!(config.model, DEFAULT_MODEL);
    }

    #[test]
    fn llm_vars_take_priority() {
        let config = Config::from_lookup(lookup(&[
            ("LLM_API_KEY", "llm-key"),
            ("DEEPSEEK_API_KEY", "ds-key"),
            ("LLM_BASE_URL", "https://example.test/v1/chat"),
            ("LLM_MODEL", "custom-model"),
        ]))
        .unwrap();
        assert_eq!(config.api_key, "llm-key");
        assert_eq!(config.base_url, "https://example.test/v1/chat");
        assert_eq!(config.model, "custom-model");
    }

    #[test]
    fn deepseek_key_is_compat_fallback() {
        let config = Config::from_lookup(lookup(&[
            ("LLM_API_KEY", ""),
            ("DEEPSEEK_API_KEY", "ds-key"),
        ]))
        .unwrap();
        assert_eq!(config.api_key, "ds-key");
    }

    #[test]
    fn missing_key_names_required_vars() {
        let err = Config::from_lookup(lookup(&[])).unwrap_err();
        assert!(err.contains("LLM_API_KEY"));
        assert!(err.contains("DEEPSEEK_API_KEY"));
    }
}
