// pub(crate) mod embedding;
// pub(crate) mod embedding_sqlite;

pub(crate) mod store;

use std::borrow::Borrow;
use std::vec::Vec;

use redb::{ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition, TableHandle};

use crate::flow::mainflow::crud as mainflow;
use crate::man::settings::{self, GlobalSettings};
use crate::result::Result;
use crate::robot::crud as robot;
use crate::web::server;

pub(crate) use store::robot_key;
pub(crate) use store::store;
pub(crate) use store::{RedbOp, RedbStore, StoreKey};

/// 建表 / 写 / 删 的统一入口。
///
/// 表名仍然是 `{robot_id}{suffix}`，但数据归属由 [`StoreKey`] 决定 —— 阶段 1 里
/// 四个 key 指向同一个文件，第 4 项分文件后表名里的前缀就冗余了（见 doc/storage.md）。
///
/// 宏体里解析 store 并 `.await`，所以**调用点必须位于 `async fn` 里**：这是"全部
/// async 化"的代价，换来的是第 4 项懒打开 store 时不用再改一遍调用点。
#[macro_export]
macro_rules! db_executor (
    ($func: expr, $robot_id: expr, $suffix: expr, $($bind: expr),*) => ({
        let table_name = format!("{}{}", $robot_id, $suffix);
        let table: redb::TableDefinition<&str, &[u8]> = redb::TableDefinition::new(&table_name);
        let store = $crate::db::store($crate::db::robot_key(&$robot_id)).await?;
        $func(store, table $(,($bind))*).await
    });
);

pub(crate) async fn init() -> Result<GlobalSettings> {
    let is_en = *server::IS_EN;

    // 第一次触碰存储：写线程在这里启动，"打不开库"这类错误也就此在启动期暴露，
    // 而不是拖到第一个请求。
    let global = store(StoreKey::Global).await?;

    // Settings
    settings::init_table(global).await?;
    mainflow::init_default_names(is_en)?;
    if settings::exists(global).await? {
        return Ok(settings::get_global_settings(global).await?.unwrap());
    }
    let settings = settings::init_global(global).await?;
    robot::init(is_en).await?;
    Ok(settings)
}

pub(crate) async fn init_table<K, V>(store: &RedbStore, table: TableDefinition<'_, K, V>) -> Result<()>
where
    K: redb::Key,
    for<'b> V: redb::Value<SelfType<'b> = &'b [u8]>,
{
    store
        .submit(vec![RedbOp::InitTable {
            table: String::from(table.name()),
        }])
        .await
}

/// 全局 store 的快捷方式 —— 只有机器人注册表和 global-settings 用它。
///
/// 其余模块一律通过 [`db_executor!`](crate::db_executor) 按 `robot_id` 取 store。
pub(crate) async fn global_store() -> Result<&'static RedbStore> {
    store(StoreKey::Global).await
}

// https://users.rust-lang.org/t/requesting-help-with-a-lifetime-problem-using-redb/98553
// https://doc.rust-lang.org/nomicon/hrtb.html
// https://users.rust-lang.org/t/implementation-is-not-general-enough/57433/4
pub(crate) async fn query<'a, K, V, KEY, D>(
    store: &RedbStore,
    table: TableDefinition<'_, K, V>,
    key: KEY,
) -> Result<Option<D>>
where
    K: redb::Key,
    for<'b> V: redb::Value<SelfType<'b> = &'b [u8]>,
    D: serde::de::DeserializeOwned,
    KEY: Borrow<&'a str> + std::borrow::Borrow<<K as redb::Value>::SelfType<'a>>,
{
    let read = store.db().begin_read()?;
    let table = read.open_table(table)?;
    let r = table.get(key)?;
    if let Some(d) = r {
        let s: D = serde_json::from_slice(d.value())?;
        Ok(Some(s))
    } else {
        Ok(None)
    }
}

pub(crate) async fn get_all<K, V, D>(
    store: &RedbStore,
    table: TableDefinition<'_, K, V>,
) -> Result<Vec<D>>
where
    K: redb::Key,
    for<'b> V: redb::Value<SelfType<'b> = &'b [u8]>,
    D: serde::de::DeserializeOwned,
{
    let read = store.db().begin_read()?;
    let table = read.open_table(table)?;
    let mut v: Vec<D> = Vec::with_capacity(20);
    for (_key, value) in (table.iter()?).flatten() {
        let s: D = serde_json::from_slice(value.value())?;
        v.push(s)
    }
    Ok(v)
}

pub(crate) async fn count<K, V>(store: &RedbStore, table: TableDefinition<'_, K, V>) -> Result<u64>
where
    K: redb::Key,
    for<'a> V: redb::Value<SelfType<'a> = &'a [u8]>,
{
    let read = store.db().begin_read()?;
    let table = read.open_table(table)?;
    let l = table.len()?;
    Ok(l)
}

// key: impl for<'a> Borrow<K::SelfType<'a>>,
// key: K::SelfType<'_>,
// pub(crate) fn write<K, V, D>(table: TableDefinition<'_, K, V>, key: &str, value: &D) -> Result<()>
// https://users.rust-lang.org/t/requesting-help-with-saving-data-into-redb-lifetime-problem/98586/7
pub(crate) async fn write<V, D>(
    store: &RedbStore,
    table: TableDefinition<'_, &str, V>,
    key: &str,
    value: &D,
) -> Result<()>
where
    V: for<'a> redb::Value<SelfType<'a> = &'a [u8]>,
    D: serde::Serialize,
{
    // 序列化放在入队**之前**：让调用方拿到 serde 错误，而不是让整批 op 因为一个
    // 坏值一起回滚。
    let value = serde_json::to_vec(value)?;
    store
        .submit(vec![RedbOp::Insert {
            table: String::from(table.name()),
            key: String::from(key),
            value,
        }])
        .await
}

pub(crate) async fn remove<K, V>(
    store: &RedbStore,
    table: TableDefinition<'_, K, V>,
    key: &str,
) -> Result<()>
where
    K: redb::Key,
    for<'b> V: redb::Value<SelfType<'b> = &'b [u8]>,
{
    store
        .submit(vec![RedbOp::Remove {
            table: String::from(table.name()),
            key: String::from(key),
        }])
        .await
}

pub(crate) async fn delete_table<K, V>(
    store: &RedbStore,
    table: TableDefinition<'_, K, V>,
) -> Result<()>
where
    K: redb::Key,
    for<'b> V: redb::Value<SelfType<'b> = &'b [u8]>,
{
    store
        .submit(vec![RedbOp::DeleteTable {
            table: String::from(table.name()),
        }])
        .await
}
