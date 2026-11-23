use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const DEFAULT_BASE_URL: &str = "https://api.deepseek.com/chat/completions";
pub const DEFAULT_MODEL: &str = "deepseek-flash";
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
}

impl Message {
    pub fn system(content: &str) -> Self {
        Self {
            role: "system".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn user(content: &str) -> Self {
        Self {
            role: "user".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn assistant(content: &str) -> Self {
        Self {
            role: "assistant".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
        }
    }

    pub fn tool(tool_call_id: &str, content: &str) -> Self {
        Self {
            role: "tool".to_string(),
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.to_string()),
            reasoning_content: None,
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

#[derive(Serialize, Clone, Copy, Debug, Default)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

impl<'de> Deserialize<'de> for Usage {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default)]
            prompt_tokens: u64,
            #[serde(default)]
            completion_tokens: u64,
            #[serde(default)]
            total_tokens: u64,
        }

        let raw = Raw::deserialize(deserializer)?;
        let total_tokens = if raw.total_tokens == 0 {
            raw.prompt_tokens + raw.completion_tokens
        } else {
            raw.total_tokens
        };
        Ok(Self {
            prompt_tokens: raw.prompt_tokens,
            completion_tokens: raw.completion_tokens,
            total_tokens,
        })
    }
}

pub struct ChatReply {
    pub message: Message,
    pub usage: Option<Usage>,
}

#[derive(Debug)]
pub enum ChatError {
    Timeout(String),
    Connection(String),
    Auth(String),
    Payment(String),
    Busy(String),
    Other(String),
}

impl std::fmt::Display for ChatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let detail = match self {
            Self::Timeout(detail)
            | Self::Connection(detail)
            | Self::Auth(detail)
            | Self::Payment(detail)
            | Self::Busy(detail)
            | Self::Other(detail) => detail,
        };
        f.write_str(detail)
    }
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

    pub async fn chat(&self, messages: &[Message]) -> Result<ChatReply, ChatError> {
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
            match tokio::time::timeout(Duration::from_secs(MODEL_TIMEOUT_SECS), exchange).await {
                Err(_) => {
                    return Err(ChatError::Timeout(format!(
                        "Model call timed out after {MODEL_TIMEOUT_SECS}s"
                    )));
                }
                Ok(Err(e)) => {
                    return Err(ChatError::Connection(format!("Model call failed: {e}")));
                }
                Ok(Ok(exchange)) => exchange,
            };

        if !status.is_success() {
            return Err(classify_status(status, &body));
        }

        let parsed: ChatResponse = serde_json::from_str(&body)
            .map_err(|e| ChatError::Other(format!("Failed to parse response JSON: {e}")))?;
        let choice = parsed
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| ChatError::Other("Model response has no choices".to_string()))?;

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

fn classify_status(status: reqwest::StatusCode, body: &str) -> ChatError {
    let detail = format!("Model API error {status}: {}", truncate_chars(body, 300));
    match status.as_u16() {
        401 | 403 => ChatError::Auth(detail),
        402 => ChatError::Payment(detail),
        429 => ChatError::Busy(detail),
        500..=599 => ChatError::Busy(detail),
        _ => ChatError::Other(detail),
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
        assert_eq!(DEFAULT_MODEL, "deepseek-flash");
    }

    #[test]
    fn keeps_reasoning_content_in_followup_request() {
        let reply_json = r#"{
            "role": "assistant",
            "content": "Let me read it.",
            "reasoning_content": "I should call read_file.",
            "tool_calls": [{
                "id": "call_1",
                "type": "function",
                "function": { "name": "read_file", "arguments": "{\"path\":\"a.txt\"}" }
            }]
        }"#;
        let reply: Message = serde_json::from_str(reply_json).unwrap();
        assert_eq!(
            reply.reasoning_content.as_deref(),
            Some("I should call read_file.")
        );

        let mut following = vec![Message::system("sys"), Message::user("read a.txt")];
        following.push(reply);
        following.push(Message::tool("call_1", "hello"));
        let body = serde_json::to_string(&following).unwrap();
        assert!(body.contains(r#""reasoning_content":"I should call read_file.""#));
        assert!(!body.contains(r#""reasoning_content":"""#));
    }

    #[test]
    fn omits_reasoning_content_when_absent() {
        let reply: Message =
            serde_json::from_str(r#"{"role":"assistant","content":"done"}"#).unwrap();
        assert!(reply.reasoning_content.is_none());
        let body = serde_json::to_string(&reply).unwrap();
        assert!(!body.contains("reasoning_content"));
        assert!(!body.contains("null"));

        let system = Message::system("sys");
        let user = Message::user("hi");
        let assistant = Message::assistant("hello");
        let tool = Message::tool("call_1", "out");
        for message in [system, user, assistant, tool] {
            assert!(message.reasoning_content.is_none());
            let body = serde_json::to_string(&message).unwrap();
            assert!(!body.contains("reasoning_content"));
        }
    }

    #[test]
    fn usage_defaults_and_falls_back_to_prompt_plus_completion() {
        let missing_total: Usage =
            serde_json::from_str(r#"{"prompt_tokens":10,"completion_tokens":5}"#).unwrap();
        assert_eq!(missing_total.total_tokens, 15);

        let zero_total: Usage =
            serde_json::from_str(r#"{"prompt_tokens":7,"completion_tokens":2,"total_tokens":0}"#)
                .unwrap();
        assert_eq!(zero_total.total_tokens, 9);

        let explicit_total: Usage =
            serde_json::from_str(r#"{"prompt_tokens":7,"completion_tokens":2,"total_tokens":30}"#)
                .unwrap();
        assert_eq!(explicit_total.total_tokens, 30);

        let empty: Usage = serde_json::from_str("{}").unwrap();
        assert_eq!(
            (
                empty.prompt_tokens,
                empty.completion_tokens,
                empty.total_tokens
            ),
            (0, 0, 0)
        );
    }

    #[test]
    fn chat_response_parses_without_usage() {
        let parsed: ChatResponse = serde_json::from_str(
            r#"{"choices":[{"message":{"role":"assistant","content":"ok"}}]}"#,
        )
        .unwrap();
        assert!(parsed.usage.is_none());

        let parsed: ChatResponse = serde_json::from_str(
            r#"{"choices":[{"message":{"role":"assistant","content":"ok"}}],"usage":{"prompt_tokens":3,"completion_tokens":4}}"#,
        )
        .unwrap();
        assert_eq!(parsed.usage.unwrap().total_tokens, 7);
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
    fn classifies_http_statuses() {
        use reqwest::StatusCode;

        assert!(matches!(
            classify_status(StatusCode::UNAUTHORIZED, "nope"),
            ChatError::Auth(_)
        ));
        assert!(matches!(
            classify_status(StatusCode::FORBIDDEN, "nope"),
            ChatError::Auth(_)
        ));
        assert!(matches!(
            classify_status(StatusCode::PAYMENT_REQUIRED, "nope"),
            ChatError::Payment(_)
        ));
        assert!(matches!(
            classify_status(StatusCode::TOO_MANY_REQUESTS, "nope"),
            ChatError::Busy(_)
        ));
        assert!(matches!(
            classify_status(StatusCode::INTERNAL_SERVER_ERROR, "nope"),
            ChatError::Busy(_)
        ));
        assert!(matches!(
            classify_status(StatusCode::BAD_GATEWAY, "nope"),
            ChatError::Busy(_)
        ));
        assert!(matches!(
            classify_status(StatusCode::NOT_FOUND, "nope"),
            ChatError::Other(_)
        ));

        let detail = classify_status(StatusCode::UNAUTHORIZED, "raw body").to_string();
        assert!(detail.contains("401"));
        assert!(detail.contains("raw body"));
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
