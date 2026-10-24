use core::time::Duration;
use std::convert::Infallible;
use std::result::Result;

use axum::body::Bytes;
use axum::response::sse::{Event, KeepAlive, Sse};
// use crossbeam_channel::bounded;
use futures::future::Either;
use futures::stream::{self, Stream};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio_stream::StreamExt as _;
use tokio_stream::wrappers::UnboundedReceiverStream;

use crate::ai::chat;

#[derive(Deserialize, Serialize)]
pub(crate) struct Request {
    pub(crate) robot_id: String,
    pub(crate) prompt: String,
    /// Qwen3 的思考模式。缺省 `false`（不思考）—— 这个接口要的是"生成一段文字"，
    /// 推理轨迹对调用方没有意义，而本地小模型上它会先烧掉几十秒才吐第一个字。
    /// 老调用方不传这个字段也照常工作。
    #[serde(default, rename = "enableThinking")]
    pub(crate) enable_thinking: bool,
}

// struct Guard;
// impl Drop for Guard {
//     fn drop(&mut self) {
//         println!("A SSE connection was dropped!")
//     }
// }

pub(crate) async fn gen_text(bytes: Bytes) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let q: Request = serde_json::from_slice(bytes.as_ref()).unwrap();
    // let _guard = Guard;
    let stream = if q.robot_id.is_empty() || q.prompt.is_empty() {
        Either::Left(stream::once(futures::future::ready(
            Ok::<Event, Infallible>(Event::default().data("Invalid robot_id or prompt")),
        )))
    } else {
        // let (sender, receiver) = bounded::<String>(1);
        // Either::Right(stream::once(futures::future::ready(Ok::<Event, Infallible>(
        //     Event::default().data("Invalid robot_id or prompt")
        // ))))
        // Unbounded so generation is never throttled by whoever is reading the
        // response, and so a slow reader cannot make the producer block.
        let (sender, receiver) =
            mpsc::unbounded_channel::<crate::flow::rt::dto::StreamingResponseData>();
        let stream = UnboundedReceiverStream::new(receiver).map(|s| {
            // log::info!("Sse sending {s}");
            let event = Event::default().data(s.content);
            Ok::<Event, Infallible>(event)
        });
        tokio::spawn(async move {
            // 走 chat 那条路径，用对话模型那一份配置（`chat::gen_text` 里
            // 说明了为什么）。路由本身保持不变，前端仍在调 `/ai/text/generation`。
            if let Err(e) = chat::gen_text(&q.robot_id, &q.prompt, q.enable_thinking, sender).await {
                log::error!("{:?}", &e);
            }
        });
        Either::Right(stream)
    };
    Sse::new(stream).keep_alive(
        KeepAlive::new()
            .interval(Duration::from_secs(30))
            .text("keep-alive-text"),
    )
}
