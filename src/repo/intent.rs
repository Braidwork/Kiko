//! `intents` / `intent_keywords` / `intent_regexes` / `intent_phrases` 仓储
//! （§5.4 / §15.4 S3.3 / B-14）。
//!
//! A 把整个 `IntentDetail` 序列化成一个 JSON blob 塞进 redb，键是意图 id；关键词、
//! 正则、短语都是 JSON 数组，删除靠**数组下标**。搬进 PG 后拆成四张表：
//!
//! - 顺序用 `position` 显式化（§5.1）：新增取 `MAX(position) + 1`，删除留空洞，
//!   查询一律 `ORDER BY position`；
//! - 子项有了自己的 `id`（scru128），增删都按 id —— 这是 D10，也是 §16.4
//!   「读整条 → 改 → 写回」丢更新的根治办法：单条 SQL 直接插入/删除，不再
//!   把整条意图读出来再写回去；
//! - 同一个意图上的并发写用父行 `SELECT … FOR UPDATE` 串行化，保证并发追加时
//!   不会算出两个相同的 `position`（仅按 id 删除本身不需要锁）。
//!
//! 跨机器人隔离：A 的 redb 表名带 `robot_id`，天然隔离；PG 单表后每个读写都必须
//! 显式带 `robot_id`，否则拿别人的意图 id 就能改到别人的数据。

use std::collections::HashMap;

use sqlx::postgres::PgPool;

use crate::intent::dto::{IntentDetail, IntentKeywordData, IntentPhraseData, IntentRegexData};
use crate::repo::vector::{self, ModelRef};
use crate::result::Result;

#[derive(sqlx::FromRow)]
struct IntentRow {
    id: String,
    name: String,
}

#[derive(sqlx::FromRow)]
struct KeywordRow {
    intent_id: String,
    id: String,
    keyword: String,
}

#[derive(sqlx::FromRow)]
struct RegexRow {
    intent_id: String,
    id: String,
    regex: String,
}

#[derive(sqlx::FromRow)]
struct PhraseRow {
    intent_id: String,
    id: String,
    phrase: String,
}

/// 一条短语的载荷（重建/重算向量用）。向量身份是 `vector_items.id`（D7），
/// 不再是过渡列 `vec_row_id`（B-30）。
#[derive(Debug, Clone, sqlx::FromRow)]
pub(crate) struct PhrasePayload {
    pub(crate) id: String,
    pub(crate) phrase: String,
}

const KEYWORD_COLUMNS: &str = "k.intent_id, k.id, k.keyword";
const REGEX_COLUMNS: &str = "r.intent_id, r.id, r.regex";
const PHRASE_COLUMNS: &str = "p.intent_id, p.id, p.phrase";

/// 一个机器人的全部意图（含子项），按 `position` 排序。
///
/// 检测器（`intent/detector.rs`）与重建（`man/reindex.rs`）都走这里，一次取全，
/// 避免每个意图再查一次子表。
pub(crate) async fn list(pool: &PgPool, robot_id: &str) -> Result<Vec<IntentDetail>> {
    let intents: Vec<IntentRow> = sqlx::query_as(
        r#"SELECT id, name FROM intents
            WHERE robot_id = $1
            ORDER BY position, id COLLATE "C""#,
    )
    .bind(robot_id)
    .fetch_all(pool)
    .await?;
    if intents.is_empty() {
        return Ok(Vec::new());
    }

    let keyword_rows: Vec<KeywordRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        r#"SELECT {KEYWORD_COLUMNS}
             FROM intent_keywords k
             JOIN intents i ON i.id = k.intent_id
            WHERE i.robot_id = $1
            ORDER BY k.intent_id COLLATE "C", k.position, k.id COLLATE "C""#
    )))
    .bind(robot_id)
    .fetch_all(pool)
    .await?;

    let regex_rows: Vec<RegexRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        r#"SELECT {REGEX_COLUMNS}
             FROM intent_regexes r
             JOIN intents i ON i.id = r.intent_id
            WHERE i.robot_id = $1
            ORDER BY r.intent_id COLLATE "C", r.position, r.id COLLATE "C""#
    )))
    .bind(robot_id)
    .fetch_all(pool)
    .await?;

    let phrase_rows: Vec<PhraseRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        r#"SELECT {PHRASE_COLUMNS}
             FROM intent_phrases p
             JOIN intents i ON i.id = p.intent_id
            WHERE i.robot_id = $1
            ORDER BY p.intent_id COLLATE "C", p.position, p.id COLLATE "C""#
    )))
    .bind(robot_id)
    .fetch_all(pool)
    .await?;

    let mut keywords: HashMap<String, Vec<IntentKeywordData>> = HashMap::new();
    for r in keyword_rows {
        keywords
            .entry(r.intent_id)
            .or_default()
            .push(IntentKeywordData {
                id: r.id,
                keyword: r.keyword,
            });
    }
    let mut regexes: HashMap<String, Vec<IntentRegexData>> = HashMap::new();
    for r in regex_rows {
        regexes
            .entry(r.intent_id)
            .or_default()
            .push(IntentRegexData {
                id: r.id,
                regex: r.regex,
            });
    }
    let mut phrases: HashMap<String, Vec<IntentPhraseData>> = HashMap::new();
    for r in phrase_rows {
        phrases
            .entry(r.intent_id)
            .or_default()
            .push(IntentPhraseData {
                id: r.id,
                phrase: r.phrase,
            });
    }

    Ok(intents
        .into_iter()
        .map(|i| IntentDetail {
            keywords: keywords.remove(&i.id).unwrap_or_default(),
            regexes: regexes.remove(&i.id).unwrap_or_default(),
            phrases: phrases.remove(&i.id).unwrap_or_default(),
            intent_id: i.id,
            intent_name: i.name,
        })
        .collect())
}

/// 单条意图。`robot_id` 必须参与过滤 —— PG 单表后少了它就跨机器人泄漏。
pub(crate) async fn get(
    pool: &PgPool,
    robot_id: &str,
    intent_id: &str,
) -> Result<Option<IntentDetail>> {
    let intent: Option<IntentRow> =
        sqlx::query_as("SELECT id, name FROM intents WHERE id = $1 AND robot_id = $2")
            .bind(intent_id)
            .bind(robot_id)
            .fetch_optional(pool)
            .await?;
    let Some(intent) = intent else {
        return Ok(None);
    };

    let keywords: Vec<KeywordRow> = sqlx::query_as(
        r#"SELECT intent_id, id, keyword FROM intent_keywords
            WHERE intent_id = $1
            ORDER BY position, id COLLATE "C""#,
    )
    .bind(intent_id)
    .fetch_all(pool)
    .await?;
    let regexes: Vec<RegexRow> = sqlx::query_as(
        r#"SELECT intent_id, id, regex FROM intent_regexes
            WHERE intent_id = $1
            ORDER BY position, id COLLATE "C""#,
    )
    .bind(intent_id)
    .fetch_all(pool)
    .await?;
    let phrases: Vec<PhraseRow> = sqlx::query_as(
        r#"SELECT intent_id, id, phrase FROM intent_phrases
            WHERE intent_id = $1
            ORDER BY position, id COLLATE "C""#,
    )
    .bind(intent_id)
    .fetch_all(pool)
    .await?;

    Ok(Some(IntentDetail {
        intent_id: intent.id,
        intent_name: intent.name,
        keywords: keywords
            .into_iter()
            .map(|r| IntentKeywordData {
                id: r.id,
                keyword: r.keyword,
            })
            .collect(),
        regexes: regexes
            .into_iter()
            .map(|r| IntentRegexData {
                id: r.id,
                regex: r.regex,
            })
            .collect(),
        phrases: phrases
            .into_iter()
            .map(|r| IntentPhraseData {
                id: r.id,
                phrase: r.phrase,
            })
            .collect(),
    }))
}

/// 新建意图（可带初始关键词）。返回新意图的 id。
///
/// 先锁住机器人行再算 `MAX(position)`：同一机器人上并发建意图时，两个事务
/// 不会拿到同一个位置。
pub(crate) async fn create(
    pool: &PgPool,
    robot_id: &str,
    name: &str,
    keywords: &[String],
) -> Result<String> {
    let mut tx = pool.begin().await?;

    // 机器人不存在时这里返回 None，紧接着的 INSERT 会以外键冲突失败 ——
    // 错误文案与其它仓储一致，由 `Error::message` 统一翻译。
    let _ = sqlx::query_scalar::<_, String>("SELECT id FROM robots WHERE id = $1 FOR UPDATE")
        .bind(robot_id)
        .fetch_optional(&mut *tx)
        .await?;

    let id = scru128::new_string();
    sqlx::query(
        "INSERT INTO intents (id, robot_id, name, position)
         VALUES ($1, $2, $3,
                 (SELECT COALESCE(MAX(position), -1) + 1 FROM intents WHERE robot_id = $2))",
    )
    .bind(&id)
    .bind(robot_id)
    .bind(name)
    .execute(&mut *tx)
    .await?;

    for (position, keyword) in keywords.iter().enumerate() {
        sqlx::query(
            "INSERT INTO intent_keywords (id, intent_id, position, keyword)
             VALUES ($1, $2, $3, $4)",
        )
        .bind(scru128::new_string())
        .bind(&id)
        .bind(position as i32)
        .bind(keyword)
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;
    Ok(id)
}

/// 删除意图。子表由 FK 级联，但**短语的向量行不会级联**（`vector_items.entity_id`
/// 是通用列，没有外键），所以先按 `intent_phrases.id` 显式删掉它们，再删意图 ——
/// 同一个事务（B-22）。返回是否真的删掉了一行。
pub(crate) async fn delete(pool: &PgPool, robot_id: &str, intent_id: &str) -> Result<bool> {
    let mut tx = pool.begin().await?;
    let phrase_ids: Vec<String> = sqlx::query_scalar(
        "SELECT p.id FROM intent_phrases p
           JOIN intents i ON i.id = p.intent_id
          WHERE p.intent_id = $1 AND i.robot_id = $2",
    )
    .bind(intent_id)
    .bind(robot_id)
    .fetch_all(&mut *tx)
    .await?;
    vector::delete_entities(&mut tx, robot_id, vector::PHRASE, &phrase_ids).await?;
    let r = sqlx::query("DELETE FROM intents WHERE id = $1 AND robot_id = $2")
        .bind(intent_id)
        .bind(robot_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(r.rows_affected() > 0)
}

/// 意图是否存在且属于该机器人（并锁住这一行，供后续追加子项用）。
async fn lock_intent(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    robot_id: &str,
    intent_id: &str,
) -> Result<bool> {
    let found: Option<String> =
        sqlx::query_scalar("SELECT id FROM intents WHERE id = $1 AND robot_id = $2 FOR UPDATE")
            .bind(intent_id)
            .bind(robot_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(found.is_some())
}

/// 追加关键词。意图不存在（或不属于该机器人）时返回 `None` ——
/// 与 A 的「意图不在就什么也不做」一致。
pub(crate) async fn add_keyword(
    pool: &PgPool,
    robot_id: &str,
    intent_id: &str,
    keyword: &str,
) -> Result<Option<String>> {
    let mut tx = pool.begin().await?;
    if !lock_intent(&mut tx, robot_id, intent_id).await? {
        return Ok(None);
    }
    let id = scru128::new_string();
    sqlx::query(
        "INSERT INTO intent_keywords (id, intent_id, position, keyword)
         VALUES ($1, $2,
                 (SELECT COALESCE(MAX(position), -1) + 1
                    FROM intent_keywords WHERE intent_id = $2),
                 $3)",
    )
    .bind(&id)
    .bind(intent_id)
    .bind(keyword)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(id))
}

/// 按 id 删除关键词。返回是否删掉了行。
pub(crate) async fn remove_keyword(
    pool: &PgPool,
    robot_id: &str,
    intent_id: &str,
    keyword_id: &str,
) -> Result<bool> {
    let r = sqlx::query(
        r#"DELETE FROM intent_keywords k
             USING intents i
            WHERE k.id = $1
              AND k.intent_id = $2
              AND i.id = k.intent_id
              AND i.robot_id = $3"#,
    )
    .bind(keyword_id)
    .bind(intent_id)
    .bind(robot_id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// 追加正则。调用方负责先校验正则语法（业务规则留在 `intent/crud.rs`）。
pub(crate) async fn add_regex(
    pool: &PgPool,
    robot_id: &str,
    intent_id: &str,
    regex: &str,
) -> Result<Option<String>> {
    let mut tx = pool.begin().await?;
    if !lock_intent(&mut tx, robot_id, intent_id).await? {
        return Ok(None);
    }
    let id = scru128::new_string();
    sqlx::query(
        "INSERT INTO intent_regexes (id, intent_id, position, regex)
         VALUES ($1, $2,
                 (SELECT COALESCE(MAX(position), -1) + 1
                    FROM intent_regexes WHERE intent_id = $2),
                 $3)",
    )
    .bind(&id)
    .bind(intent_id)
    .bind(regex)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(id))
}

pub(crate) async fn remove_regex(
    pool: &PgPool,
    robot_id: &str,
    intent_id: &str,
    regex_id: &str,
) -> Result<bool> {
    let r = sqlx::query(
        r#"DELETE FROM intent_regexes r
             USING intents i
            WHERE r.id = $1
              AND r.intent_id = $2
              AND i.id = r.intent_id
              AND i.robot_id = $3"#,
    )
    .bind(regex_id)
    .bind(intent_id)
    .bind(robot_id)
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// 记录一条短语：载荷与向量**同一个事务**（B-22）。返回 `(短语 id, vector_items.id)`。
///
/// 意图不存在时返回 `None`；`vec` 由调用方在事务外算好（模型推理不占事务）。
pub(crate) async fn add_phrase(
    pool: &PgPool,
    robot_id: &str,
    intent_id: &str,
    phrase: &str,
    vec: Vec<f32>,
    model: &ModelRef,
) -> Result<Option<(String, i64)>> {
    let mut tx = pool.begin().await?;
    if !lock_intent(&mut tx, robot_id, intent_id).await? {
        return Ok(None);
    }
    let id = scru128::new_string();
    sqlx::query(
        "INSERT INTO intent_phrases (id, intent_id, position, phrase)
         VALUES ($1, $2,
                 (SELECT COALESCE(MAX(position), -1) + 1
                    FROM intent_phrases WHERE intent_id = $2),
                 $3)",
    )
    .bind(&id)
    .bind(intent_id)
    .bind(phrase)
    .execute(&mut *tx)
    .await?;
    let ids = vector::put_many_active(
        &mut tx,
        robot_id,
        vector::PHRASE,
        model,
        &[(id.clone(), vec)],
    )
    .await?;
    tx.commit().await?;
    Ok(ids.first().map(|v| (id, *v)))
}

/// 按 id 删除短语行，**连同它的向量行**（同一个事务）。返回被删的
/// `vector_items.id`，调用方据此同步内存索引。
pub(crate) async fn delete_phrase(
    pool: &PgPool,
    robot_id: &str,
    intent_id: &str,
    phrase_id: &str,
) -> Result<Vec<i64>> {
    let mut tx = pool.begin().await?;
    let ids = vector::delete_entities(
        &mut tx,
        robot_id,
        vector::PHRASE,
        &[String::from(phrase_id)],
    )
    .await?;
    sqlx::query(
        r#"DELETE FROM intent_phrases p
             USING intents i
            WHERE p.id = $1
              AND p.intent_id = $2
              AND i.id = p.intent_id
              AND i.robot_id = $3"#,
    )
    .bind(phrase_id)
    .bind(intent_id)
    .bind(robot_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(ids)
}

/// 一个意图的全部短语（载荷），按 `position`。重算向量用。
pub(crate) async fn list_phrases(
    pool: &PgPool,
    robot_id: &str,
    intent_id: &str,
) -> Result<Vec<PhrasePayload>> {
    Ok(sqlx::query_as(
        r#"SELECT p.id, p.phrase
             FROM intent_phrases p
             JOIN intents i ON i.id = p.intent_id
            WHERE p.intent_id = $1 AND i.robot_id = $2
            ORDER BY p.position, p.id COLLATE "C""#,
    )
    .bind(intent_id)
    .bind(robot_id)
    .fetch_all(pool)
    .await?)
}

/// 还没有 `generation` 向量的短语（重建续跑用，D47）。
pub(crate) async fn pending_phrases(
    pool: &PgPool,
    robot_id: &str,
    generation: i64,
) -> Result<Vec<PhrasePayload>> {
    Ok(sqlx::query_as(
        r#"SELECT p.id, p.phrase
             FROM intent_phrases p
             JOIN intents i ON i.id = p.intent_id
            WHERE i.robot_id = $1
              AND NOT EXISTS (
                  SELECT 1 FROM vector_items v
                   WHERE v.robot_id = i.robot_id
                     AND v.collection = 'phrase'
                     AND v.generation = $2
                     AND v.entity_id = p.id)
            ORDER BY i.position, i.id COLLATE "C", p.position, p.id COLLATE "C""#,
    )
    .bind(robot_id)
    .bind(generation)
    .fetch_all(pool)
    .await?)
}

/// 重建进度里的短语总数（与 [`pending_phrases`] 同口径）。
pub(crate) async fn count_phrases(pool: &PgPool, robot_id: &str) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT count(*) FROM intent_phrases p
           JOIN intents i ON i.id = p.intent_id
          WHERE i.robot_id = $1",
    )
    .bind(robot_id)
    .fetch_one(pool)
    .await?)
}

/// 重建时给一条**已存在**的短语写指定 generation 的向量（幂等 upsert）。
pub(crate) async fn put_phrase_vector(
    pool: &PgPool,
    robot_id: &str,
    phrase_id: &str,
    generation: i64,
    vec: Vec<f32>,
    model: &ModelRef,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    vector::put_many(
        &mut tx,
        robot_id,
        vector::PHRASE,
        generation,
        model,
        &[(String::from(phrase_id), vec)],
    )
    .await?;
    tx.commit().await?;
    Ok(())
}

/// 给一组**已存在**的短语重算 active generation 的向量（`regenerate-all`）。
///
/// 实体 id 不变，所以是覆盖而不是换行 —— 返回同序的 `vector_items.id` 供内存索引
/// 原地更新。
pub(crate) async fn put_phrase_vectors(
    pool: &PgPool,
    robot_id: &str,
    items: &[(String, Vec<f32>)],
    model: &ModelRef,
) -> Result<Vec<i64>> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    let mut tx = pool.begin().await?;
    let ids = vector::put_many_active(&mut tx, robot_id, vector::PHRASE, model, items).await?;
    tx.commit().await?;
    Ok(ids)
}

/// 一组短语向量命中的意图名（短语检索 top-k → 回载荷）。
pub(crate) async fn intent_names_for_phrase_vectors(
    pool: &PgPool,
    robot_id: &str,
    vector_ids: &[i64],
) -> Result<Vec<(i64, String)>> {
    if vector_ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT v.id, i.name
           FROM vector_items v
           JOIN intent_phrases p ON p.id = v.entity_id
           JOIN intents i ON i.id = p.intent_id
          WHERE v.robot_id = $1 AND v.collection = $2 AND v.id = ANY($3)
            AND i.robot_id = $1",
    )
    .bind(robot_id)
    .bind(vector::PHRASE)
    .bind(vector_ids)
    .fetch_all(pool)
    .await?)
}
