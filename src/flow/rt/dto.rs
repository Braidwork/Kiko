use std::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::{flow::subflow::dto::NextActionType, variable::dto::SimpleVariable};

#[derive(Deserialize, PartialEq, Eq)]
pub(crate) enum UserInputResult {
    Successful,
    Timeout,
}

#[derive(Deserialize)]
pub(crate) struct Request {
    #[serde(rename = "robotId")]
    pub(crate) robot_id: String,
    #[serde(rename = "mainFlowId")]
    pub(crate) main_flow_id: String,
    #[serde(rename = "sessionId")]
    pub(crate) session_id: Option<String>,
    #[serde(rename = "userInputResult")]
    pub(crate) user_input_result: UserInputResult,
    #[serde(rename = "userInput")]
    pub(crate) user_input: String,
    #[serde(default)]
    #[serde(rename = "attachments")]
    pub(crate) attachments: Vec<crate::ai::dto::Attachment>,
    #[serde(rename = "importVariables")]
    pub(crate) import_variables: Option<Vec<SimpleVariable>>,
    #[serde(rename = "userInputIntent")]
    pub(crate) user_input_intent: Option<String>,
    /// Whether the caller wants the answer pushed frame by frame. Absent means
    /// `false`, so a client that knows nothing about streaming still gets the
    /// single-document response it expects.
    #[serde(default)]
    #[serde(rename = "stream")]
    pub(crate) stream: bool,
}

#[derive(Serialize)]
pub(crate) struct CollectData {
    #[serde(rename = "varName")]
    pub(crate) var_name: String,
    pub(crate) value: String,
}

#[derive(Clone, Deserialize, Serialize, rkyv::Archive, rkyv::Deserialize, rkyv::Serialize)]
#[rkyv(compare(PartialEq))]
pub(crate) enum AnswerContentType {
    TextPlain,
    TextHtml,
}

#[derive(Serialize)]
pub(crate) struct AnswerData {
    pub(crate) content: String,
    #[serde(rename = "contentType")]
    pub(crate) content_type: AnswerContentType,
}

/// The channel an answer travels through when the caller asked for streaming.
/// Owned by the request handler, handed to the flow runner, read by nobody in
/// this process: the frames go straight out over HTTP.
pub(crate) struct ResponseChannelWrapper {
    sender: Option<tokio::sync::mpsc::UnboundedSender<StreamingResponseData>>,
}

impl ResponseChannelWrapper {
    /// Wraps an open channel: answers become frames as they are produced.
    pub(crate) fn new(sender: tokio::sync::mpsc::UnboundedSender<StreamingResponseData>) -> Self {
        Self {
            sender: Some(sender),
        }
    }
    /// No channel: answers are buffered into the response document. This is
    /// what a client that did not ask for streaming gets.
    pub(crate) fn none() -> Self {
        Self { sender: None }
    }
    /// Whether this request asked for a streamed answer.
    pub(crate) fn is_streaming(&self) -> bool {
        self.sender.is_some()
    }
    /// The raw channel, for handing to a generator that pushes its own deltas.
    pub(crate) fn sender(&self) -> Option<&tokio::sync::mpsc::UnboundedSender<StreamingResponseData>> {
        self.sender.as_ref()
    }
    /// Pushes one answer delta, tagged with the answer it belongs to.
    ///
    /// Returns `false` once the client is gone. Every generation loop stops on
    /// that, which is the only thing that ends a run nobody is listening to.
    pub(crate) fn push_frame(&self, content_seq: usize, content: String) -> bool {
        self.send(StreamingResponseData {
            content_seq: Some(content_seq),
            content,
        })
    }
    /// Pushes the terminal frame: the same `{status, data, err}` envelope a
    /// non-streaming request returns as a document, so a client reads a result
    /// the same way whatever the transport. A `None` sequence marks it terminal.
    pub(crate) fn push_terminal(&self, envelope: String) -> bool {
        self.send(StreamingResponseData {
            content_seq: None,
            content: envelope,
        })
    }
    fn send(&self, frame: StreamingResponseData) -> bool {
        match &self.sender {
            Some(s) => {
                if let Err(e) = s.send(frame) {
                    log::warn!("Failed to send frame: {e:?}");
                    false
                } else {
                    true
                }
            },
            None => false,
        }
    }
}

#[derive(Serialize)]
pub(crate) struct StreamingResponseData {
    #[serde(rename = "contentSeq")]
    pub(crate) content_seq: Option<usize>,
    pub(crate) content: String,
}

#[derive(Serialize)]
pub(crate) struct ResponseData {
    #[serde(rename = "sessionId")]
    pub(crate) session_id: String,
    pub(crate) answers: Vec<AnswerData>,
    #[serde(rename = "collectData")]
    pub(crate) collect_data: Vec<CollectData>,
    #[serde(rename = "nextAction")]
    pub(crate) next_action: NextActionType,
    #[serde(rename = "extraData")]
    pub(crate) extra_data: ExtraData,
    #[serde(rename = "sseReceiverTicket")]
    pub(crate) sse_receiver_ticket: String,
}

impl ResponseData {
    pub(crate) fn new(req: &Request) -> Self {
        Self {
            session_id: req.session_id.as_ref().unwrap().clone(),
            answers: Vec::with_capacity(5),
            collect_data: Vec::with_capacity(10),
            next_action: NextActionType::None,
            extra_data: ExtraData {
                external_link: String::new(),
            },
            sse_receiver_ticket: String::new(),
        }
    }
    // pub(crate) fn new_with_plain_text_answer(a: String) -> Self {
    //     Self {
    //         session_id: String::new(),
    //         answers: vec![AnswerData {
    //             content: a,
    //             content_type: AnswerContentType::TextPlain,
    //         }],
    //         collect_data: Vec::with_capacity(0),
    //         next_action: NextActionType::None,
    //         extra_data: ExtraData {
    //             external_link: String::new(),
    //         },
    //         sse_receiver_ticket: String::new(),
    //     }
    // }
}

#[derive(Serialize)]
pub(crate) struct ExtraData {
    #[serde(rename = "externalLink")]
    pub(crate) external_link: String,
}
