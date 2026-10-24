use core::time::Duration;
use std::collections::HashMap;
// use std::ops::DerefMut;

// use enum_dispatch::enum_dispatch;
use lettre::transport::smtp::PoolConfig;
use rkyv::{Archive, Deserialize, Serialize, util::AlignedVec};

use super::condition::ConditionData;
use super::context::Context;
use super::dto::{
    AnswerContentType, AnswerData, CollectData, Request, ResponseChannelWrapper, ResponseData,
};
use crate::ai::chat::{ResultSender, SenderWrapper};
use crate::ai::chat::Prompt;
use crate::external::http::client as http;
use crate::flow::rt::collector;
use crate::flow::subflow::dto::NextActionType;
use crate::man::settings::get_settings;
use crate::result::Result;
use crate::variable::crud as variable;
use crate::variable::dto::{VariableType, VariableValue};

// #[repr(u8)]
// #[derive(PartialEq)]
// pub(in crate::flow::rt) enum RuntimeNodeTypeId {
//     TextNode = 1,
//     GotoAnotherNode = 2,
//     CollectNode = 3,
//     ConditionNode = 4,
//     TerminateNode = 5,
// }

// #[enum_dispatch]
#[derive(Archive, Deserialize, Serialize)]
#[rkyv(compare(PartialEq))]
pub(crate) enum RuntimeNodeEnum {
    TextNode(TextNode),
    LlmGenTextNode(LlmGenTextNode),
    ConditionNode(ConditionNode),
    GotoAnotherNode(GotoAnotherNode),
    GotoMainFlowNode(GotoMainFlowNode),
    CollectNode(CollectNode),
    ExternalHttpCallNode(ExternalHttpCallNode),
    TerminateNode(TerminateNode),
    SendEmailNode(SendEmailNode),
    LlmChatNode(LlmChatNode),
    KnowledgeBaseAnswerNode(KnowledgeBaseAnswerNode),
}

impl RuntimeNode for RuntimeNodeEnum {
    async fn exec(
        &mut self,
        req: &Request,
        ctx: &mut Context,
        response: &mut ResponseData,
        channel_sender: &ResponseChannelWrapper,
    ) -> bool {
        match self {
            RuntimeNodeEnum::TextNode(n) => n.exec(req, ctx, response, channel_sender).await,
            RuntimeNodeEnum::LlmGenTextNode(n) => n.exec(req, ctx, response, channel_sender).await,
            RuntimeNodeEnum::ConditionNode(n) => n.exec(req, ctx, response, channel_sender).await,
            RuntimeNodeEnum::GotoAnotherNode(n) => n.exec(req, ctx, response, channel_sender).await,
            RuntimeNodeEnum::GotoMainFlowNode(n) => {
                n.exec(req, ctx, response, channel_sender).await
            }
            RuntimeNodeEnum::CollectNode(n) => n.exec(req, ctx, response, channel_sender).await,
            RuntimeNodeEnum::ExternalHttpCallNode(n) => {
                n.exec(req, ctx, response, channel_sender).await
            }
            RuntimeNodeEnum::TerminateNode(n) => n.exec(req, ctx, response, channel_sender).await,
            RuntimeNodeEnum::SendEmailNode(n) => n.exec(req, ctx, response, channel_sender).await,
            RuntimeNodeEnum::LlmChatNode(n) => n.exec(req, ctx, response, channel_sender).await,
            RuntimeNodeEnum::KnowledgeBaseAnswerNode(n) => {
                n.exec(req, ctx, response, channel_sender).await
            }
        }
    }
}

// #[enum_dispatch(RuntimeNodeEnum)]
pub(crate) trait RuntimeNode {
    async fn exec(
        &mut self,
        req: &Request,
        ctx: &mut Context,
        response: &mut ResponseData,
        channel_sender: &ResponseChannelWrapper,
    ) -> bool;
}

async fn replace_vars(text: &str, req: &Request, ctx: &mut Context) -> Result<String> {
    // Values may need an await (variable sources such as API calls), so resolve
    // every referenced name first and then substitute from the resolved map.
    let names = super::var_replace::collect_var_names(text);
    let mut values: HashMap<String, Option<String>> = HashMap::with_capacity(names.len());
    for name in names {
        if values.contains_key(&name) {
            continue;
        }
        let value = match variable::get(&req.robot_id, &name).await? {
            Some(v) => v
                .get_value2(req, ctx)
                .await
                .map(|value| value.val_to_string()),
            None => None,
        };
        values.insert(name, value);
    }
    super::var_replace::replace_vars_with(text, |name| Ok(values.get(name).cloned().flatten()))
}

#[inline]
fn add_next_node(ctx: &mut Context, next_node_id: &str) {
    ctx.add_node(next_node_id);
}

/// Records one complete answer.
///
/// A streamed one leaves as a frame of its own, tagged with the index it takes
/// in the chat history — which is why it is written there now, the frame and the
/// history entry being the same event. A buffered one waits in the response
/// document and reaches the history later, in `executor::record_answers`. Either
/// way the answer is recorded exactly once, and the only difference a client sees
/// is when it arrives.
fn push_answer(
    ctx: &mut Context,
    response: &mut ResponseData,
    channel_sender: &ResponseChannelWrapper,
    content: String,
    content_type: AnswerContentType,
) {
    if channel_sender.is_streaming() {
        let slot = ctx.add_answer_history(&content);
        if !channel_sender.push_frame(slot.content_seq, content) {
            log::warn!("Answer frame dropped, the client is gone.");
        }
    } else {
        response.answers.push(AnswerData {
            content,
            content_type,
        });
    }
}

#[derive(Archive, Deserialize, Serialize)]
#[rkyv(compare(PartialEq))]
pub(crate) struct TextNode {
    pub(super) text: String,
    pub(crate) text_type: AnswerContentType,
    pub(super) ret: bool,
    pub(super) next_node_id: String,
}

impl RuntimeNode for TextNode {
    async fn exec(
        &mut self,
        req: &Request,
        ctx: &mut Context,
        response: &mut ResponseData,
        channel_sender: &ResponseChannelWrapper,
    ) -> bool {
        // log::info!("Into TextNode {}", &self.text);
        // let now = std::time::Instant::now();
        match replace_vars(&self.text, req, ctx).await {
            Ok(answer) => push_answer(
                ctx,
                response,
                channel_sender,
                answer,
                self.text_type.clone(),
            ),
            Err(e) => log::error!("{e:?}"),
        };
        // log::info!("add {}", &self.next_node_id);
        add_next_node(ctx, &self.next_node_id);
        // log::info!("TextNode used time:{:?}", now.elapsed());
        self.ret
    }
}

#[derive(Archive, Deserialize, Serialize)]
#[rkyv(compare(PartialEq))]
pub(crate) struct LlmGenTextNode {
    pub(super) prompt: String,
    pub(crate) fallback_text: String,
    pub(super) context_len: u8,
    pub(crate) connect_timeout: Option<u32>,
    pub(crate) read_timeout: Option<u32>,
    pub(crate) response_streaming: bool,
    /// Qwen3 的思考模式。对话节点里的"生成文本"默认不思考。
    pub(crate) enable_thinking: bool,
    pub(super) ret: bool,
    pub(super) next_node_id: String,
}

impl RuntimeNode for LlmGenTextNode {
    async fn exec(
        &mut self,
        req: &Request,
        ctx: &mut Context,
        response: &mut ResponseData,
        channel_sender: &ResponseChannelWrapper,
    ) -> bool {
        // log::info!("Into LlmGenTextNode");
        // let now = std::time::Instant::now();
        let mut chat_history: Vec<Prompt> = Vec::with_capacity(5);
        if self.context_len > 0 && !ctx.chat_history.is_empty() {
            let len = ctx.chat_history.len();
            let context_len = self.context_len as usize;
            if len > context_len {
                // ctx.chat_history.drain(0..ctx.chat_history.len() - self.context_len as usize);
                chat_history.extend_from_slice(&ctx.chat_history[len - context_len..len - 1]);
            } else {
                chat_history.extend_from_slice(&ctx.chat_history);
            };
        };
        let p = Prompt {
            role: "system".to_string(),
            content: self.prompt.clone(),
        };
        chat_history.push(p);
        if channel_sender.is_streaming() && self.response_streaming {
            // Token-level streaming: the client asked for frames and this node
            // is set up to push them. Awaiting is what makes that work — the
            // channel is drained by the HTTP body while this waits, so frames
            // leave as they are produced instead of piling up until the end.
            let slot = ctx.add_answer_history("");
            let mut answer = String::with_capacity(1024);
            // `is_streaming()` above guarantees the channel exists.
            let sender =
                SenderWrapper::new(channel_sender.sender().unwrap().clone(), slot.content_seq);
            let r = crate::ai::chat::chat(
                &req.robot_id,
                Some(chat_history),
                ctx.user_media.as_ref(),
                self.connect_timeout,
                self.read_timeout,
                self.enable_thinking,
                ResultSender::ChannelSender(sender, &mut answer),
            )
            .await;
            let failed = match &r {
                Err(e) => {
                    log::error!("LlmGenTextNode response failed, err: {e:?}");
                    true
                }
                Ok(()) => answer.is_empty(),
            };
            if failed {
                // Whatever went wrong, the client must not be left with
                // nothing: send the fallback this node is configured with. It is
                // what the history has to record too, not the empty answer.
                log::warn!("LlmGenTextNode produced no answer, sending the fallback text.");
                channel_sender.push_frame(slot.content_seq, self.fallback_text.clone());
                answer.clear();
                answer.push_str(&self.fallback_text);
            }
            // The frames carried the text, but the history entry was only
            // reserved. Leaving it empty would put a blank assistant turn in
            // front of every later LLM call in this conversation.
            ctx.fill_answer_history(slot, &answer);
        } else if channel_sender.is_streaming() {
            // The client asked for frames but this node is not configured for
            // token-level streaming, so the answer is generated in full and
            // sent as one frame. Buffering it into the response document
            // instead would lose it, because a streaming response never sends
            // that document.
            let mut answer = String::with_capacity(1024);
            if let Err(e) = crate::ai::chat::chat(
                &req.robot_id,
                Some(chat_history),
                ctx.user_media.as_ref(),
                self.connect_timeout,
                self.read_timeout,
                self.enable_thinking,
                ResultSender::StrBuf(&mut answer),
            )
            .await
            {
                log::error!("LlmGenTextNode response failed, err: {e:?}");
            }
            if answer.is_empty() {
                answer.push_str(&self.fallback_text);
            }
            push_answer(
                ctx,
                response,
                channel_sender,
                answer,
                AnswerContentType::TextPlain,
            );
        } else {
            let now = std::time::Instant::now();
            let mut s = String::with_capacity(1024);
            if let Err(e) = crate::ai::chat::chat(
                &req.robot_id,
                Some(chat_history),
                ctx.user_media.as_ref(),
                self.connect_timeout,
                self.read_timeout,
                self.enable_thinking,
                ResultSender::StrBuf(&mut s),
            )
            .await
            {
                log::error!("LlmGenTextNode response failed, err: {:?}", &e);
                s.push_str(&self.fallback_text);
            } else {
                log::info!("LLM response |{}|", &s);
                if s.is_empty() {
                    response.answers.push(AnswerData {
                        content: self.fallback_text.clone(),
                        content_type: AnswerContentType::TextPlain,
                    });
                } else {
                    response.answers.push(AnswerData {
                        content: s,
                        content_type: AnswerContentType::TextPlain,
                    });
                }
            }
            log::info!("LLM response took {:?}", now.elapsed());
            // let (s, rev) = std::sync::mpsc::channel::<String>();
            // let robot_id = req.robot_id.clone();
            // let prompt = self.prompt.clone();
            // tokio::task::spawn(async move {
            //     let mut r = String::with_capacity(1024);
            //     if let Err(e) =
            //         crate::ai::chat::chat(&robot_id, &prompt, ResultReceiver::StrBuf(&mut r)).await
            //     {
            //         log::info!("LlmChatNode response failed, err: {:?}", &e);
            //         drop(s);
            //         return;
            //     }
            //     if let Err(_) = s.send(r) {
            //         log::info!("LlmChatNode sent response failed.");
            //     }
            // });
            // match rev.recv() {
            //     Ok(s) => {
            //         log::info!("LLM response {}", &s);
            //         response.answers.push(AnswerData {
            //             text: s,
            //             answer_type: AnswerType::TextPlain,
            //         });
            //     }
            //     // Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
            //     Err(e) => log::info!("LlmChatNode response failed, err: {:?}", &e),
            // }
            // let mut s = String::with_capacity(1024);
            // if let Err(e) = tokio::runtime::Handle::current().block_on(async {
            //     crate::ai::chat::chat(&req.robot_id, &self.prompt, ResultReceiver::StrBuf(&mut s))
            //         .await
            // }) {
            //     log::info!("LlmChatNode response failed, err: {:?}", &e);
            // } else {
            //     log::info!("LLM response {}", &s);
            //     response.answers.push(AnswerData {
            //         text: s,
            //         answer_type: AnswerType::TextPlain,
            //     });
            // }
        }
        // log::info!("add {}", &self.next_node_id);
        add_next_node(ctx, &self.next_node_id);
        // log::info!("LlmGenTextNode used time:{:?}", now.elapsed());
        self.ret
    }
}

#[derive(Archive, Deserialize, Serialize)]
#[rkyv(compare(PartialEq))]
pub(crate) struct GotoMainFlowNode {
    pub(super) main_flow_id: String,
    pub(super) next_node_id: String,
}

impl RuntimeNode for GotoMainFlowNode {
    async fn exec(
        &mut self,
        _req: &Request,
        ctx: &mut Context,
        _response: &mut ResponseData,
        _channel_sender: &ResponseChannelWrapper,
    ) -> bool {
        // println!("Into GotoMainFlowNode");
        ctx.main_flow_id.clear();
        ctx.main_flow_id.push_str(&self.main_flow_id);
        add_next_node(ctx, &self.next_node_id);
        false
    }
}

#[derive(Archive, Deserialize, Serialize)]
#[rkyv(compare(PartialEq))]
pub(crate) struct GotoAnotherNode {
    pub(super) next_node_id: String,
}

impl RuntimeNode for GotoAnotherNode {
    async fn exec(
        &mut self,
        _req: &Request,
        ctx: &mut Context,
        _response: &mut ResponseData,
        _channel_sender: &ResponseChannelWrapper,
    ) -> bool {
        // println!("Into GotoAnotherNode");
        add_next_node(ctx, &self.next_node_id);
        false
    }
}

#[derive(Archive, Deserialize, Serialize)]
#[rkyv(compare(PartialEq))]
pub(crate) struct CollectNode {
    pub(super) var_name: String,
    pub(super) collect_type: collector::CollectType,
    pub(super) successful_node_id: String,
    pub(super) failed_node_id: String,
}

impl RuntimeNode for CollectNode {
    async fn exec(
        &mut self,
        req: &Request,
        ctx: &mut Context,
        response: &mut ResponseData,
        _channel_sender: &ResponseChannelWrapper,
    ) -> bool {
        // println!("Into CollectNode");
        if let Some(r) = collector::collect(&req.user_input, &self.collect_type) {
            // println!("{} {}", &self.var_name, r);
            let v = VariableValue::new(r, &VariableType::Str);
            // Same canonical form the variable table uses, so a flow configured
            // with an older spelling still finds the collected value.
            let var_name = variable::sanitize_var_name(&self.var_name);
            ctx.vars.insert(var_name.clone(), v);
            let collect_data = CollectData {
                var_name,
                value: String::from(r),
            };
            response.collect_data.push(collect_data);
            add_next_node(ctx, &self.successful_node_id);
            // println!("{} {}", r, &self.successful_node_id);
        } else {
            add_next_node(ctx, &self.failed_node_id);
        }
        false
    }
}

#[derive(Archive, Deserialize, Serialize)]
#[rkyv(compare(PartialEq))]
pub(crate) struct ConditionNode {
    pub(super) next_node_id: String,
    pub(super) goto_node_id: String,
    pub(super) conditions: Vec<Vec<ConditionData>>,
}

impl RuntimeNode for ConditionNode {
    async fn exec(
        &mut self,
        req: &Request,
        ctx: &mut Context,
        _response: &mut ResponseData,
        _channel_sender: &ResponseChannelWrapper,
    ) -> bool {
        // println!("Into ConditionNode");
        let mut r = false;
        for and_conditions in self.conditions.iter() {
            for cond in and_conditions.iter() {
                r = cond.compare(req, ctx).await;
                if !r {
                    break;
                }
            }
            if r {
                add_next_node(ctx, &self.goto_node_id);
                return false;
            }
        }
        add_next_node(ctx, &self.next_node_id);
        false
    }
}

#[derive(Archive, Deserialize, Serialize)]
#[rkyv(compare(PartialEq))]
pub(crate) struct TerminateNode {}

impl RuntimeNode for TerminateNode {
    async fn exec(
        &mut self,
        _req: &Request,
        _ctx: &mut Context,
        response: &mut ResponseData,
        _channel_sender: &ResponseChannelWrapper,
    ) -> bool {
        // log::info!("Into TerminateNode");
        // The terminal frame is not sent from here: the run always ends with
        // one, and sending it here as well would deliver it before the nodes
        // that still have answers to add.
        response.next_action = NextActionType::Terminate;
        true
    }
}

#[derive(Archive, Deserialize, Serialize)]
#[rkyv(compare(PartialEq))]
pub(crate) struct ExternalHttpCallNode {
    pub(super) successful_node_id: String,
    pub(super) next_node_id: String,
    pub(super) http_api_id: String,
    pub(super) connect_timeout_milliseconds: u64,
    pub(super) read_timeout_milliseconds: u64,
    pub(super) async_req: bool,
}

impl RuntimeNode for ExternalHttpCallNode {
    async fn exec(
        &mut self,
        req: &Request,
        ctx: &mut Context,
        _response: &mut ResponseData,
        _channel_sender: &ResponseChannelWrapper,
    ) -> bool {
        // println!("Into ExternalHttpCallNode");
        let mut goto_node_id = &self.next_node_id;
        if let Ok(Some(api)) =
            crate::external::http::crud::get_detail(&req.robot_id, self.http_api_id.as_str()).await
        {
            if self.async_req {
                tokio::spawn(http::status_code(
                    api,
                    self.connect_timeout_milliseconds,
                    self.read_timeout_milliseconds,
                    ctx.vars.clone(),
                ));
            } else {
                match http::status_code(
                    api,
                    self.connect_timeout_milliseconds,
                    self.read_timeout_milliseconds,
                    ctx.vars.clone(),
                )
                .await
                {
                    Ok(r) => {
                        if r == 200u16 {
                            goto_node_id = &self.successful_node_id;
                        }
                    }
                    Err(e) => {
                        log::error!("{e:?}");
                    }
                };
            }
        }
        add_next_node(ctx, goto_node_id);
        false
    }
}

#[derive(Archive, Deserialize, Serialize)]
#[rkyv(compare(PartialEq))]
pub(crate) struct SendEmailNode {
    pub(super) from: String,
    pub(super) to_recipients: Vec<String>,
    pub(super) cc_recipients: Vec<String>,
    pub(super) bcc_recipients: Vec<String>,
    pub(super) subject: String,
    pub(super) content: String,
    pub(super) content_type: String,
    pub(super) async_send: bool,
    pub(super) successful_node_id: String,
    pub(super) goto_node_id: Option<String>,
}

impl SendEmailNode {
    fn send_email(&self, settings: &crate::man::settings::Settings) -> Result<()> {
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::{
            AsyncSmtpTransport, AsyncTransport, SmtpTransport, Tokio1Executor, Transport,
            message::{
                Mailboxes, MessageBuilder,
                header::{Bcc, Cc, ContentType, To},
            },
        };
        let mailboxes: Mailboxes = self.to_recipients.join(",").parse()?;
        let to_header: To = mailboxes.into();
        let mut builder = MessageBuilder::new().mailbox(to_header);
        if !self.cc_recipients.is_empty() {
            let mailboxes: Mailboxes = self.cc_recipients.join(",").parse()?;
            let cc_header: Cc = mailboxes.into();
            builder = builder.mailbox(cc_header);
        }
        if !self.bcc_recipients.is_empty() {
            let mailboxes: Mailboxes = self.bcc_recipients.join(",").parse()?;
            let bcc_header: Bcc = mailboxes.into();
            builder = builder.mailbox(bcc_header);
        }

        let content_type: ContentType = if self.content_type.eq("HTML") {
            ContentType::TEXT_HTML
        } else {
            ContentType::TEXT_PLAIN
        };

        let email = builder
            .from(self.from.parse()?)
            .subject(&self.subject)
            .header(content_type)
            .body(self.content.clone())
            // .singlepart(SinglePart::html(&self.content))
            ?;
        let creds = Credentials::new(
            settings.smtp_username.to_owned(),
            settings.smtp_password.to_owned(),
        );
        let pool = PoolConfig::new()
            .min_idle(1)
            .max_size(2)
            .idle_timeout(Duration::from_secs(300));
        if self.async_send {
            let builder = AsyncSmtpTransport::<Tokio1Executor>::relay(&settings.smtp_host)?;
            let mailer = builder
                .credentials(creds)
                .timeout(Some(core::time::Duration::from_secs(
                    settings.smtp_timeout_sec as u64,
                )))
                .pool_config(pool)
                .build();
            tokio::spawn(async move {
                // mailer.send(email) // will be wrong
                if let Err(e) = mailer.send(email).await {
                    log::error!("Failed to send email, failure reason is: {e:?}");
                }
            });
            Ok(())
        } else {
            let mailer = SmtpTransport::relay(&settings.smtp_host)?
                .credentials(creds)
                .timeout(Some(core::time::Duration::from_secs(
                    settings.smtp_timeout_sec as u64,
                )))
                .pool_config(pool)
                .build();

            Ok(mailer.send(&email).map(|r| {
                log::info!("Sent email response: {r:?}");
            })?)
        }
    }
}

impl RuntimeNode for SendEmailNode {
    async fn exec(
        &mut self,
        req: &Request,
        ctx: &mut Context,
        _response: &mut ResponseData,
        _channel_sender: &ResponseChannelWrapper,
    ) -> bool {
        // println!("Into SendEmailNode");
        if let Ok(Some(settings)) = get_settings(&req.robot_id).await {
            if !settings.smtp_host.is_empty() {
                match self.send_email(&settings) {
                    Ok(_) => add_next_node(ctx, &self.successful_node_id),
                    Err(_) => add_next_node(ctx, self.goto_node_id.as_ref().unwrap()),
                }
            }
        }
        false
    }
}

#[derive(Archive, Clone, Deserialize, Serialize, serde::Deserialize)]
#[rkyv(compare(PartialEq))]
pub(crate) enum LlmChatNodeExitCondition {
    Intent(String),
    SpecialInputs(String),
    LlmResultContains(String),
    MaxChatTimes(u8),
}

#[derive(Archive, Clone, Deserialize, Serialize, serde::Deserialize)]
#[rkyv(compare(PartialEq))]
pub(crate) enum LlmChatAnswerTimeoutThen {
    GotoAnotherNode,
    ResponseAlternateText(String),
    DoNothing,
}

#[derive(Archive, Clone, Deserialize, Serialize)]
#[rkyv(compare(PartialEq))]
pub(crate) struct LlmChatNode {
    // pub(super) prompt: String,
    pub(super) context_len: u8,
    pub(super) cur_run_times: u8,
    pub(super) exit_condition: LlmChatNodeExitCondition,
    pub(super) answer_timeout_then: LlmChatAnswerTimeoutThen,
    pub(crate) connect_timeout: Option<u32>,
    pub(crate) read_timeout: Option<u32>,
    pub(crate) response_streaming: bool,
    /// Qwen3 的思考模式，由用户在节点里选。默认 `false`。
    pub(crate) enable_thinking: bool,
    pub(super) next_node_id: String,
}

impl LlmChatNode {
    async fn inner_exec(
        &mut self,
        req: &Request,
        ctx: &mut Context,
        response: &mut ResponseData,
        channel_sender: &ResponseChannelWrapper,
    ) -> bool {
        // log::info!("Into LlmChatNode");
        self.cur_run_times += 1;
        let mut check_contains_str: Option<&String> = None;
        match &self.exit_condition {
            LlmChatNodeExitCondition::Intent(i) => match ctx.get_user_input_intent(req).await {
                Ok(crate::flow::rt::context::UserInputIntent::Detected(intent)) => {
                    if intent.eq(i) {
                        return false;
                    }
                }
                _ => {}
            },
            LlmChatNodeExitCondition::SpecialInputs(s) => {
                if req.user_input.eq(s) {
                    // log::info!("886 {}", &self.next_node_id);
                    return false;
                }
            }
            LlmChatNodeExitCondition::LlmResultContains(s) => {
                check_contains_str = Some(s);
            }
            LlmChatNodeExitCondition::MaxChatTimes(t) => {
                if self.cur_run_times > *t {
                    return false;
                }
            }
        }
        // log::info!("self.response_streaming {}", self.response_streaming);
        let chat_history = if ctx.chat_history.is_empty() {
            None
        } else {
            Some(ctx.chat_history.clone())
        };
        if self.response_streaming && channel_sender.is_streaming() {
            // Token-level streaming. Awaiting keeps the frames in order and
            // leaves the complete answer here, which is what lets the exit
            // condition below be evaluated at all.
            let slot = ctx.add_answer_history("");
            let mut answer = String::with_capacity(1024);
            // `is_streaming()` above guarantees the channel exists.
            let sender =
                SenderWrapper::new(channel_sender.sender().unwrap().clone(), slot.content_seq);
            if let Err(e) = crate::ai::chat::chat(
                &req.robot_id,
                chat_history,
                ctx.user_media.as_ref(),
                self.connect_timeout,
                self.read_timeout,
                self.enable_thinking,
                ResultSender::ChannelSender(sender, &mut answer),
            )
            .await
            {
                log::error!("LlmChatNode response failed, branch 1, err: {e:?}");
                match &self.answer_timeout_then {
                    LlmChatAnswerTimeoutThen::GotoAnotherNode => {
                        ctx.discard_answer_history(slot);
                        return false;
                    }
                    LlmChatAnswerTimeoutThen::ResponseAlternateText(t) => {
                        channel_sender.push_frame(slot.content_seq, t.clone());
                        answer.push_str(t);
                    }
                    LlmChatAnswerTimeoutThen::DoNothing => {
                        ctx.discard_answer_history(slot);
                        return false;
                    }
                }
            }
            // The frames carried the text, but the history entry was only
            // reserved. An answer that never produced anything gives the
            // reservation back instead of leaving a blank turn behind.
            if answer.is_empty() {
                ctx.discard_answer_history(slot);
            } else {
                ctx.fill_answer_history(slot, &answer);
            }
            // Staying on this node is the normal outcome: the conversation
            // continues, so this node is kept as the current one and the client
            // sends the next turn to it.
            if !answer.is_empty()
                && let Some(s) = check_contains_str
                && answer.contains(s.as_str())
            {
                return false;
            }
            true
        } else if channel_sender.is_streaming() {
            // The client asked for frames but this node is not configured for
            // token-level streaming, so the answer is generated in full and
            // sent as one frame rather than buffered into a document that a
            // streaming response never sends.
            let mut answer = String::with_capacity(1024);
            if let Err(e) = crate::ai::chat::chat(
                &req.robot_id,
                chat_history,
                ctx.user_media.as_ref(),
                self.connect_timeout,
                self.read_timeout,
                self.enable_thinking,
                ResultSender::StrBuf(&mut answer),
            )
            .await
            {
                log::error!("LlmChatNode response failed, branch 2, err: {e:?}");
                match &self.answer_timeout_then {
                    LlmChatAnswerTimeoutThen::GotoAnotherNode => return false,
                    LlmChatAnswerTimeoutThen::ResponseAlternateText(t) => answer.push_str(t),
                    LlmChatAnswerTimeoutThen::DoNothing => return false,
                }
            }
            if !answer.is_empty() {
                let exit = check_contains_str.is_some_and(|s| answer.contains(s.as_str()));
                push_answer(
                    ctx,
                    response,
                    channel_sender,
                    answer,
                    AnswerContentType::TextPlain,
                );
                if exit {
                    return false;
                }
            }
            true
        } else {
            let now = std::time::Instant::now();
            let mut s = String::with_capacity(1024);
            if let Err(e) = crate::ai::chat::chat(
                &req.robot_id,
                chat_history,
                ctx.user_media.as_ref(),
                self.connect_timeout,
                self.read_timeout,
                self.enable_thinking,
                ResultSender::StrBuf(&mut s),
            )
            .await
            {
                log::error!("LlmChatNode response failed, branch 3, err: {:?}", &e);
                match &self.answer_timeout_then {
                    LlmChatAnswerTimeoutThen::GotoAnotherNode => {
                        return false;
                    }
                    LlmChatAnswerTimeoutThen::ResponseAlternateText(t) => s.push_str(t),
                    LlmChatAnswerTimeoutThen::DoNothing => return false,
                }
            } else {
                log::info!("LLM response |{}|", &s);
                if !s.is_empty() {
                    let mut contains_certain_str = false;
                    if check_contains_str.is_some() {
                        log::info!(
                            "check_contains_str |{}|",
                            check_contains_str.as_ref().unwrap()
                        );
                        contains_certain_str = s.contains(check_contains_str.unwrap());
                        log::info!("contains_certain_str {contains_certain_str}");
                    }
                    response.answers.push(AnswerData {
                        content: s,
                        content_type: AnswerContentType::TextPlain,
                    });
                    if contains_certain_str {
                        return false;
                    }
                }
            }
            log::info!("LLM response took {:?}", now.elapsed());
            // let (s, rev) = std::sync::mpsc::channel::<String>();
            // let robot_id = req.robot_id.clone();
            // let prompt = self.prompt.clone();
            // tokio::task::spawn(async move {
            //     let mut r = String::with_capacity(1024);
            //     if let Err(e) =
            //         crate::ai::chat::chat(&robot_id, &prompt, ResultReceiver::StrBuf(&mut r)).await
            //     {
            //         log::info!("LlmChatNode response failed, err: {:?}", &e);
            //         drop(s);
            //         return;
            //     }
            //     if let Err(_) = s.send(r) {
            //         log::info!("LlmChatNode sent response failed.");
            //     }
            // });
            // match rev.recv() {
            //     Ok(s) => {
            //         log::info!("LLM response {}", &s);
            //         response.answers.push(AnswerData {
            //             text: s,
            //             answer_type: AnswerType::TextPlain,
            //         });
            //     }
            //     // Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
            //     Err(e) => log::info!("LlmChatNode response failed, err: {:?}", &e),
            // }
            // let mut s = String::with_capacity(1024);
            // if let Err(e) = tokio::runtime::Handle::current().block_on(async {
            //     crate::ai::chat::chat(&req.robot_id, &self.prompt, ResultReceiver::StrBuf(&mut s))
            //         .await
            // }) {
            //     log::info!("LlmChatNode response failed, err: {:?}", &e);
            // } else {
            //     log::info!("LLM response {}", &s);
            //     response.answers.push(AnswerData {
            //         text: s,
            //         answer_type: AnswerType::TextPlain,
            //     });
            // }
            true
        }
    }
}

impl RuntimeNode for LlmChatNode {
    async fn exec(
        &mut self,
        req: &Request,
        ctx: &mut Context,
        response: &mut ResponseData,
        channel_sender: &ResponseChannelWrapper,
    ) -> bool {
        // log::info!("Into LlmChatNode");
        let r = self.inner_exec(req, ctx, response, channel_sender).await;
        if r {
            let r = RuntimeNodeEnum::LlmChatNode(self.clone());
            let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&r).unwrap();
            ctx.node = Some(bytes.into_vec());
        } else {
            add_next_node(ctx, &self.next_node_id);
        }
        r
        /*
        self.cur_run_times = self.cur_run_times + 1;
        let mut check_contains_str: Option<&String> = None;
        match &self.exit_condition {
            LlmChatNodeExitCondition::Intent(i) => {
                if req.user_input_intent.is_some() && req.user_input_intent.as_ref().unwrap().eq(i)
                {
                    add_next_node(ctx, &self.next_node_id);
                    return false;
                }
            }
            LlmChatNodeExitCondition::SpecialInputs(s) => {
                if req.user_input.eq(s) {
                    // log::info!("886 {}", &self.next_node_id);
                    add_next_node(ctx, &self.next_node_id);
                    return false;
                }
            }
            LlmChatNodeExitCondition::LlmResultContains(s) => {
                check_contains_str = Some(s);
            }
            LlmChatNodeExitCondition::MaxChatTimes(t) => {
                if self.cur_run_times > *t {
                    add_next_node(ctx, &self.next_node_id);
                    return false;
                }
            }
        }
        let r = RuntimeNnodeEnum::LlmChatNode(self.clone());
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&r).unwrap();
        ctx.node = Some(bytes.into_vec());
        // log::info!("self.response_streaming {}", self.response_streaming);
        let chat_history = if ctx.chat_history.is_empty() {
            None
        } else {
            Some(ctx.chat_history.clone())
        };
        if self.response_streaming {
            // let r = super::facade::get_sender(req.session_id.as_ref().unwrap());
            // if r.is_err() {
            //     add_next_node(ctx, &self.next_node_id);
            //     return false;
            // }
            // let s_op = r.unwrap();
            // if s_op.is_none() {
            //     add_next_node(ctx, &self.next_node_id);
            //     return false;
            // }
            // let s = s_op.unwrap();
            // let ticket = String::new();
            let robot_id = req.robot_id.clone();
            let connect_timeout = self.connect_timeout.clone();
            let read_timeout = self.read_timeout.clone();
            // let (s, r) = tokio::sync::mpsc::channel::<String>(1);
            let (s, r) = tokio::sync::mpsc::channel::<String>(2);
            channel_sender.receiver = Some(r);
            tokio::task::spawn(async move {
                if let Err(e) = crate::ai::chat::chat(
                    &robot_id,
                    chat_history,
                    connect_timeout,
                    read_timeout,
                    ResultSender::ChannelSender(&s),
                )
                .await
                {
                    log::info!("LlmChatNode response failed, err: {:?}", &e);
                }
            });
            true
        } else {
            let now = std::time::Instant::now();
            let mut s = String::with_capacity(1024);
            if let Err(e) = tokio::task::block_in_place(|| {
                // log::info!("prompt |{}|", &self.prompt);
                // tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(crate::ai::chat::chat(
                        &req.robot_id,
                        chat_history,
                        self.connect_timeout,
                        self.read_timeout,
                        ResultSender::StrBuf(&mut s),
                    ))
                // })
            }) {
                log::error!("LlmChatNode response failed, err: {:?}", &e);
                match &self.answer_timeout_then {
                    LlmChatAnswerTimeoutThen::GotoAnotherNode => {
                        ctx.node = None;
                        add_next_node(ctx, &self.next_node_id);
                        return false;
                    }
                    LlmChatAnswerTimeoutThen::ResponseAlternateText(t) => s.push_str(t),
                    LlmChatAnswerTimeoutThen::DoNothing => return false,
                }
            } else {
                log::info!("LLM response |{}|", &s);
                if !s.is_empty() {
                    let mut contains_certain_str = false;
                    if check_contains_str.is_some() {
                        log::info!("check_contains_str |{}|", check_contains_str.as_ref().unwrap());
                        contains_certain_str = s.contains(check_contains_str.unwrap());
                        log::info!("contains_certain_str {}", contains_certain_str);
                    }
                    response.answers.push(AnswerData {
                        content: s,
                        content_type: AnswerContentType::TextPlain,
                    });
                    if contains_certain_str {
                        add_next_node(ctx, &self.next_node_id);
                        return false;
                    }
                }
            }
            log::info!("LLM response took {:?}", now.elapsed());
            // let (s, rev) = std::sync::mpsc::channel::<String>();
            // let robot_id = req.robot_id.clone();
            // let prompt = self.prompt.clone();
            // tokio::task::spawn(async move {
            //     let mut r = String::with_capacity(1024);
            //     if let Err(e) =
            //         crate::ai::chat::chat(&robot_id, &prompt, ResultReceiver::StrBuf(&mut r)).await
            //     {
            //         log::info!("LlmChatNode response failed, err: {:?}", &e);
            //         drop(s);
            //         return;
            //     }
            //     if let Err(_) = s.send(r) {
            //         log::info!("LlmChatNode sent response failed.");
            //     }
            // });
            // match rev.recv() {
            //     Ok(s) => {
            //         log::info!("LLM response {}", &s);
            //         response.answers.push(AnswerData {
            //             text: s,
            //             answer_type: AnswerType::TextPlain,
            //         });
            //     }
            //     // Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {}
            //     Err(e) => log::info!("LlmChatNode response failed, err: {:?}", &e),
            // }
            // let mut s = String::with_capacity(1024);
            // if let Err(e) = tokio::runtime::Handle::current().block_on(async {
            //     crate::ai::chat::chat(&req.robot_id, &self.prompt, ResultReceiver::StrBuf(&mut s))
            //         .await
            // }) {
            //     log::info!("LlmChatNode response failed, err: {:?}", &e);
            // } else {
            //     log::info!("LLM response {}", &s);
            //     response.answers.push(AnswerData {
            //         text: s,
            //         answer_type: AnswerType::TextPlain,
            //     });
            // }
            true
        }
        */
    }
}

#[derive(Archive, Clone, Deserialize, Serialize, serde::Deserialize)]
#[rkyv(compare(PartialEq))]
pub(crate) enum KnowledgeBaseAnswerNoRecallThen {
    GotoAnotherNode,
    ReturnAlternateAnswerInstead(String),
}

#[derive(Archive, Clone, Debug, Deserialize, Serialize, serde::Deserialize)]
#[rkyv(compare(PartialEq))]
pub(crate) enum KnowledgeBaseAnswerSource {
    QnA,
    Doc,
}

#[derive(Archive, Clone, Deserialize, Serialize)]
#[rkyv(compare(PartialEq))]
pub(crate) struct KnowledgeBaseAnswerNode {
    pub(super) recall_distance: f64,
    pub(super) retrieve_answer_sources: Vec<crate::flow::rt::node::KnowledgeBaseAnswerSource>,
    pub(super) no_recall_then: KnowledgeBaseAnswerNoRecallThen,
    pub(super) next_node_id: String,
}

impl KnowledgeBaseAnswerNode {
    async fn retrieve_qa_answer(&self, req: &Request) -> Option<String> {
        match crate::kb::qa::retrieve_answer(&req.robot_id, &req.user_input).await {
            Ok((answer, distance)) => {
                log::info!(
                    "distance {} recall_distance {}",
                    distance,
                    self.recall_distance
                );
                if let Some(a) = answer
                    && distance <= self.recall_distance
                {
                    Some(a.answer)
                } else {
                    None
                }
            }
            Err(e) => {
                log::error!("KnowledgeBaseAnswerNode retrieve QnA failed: {:?}", &e);
                None
            }
        }
    }
    async fn retrieve_doc_answer(&self, req: &Request) -> Option<String> {
        let r = crate::kb::doc::search_doc(
            &req.robot_id,
            &req.user_input,
            self.recall_distance,
            1000,
            5000,
        )
        .await;
        match r {
            Ok(op) => op,
            Err(e) => {
                log::warn!("KnowledgeBaseAnswerNode retrieve doc failed {:?}", &e);
                None
            }
        }
    }
    fn fallback_answer(
        &self,
        ctx: &mut Context,
        response: &mut ResponseData,
        channel_sender: &ResponseChannelWrapper,
    ) -> bool {
        match &self.no_recall_then {
            KnowledgeBaseAnswerNoRecallThen::GotoAnotherNode => {
                add_next_node(ctx, &self.next_node_id);
                false
            }
            KnowledgeBaseAnswerNoRecallThen::ReturnAlternateAnswerInstead(s) => {
                push_answer(
                    ctx,
                    response,
                    channel_sender,
                    s.clone(),
                    AnswerContentType::TextPlain,
                );
                let r = RuntimeNodeEnum::KnowledgeBaseAnswerNode(self.clone());
                let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&r).unwrap();
                ctx.node = Some(bytes.into_vec());
                true
            }
        }
    }
}

impl RuntimeNode for KnowledgeBaseAnswerNode {
    async fn exec(
        &mut self,
        req: &Request,
        ctx: &mut Context,
        response: &mut ResponseData,
        channel_sender: &ResponseChannelWrapper,
    ) -> bool {
        // log::info!("Into LlmChaKnowledgeBaseAnswerNodetNode");
        for answer_source in &self.retrieve_answer_sources {
            log::info!("answer_source={:?}", &answer_source);
            let r = match answer_source {
                KnowledgeBaseAnswerSource::QnA => self.retrieve_qa_answer(req).await,
                KnowledgeBaseAnswerSource::Doc => self.retrieve_doc_answer(req).await,
            };
            if let Some(content) = r
                && !content.is_empty()
            {
                // A retrieved answer is complete the moment it arrives, so it
                // is pushed like any other answer instead of being buffered.
                // Previously it ignored the channel, which meant a streamed
                // response never carried a knowledge base answer.
                push_answer(
                    ctx,
                    response,
                    channel_sender,
                    content,
                    AnswerContentType::TextPlain,
                );
                add_next_node(ctx, &self.next_node_id);
                return false;
            }
        }
        self.fallback_answer(ctx, response, channel_sender)
        /*
        let result = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(crate::kb::qa::retrieve_answer(
                &req.robot_id,
                &req.user_input,
            ))
        });
        match result {
            Ok((answer, distance)) => {
                log::info!(
                    "distance {} recall_distance {}",
                    distance,
                    self.recall_distance
                );
                if answer.is_some() && distance <= self.recall_distance {
                    response.answers.push(AnswerData {
                        text: answer.unwrap().answer,
                        answer_type: AnswerType::TextPlain,
                    });
                    add_next_node(ctx, &self.next_node_id);
                    false
                } else {
                    self.fallback_answer(ctx, response)
                }
            }
            Err(e) => {
                log::error!("KnowledgeBaseAnswerNode answer failed: {:?}", &e);
                self.fallback_answer(ctx, response)
            }
        }
        */
    }
}

pub(crate) fn deser_node(bytes: &[u8]) -> Result<RuntimeNodeEnum> {
    // let now = std::time::Instant::now();
    let mut v = AlignedVec::<256>::with_capacity(bytes.len());
    v.extend_from_slice(bytes);
    let r = rkyv::from_bytes::<RuntimeNodeEnum, rkyv::rancor::Error>(&v).unwrap();
    // let archived = rkyv::access::<ArchivedRuntimeNnodeEnum, rkyv::rancor::Error>(bytes).unwrap();
    // let deserialized = rkyv::deserialize::<RuntimeNnodeEnum, rkyv::rancor::Error>(archived).unwrap();
    // log::info!("deser_node time {:?}", now.elapsed());
    Ok(r)
}
