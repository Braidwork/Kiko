// use std::borrow::Cow;
use std::vec::Vec;

use serde::{Deserialize, Serialize};

use crate::flow::rt::context::Context;
use crate::flow::rt::dto::Request;

use super::unify_var_name;

#[derive(Deserialize, Serialize)]
pub(crate) struct SimpleVariable {
    #[serde(rename = "varName")]
    pub(crate) var_name: String,
    #[serde(rename = "varType")]
    pub(crate) var_type: VariableType,
    #[serde(rename = "varVal")]
    pub(crate) var_val: String,
}

#[derive(Clone, Deserialize, Serialize)]
pub(crate) struct Variable {
    // #[serde(rename(deserialize = "varName"))]
    #[serde(rename = "varName")]
    pub(crate) var_name: String,
    /// 归一化后的内部 code。服务端在新增/更新时重算，不信任客户端传值。
    #[serde(rename = "varUnifiedName", default)]
    pub(crate) var_unified_name: String,
    #[serde(rename = "varType")]
    pub(crate) var_type: VariableType,
    #[serde(rename = "varValueSource")]
    pub(crate) var_val_source: VariableValueSource,
    #[serde(rename = "varAssociateData")]
    pub(crate) var_associate_data: String,
    #[serde(rename = "obtainValueExpressionType")]
    pub(crate) obtain_value_expression_type: VariableObtainValueExpressionType,
    #[serde(rename = "obtainValueExpression")]
    pub(crate) obtain_value_expression: String,
    #[serde(rename = "connectTimeoutMilliseconds")]
    pub(crate) connect_timeout_milliseconds: u64,
    #[serde(rename = "readTimeoutMilliseconds")]
    pub(crate) read_timeout_milliseconds: u64,
    /// 取值缓存策略（0=不缓存，1=按请求，2=按会话）。变量跨机器人共享。
    #[serde(rename = "valCacheType", default)]
    pub(crate) val_cache_type: VariableValueCacheType,
    /// 乐观锁版本（B-17）：读接口返回，编辑时原样带回。
    #[serde(default)]
    pub(crate) version: Option<i64>,
}

/// `variables.val_cache_type` 的值。JSON 里是数字，与数据库的 SMALLINT 一致。
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(try_from = "u8", into = "u8")]
pub(crate) enum VariableValueCacheType {
    /// 0：每次取值都重新计算/请求。
    NoCache,
    /// 1：只在当前请求内缓存（`Context::none_persistent_vars`）。
    Request,
    /// 2：随会话持久化（`Context::vars`，写进 `contexts.data`）。
    #[default]
    Session,
}

impl TryFrom<u8> for VariableValueCacheType {
    type Error = String;

    fn try_from(v: u8) -> std::result::Result<Self, Self::Error> {
        match v {
            0 => Ok(Self::NoCache),
            1 => Ok(Self::Request),
            2 => Ok(Self::Session),
            other => Err(format!("valCacheType must be 0, 1 or 2, got {other}")),
        }
    }
}

impl From<VariableValueCacheType> for u8 {
    fn from(t: VariableValueCacheType) -> Self {
        match t {
            VariableValueCacheType::NoCache => 0,
            VariableValueCacheType::Request => 1,
            VariableValueCacheType::Session => 2,
        }
    }
}

impl Variable {
    fn get_data_from_scraper(&self, s: &str) -> Option<String> {
        let parts = &self
            .obtain_value_expression
            .split_whitespace()
            .collect::<Vec<&str>>();
        let mut selectors = Vec::with_capacity(parts.len());
        for part in parts {
            if let Ok(selector) = scraper::Selector::parse(part) {
                selectors.push(selector);
            } else {
                return None;
            }
        }
        let doc = scraper::Html::parse_document(s);
        let mut ele_ref_op = doc.select(&selectors.pop().unwrap()).next();
        for sel in selectors.iter() {
            if let Some(ele_ref) = ele_ref_op {
                ele_ref_op = ele_ref.select(sel).next();
            } else {
                return None;
            }
        }
        if let Some(ele_ref) = ele_ref_op {
            if let Some(res) = ele_ref.text().next() {
                return Some(String::from(res));
            }
        }
        None
    }
    /// 把 HTTP 响应按表达式取出值，并按 `val_cache_type` 决定写到哪个缓存。
    fn get_data_from_res(&self, ctx: &mut Context, s: &str) -> Option<VariableValue> {
        let str_store: Option<String>;
        let r = match self.obtain_value_expression_type {
            VariableObtainValueExpressionType::JsonPointer => {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(s) {
                    if let Some(r) = v.pointer(&self.obtain_value_expression) {
                        if r.is_string() {
                            str_store = Some(String::from(r.as_str().unwrap()));
                            str_store.as_ref().unwrap()
                        } else {
                            s
                        }
                    } else {
                        s
                    }
                } else {
                    s
                }
            }
            VariableObtainValueExpressionType::HtmlScrape => {
                str_store = self.get_data_from_scraper(s);
                str_store.as_deref().unwrap_or(s)
            }
            VariableObtainValueExpressionType::JavaScript => s,
            VariableObtainValueExpressionType::None => s,
        };
        let v = VariableValue::new(r, &self.var_type);
        Some(self.store_value(ctx, v))
    }

    /// 内部 code：会话缓存和流程表达式都用这个值，不直接用显示名。
    fn unified_name(&self) -> String {
        if self.var_unified_name.is_empty() {
            unify_var_name(&self.var_name)
        } else {
            self.var_unified_name.clone()
        }
    }

    /// 只在缓存策略允许的 map 里取值。
    fn cached_value(&self, ctx: &Context) -> Option<VariableValue> {
        let key = self.unified_name();
        match self.val_cache_type {
            VariableValueCacheType::NoCache => None,
            VariableValueCacheType::Request => ctx.none_persistent_vars.get(&key).cloned(),
            VariableValueCacheType::Session => ctx.vars.get(&key).cloned(),
        }
    }

    /// 按缓存策略写入，并返回同一个值（`NoCache` 只返回、不写）。
    fn store_value(&self, ctx: &mut Context, v: VariableValue) -> VariableValue {
        let key = self.unified_name();
        match self.val_cache_type {
            VariableValueCacheType::NoCache => {}
            VariableValueCacheType::Request => {
                ctx.none_persistent_vars.insert(key, v.clone());
            }
            VariableValueCacheType::Session => {
                ctx.vars.insert(key, v.clone());
            }
        }
        v
    }
    pub(crate) async fn get_value2(
        &self,
        req: &Request,
        ctx: &mut Context,
    ) -> Option<VariableValue> {
        if let Some(v) = self.cached_value(ctx) {
            return Some(v);
        }
        self.get_value_inner2(req, ctx).await
    }

    async fn get_value_inner2(&self, req: &Request, ctx: &mut Context) -> Option<VariableValue> {
        match &self.var_val_source {
            // Collect/Import 的值由运行时节点写入：Session 在 ctx.vars，Request/NoCache
            // 在 ctx.none_persistent_vars（NoCache 也要在本轮内可读）。
            VariableValueSource::Collect | VariableValueSource::Import => {
                let key = self.unified_name();
                ctx.none_persistent_vars
                    .get(&key)
                    .or_else(|| ctx.vars.get(&key))
                    .cloned()
            }
            VariableValueSource::UserInput => {
                let v = VariableValue::new(&req.user_input, &self.var_type);
                Some(self.store_value(ctx, v))
            }
            VariableValueSource::ExternalHttp => {
                // 原始响应缓存只在请求内有效；NoCache 连它也不用，确保每次都发请求。
                let cache = if self.val_cache_type == VariableValueCacheType::NoCache {
                    None
                } else {
                    ctx.none_persistent_data
                        .get(&self.var_associate_data)
                        .cloned()
                };
                if let Some(c) = cache {
                    return self.get_data_from_res(ctx, &c);
                }
                if let Ok(Some(api)) =
                    crate::external::http::crud::get_detail(&self.var_associate_data).await
                {
                    return match crate::external::http::client::req(
                        api,
                        self.connect_timeout_milliseconds,
                        self.read_timeout_milliseconds,
                        &ctx.substitution_vars(),
                    )
                    .await
                    {
                        Ok(r) => match r {
                            crate::external::http::dto::ResponseData::Str(s) => {
                                if self.val_cache_type != VariableValueCacheType::NoCache {
                                    ctx.none_persistent_data
                                        .insert(self.var_associate_data.clone(), s.clone());
                                }
                                self.get_data_from_res(ctx, &s)
                            }
                            _ => None,
                        },
                        Err(e) => {
                            log::error!("{e:?}");
                            None
                        }
                    };
                }
                None
            }
        }
    }
}
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) enum VariableValue {
    Str(String),
    Num(f64),
    Array(Vec<VariableValue>),
}

impl VariableValue {
    pub(crate) fn new(v: &str, t: &VariableType) -> Self {
        match t {
            VariableType::Str => VariableValue::Str(String::from(v)),
            VariableType::Num => VariableValue::Num(v.parse::<f64>().unwrap_or(0f64)),
        }
    }
    // pub(crate) fn val_to_string(&self) -> Cow<'_, str> {
    pub(crate) fn val_to_string(&self) -> String {
        match self {
            VariableValue::Str(s) => String::from(s),
            VariableValue::Num(n) => n.to_string(),
            VariableValue::Array(arr) => {
                let mut s = String::with_capacity(512);
                s.push('[');
                for item in arr.iter() {
                    s.push_str(&item.val_to_string());
                    s.push(',');
                }
                s.pop();
                s.push(']');
                s
            }
        }
        // Cow::Owned(s)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) enum VariableType {
    Str,
    Num,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) enum VariableValueSource {
    Import,
    Collect,
    UserInput,
    ExternalHttp,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub(crate) enum VariableObtainValueExpressionType {
    None,
    JsonPointer,
    HtmlScrape,
    JavaScript,
}

#[cfg(test)]
mod tests {
    use super::VariableValueCacheType;

    #[test]
    fn cache_type_is_a_number_on_the_wire() {
        assert_eq!(
            serde_json::to_value(VariableValueCacheType::NoCache).unwrap(),
            serde_json::json!(0)
        );
        assert_eq!(
            serde_json::to_value(VariableValueCacheType::Request).unwrap(),
            serde_json::json!(1)
        );
        assert_eq!(
            serde_json::to_value(VariableValueCacheType::Session).unwrap(),
            serde_json::json!(2)
        );
        let back: VariableValueCacheType = serde_json::from_value(serde_json::json!(1)).unwrap();
        assert_eq!(back, VariableValueCacheType::Request);
        // Values outside the CHECK constraint are rejected at the API edge too.
        assert!(serde_json::from_value::<VariableValueCacheType>(serde_json::json!(3)).is_err());
    }
}
