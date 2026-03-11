//! Google Docs and Sheets integration via service account.
//! Users share docs/sheets with the service account email; we fetch content and return it for indexing.

use std::env;
use std::path::Path;

use anyhow::{Context, Result};
use yup_oauth2::ServiceAccountAuthenticator;

const DOCS_SCOPE: &str = "https://www.googleapis.com/auth/documents.readonly";
const SHEETS_SCOPE: &str = "https://www.googleapis.com/auth/spreadsheets.readonly";

/// Load service account key from env:
/// - GOOGLE_APPLICATION_CREDENTIALS: path to JSON key file
/// - MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON: inline JSON string
pub async fn load_service_account_key() -> Result<yup_oauth2::ServiceAccountKey> {
    if let Ok(json) = env::var("MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON") {
        return yup_oauth2::parse_service_account_key(json.as_bytes())
            .context("parse MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON");
    }
    if let Ok(path) = env::var("GOOGLE_APPLICATION_CREDENTIALS") {
        return yup_oauth2::read_service_account_key(Path::new(&path))
            .await
            .context("read GOOGLE_APPLICATION_CREDENTIALS file");
    }
    anyhow::bail!(
        "Google integration not configured: set GOOGLE_APPLICATION_CREDENTIALS (path to JSON key) \
         or MEMKIT_GOOGLE_SERVICE_ACCOUNT_JSON (inline JSON)"
    );
}

/// Authenticator type returned by build_google_authenticator (opaque for storage in AppState).
pub type GoogleAuthenticator = yup_oauth2::authenticator::Authenticator<
    hyper_rustls::HttpsConnector<hyper_util::client::legacy::connect::HttpConnector>,
>;

/// Build an authenticator from a service account key. Caller can cache this.
pub async fn build_google_authenticator(
    key: yup_oauth2::ServiceAccountKey,
) -> Result<GoogleAuthenticator> {
    let auth = ServiceAccountAuthenticator::builder(key)
        .build()
        .await
        .context("build Google service account authenticator")?;
    Ok(auth)
}

/// Returns the service account client_email from the key, for display to users (share doc with this email).
pub fn service_account_email_from_key(key: &yup_oauth2::ServiceAccountKey) -> &str {
    &key.client_email
}

/// Obtain a Bearer token for Docs + Sheets APIs. Uses the given authenticator (cached by caller).
pub async fn get_access_token(auth: &GoogleAuthenticator) -> Result<String> {
    let scopes: &[&str] = &[DOCS_SCOPE, SHEETS_SCOPE];
    let token = auth
        .token(scopes)
        .await
        .context("obtain Google access token")?;
    Ok(token
        .token()
        .ok_or_else(|| anyhow::anyhow!("missing access token"))?
        .to_string())
}

/// Parse document ID from a Google Docs URL or return the value as-is if it looks like a raw ID.
/// URL format: https://docs.google.com/document/d/{documentId}/edit
pub fn parse_doc_id(value: &str) -> Option<String> {
    let value = value.trim();
    if value.contains("docs.google.com/document/d/") {
        let start = value.find("/d/")? + 3;
        let rest = &value[start..];
        let end = rest.find('/').unwrap_or(rest.len());
        let id = &rest[..end];
        if id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Some(id.to_string());
        }
    }
    if !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Some(value.to_string());
    }
    None
}

/// Parse spreadsheet ID and optional sheet gid from a Google Sheets URL, or return (value, None) as raw spreadsheet ID.
/// URL format: https://docs.google.com/spreadsheets/d/{spreadsheetId}/edit#gid={sheetId}
pub fn parse_sheet_ids(value: &str) -> Option<(String, Option<u64>)> {
    let value = value.trim();
    if value.contains("docs.google.com/spreadsheets/d/") {
        let start = value.find("/d/")? + 3;
        let rest = &value[start..];
        let end = rest.find('/').unwrap_or(rest.len());
        let spreadsheet_id = rest[..end].to_string();
        if !spreadsheet_id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return None;
        }
        let gid = value
            .find("#gid=")
            .and_then(|i| value[i + 5..].split_terminator(&['&', '?', '#'][..]).next())
            .and_then(|s| s.parse::<u64>().ok());
        return Some((spreadsheet_id, gid));
    }
    if !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        return Some((value.to_string(), None));
    }
    None
}

/// Fetch a Google Doc and extract plain text. Returns (content, source_path).
pub async fn fetch_doc_content(
    document_id: &str,
    access_token: &str,
) -> Result<(String, String)> {
    let url = format!("https://docs.googleapis.com/v1/documents/{}", document_id);
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("build HTTP client")?;
    let resp = client
        .get(&url)
        .bearer_auth(access_token)
        .send()
        .await
        .context("fetch Google Doc")?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "Google Docs API error ({}): {}",
            status,
            if body.is_empty() { "check doc is shared with service account" } else { &body }
        );
    }
    let json: serde_json::Value = resp.json().await.context("parse Docs API response")?;
    let body = json
        .get("body")
        .and_then(|b| b.get("content"))
        .and_then(|c| c.as_array())
        .context("missing body.content")?;
    let mut text_parts: Vec<String> = Vec::new();
    for elem in body {
        if let Some(para) = elem.get("paragraph") {
            if let Some(elements) = para.get("elements").and_then(|e| e.as_array()) {
                for e in elements {
                    if let Some(content) = e.get("textRun").and_then(|r| r.get("content")).and_then(|c| c.as_str()) {
                        text_parts.push(content.to_string());
                    }
                }
            }
        } else if let Some(table) = elem.get("table") {
            if let Some(rows) = table.get("tableRows").and_then(|r| r.as_array()) {
                for row in rows {
                    if let Some(cells) = row.get("tableCells").and_then(|c| c.as_array()) {
                        let row_texts: Vec<String> = cells
                            .iter()
                            .filter_map(|cell| {
                                cell.get("content")
                                    .and_then(|c| c.as_array())
                                    .and_then(|arr| {
                                        arr.iter().filter_map(extract_text_from_struct).next()
                                    })
                            })
                            .collect();
                        if !row_texts.is_empty() {
                            text_parts.push(row_texts.join("\t"));
                        }
                    }
                }
            }
        }
    }
    let content = text_parts.join("").trim().to_string();
    let source_path = format!("memkit://google/doc/{}", document_id);
    Ok((content, source_path))
}

fn extract_text_from_struct(v: &serde_json::Value) -> Option<String> {
    let para = v.get("paragraph")?.get("elements")?.as_array()?;
    let parts: Vec<&str> = para
        .iter()
        .filter_map(|e| e.get("textRun").and_then(|r| r.get("content")).and_then(|c| c.as_str()))
        .collect();
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(""))
    }
}

/// Fetch a Google Sheet and return one (content, source_path) per sheet. If gid is Some, only that sheet.
pub async fn fetch_sheet_content(
    spreadsheet_id: &str,
    gid: Option<u64>,
    access_token: &str,
) -> Result<Vec<(String, String)>> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .context("build HTTP client")?;

    let meta_url = format!("https://sheets.googleapis.com/v4/spreadsheets/{}", spreadsheet_id);
    let meta_resp = client
        .get(&meta_url)
        .bearer_auth(access_token)
        .send()
        .await
        .context("fetch spreadsheet metadata")?;
    if !meta_resp.status().is_success() {
        let status = meta_resp.status();
        let body = meta_resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "Google Sheets API error ({}): {}",
            status,
            if body.is_empty() { "check sheet is shared with service account" } else { &body }
        );
    }
    let meta: serde_json::Value = meta_resp.json().await.context("parse Sheets metadata")?;
    let sheets = meta
        .get("sheets")
        .and_then(|s| s.as_array())
        .context("missing sheets")?;

    let mut out = Vec::new();
    for sheet in sheets {
        let props = sheet.get("properties").context("sheet missing properties")?;
        let sheet_id = props.get("sheetId").and_then(|v| v.as_i64()).context("sheetId")? as u64;
        if let Some(want_gid) = gid {
            if sheet_id != want_gid {
                continue;
            }
        }
        let title = props
            .get("title")
            .and_then(|t| t.as_str())
            .unwrap_or("Sheet1");
        let range = format!("{}!A1:ZZ1000", title);
        let values_url = format!(
            "https://sheets.googleapis.com/v4/spreadsheets/{}/values/{}",
            spreadsheet_id,
            urlencoding::encode(&range)
        );
        let values_resp = client
            .get(&values_url)
            .bearer_auth(access_token)
            .send()
            .await
            .context("fetch sheet values")?;
        if !values_resp.status().is_success() {
            continue;
        }
        let values_json: serde_json::Value = values_resp.json().await.unwrap_or_default();
        let empty: Vec<serde_json::Value> = vec![];
        let rows = values_json
            .get("values")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);
        let mut csv_lines = Vec::new();
        for row in rows {
            let empty_row: Vec<serde_json::Value> = vec![];
            let cells: Vec<String> = row
                .as_array()
                .unwrap_or(&empty_row)
                .iter()
                .map(|c| {
                    let s = c.as_str().unwrap_or("");
                    if s.contains(',') || s.contains('"') || s.contains('\n') {
                        format!("\"{}\"", s.replace('"', "\"\""))
                    } else {
                        s.to_string()
                    }
                })
                .collect();
            csv_lines.push(cells.join(","));
        }
        let content = csv_lines.join("\n");
        let source_path = format!("memkit://google/sheet/{}/{}", spreadsheet_id, sheet_id);
        out.push((content, source_path));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_doc_id() {
        assert_eq!(
            parse_doc_id("https://docs.google.com/document/d/1AbCDeFGhiJKlmnOPQRsTuvWxyZ/edit"),
            Some("1AbCDeFGhiJKlmnOPQRsTuvWxyZ".to_string())
        );
        assert_eq!(parse_doc_id("1AbCDeFGhiJKlmnOPQRsTuvWxyZ"), Some("1AbCDeFGhiJKlmnOPQRsTuvWxyZ".to_string()));
    }

    #[test]
    fn test_parse_sheet_ids() {
        let (id, gid) = parse_sheet_ids("https://docs.google.com/spreadsheets/d/abc123/edit#gid=0").unwrap();
        assert_eq!(id, "abc123");
        assert_eq!(gid, Some(0));
        let (id, gid) = parse_sheet_ids("abc123").unwrap();
        assert_eq!(id, "abc123");
        assert_eq!(gid, None);
    }
}
