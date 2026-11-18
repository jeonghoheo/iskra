use serde::{Deserialize, Serialize};
use std::time::Duration;

const API_URL: &str = "https://api.deepseek.com/chat/completions";
const MODEL: &str = "deepseek-chat";
pub const MODEL_TIMEOUT_SECS: u64 = 60;

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
    api_key: String,
}

impl Brain {
    pub fn new(api_key: String) -> Self {
        Self {
            client: reqwest::Client::new(),
            api_key,
        }
    }

    pub async fn chat(&self, messages: &[Message]) -> Result<ChatReply, String> {
        let request = ChatRequest {
            model: MODEL,
            messages,
            tools: crate::tools::schemas(),
            tool_choice: "auto",
        };

        let exchange = async {
            let response = self
                .client
                .post(API_URL)
                .bearer_auth(&self.api_key)
                .json(&request)
                .send()
                .await?;
            let status = response.status();
            let body = response.text().await?;
            Ok::<_, reqwest::Error>((status, body))
        };

        let (status, body) = tokio::time::timeout(
            Duration::from_secs(MODEL_TIMEOUT_SECS),
            exchange,
        )
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
