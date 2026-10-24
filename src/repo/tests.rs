//! 存储路径的集成测试（§7 测试约定）。
//!
//! 每个用例自建 `tmp_<uuid>` schema、跑 migration、结束时 drop，不污染共享库。
//! 没配 `KIKO_TEST_DATABASE_URL` 时整组跳过 —— 没有 PG 的环境不该因此红一片。

use super::{robot, test_support::TestDb, variable};
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
