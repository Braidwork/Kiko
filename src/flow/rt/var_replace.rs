//! Variable substitution helpers shared by all runtime node flavours.
//!
//! Two notations are recognised:
//! - `` `varName` `` — the legacy plain-text notation;
//! - `<var data-var-name="varName">label</var>` — the notation emitted by the
//!   rich-text editor, where the variable is an atomic inline node. The body
//!   (label) is skipped entirely, the value is taken from `data-var-name`.
//!
//! [`rich_text_body_to_plain`] additionally turns a rich-text *request body*
//! back into the plain text that goes on the wire, chips and all.

use crate::flow::rt::dto::AnswerContentType;
use crate::result::Result;

const VAR_WRAP_SYMBOL: char = '`';

const VAR_TAG_PREFIX: &str = "<var ";
const VAR_TAG_END: &str = "</var>";
const VAR_NAME_ATTR: &str = "data-var-name";

/// Find `attr="value"` inside an opening `<var ...>` tag body.
/// Returns `(value_start, value_end)` offsets into `tag`, relative to `from`.
fn find_attr_value(tag: &str, from: usize, attr: &str) -> Option<(usize, usize)> {
    let pat = format!("{}=\"", attr);
    let start = tag[from..].find(&pat)? + from + pat.len();
    let rest = &tag[start..];
    if rest.starts_with('"') {
        return None;
    }
    let end = rest.find('"')? + start;
    if end == start {
        return None;
    }
    Some((start, end))
}

/// Replace variables wrapped by `VAR_WRAP_SYMBOL`.
fn replace_wrapped_vars<F>(text: &str, out: &mut String, mut start: usize, resolve: &F) -> usize
where
    F: Fn(&str) -> Result<Option<String>>,
{
    while let Some(rel) = text[start..].find(VAR_WRAP_SYMBOL) {
        let begin = start + rel;
        out.push_str(&text[start..begin]);
        if let Some(rel) = text[begin + 1..].find(VAR_WRAP_SYMBOL) {
            let end = begin + rel + 1;
            let name = &text[begin + 1..end];
            match resolve(name) {
                Ok(Some(value)) => out.push_str(&value),
                Ok(None) => out.push_str(&text[begin..=end]),
                Err(e) => return_log_and_keep(out, &text[begin..=end], e),
            }
            start = end + 1;
        } else {
            // Unpaired opening symbol: keep it literally.
            out.push(VAR_WRAP_SYMBOL);
            start = begin + 1;
        }
    }
    start
}

/// Replace `<var data-var-name="...">label</var>` inline nodes.
fn replace_var_tags<F>(text: &str, out: &mut String, mut start: usize, resolve: &F) -> usize
where
    F: Fn(&str) -> Result<Option<String>>,
{
    while let Some(rel) = text[start..].find(VAR_TAG_PREFIX) {
        let begin = start + rel;
        out.push_str(&text[start..begin]);
        let tag_start = begin + VAR_TAG_PREFIX.len();
        match text[tag_start..].find('>') {
            Some(rel_gt) => {
                let gt = tag_start + rel_gt;
                let open_tag = &text[begin..=gt];
                let after_open = gt + 1;
                match text[after_open..].find(VAR_TAG_END) {
                    Some(rel_end) => {
                        let close_begin = after_open + rel_end;
                        let close_end = close_begin + VAR_TAG_END.len();
                        match find_attr_value(open_tag, VAR_TAG_PREFIX.len(), VAR_NAME_ATTR) {
                            Some((ns, ne)) => {
                                let name = &open_tag[ns..ne];
                                match resolve(name) {
                                    Ok(Some(value)) => out.push_str(&value),
                                    Ok(None) => {
                                        // Unknown variable: keep the label shown to end users.
                                        out.push_str(&text[after_open..close_begin]);
                                    }
                                    Err(e) => {
                                        return_log_and_keep(
                                            out,
                                            &text[after_open..close_begin],
                                            e,
                                        );
                                    }
                                }
                            }
                            None => {
                                // Malformed tag without the name attribute: keep the label.
                                out.push_str(&text[after_open..close_begin]);
                            }
                        }
                        start = close_end;
                    }
                    None => {
                        // No closing tag: keep everything literally.
                        out.push_str(&text[begin..]);
                        return text.len();
                    }
                }
            }
            None => {
                // Unterminated opening tag: keep everything literally.
                out.push_str(&text[begin..]);
                return text.len();
            }
        }
    }
    start
}

#[inline]
fn return_log_and_keep(out: &mut String, keep: &str, e: crate::result::Error) {
    log::error!("replace_vars: {e:?}");
    out.push_str(keep);
}

/// Replace all variable references (both notations) in `text`.
/// `resolve` looks a variable up by name and returns its string value.
pub(crate) fn replace_vars_with<F>(text: &str, resolve: F) -> Result<String>
where
    F: Fn(&str) -> Result<Option<String>>,
{
    let mut out = String::with_capacity(text.len().max(128));
    let mut start = 0usize;
    while start < text.len() {
        let next_tag = text[start..].find(VAR_TAG_PREFIX);
        let next_wrap = text[start..].find(VAR_WRAP_SYMBOL);
        match (next_wrap, next_tag) {
            (Some(w), t) if t.is_none_or(|tag| w < tag) => {
                start = replace_wrapped_vars(text, &mut out, start, &resolve);
            }
            (_, Some(tag)) => {
                start = replace_var_tags(text, &mut out, start, &resolve);
            }
            _ => break,
        }
    }
    out.push_str(&text[start..]);
    Ok(out)
}

/// Collect the names of every variable referenced in `text` (both notations),
/// in order of first appearance. The text itself is not resolved, so callers
/// that need asynchronous lookups can resolve these names up front and then
/// feed the values back to [`replace_vars_with`].
pub(crate) fn collect_var_names(text: &str) -> Vec<String> {
    let names = std::cell::RefCell::new(Vec::new());
    // `Ok(None)` keeps every reference verbatim; only the side effect matters.
    let _ = replace_vars_with(text, |name| {
        let mut names = names.borrow_mut();
        if !names.iter().any(|n| n == name) {
            names.push(name.to_string());
        }
        Ok(None)
    });
    names.into_inner()
}

/// Strip rich-text markup from a dialog text, leaving plain display text.
///
/// - `<var>` inline nodes collapse to their inner label;
/// - `` `varName` `` wrapped variables collapse to `varName`;
/// - all other tags are removed;
/// - `<br>`/`</p>` become newlines.
pub(crate) fn text_to_plain(html: &str) -> String {
    let no_wrapped = replace_wrapped_markers(html);
    let mut out = String::with_capacity(no_wrapped.len());
    let mut rest: &str = &no_wrapped;
    while let Some(pos) = rest.find('<') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        if rest.starts_with(VAR_TAG_PREFIX) {
            let tag_start = VAR_TAG_PREFIX.len();
            if let Some(rel) = rest[tag_start..].find('>') {
                let after_open = tag_start + rel + 1;
                if let Some(rel_end) = rest[after_open..].find(VAR_TAG_END) {
                    let close_begin = after_open + rel_end;
                    out.push_str(&rest[after_open..close_begin]);
                    rest = &rest[close_begin + VAR_TAG_END.len()..];
                    continue;
                }
            }
        }
        // Regular (or malformed) tag: skip up to the closing '>'.
        match rest.find('>') {
            Some(rel) => {
                // The line breaks have to be taken here: the tag is gone by the
                // time the loop ends, so a later pass would never see it.
                if is_line_break_tag(&rest[..=rel]) {
                    out.push('\n');
                }
                rest = &rest[rel + 1..];
            }
            None => {
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out.replace('`', "")
}

/// Replace `` `name` `` with `name` so the plain conversion above can keep
/// treating the wrapped notation as ordinary text.
fn replace_wrapped_markers(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut start = 0usize;
    while let Some(rel) = s[start..].find(VAR_WRAP_SYMBOL) {
        let begin = start + rel;
        out.push_str(&s[start..begin]);
        if let Some(rel) = s[begin + 1..].find(VAR_WRAP_SYMBOL) {
            let end = begin + rel + 1;
            out.push_str(&s[begin + 1..end]);
            start = end + 1;
        } else {
            out.push_str(&s[begin..]);
            return out;
        }
    }
    out.push_str(&s[start..]);
    out
}

/// Convert a dialog text to the plain form used by runtime nodes that only
/// support plain text (e.g. sub-flows exported for other channels).
pub(crate) fn dialog_text_to_plain(text: &str, text_type: &AnswerContentType) -> String {
    if matches!(text_type, AnswerContentType::TextHtml) {
        text_to_plain(text)
    } else {
        text.replace("`", "")
    }
}

/// Does `body` come from the rich-text editor rather than from the plain-text
/// textarea that preceded it? Editor output is always a run of top level tags,
/// and a hand written request body is JSON. It never starts with `<`.
fn looks_like_rich_text(body: &str) -> bool {
    body.contains(VAR_TAG_PREFIX) || (body.starts_with('<') && body.ends_with('>'))
}

/// Is `tag` (a full `<...>` slice) a line break in the editor's output?
fn is_line_break_tag(tag: &str) -> bool {
    let inner = tag.trim_start_matches('<').trim_end_matches('>').trim();
    let (name, closing) = match inner.strip_prefix('/') {
        Some(rest) => (rest, true),
        None => (inner, false),
    };
    let name = name
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_end_matches('/');
    name.eq_ignore_ascii_case("br") || (closing && name.eq_ignore_ascii_case("p"))
}

/// Decode the entity at the start of `s` (`s` begins with `&`), returning the
/// character and how many bytes it spans. `None` when it is not an entity.
fn decode_entity_at(s: &str) -> Option<(char, usize)> {
    // The longest entity we handle, `&#x10FFFF;`, in bytes.
    const MAX_ENTITY_BYTES: usize = 10;
    // Only look that far ahead: a `;` further away belongs to something else.
    // Scanning bytes cannot split a character here — a UTF-8 continuation byte
    // is never `;`.
    let end = s
        .as_bytes()
        .iter()
        .take(MAX_ENTITY_BYTES + 1)
        .position(|b| *b == b';')?;
    let body = &s[1..end];
    let c = match body {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        // The editor uses this for spaces it had to protect from collapsing.
        // A plain space is what the request body should carry.
        "nbsp" => ' ',
        _ => {
            let digits = body.strip_prefix('#')?;
            let code = match digits.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => digits.parse::<u32>().ok()?,
            };
            char::from_u32(code)?
        }
    };
    Some((c, end + 1))
}

/// Decode the HTML entities the editor escapes into the text it stores.
fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        match decode_entity_at(rest) {
            Some((c, len)) => {
                out.push(c);
                rest = &rest[len..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Convert a request body written in the rich-text editor into the plain text
/// that is sent on the wire.
///
/// Variable chips become `` `name` `` — deliberately not their label, unlike
/// [`text_to_plain`] — so that the substitution pass that follows can resolve
/// them from either notation. `<br>`/`</p>` become newlines, every other tag is
/// dropped, and entities are decoded last so that text reading `&lt;var` is not
/// taken for a tag.
///
/// Bodies that do not look like rich text are returned unchanged: they were
/// written before the editor existed and may legitimately contain `<`.
pub(crate) fn rich_text_body_to_plain(body: &str) -> String {
    if !looks_like_rich_text(body) {
        return body.to_string();
    }
    let mut out = String::with_capacity(body.len());
    let mut rest = body;
    while let Some(pos) = rest.find('<') {
        out.push_str(&rest[..pos]);
        rest = &rest[pos..];
        if rest.starts_with(VAR_TAG_PREFIX) {
            let tag_start = VAR_TAG_PREFIX.len();
            if let Some(rel) = rest[tag_start..].find('>') {
                let open_tag_end = tag_start + rel;
                let after_open = open_tag_end + 1;
                if let Some(rel_end) = rest[after_open..].find(VAR_TAG_END) {
                    let close_begin = after_open + rel_end;
                    let open_tag = &rest[..=open_tag_end];
                    match find_attr_value(open_tag, tag_start, VAR_NAME_ATTR) {
                        Some((ns, ne)) => {
                            out.push(VAR_WRAP_SYMBOL);
                            out.push_str(&open_tag[ns..ne]);
                            out.push(VAR_WRAP_SYMBOL);
                        }
                        // Malformed chip: keep the label the user sees.
                        None => out.push_str(&rest[after_open..close_begin]),
                    }
                    rest = &rest[close_begin + VAR_TAG_END.len()..];
                    continue;
                }
            }
        }
        // Regular (or malformed) tag: drop it, keeping the line breaks.
        match rest.find('>') {
            Some(rel) => {
                if is_line_break_tag(&rest[..=rel]) {
                    out.push('\n');
                }
                rest = &rest[rel + 1..];
            }
            None => {
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    decode_entities(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver(name: &str) -> Result<Option<String>> {
        match name {
            "known" => Ok(Some("VALUE".to_string())),
            _ => Ok(None),
        }
    }

    #[test]
    fn wrapped_vars_replaced() {
        assert_eq!(
            replace_vars_with("a `known` b", resolver).unwrap(),
            "a VALUE b"
        );
    }

    #[test]
    fn unknown_wrapped_var_kept() {
        assert_eq!(
            replace_vars_with("a `nope` b", resolver).unwrap(),
            "a `nope` b"
        );
    }

    #[test]
    fn unpaired_symbol_kept() {
        assert_eq!(replace_vars_with("a ` b", resolver).unwrap(), "a ` b");
        assert_eq!(replace_vars_with("`", resolver).unwrap(), "`");
    }

    #[test]
    fn var_tag_replaced() {
        let t = "a <var data-var-name=\"known\" data-var-type=\"String\">known</var> b";
        assert_eq!(replace_vars_with(t, resolver).unwrap(), "a VALUE b");
    }

    #[test]
    fn unknown_var_tag_keeps_label() {
        let t = "a <var data-var-name=\"nope\">nope</var> b";
        assert_eq!(replace_vars_with(t, resolver).unwrap(), "a nope b");
    }

    #[test]
    fn malformed_var_tag_kept_literally() {
        let t = "a <var known> b";
        assert_eq!(replace_vars_with(t, resolver).unwrap(), t);
        let t2 = "a <var data-var-name=\"known\" b";
        assert_eq!(replace_vars_with(t2, resolver).unwrap(), t2);
    }

    #[test]
    fn mixed_notations() {
        let t = "`known` and <var data-var-name=\"known\">known</var>";
        assert_eq!(
            replace_vars_with(t, resolver).unwrap(),
            "VALUE and VALUE"
        );
    }

    #[test]
    fn html_to_plain() {
        let t = "<p>hi <var data-var-name=\"known\">known</var>!</p><p>bye `known`</p>";
        assert_eq!(text_to_plain(t), "hi known!\nbye known\n");
    }

    #[test]
    fn html_to_plain_line_breaks() {
        assert_eq!(text_to_plain("a<br>b"), "a\nb");
        assert_eq!(text_to_plain("a<br/>b"), "a\nb");
        assert_eq!(text_to_plain("a<br />b"), "a\nb");
        // An opening tag is not a break of its own.
        assert_eq!(text_to_plain("<p>a</p><p>b</p>"), "a\nb\n");
    }

    #[test]
    fn plain_type_strips_marks() {
        assert_eq!(
            dialog_text_to_plain("hi `known`", &AnswerContentType::TextPlain),
            "hi known"
        );
    }

    /// The body stored by the editor, as `editor.getHTML()` writes it.
    const RICH_BODY: &str = "<p>{\"a\": <var class=\"var-chip\" data-var-name=\"known\" data-var-type=\"String\">known</var>}</p>";

    #[test]
    fn rich_body_chip_becomes_wrapped_var() {
        assert_eq!(rich_text_body_to_plain(RICH_BODY), "{\"a\": `known`}\n");
        // …which the substitution pass that follows can then resolve.
        assert_eq!(
            replace_vars_with(&rich_text_body_to_plain(RICH_BODY), resolver).unwrap(),
            "{\"a\": VALUE}\n"
        );
    }

    #[test]
    fn rich_body_keeps_line_breaks() {
        assert_eq!(
            rich_text_body_to_plain("<p>line1</p><p>line2</p>"),
            "line1\nline2\n"
        );
        assert_eq!(rich_text_body_to_plain("<p>a<br>b</p>"), "a\nb\n");
    }

    #[test]
    fn rich_body_decodes_entities() {
        assert_eq!(
            rich_text_body_to_plain("<p>a &lt; b &amp;&amp; c&nbsp;d</p>"),
            "a < b && c d\n"
        );
        assert_eq!(rich_text_body_to_plain("<p>&#34;x&#x22;</p>"), "\"x\"\n");
    }

    #[test]
    fn rich_body_malformed_chip_keeps_label() {
        let t = "<p><var data-var-name>known</var></p>";
        assert_eq!(rich_text_body_to_plain(t), "known\n");
    }

    #[test]
    fn plain_body_untouched() {
        // Written before the rich-text editor existed: `<` is literal here, and
        // so is an entity-looking sequence.
        assert_eq!(
            rich_text_body_to_plain("{\"a\": \"a<b\"}"),
            "{\"a\": \"a<b\"}"
        );
        assert_eq!(rich_text_body_to_plain("a &amp; b"), "a &amp; b");
        assert_eq!(rich_text_body_to_plain(""), "");
    }
}
