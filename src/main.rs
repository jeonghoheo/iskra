use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    dotenvy::dotenv().ok();
    let key = std::env::var("DEEPSEEK_API_KEY").expect("DEEPSEEK_API_KEY가 .env에 없습니다");

    let body = json!({
        "model": "deepseek-chat",
        "messages": [
            {"role": "user", "content": "한 문장으로 자기소개 해줘"}
        ]
    });

    let res = reqwest::Client::new()
        .post("https://api.deepseek.com/chat/completions")
        .bearer_auth(key)
        .json(&body)
        .send()
        .await?;

    let v: serde_json::Value = res.json().await?;
    println!("{}", v["choices"][0]["message"]["content"]);
    Ok(())
}
