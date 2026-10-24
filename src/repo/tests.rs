//! 存储路径的集成测试（§7 测试约定）。
//!
//! 每个用例自建 `tmp_<uuid>` schema、跑 migration、结束时 drop，不污染共享库。
//! 没配 `KIKO_TEST_DATABASE_URL` 时整组跳过 —— 没有 PG 的环境不该因此红一片。

use super::{
    context, external_http, intent, mainflow, robot, runtime_node, subflow, test_support::TestDb,
    variable,
};
use crate::external::http::dto::{
    HttpReqInfo, HttpReqParam, Method, PostContentType, Protocol, ValueSource,
};
use crate::robot::dto::{RobotData, RobotType};
use crate::variable::dto::{
    Variable, VariableObtainValueExpressionType, VariableType, VariableValueSource,
};

fn robot(id: &str, name: &str) -> RobotData {
    RobotData {
        robot_id: String::from(id),
        robot_name: String::from(name),
        robot_type: RobotType::TextBot,
    }
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
    let p1 = intent::add_phrase(&db.pool, "rI", &alpha, "hello", 101)
        .await
        .unwrap()
        .expect("意图存在时必须返回新短语 id");

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

    // 短语的向量行号是内部列，但删除短语需要它。
    let phrase = intent::get_phrase(&db.pool, "rI", &alpha, &p1)
        .await
        .unwrap()
        .expect("p1 应该存在");
    assert_eq!(phrase.vec_row_id, Some(101));
    assert_eq!(phrase.phrase, "hello");
    assert!(
        intent::delete_phrase(&db.pool, "rI", &alpha, &p1)
            .await
            .unwrap()
    );
    assert!(
        intent::get_phrase(&db.pool, "rI", &alpha, &p1)
            .await
            .unwrap()
            .is_none()
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
    let phrase = intent::add_phrase(&db.pool, "rA", &id, "p", 7)
        .await
        .unwrap()
        .unwrap();

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
        intent::add_phrase(&db.pool, "rB", &id, "stolen", 8)
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
        !intent::delete_phrase(&db.pool, "rB", &id, &phrase)
            .await
            .unwrap()
    );
    assert!(!intent::delete(&db.pool, "rB", &id).await.unwrap());
    assert!(
        intent::get_meta(&db.pool, "rB", &id)
            .await
            .unwrap()
            .is_none()
    );

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
    intent::add_phrase(&db.pool, "rD", &id, "d", 9)
        .await
        .unwrap()
        .unwrap();

    assert!(robot::delete(&db.pool, "rD").await.unwrap());
    for table in [
        "intents",
        "intent_keywords",
        "intent_regexes",
        "intent_phrases",
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
