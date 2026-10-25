//! 文档知识库（B-21 / B-31 / B-32）。
//!
//! 存储从 turso 搬到 PG：
//!
//! | A（turso） | B（PG） |
//! | --- | --- |
//! | `{robot_id}.doc_content` | `documents.doc_content` |
//! | `{robot_id}_vec.chunk_text` | `doc_chunks.chunk_text`（原文） |
//! | —— | `doc_chunks.chunk_text_norm`（小写，只给关键词 `LIKE`） |
//! | `{robot_id}_vec.chunk_vec` | `vector_items`（`collection = 'doc'`） |
//! | 上传文件被丢掉 | `data/uploads/<robot_id>/<doc_id>.<ext>`（D30） |
//!
//! 检索仍是**向量召回 + 关键词召回 → RRF 融合**（§16.3 / B-32），只是：
//! - 向量召回走进程内索引（`vector::search`），分数是**相似度**（D41）；
//! - 关键词召回走 PG 的 `chunk_text_norm LIKE`，归一化在 Rust 侧做，
//!   不依赖数据库 collation（§15.4 S4.6）；
//! - 顺手修掉 A 的大小写 bug：`extract_keywords` 小写化、`chunk_text` 没有，
//!   所以英文关键词常常匹配不到。现在两边都用同一份 `normalize_text`。

use std::collections::HashMap;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::vec::Vec;

use quick_xml::Reader;
use quick_xml::events::Event;
use sha2::{Digest, Sha256};
use text_splitter::TextSplitter;
use zip::ZipArchive;

use super::dto::DocData;
use crate::ai::embedding;
use crate::man::settings;
use crate::repo;
use crate::repo::doc::{ChunkInput, NewDocument};
use crate::result::{Error, Result};
use crate::vector;

/// 上传原文件的根目录（相对仓库根 / 运行时工作目录，与 `data/` 一致）。
const UPLOADS: &str = "uploads";

pub(crate) async fn list(robot_id: &str) -> Result<Vec<DocData>> {
    let rows = repo::doc::list(repo::pool(), robot_id).await?;
    Ok(rows
        .into_iter()
        .map(|d| DocData {
            id: d.id,
            version: Some(d.version),
            file_name: d.file_name,
            file_size: d.file_size,
            doc_content: d.doc_content,
        })
        .collect())
}

/// 保存一篇文档：正文 + 分块 + 向量（一个事务），原文件落到 `data/uploads/`。
///
/// 顺序是有意的：先写文件、再写库。库写失败时把刚写的文件删掉（尽力而为），
/// 反过来会留下一条 `original_path` 指向不存在的文件的记录。
pub(crate) async fn save(
    robot_id: &str,
    file_name: &str,
    mime_type: &str,
    bytes: &[u8],
) -> Result<String> {
    let doc_content = match mime_type {
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
            parse_docx(bytes.to_vec())?
        }
        _ => return Err(Error::WithMessage(String::from("Unsupported format"))),
    };
    let doc_id = scru128::new_string();
    let rel_path = upload_rel_path(robot_id, &doc_id, file_name);
    write_upload(&rel_path, bytes).await?;

    let result = save_with_chunks(
        robot_id,
        &doc_id,
        file_name,
        bytes.len() as i64,
        mime_type,
        &rel_path,
        bytes,
        &doc_content,
    )
    .await;
    if result.is_err() {
        remove_upload(&rel_path).await;
    }
    result.map(|()| doc_content)
}

#[allow(clippy::too_many_arguments)]
async fn save_with_chunks(
    robot_id: &str,
    doc_id: &str,
    file_name: &str,
    file_size: i64,
    mime_type: &str,
    original_path: &str,
    bytes: &[u8],
    doc_content: &str,
) -> Result<()> {
    let (chunks, model) = chunks_with_vectors(robot_id, doc_content, EmbedMode::Active).await?;
    let doc = NewDocument {
        file_name: String::from(file_name),
        file_size,
        mime_type: Some(String::from(mime_type)),
        original_path: Some(String::from(original_path)),
        content_hash: Some(sha256_hex(bytes)),
        doc_content: String::from(doc_content),
    };
    let vec_ids = repo::doc::create(repo::pool(), robot_id, doc_id, &doc, &chunks, &model).await?;
    // 写后同步：只有新增，不需要作废旧缓存。
    let rows: Vec<(u64, Vec<f32>)> = vec_ids
        .iter()
        .zip(chunks.iter())
        .map(|(id, c)| (*id as u64, c.vec.clone()))
        .collect();
    vector::upsert(robot_id, repo::vector::DOC, &rows).await;
    if let Err(e) = settings::stamp_embedding_index(robot_id).await {
        log::warn!("Stamping embedding index of {robot_id} failed: {e:?}");
    }
    Ok(())
}

/// 改正文：整篇重新分块 + 重算向量（同一个事务）。`version` 是乐观锁（B-17）。
pub(crate) async fn update(
    robot_id: &str,
    doc_id: &str,
    doc_content: &str,
    version: Option<i64>,
) -> Result<()> {
    let (chunks, model) = chunks_with_vectors(robot_id, doc_content, EmbedMode::Active).await?;
    let r = repo::doc::update_content(
        repo::pool(),
        robot_id,
        doc_id,
        doc_content,
        &chunks,
        &model,
        version,
    )
    .await?;
    if r.is_none() {
        return Err(Error::WithMessage(format!(
            "Document {doc_id} does not exist any more."
        )));
    }
    // 分块整体替换 → 旧 chunk id 的向量行没了，索引里可能还留着它们。
    vector::invalidate(robot_id, repo::vector::DOC);
    if let Err(e) = settings::stamp_embedding_index(robot_id).await {
        log::warn!("Stamping embedding index of {robot_id} failed: {e:?}");
    }
    Ok(())
}

/// 重建用：把一篇文档的分块与向量写到**指定 generation**（D47 的续跑单位）。
///
/// 文档在这期间被删掉时返回 0，由编排层跳过。整篇替换分块是原子的：换模型可能换
/// 切法（本地 HF 500 字 vs 长上下文 2000 字），原地改 chunk_text 会让新旧分块混在
/// 一起。
pub(crate) async fn reindex_at(robot_id: &str, doc_id: &str, generation: i64) -> Result<usize> {
    let Some(doc) = repo::doc::get_payload(repo::pool(), robot_id, doc_id).await? else {
        return Ok(0);
    };
    let (chunks, model) =
        chunks_with_vectors(robot_id, &doc.doc_content, EmbedMode::Configured).await?;
    repo::doc::reindex_document(repo::pool(), robot_id, doc_id, generation, &chunks, &model).await
}

pub(crate) async fn delete(robot_id: &str, doc_id: &str) -> Result<()> {
    let original_path = repo::doc::delete(repo::pool(), robot_id, doc_id).await?;
    if let Some(p) = original_path {
        remove_upload(&p).await;
    }
    vector::invalidate(robot_id, repo::vector::DOC);
    Ok(())
}

/// 机器人被删时清掉整个上传目录（A 里这段逻辑被注释掉了，§5.6 要求 B 恢复）。
pub(crate) async fn remove_uploads(robot_id: &str) -> Result<()> {
    let p = uploads_root().join(robot_id);
    if p.exists()
        && let Err(e) = tokio::fs::remove_dir_all(&p).await
    {
        log::warn!("Removing uploads of {robot_id} failed: {e}");
    }
    Ok(())
}

// --------------------------------------------------------------------------- 检索

pub(crate) async fn search_doc(
    robot_id: &str,
    query: &str,
    recall_similarity: f32,
    connect_timeout: u32,
    read_timeout: u32,
) -> Result<Option<String>> {
    // 检索用的嵌入走 active model（§6.4 / B-23），与存储向量的那一代保持一致。
    let (q, _) = vector::embed_for_query(robot_id, repo::vector::DOC, query).await?;
    if q.is_empty() {
        return Ok(None);
    }
    // 向量召回：多取一些（8，与 A 的 LIMIT 8 对齐），再按阈值过滤。
    let hits = vector::search(robot_id, repo::vector::DOC, &q, 8).await?;
    let ids: Vec<i64> = hits.iter().map(|h| h.id as i64).collect();
    let payload = repo::doc::chunks_for_vectors(repo::pool(), robot_id, &ids).await?;
    let by_vector: HashMap<i64, (String, String)> = payload
        .into_iter()
        .map(|(vid, cid, text)| (vid, (cid, text)))
        .collect();
    let vec_hits: Vec<(String, String)> = hits
        .iter()
        .filter(|h| vector::is_hit(h.score, recall_similarity))
        .filter_map(|h| {
            by_vector
                .get(&(h.id as i64))
                .map(|(cid, text)| (cid.clone(), text.clone()))
        })
        .collect();

    // 关键词召回 + RRF 融合（B-32）。失败时退回纯向量，和 A 一样不报错。
    let chunks = match keyword_recall(robot_id, query).await {
        Ok(kw_hits) => rrf_merge(&vec_hits, &kw_hits),
        Err(e) => {
            log::warn!("Keyword recall failed, fallback to vector only, err: {e:?}");
            vec_hits.iter().take(4).map(|(_, t)| t.clone()).collect()
        }
    };
    if chunks.is_empty() {
        return Ok(None);
    }
    let prompts = vec![
        crate::ai::chat::Prompt {
            role: String::from("system"),
            content: String::from(
                "你是一个专业的文档助手。请根据提供的文档内容回答问题。\
                 如果文档内容中没有相关信息，请明确说明。\
                 回答要基于文档内容，不要编造信息。",
            ),
        },
        crate::ai::chat::Prompt {
            role: String::from("user"),
            content: format!("文档内容：\n{}\n\n问题：{}", chunks.join("\n\n"), query),
        },
    ];
    let mut s = String::with_capacity(1024);
    if let Err(e) = crate::ai::chat::chat(
        robot_id,
        Some(prompts),
        None,
        Some(connect_timeout),
        Some(read_timeout),
        false,
        crate::ai::chat::ResultSender::StrBuf(&mut s),
    )
    .await
    {
        log::error!("LlmChatNode response failed, err: {e:?}");
        return Ok(None);
    }
    Ok(Some(s))
}

/// 关键词召回：命中分块 → 按"命中几个关键词"排序，取前 8。
async fn keyword_recall(robot_id: &str, query: &str) -> Result<Vec<(String, String)>> {
    let keywords = extract_keywords(query);
    if keywords.is_empty() {
        return Ok(Vec::new());
    }
    let patterns: Vec<String> = keywords.iter().map(|k| like_pattern(k)).collect();
    let hits = repo::doc::keyword_recall(repo::pool(), robot_id, &patterns, 64).await?;
    let mut scored: Vec<(String, String, usize)> = hits
        .into_iter()
        .map(|h| {
            let score = keywords
                .iter()
                .filter(|k| h.chunk_text_norm.contains(k.as_str()))
                .count();
            (h.id, h.chunk_text, score)
        })
        .collect();
    // 只留命中至少两个关键词的分块（命中不到两个时退化成 1），避免一个常见词
    // 淹掉召回榜。与 A 的 `min_score` 规则一致。
    scored.sort_by_key(|h| std::cmp::Reverse(h.2));
    let min_score = scored.first().map(|h| h.2).unwrap_or(0).clamp(1, 2);
    scored.retain(|h| h.2 >= min_score);
    scored.truncate(8);
    Ok(scored.into_iter().map(|(id, text, _)| (id, text)).collect())
}

/// Reciprocal Rank Fusion：两个榜单各自 1/(60+rank)，同一条分块两边都出现就叠加。
fn rrf_merge(vec_hits: &[(String, String)], kw_hits: &[(String, String)]) -> Vec<String> {
    let mut fused: HashMap<&str, (f64, &str)> = HashMap::new();
    for hits in [vec_hits, kw_hits] {
        for (rank, (id, text)) in hits.iter().enumerate() {
            let score = 1.0 / (60.0 + rank as f64);
            match fused.entry(id.as_str()) {
                std::collections::hash_map::Entry::Occupied(mut e) => {
                    e.get_mut().0 += score;
                }
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert((score, text.as_str()));
                }
            }
        }
    }
    let mut ranked: Vec<(f64, &str)> = fused.into_values().collect();
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    ranked
        .into_iter()
        .take(4)
        .map(|(_, t)| String::from(t))
        .collect()
}

/// 应用层归一化：小写 + 保持原文其余部分。关键词侧与 `chunk_text_norm` 必须
/// 用**同一个函数**，否则客户的库是 `C` 还是 `zh_CN.UTF-8` 会悄悄改变匹配结果。
fn normalize_text(s: &str) -> String {
    s.to_lowercase()
}

/// 抽取检索关键词：ASCII 词（>= 2 字符）小写化；中文按 bigram 切分
/// （"知识库" → "知识" / "识库"），不需要任何分词器。
fn extract_keywords(query: &str) -> Vec<String> {
    let mut kws: Vec<String> = Vec::new();
    let mut cjk = String::new();
    let r = regex::Regex::new(r"[0-9A-Za-z_]{2,}|\p{Han}").unwrap();
    for m in r.find_iter(query) {
        let t = m.as_str();
        if t.chars().next().unwrap().is_ascii() {
            let w = normalize_text(t);
            if !kws.contains(&w) {
                kws.push(w);
            }
        } else {
            cjk.push_str(t);
        }
    }
    let chars: Vec<char> = cjk.chars().collect();
    if chars.len() <= 2 {
        if !chars.is_empty() {
            let w: String = chars.iter().collect();
            if !kws.contains(&w) {
                kws.push(w);
            }
        }
    } else {
        for w in chars.windows(2) {
            let w: String = w.iter().collect();
            if !kws.contains(&w) {
                kws.push(w);
            }
        }
    }
    kws.truncate(8);
    kws
}

/// LIKE 模式：**子串匹配**（§16.3 的 `LIKE '%kw%'`）。
///
/// A 的实现只把关键词转义后直接塞进 `LIKE ?`，没有加 `%` —— 那等于"整段分块必须
/// 恰好等于一个关键词"，关键词召回事实上永远不会命中（§16.3 说它是 `%kw%`，代码
/// 与文档不符）。B 按文档的意图补上通配符：`%` + 转义后的关键词 + `%`。
fn like_pattern(k: &str) -> String {
    format!("%{}%", escape_like(k))
}

/// LIKE 通配符转义（配合 SQL 里的 `ESCAPE '\'`）。
fn escape_like(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

// --------------------------------------------------------------------------- 分块

/// 分块时用哪套模型（§6.4 / B-23）。
#[derive(Clone, Copy, PartialEq, Eq)]
enum EmbedMode {
    /// 上传/编辑：跟随 active model（有就用它，没有就用当前配置并记进这一代）。
    Active,
    /// 重建：用当前配置 —— 重建的目的就是换模型。
    Configured,
}

/// 分块 + 推理 + 模型元数据。**不开事务**：一份长文档要跑几百个 chunk 的推理。
async fn chunks_with_vectors(
    robot_id: &str,
    doc_content: &str,
    mode: EmbedMode,
) -> Result<(Vec<ChunkInput>, repo::vector::ModelRef)> {
    // 本地 HF BERT 模型上限 512 token，分块要小；长上下文远端模型可以大很多。
    let long_context = !matches!(
        crate::man::settings::get_settings(robot_id)
            .await?
            .map(|s| s.sentence_embedding_provider.provider.clone()),
        Some(embedding::SentenceEmbeddingProvider::HuggingFace(_))
    );
    let texts = if long_context {
        chunk_text_semantic(doc_content, 2000, 200)?
    } else {
        chunk_text(doc_content, 500, 70)
    };
    if texts.is_empty() {
        // 空文档：没有分块也没有向量。模型定义在这里拿不到维度，用当前配置的期望
        // 维度（0 = 未知）占位——没有向量行时它不会被任何查询用到。
        let model = match mode {
            EmbedMode::Active => match vector::active_model(robot_id, repo::vector::DOC).await? {
                Some(m) => m,
                None => vector::current_model(robot_id, 0).await?,
            },
            EmbedMode::Configured => vector::current_model(robot_id, 0).await?,
        };
        return Ok((Vec::new(), model));
    }
    let (embeddings, model) = match mode {
        EmbedMode::Active => vector::embed_for_write(robot_id, repo::vector::DOC, &texts).await?,
        EmbedMode::Configured => vector::embed_configured(robot_id, &texts).await?,
    };
    let chunks = texts
        .into_iter()
        .zip(embeddings)
        .enumerate()
        .map(|(position, (text, vec))| ChunkInput {
            position: position as i32,
            norm: normalize_text(&text),
            text,
            vec,
        })
        .collect();
    Ok((chunks, model))
}

// chunk_size and overlap are counted in characters, so this works for both
// Chinese (no whitespace between words) and English texts.
// Paragraphs are kept intact whenever they fit into one chunk.
fn chunk_text(text: &str, chunk_size: usize, overlap: usize) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let mut chunks: Vec<String> = Vec::new();
    let mut current = String::with_capacity(chunk_size * 4);
    for para in text.split('\n') {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        let para_len = para.chars().count();
        if para_len > chunk_size {
            // A single paragraph longer than chunk_size: split it into
            // sliding windows of characters.
            if !current.is_empty() {
                chunks.push(std::mem::take(&mut current));
            }
            let chars: Vec<char> = para.chars().collect();
            let mut start = 0;
            while start < chars.len() {
                let end = std::cmp::min(start + chunk_size, chars.len());
                chunks.push(chars[start..end].iter().collect());
                if end == chars.len() {
                    break;
                }
                start = end - overlap.min(chunk_size - 1);
            }
        } else {
            let cur_len = current.chars().count();
            if !current.is_empty() && cur_len + para_len + 1 > chunk_size {
                // Flush the current chunk, carrying its tail over as overlap.
                let tail: String = current
                    .chars()
                    .skip(current.chars().count().saturating_sub(overlap))
                    .collect();
                chunks.push(std::mem::take(&mut current));
                current = tail;
            }
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(para);
        }
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

// Semantic chunking via text-splitter: splits at sentence/paragraph boundaries
// while maximizing chunk length. Suitable for long-context embedding models
// (OpenAI / Ollama etc.), where a much larger chunk capacity is affordable.
fn chunk_text_semantic(text: &str, chunk_size: usize, overlap: usize) -> Result<Vec<String>> {
    let splitter = TextSplitter::new(
        text_splitter::ChunkConfig::new(chunk_size)
            .with_overlap(overlap)
            .map_err(|e| Error::WithMessage(format!("Invalid chunk config: {e:?}")))?,
    );
    Ok(splitter.chunks(text).map(String::from).collect())
}

// --------------------------------------------------------------------------- 原文件

fn uploads_root() -> PathBuf {
    Path::new(".").join("data").join(UPLOADS)
}

/// 相对 `data/` 的路径，存进 `documents.original_path`。
///
/// 文件名只取**扩展名**（清洗过），绝不把用户给的文件名拼进路径，避免路径穿越。
fn upload_rel_path(robot_id: &str, doc_id: &str, file_name: &str) -> String {
    let ext = Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            e.chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .take(16)
                .collect::<String>()
                .to_lowercase()
        })
        .filter(|e| !e.is_empty());
    match ext {
        Some(ext) => format!("{UPLOADS}/{robot_id}/{doc_id}.{ext}"),
        None => format!("{UPLOADS}/{robot_id}/{doc_id}"),
    }
}

async fn write_upload(rel_path: &str, bytes: &[u8]) -> Result<()> {
    let abs = Path::new(".").join("data").join(rel_path);
    if let Some(dir) = abs.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    tokio::fs::write(&abs, bytes).await?;
    Ok(())
}

async fn remove_upload(rel_path: &str) {
    let abs = Path::new(".").join("data").join(rel_path);
    match tokio::fs::remove_file(&abs).await {
        Ok(()) => (),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
        Err(e) => log::warn!("Removing upload {} failed: {e}", abs.display()),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let out = h.finalize();
    let mut s = String::with_capacity(64);
    for b in out {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

// --------------------------------------------------------------------------- docx

pub(super) fn parse_docx(b: Vec<u8>) -> Result<String> {
    let mut doc_text = String::with_capacity(3096);
    let reader = Cursor::new(b);
    let mut archive = ZipArchive::new(reader)?;
    let mut zip_file = archive.by_name("word/document.xml")?;
    let mut cache = String::with_capacity(zip_file.size() as usize);
    zip_file.read_to_string(&mut cache)?;

    let mut reader = Reader::from_str(&cache);
    reader.config_mut().trim_text(false);
    let mut in_paragraph = false;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) if e.name().0.eq("w:p") => in_paragraph = true,
            Ok(Event::End(ref e)) if e.name().0.eq("w:p") => {
                doc_text.push('\n');
                in_paragraph = false;
            }
            Ok(Event::Empty(ref e)) if e.name().0.eq("w:p") => doc_text.push('\n'),
            Ok(Event::Text(e)) if in_paragraph => {
                doc_text.push_str(e.as_ref());
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(Error::WithMessage(format!(
                    "Parsing word/document.xml failed at position {}: {e:?}",
                    reader.error_position()
                )));
            }
            _ => (),
        }
    }
    Ok(doc_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_are_lowercased_and_split_into_cjk_bigrams() {
        let k = extract_keywords("Rust 知识库 Q&A");
        assert!(k.contains(&String::from("rust")));
        assert!(k.contains(&String::from("知识")));
        assert!(k.contains(&String::from("识库")));
        assert!(!k.iter().any(|w| w.contains('&')), "标点不是关键词"); // bigram 只切中文，不切英文单词。
        assert!(!k.iter().any(|w| w == "知"));
    }

    #[test]
    fn keywords_handle_short_cjk_and_empty_input() {
        assert_eq!(extract_keywords("好"), vec![String::from("好")]);
        assert_eq!(
            extract_keywords("好呀就"),
            vec![String::from("好呀"), String::from("呀就")]
        );
        assert!(extract_keywords("").is_empty());
        assert!(extract_keywords("a b c").is_empty(), "单字符 ASCII 词不算");
    }

    /// §15.4 S4.6：关键词侧与 `chunk_text_norm` 用同一个归一化函数，英文大小写
    /// 必须能互相匹配（A 的 bug 是 chunk_text 没小写化）。
    #[test]
    fn normalize_matches_keywords_case_insensitively() {
        let norm = normalize_text("Rust 知识库");
        assert!(norm.contains("rust"));
        assert!(extract_keywords("RUST").iter().all(|k| norm.contains(k)));
    }

    #[test]
    fn like_wildcards_are_escaped() {
        assert_eq!(escape_like("100%_a\\b"), "100\\%\\_a\\\\b");
        // 关键词召回是子串匹配，不是整段相等（§16.3）。
        assert_eq!(like_pattern("rust"), "%rust%");
        assert_eq!(like_pattern("50%"), "%50\\%%");
    }

    /// RRF 的硬性质：两个榜单都命中的分块排在最前；只取前 4 条。
    #[test]
    fn rrf_favours_chunks_found_by_both_channels() {
        let vec_hits = vec![
            (String::from("a"), String::from("A")),
            (String::from("b"), String::from("B")),
            (String::from("c"), String::from("C")),
        ];
        let kw_hits = vec![
            (String::from("c"), String::from("C")),
            (String::from("d"), String::from("D")),
        ];
        let fused = rrf_merge(&vec_hits, &kw_hits);
        assert_eq!(
            fused.first().map(String::as_str),
            Some("C"),
            "两边都命中的 C 应该第一"
        );
        assert_eq!(fused.len(), 4);
        assert!(fused.contains(&String::from("A")));
        assert!(fused.contains(&String::from("D")));
    }

    #[test]
    fn rrf_with_only_one_channel_keeps_its_order() {
        let vec_hits = vec![
            (String::from("a"), String::from("A")),
            (String::from("b"), String::from("B")),
        ];
        assert_eq!(rrf_merge(&vec_hits, &[]), vec!["A", "B"]);
    }

    #[test]
    fn sha256_matches_known_vector() {
        // NIST 测试向量：空串 / "abc"。
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn upload_path_only_keeps_a_sanitised_extension() {
        let p = upload_rel_path("r1", "d1", "../../etc/passwd");
        assert_eq!(p, "uploads/r1/d1", "没有扩展名就不拼扩展名");
        let p = upload_rel_path("r1", "d1", "报告.DOCX");
        assert_eq!(p, "uploads/r1/d1.docx");
        let p = upload_rel_path("r1", "d1", "a.tar.gz");
        assert_eq!(p, "uploads/r1/d1.gz");
        let p = upload_rel_path("r1", "d1", "x.a/b");
        assert!(!p.contains(".."), "不能把用户文件名拼进路径");
    }

    #[test]
    fn chunking_splits_long_paragraphs_with_overlap() {
        let text = "a".repeat(120);
        let chunks = chunk_text(&text, 50, 10);
        assert!(chunks.len() >= 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= 50));
    }
}
