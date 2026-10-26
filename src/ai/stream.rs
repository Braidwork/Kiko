//! Incremental parsing of streamed LLM answers.
//!
//! Every online provider is now spoken to over the OpenAI-compatible
//! `/v1/chat/completions` shape, so this is the only framing left (Ollama's
//! native `/api/chat` NDJSON went with its provider). The items that
//! `bytes_stream()` yields are arbitrary byte fragments: they can carry several
//! events, half an event, or a fragment that ends in the middle of a UTF-8
//! character. Parsing them one by one as if each were a complete JSON document
//! is what the old code did, and it aborted the whole generation on the first
//! fragment that did not happen to line up. Everything here is byte-buffered so
//! chunk boundaries never matter.

use std::vec::Vec;

use serde_json::Value;

use crate::result::{Error, Result};

/// Cap on a single line, so a server that never sends a newline is rejected
/// instead of buffered without bound.
const MAX_LINE: usize = 1024 * 1024;

/// A byte buffer that hands out complete lines.
pub(crate) struct LineBuffer {
    buf: Vec<u8>,
    max_line: usize,
}

impl LineBuffer {
    pub(crate) fn new(max_line: usize) -> Self {
        Self {
            buf: Vec::with_capacity(8 * 1024),
            max_line,
        }
    }

    /// Appends a chunk, failing when a single unterminated line grows past
    /// `max_line`.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<()> {
        self.buf.extend_from_slice(chunk);
        if !self.buf.contains(&b'\n') && self.buf.len() > self.max_line {
            return Err(Error::WithMessage(format!(
                "A streaming response line grew past {} bytes without a newline.",
                self.max_line
            )));
        }
        Ok(())
    }

    /// Removes and returns the next complete line, without its `\n` and
    /// without one trailing `\r` so CRLF servers read the same.
    pub(crate) fn next_line(&mut self) -> Option<String> {
        let end = self.buf.iter().position(|&b| b == b'\n')?;
        let mut line: Vec<u8> = self.buf.drain(..=end).collect();
        line.pop();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        // A complete line is valid UTF-8 by construction: a newline byte can
        // never be part of a multi-byte sequence, so the split cannot land
        // inside a character. Lossy is a never-taken safety net, preferred
        // over a panic path.
        Some(String::from_utf8_lossy(&line).into_owned())
    }

    /// Removes whatever follows the last newline, for the end of a stream that
    /// was not newline-terminated.
    pub(crate) fn take_rest(&mut self) -> Option<String> {
        if self.buf.is_empty() {
            return None;
        }
        let mut rest = std::mem::take(&mut self.buf);
        if rest.last() == Some(&b'\r') {
            rest.pop();
        }
        Some(String::from_utf8_lossy(&rest).into_owned())
    }
}

/// Turns raw response chunks into ordered text deltas.
pub(crate) struct DeltaStream {
    lines: LineBuffer,
    /// Payloads of the `data:` lines seen since the last blank line. SSE allows
    /// an event to span several of them; they are joined with `\n` on dispatch.
    event: String,
    done: bool,
}

impl DeltaStream {
    pub(crate) fn new() -> Self {
        Self {
            lines: LineBuffer::new(MAX_LINE),
            event: String::new(),
            done: false,
        }
    }

    /// Whether the provider signalled the end of the answer (`[DONE]`), letting
    /// the caller stop reading early.
    pub(crate) fn is_done(&self) -> bool {
        self.done
    }

    /// Feeds one response chunk and returns the deltas it completed, in order.
    pub(crate) fn push(&mut self, chunk: &[u8]) -> Result<Vec<String>> {
        if self.done {
            return Ok(Vec::new());
        }
        self.lines.push(chunk)?;
        self.drain_lines()
    }

    /// Flushes anything left over at end of stream, so a final payload that
    /// arrived without its terminating newline or blank line is not dropped.
    pub(crate) fn finish(&mut self) -> Result<Vec<String>> {
        let mut out = self.drain_lines()?;
        if self.done {
            return Ok(out);
        }
        // A server that closes right after its last payload leaves a line with
        // no `\n` on the end. Treat it as a line anyway.
        if let Some(line) = self.lines.take_rest() {
            self.open_ai_line(&line, &mut out);
        }
        // ...and an event whose payload lines were seen but whose closing blank
        // line never arrived.
        if !self.event.is_empty() {
            let payload = std::mem::take(&mut self.event);
            if let Some(t) = self.payload_delta(&payload) {
                out.push(t);
            }
        }
        Ok(out)
    }

    fn drain_lines(&mut self) -> Result<Vec<String>> {
        let mut out: Vec<String> = Vec::new();
        while let Some(line) = self.lines.next_line() {
            if self.done {
                break;
            }
            self.open_ai_line(&line, &mut out);
        }
        Ok(out)
    }

    fn open_ai_line(&mut self, line: &str, out: &mut Vec<String>) {
        if line.is_empty() {
            // A blank line ends the event.
            if !self.event.is_empty() {
                let payload = std::mem::take(&mut self.event);
                if let Some(t) = self.payload_delta(&payload) {
                    out.push(t);
                }
            }
        } else if let Some(rest) = line.strip_prefix("data:") {
            if !self.event.is_empty() {
                self.event.push('\n');
            }
            self.event.push_str(rest.strip_prefix(' ').unwrap_or(rest));
        } else if line.starts_with(':') {
            // An SSE comment. OpenRouter and others use these as keep-alives.
        } else if line.starts_with('{') {
            // Some OpenAI-compatible servers answer with bare NDJSON rather
            // than SSE. A complete object on a line is its own event, so both
            // framings work without having to sniff the content type.
            if let Some(t) = self.payload_delta(line) {
                out.push(t);
            }
        } else {
            log::debug!("Ignoring unrecognized streaming line: {line}");
        }
    }

    /// Extracts the text of one payload, updating the end-of-answer state.
    ///
    /// Returns `None` for payloads that carry no text and for payloads that
    /// cannot be parsed at all: one malformed fragment must not end the answer,
    /// which is exactly what the previous code did.
    fn payload_delta(&mut self, payload: &str) -> Option<String> {
        let payload = payload.trim();
        if payload.is_empty() {
            return None;
        }
        if payload == "[DONE]" {
            self.done = true;
            return None;
        }
        let value: Value = match serde_json::from_str(payload) {
            Ok(v) => v,
            Err(e) => {
                log::warn!("Skipping unparseable streaming payload ({e:?}): {payload}");
                return None;
            }
        };
        if let Some(message) = provider_error(&value) {
            // Providers report failures *inside* a 200 response. Logged rather
            // than returned so the caller's loop stays simple; the caller
            // checks the answer it built and falls back when it is empty.
            log::warn!("Provider error in streaming response: {message}");
            self.done = true;
            return None;
        }
        let text = value
            .get("choices")?
            .as_array()?
            .first()?
            .get("delta")?
            .get("content")?
            .as_str()?;
        if text.is_empty() {
            return None;
        }
        Some(String::from(text))
    }
}

/// Recognizes the `error` object providers put inside an otherwise successful
/// stream, e.g. `{"error":{"message":"...","type":"..."}}`.
fn provider_error(value: &Value) -> Option<String> {
    let error = value.get("error")?;
    if error.is_null() {
        return None;
    }
    if let Some(s) = error.as_str() {
        return Some(String::from(s));
    }
    if let Some(m) = error.get("message").and_then(Value::as_str) {
        return Some(String::from(m));
    }
    Some(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every delta produced by feeding `chunks` one at a time must equal the
    /// deltas produced by feeding the same bytes whole. Chunk boundaries are
    /// the network's choice, not ours, so this is the property that matters.
    fn assert_chunking_is_irrelevant(bytes: &[u8]) -> Vec<String> {
        let mut whole = DeltaStream::new();
        let mut expected: Vec<String> = whole.push(bytes).unwrap();
        expected.extend(whole.finish().unwrap());

        let mut split = DeltaStream::new();
        let mut actual: Vec<String> = Vec::new();
        for b in bytes {
            actual.extend(split.push(&[*b]).unwrap());
        }
        actual.extend(split.finish().unwrap());

        assert_eq!(actual, expected, "one byte at a time changed the result");
        expected
    }

    fn open_ai_event(text: &str) -> String {
        format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n")
    }

    #[test]
    fn open_ai_reads_deltas_across_blank_lines() {
        let mut s = DeltaStream::new();
        let out = s
            .push(open_ai_event("Hel").as_bytes())
            .unwrap()
            .into_iter()
            .chain(s.push(open_ai_event("lo").as_bytes()).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(out, vec!["Hel", "lo"]);
    }

    #[test]
    fn open_ai_event_split_across_three_chunks() {
        let bytes = open_ai_event("hi").into_bytes();
        let (a, rest) = bytes.split_at(7);
        let (b, c) = rest.split_at(9);
        let mut s = DeltaStream::new();
        let mut out = s.push(a).unwrap();
        out.extend(s.push(b).unwrap());
        out.extend(s.push(c).unwrap());
        assert_eq!(out, vec!["hi"]);
    }

    /// A multi-byte character cut in half by a chunk boundary must survive:
    /// decoding each chunk on its own would replace it with U+FFFD.
    #[test]
    fn multibyte_character_split_across_chunks() {
        for text in ["好", "𝄞", "a好b"] {
            let bytes = open_ai_event(text).into_bytes();
            assert_eq!(
                assert_chunking_is_irrelevant(&bytes),
                vec![text],
            );
        }
    }

    #[test]
    fn carriage_return_and_newline_in_different_chunks() {
        let mut s = DeltaStream::new();
        let a = b"data: {\"choices\":[{\"delta\":{\"content\":\"x\"}}]}\r";
        let b = b"\n\r\n";
        assert!(s.push(a).unwrap().is_empty());
        assert_eq!(s.push(b).unwrap(), vec!["x"]);
    }

    /// SSE permits an event to span several `data:` lines, joined with `\n`.
    #[test]
    fn multiple_data_lines_form_one_event() {
        let mut s = DeltaStream::new();
        let out = s
            .push(b"data: {\"choices\":[{\"delta\":\ndata: {\"content\":\"x\"}}]}\n\n")
            .unwrap();
        assert_eq!(out, vec!["x"]);
    }

    /// Comments are keep-alives, often sent while a reasoning model is silent.
    #[test]
    fn comment_lines_are_ignored() {
        let mut s = DeltaStream::new();
        let mut out = s.push(b": OPENROUTER PROCESSING\n\n").unwrap();
        out.extend(s.push(open_ai_event("x").as_bytes()).unwrap());
        assert_eq!(out, vec!["x"]);
    }

    #[test]
    fn done_marker_ends_the_stream() {
        let mut s = DeltaStream::new();
        let out = s
            .push(b"data: [DONE]\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"late\"}}]}\n\n")
            .unwrap();
        assert!(out.is_empty());
        assert!(s.is_done());
    }

    /// A server that answers with plain NDJSON instead of SSE must work too.
    #[test]
    fn bare_ndjson_objects_are_accepted() {
        let bytes = b"{\"choices\":[{\"delta\":{\"content\":\"a\"}}]}\n{\"choices\":[{\"delta\":{\"content\":\"b\"}}]}\n";
        assert_eq!(
            assert_chunking_is_irrelevant(bytes),
            vec!["a", "b"],
        );
    }

    /// The old code returned an error here and lost the entire answer.
    #[test]
    fn unparseable_line_is_skipped_not_fatal() {
        let mut s = DeltaStream::new();
        let mut out = s.push(b"data: not json at all\n\n").unwrap();
        out.extend(s.push(open_ai_event("kept").as_bytes()).unwrap());
        assert_eq!(out, vec!["kept"]);
    }

    #[test]
    fn provider_error_stops_the_stream() {
        let mut s = DeltaStream::new();
        let out = s
            .push(b"data: {\"error\":{\"message\":\"invalid api key\"}}\n\n")
            .unwrap();
        assert!(out.is_empty());
        assert!(s.is_done());
    }

    /// The last payload has to survive either way a stream can end early: with
    /// its line complete but the closing blank line missing, and with no
    /// newline at all.
    #[test]
    fn finish_flushes_an_unterminated_final_event() {
        let mut s = DeltaStream::new();
        assert!(
            s.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"half\"}}]}\n")
                .unwrap()
                .is_empty()
        );
        assert_eq!(s.finish().unwrap(), vec!["half"]);
        assert!(s.finish().unwrap().is_empty(), "finish must be idempotent");

        let mut s = DeltaStream::new();
        assert!(
            s.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"tail\"}}]}")
                .unwrap()
                .is_empty()
        );
        assert_eq!(s.finish().unwrap(), vec!["tail"]);
        assert!(s.finish().unwrap().is_empty(), "finish must be idempotent");
    }

    /// A payload truncated mid-JSON is unparseable, not fatal, and must not
    /// leave anything behind for the next `finish`.
    #[test]
    fn finish_tolerates_a_truncated_final_line() {
        let mut s = DeltaStream::new();
        assert!(s.push(b"data: {\"choices\":[{\"delta\":{\"conte").unwrap().is_empty());
        assert!(s.finish().unwrap().is_empty());
        assert!(s.finish().unwrap().is_empty());
    }

    #[test]
    fn line_without_newline_past_the_cap_is_rejected() {
        let mut buffer = LineBuffer::new(16);
        assert!(buffer.push(b"0123456789").is_ok());
        assert!(buffer.push(b"0123456789").is_err());
        // A newline below the cap keeps working.
        let mut buffer = LineBuffer::new(16);
        assert!(buffer.push(b"short line\nand some more bytes").is_ok());
    }
}
