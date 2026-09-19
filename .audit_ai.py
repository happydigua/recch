from pathlib import Path
p = Path('src-tauri/src/ai_service.rs')
s = p.read_text().replace('#[derive(Debug, Serialize, Deserialize, Clone)]', '#[derive(Serialize, Deserialize, Clone)]', 1)
a = s.index('/// Call LLM API')
s = s[:a] + r'''fn endpoint(api_url: &str) -> Result<(reqwest::Url, bool), String> {
    let input = if api_url.trim().is_empty() { DEFAULT_API_URL } else { api_url.trim() };
    let mut url = reqwest::Url::parse(input).map_err(|_| "Invalid AI endpoint URL")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1" | "[::1]"));
    if url.scheme() != "https" && !(url.scheme() == "http" && local) {
        return Err("AI requests require HTTPS, except for explicitly configured loopback services".into());
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err("Do not put credentials or fragments in the AI endpoint URL".into());
    }
    let path = url.path().trim_end_matches('/');
    if !path.ends_with("/chat/completions") { url.set_path(&format!("{path}/chat/completions")); }
    Ok((url, local))
}

fn clean_query(content: &str) -> Result<String, String> {
    let content = content.trim();
    let content = if content.starts_with("```") {
        let (_, body) = content.split_once('\n').ok_or("Incomplete fenced AI response")?;
        body.trim_end().strip_suffix("```").ok_or("Incomplete fenced AI response")?.trim()
    } else { content };
    if content.is_empty() { return Err("AI returned an empty query".into()); }
    Ok(content.to_string())
}

/// AI output is a suggestion only; execution always remains a separate user action.
pub async fn generate_sql(api_key: &str, api_url: &str, model: &str, db_type: &str, table_schemas: &str, user_request: &str) -> Result<String, String> {
    let (url, local) = endpoint(api_url)?;
    let api_key = api_key.trim(); let model = model.trim();
    if api_key.is_empty() && !local { return Err("API Key 未配置。请先在设置中配置 API Key。".into()); }
    if model.is_empty() || user_request.trim().is_empty() { return Err("Model and request must not be empty".into()); }
    let body = ChatRequest { model: model.into(), messages: vec![ChatMessage {
        role: "user".into(), content: build_prompt(db_type, table_schemas, user_request),
    }], temperature: 0.1 };
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(60))
        .redirect(reqwest::redirect::Policy::none())
        .build().map_err(|e| e.to_string())?;
    let mut request = client.post(url).json(&body);
    if !api_key.is_empty() { request = request.bearer_auth(api_key); }
    let mut response = request.send().await.map_err(|e| format!("AI network request failed: {}", e.without_url()))?;
    let status = response.status();
    const MAX_RESPONSE: usize = 1024 * 1024;
    if response.content_length().map(|n| n > MAX_RESPONSE as u64).unwrap_or(false) { return Err("AI response exceeds 1 MiB".into()); }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|e| format!("AI response read failed: {}", e.without_url()))? {
        if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE { return Err("AI response exceeds 1 MiB".into()); }
        bytes.extend_from_slice(&chunk);
    }
    if !status.is_success() {
        if let Ok(error) = serde_json::from_slice::<ErrorResponse>(&bytes) {
            let message = if api_key.is_empty() { error.error.message } else { error.error.message.replace(api_key, "[redacted]") };
            return Err(format!("AI API error ({status}): {}", message.chars().take(512).collect::<String>()));
        }
        return Err(format!("AI API request failed ({status}); response body omitted to protect sensitive data"));
    }
    let result: ChatResponse = serde_json::from_slice(&bytes).map_err(|_| "Invalid AI response format; body omitted to protect sensitive data")?;
    let choice = result.choices.first().ok_or("AI returned no choices")?;
    clean_query(&choice.message.content)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn endpoint_appends_path_without_corrupting_query_parameters() {
        let (url, local) = endpoint("https://example.com/v1?tenant=test").unwrap();
        assert_eq!(url.path(), "/v1/chat/completions"); assert_eq!(url.query(), Some("tenant=test")); assert!(!local);
    }
    #[test]
    fn endpoint_rejects_plaintext_remote_and_embedded_credentials() {
        assert!(endpoint("http://example.com/v1").is_err()); assert!(endpoint("https://user:secret@example.com/v1").is_err());
        assert!(endpoint("file:///tmp/key").is_err()); assert!(endpoint("http://localhost.attacker.test/v1").is_err());
    }
    #[test]
    fn endpoint_supports_loopback_without_double_suffix() {
        let (url, local) = endpoint("http://127.0.0.1:11434/v1/chat/completions/").unwrap();
        assert!(local); assert_eq!(url.path(), "/v1/chat/completions/");
        assert!(endpoint("http://[::1]:11434/v1").unwrap().1);
    }
    #[test]
    fn sql_and_redis_code_fences_are_cleaned_and_empty_output_rejected() {
        assert_eq!(clean_query("```redis\nGET key\n```").unwrap(), "GET key");
        assert_eq!(clean_query("```sql\nSELECT 1;\n```").unwrap(), "SELECT 1;");
        assert!(clean_query("```sql\n```").is_err()); assert!(clean_query("  ").is_err());
    }
}
'''
p.write_text(s)
# Exact numeric columns must enter the text widget as strings, including small bigints.
p = Path('src/components/DataGrid.vue'); s = p.read_text()
marker = 'function hasBinaryColumns()'
a = s.index(marker)
s = s[:a] + '''function inputValue(name: string, value: any) {
    const column = tableMetadata.value.find(c => c.name === name)
    return value !== null && value !== undefined && column && /BIGINT|INT8|DECIMAL|NUMERIC/i.test(column.type_name)
        ? String(value) : editValue(value)
}
''' + s[a:]
s = s.replace('[key, editValue(value)]', '[key, inputValue(key, value)]').replace('value !== editValue(originalRow.value[key])', 'value !== inputValue(key, originalRow.value[key])')
s = s.replace("            return insertQuery(target.table, row, target.dialect)", """            for (const column of tableMetadata.value) {
                const value = row[column.name]
                if (typeof value === 'number' && (/BIGINT|INT8|DECIMAL|NUMERIC/i.test(column.type_name) || (Number.isInteger(value) && !Number.isSafeInteger(value)))) {
                    throw new Error(`Import ${column.name} as a JSON string to preserve exact numeric digits`)
                }
            }
            return insertQuery(target.table, row, target.dialect)""")
p.write_text(s)
print('AI endpoint, timeout, response-size and numeric-edit safeguards applied.')
