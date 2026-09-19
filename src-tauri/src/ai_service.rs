use serde::{Deserialize, Serialize};

const DEFAULT_API_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions";

#[derive(Serialize, Deserialize, Clone)]
pub struct AIConfig {
    pub api_key: String,
    pub api_url: String,
    pub model: String,
}

impl Default for AIConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            api_url: DEFAULT_API_URL.to_string(),
            model: "qwen-turbo".to_string(),
        }
    }
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessageResponse,
}

#[derive(Debug, Deserialize)]
struct ChatMessageResponse {
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

/// Build the system prompt for Text-to-SQL
fn build_prompt(db_type: &str, table_schemas: &str, user_request: &str) -> String {
    let specific_instruction = match db_type.to_lowercase().as_str() {
        "redis" => "注意：这是一个 Redis 数据库。请返回 Redis CLI 命令（如 GET, HGETALL, LRANGE 等），而不是 SQL。",
        "postgresql" => "注意：使用 PostgreSQL 方言（如使用双引号引用标识符，日期函数等）。",
        "mysql" => "注意：使用 MySQL 方言（如使用反引号引用标识符）。",
        _ => "使用标准 SQL 语法。"
    };

    format!(
        r#"你是一个数据库查询专家。根据以下信息生成准确的查询语句。

## 目标数据库类型
**{db_type}**

## 关键指令
{specific_instruction}

## 表结构/Schema 信息
{table_schemas}

## 用户需求
{user_request}

##不仅要生成 SQL，如果是 Redis 请生成 Redis 命令。
## 输出要求
1. **只返回** 最终的查询语句 (SQL 或 Redis 命令)
2. **不要** 包含 Markdown 标记（如 ```sql ... ```），不要包含解释性文字
3. 确保语法符合目标数据库版本要求
4. 如果是 SQL，尽量使用优化的高效查询"#
    )
}

pub fn endpoint(api_url: &str) -> Result<reqwest::Url, String> {
    let mut url = reqwest::Url::parse(if api_url.trim().is_empty() {
        DEFAULT_API_URL
    } else {
        api_url.trim()
    })
    .map_err(|_| "AI endpoint must be a valid HTTPS URL (HTTP is allowed only for loopback)")?;
    let loopback = match url.host_str().unwrap_or("") {
        "localhost" | "[::1]" | "::1" => true,
        host => host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
    };
    if !(url.scheme() == "https" || url.scheme() == "http" && loopback)
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("Use HTTPS for remote AI; credentials, query strings and fragments are not allowed in the endpoint".into());
    }
    let path = url.path().trim_end_matches('/');
    let path = if path.ends_with("/chat/completions") {
        path.to_string()
    } else {
        format!("{}/chat/completions", path)
    };
    url.set_path(&path);
    Ok(url)
}

fn clean_reply(content: &str) -> Result<String, String> {
    let text = content.trim();
    let text = if text.starts_with("```") && text.ends_with("```") {
        text.split_once('\n')
            .map(|(_, body)| body.trim_end_matches("```").trim())
            .unwrap_or("")
    } else {
        text
    };
    if text.is_empty() {
        Err("AI returned no query; nothing was executed".into())
    } else {
        Ok(text.to_string())
    }
}

/// Generate only; the user must review and explicitly execute the result.
pub async fn generate_sql(
    api_key: &str,
    api_url: &str,
    model: &str,
    db_type: &str,
    table_schemas: &str,
    user_request: &str,
) -> Result<String, String> {
    let url = endpoint(api_url)?;
    if model.trim().is_empty() || model.len() > 256 || user_request.trim().is_empty() {
        return Err("AI model and request must be nonempty".into());
    }
    if table_schemas.len() > 524_288 || user_request.len() > 65_536 {
        return Err("AI request is too large; reduce the schema or prompt".into());
    }
    let request_body = ChatRequest {
        model: model.trim().into(),
        messages: vec![ChatMessage {
            role: "user".into(),
            content: build_prompt(db_type, table_schemas, user_request),
        }],
        temperature: 0.1,
    };
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Cannot initialize AI HTTP client")?;
    let mut request = client.post(url).json(&request_body);
    if !api_key.trim().is_empty() {
        request = request.bearer_auth(api_key.trim());
    }
    let mut response = request
        .send()
        .await
        .map_err(|_| "AI connection failed or timed out; check endpoint and network")?;
    if !response.status().is_success() {
        // Providers may echo prompts or credentials in errors. Do not reflect them.
        return Err(format!(
            "AI request failed (HTTP {}); check model, endpoint and credentials",
            response.status().as_u16()
        ));
    }
    const MAX_RESPONSE: usize = 2 * 1024 * 1024;
    let mut bytes = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "AI response failed or timed out")?
    {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE {
            return Err("AI response exceeds 2 MiB limit".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let result: ChatResponse =
        serde_json::from_slice(&bytes).map_err(|_| "AI returned an invalid response")?;
    clean_reply(
        &result
            .choices
            .first()
            .ok_or("AI returned no choices")?
            .message
            .content,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_rejects_remote_cleartext_and_embedded_secrets() {
        for value in [
            "http://example.com/v1",
            "https://u:p@example.com/v1",
            "https://example.com/v1?key=secret",
            "file:///tmp/a",
            "https://example.com/#fragment",
        ] {
            assert!(endpoint(value).is_err(), "{value}");
        }
        for value in [
            "http://localhost:11434/v1",
            "http://127.0.0.1:8000/v1",
            "http://[::1]:8000/v1",
            "https://example.com/v1",
        ] {
            assert!(endpoint(value).is_ok(), "{value}");
        }
        assert_eq!(
            endpoint("https://example.com/v1/").unwrap().as_str(),
            "https://example.com/v1/chat/completions"
        );
    }
    #[test]
    fn clean_reply_preserves_sql_and_rejects_empty_response() {
        assert_eq!(clean_reply("```redis\nGET key\n```").unwrap(), "GET key");
        assert_eq!(clean_reply(" SELECT 1; ").unwrap(), "SELECT 1;");
        assert!(clean_reply("```sql\n\n```").is_err());
    }
}
