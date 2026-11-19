use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/chat/completions";
pub const DEFAULT_MODEL: &str = "deepseek-chat";
pub const MODEL_TIMEOUT_SECS: u64 = 60;
const BALANCE_TIMEOUT_SECS: u64 = 3;
const BALANCE_CACHE_TTL: Duration = Duration::from_secs(60);

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
                "Model API key is missing: set LLM_API_KEY or DEEPSEEK_API_KEY".to_string()
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

#[derive(Clone, Debug, Deserialize)]
pub struct BalanceResponse {
    #[serde(default)]
    pub is_available: bool,
    #[serde(default)]
    pub balance_infos: Vec<BalanceInfo>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct BalanceInfo {
    pub currency: String,
    pub total_balance: String,
}

impl BalanceResponse {
    pub fn display(&self) -> String {
        if self.balance_infos.is_empty() {
            return "unavailable".to_string();
        }
        let mut text = self
            .balance_infos
            .iter()
            .map(|balance| format!("{} {}", balance.total_balance, balance.currency))
            .collect::<Vec<_>>()
            .join(", ");
        if !self.is_available {
            text.push_str(" (insufficient for API calls)");
        }
        text
    }
}

pub fn balance_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    let root = trimmed.strip_suffix("/chat/completions").unwrap_or(trimmed);
    format!("{root}/user/balance")
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

struct BalanceCache {
    fetched: Instant,
    value: Result<BalanceResponse, String>,
}

pub struct Brain {
    client: reqwest::Client,
    config: Config,
    balance_cache: Mutex<Option<BalanceCache>>,
}

impl Brain {
    pub fn new(config: Config) -> Self {
        Self {
            client: reqwest::Client::new(),
            config,
            balance_cache: Mutex::new(None),
        }
    }

    pub fn model(&self) -> &str {
        &self.config.model
    }

    pub fn base_url(&self) -> &str {
        &self.config.base_url
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
                .map_err(|_| format!("Model call timed out after {MODEL_TIMEOUT_SECS}s"))?
                .map_err(|e| format!("Model call failed: {e}"))?;

        if !status.is_success() {
            return Err(format!(
                "Model API error {status}: {}",
                truncate_chars(&body, 300)
            ));
        }

        let parsed: ChatResponse = serde_json::from_str(&body)
            .map_err(|e| format!("Failed to parse response JSON: {e}"))?;
        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| "Model response has no choices".to_string())?;

        Ok(ChatReply {
            message: choice.message,
            usage: parsed.usage,
        })
    }

    pub async fn balance(&self) -> Result<BalanceResponse, String> {
        let url = balance_url(&self.config.base_url);
        let exchange = async {
            let response = self
                .client
                .get(&url)
                .bearer_auth(&self.config.api_key)
                .send()
                .await?;
            let status = response.status();
            let body = response.text().await?;
            Ok::<_, reqwest::Error>((status, body))
        };

        let result =
            match tokio::time::timeout(Duration::from_secs(BALANCE_TIMEOUT_SECS), exchange).await {
                Err(_) => Err(format!(
                    "Balance lookup timed out after {BALANCE_TIMEOUT_SECS}s"
                )),
                Ok(Err(e)) => Err(format!("Balance lookup failed: {e}")),
                Ok(Ok((status, body))) => {
                    if !status.is_success() {
                        Err(format!(
                            "Balance API error {status}: {}",
                            truncate_chars(&body, 300)
                        ))
                    } else {
                        serde_json::from_str(&body)
                            .map_err(|e| format!("Failed to parse balance response: {e}"))
                    }
                }
            };

        if let Err(e) = &result {
            crate::logging::append(&format!("balance_error {e}"));
        }
        *self.balance_cache.lock().unwrap() = Some(BalanceCache {
            fetched: Instant::now(),
            value: result.clone(),
        });
        result
    }

    pub async fn balance_cached(&self) -> Result<BalanceResponse, String> {
        if let Some(value) = self.cached_balance() {
            return value;
        }
        self.balance().await
    }

    fn cached_balance(&self) -> Option<Result<BalanceResponse, String>> {
        let cache = self.balance_cache.lock().unwrap();
        cache
            .as_ref()
            .filter(|entry| entry.fetched.elapsed() < BALANCE_CACHE_TTL)
            .map(|entry| entry.value.clone())
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

    #[test]
    fn derives_balance_url_from_chat_endpoint() {
        assert_eq!(
            balance_url("https://api.deepseek.com/chat/completions"),
            "https://api.deepseek.com/user/balance"
        );
        assert_eq!(
            balance_url("https://api.deepseek.com"),
            "https://api.deepseek.com/user/balance"
        );
        assert_eq!(
            balance_url("https://proxy.example/v1/chat/completions/"),
            "https://proxy.example/v1/user/balance"
        );
    }

    #[test]
    fn parses_and_formats_balance_response() {
        let json = r#"{
            "is_available": true,
            "balance_infos": [
                {
                    "currency": "CNY",
                    "total_balance": "110.00",
                    "granted_balance": "10.00",
                    "topped_up_balance": "100.00"
                }
            ]
        }"#;
        let response: BalanceResponse = serde_json::from_str(json).unwrap();
        assert!(response.is_available);
        assert_eq!(response.balance_infos[0].currency, "CNY");
        assert_eq!(response.balance_infos[0].total_balance, "110.00");
        assert_eq!(response.display(), "110.00 CNY");
    }

    #[test]
    fn formats_unavailable_balances() {
        let empty = BalanceResponse {
            is_available: false,
            balance_infos: vec![],
        };
        assert_eq!(empty.display(), "unavailable");

        let insufficient = BalanceResponse {
            is_available: false,
            balance_infos: vec![BalanceInfo {
                currency: "USD".to_string(),
                total_balance: "0.10".to_string(),
            }],
        };
        assert_eq!(
            insufficient.display(),
            "0.10 USD (insufficient for API calls)"
        );
    }
}
