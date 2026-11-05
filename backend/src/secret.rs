//! 脱敏工具（对外统一入口，内部实现复用 `crypto` 的脱敏函数）。

pub use crate::crypto::{is_sensitive_key, mask_secret, redact_line, redact_lines};

/// 对配置内容做"可展示"处理：把 token / password / secret_key 之类替换掉。
/// 用于日志预览、审计详情与错误信息。
pub fn redact_config(text: &str) -> String {
    redact_lines(&text.lines().map(str::to_string).collect::<Vec<_>>()).join("\n")
}

/// 计算两份文本的差异统计（用于审计记录"改了几行"）。
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct DiffStat {
    pub added: usize,
    pub removed: usize,
    pub changed: bool,
}

pub fn diff_stat(old: &str, new: &str) -> DiffStat {
    use std::collections::HashMap;
    let mut old_lines: HashMap<&str, i32> = HashMap::new();
    for l in old.lines() {
        *old_lines.entry(l).or_insert(0) += 1;
    }
    let mut added = 0;
    for l in new.lines() {
        match old_lines.get_mut(l) {
            Some(n) if *n > 0 => *n -= 1,
            _ => added += 1,
        }
    }
    let removed = old_lines.values().filter(|n| **n > 0).sum::<i32>() as usize;
    DiffStat {
        added,
        removed,
        changed: added > 0 || removed > 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_toml_secrets() {
        let cfg = "token = \"abcdef\"\nwebServer.password = \"pw\"\nbind_port = 7000\n";
        let out = redact_config(cfg);
        assert!(!out.contains("abcdef"));
        assert!(!out.contains("\"pw\""));
        assert!(out.contains("bind_port = 7000"));
    }

    #[test]
    fn diff_counts_lines() {
        let a = "a\nb\nc";
        let b = "a\nc\nd";
        let d = diff_stat(a, b);
        assert_eq!(d.added, 1);
        assert_eq!(d.removed, 1);
        assert!(d.changed);
        assert!(!diff_stat(a, a).changed);
    }
}
