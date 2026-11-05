//! 变量名归一化：显示名（`var_name`）与内部 code（`var_unified_name`）。
//!
//! 允许的字符与 `^[\p{L}\p{M}\p{Nd}-_]+$` 一致：Unicode 字母、组合标记、十进制
//! 数字，外加 `-` 与 `_`；其余字符一律丢弃（沿用旧行为，只是不再只认 ASCII）。
//!
//! * 显示名保留大小写，只做 NFKC，所以 `My-Var` 仍是 `My-Var`；
//! * 内部 code 先 NFKC、再全量 case fold、再 NFKC，所以 `My-Var` 的 code 是
//!   `my-var`，流程表达式、会话缓存和 SQL 查询都用它。

use std::sync::OnceLock;

use regex::Regex;
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

/// 一个或多个允许字符。用 `find_iter` 过滤掉所有不在字符类里的字符。
fn allowed_chars() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"[\p{L}\p{M}\p{Nd}_-]+").expect("variable-name character class is valid")
    })
}

fn keep_allowed(s: &str) -> String {
    allowed_chars().find_iter(s).map(|m| m.as_str()).collect()
}

/// 显示名：NFKC 归一化后只保留允许字符，大小写原样。
pub(crate) fn filter_var_name(name: &str) -> String {
    let normalized: String = name.nfkc().collect();
    keep_allowed(&normalized)
}

/// 内部 code：NFKC → 全量 case fold → 再 NFKC，然后只保留允许字符。
///
/// 第二次 NFKC 让 case fold 拆出来的组合序列尽量重新合成；拆不掉但属于
/// `\p{M}` 的标记会保留，因为变量名允许组合标记。
pub(crate) fn unify_var_name(name: &str) -> String {
    let folded: String = name.nfkc().flat_map(char::case_fold).nfkc().collect();
    keep_allowed(&folded)
}

#[cfg(test)]
mod tests {
    use super::{filter_var_name, unify_var_name};

    #[test]
    fn strips_spaces_and_symbols() {
        assert_eq!(filter_var_name(" my var~!@# "), "myvar");
        assert_eq!(filter_var_name("my-var.2"), "my-var2");
        assert_eq!(filter_var_name("my_var"), "my_var");
    }

    #[test]
    fn keeps_non_latin_scripts_and_marks() {
        assert_eq!(filter_var_name("采集 变量"), "采集变量");
        assert_eq!(filter_var_name("año_niño"), "año_niño");
        assert_eq!(filter_var_name("محمد"), "محمد");
        assert_eq!(filter_var_name("変換する"), "変換する");
        assert_eq!(filter_var_name("안녕하세요"), "안녕하세요");
        // 天城文里组合标记（Mn）必须保留，不能被当成符号丢掉。
        assert_eq!(filter_var_name("नमस्ते"), "नमस्ते");
    }

    #[test]
    fn display_name_keeps_case() {
        assert_eq!(filter_var_name("MyVar"), "MyVar");
        assert_eq!(filter_var_name("My-Var_2"), "My-Var_2");
        assert_eq!(unify_var_name("MyVar"), "myvar");
        assert_eq!(unify_var_name("My-Var_2"), "my-var_2");
    }

    #[test]
    fn folds_decomposed_and_compatibility_forms() {
        // `n` + COMBINING TILDE and `か` + COMBINING DAKUTEN survive as `ñ` / `が`.
        assert_eq!(filter_var_name("an\u{0303}o"), "año");
        assert_eq!(unify_var_name("an\u{0303}o"), "año");
        assert_eq!(filter_var_name("\u{304b}\u{3099}"), "が");
        // Full-width digits and letters fold to ASCII.
        assert_eq!(filter_var_name("１２３"), "123");
        assert_eq!(filter_var_name("ＡＢＣ"), "ABC");
        assert_eq!(unify_var_name("ＡＢＣ"), "abc");
    }

    #[test]
    fn drops_joiners() {
        // ZWNJ is a default-ignorable (Cf), stripped like any other symbol.
        assert_eq!(filter_var_name("می\u{200c}رود"), "میرود");
    }

    #[test]
    fn code_ignores_case() {
        assert_eq!(unify_var_name("Name"), "name");
        assert_eq!(unify_var_name("NAME"), "name");
        assert_eq!(unify_var_name("MyVar"), unify_var_name("myvar"));
        // Full case folding, so `ß` expands to `ss`.
        assert_eq!(unify_var_name("straße"), "strasse");
        assert_eq!(unify_var_name("Straße"), unify_var_name("STRASSE"));
    }

    #[test]
    fn hyphen_and_underscore_are_distinct_codes() {
        assert_ne!(unify_var_name("my-var"), unify_var_name("my_var"));
    }

    #[test]
    fn empty_when_nothing_remains() {
        assert_eq!(filter_var_name("~!@# "), "");
        assert_eq!(unify_var_name("~!@# "), "");
    }

    /// Documents what NFKC does *not* fold, so the behaviour is not a surprise.
    #[test]
    fn known_limitations() {
        // Arabic tatweel is a letter (Lm), so it survives and a name can be
        // visually confused with the plain spelling.
        assert_eq!(filter_var_name("متـــغير"), "متـــغير");
        assert_ne!(unify_var_name("متـــغير"), unify_var_name("متغير"));
        // Simplified and traditional forms are intentionally not merged.
        assert_ne!(unify_var_name("变量"), unify_var_name("變量"));
    }
}
