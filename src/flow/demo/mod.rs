pub(crate) const DEMO_COLLECT: &str = include_str!("demo_collect.txt");
pub(crate) const DEMO_NOTIFY: &str = include_str!("demo_notify.txt");
pub(crate) const DEMO_REPAY: &str = include_str!("demo_repay.txt");
pub(crate) const DEMO_COLLECT_EN: &str = include_str!("demo_collect_en.txt");
pub(crate) const DEMO_NOTIFY_EN: &str = include_str!("demo_notify_en.txt");
pub(crate) const DEMO_REPAY_EN: &str = include_str!("demo_repay_en.txt");

/// demo 的 `main_flow_id` 是**固定**的：整个平台共用同一套模板，没有对应的
/// `mainflows` 行，也不能被保存/新建/删除。前端路由与这里的常量必须保持一致。
pub(crate) const DEMO_COLLECT_ID: &str = "demo-collect";
pub(crate) const DEMO_NOTIFY_ID: &str = "demo-notify";
pub(crate) const DEMO_REPAY_ID: &str = "demo-repay";

/// 是不是内置 demo 的固定 id。
pub(crate) fn is_demo_id(id: &str) -> bool {
    id == DEMO_COLLECT_ID || id == DEMO_NOTIFY_ID || id == DEMO_REPAY_ID
}

pub(crate) fn get_demo(is_en: bool, name: &str) -> Option<&'static str> {
    match name {
        DEMO_COLLECT_ID => Some(if is_en { DEMO_COLLECT_EN } else { DEMO_COLLECT }),
        DEMO_NOTIFY_ID => Some(if is_en { DEMO_NOTIFY_EN } else { DEMO_NOTIFY }),
        DEMO_REPAY_ID => Some(if is_en { DEMO_REPAY_EN } else { DEMO_REPAY }),
        _ => None,
    }
}

/// demo 只读：任何保存画布、新建/删除子流程、保存/删除主流程的请求都拒绝，
/// 而不是静默 no-op。允许的写操作只有 `/mainflow/release`（直接发布）。
pub(crate) fn ensure_editable(main_flow_id: &str) -> crate::result::Result<()> {
    if is_demo_id(main_flow_id) {
        Err(crate::result::Error::WithMessage(String::from(
            "Demo flows are read-only and can only be published",
        )))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_three_fixed_ids_are_recognised() {
        for id in [DEMO_COLLECT_ID, DEMO_NOTIFY_ID, DEMO_REPAY_ID] {
            assert!(is_demo_id(id), "{id} 应该是 demo id");
            assert!(get_demo(true, id).is_some());
            assert!(get_demo(false, id).is_some());
        }
        assert!(!is_demo_id("demo"));
        assert!(!is_demo_id("demo-collect-2"));
        assert!(!is_demo_id("rABC"));
        assert!(get_demo(true, "demo").is_none());
    }

    #[test]
    fn demo_ids_are_read_only() {
        for id in [DEMO_COLLECT_ID, DEMO_NOTIFY_ID, DEMO_REPAY_ID] {
            assert!(ensure_editable(id).is_err(), "{id} 必须拒绝修改");
        }
        assert!(ensure_editable("rABC").is_ok());
    }
}
