use redb::{ReadableDatabase, TableDefinition};

use crate::db;
use crate::db::{RedbOp, RedbStore};
use crate::result::Result;

/// 运行时节点表按 `main_flow_id` 命名 —— 它是全局唯一的 scru128，所以表名不用变。
/// 但数据归属机器人：store 由调用方解析后传进来（见 `store()` 的说明）。
fn get_table_name(main_flow_id: &str) -> String {
    format!("RTN{main_flow_id}")
}

pub(crate) async fn get_runtime_node(
    store: &RedbStore,
    main_flow_id: &str,
    key: &str,
) -> Result<Option<crate::flow::rt::node::RuntimeNodeEnum>> {
    let table_name = get_table_name(main_flow_id);
    let table: TableDefinition<&str, &[u8]> = TableDefinition::new(&table_name);
    let read = store.db().begin_read()?;
    let table = read.open_table(table)?;
    let record = table.get(key)?;
    if let Some(r) = record {
        let n = crate::flow::rt::node::deser_node(r.value())?;
        return Ok(Some(n));
    }
    Ok(None)
}

pub(crate) async fn save_runtime_nodes(
    store: &RedbStore,
    main_flow_id: &str,
    nodes: Vec<(String, rkyv::util::AlignedVec)>,
) -> Result<()> {
    // 整批节点一次入队 —— 一个写事务、一次 fsync，而不是每个节点各写一次。
    let table_name = get_table_name(main_flow_id);
    let ops = nodes
        .into_iter()
        .map(|(key, value)| RedbOp::Insert {
            table: table_name.clone(),
            key,
            value: Vec::from(value.as_slice()),
        })
        .collect();
    store.submit(ops).await
}

pub(crate) async fn remove_runtime_nodes(store: &RedbStore, main_flow_id: &str) -> Result<()> {
    let table_name = get_table_name(main_flow_id);
    let table: TableDefinition<&str, &[u8]> = TableDefinition::new(&table_name);
    db::delete_table(store, table).await
}
