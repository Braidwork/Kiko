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

/// 意图的最小身份：加短语前要拿 `name` 去算向量，没必要把整条详情读出来。
#[derive(sqlx::FromRow)]
pub(crate) struct IntentMeta {
    pub(crate) name: String,
}

/// 一条短语的载荷 + 它在 `phrase.dat` 里的向量行号。
///
/// `vec_row_id` 是存储层的过渡引用（见 `0002_intent_phrase_vec_row_id.sql`），
/// 不进 API。
#[derive(sqlx::FromRow)]
pub(crate) struct PhraseVector {
    pub(crate) vec_row_id: Option<i64>,
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

/// 删除意图。子表由 FK 级联。返回是否真的删掉了一行。
pub(crate) async fn delete(pool: &PgPool, robot_id: &str, intent_id: &str) -> Result<bool> {
    let r = sqlx::query("DELETE FROM intents WHERE id = $1 AND robot_id = $2")
        .bind(intent_id)
        .bind(robot_id)
        .execute(pool)
        .await?;
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

/// 加短语之前的身份查询（算向量要用意图名）。
pub(crate) async fn get_meta(
    pool: &PgPool,
    robot_id: &str,
    intent_id: &str,
) -> Result<Option<IntentMeta>> {
    Ok(
        sqlx::query_as("SELECT name FROM intents WHERE id = $1 AND robot_id = $2")
            .bind(intent_id)
            .bind(robot_id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 记录一条短语。向量已经由 `intent/phrase.rs` 写入 turso，这里只落载荷 +
/// 过渡用的 `vec_row_id`。意图不存在时返回 `None`，调用方负责清掉刚写的向量行。
pub(crate) async fn add_phrase(
    pool: &PgPool,
    robot_id: &str,
    intent_id: &str,
    phrase: &str,
    vec_row_id: i64,
) -> Result<Option<String>> {
    let mut tx = pool.begin().await?;
    if !lock_intent(&mut tx, robot_id, intent_id).await? {
        return Ok(None);
    }
    let id = scru128::new_string();
    sqlx::query(
        "INSERT INTO intent_phrases (id, intent_id, position, phrase, vec_row_id)
         VALUES ($1, $2,
                 (SELECT COALESCE(MAX(position), -1) + 1
                    FROM intent_phrases WHERE intent_id = $2),
                 $3, $4)",
    )
    .bind(&id)
    .bind(intent_id)
    .bind(phrase)
    .bind(vec_row_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(id))
}

/// 按 id 取一条短语（含向量行号）。
pub(crate) async fn get_phrase(
    pool: &PgPool,
    robot_id: &str,
    intent_id: &str,
    phrase_id: &str,
) -> Result<Option<PhraseVector>> {
    Ok(sqlx::query_as(
        r#"SELECT p.vec_row_id, p.phrase
             FROM intent_phrases p
             JOIN intents i ON i.id = p.intent_id
            WHERE p.id = $1 AND p.intent_id = $2 AND i.robot_id = $3"#,
    )
    .bind(phrase_id)
    .bind(intent_id)
    .bind(robot_id)
    .fetch_optional(pool)
    .await?)
}

/// 按 id 删除短语行。向量行由调用方先删（turso 与 PG 不能同事务，见 §15.3）。
pub(crate) async fn delete_phrase(
    pool: &PgPool,
    robot_id: &str,
    intent_id: &str,
    phrase_id: &str,
) -> Result<bool> {
    let r = sqlx::query(
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
    .execute(pool)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// 一个意图的全部短语 + 向量行号，按 `position`。重建/重算向量用。
pub(crate) async fn list_phrase_vectors(
    pool: &PgPool,
    robot_id: &str,
    intent_id: &str,
) -> Result<Vec<PhraseVector>> {
    Ok(sqlx::query_as(
        r#"SELECT p.vec_row_id, p.phrase
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

/// 重建向量后把新的 turso 行号写回 PG。
///
/// 整批一个事务：一次重建要么全部换成新行号，要么保持旧行号（下一轮重建会
/// 重新清表，残留旧行号不会造成不可恢复的错误）。
pub(crate) async fn update_phrase_vec_row_ids(
    pool: &PgPool,
    intent_id: &str,
    updates: &[(String, i64)],
) -> Result<()> {
    if updates.is_empty() {
        return Ok(());
    }
    let mut tx = pool.begin().await?;
    for (phrase_id, vec_row_id) in updates {
        sqlx::query("UPDATE intent_phrases SET vec_row_id = $1 WHERE id = $2 AND intent_id = $3")
            .bind(vec_row_id)
            .bind(phrase_id)
            .bind(intent_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}
