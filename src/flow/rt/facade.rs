use axum::Json;
use axum::extract::Multipart;
use axum::response::IntoResponse;
use base64::Engine;

use super::dto::{Request, ResponseChannelWrapper, ResponseData};
use super::executor;
use crate::ai::dto::Attachment;
use crate::result::Error;
use crate::web::server::{to_ndjson, to_res2};

pub(crate) async fn answer(Json(mut req): Json<Request>) -> impl IntoResponse {
    let now = std::time::Instant::now();
    let res = if req.stream {
        stream(req)
    } else {
        to_res2(executor::process(&mut req).await)
    };
    // println!("exec used time:{:?}", now.elapsed());
    log::info!("Response used time:{:?}", now.elapsed());
    res
}

/// Starts the flow on its own task and returns the body immediately.
///
/// The channel has to be created here, not inside the flow, because this is the
/// last moment before the response body starts being polled. Opening it any
/// later would mean nothing drains the frames while the flow runs, and every
/// answer would arrive in one burst at the end — which is not streaming.
fn stream(mut req: Request) -> axum::response::Response {
    let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let channel = ResponseChannelWrapper::new(sender);
    tokio::spawn(async move {
        executor::process_streaming(&mut req, channel).await;
    });
    // The flow task holds the only sender, so the body ends when it finishes.
    to_ndjson(receiver)
}

/// Like `answer` but accepts image attachments via multipart/form-data.
/// Fields:
/// - `request`: the JSON body, same schema as `/flow/answer`.
/// - `images` (repeatable): binary image files.
pub(crate) async fn answer_multipart(mut multipart: Multipart) -> impl IntoResponse {
    let now = std::time::Instant::now();
    let mut op_req: Option<Request> = None;
    let mut attachments: Vec<Attachment> = Vec::with_capacity(4);
    let r = loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                if field.name().is_some_and(|n| n.eq("request")) {
                    match field.bytes().await {
                        Ok(b) => match serde_json::from_slice::<Request>(b.as_ref()) {
                            Ok(req) => op_req = Some(req),
                            Err(e) => {
                                break Err(crate::result::Error::InvalidJsonStructure(Box::new(e)))
                            }
                        },
                        Err(e) => break Err(e.into()),
                    }
                } else if field.name().is_some_and(|n| n.eq("images")) {
                    // `bytes()` consumes the field, so take the content type first.
                    let mime_type = field
                        .content_type()
                        .map(|t| t.to_string())
                        .unwrap_or(String::from("image/jpeg"));
                    match field.bytes().await {
                        Ok(b) => {
                            let data =
                                base64::engine::general_purpose::STANDARD.encode(b.as_ref());
                            attachments.push(Attachment { mime_type, data });
                        }
                        Err(e) => break Err(e.into()),
                    }
                }
            }
            Ok(None) => break Ok(op_req),
            Err(e) => break Err(e.into()),
        }
    };
    let res = match r {
        Ok(Some(mut req)) => {
            if !attachments.is_empty() {
                req.attachments.extend(attachments);
            }
            if req.stream {
                stream(req)
            } else {
                to_res2(executor::process(&mut req).await)
            }
        }
        Ok(None) => to_res2::<ResponseData>(Err(Error::WithMessage(String::from(
            "Field `request` is missing.",
        )))),
        Err(e) => to_res2::<ResponseData>(Err(e)),
    };
    log::info!("Response used time:{:?}", now.elapsed());
    res
}
