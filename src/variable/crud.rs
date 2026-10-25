use std::collections::HashMap;
// use std::borrow::BorrowMut;
use std::vec::Vec;

use axum::Json;
use axum::extract::Query;
use axum::response::IntoResponse;
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

use super::dto::Variable;
use super::dto::{VariableObtainValueExpressionType, VariableType, VariableValueSource};
use crate::flow::rt::context::Context;
use crate::flow::rt::dto::Request;
use crate::repo;
use crate::result::{Error, Result};
use crate::web::server::to_res;

pub(crate) async fn init(robot_id: &str, is_en: bool) -> Result<()> {
    let v = Variable {
        // Kept in canonical form so the seeded variable is reachable by the
        // same name the flow text has to use.
        var_name: sanitize_var_name(if is_en {
            "CollectionVar"
        } else {
            "采集变量"
        }),
        var_type: VariableType::Str,
        var_val_source: VariableValueSource::Collect,
        var_constant_value: String::new(),
        var_associate_data: String::new(),
        obtain_value_expression_type: VariableObtainValueExpressionType::None,
        obtain_value_expression: String::new(),
        connect_timeout_milliseconds: 1000u64,
        read_timeout_milliseconds: 1500u64,
        cache_enabled: true,
        version: None,
    };
    // 变量表已经在 PG（S3.1），这里只写这一条种子变量。
    repo::variable::upsert(repo::pool(), robot_id, &v).await
}

pub(crate) async fn list(Query(q): Query<HashMap<String, String>>) -> impl IntoResponse {
    if let Some(robot_id) = q.get("robotId") {
        let r: Result<Vec<Variable>> = repo::variable::list(repo::pool(), robot_id).await;
        to_res(r)
    } else {
        to_res(Err(Error::WithMessage(String::from(
            "Parameter: robotId is missing.",
        ))))
    }
}

/// Canonical form of a variable name, used for the PG primary key and every
/// lookup, so a name means the same thing wherever it appears.
///
/// - NFKC first, so decomposed input (`n` + U+0303) and compatibility forms
///   (full-width `１２３`, half-width kana) collapse to their canonical composed
///   form and survive the filter below instead of being lost;
/// - case folded (full, non-Turkic) rather than merely lower-cased, so case is
///   ignored *and* `ß`/`ss` cannot both exist as separate variables;
/// - everything that is not a letter, digit or `_` is dropped: spaces,
///   `~!@#`, punctuation, and joiners such as ZWNJ/ZWJ.
pub(crate) fn sanitize_var_name(name: &str) -> String {
    name.nfkc()
        .flat_map(char::case_fold)
        .filter(|c| c.is_alphanumeric() || *c == '_')
        .collect()
}

pub(crate) async fn add(
    Query(q): Query<HashMap<String, String>>,
    Json(mut v): Json<Variable>,
) -> impl IntoResponse {
    v.var_name = sanitize_var_name(&v.var_name);
    if v.var_name.is_empty() {
        return to_res::<()>(Err(Error::WithMessage(String::from(
            "Parameter: varName is invalid.",
        ))));
    }

    if let Some(robot_id) = q.get("robotId") {
        // 新增走 insert/upsert；已存在的同名变量必须带 version（B-17 强制模式）。
        let r: Result<()> = async {
            let existing = repo::variable::get(repo::pool(), robot_id, &v.var_name).await?;
            if existing.is_some() {
                let Some(version) = v.version else {
                    return Err(repo::version_required("Variable"));
                };
                if !repo::variable::update(repo::pool(), robot_id, &v, version).await? {
                    return Err(Error::WithMessage(format!(
                        "Variable {} does not exist any more.",
                        v.var_name
                    )));
                }
            } else {
                repo::variable::upsert(repo::pool(), robot_id, &v).await?;
            }
            Ok(())
        }
        .await;
        to_res(r)
    } else {
        to_res(Err(Error::WithMessage(String::from(
            "Parameter: robotId is missing.",
        ))))
    }
}

pub(crate) async fn delete(
    Query(q): Query<HashMap<String, String>>,
    Json(v): Json<Variable>,
) -> impl IntoResponse {
    let name = sanitize_var_name(&v.var_name);
    if let Some(robot_id) = q.get("robotId") {
        let r = repo::variable::delete(repo::pool(), robot_id, &name).await;
        to_res(r)
    } else {
        to_res(Err(Error::WithMessage(String::from(
            "Parameter: robotId is missing.",
        ))))
    }
}

pub(crate) async fn get(robot_id: &str, name: &str) -> Result<Option<Variable>> {
    let name = sanitize_var_name(name);
    repo::variable::get(repo::pool(), robot_id, &name).await
}

pub(crate) async fn get_value(name: &str, req: &Request, ctx: &mut Context) -> String {
    if let Ok(Some(v)) = get(&req.robot_id, name).await {
        if let Some(val) = v.get_value2(req, ctx).await {
            return val.val_to_string();
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::sanitize_var_name;

    #[test]
    fn strips_spaces_and_symbols() {
        assert_eq!(sanitize_var_name(" my var~!@# "), "myvar");
        assert_eq!(sanitize_var_name("my-var.2"), "myvar2");
        assert_eq!(sanitize_var_name("my_var"), "my_var");
    }

    #[test]
    fn keeps_non_latin_scripts() {
        assert_eq!(sanitize_var_name("采集 变量"), "采集变量");
        assert_eq!(sanitize_var_name("año_niño"), "año_niño");
        assert_eq!(sanitize_var_name("محمد"), "محمد");
        assert_eq!(sanitize_var_name("変換する"), "変換する");
        assert_eq!(sanitize_var_name("안녕하세요"), "안녕하세요");
    }

    #[test]
    fn folds_decomposed_and_compatibility_forms() {
        // `n` + COMBINING TILDE and `か` + COMBINING DAKUTEN survive as `ñ` / `が`.
        assert_eq!(sanitize_var_name("an\u{0303}o"), "año");
        assert_eq!(sanitize_var_name("\u{304b}\u{3099}"), "が");
        // Full-width digits and letters fold to ASCII, so they cannot collide
        // with a visually identical ASCII name as a separate record.
        assert_eq!(sanitize_var_name("１２３"), "123");
        assert_eq!(sanitize_var_name("ＡＢＣ"), "abc");
    }

    #[test]
    fn drops_joiners() {
        // ZWNJ is a default-ignorable, stripped like any other symbol.
        assert_eq!(sanitize_var_name("می\u{200c}رود"), "میرود");
    }

    #[test]
    fn case_is_ignored() {
        assert_eq!(sanitize_var_name("Name"), "name");
        assert_eq!(sanitize_var_name("NAME"), "name");
        assert_eq!(sanitize_var_name("MyVar"), sanitize_var_name("myvar"));
        // Dotted capital I lower-cases to `i` + COMBINING DOT ABOVE, which the
        // filter then drops, so it lands on plain `i`.
        assert_eq!(sanitize_var_name("\u{130}stanbul"), "istanbul");
        // Full case folding, so `ß` expands to `ss` and the two German
        // spellings are one variable rather than two.
        assert_eq!(sanitize_var_name("straße"), "strasse");
        assert_eq!(sanitize_var_name("Straße"), sanitize_var_name("STRASSE"));
    }

    #[test]
    fn empty_when_nothing_remains() {
        assert_eq!(sanitize_var_name("~!@# "), "");
    }

    /// Documents what NFKC does *not* fold, so the behaviour is not a surprise.
    #[test]
    fn known_limitations() {
        // Arabic tatweel is a letter (Lm), so it survives and a name can be
        // visually confused with the plain spelling.
        assert_eq!(sanitize_var_name("متـــغير"), "متـــغير");
        assert_ne!(sanitize_var_name("متـــغير"), sanitize_var_name("متغير"));
        // Simplified and traditional forms are intentionally not merged.
        assert_ne!(sanitize_var_name("变量"), sanitize_var_name("變量"));
    }
}
