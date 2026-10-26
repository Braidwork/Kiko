use std::sync::LazyLock;

use regex::Regex;

use super::context::Context;
use super::dto::{Request, ResponseChannelWrapper, ResponseData};
use crate::ai::chat::Prompt;
use crate::flow::rt::node::RuntimeNode;
use crate::result::{Error, Result};
use crate::web::server::envelope_json;

pub(crate) static HTML_TAG_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<[^>]+>").unwrap());

/// Nodes a single request may run before the flow is assumed to be looping.
const MAX_NODE_EXECUTIONS: usize = 100;

/// Everything that happens before the first node runs: the session, its
/// context, attachments, the entry node, imported variables and the user turn.
async fn prepare(req: &mut Request) -> Result<Context> {
    // log::info!("user input: {}", &req.user_input);
    // let now = std::time::Instant::now();
    if req.session_id.is_none() || req.session_id.as_ref().unwrap().is_empty() {
        req.session_id = Some(scru128::new_string());
    }
    let mut ctx = Context::get(&req.robot_id, req.session_id.as_ref().unwrap()).await?;
    if !req.attachments.is_empty() {
        match crate::ai::dto::UserMediaData::from_attachments(&req.attachments) {
            Ok(m) => {
                if !m.is_empty() {
                    ctx.user_media = Some(m);
                }
            }
            Err(m) => return Err(Error::WithMessage(m)),
        }
    }
    // log::info!("get ctx {:?}", now.elapsed());
    // let now = std::time::Instant::now();
    if ctx.no_node() {
        if ctx.main_flow_id.is_empty() {
            ctx.main_flow_id.push_str(&req.main_flow_id);
        }
        ctx.add_node(&req.main_flow_id);
    }
    // log::info!("add_node time {:?}", now.elapsed());
    // let now = std::time::Instant::now();
    if req.user_input_intent.is_some() {
        // req.user_input_intent = detector::detect(&req.robot_id, &req.user_input).await?;
        // println!("{:?}", req.user_input_intent);
        let user_input_intent = std::mem::take(&mut req.user_input_intent);
        ctx.set_user_input_intent(user_input_intent.unwrap())
    }
    // log::info!("Intent detection took {:?}", now.elapsed());
    if req.import_variables.is_some() {
        let import_variables = Option::take(&mut req.import_variables);
        let mut import_variables = import_variables.unwrap();
        for v in import_variables.iter_mut() {
            // Canonicalized for the same reason as in `CollectNode`, so callers
            // passing an older spelling still hit the variable.
            let k = crate::variable::crud::sanitize_var_name(&v.var_name);
            let v = crate::variable::dto::VariableValue::new(&v.var_val, &v.var_type);
            ctx.vars.insert(k, v);
        }
    }
    // println!("intent detect {:?}", now.elapsed());
    // let now = std::time::Instant::now();
    ctx.chat_history.push(Prompt {
        role: String::from("user"),
        content: HTML_TAG_REGEX.replace_all(&req.user_input, "").to_string(),
    });
    Ok(ctx)
}

/// Adds the assistant turns a run produced to the chat history.
///
/// Only the buffered answers are left to do here: a streamed one went out as
/// frames and was recorded by the node that produced it, as soon as its text was
/// complete (`Context::fill_answer_history`). Nothing lands twice, because a
/// streaming response never puts its answers in `res.answers`.
fn record_answers(ctx: &mut Context, res: &ResponseData) {
    if res.answers.is_empty() {
        return;
    }
    for a in res.answers.iter() {
        ctx.chat_history.push(Prompt {
            role: String::from("assistant"),
            content: HTML_TAG_REGEX.replace_all(&a.content, "").to_string(),
        });
    }
}

/// Runs nodes until one asks to stop or the queue empties.
async fn run(
    req: &Request,
    ctx: &mut Context,
    channel: &ResponseChannelWrapper,
) -> Result<ResponseData> {
    // let now = std::time::Instant::now();
    let mut response = ResponseData::new(req);
    for _i in 0..MAX_NODE_EXECUTIONS {
        // let now = std::time::Instant::now();
        if let Some(mut n) = ctx.pop_node().await {
            // println!("pop node {:?}", now.elapsed());
            let ret = n.exec(req, ctx, &mut response, channel).await;
            // println!("node exec {:?}", now.elapsed());
            if ret {
                // log::info!("exec time {:?}", now.elapsed());
                return Ok(response);
            }
        } else {
            return Ok(response);
        }
    }
    let m = if *crate::web::server::IS_EN {
        "Too many executions, please check if the process configuration is correct."
    } else {
        "执行次数太多，请检查流程配置是否正确。"
    };
    Err(Error::WithMessage(String::from(m)))
}

/// Runs a flow whose answers are buffered and sent as one document.
pub(in crate::flow::rt) async fn process(req: &mut Request) -> Result<ResponseData> {
    let mut ctx = prepare(req).await?;
    let r = run(req, &mut ctx, &ResponseChannelWrapper::none()).await;
    if let Ok(res) = r.as_ref() {
        record_answers(&mut ctx, res);
    }
    // println!("exec {:?}", now.elapsed());
    ctx.save().await?;
    // log::info!("ctx save time {:?}", now.elapsed());
    r
}

/// Runs a flow whose answers are pushed frame by frame as they are produced,
/// and ends it with one terminal frame carrying the same `{status, data, err}`
/// envelope a non-streaming request returns.
///
/// That frame is the only way a failure can be reported here: by the time this
/// runs the response headers have already gone out, so the status is committed.
/// Nothing is returned to the caller for the same reason.
pub(in crate::flow::rt) async fn process_streaming(
    req: &mut Request,
    channel: ResponseChannelWrapper,
) {
    let r = match prepare(req).await {
        Ok(mut ctx) => {
            let r = run(req, &mut ctx, &channel).await;
            if let Ok(res) = r.as_ref() {
                record_answers(&mut ctx, res);
            }
            match ctx.save().await {
                Ok(()) => r,
                Err(e) => Err(e),
            }
        }
        Err(e) => Err(e),
    };
    if let Err(e) = &r {
        log::error!("Streamed flow failed: {e:?}");
    }
    let r = channel.push_terminal(envelope_json(r));
    if !r {
        log::warn!("Terminal frame dropped, the client is gone.");
    }
}
