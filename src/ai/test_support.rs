//! Helpers shared by the OpenAI-compatible provider tests.
//!
//! `chat.rs` and `embedding.rs` both assert the same thing about the request
//! that leaves the process, so they share one local HTTP server rather than two
//! copies that drift apart. Every helper here is `pub(crate)` and compiled only
//! under `cfg(test)`.

use std::vec::Vec;

use super::chat::Prompt;
use crate::flow::rt::dto::StreamingResponseData;

/// Serves one canned response, written in the given pieces so the write
/// boundaries fall exactly where a test wants them — including in the middle
/// of a multi-byte character. `cuts` are absolute byte offsets into `body`, so
/// the client sees the body arrive in `cuts.len() + 1` reads.
///
/// The URL is always `/v1/chat/completions`. A test that needs to assert on the
/// request the client sent wants [`serve_capturing`] instead.
///
/// Each test uses its own read timeout, and that is part of the client cache
/// key, so no two tests share a client or its connection pool.
pub(crate) async fn serve(head: String, body: &str, cuts: &[usize]) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let body = body.as_bytes();
    let mut pieces: Vec<Vec<u8>> = vec![head.into_bytes()];
    let mut start = 0;
    for &cut in cuts {
        assert!(
            cut >= start && cut <= body.len(),
            "cut {cut} is out of order or past the end"
        );
        pieces.push(body[start..cut].to_vec());
        start = cut;
    }
    pieces.push(body[start..].to_vec());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        // Read the request head so the client is not still writing when the
        // response arrives.
        let mut buf = [0u8; 2048];
        let _ = socket.read(&mut buf).await;
        for piece in pieces {
            if socket.write_all(&piece).await.is_err() {
                return;
            }
            tokio::task::yield_now().await;
        }
    });
    format!("http://{addr}/v1/chat/completions")
}

/// The index just past the blank line that ends an HTTP request head.
fn head_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
}

/// The head's `Content-Length`, or 0 when it carries none.
fn content_length(head: &[u8]) -> usize {
    String::from_utf8_lossy(head)
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            if k.eq_ignore_ascii_case("content-length") {
                v.trim().parse().ok()
            } else {
                None
            }
        })
        .unwrap_or(0)
}

/// Like [`serve`], but on an arbitrary path and it hands back the raw request
/// the client actually sent. `serve` reads one chunk and throws it away, so it
/// cannot catch a wrong URL, a missing header or a wrong body. Reads the whole
/// request (head + body, by `Content-Length`) before replying.
pub(crate) async fn serve_capturing(
    path: &str,
    head: String,
    body: &str,
) -> (String, tokio::sync::oneshot::Receiver<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let head = head.into_bytes();
    let body = body.as_bytes().to_vec();
    tokio::spawn(async move {
        let Ok((mut socket, _)) = listener.accept().await else {
            return;
        };
        let mut raw: Vec<u8> = Vec::with_capacity(4096);
        let mut buf = [0u8; 4096];
        loop {
            let n = match socket.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            raw.extend_from_slice(&buf[..n]);
            if let Some(end) = head_end(&raw)
                && raw.len() >= end + content_length(&raw[..end])
            {
                break;
            }
        }
        let _ = tx.send(String::from_utf8_lossy(&raw).into_owned());
        if socket.write_all(&head).await.is_err() {
            return;
        }
        let _ = socket.write_all(&body).await;
    });
    (format!("http://{addr}{path}"), rx)
}

pub(crate) fn sse_head() -> String {
    String::from("HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n")
}

/// 本地假 embedding 端点：服务 `requests` 次，按请求里的 `input` 生成**确定性**
/// 4 维向量（同样文本 → 同样向量，相似度 1）。知识库端到端测试用它替代真模型：
/// 既不需要下载权重，也不会出网。返回的 channel 会收到每次请求的 `model` 名字，
/// 用来断言"检索/写入用的是 active model 还是配置里的模型"（B-23 / §6.4）。
pub(crate) async fn serve_embeddings_capturing(
    requests: usize,
) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        for _ in 0..requests {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut raw: Vec<u8> = Vec::with_capacity(4096);
            let mut buf = [0u8; 4096];
            loop {
                let n = match socket.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                raw.extend_from_slice(&buf[..n]);
                if let Some(end) = head_end(&raw)
                    && raw.len() >= end + content_length(&raw[..end])
                {
                    break;
                }
            }
            let _ = tx.send(request_model(&raw));
            let body = format!(
                "{{\"data\":[{{\"embedding\":{}}}]}}",
                fake_embedding(&request_input(&raw))
            );
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            if socket.write_all(head.as_bytes()).await.is_err() {
                return;
            }
            let _ = socket.write_all(body.as_bytes()).await;
            let _ = socket.shutdown().await;
        }
    });
    (format!("http://{addr}/v1/embeddings"), rx)
}

/// 从请求体里取 `"input":"…"`。测试用的文本不含引号/反斜杠，所以不需要完整 JSON 解析。
fn request_input(raw: &[u8]) -> String {
    let Some(end) = head_end(raw) else {
        return String::new();
    };
    let body = String::from_utf8_lossy(&raw[end..]).into_owned();
    let Some(i) = body.find("\"input\":\"") else {
        return String::new();
    };
    body[i + 9..]
        .split('"')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// 从请求体里取 `"model":"…"`（同样只做够用的字符串解析）。
fn request_model(raw: &[u8]) -> String {
    let Some(end) = head_end(raw) else {
        return String::new();
    };
    let body = String::from_utf8_lossy(&raw[end..]).into_owned();
    let Some(i) = body.find("\"model\":\"") else {
        return String::new();
    };
    body[i + 9..]
        .split('"')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// 确定性 4 维向量：同样文本 → 完全相同的向量。
fn fake_embedding(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let second = (chars.len() % 7) as f32 / 7.0 + 0.01;
    let third = (chars.first().map(|c| *c as u32).unwrap_or(0) % 11) as f32 / 11.0 + 0.01;
    format!("[1.0,{second:.4},{third:.4},0.5]")
}

pub(crate) fn sse_event(text: &str) -> String {
    format!("data: {{\"choices\":[{{\"delta\":{{\"content\":\"{text}\"}}}}]}}\n\n")
}

pub(crate) fn prompt() -> Option<Vec<Prompt>> {
    Some(vec![Prompt {
        role: String::from("user"),
        content: String::from("hi"),
    }])
}

/// Collects what the stream pushed, in order.
pub(crate) fn drain(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<StreamingResponseData>,
) -> Vec<String> {
    let mut out = Vec::new();
    while let Ok(f) = rx.try_recv() {
        assert_eq!(f.content_seq, Some(0), "every delta belongs to answer 0");
        out.push(f.content);
    }
    out
}
