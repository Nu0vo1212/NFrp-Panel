//! 内置二进制版本清单（离线可用）。
//!
//! ★ 说明：这里**不内置任何真实下载链接**（避免面板替用户"决定信任哪个二进制"）。
//! 用户可在设置页填入自己的镜像与 SHA256；清单只提供命名约定与校验字段。

use serde::Serialize;

/// 版本清单条目（离线内置，URL 由用户填写）。
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEntry {
    pub flavor: String,
    pub version: String,
    pub arch: String,
    pub os: String,
    pub url: String,
    pub sha256: Option<String>,
    pub note: String,
}

/// 内置示例条目：URL 为空，提示用户自行填写。
pub fn builtin_entries() -> Vec<CatalogEntry> {
    let mut out = Vec::new();
    for (flavor, version) in [("nfrp", "0.7.4"), ("official_frp", "0.71.1")] {
        for arch in ["amd64", "arm64"] {
            out.push(CatalogEntry {
                flavor: flavor.into(),
                version: version.into(),
                arch: arch.into(),
                os: "linux".into(),
                url: String::new(),
                sha256: None,
                note: if flavor == "nfrp" {
                    format!("NFrp {version} linux-{arch}（来自 Rust-Frp Releases）")
                } else {
                    format!("frp {version} linux-{arch}（来自 fatedier/frp Releases）")
                },
            });
        }
    }
    out
}

/// 包命名约定（用于识别上传文件与生成下载名）。
pub fn package_name(flavor: &str, version: &str, os: &str, arch: &str) -> String {
    match flavor {
        "nfrp" => format!("nfrp-{os}-{arch}"),
        _ => format!("frp_{version}_{os}_{arch}"),
    }
}

/// 校验下载/上传的二进制是否与目标架构匹配（通过 ELF 头判断）。
pub fn elf_machine(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 20 || &bytes[0..4] != b"\x7fELF" {
        return None;
    }
    // e_machine 位于偏移 18，小端
    let machine = u16::from_le_bytes([bytes[18], bytes[19]]);
    Some(match machine {
        0x3E => "amd64",
        0xB7 => "arm64",
        0x28 => "arm",
        0x03 => "386",
        _ => "unknown",
    })
}

/// 是否为 Windows PE 可执行文件。
pub fn is_pe(bytes: &[u8]) -> bool {
    bytes.len() >= 2 && &bytes[0..2] == b"MZ"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_elf_arch() {
        // 标准 ELF64 头：e_machine 位于偏移 18（小端）
        let mut amd64 = vec![0x7f, b'E', b'L', b'F']; // e_ident 魔数
        amd64.extend_from_slice(&[2, 1, 1, 0]); // 64 位 / 小端 / 版本 1
        amd64.extend_from_slice(&[0u8; 8]); // 补齐 e_ident 到 16 字节
        amd64.extend_from_slice(&2u16.to_le_bytes()); // e_type
        amd64.extend_from_slice(&0x3Eu16.to_le_bytes()); // e_machine = x86-64
        amd64.extend_from_slice(&[0u8; 8]); // 其余头字段
        assert_eq!(amd64.len() >= 20, true);
        assert_eq!(elf_machine(&amd64), Some("amd64"));

        // aarch64：只改 e_machine
        let mut arm64 = amd64.clone();
        arm64[18] = 0xB7;
        arm64[19] = 0x00;
        assert_eq!(elf_machine(&arm64), Some("arm64"));
    }

    #[test]
    fn detects_pe() {
        assert!(is_pe(b"MZ\x90\x00"));
        assert!(!is_pe(b"\x7fELF"));
        assert!(!is_pe(b"M"));
    }

    #[test]
    fn not_elf_for_short_input() {
        assert_eq!(elf_machine(b"\x7fELF"), None);
        assert_eq!(elf_machine(b"hello"), None);
    }

    #[test]
    fn package_naming() {
        assert_eq!(package_name("nfrp", "0.7.4", "linux", "amd64"), "nfrp-linux-amd64");
        assert_eq!(
            package_name("official_frp", "0.71.1", "linux", "arm64"),
            "frp_0.71.1_linux_arm64"
        );
    }
}
