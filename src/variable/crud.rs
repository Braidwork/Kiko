use std::vec::Vec;

use axum::Json;
use axum::response::IntoResponse;

use super::dto::{
    Variable, VariableObtainValueExpressionType, VariableType, VariableValue,
    VariableValueCacheType, VariableValueSource,
};
use super::{filter_var_name, unify_var_name};
use crate::flow::rt::context::Context;
use crate::flow::rt::dto::Request;
use crate::repo;
use crate::result::{Error, Result};
use crate::web::server::to_res;

pub(crate) async fn init(is_en: bool) -> Result<()> {
    let var_name = filter_var_name(if is_en {
        "CollectionVar"
    } else {
        "采集变量"
    });
    let v = Variable {
        // Kept in canonical form so the seeded variable is reachable by the
        // same name the flow text has to use.
        var_unified_name: unify_var_name(&var_name),
        var_name,
        var_type: VariableType::Str,
        var_val_source: VariableValueSource::Collect,
        var_associate_data: String::new(),
        obtain_value_expression_type: VariableObtainValueExpressionType::None,
        obtain_value_expression: String::new(),
        connect_timeout_milliseconds: 1000u64,
        read_timeout_milliseconds: 1500u64,
        val_cache_type: VariableValueCacheType::Session,
        version: None,
    };
    // 变量跨机器人共享：只在缺失时种这一条，之后新建机器人不能覆盖用户改过的值。
    if repo::variable::get(repo::pool(), &v.var_unified_name)
        .await?
        .is_none()
    {
        repo::variable::upsert(repo::pool(), &v).await?;
    }
    Ok(())
}

pub(crate) async fn list() -> impl IntoResponse {
    let r: Result<Vec<Variable>> = repo::variable::list(repo::pool()).await;
    to_res(r)
}

pub(crate) async fn add(Json(mut v): Json<Variable>) -> impl IntoResponse {
    // 显示名保留大小写，只过滤字符；内部 code 由服务端重算。
    v.var_name = filter_var_name(&v.var_name);
    if v.var_name.is_empty() {
        return to_res::<()>(Err(Error::WithMessage(String::from(
            "Parameter: varName is invalid.",
        ))));
    }
    v.var_unified_name = unify_var_name(&v.var_name);

    // 新增走 insert/upsert；已存在的同名变量必须带 version（B-17 强制模式）。
    let r: Result<()> = async {
        let existing = repo::variable::get(repo::pool(), &v.var_unified_name).await?;
        if existing.is_some() {
            let Some(version) = v.version else {
                return Err(repo::version_required("Variable"));
            };
            if !repo::variable::update(repo::pool(), &v, version).await? {
                return Err(Error::WithMessage(format!(
                    "Variable {} does not exist any more.",
                    v.var_name
                )));
            }
        } else {
            repo::variable::upsert(repo::pool(), &v).await?;
        }
        Ok(())
    }
    .await;
    to_res(r)
}

pub(crate) async fn delete(Json(v): Json<Variable>) -> impl IntoResponse {
    let unified = if v.var_unified_name.is_empty() {
        unify_var_name(&v.var_name)
    } else {
        v.var_unified_name
    };
    let r = repo::variable::delete(repo::pool(), &unified).await;
    to_res(r)
}

pub(crate) async fn get(name: &str) -> Result<Option<Variable>> {
    repo::variable::get(repo::pool(), name).await
}

pub(crate) async fn get_value(name: &str, req: &Request, ctx: &mut Context) -> String {
    if let Ok(Some(v)) = get(name).await {
        if let Some(val) = v.get_value2(req, ctx).await {
            return val.val_to_string();
        }
    }
    String::new()
}

/// 运行时节点（Collect / Import）写入一个变量值。
///
/// 变量存在时按它的 `val_cache_type` 选缓存：会话缓存写 `ctx.vars`（随 `contexts`
/// 持久化），按请求缓存写 `ctx.none_persistent_vars`。不缓存的值也必须在本轮内
/// 可读，所以同样放请求级 map。变量不存在时沿用旧行为写会话缓存。
pub(crate) async fn set_value(name: &str, value: VariableValue, ctx: &mut Context) {
    let unified = unify_var_name(name);
    let cache_type = match get(name).await {
        Ok(Some(v)) => v.val_cache_type,
        _ => VariableValueCacheType::Session,
    };
    match cache_type {
        VariableValueCacheType::Session => {
            ctx.vars.insert(unified, value);
        }
        VariableValueCacheType::Request | VariableValueCacheType::NoCache => {
            ctx.none_persistent_vars.insert(unified, value);
        }
    }
}
