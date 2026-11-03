//! Files and typed notes the team keeps about a client company, on the workspace server.
//!
//! A client is named by a key: `crm:<company id>` when the company is in the CRM, otherwise
//! `name:<company>`. At the start of a call with them, their text goes into the coach's cached
//! prompt: one cache write per call, and a longer prompt to read on every hint. Measured on
//! claude-haiku-4-5 (2026-10-27): no change at 5k words; at the 18k-word cap the opening hints'
//! first token came 0.11 s later and each live hint's 0.16 s later.

use serde_json::{json, Value};

use crate::backend::{call_backend, upload_to_backend};

/// `path?client_key=...`, with the key encoded.
fn with_key(path: &str, client_key: &str) -> String {
    let mut url = reqwest::Url::parse("http://backend").expect("static url");
    url.set_path(path);
    url.query_pairs_mut().append_pair("client_key", client_key);
    format!("{}?{}", url.path(), url.query().unwrap_or_default())
}

/// A client's files and notes, newest first, with the words used and the limit.
#[tauri::command]
pub async fn client_documents(client_key: String) -> Result<Value, String> {
    call_backend(
        reqwest::Method::GET,
        &with_key("/clients/documents", &client_key),
        None,
    )
    .await
}

/// Uploads a file. The page sends the file's bytes as the raw request body, so a 10 MB file
/// doesn't go through JSON, and names it in headers (URI-encoded, as headers are ASCII).
#[tauri::command]
pub async fn client_upload(request: tauri::ipc::Request<'_>) -> Result<Value, String> {
    let tauri::ipc::InvokeBody::Raw(bytes) = request.body() else {
        return Err("No file came with the upload".into());
    };
    let header = |name: &str| {
        request
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(uri_decode)
            .unwrap_or_default()
    };
    let (client_key, client_name, filename) = (
        header("client-key"),
        header("client-name"),
        header("file-name"),
    );
    let form = reqwest::multipart::Form::new()
        .text("client_key", client_key)
        .text("client_name", client_name)
        .part(
            "file",
            reqwest::multipart::Part::bytes(bytes.clone()).file_name(filename),
        );
    upload_to_backend("/clients/documents", form).await
}

#[tauri::command]
pub async fn client_add_note(
    client_key: String,
    client_name: String,
    text: String,
) -> Result<Value, String> {
    call_backend(
        reqwest::Method::POST,
        "/clients/notes",
        Some(json!({ "client_key": client_key, "client_name": client_name, "text": text })),
    )
    .await
}

#[tauri::command]
pub async fn client_update_note(id: String, text: String) -> Result<Value, String> {
    call_backend(
        reqwest::Method::PUT,
        &format!("/clients/notes/{}", uri_safe_id(&id)?),
        Some(json!({ "text": text })),
    )
    .await
}

#[tauri::command]
pub async fn client_remove(id: String) -> Result<(), String> {
    call_backend(
        reqwest::Method::DELETE,
        &format!("/clients/documents/{}", uri_safe_id(&id)?),
        None,
    )
    .await
    .map(|_| ())
}

/// Everything kept about the client as one text for the coach; empty when there is nothing.
#[tauri::command]
pub async fn client_context(client_key: String) -> Result<String, String> {
    let context = call_backend(
        reqwest::Method::GET,
        &with_key("/clients/context", &client_key),
        None,
    )
    .await?;
    Ok(context
        .get("text")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

/// Document ids are UUIDs; anything else must not reach a URL path.
fn uri_safe_id(id: &str) -> Result<&str, String> {
    let id = id.trim();
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        Ok(id)
    } else {
        Err("That file or note id isn't valid".into())
    }
}

/// Reverses JavaScript's `encodeURIComponent`.
fn uri_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|pair| std::str::from_utf8(pair).ok())
            .and_then(|pair| u8::from_str_radix(pair, 16).ok());
        match (bytes[i], hex) {
            (b'%', Some(byte)) => {
                out.push(byte);
                i += 3;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_what_encode_uri_component_sends() {
        assert_eq!(
            uri_decode("Pricing%20sheet%20%E2%80%94%20v2.pdf"),
            "Pricing sheet — v2.pdf"
        );
        assert_eq!(uri_decode("name%3Aacme%20%26%20co"), "name:acme & co");
        assert_eq!(uri_decode("100%"), "100%");
    }

    #[test]
    fn the_key_is_encoded_in_the_query() {
        assert_eq!(
            with_key("/clients/context", "name:acme & co"),
            "/clients/context?client_key=name%3Aacme+%26+co"
        );
    }

    #[test]
    fn only_uuids_go_into_a_path() {
        assert!(uri_safe_id("7767e2f6-1fa7-4d5b-aea7-84194c8214fe").is_ok());
        assert!(uri_safe_id("../settings").is_err());
        assert!(uri_safe_id("").is_err());
    }
}
