use std::collections::{HashMap, HashSet};
use std::vec::Vec;

use super::condition::ConditionData;
use super::node::{
    CollectNode, ConditionNode, ExternalHttpCallNode, GotoAnotherNode, GotoMainFlowNode,
    KnowledgeBaseAnswerNode, LlmChatNode, LlmGenTextNode, RuntimeNodeEnum, SendEmailNode,
    TerminateNode, TextNode,
};
use crate::flow::demo;
use crate::flow::subflow::dto::{BranchType, CanvasCells, NextActionType, Node, SubFlowDetail};
use crate::repo;
use crate::result::{Error, Result};

pub(crate) async fn convert_flow(is_en: bool, robot_id: &str, mainflow_id: &str) -> Result<()> {
    let flows: Vec<SubFlowDetail> = if let Some(t) = demo::get_demo(is_en, mainflow_id) {
        serde_json::from_str(t)?
    } else {
        let r = repo::subflow::list(repo::pool(), robot_id, mainflow_id).await?;
        if r.is_empty() {
            return Err(Error::WithMessage(String::from("Flow data not found")));
        }
        r
    };

    // 所有节点先收集、最后一次事务写入：release 要么整批生效，要么保留上一版。
    // demo 主流程没有 `mainflows` 行也没关系 —— 运行时节点按 (robot, main_flow_id)
    // 归属，`main_flow_id` 只是命名空间（见 §5.3）。
    let mut runtime_nodes: Vec<(String, Vec<u8>)> = Vec::with_capacity(32);
    for (idx, f) in flows.iter().enumerate() {
        convert_subflow(mainflow_id, idx, f, &mut runtime_nodes)?;
    }
    repo::runtime_node::upsert_many(repo::pool(), robot_id, mainflow_id, &runtime_nodes).await
}

fn validate_nodes(f: &SubFlowDetail, nodes: &Vec<&mut Node>) -> Result<()> {
    for node in nodes.iter() {
        node.is_valid(f)?;
    }
    Ok(())
}

fn check_first_node(
    mainflow_id: &str,
    flow_idx: usize,
    f: &SubFlowDetail,
    nodes: &mut Vec<&mut Node>,
) -> Result<()> {
    let mut ids: HashSet<String> = HashSet::with_capacity(16);
    for node in nodes.iter() {
        ids.insert(node.get_node_id());
    }
    for node in nodes.iter() {
        node.get_branch_target_ids().iter().for_each(|id| {
            ids.remove(id);
        });
    }
    if ids.len() == 1 {
        let mut id = String::with_capacity(36);
        for i in ids.drain() {
            id.push_str(i.as_str());
        }
        for node in nodes.iter_mut() {
            let first_node_id = if flow_idx == 0 { mainflow_id } else { &f.id };
            if node.get_node_id().eq(&id) {
                match node {
                    Node::DialogNode(n) => n.node_id = String::from(first_node_id),
                    Node::LlmChatNode(n) => n.node_id = String::from(first_node_id),
                    Node::ConditionNode(n) => n.node_id = String::from(first_node_id),
                    Node::CollectNode(n) => n.node_id = String::from(first_node_id),
                    Node::GotoNode(n) => n.node_id = String::from(first_node_id),
                    Node::ExternalHttpNode(n) => n.node_id = String::from(first_node_id),
                    Node::SendEmailNode(n) => n.node_id = String::from(first_node_id),
                    Node::EndNode(n) => n.node_id = String::from(first_node_id),
                    Node::KnowledgeBaseAnswerNode(n) => n.node_id = String::from(first_node_id),
                };
            }
        }
        Ok(())
    } else {
        Err(Error::WithMessage(format!(
            "Sub flow: {} can not find the start node.",
            f.name
        )))
    }
}

fn convert_subflow(
    mainflow_id: &str,
    flow_idx: usize,
    f: &SubFlowDetail,
    runtime_nodes: &mut Vec<(String, Vec<u8>)>,
) -> Result<()> {
    let mut cells: CanvasCells = serde_json::from_str(&f.canvas)?;
    let mut branches_link: HashMap<String, String> = HashMap::with_capacity(32);
    let mut node_cnt = 0usize;
    for n in cells.cells.iter() {
        if n.shape.eq("edge") {
            let source = n.extra.get("source");
            let port = source.as_ref().unwrap().get("port");
            let target = n.extra.get("target");
            let cell = target.as_ref().unwrap().get("cell");
            branches_link.insert(
                String::from(port.unwrap().as_str().unwrap()),
                String::from(cell.unwrap().as_str().unwrap()),
            );
        } else {
            if n.data.is_none() {
                return Err(Error::WithMessage(String::from(
                    "Node data information not found",
                )));
            }
            node_cnt += 1;
        }
    }
    let mut nodes: Vec<&mut Node> = Vec::with_capacity(node_cnt);
    for n in cells.cells.iter_mut() {
        if let Some(node) = n.data.as_mut() {
            if let Some(branches) = node.get_branches() {
                for branch in branches.iter_mut() {
                    if branch.branch_id.is_empty() {
                        return Err(Error::WithMessage(format!(
                            "Branch '{}' of '{}' id information not found",
                            branch.branch_name, f.name
                        )));
                    }
                    let target_node_id = branches_link.remove(branch.branch_id.as_str());
                    if let Some(t) = target_node_id {
                        branch.target_node_id = t;
                    } else {
                        return Err(Error::WithMessage(format!(
                            "Branch '{}' of '{}' target id information not found",
                            branch.branch_name, f.name
                        )));
                    }
                }
            }
            nodes.push(node);
        }
    }

    validate_nodes(f, &nodes)?;
    check_first_node(mainflow_id, flow_idx, f, &mut nodes)?;
    for node in nodes {
        convert_node(node, runtime_nodes)?;
    }
    Ok(())
}

/// 把一个画布节点序列化成运行时节点。写失败返回错误，不再 `unwrap`（§16.1）。
fn push_node(
    nodes: &mut Vec<(String, Vec<u8>)>,
    node_id: String,
    node: RuntimeNodeEnum,
) -> Result<()> {
    let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&node)
        .map_err(|e| Error::WithMessage(format!("Serialize runtime node failed: {e}")))?;
    nodes.push((node_id, bytes.as_slice().to_vec()));
    Ok(())
}

fn convert_node(node: &mut Node, nodes: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
    match node {
        Node::DialogNode(n) => match n.dialog_text_source {
            crate::flow::subflow::dto::DialogTextSource::FixedText => {
                let node = TextNode {
                    text: std::mem::take(&mut n.dialog_text),
                    text_type: n.dialog_text_type.clone(),
                    ret: NextActionType::WaitUserResponse == n.next_step,
                    next_node_id: n.branches[0].target_node_id.clone(),
                };
                push_node(nodes, n.node_id.clone(), RuntimeNodeEnum::TextNode(node))?;
            }
            crate::flow::subflow::dto::DialogTextSource::LlmGenText => {
                let node = LlmGenTextNode {
                    prompt: std::mem::take(&mut n.dialog_llm_gen_prompt),
                    fallback_text: std::mem::take(&mut n.dialog_fallback_text),
                    context_len: n.context_length,
                    connect_timeout: n.connect_timeout,
                    read_timeout: n.read_timeout,
                    response_streaming: n.response_streaming,
                    enable_thinking: n.enable_thinking,
                    ret: NextActionType::WaitUserResponse == n.next_step,
                    next_node_id: n.branches[0].target_node_id.clone(),
                };
                push_node(
                    nodes,
                    n.node_id.clone(),
                    RuntimeNodeEnum::LlmGenTextNode(node),
                )?;
            }
        },
        Node::LlmChatNode(n) => {
            let node = LlmChatNode {
                context_len: n.context_length,
                cur_run_times: 0,
                exit_condition: n.exit_condition.clone(),
                answer_timeout_then: n.when_timeout_then.clone(),
                connect_timeout: n.connect_timeout,
                read_timeout: n.read_timeout,
                response_streaming: n.response_streaming,
                enable_thinking: n.enable_thinking,
                next_node_id: n.branches[0].target_node_id.clone(),
            };
            push_node(nodes, n.node_id.clone(), RuntimeNodeEnum::LlmChatNode(node))?;
        }
        Node::ConditionNode(n) => {
            let mut cnt = 1u8;
            for b in n.branches.iter() {
                let node_id = if cnt == 1 {
                    n.node_id.clone()
                } else {
                    format!("{}-{}", n.node_id, cnt)
                };
                cnt += 1;
                if BranchType::GotoAnotherNode == b.branch_type {
                    let node = GotoAnotherNode {
                        next_node_id: b.target_node_id.clone(),
                    };
                    push_node(nodes, node_id, RuntimeNodeEnum::GotoAnotherNode(node))?;
                } else {
                    let mut conditions: Vec<Vec<ConditionData>> = Vec::with_capacity(10);
                    for and_condition in b.condition_group.iter() {
                        let mut and_conditions: Vec<ConditionData> = Vec::with_capacity(10);
                        for cond in and_condition.iter() {
                            let c = ConditionData {
                                condition_type: cond.condition_type,
                                compare_type: cond.compare_type,
                                ref_data: cond.ref_choice.clone(),
                                target_data: cond.target_value.clone(),
                                target_data_variant: cond.target_value_variant,
                                case_sensitive_comparison: cond.case_sensitive_comparison,
                            };
                            and_conditions.push(c);
                        }
                        conditions.push(and_conditions);
                    }
                    let node = ConditionNode {
                        next_node_id: format!("{}-{}", n.node_id, cnt),
                        goto_node_id: b.target_node_id.clone(),
                        conditions,
                    };
                    push_node(nodes, node_id, RuntimeNodeEnum::ConditionNode(node))?;
                }
            }
        }
        Node::CollectNode(n) => {
            let mut successful_node_id = String::with_capacity(36);
            let mut failed_node_id = String::with_capacity(36);
            for b in n.branches.iter() {
                match b.branch_type {
                    BranchType::InfoCollectedSuccessfully => {
                        successful_node_id.push_str(b.target_node_id.as_str())
                    }
                    BranchType::GotoAnotherNode => {
                        failed_node_id.push_str(b.target_node_id.as_str())
                    }
                    _ => {
                        return Err(Error::WithMessage(String::from(
                            "Unknown collection branch type",
                        )));
                    }
                };
            }
            let node = CollectNode {
                var_name: n.collect_save_var_name.clone(),
                collect_type: n.collect_type.clone(),
                successful_node_id,
                failed_node_id,
            };
            push_node(nodes, n.node_id.clone(), RuntimeNodeEnum::CollectNode(node))?;
        }
        Node::ExternalHttpNode(n) => {
            let (successful_node_id, next_node_id) = if n.branches.len() == 2 {
                (
                    n.branches[0].target_node_id.clone(),
                    n.branches[1].target_node_id.clone(),
                )
            } else {
                (String::new(), n.branches[0].target_node_id.clone())
            };
            let node = ExternalHttpCallNode {
                successful_node_id,
                next_node_id,
                http_api_id: n.http_api_id.clone(),
                connect_timeout_milliseconds: n.connect_timeout_milliseconds,
                read_timeout_milliseconds: n.read_timeout_milliseconds,
                async_req: n.async_req,
            };
            push_node(
                nodes,
                n.node_id.clone(),
                RuntimeNodeEnum::ExternalHttpCallNode(node),
            )?;
        }
        Node::SendEmailNode(n) => {
            let successful_node_id = std::mem::take(&mut n.branches[0].target_node_id);
            let goto_node_id = {
                if n.async_send {
                    None
                } else {
                    Some(std::mem::take(&mut n.branches[1].target_node_id))
                }
            };
            let node = SendEmailNode {
                from: std::mem::take(&mut n.from),
                to_recipients: std::mem::take(&mut n.to_recipients),
                cc_recipients: std::mem::take(&mut n.cc_recipients),
                bcc_recipients: std::mem::take(&mut n.bcc_recipients),
                subject: std::mem::take(&mut n.subject),
                content: std::mem::take(&mut n.content),
                content_type: std::mem::take(&mut n.content_type),
                async_send: n.async_send,
                successful_node_id,
                goto_node_id,
            };
            push_node(
                nodes,
                n.node_id.clone(),
                RuntimeNodeEnum::SendEmailNode(node),
            )?;
        }
        Node::GotoNode(n) => match n.goto_type {
            NextActionType::GotoMainFlow => {
                let node = GotoMainFlowNode {
                    main_flow_id: n.goto_mainflow_id.clone(),
                    next_node_id: n.goto_subflow_id.clone(),
                };
                push_node(
                    nodes,
                    n.node_id.clone(),
                    RuntimeNodeEnum::GotoMainFlowNode(node),
                )?;
            }
            NextActionType::GotoSubFlow => {
                let node = GotoAnotherNode {
                    next_node_id: n.goto_subflow_id.clone(),
                };
                push_node(
                    nodes,
                    n.node_id.clone(),
                    RuntimeNodeEnum::GotoAnotherNode(node),
                )?;
            }
            NextActionType::GotoExternalLink => {}
            _ => {}
        },
        Node::EndNode(n) => {
            let ter_bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&RuntimeNodeEnum::TerminateNode(
                TerminateNode {},
            ))
            .map_err(|e| Error::WithMessage(format!("Serialize runtime node failed: {e}")))?;
            if n.ending_text.is_empty() {
                nodes.push((n.node_id.clone(), ter_bytes.as_slice().to_vec()));
            } else {
                let end_node_id = format!("{}-2", n.node_id);
                let node = TextNode {
                    text: n.ending_text.clone(),
                    text_type: super::dto::AnswerContentType::TextPlain,
                    ret: false,
                    next_node_id: end_node_id.clone(),
                };
                push_node(nodes, n.node_id.clone(), RuntimeNodeEnum::TextNode(node))?;
                nodes.push((end_node_id, ter_bytes.as_slice().to_vec()));
            }
        }
        Node::KnowledgeBaseAnswerNode(n) => {
            let node = KnowledgeBaseAnswerNode {
                recall_distance: 1f64 - n.recall_thresholds as f64 / 100f64,
                retrieve_answer_sources: n.retrieve_answer_sources.clone(),
                no_recall_then: n.no_answer_then.clone(),
                next_node_id: n.branches[0].target_node_id.clone(),
            };
            push_node(
                nodes,
                n.node_id.clone(),
                RuntimeNodeEnum::KnowledgeBaseAnswerNode(node),
            )?;
        }
    };
    Ok(())
}

/*
#[derive(Archive, Deserialize, Serialize)]
#[archive(compare(PartialEq), check_bytes)]
struct MyNode {}

trait TestT {
    fn say(&self) -> String;
}

impl TestT for MyNode {
    fn say(&self) -> String {
        String::from("Into MyNode")
    }
}

pub fn t1() {
    let mut serializer = AllocSerializer::<512>::default();
    let nn: MyNode = MyNode {};
    serializer.serialize_value(&nn).unwrap();
    let mut bytes = serializer.into_serializer().into_inner();
    let b = bytes.as_slice();
    bytes.push(1u8);
    bytes.last();
    let archived = rkyv::check_archived_root::<MyNode>(&bytes[..]).unwrap();
    let deserialized: MyNode = archived.deserialize(&mut rkyv::Infallible).unwrap();
    t2(deserialized);
}

fn t2(n: impl TestT) {
    println!("{}", n.say());
}
*/
