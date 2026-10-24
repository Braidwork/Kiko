use std::sync::OnceLock;
use std::vec::Vec;

// use futures_util::StreamExt;
// use sqlx::{Row, Sqlite};

use crate::ai::embedding::embedding;
use crate::man::settings;
use crate::repo;
use crate::result::{Error, Result};
use crate::retry_on_busy;

// type SqliteConnPool = sqlx::Pool<Sqlite>;

// static DATA_SOURCE: OnceCell<SqliteConnPool> = OnceCell::new();
static DATA_SOURCE: OnceLock<turso::Database> = OnceLock::new();
// static DATA_SOURCES: OnceLock<Mutex<HashMap<String, SqliteConnPool>>> = OnceLock::new();

/// 写锁争用的总等待时长。
///
/// turso 的忙等是基于 yield 的、不阻塞线程（见 turso_core 的 busy.rs），所以
/// 它既能把并发写串行化成"排队等待"，又不会占住 tokio worker。
const BUSY_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// 所有连接都必须走这里。
///
/// 连接是每次调用新建、从不复用的，`busy_timeout` 又是连接级设置，所以散在
/// 各调用点设置必然漏 —— 集中在这一个 helper 里。
fn conn() -> Result<turso::Connection> {
    let c = DATA_SOURCE.get().unwrap().connect()?;
    c.busy_timeout(BUSY_TIMEOUT)?;
    Ok(c)
}
// static INDEXES: LazyLock<Mutex<HashMap<String, usearch::Index>>> =
//     LazyLock::new(|| Mutex::new(HashMap::with_capacity(32)));

// fn get_sqlite_path() -> std::path::PathBuf {
//     let p = std::path::Path::new(".").join("data");
//     if !p.exists() {
//         std::fs::create_dir_all(&p).expect("Create data directory failed.");
//     }
//     p.join("ripd.dat")
// }

pub(crate) async fn init_datasource() -> Result<()> {
    let p = std::path::Path::new(".").join("data");
    if !p.exists() {
        std::fs::create_dir_all(&p).expect("Create data directory failed.");
    }
    let p = p.join("phrase.dat");
    // log::info!("Init phrase datasource, path = {:?}", p.as_path().to_str().unwrap());
    let turso = turso::Builder::new_local(p.as_path().to_str().unwrap())
        .build()
        .await?;
    DATA_SOURCE
        .set(turso)
        .map_err(|_| Error::WithMessage(String::from("Datasource has been set.")))
}

// pub(crate) async fn init_tables(robot_id: &str) -> Result<()> {
//     // println!("Init database");
//     // let ddl = include_str!("./embedding_ddl.sql");
//     let sql = format!(
//         "CREATE TABLE {robot_id} (
//             id INTEGER NOT NULL PRIMARY KEY,
//             -- id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
//             intent_id TEXT NOT NULL,
//             intent_name TEXT NOT NULL,
//             phrase_vec F32_BLOB(384)
//         );",
//     );
//     // log::info!("sql = {}", &sql);
//     let mut stream = sqlx::raw_sql(&sql).execute_many(DATA_SOURCE.get().unwrap());
//     while let Some(res) = stream.next().await {
//         match res {
//             Ok(_r) => log::info!("Initialized phrase table"),
//             Err(e) => log::error!("Create table failed, err: {e:?}"),
//         }
//     }
//     // let dml = include_str!("../resource/sql/dml.sql");
//     // if let Err(e) = sqlx::query(dml).execute(&pool).await {
//     //     panic!("{:?}", e);
//     // }
//     Ok(())
// }

pub(crate) async fn search(robot_id: &str, vectors: &Vec<f32>) -> Result<Vec<(String, f64)>> {
    let conn = conn()?;
    let sql = format!(
        "SELECT intent_id, intent_name, vector_distance_cos(phrase_vec, vector32(?1)) AS distance FROM {robot_id} ORDER BY distance ASC LIMIT 1",
    );
    let mut results = conn.query(sql, [serde_json::to_string(vectors)?]).await?;
    let mut names = Vec::with_capacity(1);
    while let Some(r) = results.next().await? {
        names.push((
            r.get_value(1)?.as_text().unwrap().to_string(),
            *r.get_value(2)?.as_real().unwrap(),
        ));
    }
    Ok(names)
}

// fn update_idx(robot_id: &str, key: u64, vec: &[f32]) -> Result<()> {
//     let mut idxes = INDEXES.lock()?;
//     let p = std::path::Path::new("ipvd.vec");
//     let s = p.display().to_string();
//     if !idxes.contains_key(robot_id) {
//         let options = usearch::IndexOptions {
//             dimensions: vec.len(),
//             metric: usearch::MetricKind::Cos,
//             quantization: usearch::ScalarKind::F32,
//             connectivity: 0,                        // zero for auto
//             expansion_add: 0,                       // zero for auto
//             expansion_search: 0,                    // zero for auto
//             multi: false,
//         };
//         let index: usearch::Index = usearch::new_index(&options).unwrap();
//         if p.exists() {
//             index.load(&s)?;
//         }
//         idxes.insert(String::from(robot_id), index);
//     }
//     let idx = idxes.get(robot_id).unwrap();
//     log::info!("idx memory_usage: {}", idx.memory_usage());
//     idx.add(key, vec)?;
//     idx.save(&s)?;
//     Ok(())
// }

/// 插入一条短语向量，返回 turso 行号。调用方负责把它记进 PG（`vec_row_id`）。
pub(crate) async fn add(
    robot_id: &str,
    intent_id: &str,
    intent_name: &str,
    phrase: &str,
) -> Result<i64> {
    let ids = insert_vectors(robot_id, intent_id, intent_name, &[String::from(phrase)]).await?;
    let id = ids[0];
    // 短语向量已经落库，记下它是用哪个模型/维度算的（见
    // settings::stamp_embedding_index）。打标失败不影响这条短语。
    if let Err(e) = settings::stamp_embedding_index(robot_id).await {
        log::warn!("Stamping embedding index of {robot_id} failed: {e:?}");
    }
    Ok(id)
}

/// 批量插入向量行，返回与入参**同序**的行号。不打标（重建时统一打一次）。
///
/// 推理全部在事务外：N 条短语只抢一次写锁。`CREATE TABLE` 和 INSERT 收在同一个
/// 事务里，避免「表建好了但没数据」的中间态。
async fn insert_vectors(
    robot_id: &str,
    intent_id: &str,
    intent_name: &str,
    phrases: &[String],
) -> Result<Vec<i64>> {
    if phrases.is_empty() {
        return Ok(Vec::new());
    }
    if intent_name.is_empty() {
        let err = format!("intent_id {intent_id} intent_name is empty");
        log::warn!("{err}");
        return Err(Error::WithMessage(err));
    }

    let mut vec_len = 0usize;
    let mut vecs: Vec<String> = Vec::with_capacity(phrases.len());
    for p in phrases.iter() {
        let vectors = embedding(robot_id, p).await?;
        if vectors.0.is_empty() {
            let err = format!("{p} embedding data is empty");
            log::warn!("{err}");
            return Err(Error::WithMessage(err));
        }
        if vec_len == 0 {
            vec_len = vectors.0.len();
        }
        vecs.push(serde_json::to_string(&vectors.0)?);
    }

    retry_on_busy!(async {
        let mut conn = conn()?;
        let tx = conn.transaction().await?;
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {robot_id} (
                id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                intent_id TEXT NOT NULL,
                intent_name TEXT NOT NULL,
                phrase TEXT NOT NULL,
                phrase_vec F32_BLOB({vec_len}) NOT NULL
            )"
        );
        tx.execute(sql.as_str(), ()).await?;
        let sql = format!(
            "INSERT INTO {robot_id} (intent_id, intent_name, phrase, phrase_vec)VALUES(?1, ?2, ?3, vector32(?4))",
        );
        let mut stmt = tx.prepare(&sql).await?;
        let mut ids = Vec::with_capacity(phrases.len());
        for (p, vec_json) in phrases.iter().zip(vecs.iter()) {
            stmt.execute((
                String::from(intent_id),
                String::from(intent_name),
                p.clone(),
                vec_json.clone(),
            ))
            .await?;
            ids.push(tx.last_insert_rowid());
        }
        tx.commit().await?;
        Ok(ids)
    })
}

/// 给一组**已存在**的短语重算向量（`regenerate-all`）。
///
/// 推理全部提到事务外；所有 UPDATE 合并进一个事务，按 `vec_row_id` 原地更新。
pub(crate) async fn update_vectors(
    robot_id: &str,
    intent_id: &str,
    intent_name: &str,
    phrases: &[(i64, String)],
) -> Result<()> {
    if phrases.is_empty() {
        return Ok(());
    }
    if intent_name.is_empty() {
        let err = format!("intent_id {intent_id} intent_name is empty");
        log::warn!("{err}");
        return Err(Error::WithMessage(err));
    }
    let mut vec_len = 0usize;
    let mut vecs: Vec<String> = Vec::with_capacity(phrases.len());
    for (_, phrase) in phrases.iter() {
        let vectors = embedding(robot_id, phrase).await?;
        if vectors.0.is_empty() {
            let err = format!("{phrase} embedding data is empty");
            log::warn!("{err}");
            return Err(Error::WithMessage(err));
        }
        if vec_len == 0 {
            vec_len = vectors.0.len();
        }
        vecs.push(serde_json::to_string(&vectors.0)?);
    }
    retry_on_busy!(async {
        let mut conn = conn()?;
        let tx = conn.transaction().await?;
        let sql = format!(
            "CREATE TABLE IF NOT EXISTS {robot_id} (
                id INTEGER NOT NULL PRIMARY KEY AUTOINCREMENT,
                intent_id TEXT NOT NULL,
                intent_name TEXT NOT NULL,
                phrase TEXT NOT NULL,
                phrase_vec F32_BLOB({vec_len}) NOT NULL
            )"
        );
        tx.execute(sql.as_str(), ()).await?;
        let sql = format!(
            "UPDATE {robot_id} SET intent_id = ?1, intent_name = ?2, phrase = ?3, phrase_vec = vector32(?4) WHERE id = ?5",
        );
        let mut stmt = tx.prepare(&sql).await?;
        for ((vec_row_id, phrase), vec_json) in phrases.iter().zip(vecs.iter()) {
            stmt.execute((
                String::from(intent_id),
                String::from(intent_name),
                phrase.clone(),
                vec_json.clone(),
                *vec_row_id,
            ))
            .await?;
        }
        tx.commit().await?;
        Ok(())
    })?;
    if let Err(e) = settings::stamp_embedding_index(robot_id).await {
        log::warn!("Stamping embedding index of {robot_id} failed: {e:?}");
    }
    Ok(())
}

/// 重建意图短语向量：**整表已经先被调用方清掉**（见 `man/reindex.rs`），
/// 因此这里逐条插入新行，并把新行号写回 PG 的 `intent_phrases.vec_row_id`。
///
/// A 的实现是清表后按旧行号 `UPDATE`，一行也更新不到 —— 重建会静默产出一张空表
/// （§18）。载荷在 PG，行号可重建，所以新行号必须回写。
pub(crate) async fn reindex(
    pool: &sqlx::postgres::PgPool,
    robot_id: &str,
    intents: &[crate::intent::dto::IntentDetail],
) -> Result<usize> {
    let mut n = 0usize;
    for d in intents {
        if d.phrases.is_empty() {
            continue;
        }
        let texts: Vec<String> = d.phrases.iter().map(|p| p.phrase.clone()).collect();
        let vec_ids = insert_vectors(robot_id, &d.intent_id, &d.intent_name, &texts).await?;
        let updates: Vec<(String, i64)> = d
            .phrases
            .iter()
            .map(|p| p.id.clone())
            .zip(vec_ids)
            .collect();
        repo::intent::update_phrase_vec_row_ids(pool, &d.intent_id, &updates).await?;
        n += texts.len();
    }
    Ok(n)
}

pub(crate) async fn remove(robot_id: &str, id: i64) -> Result<()> {
    // INDEXES.lock()?.get(robot_id).and_then(|idx| {idx.remove(id as u64); None::<()>});
    let sql = format!("DELETE FROM {robot_id} WHERE id = ?1");
    // sqlx::query::<Sqlite>(&sql)
    //     .bind(id)
    //     .execute(DATA_SOURCE.get().unwrap())
    //     .await?;
    retry_on_busy!(async {
        let conn = conn()?;
        conn.execute(sql.as_str(), [id]).await?;
        Ok(())
    })
}

/// 短语表是懒创建的：没加过短语的机器人没有这张表。
///
/// 比较时用 `lower()`：turso 在 `sqlite_schema` 里存的是小写表名（标识符本身
/// 大小写不敏感），而机器人 id 是大小写混合的 scru128。
async fn table_exists(conn: &turso::Connection, robot_id: &str) -> Result<bool> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM sqlite_schema WHERE type = 'table' AND lower(name) = lower(?1)",
            [String::from(robot_id)],
        )
        .await?;
    Ok(rows.next().await?.is_some())
}

pub(crate) async fn remove_by_intent_id(robot_id: &str, intent_id: &str) -> Result<()> {
    let sql = format!("DELETE FROM {robot_id} WHERE intent_id = ?1");
    // 表只在第一次 `add` 时创建。删意图时机器人可能一条短语都没加过，
    // 直接 DELETE 会报 "no such table" 把整个删除请求打回去（A 的老毛病）。
    retry_on_busy!(async {
        let conn = conn()?;
        if !table_exists(&conn, robot_id).await? {
            return Ok(());
        }
        conn.execute(sql.as_str(), [String::from(intent_id)])
            .await?;
        Ok(())
    })
}

pub(crate) async fn remove_tables(robot_id: &str) -> Result<()> {
    // 表只在第一次 `add` 时创建，没加过短语的机器人根本没有这张表 ——
    // 缺 IF EXISTS 会让 `robot::purge` 在第一步就整个中断，什么也删不掉。
    let sql = format!("DROP TABLE IF EXISTS {robot_id}");
    // match sqlx::query::<Sqlite>(&sql)
    //     .execute(DATA_SOURCE.get().unwrap())
    //     .await
    // {
    //     Ok(_) => return Ok(()),
    //     Err(e) => match e {
    //         sqlx::Error::Database(database_error) => {
    //             if let Some(code) = database_error.code() {
    //                 if code.eq("1") {
    //                     return Ok::<_, Error>(());
    //                 }
    //             }
    //         }
    //         _ => return Err(e.into()),
    //     },
    // };
    retry_on_busy!(async {
        let conn = conn()?;
        conn.execute(sql.as_str(), ()).await?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 短语表是懒创建的：`remove_by_intent_id` 必须先判断表在不在，
    /// 否则「删一个还没加过短语的意图」会被 turso 的 "no such table" 打回。
    #[tokio::test]
    async fn lazily_created_phrase_table_is_detected() {
        let db = turso::Builder::new_local(":memory:").build().await.unwrap();
        let conn = db.connect().unwrap();

        assert!(!table_exists(&conn, "rNotYet").await.unwrap());
        conn.execute(
            "CREATE TABLE rNotYet (id INTEGER PRIMARY KEY AUTOINCREMENT, intent_id TEXT NOT NULL)",
            (),
        )
        .await
        .unwrap();
        assert!(table_exists(&conn, "rNotYet").await.unwrap());
        // turso 在 sqlite_schema 里把表名存成小写；大小写混用的机器人 id 也要命中。
        assert!(table_exists(&conn, "rnotyet").await.unwrap());
    }
}
