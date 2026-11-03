//! Reads the Anthropic Messages API's server-sent events (`"stream": true`) as they arrive.
//! Network chunks can end anywhere, mid-event or even mid-character, so bytes are buffered until
//! a whole event (ended by a blank line) is in.

use super::Usage;

#[derive(Debug, PartialEq)]
pub enum StreamEvent {
    /// `message_start`: the prompt's token counts.
    Started(Usage),
    /// `content_block_delta` with text: the next piece of the answer.
    Text(String),
    /// `message_delta`: why the answer ended, and output tokens so far.
    Finished {
        stop_reason: Option<String>,
        output_tokens: u64,
    },
    /// `error`: the stream failed after it started (e.g. overloaded).
    Error(String),
}

#[derive(Default)]
pub struct SseReader {
    buffer: Vec<u8>,
}

impl SseReader {
    /// Adds a network chunk; returns the events it completed, in order.
    pub fn push(&mut self, chunk: &[u8]) -> Vec<StreamEvent> {
        self.buffer.extend_from_slice(chunk);
        let mut events = Vec::new();
        while let Some((end, separator)) = frame_end(&self.buffer) {
            let frame: Vec<u8> = self.buffer.drain(..end + separator).collect();
            // A whole frame is whole UTF-8, even if its chunks weren't.
            let frame = String::from_utf8_lossy(&frame[..end]);
            let data: String = frame
                .lines()
                .filter_map(|line| line.trim_end_matches('\r').strip_prefix("data:"))
                .map(str::trim_start)
                .collect::<Vec<_>>()
                .join("\n");
            if let Some(event) = parse_event(&data) {
                events.push(event);
            }
        }
        events
    }
}

/// Where the first complete frame ends and how long its blank-line separator is.
fn frame_end(buffer: &[u8]) -> Option<(usize, usize)> {
    let lf = buffer.windows(2).position(|w| w == b"\n\n").map(|i| (i, 2));
    let crlf = buffer
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| (i, 4));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if a.0 <= b.0 { a } else { b }),
        (a, b) => a.or(b),
    }
}

fn parse_event(data: &str) -> Option<StreamEvent> {
    let event: serde_json::Value = serde_json::from_str(data).ok()?;
    let count = |v: &serde_json::Value, key: &str| v[key].as_u64().unwrap_or(0);
    match event["type"].as_str()? {
        "message_start" => {
            let usage = &event["message"]["usage"];
            let (written, read) = (
                count(usage, "cache_creation_input_tokens"),
                count(usage, "cache_read_input_tokens"),
            );
            Some(StreamEvent::Started(Usage {
                input_tokens: count(usage, "input_tokens") + written + read,
                cache_read_tokens: read,
                cache_write_tokens: written,
                output_tokens: 0,
            }))
        }
        "content_block_delta" => event["delta"]["text"]
            .as_str()
            .map(|text| StreamEvent::Text(text.to_string())),
        "message_delta" => Some(StreamEvent::Finished {
            stop_reason: event["delta"]["stop_reason"].as_str().map(str::to_string),
            output_tokens: count(&event["usage"], "output_tokens"),
        }),
        "error" => Some(StreamEvent::Error(
            event["error"]["message"]
                .as_str()
                .unwrap_or("Claude's stream failed")
                .to_string(),
        )),
        _ => None, // ping, content_block_start/stop, message_stop
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const START: &str = "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"usage\":{\"input_tokens\":12,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":4400,\"output_tokens\":1}}}\n\n";

    fn delta(text: &str) -> String {
        format!(
            "event: content_block_delta\ndata: {{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{{\"type\":\"text_delta\",\"text\":{}}}}}\n\n",
            serde_json::to_string(text).unwrap()
        )
    }

    #[test]
    fn reads_a_whole_answer() {
        let stream = format!(
            "{START}event: ping\ndata: {{\"type\":\"ping\"}}\n\n{}{}\
             event: message_delta\ndata: {{\"type\":\"message_delta\",\"delta\":{{\"stop_reason\":\"end_turn\"}},\"usage\":{{\"output_tokens\":15}}}}\n\n\
             event: message_stop\ndata: {{\"type\":\"message_stop\"}}\n\n",
            delta("{\"hints\":[{\"text\":\"Ask"),
            delta(" about budget\"}]}"),
        );
        let events = SseReader::default().push(stream.as_bytes());
        assert_eq!(
            events,
            vec![
                StreamEvent::Started(Usage {
                    input_tokens: 4412,
                    cache_read_tokens: 4400,
                    cache_write_tokens: 0,
                    output_tokens: 0,
                }),
                StreamEvent::Text("{\"hints\":[{\"text\":\"Ask".into()),
                StreamEvent::Text(" about budget\"}]}".into()),
                StreamEvent::Finished {
                    stop_reason: Some("end_turn".into()),
                    output_tokens: 15
                },
            ]
        );
    }

    #[test]
    fn an_event_split_across_chunks_waits_for_the_rest() {
        let event = delta("कितना budget");
        let bytes = event.as_bytes();
        // Split inside the Devanagari: a UTF-8 character is cut in half.
        let cut = event.find("कि").unwrap() + 1;
        let mut reader = SseReader::default();
        assert!(reader.push(&bytes[..cut]).is_empty());
        assert_eq!(
            reader.push(&bytes[cut..]),
            vec![StreamEvent::Text("कितना budget".into())]
        );
    }

    #[test]
    fn crlf_line_endings_work_too() {
        let event = delta("hi").replace('\n', "\r\n");
        assert_eq!(
            SseReader::default().push(event.as_bytes()),
            vec![StreamEvent::Text("hi".into())]
        );
    }

    #[test]
    fn an_error_event_is_reported() {
        let event = "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"overloaded_error\",\"message\":\"Overloaded\"}}\n\n";
        assert_eq!(
            SseReader::default().push(event.as_bytes()),
            vec![StreamEvent::Error("Overloaded".into())]
        );
    }
}
