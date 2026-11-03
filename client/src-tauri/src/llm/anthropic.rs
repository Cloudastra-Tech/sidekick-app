//! Anthropic Messages API (raw HTTP; there is no official Rust SDK) with structured JSON
//! output and automatic prompt caching.

use super::sse::{SseReader, StreamEvent};
use super::{Access, LlmRequest, LlmResponse, Role, Usage};
use serde::Deserialize;
use serde_json::json;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const API_URL: &str = "https://api.anthropic.com/v1/messages";
const API_VERSION: &str = "2023-06-01";

pub async fn generate(
    client: &reqwest::Client,
    access: &Access,
    model: &str,
    request: &LlmRequest<'_>,
) -> Result<LlmResponse, String> {
    let body = request_body(model, request);
    let deadline = Instant::now() + request.timeout;
    hedged(client, request.hedge_after, |pool| {
        let body = &body;
        async move { with_retry(deadline, || send_once(&pool, access, body, deadline)).await }
    })
    .await
}

fn request_body(model: &str, request: &LlmRequest<'_>) -> serde_json::Value {
    let messages: Vec<_> = request
        .turns
        .iter()
        .map(|turn| {
            let role = match turn.role {
                Role::User => "user",
                Role::Model => "assistant",
            };
            json!({ "role": role, "content": turn.text })
        })
        .collect();

    let mut output_config = json!({
        "format": { "type": "json_schema", "schema": request.schema }
    });
    if let Some(effort) = request.effort {
        output_config["effort"] = json!(effort);
    }
    let mut body = json!({
        "model": model,
        "max_tokens": request.max_tokens,
        "system": [{ "type": "text", "text": request.system }],
        "messages": messages,
        "output_config": output_config,
    });
    if let Some(temperature) = request.temperature {
        body["temperature"] = json!(temperature);
    }
    if request.cache {
        // Top-level cache_control places the breakpoint on the last block, so each request
        // caches the whole conversation and the next one reads it back. Prefixes under the
        // model's minimum (4096 tokens on Haiku 4.5) silently don't cache.
        body["cache_control"] = json!({ "type": "ephemeral" });
    }
    body
}

/// Runs `start` on one connection pool; with `hedge_after`, if it hasn't succeeded by then, runs
/// it again on the other pool and takes whichever succeeds first.
///
/// No answer by `hedge_after`, which a healthy request almost never reaches: send the same
/// request again on a fresh connection and take whichever answers first. A stalled connection
/// (a dead socket through Cloudflare, a slow proxy) otherwise held the coach for the whole
/// timeout and lost the hint. The copy reads the same cached prompt, so it costs little.
async fn hedged<T, F, Fut>(
    client: &reqwest::Client,
    hedge_after: Option<Duration>,
    start: F,
) -> Result<T, String>
where
    F: Fn(reqwest::Client) -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    // After a hedge wins, its pool leads: the other pool's connection may still be stuck, and
    // every hint sent down it would wait for the hedge again.
    let other = crate::backend::hedge_client().unwrap_or_else(|_| client.clone());
    let hedge_leads = HEDGE_POOL_LEADS.load(Ordering::Relaxed);
    let (first_pool, second_pool) = if hedge_leads {
        (other, client.clone())
    } else {
        (client.clone(), other)
    };
    let primary = start(first_pool);
    let Some(hedge_after) = hedge_after else {
        return primary.await;
    };
    tokio::pin!(primary);
    tokio::select! {
        result = &mut primary => return result,
        _ = tokio::time::sleep(hedge_after) => {}
    }
    let hedge = start(second_pool);
    tokio::pin!(hedge);
    tokio::select! {
        result = &mut primary => match result {
            Ok(response) => Ok(response),
            Err(first) => {
                let result = hedge.await.map_err(|_| first);
                if result.is_ok() {
                    HEDGE_POOL_LEADS.store(!hedge_leads, Ordering::Relaxed);
                }
                result
            }
        },
        result = &mut hedge => match result {
            Ok(response) => {
                HEDGE_POOL_LEADS.store(!hedge_leads, Ordering::Relaxed);
                Ok(response)
            }
            Err(_) => primary.await,
        },
    }
}

/// Which of the two connection pools sends a request first.
static HEDGE_POOL_LEADS: AtomicBool = AtomicBool::new(false);

/// How long to wait before the one retry, so a server that just said "overloaded" isn't asked
/// again in the same instant.
const RETRY_PAUSE: Duration = Duration::from_millis(200);

/// One request, tried a second time when the failure says a retry could work and at least half
/// the time is left, so a hint never arrives late because of a retry.
async fn with_retry<T, F, Fut>(deadline: Instant, mut send: F) -> Result<T, String>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, Failure>>,
{
    let budget = deadline.saturating_duration_since(Instant::now());
    let mut tries = 0;
    loop {
        tries += 1;
        match send().await {
            Ok(response) => return Ok(response),
            Err(failure) => {
                let left = deadline.saturating_duration_since(Instant::now());
                if !(failure.retry && tries == 1 && left * 2 > budget) {
                    return Err(failure.message);
                }
                eprintln!("{}; trying once more", failure.message);
                tokio::time::sleep(RETRY_PAUSE).await;
            }
        }
    }
}

struct Failure {
    message: String,
    /// Whether the same request could plausibly succeed straight away.
    retry: bool,
}

async fn send_once(
    client: &reqwest::Client,
    access: &Access,
    body: &serde_json::Value,
    deadline: Instant,
) -> Result<LlmResponse, Failure> {
    // Through the backend when signed in (it holds the key and applies spend limits), straight
    // to Anthropic otherwise. The body is identical either way, so prompt caching is unaffected.
    let request = match access {
        Access::Direct { api_key } => client
            .post(API_URL)
            .header("x-api-key", api_key)
            .header("anthropic-version", API_VERSION),
        Access::Backend { url, token } => client
            .post(format!("{url}/llm/messages"))
            .bearer_auth(token),
    };
    let left = deadline.saturating_duration_since(Instant::now());
    super::REQUESTS_SENT.fetch_add(1, Ordering::Relaxed);
    // Retried: the request never got out (a socket that failed or a connect that hung), which
    // a flaky connection does to the first request of a call. Not retried: a timeout once the
    // request was out, since the moment has passed and the model may well have answered.
    let response = request
        .timeout(left)
        .json(body)
        .send()
        .await
        .map_err(|e| Failure {
            retry: e.is_connect() || !e.is_timeout(),
            message: format!("Claude request failed: {e}"),
        })?;

    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        return Err(Failure {
            // Rate limited, overloaded, or a proxy/Cloudflare hiccup: worth one more try. Not a
            // bad request, a spend limit or a rejected key.
            retry: matches!(status.as_u16(), 408 | 429 | 500 | 502..=504 | 520..=524 | 529),
            message: format!(
                "Claude HTTP {status}: {}",
                detail.chars().take(300).collect::<String>()
            ),
        });
    }
    let parsed: MessagesResponse = response.json().await.map_err(|e| Failure {
        retry: false,
        message: format!("Unexpected Claude response: {e}"),
    })?;
    into_response(parsed).map_err(|message| Failure {
        retry: false,
        message,
    })
}

fn into_response(parsed: MessagesResponse) -> Result<LlmResponse, String> {
    match parsed.stop_reason.as_deref() {
        Some("refusal") => return Err("Claude declined to answer this request".into()),
        Some("max_tokens") => return Err("Claude response was cut off at max_tokens".into()),
        _ => {}
    }
    let text: String = parsed
        .content
        .into_iter()
        .filter(|block| block.kind == "text")
        .filter_map(|block| block.text)
        .collect();
    if text.trim().is_empty() {
        return Err("Claude returned no text".into());
    }

    let usage = parsed.usage;
    Ok(LlmResponse {
        text,
        usage: Usage {
            input_tokens: usage.input_tokens
                + usage.cache_creation_input_tokens
                + usage.cache_read_input_tokens,
            cache_read_tokens: usage.cache_read_input_tokens,
            cache_write_tokens: usage.cache_creation_input_tokens,
            output_tokens: usage.output_tokens,
        },
    })
}

#[derive(Deserialize)]
struct MessagesResponse {
    #[serde(default)]
    content: Vec<ContentBlock>,
    stop_reason: Option<String>,
    usage: ApiUsage,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    kind: String,
    text: Option<String>,
}

#[derive(Deserialize)]
struct ApiUsage {
    /// Uncached remainder only.
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
}

/// Like `generate`, but Claude streams the answer and `on_text` gets each piece as it arrives.
/// The hedge fires when no words have arrived by `hedge_after` (a stream that connected but
/// stays silent is as stuck as a request with no answer).
pub async fn stream(
    client: &reqwest::Client,
    access: &Access,
    model: &str,
    request: &LlmRequest<'_>,
    on_text: &mut (dyn FnMut(&str) + Send),
) -> Result<LlmResponse, String> {
    let mut body = request_body(model, request);
    body["stream"] = json!(true);
    let deadline = Instant::now() + request.timeout;
    let opened = hedged(client, request.hedge_after, |pool| {
        let body = &body;
        async move { with_retry(deadline, || open_stream(&pool, access, body, deadline)).await }
    })
    .await?;
    finish_stream(opened, on_text).await
}

/// A stream that has started answering: its first words are in `pending`.
struct OpenedStream {
    response: reqwest::Response,
    reader: SseReader,
    pending: Vec<StreamEvent>,
}

/// Sends a streaming request and reads until the first words (or the end) arrive, so the hedge
/// races on time to first words.
async fn open_stream(
    client: &reqwest::Client,
    access: &Access,
    body: &serde_json::Value,
    deadline: Instant,
) -> Result<OpenedStream, Failure> {
    let request = match access {
        Access::Direct { api_key } => client
            .post(API_URL)
            .header("x-api-key", api_key)
            .header("anthropic-version", API_VERSION),
        Access::Backend { url, token } => client
            .post(format!("{url}/llm/messages/stream"))
            .bearer_auth(token),
    };
    let left = deadline.saturating_duration_since(Instant::now());
    super::REQUESTS_SENT.fetch_add(1, Ordering::Relaxed);
    let mut response = request
        .timeout(left)
        .json(body)
        .send()
        .await
        .map_err(|e| Failure {
            retry: e.is_connect() || !e.is_timeout(),
            message: format!("Claude request failed: {e}"),
        })?;
    let status = response.status();
    if !status.is_success() {
        let detail = response.text().await.unwrap_or_default();
        return Err(Failure {
            retry: matches!(status.as_u16(), 408 | 429 | 500 | 502..=504 | 520..=524 | 529),
            message: format!(
                "Claude HTTP {status}: {}",
                detail.chars().take(300).collect::<String>()
            ),
        });
    }
    let mut reader = SseReader::default();
    let mut pending = Vec::new();
    loop {
        let chunk = response.chunk().await.map_err(|e| Failure {
            retry: false,
            message: format!("Claude stream failed: {e}"),
        })?;
        let Some(chunk) = chunk else {
            break; // ended before any words; `finish_stream` reports why
        };
        let events = reader.push(&chunk);
        if let Some(StreamEvent::Error(message)) =
            events.iter().find(|e| matches!(e, StreamEvent::Error(_)))
        {
            // Overloaded before any words: as retryable as an HTTP 529.
            return Err(Failure {
                retry: true,
                message: format!("Claude stream error: {message}"),
            });
        }
        let has_words = events.iter().any(|e| matches!(e, StreamEvent::Text(_)));
        pending.extend(events);
        if has_words {
            break;
        }
    }
    Ok(OpenedStream {
        response,
        reader,
        pending,
    })
}

/// Reads the rest of the stream, handing each piece of text to `on_text`.
async fn finish_stream(
    mut opened: OpenedStream,
    on_text: &mut (dyn FnMut(&str) + Send),
) -> Result<LlmResponse, String> {
    let mut text = String::new();
    let mut usage = Usage::default();
    let mut stop_reason = None;
    let mut events = std::mem::take(&mut opened.pending);
    loop {
        for event in events.drain(..) {
            match event {
                StreamEvent::Started(start) => usage = start,
                StreamEvent::Text(piece) => {
                    text.push_str(&piece);
                    on_text(&piece);
                }
                StreamEvent::Finished {
                    stop_reason: reason,
                    output_tokens,
                } => {
                    stop_reason = reason;
                    usage.output_tokens = output_tokens;
                }
                StreamEvent::Error(message) => {
                    return Err(format!("Claude stream error: {message}"))
                }
            }
        }
        match opened.response.chunk().await {
            Ok(Some(chunk)) => events = opened.reader.push(&chunk),
            Ok(None) => break,
            Err(e) => return Err(format!("Claude stream failed: {e}")),
        }
    }
    match stop_reason.as_deref() {
        Some("refusal") => return Err("Claude declined to answer this request".into()),
        Some("max_tokens") => return Err("Claude response was cut off at max_tokens".into()),
        _ => {}
    }
    if text.trim().is_empty() {
        return Err("Claude returned no text".into());
    }
    Ok(LlmResponse { text, usage })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::{Access, Role, Turn};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const OK_BODY: &str = r#"{"content":[{"type":"text","text":"{\"hints\":[]}"}],"stop_reason":"end_turn","usage":{"input_tokens":1,"output_tokens":1}}"#;

    /// Reads one HTTP request (headers and a Content-Length body) from a plain socket.
    async fn read_request(tcp: &mut tokio::net::TcpStream) {
        let mut data = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let Ok(n) = tcp.read(&mut buf).await else {
                return;
            };
            if n == 0 {
                return;
            }
            data.extend_from_slice(&buf[..n]);
            let text = String::from_utf8_lossy(&data);
            if let Some(end) = text.find("\r\n\r\n") {
                let length = text[..end]
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("content-length:")
                            .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                    })
                    .unwrap_or(0);
                if data.len() >= end + 4 + length {
                    return;
                }
            }
        }
    }

    /// A stand-in for the backend's /llm/messages: `reply(n)` answers the n-th request (from 0)
    /// with (status, body), or `None` to never answer.
    async fn fake_backend(
        reply: fn(usize) -> Option<(u16, &'static str)>,
    ) -> (String, Arc<AtomicUsize>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let counted = count.clone();
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                let n = counted.fetch_add(1, Ordering::SeqCst);
                tokio::spawn(async move {
                    read_request(&mut tcp).await;
                    match reply(n) {
                        Some((status, body)) => {
                            let response = format!(
                                "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                                body.len()
                            );
                            let _ = tcp.write_all(response.as_bytes()).await;
                        }
                        // Stuck: hold the connection open and say nothing.
                        None => tokio::time::sleep(Duration::from_secs(30)).await,
                    }
                });
            }
        });
        (url, count)
    }

    fn request<'a>(turns: &'a [Turn], hedge_after: Option<Duration>) -> LlmRequest<'a> {
        static SCHEMA: std::sync::LazyLock<serde_json::Value> =
            std::sync::LazyLock::new(|| json!({ "type": "object" }));
        LlmRequest {
            system: "test",
            turns,
            schema: &SCHEMA,
            max_tokens: 10,
            cache: true,
            effort: None,
            temperature: None,
            timeout: Duration::from_secs(4),
            hedge_after,
        }
    }

    #[test]
    fn temperature_is_sent_only_when_set() {
        let turns = turns();
        let plain = request(&turns, None);
        assert!(request_body("m", &plain).get("temperature").is_none());
        let steady = LlmRequest {
            temperature: Some(0.25),
            ..request(&turns, None)
        };
        assert_eq!(request_body("m", &steady)["temperature"], json!(0.25));
    }

    fn turns() -> Vec<Turn> {
        vec![Turn {
            role: Role::User,
            text: "hi".into(),
        }]
    }

    async fn call(url: String, hedge_after: Option<Duration>) -> Result<LlmResponse, String> {
        let access = Access::Backend {
            url,
            token: "t".into(),
        };
        let turns = turns();
        generate(
            &reqwest::Client::new(),
            &access,
            "m",
            &request(&turns, hedge_after),
        )
        .await
    }

    #[test]
    fn a_stuck_request_is_answered_by_its_hedge() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (url, count) = fake_backend(|n| (n > 0).then_some((200, OK_BODY))).await;
            let started = Instant::now();
            let response = call(url, Some(Duration::from_millis(300))).await;
            assert!(response.is_ok(), "{:?}", response.err());
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "{:?}",
                started.elapsed()
            );
            assert_eq!(count.load(Ordering::SeqCst), 2);
        });
    }

    #[test]
    fn overloaded_is_retried_once_and_a_bad_request_is_not() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (url, count) =
                fake_backend(|n| Some(if n == 0 { (529, "{}") } else { (200, OK_BODY) })).await;
            assert!(call(url, None).await.is_ok());
            assert_eq!(count.load(Ordering::SeqCst), 2);

            let (url, count) = fake_backend(|_| Some((400, "{}"))).await;
            let Err(error) = call(url, None).await else {
                panic!("a 400 must fail")
            };
            assert!(error.contains("400"), "{error}");
            assert_eq!(count.load(Ordering::SeqCst), 1);
        });
    }

    const STREAM_START: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":4400,\"output_tokens\":1}}}\n\n";
    const STREAM_PIECE_1: &str = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"{\\\"hints\\\":[{\\\"text\\\":\\\"Ask\"}}\n\n";
    const STREAM_PIECE_2: &str = "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\" about it\\\"}]}\"}}\n\n";
    const STREAM_END: &str = "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\"},\"usage\":{\"output_tokens\":7}}\n\nevent: message_stop\ndata: {\"type\":\"message_stop\"}\n\n";
    const STREAM_CUT_OFF: &str = "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"max_tokens\"},\"usage\":{\"output_tokens\":10}}\n\n";

    /// A stand-in for the backend's /llm/messages/stream: `reply(n)` gives the n-th request's
    /// events, sent a little apart; `None` sends the headers and then nothing. Returns the URL,
    /// the request count, and the first request as text.
    async fn fake_streaming(
        reply: fn(usize) -> Option<Vec<&'static str>>,
    ) -> (String, Arc<AtomicUsize>, Arc<std::sync::Mutex<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let count = Arc::new(AtomicUsize::new(0));
        let first_request = Arc::new(std::sync::Mutex::new(String::new()));
        let (counted, seen) = (count.clone(), first_request.clone());
        tokio::spawn(async move {
            while let Ok((mut tcp, _)) = listener.accept().await {
                let n = counted.fetch_add(1, Ordering::SeqCst);
                let seen = seen.clone();
                tokio::spawn(async move {
                    let mut data = vec![0u8; 16384];
                    let read = tcp.read(&mut data).await.unwrap_or(0);
                    if n == 0 {
                        *seen.lock().unwrap() = String::from_utf8_lossy(&data[..read]).into();
                    }
                    let _ = tcp
                        .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
                        .await;
                    match reply(n) {
                        Some(events) => {
                            for event in events {
                                tokio::time::sleep(Duration::from_millis(20)).await;
                                let _ = tcp.write_all(event.as_bytes()).await;
                            }
                        }
                        None => tokio::time::sleep(Duration::from_secs(30)).await,
                    }
                });
            }
        });
        (url, count, first_request)
    }

    async fn stream_call(
        url: String,
        hedge_after: Option<Duration>,
    ) -> (Result<LlmResponse, String>, Vec<String>) {
        let access = Access::Backend {
            url,
            token: "t".into(),
        };
        let turns = turns();
        let mut pieces = Vec::new();
        let result = stream(
            &reqwest::Client::new(),
            &access,
            "m",
            &request(&turns, hedge_after),
            &mut |text: &str| pieces.push(text.to_string()),
        )
        .await;
        (result, pieces)
    }

    #[test]
    fn a_streamed_answer_arrives_piece_by_piece() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (url, _, first_request) = fake_streaming(|_| {
                Some(vec![
                    STREAM_START,
                    STREAM_PIECE_1,
                    STREAM_PIECE_2,
                    STREAM_END,
                ])
            })
            .await;
            let (result, pieces) = stream_call(url, None).await;
            let response = result.unwrap();
            assert_eq!(pieces, ["{\"hints\":[{\"text\":\"Ask", " about it\"}]}"]);
            assert_eq!(response.text, "{\"hints\":[{\"text\":\"Ask about it\"}]}");
            assert_eq!(response.usage.input_tokens, 4412);
            assert_eq!(response.usage.cache_read_tokens, 4400);
            assert_eq!(response.usage.output_tokens, 7);
            let sent = first_request.lock().unwrap().clone();
            assert!(sent.starts_with("POST /llm/messages/stream "), "{sent}");
            assert!(sent.contains("\"stream\":true"), "{sent}");
        });
    }

    #[test]
    fn a_stream_with_no_words_is_hedged() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (url, count, _) = fake_streaming(|n| {
                (n > 0).then(|| vec![STREAM_START, STREAM_PIECE_1, STREAM_PIECE_2, STREAM_END])
            })
            .await;
            let started = Instant::now();
            let (result, pieces) = stream_call(url, Some(Duration::from_millis(300))).await;
            assert!(result.is_ok(), "{:?}", result.err());
            assert_eq!(pieces.len(), 2, "only the winner's words are shown");
            assert!(
                started.elapsed() < Duration::from_secs(2),
                "{:?}",
                started.elapsed()
            );
            assert_eq!(count.load(Ordering::SeqCst), 2);
        });
    }

    #[test]
    fn a_cut_off_stream_is_an_error() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        runtime.block_on(async {
            let (url, _, _) =
                fake_streaming(|_| Some(vec![STREAM_START, STREAM_PIECE_1, STREAM_CUT_OFF])).await;
            let (result, _) = stream_call(url, None).await;
            assert!(result.unwrap_err().contains("max_tokens"));
        });
    }
}
