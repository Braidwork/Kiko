//! 存储路径的集成测试（§7 测试约定）。
//!
//! 每个用例自建 `tmp_<uuid>` schema、跑 migration、结束时 drop，不污染共享库。
//! 没配 `KIKO_TEST_DATABASE_URL` 时整组跳过 —— 没有 PG 的环境不该因此红一片。

use super::{
    context, doc, external_http, intent, mainflow, qa, robot, runtime_node, secret,
    settings as settings_repo, subflow, test_support::TestDb, variable,
};
use crate::external::http::dto::{
    HttpReqInfo, HttpReqParam, Method, PostContentType, Protocol, ValueSource,
};
use crate::man::settings::{GlobalSettings, Settings, split_global, split_settings};
use crate::robot::dto::{RobotData, RobotType};
use crate::variable::dto::{
    Variable, VariableObtainValueExpressionType, VariableType, VariableValueSource,
};
use crate::vector;

fn robot(id: &str, name: &str) -> RobotData {
    RobotData {
        robot_id: String::from(id),
        robot_name: String::from(name),
        robot_type: RobotType::TextBot,
    }
}

/// 测试用的模型定义：2 维向量，指纹与 `vector::current_model` 同格式。
fn test_model() -> crate::repo::vector::ModelRef {
    crate::repo::vector::ModelRef {
        provider: String::from("test"),
        model: String::from("test-model"),
        dimension: 2,
        normalized: false,
        fingerprint: String::from("v1|test|test-model|2"),
    }
}

/// 一小段可区分的测试向量（第 `i` 维为 1，其余 0）：相同文本 → 相似度 1。
fn unit_vec(dim: usize, i: usize) -> Vec<f32> {
    let mut v = vec![0.0f32; dim];
    v[i] = 1.0;
    v
}

fn variable(name: &str) -> Variable {
    Variable {
        var_name: String::from(name),
        var_type: VariableType::Str,
        var_val_source: VariableValueSource::Collect,
        var_constant_value: String::new(),
        var_associate_data: String::new(),
        obtain_value_expression_type: VariableObtainValueExpressionType::None,
        obtain_value_expression: String::new(),
        connect_timeout_milliseconds: 1000,
        read_timeout_milliseconds: 1500,
        cache_enabled: true,
    }
}

#[tokio::test]
async fn migration_creates_all_v1_tables() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };

    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT table_name::text FROM information_schema.tables WHERE table_schema = $1",
    )
    .bind(&db.schema)
    .fetch_all(&db.pool)
    .await
    .expect("查询 information_schema 失败");

    for expected in [
        "app_meta",
        "global_settings",
        "robots",
        "robot_settings",
        "secrets",
        "mainflows",
        "subflows",
        "variables",
        "contexts",
        "runtime_nodes",
        "intents",
        "intent_keywords",
        "intent_regexes",
        "intent_phrases",
        "external_http_apis",
        "external_http_params",
        "documents",
        "doc_chunks",
        "qa",
        "qa_questions",
        "vector_items",
        "embedding_models",
        "vector_collection_state",
        "embedding_generations",
        "users",
        "sessions",
        "audit_log",
        "audit_access_log",
    ] {
        assert!(
            tables.iter().any(|t| t == expected),
            "migration 少建了表 {expected}；实际：{tables:?}"
        );
    }

    db.cleanup().await;
}

#[tokio::test]
async fn robot_crud_roundtrip_and_order() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };

    // 顺序按 id 的字节序（COLLATE "C"）：大写在小写前、数字在字母前。
    robot::upsert(&db.pool, &robot("rB", "second"))
        .await
        .unwrap();
    robot::upsert(&db.pool, &robot("rA", "first"))
        .await
        .unwrap();
    robot::upsert(&db.pool, &robot("r1", "third"))
        .await
        .unwrap();

    let list = robot::list(&db.pool).await.unwrap();
    let ids: Vec<&str> = list.iter().map(|r| r.robot_id.as_str()).collect();
    assert_eq!(ids, ["r1", "rA", "rB"], "列表顺序应与 redb 的 key 序一致");

    let got = robot::get(&db.pool, "rA")
        .await
        .unwrap()
        .expect("rA 应该存在");
    assert_eq!(got.robot_name, "first");
    assert_eq!(robot::count(&db.pool).await.unwrap(), 3);

    // upsert = 整条覆盖，不是插入第二行。
    robot::upsert(&db.pool, &robot("rA", "renamed"))
        .await
        .unwrap();
    assert_eq!(robot::count(&db.pool).await.unwrap(), 3);
    assert_eq!(
        robot::get(&db.pool, "rA")
            .await
            .unwrap()
            .unwrap()
            .robot_name,
        "renamed"
    );

    assert!(robot::delete(&db.pool, "rA").await.unwrap());
    assert!(robot::get(&db.pool, "rA").await.unwrap().is_none());
    // 重复删除是幂等的（purge 需要）。
    assert!(!robot::delete(&db.pool, "rA").await.unwrap());

    db.cleanup().await;
}

#[tokio::test]
async fn variable_crud_roundtrip_and_order() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rX", "kb")).await.unwrap();

    variable::upsert(&db.pool, "rX", &variable("b"))
        .await
        .unwrap();
    variable::upsert(&db.pool, "rX", &variable("A"))
        .await
        .unwrap();
    variable::upsert(&db.pool, "rX", &variable("a"))
        .await
        .unwrap();

    let list = variable::list(&db.pool, "rX").await.unwrap();
    let names: Vec<&str> = list.iter().map(|v| v.var_name.as_str()).collect();
    assert_eq!(names, ["A", "a", "b"], "变量列表按 var_name 字节序");

    let mut v = variable("a");
    v.var_type = VariableType::Num;
    v.var_constant_value = String::from("42");
    v.obtain_value_expression_type = VariableObtainValueExpressionType::JsonPointer;
    v.obtain_value_expression = String::from("/data/x");
    v.connect_timeout_milliseconds = 250;
    v.cache_enabled = false;
    variable::upsert(&db.pool, "rX", &v).await.unwrap();

    let got = variable::get(&db.pool, "rX", "a").await.unwrap().unwrap();
    assert_eq!(got.var_type, VariableType::Num, "同名写入是整条覆盖");
    assert_eq!(got.var_constant_value, "42");
    assert_eq!(
        got.obtain_value_expression_type,
        VariableObtainValueExpressionType::JsonPointer
    );
    assert_eq!(got.obtain_value_expression, "/data/x");
    assert_eq!(got.connect_timeout_milliseconds, 250);
    assert!(!got.cache_enabled);
    assert_eq!(variable::list(&db.pool, "rX").await.unwrap().len(), 3);

    variable::delete(&db.pool, "rX", "a").await.unwrap();
    assert!(variable::get(&db.pool, "rX", "a").await.unwrap().is_none());
    // 删除不存在的变量不报错（与 redb 的 remove 一致）。
    variable::delete(&db.pool, "rX", "nope").await.unwrap();

    db.cleanup().await;
}

#[tokio::test]
async fn deleting_robot_cascades_variables() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rY", "cascade"))
        .await
        .unwrap();
    variable::upsert(&db.pool, "rY", &variable("v1"))
        .await
        .unwrap();
    variable::upsert(&db.pool, "rY", &variable("v2"))
        .await
        .unwrap();

    assert!(robot::delete(&db.pool, "rY").await.unwrap());
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM variables WHERE robot_id = $1")
        .bind("rY")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "删机器人必须级联清掉它的变量");

    db.cleanup().await;
}

fn param(name: &str, value: &str, vs: ValueSource) -> HttpReqParam {
    HttpReqParam {
        name: String::from(name),
        value: String::from(value),
        value_source: vs,
    }
}

fn http_api(id: &str, name: &str) -> HttpReqInfo {
    HttpReqInfo {
        id: String::from(id),
        name: String::from(name),
        description: String::from("desc"),
        protocol: Protocol::HTTPS,
        method: Method::POST,
        address: String::from("api.example.com/v1"),
        post_content_type: PostContentType::Raw,
        headers: vec![
            param("X-A", "1", ValueSource::Val),
            param("X-B", "var", ValueSource::Var),
        ],
        query_params: vec![param("q", "v", ValueSource::Var)],
        form_data: vec![
            param("f1", "a", ValueSource::Val),
            param("f2", "b", ValueSource::Val),
        ],
        request_body: String::from("{}"),
        content_type: String::from("application/json"),
        user_agent: String::from("kiko-test"),
    }
}

#[tokio::test]
async fn external_http_roundtrip_and_replace() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rH", "http")).await.unwrap();

    let mut api = http_api("api1", "first");
    external_http::upsert(&db.pool, "rH", &api).await.unwrap();

    let got = external_http::get(&db.pool, "rH", "api1")
        .await
        .unwrap()
        .expect("api1 应该存在");
    assert_eq!(got.name, "first");
    assert_eq!(got.protocol, Protocol::HTTPS);
    assert_eq!(got.method, Method::POST);
    assert_eq!(got.post_content_type, PostContentType::Raw);
    assert_eq!(got.content_type, "application/json");
    assert_eq!(got.user_agent, "kiko-test");
    assert_eq!(
        got.headers, api.headers,
        "header 顺序与 valueSource 都要保序"
    );
    assert_eq!(got.query_params, api.query_params);
    assert_eq!(got.form_data, api.form_data);

    // 同一个 id 再存一次 = 主表更新 + 参数整组替换，不能残留旧参数。
    api.name = String::from("renamed");
    api.headers = vec![param("X-Only", "1", ValueSource::Val)];
    api.query_params.clear();
    api.form_data = vec![param("f", "z", ValueSource::Var)];
    external_http::upsert(&db.pool, "rH", &api).await.unwrap();

    let got = external_http::get(&db.pool, "rH", "api1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.name, "renamed");
    assert_eq!(got.headers, api.headers);
    assert!(got.query_params.is_empty());
    assert_eq!(got.form_data, api.form_data);

    let params: i64 = sqlx::query_scalar("SELECT count(*) FROM external_http_params")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(params, 2, "旧参数必须被删掉，不能重复累积");

    // 列表与单条一致。
    let list = external_http::list(&db.pool, "rH").await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, "api1");

    external_http::delete(&db.pool, "rH", "api1").await.unwrap();
    assert!(
        external_http::get(&db.pool, "rH", "api1")
            .await
            .unwrap()
            .is_none()
    );
    let params: i64 = sqlx::query_scalar("SELECT count(*) FROM external_http_params")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(params, 0, "删 API 必须级联删参数");

    db.cleanup().await;
}

#[tokio::test]
async fn external_http_list_orders_by_id_and_isolates_robots() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rA", "a")).await.unwrap();
    robot::upsert(&db.pool, &robot("rB", "b")).await.unwrap();

    external_http::upsert(&db.pool, "rA", &http_api("b-api", "b"))
        .await
        .unwrap();
    external_http::upsert(&db.pool, "rA", &http_api("a-api", "a"))
        .await
        .unwrap();
    external_http::upsert(&db.pool, "rB", &http_api("c-api", "c"))
        .await
        .unwrap();

    let a = external_http::list(&db.pool, "rA").await.unwrap();
    let ids: Vec<&str> = a.iter().map(|x| x.id.as_str()).collect();
    assert_eq!(
        ids,
        ["a-api", "b-api"],
        "列表按 id 字节序，与 redb key 序一致"
    );

    let b = external_http::list(&db.pool, "rB").await.unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].id, "c-api");

    // 别的机器人拿不到别人的 API（防止跨机器人泄漏）。
    assert!(
        external_http::get(&db.pool, "rB", "a-api")
            .await
            .unwrap()
            .is_none()
    );

    db.cleanup().await;
}

#[tokio::test]
async fn external_http_surfaces_missing_robot_as_business_error() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };

    // 机器人不存在 → 外键冲突。错误文案要是人话（B-08），不是 sqlx 的原文。
    let err = external_http::upsert(&db.pool, "no-such-robot", &http_api("api1", "x"))
        .await
        .expect_err("外键冲突必须返回错误");
    let msg = err.message();
    assert!(
        msg.contains("Referenced row is missing"),
        "unexpected error message: {msg}"
    );

    db.cleanup().await;
}

/// 读一个意图的子项 `(id, 文本, position)`，按 position 排好。
async fn intent_children(
    db: &TestDb,
    table: &str,
    text_column: &str,
    intent_id: &str,
) -> Vec<(String, String, i32)> {
    let sql = format!(
        "SELECT id, {text_column}, position FROM {table} WHERE intent_id = $1 ORDER BY position"
    );
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(intent_id)
        .fetch_all(&db.pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn intent_crud_roundtrip_and_position_order() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rI", "intents"))
        .await
        .unwrap();

    let alpha = intent::create(
        &db.pool,
        "rI",
        "alpha",
        &[String::from("k1"), String::from("k2")],
    )
    .await
    .unwrap();
    let beta = intent::create(&db.pool, "rI", "beta", &[]).await.unwrap();

    let k3 = intent::add_keyword(&db.pool, "rI", &alpha, "k3")
        .await
        .unwrap()
        .expect("意图存在时必须返回新关键词 id");
    let r1 = intent::add_regex(&db.pool, "rI", &alpha, "^hi$")
        .await
        .unwrap()
        .expect("意图存在时必须返回新正则 id");
    let p1 = intent::add_phrase(
        &db.pool,
        "rI",
        &alpha,
        "hello",
        unit_vec(2, 0),
        &test_model(),
    )
    .await
    .unwrap()
    .expect("意图存在时必须返回新短语 id")
    .0;

    let got = intent::get(&db.pool, "rI", &alpha)
        .await
        .unwrap()
        .expect("alpha 应该存在");
    assert_eq!(got.intent_name, "alpha");
    let keywords: Vec<&str> = got.keywords.iter().map(|k| k.keyword.as_str()).collect();
    assert_eq!(keywords, ["k1", "k2", "k3"], "关键词按 position 追加");
    assert_eq!(got.keywords[2].id, k3, "返回的 id 要能定位到这一行");
    assert_eq!(got.regexes.len(), 1);
    assert_eq!(got.regexes[0].id, r1);
    assert_eq!(got.regexes[0].regex, "^hi$");
    assert_eq!(got.phrases.len(), 1);
    assert_eq!(got.phrases[0].id, p1);
    assert_eq!(got.phrases[0].phrase, "hello");

    let list = intent::list(&db.pool, "rI").await.unwrap();
    let names: Vec<&str> = list.iter().map(|d| d.intent_name.as_str()).collect();
    assert_eq!(names, ["alpha", "beta"], "意图按 position 排序");

    // 按 id 删除中间的关键词，剩下的顺序不变。
    let k2_id = got.keywords[1].id.clone();
    assert!(
        intent::remove_keyword(&db.pool, "rI", &alpha, &k2_id)
            .await
            .unwrap()
    );
    assert!(
        !intent::remove_keyword(&db.pool, "rI", &alpha, &k2_id)
            .await
            .unwrap()
    );
    let got = intent::get(&db.pool, "rI", &alpha).await.unwrap().unwrap();
    let keywords: Vec<&str> = got.keywords.iter().map(|k| k.keyword.as_str()).collect();
    assert_eq!(keywords, ["k1", "k3"]);

    // 短语的向量与载荷同事务：`vector_items` 里必须有一条指向这条短语的行
    // （B-22 / B-30：向量身份是 `vector_items.id`，不再有 `vec_row_id`）。
    let vectors: Vec<String> = sqlx::query_scalar(
        "SELECT entity_id FROM vector_items WHERE robot_id = $1 AND collection = 'phrase'",
    )
    .bind("rI")
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(vectors, vec![p1.clone()], "短语必须同时有向量行");
    let removed = intent::delete_phrase(&db.pool, "rI", &alpha, &p1)
        .await
        .unwrap();
    assert_eq!(removed.len(), 1, "删短语要返回被删的 vector id");
    let left: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM vector_items WHERE robot_id = $1 AND collection = 'phrase'",
    )
    .bind("rI")
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(left, 0, "删短语不能留下孤儿向量");
    assert!(
        intent::delete_phrase(&db.pool, "rI", &alpha, &p1)
            .await
            .unwrap()
            .is_empty(),
        "重复删除是幂等的"
    );

    // 删意图：子表全部级联。
    assert!(intent::delete(&db.pool, "rI", &beta).await.unwrap());
    assert!(intent::get(&db.pool, "rI", &beta).await.unwrap().is_none());
    assert!(!intent::delete(&db.pool, "rI", &beta).await.unwrap());
    assert!(intent::delete(&db.pool, "rI", &alpha).await.unwrap());
    for table in ["intent_keywords", "intent_regexes", "intent_phrases"] {
        let left: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(left, 0, "删意图必须级联清掉 {table}");
    }

    db.cleanup().await;
}

#[tokio::test]
async fn intent_positions_leave_holes_and_append() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rP", "positions"))
        .await
        .unwrap();
    let id = intent::create(&db.pool, "rP", "p", &[]).await.unwrap();

    for k in ["a", "b", "c"] {
        intent::add_keyword(&db.pool, "rP", &id, k)
            .await
            .unwrap()
            .unwrap();
    }
    let rows = intent_children(&db, "intent_keywords", "keyword", &id).await;
    let b_id = rows[1].0.clone();
    assert!(
        intent::remove_keyword(&db.pool, "rP", &id, &b_id)
            .await
            .unwrap()
    );
    intent::add_keyword(&db.pool, "rP", &id, "d")
        .await
        .unwrap()
        .unwrap();

    // 删除留空洞，新增取 MAX+1 —— 不重排已有行（§5.1）。
    let rows = intent_children(&db, "intent_keywords", "keyword", &id).await;
    let got: Vec<(&str, i32)> = rows.iter().map(|(_, k, p)| (k.as_str(), *p)).collect();
    assert_eq!(got, [("a", 0), ("c", 2), ("d", 3)]);

    let got = intent::get(&db.pool, "rP", &id).await.unwrap().unwrap();
    let keywords: Vec<&str> = got.keywords.iter().map(|k| k.keyword.as_str()).collect();
    assert_eq!(keywords, ["a", "c", "d"], "读出来的顺序必须是 position 序");

    // 意图自身的 position 同理：删 middle 不影响后面。
    let first = id;
    let second = intent::create(&db.pool, "rP", "second", &[]).await.unwrap();
    let third = intent::create(&db.pool, "rP", "third", &[]).await.unwrap();
    assert!(intent::delete(&db.pool, "rP", &second).await.unwrap());
    let fourth = intent::create(&db.pool, "rP", "fourth", &[]).await.unwrap();
    let list = intent::list(&db.pool, "rP").await.unwrap();
    let ids: Vec<&str> = list.iter().map(|d| d.intent_id.as_str()).collect();
    assert_eq!(ids, [first.as_str(), third.as_str(), fourth.as_str()]);

    db.cleanup().await;
}

#[tokio::test]
async fn concurrent_intent_writes_get_distinct_positions() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rC", "concurrent"))
        .await
        .unwrap();
    let id = intent::create(&db.pool, "rC", "c", &[]).await.unwrap();

    // 8 个并发追加：没有父行锁时，多个事务会读到同一个 MAX(position)，
    // 写出重复的 position（顺序变得不确定）。
    let mut tasks = Vec::new();
    for i in 0..8 {
        let pool = db.pool.clone();
        let intent_id = id.clone();
        tasks.push(tokio::spawn(async move {
            let keyword = format!("k{i}");
            intent::add_keyword(&pool, "rC", &intent_id, &keyword)
                .await
                .unwrap()
                .expect("意图存在");
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }

    let mut positions: Vec<i32> = sqlx::query_scalar(
        "SELECT position FROM intent_keywords WHERE intent_id = $1 ORDER BY position",
    )
    .bind(&id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    positions.sort_unstable();
    assert_eq!(
        positions,
        (0..8).collect::<Vec<i32>>(),
        "并发追加必须拿到互不相同的 position"
    );

    // 并发建意图同理：position 不能重复。
    let mut tasks = Vec::new();
    for i in 0..4 {
        let pool = db.pool.clone();
        tasks.push(tokio::spawn(async move {
            intent::create(&pool, "rC", &format!("i{i}"), &[])
                .await
                .unwrap()
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    let mut positions: Vec<i32> =
        sqlx::query_scalar("SELECT position FROM intents WHERE robot_id = $1")
            .bind("rC")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    positions.sort_unstable();
    assert_eq!(positions, vec![0, 1, 2, 3, 4], "并发建意图不能撞 position");

    db.cleanup().await;
}

#[tokio::test]
async fn intent_isolates_robots() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rA", "a")).await.unwrap();
    robot::upsert(&db.pool, &robot("rB", "b")).await.unwrap();
    let id = intent::create(&db.pool, "rA", "only-a", &[]).await.unwrap();
    let keyword = intent::add_keyword(&db.pool, "rA", &id, "k")
        .await
        .unwrap()
        .unwrap();
    let phrase = intent::add_phrase(&db.pool, "rA", &id, "p", unit_vec(2, 0), &test_model())
        .await
        .unwrap()
        .unwrap()
        .0;

    // 另一个机器人既读不到，也不能通过别人的 id 改数据。
    assert!(intent::get(&db.pool, "rB", &id).await.unwrap().is_none());
    assert!(intent::list(&db.pool, "rB").await.unwrap().is_empty());
    assert!(
        intent::add_keyword(&db.pool, "rB", &id, "stolen")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        intent::add_regex(&db.pool, "rB", &id, "x")
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        intent::add_phrase(&db.pool, "rB", &id, "stolen", unit_vec(2, 1), &test_model())
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !intent::remove_keyword(&db.pool, "rB", &id, &keyword)
            .await
            .unwrap()
    );
    assert!(
        intent::delete_phrase(&db.pool, "rB", &id, &phrase)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(!intent::delete(&db.pool, "rB", &id).await.unwrap());

    // rA 的数据原封不动。
    let got = intent::get(&db.pool, "rA", &id).await.unwrap().unwrap();
    assert_eq!(got.keywords.len(), 1);
    assert_eq!(got.phrases.len(), 1);

    db.cleanup().await;
}

#[tokio::test]
async fn deleting_robot_cascades_intents() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rD", "cascade"))
        .await
        .unwrap();
    let id = intent::create(&db.pool, "rD", "d", &[String::from("k")])
        .await
        .unwrap();
    intent::add_regex(&db.pool, "rD", &id, "^d$")
        .await
        .unwrap()
        .unwrap();
    intent::add_phrase(&db.pool, "rD", &id, "d", unit_vec(2, 0), &test_model())
        .await
        .unwrap()
        .unwrap();

    assert!(robot::delete(&db.pool, "rD").await.unwrap());
    for table in [
        "intents",
        "intent_keywords",
        "intent_regexes",
        "intent_phrases",
        "vector_items",
    ] {
        let left: i64 =
            sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT count(*) FROM {table}")))
                .fetch_one(&db.pool)
                .await
                .unwrap();
        assert_eq!(left, 0, "删机器人必须级联清掉 {table}");
    }

    db.cleanup().await;
}

#[tokio::test]
async fn intent_create_surfaces_missing_robot_as_business_error() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };

    let err = intent::create(&db.pool, "no-such-robot", "x", &[])
        .await
        .expect_err("外键冲突必须返回错误");
    let msg = err.message();
    assert!(
        msg.contains("Referenced row is missing"),
        "unexpected error message: {msg}"
    );

    db.cleanup().await;
}

fn context_payload(robot_id: &str, session_id: &str) -> serde_json::Value {
    serde_json::json!({
        "robot_id": robot_id,
        "session_id": session_id,
        "main_flow_id": "mf1",
        "nodes": ["node-a", "node-b"],
        "vars": {"name": {"Str": "kiko"}},
        "last_active_time": 123,
        "chat_history": [{"role": "user", "content": "hi"}]
    })
}

#[tokio::test]
async fn context_roundtrip_and_last_write_wins() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rS", "sessions"))
        .await
        .unwrap();
    robot::upsert(&db.pool, &robot("rT", "other"))
        .await
        .unwrap();

    let data = context_payload("rS", "s1");
    context::upsert(&db.pool, "rS", "s1", &data, Some(&[1, 2, 3]), 3600)
        .await
        .unwrap();

    let row = context::get(&db.pool, "rS", "s1")
        .await
        .unwrap()
        .expect("会话应该存在");
    assert_eq!(row.data, data, "JSONB 载荷要原样往返");
    assert_eq!(row.node_state, Some(vec![1, 2, 3]), "节点状态单独走 BYTEA");

    // 同一个 session_id 在不同机器人下是两行，互不影响。
    context::upsert(&db.pool, "rT", "s1", &data, None, 3600)
        .await
        .unwrap();
    assert!(context::get(&db.pool, "rT", "s1").await.unwrap().is_some());
    assert!(
        context::get(&db.pool, "rS", "nope")
            .await
            .unwrap()
            .is_none()
    );

    // 同一主键再存 = 整条覆盖（最后写入胜出），不是插入第二行。
    let updated = serde_json::json!({"robot_id": "rS", "session_id": "s1", "vars": {}});
    context::upsert(&db.pool, "rS", "s1", &updated, None, 3600)
        .await
        .unwrap();
    let row = context::get(&db.pool, "rS", "s1").await.unwrap().unwrap();
    assert_eq!(row.data, updated);
    assert_eq!(row.node_state, None, "覆盖时 node_state 也要跟着变");
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM contexts WHERE robot_id = $1 AND session_id = $2")
            .bind("rS")
            .bind("s1")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(count, 1, "upsert 不能留下重复行");

    db.cleanup().await;
}

#[tokio::test]
async fn context_expiry_deletes_only_expired_sessions() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rE", "expiry"))
        .await
        .unwrap();

    // ttl=0 → expires_at = now()；ttl=3600 → 一小时后。
    context::upsert(
        &db.pool,
        "rE",
        "old",
        &context_payload("rE", "old"),
        None,
        0,
    )
    .await
    .unwrap();
    context::upsert(
        &db.pool,
        "rE",
        "fresh",
        &context_payload("rE", "fresh"),
        None,
        3600,
    )
    .await
    .unwrap();

    // 给 `now()` 一点前进的余地（清理是另一条事务）。
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    let deleted = context::delete_expired(&db.pool).await.unwrap();
    assert_eq!(deleted, 1, "只删已过期的那一条");
    assert!(context::get(&db.pool, "rE", "old").await.unwrap().is_none());
    assert!(
        context::get(&db.pool, "rE", "fresh")
            .await
            .unwrap()
            .is_some()
    );

    db.cleanup().await;
}

#[tokio::test]
async fn deleting_robot_cascades_contexts() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rX", "cascade"))
        .await
        .unwrap();
    context::upsert(
        &db.pool,
        "rX",
        "s1",
        &context_payload("rX", "s1"),
        Some(&[9]),
        3600,
    )
    .await
    .unwrap();

    assert!(robot::delete(&db.pool, "rX").await.unwrap());
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM contexts WHERE robot_id = $1")
        .bind("rX")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "删机器人必须级联清掉会话");

    db.cleanup().await;
}

#[tokio::test]
async fn mainflow_create_lists_and_cascades() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rF", "flows"))
        .await
        .unwrap();

    let first = mainflow::create(&db.pool, "rF", "first", "default sub")
        .await
        .unwrap();
    assert_eq!(first.name, "first");
    assert!(first.enabled);
    // D48：id 是纯 scru128，不再有 count 前缀。
    assert_eq!(
        first.id.len(),
        scru128::new_string().len(),
        "主流程 id 不应再带 count 前缀"
    );

    // 默认子流程与主流程在同一个事务里创建。
    let subs = subflow::list(&db.pool, "rF", &first.id).await.unwrap();
    assert_eq!(subs.len(), 1);
    assert_eq!(subs[0].name, "default sub");
    assert_eq!(subs[0].canvas, "", "空画布读回空串");
    assert_ne!(subs[0].id, first.id, "子流程用自己的 id");

    let second = mainflow::create(&db.pool, "rF", "second", "default sub")
        .await
        .unwrap();
    let list = mainflow::list(&db.pool, "rF").await.unwrap();
    let names: Vec<&str> = list.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["first", "second"], "主流程按 position 排序");

    // 保存（PUT）是更新，不新增行。
    let mut renamed = first.clone();
    renamed.name = String::from("renamed");
    renamed.enabled = false;
    assert!(mainflow::upsert(&db.pool, "rF", &renamed).await.unwrap());
    let list = mainflow::list(&db.pool, "rF").await.unwrap();
    assert_eq!(list.len(), 2);
    assert_eq!(list[0].name, "renamed");
    assert!(!list[0].enabled);

    // 删主流程：子流程级联，运行时节点由仓储显式清掉。
    runtime_node::upsert_many(
        &db.pool,
        "rF",
        &first.id,
        &[(String::from("n1"), vec![1, 2, 3])],
    )
    .await
    .unwrap();
    assert!(mainflow::delete(&db.pool, "rF", &first.id).await.unwrap());
    assert!(
        subflow::list(&db.pool, "rF", &first.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        runtime_node::get(&db.pool, "rF", &first.id, "n1")
            .await
            .unwrap()
            .is_none()
    );
    let list = mainflow::list(&db.pool, "rF").await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, second.id);

    db.cleanup().await;
}

#[tokio::test]
async fn subflow_crud_by_id_and_position() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rG", "subflows"))
        .await
        .unwrap();
    let mf = mainflow::create(&db.pool, "rG", "mf", "one").await.unwrap();
    let mut flows = subflow::list(&db.pool, "rG", &mf.id).await.unwrap();
    assert_eq!(flows.len(), 1);

    let added = subflow::create(&db.pool, "rG", &mf.id, "two")
        .await
        .unwrap();
    assert_eq!(added.len(), 2);
    let two_id = added[1].id.clone();

    // 位置 0/1，按 position 读。
    let positions: Vec<i32> = sqlx::query_scalar(
        "SELECT position FROM subflows WHERE main_flow_id = $1 ORDER BY position",
    )
    .bind(&mf.id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(positions, [0, 1]);

    // 保存画布按 id（不再按下标）。
    let canvas = r#"{"cells":[{"id":"a","shape":"edge"}]}"#;
    assert!(
        subflow::save_canvas(&db.pool, "rG", &mf.id, &two_id, canvas)
            .await
            .unwrap()
    );
    flows = subflow::list(&db.pool, "rG", &mf.id).await.unwrap();
    assert_eq!(flows[1].canvas, canvas);

    // 空画布写库是合法 JSON（{}），读回仍是空串。
    assert!(
        subflow::save_canvas(&db.pool, "rG", &mf.id, &two_id, "")
            .await
            .unwrap()
    );
    flows = subflow::list(&db.pool, "rG", &mf.id).await.unwrap();
    assert_eq!(flows[1].canvas, "");
    let raw: serde_json::Value = sqlx::query_scalar("SELECT canvas FROM subflows WHERE id = $1")
        .bind(&two_id)
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(raw, serde_json::json!({}), "空画布必须存成合法 JSON 对象");

    // 另一个机器人不能通过别人的 mainflow id 改数据。
    assert!(
        !subflow::save_canvas(&db.pool, "rOther", &mf.id, &two_id, "{}")
            .await
            .unwrap()
    );
    assert!(
        !subflow::delete(&db.pool, "rOther", &mf.id, &two_id)
            .await
            .unwrap()
    );

    // 按 id 删除；重复删除是幂等的。
    assert!(
        subflow::delete(&db.pool, "rG", &mf.id, &two_id)
            .await
            .unwrap()
    );
    assert!(
        !subflow::delete(&db.pool, "rG", &mf.id, &two_id)
            .await
            .unwrap()
    );
    assert_eq!(
        subflow::list(&db.pool, "rG", &mf.id).await.unwrap().len(),
        1
    );

    db.cleanup().await;
}

#[tokio::test]
async fn concurrent_subflow_creates_get_distinct_positions() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rH", "concurrent"))
        .await
        .unwrap();
    let mf = mainflow::create(&db.pool, "rH", "mf", "one").await.unwrap();

    // 8 路并发追加：没有父主流程行锁时会算出重复 position。
    let mut tasks = Vec::new();
    for i in 0..8 {
        let pool = db.pool.clone();
        let main_flow_id = mf.id.clone();
        tasks.push(tokio::spawn(async move {
            subflow::create(&pool, "rH", &main_flow_id, &format!("s{i}"))
                .await
                .unwrap();
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    let mut positions: Vec<i32> = sqlx::query_scalar(
        "SELECT position FROM subflows WHERE main_flow_id = $1 ORDER BY position",
    )
    .bind(&mf.id)
    .fetch_all(&db.pool)
    .await
    .unwrap();
    positions.sort_unstable();
    assert_eq!(
        positions,
        (0..9).collect::<Vec<i32>>(),
        "默认子流程 + 8 条并发追加，position 必须互不相同"
    );

    // 并发建主流程同理。
    let mut tasks = Vec::new();
    for i in 0..4 {
        let pool = db.pool.clone();
        tasks.push(tokio::spawn(async move {
            mainflow::create(&pool, "rH", &format!("m{i}"), "sub")
                .await
                .unwrap();
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    let mut positions: Vec<i32> =
        sqlx::query_scalar("SELECT position FROM mainflows WHERE robot_id = $1")
            .bind("rH")
            .fetch_all(&db.pool)
            .await
            .unwrap();
    positions.sort_unstable();
    assert_eq!(positions, vec![0, 1, 2, 3, 4]);

    db.cleanup().await;
}

#[tokio::test]
async fn runtime_nodes_roundtrip_and_demo_ids() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rN", "nodes"))
        .await
        .unwrap();
    let mf = mainflow::create(&db.pool, "rN", "mf", "one").await.unwrap();

    runtime_node::upsert_many(
        &db.pool,
        "rN",
        &mf.id,
        &[
            (String::from("n1"), vec![1, 2, 3]),
            (String::from("n2"), vec![4]),
        ],
    )
    .await
    .unwrap();
    assert_eq!(
        runtime_node::get(&db.pool, "rN", &mf.id, "n1")
            .await
            .unwrap(),
        Some(vec![1, 2, 3])
    );
    // 同一 node_id 再写 = 覆盖。
    runtime_node::upsert_many(&db.pool, "rN", &mf.id, &[(String::from("n1"), vec![9, 9])])
        .await
        .unwrap();
    assert_eq!(
        runtime_node::get(&db.pool, "rN", &mf.id, "n1")
            .await
            .unwrap(),
        Some(vec![9, 9])
    );
    assert_eq!(
        runtime_node::get(&db.pool, "rN", &mf.id, "n2")
            .await
            .unwrap(),
        Some(vec![4])
    );

    // demo 流程没有 mainflows 行，但运行时节点要能写/读（靠 robot_id 归属）。
    runtime_node::upsert_many(
        &db.pool,
        "rN",
        "demo-collect",
        &[(String::from("start"), vec![7])],
    )
    .await
    .unwrap();
    assert_eq!(
        runtime_node::get(&db.pool, "rN", "demo-collect", "start")
            .await
            .unwrap(),
        Some(vec![7])
    );

    // 删机器人：真实主流程与 demo 的节点都由 robot_id 外键级联清掉。
    assert!(robot::delete(&db.pool, "rN").await.unwrap());
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_nodes WHERE robot_id = $1")
        .bind("rN")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(left, 0, "删机器人必须级联清掉运行时节点（含 demo）");

    db.cleanup().await;
}

#[tokio::test]
async fn robot_settings_and_secrets_roundtrip() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rSet", "settings"))
        .await
        .unwrap();

    let mut s = Settings::default();
    s.chat_provider.api_key = String::from("sk-plain");
    s.smtp_password = String::from("smtp-pw");
    let (stored, changes) = split_settings(&s).unwrap();
    // 设置 JSON 里只有引用串，密钥在 secrets 表里是密文。
    assert_eq!(stored.chat_provider.api_key, "enc:chat_api_key");
    assert_eq!(stored.smtp_password, "enc:smtp_password");
    settings_repo::save_robot(&db.pool, "rSet", &stored, &changes)
        .await
        .unwrap();

    let got = settings_repo::get_robot(&db.pool, "rSet")
        .await
        .unwrap()
        .expect("设置应该存在");
    assert_eq!(got.chat_provider.api_key, "enc:chat_api_key");
    let chat_secret = secret::get(&db.pool, "rSet", "chat_api_key")
        .await
        .unwrap()
        .expect("chat secret 应该存在");
    assert!(
        !chat_secret.value.contains("sk-plain"),
        "secrets.value 不能是明文"
    );
    assert_eq!(
        crate::security::decrypt(&chat_secret.value).unwrap(),
        "sk-plain"
    );

    // D36 的关键往返：把读到的引用串原样写回，密文不能变。
    let (stored2, changes2) = split_settings(&got).unwrap();
    assert!(
        changes2
            .iter()
            .all(|c| matches!(c, crate::repo::secret::SecretChange::Delete { .. })),
        "原样写回引用串不应重新加密/覆盖已有密文"
    );
    settings_repo::save_robot(&db.pool, "rSet", &stored2, &changes2)
        .await
        .unwrap();
    let after = secret::get(&db.pool, "rSet", "chat_api_key")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.value, chat_secret.value, "密文必须保持不变");

    // 改成 env: 引用：设置里保留引用串，旧的密文被清掉。
    let mut s3 = got;
    s3.chat_provider.api_key = String::from("env:MY_API_KEY");
    let (stored3, changes3) = split_settings(&s3).unwrap();
    assert_eq!(stored3.chat_provider.api_key, "env:MY_API_KEY");
    assert!(
        changes3
            .iter()
            .any(|c| matches!(c, crate::repo::secret::SecretChange::Delete { name } if name == "chat_api_key")),
        "env: 引用必须清掉同名密文"
    );

    // 删机器人级联 robot_settings；secrets.scope 没有外键，要单独清。
    assert!(robot::delete(&db.pool, "rSet").await.unwrap());
    assert!(
        settings_repo::get_robot(&db.pool, "rSet")
            .await
            .unwrap()
            .is_none()
    );
    let left: i64 = sqlx::query_scalar("SELECT count(*) FROM robot_settings WHERE robot_id = $1")
        .bind("rSet")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(left, 0);
    assert_eq!(
        secret::delete_scope(&db.pool, "rSet").await.unwrap(),
        2,
        "chat_api_key 与 smtp_password 两行"
    );

    db.cleanup().await;
}

#[tokio::test]
async fn global_settings_and_hf_token_roundtrip() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };

    let mut g = GlobalSettings::default();
    g.hf_model_download.access_token = String::from("hf-plain-token");
    let (stored, changes) = split_global(&g).unwrap();
    assert_eq!(stored.hf_model_download.access_token, "enc:hf_token");
    settings_repo::save_global(&db.pool, &stored, &changes)
        .await
        .unwrap();

    let got = settings_repo::get_global(&db.pool)
        .await
        .unwrap()
        .expect("全局设置应该存在");
    assert_eq!(got.ip, g.ip);
    assert_eq!(got.port, g.port);
    assert_eq!(got.hf_model_download.access_token, "enc:hf_token");

    let secret = secret::get(&db.pool, "global", "hf_token")
        .await
        .unwrap()
        .expect("hf_token 应该存在");
    assert_eq!(
        crate::security::decrypt(&secret.value).unwrap(),
        "hf-plain-token"
    );

    // upsert 不产生第二行。
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM global_settings")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 1);

    db.cleanup().await;
}

/// 业务粘合层端到端：全局池指向测试 schema，跑一遍真实启动会走到的设置路径。
#[tokio::test]
async fn settings_service_roundtrip_through_global_pool() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    // 只有本用例用全局池；其它仓储测试都显式传 &db.pool。
    if !super::install_test_pool(db.pool.clone()) {
        db.cleanup().await;
        return;
    }

    // 1. 启动：默认全局设置 + app_meta。
    let global = crate::man::settings::init_global().await.unwrap();
    assert_eq!(global.port, 12715);
    assert_eq!(
        crate::man::settings::get_global_settings()
            .await
            .unwrap()
            .unwrap()
            .ip,
        global.ip
    );

    // 2. 新机器人：默认设置落库。
    robot::upsert(&db.pool, &robot("rSvc", "service"))
        .await
        .unwrap();
    crate::man::settings::init("rSvc").await.unwrap();
    assert!(
        crate::man::settings::get_settings("rSvc")
            .await
            .unwrap()
            .is_some()
    );

    // 3. 设置页保存：明文密钥被拆到 secrets，设置 JSON 只有 `enc:` 引用。
    let mut s = Settings::default();
    s.chat_provider.api_key = String::from("sk-service");
    s.smtp_password = String::from("smtp-service");
    crate::man::settings::save_settings("rSvc", s)
        .await
        .unwrap();
    let stored = crate::man::settings::get_stored_settings("rSvc")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.chat_provider.api_key, "enc:chat_api_key");
    assert_eq!(stored.smtp_password, "enc:smtp_password");

    // 4. 运行期读回：引用被解密，缓存里也是明文形态。
    let hydrated = crate::man::settings::get_settings("rSvc")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(hydrated.chat_provider.api_key, "sk-service");
    assert_eq!(hydrated.smtp_password, "smtp-service");

    // 5. 原样写回（设置页读取 → 不改 → 保存）：密文不变。
    let before = secret::get(&db.pool, "rSvc", "chat_api_key")
        .await
        .unwrap()
        .unwrap()
        .value;
    crate::man::settings::save_settings("rSvc", stored)
        .await
        .unwrap();
    let after = secret::get(&db.pool, "rSvc", "chat_api_key")
        .await
        .unwrap()
        .unwrap()
        .value;
    assert_eq!(before, after);

    // 6. 知识库端到端（S4）：假 embedding 端点 + 真实 PG + 进程内索引。
    //
    //    这一段覆盖三条检索：QA（相似度）、文档（分块 + 向量 + 原文件落盘）、
    //    意图短语（阈值方向 D41）。模型推理被换成本地假端点，所以既不需要下载权重，
    //    也不会出网。
    let embed_url = crate::ai::test_support::serve_embeddings(16).await;
    robot::upsert(&db.pool, &robot("rKb", "kb")).await.unwrap();
    crate::man::settings::init("rKb").await.unwrap();
    let mut s = Settings::default();
    s.sentence_embedding_provider.provider =
        crate::ai::embedding::SentenceEmbeddingProvider::OpenAICompatible(String::from(
            "fake-embed",
        ));
    s.sentence_embedding_provider.api_url = embed_url;
    crate::man::settings::save_settings("rKb", s).await.unwrap();

    // 6.1 QA：保存 → 列表 → 检索。
    let qa_id = crate::kb::qa::save(
        "rKb",
        crate::kb::dto::QuestionAnswerPair {
            id: None,
            question: crate::kb::dto::QuestionData {
                question: String::from("hello"),
            },
            similar_questions: Vec::new(),
            answer: String::from("world"),
        },
    )
    .await
    .unwrap();
    let qa_list = crate::kb::qa::list("rKb").await.unwrap();
    assert_eq!(qa_list.len(), 1);
    assert_eq!(qa_list[0].id.as_deref(), Some(qa_id.as_str()));
    let (found, similarity) = crate::kb::qa::retrieve_answer("rKb", "hello")
        .await
        .unwrap();
    assert!(similarity > 0.99, "同一文本的相似度应接近 1：{similarity}");
    assert_eq!(found.expect("必须命中").answer, "world");

    // 6.2 文档：docx → 分块 + 向量 + 原文件 → 索引检索。
    let docx = minimal_docx(&["Rust 知识库", "second paragraph"]);
    let text = crate::kb::doc::save("rKb", "a.docx", DOCX_MIME, &docx)
        .await
        .unwrap();
    assert!(text.contains("Rust 知识库"));
    let docs = crate::kb::doc::list("rKb").await.unwrap();
    assert_eq!(docs.len(), 1);
    let doc_id = docs[0].id.clone();
    let upload = std::path::Path::new("data")
        .join("uploads")
        .join("rKb")
        .join(format!("{doc_id}.docx"));
    assert!(upload.exists(), "原文件必须落盘：{}", upload.display());
    let q = crate::ai::embedding::embedding("rKb", "Rust 知识库")
        .await
        .unwrap()
        .0;
    let hits = vector::search("rKb", super::vector::DOC, &q, 8)
        .await
        .unwrap();
    assert!(!hits.is_empty(), "文档向量必须进索引");
    assert!(
        hits[0].score > 0.85,
        "同一段文本命中分数应高于默认阈值：{}",
        hits[0].score
    );
    let payload = doc::chunks_for_vectors(&db.pool, "rKb", &[hits[0].id as i64])
        .await
        .unwrap();
    assert!(
        payload[0].2.contains("Rust 知识库"),
        "索引命中的分块应包含原文：{}",
        payload[0].2
    );

    // 6.3 意图短语：加一条 → 检测命中（相似度阈值方向）。
    let intent_id = intent::create(&db.pool, "rKb", "only", &[]).await.unwrap();
    let (vec, model) = crate::intent::phrase::embed("rKb", "exact phrase")
        .await
        .unwrap();
    intent::add_phrase(&db.pool, "rKb", &intent_id, "exact phrase", vec, &model)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        crate::intent::detector::detect("rKb", "exact phrase")
            .await
            .unwrap()
            .as_deref(),
        Some("only")
    );

    // 6.4 重建（B-23 / D47）：三个集合各翻到第 1 代，旧代清掉，检索仍然命中。
    crate::man::reindex::run("rKb").await.unwrap();
    for collection in super::vector::COLLECTIONS {
        let st = super::vector::state(&db.pool, "rKb", collection)
            .await
            .unwrap()
            .unwrap_or_else(|| panic!("{collection} 应该有 state 行"));
        assert_eq!(st.active_generation, 1, "{collection} 应该翻到第 1 代");
        assert_eq!(st.pending_generation, None, "{collection} pending 必须清空");
        assert_eq!(st.status, "idle", "{collection} 状态必须回到 idle");
        let generations: Vec<i64> = sqlx::query_scalar(
            "SELECT DISTINCT generation FROM vector_items
              WHERE robot_id = 'rKb' AND collection = $1",
        )
        .bind(collection)
        .fetch_all(&db.pool)
        .await
        .unwrap();
        assert_eq!(generations, vec![1], "{collection} 旧代向量必须清掉");
    }
    // 重建作废了索引缓存；下一次检索要从 PG 重载新代并继续命中。
    let (found, similarity) = crate::kb::qa::retrieve_answer("rKb", "hello")
        .await
        .unwrap();
    assert!(similarity > 0.99, "重建后同一文本仍应接近 1：{similarity}");
    assert_eq!(found.expect("重建后仍要命中").answer, "world");
    assert_eq!(
        crate::intent::detector::detect("rKb", "exact phrase")
            .await
            .unwrap()
            .as_deref(),
        Some("only")
    );

    // 6.5 B-28：写后同步失败 → 标 dirty；下一次检索从 PG 重载并清掉标记。
    assert!(
        !super::vector::state(&db.pool, "rKb", super::vector::QA)
            .await
            .unwrap()
            .unwrap()
            .dirty
    );
    // QA 索引是 4 维；塞一条 3 维向量会被 BruteForce 拒绝（模拟"索引更新失败"）。
    vector::upsert("rKb", super::vector::QA, &[(999_u64, vec![1.0, 0.0, 0.0])]).await;
    let st = super::vector::state(&db.pool, "rKb", super::vector::QA)
        .await
        .unwrap()
        .unwrap();
    assert!(st.dirty, "索引同步失败必须标 dirty");
    let (found, _) = crate::kb::qa::retrieve_answer("rKb", "hello")
        .await
        .unwrap();
    assert_eq!(found.expect("重载后仍要命中").answer, "world");
    assert!(
        !super::vector::state(&db.pool, "rKb", super::vector::QA)
            .await
            .unwrap()
            .unwrap()
            .dirty,
        "从 PG 重载成功后 dirty 必须清掉"
    );

    // 6.6 对账：先让 doc 索引载入，再手工插一条"进程外写入"的向量，行数漂移必须
    // 被发现 → 作废缓存 + 标 dirty；下一次检索重载后恢复。
    // 注意 generation 必须是 active 那一代（重建后是 1），否则不计入行数。
    let _ = vector::search("rKb", super::vector::DOC, &[1.0, 0.0, 0.0, 0.0], 1)
        .await
        .unwrap();
    let doc_generation = super::vector::state(&db.pool, "rKb", super::vector::DOC)
        .await
        .unwrap()
        .unwrap()
        .active_generation;
    sqlx::query(
        "INSERT INTO vector_items (robot_id, collection, entity_id, generation, vec)
         VALUES ('rKb', 'doc', 'ghost', $2, $1)",
    )
    .bind(crate::vector::index::encode(&[1.0, 0.0, 0.0, 0.0]))
    .bind(doc_generation)
    .execute(&db.pool)
    .await
    .unwrap();
    let fixed = crate::vector::reconcile::reconcile_once().await.unwrap();
    assert!(fixed >= 1, "行数漂移必须被对账发现");
    assert!(
        super::vector::state(&db.pool, "rKb", super::vector::DOC)
            .await
            .unwrap()
            .unwrap()
            .dirty,
        "对账作废缓存时要标 dirty"
    );
    let _ = vector::search("rKb", super::vector::DOC, &[1.0, 0.0, 0.0, 0.0], 5)
        .await
        .unwrap();
    assert!(
        !super::vector::state(&db.pool, "rKb", super::vector::DOC)
            .await
            .unwrap()
            .unwrap()
            .dirty,
        "重载成功后 dirty 必须清掉"
    );
    sqlx::query("DELETE FROM vector_items WHERE robot_id = 'rKb' AND entity_id = 'ghost'")
        .execute(&db.pool)
        .await
        .unwrap();
    vector::invalidate("rKb", super::vector::DOC);

    // 本地文件清理（PG 数据随 schema drop 一起消失）。
    crate::kb::doc::remove_uploads("rKb").await.unwrap();

    db.cleanup().await;
}

const DOCX_MIME: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";

/// 内存里拼一个最小 docx（WordprocessingML），供上传路径测试用。
fn minimal_docx(paragraphs: &[&str]) -> Vec<u8> {
    use std::io::Write;

    use zip::write::SimpleFileOptions;

    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut w = zip::ZipWriter::new(&mut cursor);
        w.start_file("word/document.xml", SimpleFileOptions::default())
            .unwrap();
        let mut xml = String::from(
            "<?xml version=\"1.0\"?><w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\"><w:body>",
        );
        for p in paragraphs {
            xml.push_str(&format!("<w:p><w:r><w:t>{p}</w:t></w:r></w:p>"));
        }
        xml.push_str("</w:body></w:document>");
        w.write_all(xml.as_bytes()).unwrap();
        w.finish().unwrap();
    }
    cursor.into_inner()
}

// ============================================================ S4 知识库 + 向量

fn chunk(position: i32, text: &str, axis: usize) -> doc::ChunkInput {
    doc::ChunkInput {
        position,
        text: String::from(text),
        norm: text.to_lowercase(),
        vec: unit_vec(2, axis),
    }
}

fn q(text: &str, is_primary: bool, axis: usize) -> qa::QuestionInput {
    qa::QuestionInput {
        question: String::from(text),
        is_primary,
        vec: unit_vec(2, axis),
    }
}

/// B-21：文档、分块（含 `chunk_text_norm`）与向量在同一个事务里落库；删文档时
/// 三者一起消失，并带回原文件路径。
#[tokio::test]
async fn document_chunks_and_vectors_are_written_in_one_transaction() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rDoc", "doc"))
        .await
        .unwrap();
    let model = test_model();
    let chunks = vec![chunk(0, "Rust 知识库", 0), chunk(1, "second chunk", 1)];
    let ids = doc::create(
        &db.pool,
        "rDoc",
        "doc1",
        &doc::NewDocument {
            file_name: String::from("a.docx"),
            file_size: 10,
            mime_type: Some(String::from(
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            )),
            original_path: Some(String::from("uploads/rDoc/doc1.docx")),
            content_hash: Some(String::from("hash")),
            doc_content: String::from("Rust 知识库\nsecond chunk"),
        },
        &chunks,
        &model,
    )
    .await
    .unwrap();
    assert_eq!(ids.len(), 2, "每个分块一条向量");

    let rows = doc::list(&db.pool, "rDoc").await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, "doc1");
    assert_eq!(rows[0].doc_content, "Rust 知识库\nsecond chunk");

    // chunk_text 保原文、chunk_text_norm 小写（§16.3：原文要进 prompt）。
    let (orig, norm): (String, String) = sqlx::query_as(
        "SELECT chunk_text, chunk_text_norm FROM doc_chunks
          WHERE document_id = 'doc1' ORDER BY position LIMIT 1",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(orig, "Rust 知识库", "chunk_text 不能小写化");
    assert_eq!(norm, "rust 知识库", "chunk_text_norm 是应用层小写化结果");

    // 实体与向量同事务：每个向量行都能找回自己的分块。
    let orphan: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM vector_items v
          WHERE v.collection = 'doc'
            AND NOT EXISTS (SELECT 1 FROM doc_chunks c WHERE c.id = v.entity_id)",
    )
    .fetch_one(&db.pool)
    .await
    .unwrap();
    assert_eq!(orphan, 0, "不能出现有向量无分块");

    // 关键词召回走 chunk_text_norm，大小写无关。仓储收到的已经是 LIKE 模式
    // （`%kw%` 由 `kb::doc` 负责拼，见 §16.3）。
    let hits = doc::keyword_recall(&db.pool, "rDoc", &[String::from("%rust%")], 10)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1, "keyword_recall 应该是子串匹配");
    assert_eq!(hits[0].chunk_text, "Rust 知识库");

    // 索引 top-k → 回 PG 取载荷：vector id 能映射回分块 id + 原文。
    let payload = doc::chunks_for_vectors(&db.pool, "rDoc", &[ids[0]])
        .await
        .unwrap();
    assert_eq!(payload.len(), 1);
    assert_eq!(payload[0].2, "Rust 知识库");
    let entities = super::vector::entities_with_vector(&db.pool, "rDoc", super::vector::DOC)
        .await
        .unwrap();
    assert_eq!(entities.len(), 2, "two vectors for two chunks");
    let vectors = super::vector::vector_ids(&db.pool, "rDoc", super::vector::DOC)
        .await
        .unwrap();
    assert_eq!(vectors.len(), 2);

    let path = doc::delete(&db.pool, "rDoc", "doc1").await.unwrap();
    assert_eq!(path.as_deref(), Some("uploads/rDoc/doc1.docx"));
    let chunks_left: i64 = sqlx::query_scalar("SELECT count(*) FROM doc_chunks")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(chunks_left, 0);
    let vectors_left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM vector_items WHERE collection = 'doc'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(vectors_left, 0, "删文档不能留下孤儿向量");

    db.cleanup().await;
}

/// B-21/B-22：问答整体替换时，旧问题与它们的向量一起删、新的一起写。
#[tokio::test]
async fn qa_questions_and_vectors_are_replaced_together() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rQa", "qa")).await.unwrap();
    let model = test_model();

    let (qa_id, vec_ids) = qa::save(
        &db.pool,
        "rQa",
        None,
        "answer",
        &[q("hello", true, 0), q("hi", false, 1)],
        &model,
    )
    .await
    .unwrap()
    .expect("新建必须成功");
    assert_eq!(vec_ids.len(), 2);
    let detail = qa::get(&db.pool, "rQa", &qa_id).await.unwrap().unwrap();
    assert_eq!(detail.questions.len(), 2);
    assert!(detail.questions[0].is_primary, "主问题排第一");
    assert!(!detail.questions[1].is_primary);

    let (vid, found_qa_id) = qa::qa_ids_for_vectors(&db.pool, "rQa", &[vec_ids[0]])
        .await
        .unwrap()
        .remove(0);
    assert_eq!(vid, vec_ids[0]);
    assert_eq!(found_qa_id, qa_id);

    // 编辑：整体替换成一条问题，旧的两条问题行与向量行都必须消失。
    let (same_id, new_vecs) = qa::save(
        &db.pool,
        "rQa",
        Some(&qa_id),
        "answer2",
        &[q("hello again", true, 0)],
        &model,
    )
    .await
    .unwrap()
    .expect("编辑必须成功");
    assert_eq!(same_id, qa_id);
    assert_eq!(new_vecs.len(), 1);
    let detail = qa::get(&db.pool, "rQa", &qa_id).await.unwrap().unwrap();
    assert_eq!(detail.answer, "answer2");
    assert_eq!(detail.questions.len(), 1);
    let vec_left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM vector_items WHERE collection = 'qa'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(vec_left, 1, "旧问题的向量行必须被同事务删掉");

    assert!(qa::delete(&db.pool, "rQa", &qa_id).await.unwrap());
    assert!(qa::get(&db.pool, "rQa", &qa_id).await.unwrap().is_none());
    let vec_left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM vector_items WHERE collection = 'qa'")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert_eq!(vec_left, 0);

    db.cleanup().await;
}

/// B-23 / D47：重建写进 pending generation，续跑复用同一代，翻转 active 后旧代
/// 的向量行被清掉；元数据报告读出维度/模型而不扫 BLOB。
#[tokio::test]
async fn rebuild_flips_generation_and_drops_the_old_one() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rGen", "gen"))
        .await
        .unwrap();
    let model = test_model();
    let chunks = vec![chunk(0, "alpha", 0), chunk(1, "beta", 1)];
    let old_ids = doc::create(
        &db.pool,
        "rGen",
        "doc1",
        &doc::NewDocument {
            file_name: String::from("a.docx"),
            file_size: 1,
            mime_type: None,
            original_path: None,
            content_hash: None,
            doc_content: String::from("alpha\nbeta"),
        },
        &chunks,
        &model,
    )
    .await
    .unwrap();
    assert_eq!(
        super::vector::state(&db.pool, "rGen", super::vector::DOC)
            .await
            .unwrap()
            .map(|s| s.active_generation),
        Some(0)
    );

    let new_model = crate::repo::vector::ModelRef {
        provider: String::from("test"),
        model: String::from("m2"),
        dimension: 2,
        normalized: false,
        fingerprint: String::from("v1|test|m2|2"),
    };
    let g = super::vector::begin_rebuild(&db.pool, "rGen", super::vector::DOC, &new_model)
        .await
        .unwrap();
    assert_eq!(g, 1, "第一代之后是第二代");
    // 幂等续跑：重建中再次 begin 必须复用同一代，不能每点一次就 +1。
    let again = super::vector::begin_rebuild(&db.pool, "rGen", super::vector::DOC, &new_model)
        .await
        .unwrap();
    assert_eq!(again, g);

    // 待办查询按 generation 过滤：还没写过 gen 1 的文档就是待办。
    let pending = doc::pending_documents(&db.pool, "rGen", g).await.unwrap();
    assert_eq!(pending.len(), 1);
    doc::reindex_document(&db.pool, "rGen", "doc1", g, &chunks, &new_model)
        .await
        .unwrap();
    assert!(
        doc::pending_documents(&db.pool, "rGen", g)
            .await
            .unwrap()
            .is_empty(),
        "写过 gen 1 之后不再是待办（续跑靠这个跳过）"
    );

    // 翻转前查询仍然只认 active generation（第 0 代）。文档重建是整篇替换，
    // 旧分块已删，所以这一代此刻是空的 —— 这是"单篇原子"的已知窗口。
    let active = super::vector::load_active(&db.pool, "rGen", super::vector::DOC)
        .await
        .unwrap();
    assert!(active.is_empty(), "翻转前 active 还是第 0 代");

    super::vector::finish_rebuild(&db.pool, "rGen", super::vector::DOC, g)
        .await
        .unwrap();
    let st = super::vector::state(&db.pool, "rGen", super::vector::DOC)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(st.active_generation, 1);
    assert_eq!(st.pending_generation, None);
    assert_eq!(st.status, "idle");

    let gens: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT generation FROM vector_items
          WHERE robot_id = 'rGen' AND collection = 'doc'",
    )
    .fetch_all(&db.pool)
    .await
    .unwrap();
    assert_eq!(gens, vec![1], "旧 generation 的向量行必须被清掉");

    let stats = super::vector::collection_stats(&db.pool, "rGen", super::vector::DOC)
        .await
        .unwrap();
    assert_eq!(stats.rows, 2);
    assert_eq!(stats.dims, vec![2], "维度来自 embedding_models，不扫 BLOB");
    assert_eq!(stats.active_model.as_deref(), Some("v1|test|m2|2"));
    assert_eq!(stats.status, "idle");
    let _ = old_ids;

    db.cleanup().await;
}

/// B-22：换维度但还没重建时，写入点直接拒绝，避免一个 generation 里混两种维度
/// 把整个索引拖垮（错误文案要能指导用户去重建）。
#[tokio::test]
async fn vector_dimension_change_is_rejected_at_write_time() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rDim", "dim"))
        .await
        .unwrap();
    let model = test_model();
    let mut tx = db.pool.begin().await.unwrap();
    super::vector::put_many_active(
        &mut tx,
        "rDim",
        super::vector::PHRASE,
        &model,
        &[(String::from("p1"), unit_vec(2, 0))],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let model3 = crate::repo::vector::ModelRef {
        provider: String::from("test"),
        model: String::from("m3"),
        dimension: 3,
        normalized: false,
        fingerprint: String::from("v1|test|m3|3"),
    };
    let mut tx = db.pool.begin().await.unwrap();
    let e = super::vector::put_many_active(
        &mut tx,
        "rDim",
        super::vector::PHRASE,
        &model3,
        &[(String::from("p2"), vec![1.0, 0.0, 0.0])],
    )
    .await
    .unwrap_err();
    assert!(
        e.message().contains("dimension changed"),
        "错误要说清楚是维度变了：{}",
        e.message()
    );
    drop(tx);

    // 空集合的报告形状：exists=false（前端据此显示"未建"）。
    let stats = super::vector::collection_stats(&db.pool, "rDim", super::vector::QA)
        .await
        .unwrap();
    assert_eq!(stats.rows, 0);
    assert!(stats.dims.is_empty());
    assert_eq!(stats.status, "idle");

    db.cleanup().await;
}

/// B-28：`dirty` 是"内存索引可能与 PG 不一致"的持久标记（migration `0005`），
/// 状态查询与报告都要能读到它。
#[tokio::test]
async fn vector_state_dirty_flag_roundtrips() {
    let Some(db) = TestDb::new().await else {
        eprintln!("skipped: KIKO_TEST_DATABASE_URL is not set");
        return;
    };
    robot::upsert(&db.pool, &robot("rDirty", "dirty"))
        .await
        .unwrap();
    let mut tx = db.pool.begin().await.unwrap();
    super::vector::put_many_active(
        &mut tx,
        "rDirty",
        super::vector::PHRASE,
        &test_model(),
        &[(String::from("p1"), unit_vec(2, 0))],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let st = super::vector::state(&db.pool, "rDirty", super::vector::PHRASE)
        .await
        .unwrap()
        .unwrap();
    assert!(!st.dirty, "新写的集合默认不是 dirty");

    super::vector::set_dirty(&db.pool, "rDirty", super::vector::PHRASE, true)
        .await
        .unwrap();
    let stats = super::vector::collection_stats(&db.pool, "rDirty", super::vector::PHRASE)
        .await
        .unwrap();
    assert!(stats.dirty, "报告要能暴露 dirty");
    assert!(stats.rows >= 1);

    super::vector::set_dirty(&db.pool, "rDirty", super::vector::PHRASE, false)
        .await
        .unwrap();
    assert!(
        !super::vector::state(&db.pool, "rDirty", super::vector::PHRASE)
            .await
            .unwrap()
            .unwrap()
            .dirty
    );

    // migration 0005 的列必须真的在（migration 是事实，文档是意图）。
    let column: Option<String> = sqlx::query_scalar(
        "SELECT column_name::text FROM information_schema.columns
          WHERE table_schema = $1 AND table_name = 'vector_collection_state'
            AND column_name = 'dirty'",
    )
    .bind(&db.schema)
    .fetch_optional(&db.pool)
    .await
    .unwrap();
    assert_eq!(column.as_deref(), Some("dirty"));

    db.cleanup().await;
}
