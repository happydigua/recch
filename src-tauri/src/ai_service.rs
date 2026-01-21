use serde::{Deserialize, Serialize};

const DEFAULT_API_URL: &str = "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions";

#[derive(Debug, Serialize, Deserialize, Clone)]
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

#[derive(Debug, Deserialize)]
struct ErrorResponse {
    error: ErrorDetail,
}

#[derive(Debug, Deserialize)]
struct ErrorDetail {
    message: String,
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

/// Call LLM API to generate SQL (OpenAI-compatible)
pub async fn generate_sql(
    api_key: &str,
    api_url: &str,
    model: &str,
    db_type: &str,
    table_schemas: &str,
    user_request: &str,
) -> Result<String, String> {
    if api_key.is_empty() {
        return Err("API Key 未配置。请先在设置中配置 API Key。".to_string());
    }
    
    let mut url = if api_url.trim().is_empty() { DEFAULT_API_URL.to_string() } else { api_url.trim().to_string() };
    
    // Auto-fix URL if user provided base URL only
    if !url.ends_with("/chat/completions") && !url.ends_with("/chat/completions/") {
        if url.ends_with("/") {
            url.push_str("chat/completions");
        } else {
            url.push_str("/chat/completions");
        }
    }
    
    let api_key = api_key.trim();
    let model = model.trim();

    println!("Attempting AI Request:");
    println!("URL: {}", url);
    println!("Model: {}", model);
    // don't print api_key for security

    let prompt = build_prompt(db_type, table_schemas, user_request);

    let request_body = ChatRequest {
        model: model.to_string(),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: prompt,
        }],
        temperature: 0.1, 
    };

    let client = reqwest::Client::new();
    let response = client
        .post(url)
        .header("Authorization", format!("Bearer {}", api_key))
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await
        .map_err(|e| format!("网络请求失败: {}", e))?;

    let status = response.status();
    let response_text = response.text().await.map_err(|e| format!("读取响应失败: {}", e))?;

    if !status.is_success() {
        // Try to parse error response
        if let Ok(error_resp) = serde_json::from_str::<ErrorResponse>(&response_text) {
            return Err(format!("API 错误: {}", error_resp.error.message));
        }
        return Err(format!("API 请求失败 ({}): {}", status, response_text));
    }

    let chat_response: ChatResponse = serde_json::from_str(&response_text)
        .map_err(|e| format!("解析响应失败: {} - {}", e, response_text))?;

    if let Some(choice) = chat_response.choices.first() {
        let sql = choice.message.content.trim().to_string();
        // Clean up potential markdown code blocks
        let sql = sql
            .trim_start_matches("```sql")
            .trim_start_matches("```")
            .trim_end_matches("```")
            .trim()
            .to_string();
        Ok(sql)
    } else {
        Err("API 未返回有效内容".to_string())
    }
}
