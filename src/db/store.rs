//! redb 的存储定位器 + 单写者队列。
//!
//! 定位器和写队列刻意做成同一个句柄：第 4 项按机器人分文件时，"这份数据落在哪个
//! 文件"和"这个文件的写队列"必须一起换，拆成两个东西会让调用点改两遍。
//!
//! # 队列不变量
//!
//! - **只有写线程能写。** 所有写操作走 [`RedbStore::submit`] 入队。
//! - **读不进队列。** `db()` 直接开读事务：redb 是 MVCC 的，读快照不与写者互斥。
//! - **job 内不得再入队。** 写线程是单线程循环，在 job 里等另一个 job 会死锁。

use std::sync::{Arc, LazyLock};
use std::vec::Vec;

use redb::{Database, TableDefinition};
use tokio::sync::{mpsc, oneshot};

use crate::result::{Error, Result};

/// 单批最多合并多少个 job。
///
/// 只是个上限，防止一次 drain 把内存吃满；正常负载下队列长度远小于它。
const MAX_BATCH: usize = 256;

/// 写队列容量。满了以后 `send` 会等（而不是丢弃，也不阻塞线程）。
const WRITE_QUEUE_CAPACITY: usize = 1024;

const TABLE_FILE_NAME: &str = "./data/flow.dat";

/// 一份数据属于谁 —— 也就是它该落在哪个文件里。
///
/// 现在（阶段 1）四个 key 都指向同一个文件，`store()` 忽略它。第 4 项改成
/// `data/robots/<id>/flow.dat` 时**只改 `store()` 的内部实现**，调用点一律不动。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum StoreKey<'a> {
    /// 跨机器人共享：机器人注册表、global-settings 及其系统元数据。
    Global,
    /// 属于单个机器人的数据。除上面那两类外，其余全部按机器人划分。
    Robot(&'a str),
}

/// 给 `db_executor!` 用的 key 构造。
///
/// 调用点手里的 `robot_id` 有 `String` / `&String` / `&str` 三种（历史遗留，混用），
/// 直接塞进 `StoreKey::Robot` 会撞上 `&&String` 不能退化到 `&str` 的问题。这里用一个
/// 泛型收口，让宏体和调用点都不必关心到底是哪一种。
pub(crate) fn robot_key<S: AsRef<str> + ?Sized>(robot_id: &S) -> StoreKey<'_> {
    let robot_id: &str = robot_id.as_ref();
    StoreKey::Robot(robot_id)
}

/// 写线程要执行的一个操作。
///
/// 四种 op 都作用在同一个 `begin_write()` 事务内，所以一批 op 只有一次 commit、
/// 一次 fsync —— 合批在这里是免费的，不需要额外设计。
pub(crate) enum RedbOp {
    /// 建表（幂等：redb 的 `open_table` 在写事务里就会建出来）。
    InitTable { table: String },
    Insert {
        table: String,
        key: String,
        value: Vec<u8>,
    },
    Remove { table: String, key: String },
    /// 整表删除，不报错（重建由 `InitTable` 负责）。
    DeleteTable { table: String },
}

/// 一批操作 + 一条回执。回执在**提交之后**才发出。
pub(crate) struct RedbJob {
    pub(super) ops: Vec<RedbOp>,
    pub(super) reply: oneshot::Sender<Result<()>>,
}

pub(crate) struct RedbStore {
    db: Arc<Database>,
    write_tx: mpsc::Sender<RedbJob>,
}

impl RedbStore {
    fn open(path: &str) -> Self {
        let data_folder = std::path::Path::new(".").join("data");
        if !data_folder.exists() {
            std::fs::create_dir(data_folder).expect("Create data directory failed.");
        }
        let db = if std::path::Path::new(path).exists() {
            Database::open(path).expect("Open database failed.")
        } else {
            Database::create(path).expect("Create database failed.")
        };
        let db = Arc::new(db);
        let (write_tx, write_rx) = mpsc::channel(WRITE_QUEUE_CAPACITY);
        let writer_db = Arc::clone(&db);
        std::thread::Builder::new()
            .name(String::from("redb-writer"))
            .spawn(move || writer_loop(&writer_db, write_rx))
            .expect("Spawn redb writer thread failed.");
        Self { db, write_tx }
    }

    /// 读路径。**同步**执行是刻意的。
    ///
    /// 调用方基本都是 async fn，但这里不做 `spawn_blocking`：`begin_read` 拿的
    /// 是 MVCC 快照，命中页缓存时是纯内存的 B 树查找，扔到阻塞线程池的往返开销
    /// 比查找本身还大。签名是 `sync` 而不是 `async`，也就不需要为第 4 项的懒打开
    /// 承担"读路径要不要变 async"的问题 —— 那件事在 `store()` 那一层解决。
    pub(crate) fn db(&self) -> &Database {
        &self.db
    }

    /// 把一批操作交给写线程，等提交完成再返回。
    ///
    /// 同一批 op 在一个 `begin_write()` 里提交：K 次写 = 1 次 fsync。跨调用能否
    /// 合批取决于写线程 drain 到了多少（见 [`writer_loop`]）。
    pub(crate) async fn submit(&self, ops: Vec<RedbOp>) -> Result<()> {
        if ops.is_empty() {
            return Ok(());
        }
        let (reply, rx) = oneshot::channel();
        // 队列满时在这里等，而不是阻塞线程 —— 调用方是 tokio worker。
        self.write_tx.send(RedbJob { ops, reply }).await?;
        rx.await.map_err(|e| {
            Error::WithMessage(format!("Redb writer thread dropped the reply: {e:?}"))
        })?
    }
}

/// 按 key 解析到对应的 store。
///
/// 阶段 1 忽略 key：四个 store 共用 `data/flow.dat`，返回同一个句柄。第 4 项在
/// 这里按 key 打开 `data/robots/<id>/…`，并配一套 LRU + 引用计数的常驻集淘汰。
///
/// **已经是 `async` 了**，因为第 4 项的懒打开要等 I/O 和空闲槽位；现在内部没有
/// 真正的 await，为的是让那次改造成纯函数体替换。
pub(crate) async fn store(_key: StoreKey<'_>) -> Result<&'static RedbStore> {
    static STORE: LazyLock<RedbStore> = LazyLock::new(|| RedbStore::open(TABLE_FILE_NAME));
    Ok(&STORE)
}

/// 写线程主循环。**单线程**，串行化了所有写。
fn writer_loop(db: &Database, mut rx: mpsc::Receiver<RedbJob>) {
    // 这个线程不在 tokio 运行时里，`blocking_recv` 正是给"专用线程从 async 队列
    // 取活"设计的。
    while let Some(first) = rx.blocking_recv() {
        let mut jobs = Vec::with_capacity(8);
        jobs.push(first);
        // 尽量把已经排队的都收进来：合批在这里发生，队列越长合批收益越大。
        while jobs.len() < MAX_BATCH {
            match rx.try_recv() {
                Ok(job) => jobs.push(job),
                Err(_) => break,
            }
        }
        run_batch(db, jobs);
    }
}

fn run_batch(db: &Database, jobs: Vec<RedbJob>) {
    match apply(db, &jobs) {
        Ok(()) => {
            for job in jobs {
                let _ = job.reply.send(Ok(()));
            }
        }
        Err(e) => {
            // 一个 op 失败会 abort 整批，从而牵连同批其它本可成功的 job。逐个重跑，
            // 让每个 job 拿到自己的结果 —— 否则一次偶发失败会放大成一片失败。
            log::warn!(
                "Redb batch of {} job(s) failed ({e:?}), retrying them one by one",
                jobs.len()
            );
            for job in jobs {
                let r = apply(db, std::slice::from_ref(&job));
                let _ = job.reply.send(r);
            }
        }
    }
}

/// 在一写事务里施加这批 job 的全部 op，然后**一次** commit。
fn apply(db: &Database, jobs: &[RedbJob]) -> Result<()> {
    if jobs.is_empty() {
        return Ok(());
    }
    let write_txn = db.begin_write()?;
    for job in jobs {
        for op in job.ops.iter() {
            match op {
                RedbOp::InitTable { table } => {
                    let table: TableDefinition<&str, &[u8]> = TableDefinition::new(table);
                    let _ = write_txn.open_table(table)?;
                }
                RedbOp::Insert { table, key, value } => {
                    let table: TableDefinition<&str, &[u8]> = TableDefinition::new(table);
                    let mut table = write_txn.open_table(table)?;
                    table.insert(key.as_str(), value.as_slice())?;
                }
                RedbOp::Remove { table, key } => {
                    let table: TableDefinition<&str, &[u8]> = TableDefinition::new(table);
                    let mut table = write_txn.open_table(table)?;
                    table.remove(key.as_str())?;
                }
                RedbOp::DeleteTable { table } => {
                    let table: TableDefinition<&str, &[u8]> = TableDefinition::new(table);
                    write_txn.delete_table(table)?;
                }
            }
        }
    }
    write_txn.commit()?;
    Ok(())
}
