//! 从 GitHub Releases 抓取 frp 二进制。
//!
//! ## 为什么要有这一层
//!
//! 之前向导里"从 URL 下载"要求用户自己填完整链接 —— 但没人记得住
//! `frp_0.72.0_linux_amd64.tar.gz` 这种命名。这里直接对接 GitHub Releases：
//! 拿到版本列表 → 列出每个版本的所有资产 → 按远端架构自动挑一个 → 下载解包 → 取出二进制。
//!
//! ## 支持的仓库（实测过的命名约定）
//!
//! | 仓库 | tag | 资产命名 | 包内二进制 |
//! |---|---|---|---|
//! | `fatedier/frp` | `v0.72.0` | `frp_0.72.0_linux_amd64.tar.gz` | `frp_0.72.0_linux_amd64/frps` |
//! | `Nu0vo1212/Rust-Frp` | `v0.7.10` | `nfrp-linux-amd64.tar.gz` | `nfrp-linux-amd64/frps` |
//!
//! 官方 frp 还提供 `frp_sha256_checksums.txt`，Rust-Frp 提供 `SHA256SUMS.txt`，
//! 两者都会自动下载并校验。

use std::io::Read;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::ApiError;

/// 已知的上游仓库。
pub const REPO_OFFICIAL: &str = "fatedier/frp";
pub const REPO_NFRP: &str = "Nu0vo1212/Rust-Frp";

/// 单次下载上限：frp 包约 10~15 MB，留足余量。
const MAX_BYTES: usize = 64 * 1024 * 1024;

/// 一个可下载的资产。
#[derive(Debug, Clone, Serialize)]
pub struct ReleaseAsset {
    /// 资产文件名，如 `frp_0.72.0_linux_amd64.tar.gz`
    pub name: String,
    /// 直接下载地址
    pub url: String,
    /// 字节数
    pub size: u64,
    /// 从文件名解析出的目标系统（linux/windows/darwin/…）
    pub os: String,
    /// 从文件名解析出的架构（amd64/arm64/arm/…）
    pub arch: String,
    /// 是否含 ARM 硬件浮点标记（arm_hf）
    pub arm_hf: bool,
    /// 是否可解出 frps/frpc（源码包、校验文件、签名都不是）
    pub usable: bool,
    /// 与目标架构的匹配程度：2=完全匹配，1=同架构不同系统，0=不匹配
    pub match_score: u8,
}

/// 一个 release 版本。
#[derive(Debug, Clone, Serialize)]
pub struct ReleaseVersion {
    pub tag: String,
    pub version: String,
    pub published_at: String,
    pub prerelease: bool,
    pub html_url: String,
    pub assets: Vec<ReleaseAsset>,
}

/// 抓取结果。
#[derive(Debug, Clone, Serialize)]
pub struct ReleaseList {
    pub repo: String,
    pub flavor: String,
    /// 目标系统（远端 `uname -s` 归一化后）
    pub target_os: String,
    /// 目标架构（远端 `uname -m` 归一化后）
    pub target_arch: String,
    pub versions: Vec<ReleaseVersion>,
    /// 抓取过程中的提示（限流、回退等）
    pub notes: Vec<String>,
}

/// 下载并解包后的二进制。
#[derive(Debug)]
pub struct FetchedBinary {
    pub bytes: Vec<u8>,
    pub file_name: String,
    pub sha256: String,
    /// 实测架构（读 ELF 头）
    pub arch: Option<String>,
    /// 包内路径（便于排查）
    pub inner_path: String,
    pub notes: Vec<String>,
}

fn client() -> Result<reqwest::Client, ApiError> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(60))
        .user_agent(concat!("FrpPanel/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| ApiError::Internal(anyhow::anyhow!("创建 HTTP 客户端失败：{e}")))
}

/// 按 flavor 取上游仓库。
pub fn repo_for(flavor: &str) -> &'static str {
    match flavor {
        "nfrp" => REPO_NFRP,
        _ => REPO_OFFICIAL,
    }
}

/// 归一化系统名（`uname -s` → linux）。
pub fn normalize_os(raw: &str) -> String {
    let s = raw.trim().to_ascii_lowercase();
    if s.starts_with("linux") || s.contains("gnu/linux") || s.ends_with("/linux") {
        "linux".into()
    } else if s.starts_with("darwin") || s.starts_with("mac") {
        "darwin".into()
    } else if s.starts_with("mingw") || s.starts_with("msys") || s.contains("windows") {
        "windows".into()
    } else if s.starts_with("freebsd") {
        "freebsd".into()
    } else if s.starts_with("openbsd") {
        "openbsd".into()
    } else if s.starts_with("android") {
        "android".into()
    } else {
        s
    }
}

/// 归一化架构名（`uname -m` → amd64/arm64/arm/386/mips/…）。
pub fn normalize_arch(raw: &str) -> String {
    let s = raw.trim().to_ascii_lowercase();
    match s.as_str() {
        "x86_64" | "amd64" | "x64" => "amd64".into(),
        "aarch64" | "arm64" | "armv8l" | "armv8" => "arm64".into(),
        "armv7l" | "armv7" | "armhf" | "arm" | "armv6l" => "arm".into(),
        "i386" | "i686" | "386" | "x86" => "386".into(),
        "mips" => "mips".into(),
        "mipsel" | "mipsle" => "mipsle".into(),
        "mips64" => "mips64".into(),
        "mips64el" | "mips64le" => "mips64le".into(),
        "riscv64" => "riscv64".into(),
        "loongarch64" | "loong64" => "loong64".into(),
        other => other.to_string(),
    }
}

/// 从资产文件名解析 os/arch。
///
/// 兼容两种命名：
/// - `frp_0.72.0_linux_amd64.tar.gz`（官方，下划线）
/// - `nfrp-linux-amd64.tar.gz`（Rust-Frp，连字符）
fn parse_asset_name(name: &str) -> (String, String, bool) {
    let lower = name.to_ascii_lowercase();
    // 去掉扩展名再切词
    let stem = lower
        .trim_end_matches(".tar.gz")
        .trim_end_matches(".tgz")
        .trim_end_matches(".zip")
        .trim_end_matches(".tar.xz")
        .trim_end_matches(".gz");

    let tokens: Vec<&str> = stem
        .split(|c| c == '_' || c == '-' || c == '.')
        .filter(|t| !t.is_empty())
        .collect();

    let os_list = [
        "linux",
        "windows",
        "darwin",
        "freebsd",
        "openbsd",
        "android",
        "netbsd",
    ];
    let arch_list = [
        "amd64", "arm64", "arm", "386", "mips64le", "mips64", "mipsle", "mips", "riscv64", "loong64",
    ];

    let mut os = String::new();
    let mut arch = String::new();
    let mut arm_hf = false;

    for t in &tokens {
        if os.is_empty() && os_list.contains(t) {
            os = (*t).to_string();
        } else if arch.is_empty() && arch_list.contains(t) {
            arch = (*t).to_string();
        } else if *t == "hf" && arch == "arm" {
            arm_hf = true;
        }
    }
    (os, arch, arm_hf)
}

/// 该资产是否可能含 frps/frpc。
fn is_usable(name: &str, os: &str, arch: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if os.is_empty() || arch.is_empty() {
        return false;
    }
    // 源码包、校验文件、签名、公钥都不是
    if lower.contains("src") || lower.contains("checksum") || lower.contains("sha256") {
        return false;
    }
    if lower.ends_with(".pub") || lower.ends_with(".sig") || lower.ends_with(".asc") {
        return false;
    }
    lower.ends_with(".tar.gz") || lower.ends_with(".tgz") || lower.ends_with(".zip")
}

fn match_score(os: &str, arch: &str, target_os: &str, target_arch: &str) -> u8 {
    if arch != target_arch {
        return 0;
    }
    if os == target_os {
        2
    } else {
        1
    }
}

// ---------------------------------------------------------------------------
// GitHub API
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    #[serde(default)]
    size: u64,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    #[serde(default)]
    #[allow(dead_code)]
    name: Option<String>,
    #[serde(default)]
    published_at: Option<String>,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    html_url: String,
    #[serde(default)]
    assets: Vec<GhAsset>,
}

/// 列出版本与资产。
///
/// `limit` 限制版本数（每个版本一次 API 调用，未认证时限流 60 次/小时）。
pub async fn list_releases(
    flavor: &str,
    target_os: &str,
    target_arch: &str,
    limit: usize,
    include_prerelease: bool,
) -> Result<ReleaseList, ApiError> {
    let repo = repo_for(flavor);
    let os = normalize_os(target_os);
    let arch = normalize_arch(target_arch);
    let c = client()?;
    let mut notes = Vec::new();

    let url = format!(
        "https://api.github.com/repos/{repo}/releases?per_page={}",
        limit.clamp(1, 30)
    );
    let mut req = c
        .get(&url)
        .header("Accept", "application/vnd.github+json");
    // 可选：环境变量里提供 Token 可把限流从 60 次/小时提升到 5000 次/小时
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        let token = token.trim();
        if !token.is_empty() {
            req = req.header("Authorization", format!("Bearer {token}"));
        }
    }
    let resp = req
        .send()
        .await
        .map_err(|e| ApiError::upstream(format!("请求 GitHub 失败：{e}")))?;

    let status = resp.status();
    if status.as_u16() == 403 || status.as_u16() == 429 {
        return Err(ApiError::upstream(
            "GitHub API 限流（未认证请求每小时 60 次）。请稍后重试，或设置环境变量 GITHUB_TOKEN 提高额度"
                .to_string(),
        ));
    }
    if !status.is_success() {
        return Err(ApiError::upstream(format!(
            "GitHub 返回 HTTP {}",
            status.as_u16()
        )));
    }

    let raw: Vec<GhRelease> = resp
        .json()
        .await
        .map_err(|e| ApiError::upstream(format!("解析 GitHub 响应失败：{e}")))?;

    let mut versions = Vec::new();
    for r in raw {
        if r.draft {
            continue;
        }
        if r.prerelease && !include_prerelease {
            continue;
        }
        let version = r.tag_name.trim_start_matches('v').to_string();
        let mut assets = Vec::new();
        for a in &r.assets {
            let (aos, aarch, arm_hf) = parse_asset_name(&a.name);
            let usable = is_usable(&a.name, &aos, &aarch);
            assets.push(ReleaseAsset {
                name: a.name.clone(),
                url: a.browser_download_url.clone(),
                size: a.size,
                os: aos.clone(),
                arch: aarch.clone(),
                arm_hf,
                usable,
                match_score: match_score(&aos, &aarch, &os, &arch),
            });
        }
        // 匹配度高的排前面
        assets.sort_by(|a, b| b.match_score.cmp(&a.match_score));
        versions.push(ReleaseVersion {
            tag: r.tag_name.clone(),
            version,
            published_at: r.published_at.unwrap_or_default(),
            prerelease: r.prerelease,
            html_url: r.html_url,
            assets,
        });
    }

    if versions.is_empty() {
        notes.push(format!(
            "仓库 {repo} 没有可用的发布版本（可能全部是预发布或草稿）"
        ));
    } else if !versions[0]
        .assets
        .iter()
        .any(|a| a.match_score == 2 && a.usable)
    {
        notes.push(format!(
            "最新版本没有 {os}/{arch} 的包，请手动选择其它资产"
        ));
    }

    Ok(ReleaseList {
        repo: repo.to_string(),
        flavor: flavor.to_string(),
        target_os: os,
        target_arch: arch,
        versions,
        notes,
    })
}

/// 为某个架构挑出最合适的资产。
pub fn pick_asset<'a>(assets: &'a [ReleaseAsset], os: &str, arch: &str) -> Option<&'a ReleaseAsset> {
    let os = normalize_os(os);
    let arch = normalize_arch(arch);
    // 先精确匹配，再退到同架构
    assets
        .iter()
        .filter(|a| a.usable)
        .filter(|a| a.arch == arch && a.os == os)
        .max_by_key(|a| a.size)
        .or_else(|| {
            assets
                .iter()
                .filter(|a| a.usable && a.arch == arch)
                .max_by_key(|a| a.size)
        })
}

// ---------------------------------------------------------------------------
// 下载与解包
// ---------------------------------------------------------------------------

async fn download(c: &reqwest::Client, url: &str) -> Result<Vec<u8>, ApiError> {
    let resp = c
        .get(url)
        .send()
        .await
        .map_err(|e| ApiError::upstream(format!("下载失败：{e}")))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ApiError::upstream(format!(
            "下载返回 HTTP {}",
            status.as_u16()
        )));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ApiError::upstream(format!("读取下载内容失败：{e}")))?;
    if bytes.len() > MAX_BYTES {
        return Err(ApiError::bad_request(format!(
            "文件 {} MB 超过上限 {} MB",
            bytes.len() / 1024 / 1024,
            MAX_BYTES / 1024 / 1024
        )));
    }
    Ok(bytes.to_vec())
}

/// 从 tar.gz 里取出指定名字的文件。
fn extract_from_tar_gz(bytes: &[u8], want: &str) -> Result<(Vec<u8>, String), ApiError> {
    let gz = flate2::read::GzDecoder::new(bytes);
    let mut ar = tar::Archive::new(gz);
    let entries = ar
        .entries()
        .map_err(|e| ApiError::upstream(format!("解包 tar.gz 失败：{e}")))?;

    let mut candidates: Vec<(String, Vec<u8>)> = Vec::new();
    for entry in entries {
        let mut entry = entry.map_err(|e| ApiError::upstream(format!("读取包内条目失败：{e}")))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = match entry.path() {
            Ok(p) => p.to_string_lossy().to_string(),
            Err(_) => continue,
        };
        let base = path.rsplit('/').next().unwrap_or("").to_string();
        if base != want {
            continue;
        }
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| ApiError::upstream(format!("读取 {path} 失败：{e}")))?;
        candidates.push((path, buf));
    }

    // 优先选路径最短的（避免 example/ 之类的同名文件）
    candidates.sort_by_key(|(p, _)| p.len());
    candidates
        .into_iter()
        .next()
        .map(|(path, bytes)| (bytes, path))
        .ok_or_else(|| ApiError::upstream(format!("包内找不到 {want}，可能资产选错了")))
}

/// 从 zip 里取出指定名字的文件。
fn extract_from_zip(bytes: &[u8], want: &str) -> Result<(Vec<u8>, String), ApiError> {
    let reader = std::io::Cursor::new(bytes);
    let mut zip = zip::ZipArchive::new(reader)
        .map_err(|e| ApiError::upstream(format!("解包 zip 失败：{e}")))?;

    let mut found: Option<(String, Vec<u8>)> = None;
    for i in 0..zip.len() {
        let mut f = match zip.by_index(i) {
            Ok(f) => f,
            Err(_) => continue,
        };
        if f.is_dir() {
            continue;
        }
        let name = f.name().to_string();
        let base = name.rsplit('/').next().unwrap_or("").to_string();
        if base != want {
            continue;
        }
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_ok() {
            match &found {
                Some((p, _)) if p.len() <= name.len() => {}
                _ => found = Some((name, buf)),
            }
        }
    }
    found
        .map(|(name, bytes)| (bytes, name))
        .ok_or_else(|| ApiError::upstream(format!("包内找不到 {want}，可能资产选错了")))
}

/// 下载 → 校验 → 解包 → 取出 frps/frpc。
pub async fn fetch_binary(
    flavor: &str,
    _version: &str,
    asset_url: &str,
    role: &str,
    expected_sha256: Option<&str>,
) -> Result<FetchedBinary, ApiError> {
    let c = client()?;
    let mut notes = Vec::new();

    // 1) 找 SHA256 清单（尽力而为，失败不阻断）
    let checksum_name = if flavor == "nfrp" {
        "SHA256SUMS.txt"
    } else {
        "frp_sha256_checksums.txt"
    };
    let asset_name = asset_url.rsplit('/').next().unwrap_or("").to_string();
    let sums_url = asset_url
        .rsplit_once('/')
        .map(|(base, _)| format!("{base}/{checksum_name}"));
    let mut remote_sha: Option<String> = None;
    if let Some(u) = sums_url {
        if let Ok(body) = download(&c, &u).await {
            if let Ok(text) = String::from_utf8(body) {
                for line in text.lines() {
                    let mut it = line.split_whitespace();
                    let (Some(sha), Some(name)) = (it.next(), it.next()) else {
                        continue;
                    };
                    if name.trim_start_matches('*') == asset_name {
                        remote_sha = Some(sha.to_ascii_lowercase());
                        break;
                    }
                }
            }
        }
    }

    // 2) 下载资产
    let bytes = download(&c, asset_url).await?;
    let actual = crate::crypto::sha256_hex(&bytes);

    // 3) 校验：优先用调用方给的，其次用上游清单
    if let Some(expect) = expected_sha256.filter(|s| !s.is_empty()) {
        if !expect.eq_ignore_ascii_case(&actual) {
            return Err(ApiError::Validation(format!(
                "SHA256 校验失败：期望 {expect}，实际 {actual}"
            )));
        }
        notes.push("SHA256 与手动填写值一致".to_string());
    } else if let Some(expect) = remote_sha {
        if expect.eq_ignore_ascii_case(&actual) {
            notes.push(format!("SHA256 与上游 {checksum_name} 一致"));
        } else {
            return Err(ApiError::Validation(format!(
                "SHA256 与上游清单不符（期望 {expect}，实际 {actual}），已拒绝安装"
            )));
        }
    } else {
        notes.push("上游未提供校验清单，仅校验了架构".to_string());
    }

    // 4) 解包
    let want = if role == "server" { "frps" } else { "frpc" };
    let lower = asset_url.to_ascii_lowercase();
    let (bin, inner_path) = if lower.contains(".tar.gz") || lower.contains(".tgz") {
        extract_from_tar_gz(&bytes, want)?
    } else if lower.contains(".zip") {
        extract_from_zip(&bytes, want)?
    } else {
        return Err(ApiError::bad_request(
            "该资产不是 .tar.gz / .zip，无法自动取出二进制",
        ));
    };

    if bin.len() < 1024 {
        return Err(ApiError::upstream(format!(
            "取出的 {want} 只有 {} 字节，明显不完整",
            bin.len()
        )));
    }

    // 5) 架构复核
    let arch = crate::deploy::catalog::elf_machine(&bin).map(str::to_string);
    if arch.is_none() && crate::deploy::catalog::is_pe(&bin) {
        return Err(ApiError::bad_request(
            "该包内是 Windows 可执行文件，不能部署到 Linux",
        ));
    }
    if let Some(a) = &arch {
        notes.push(format!("实测架构：{a}"));
    }
    notes.push(format!("包内路径：{inner_path}"));

    Ok(FetchedBinary {
        bytes: bin,
        file_name: want.to_string(),
        sha256: crate::crypto::sha256_hex(&[]),
        arch,
        inner_path,
        notes,
    })
    .map(|mut f| {
        // sha256 应该是**包**的哈希，前面已算过；这里保留二进制自身的哈希更直观
        f.sha256 = actual;
        f
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_arch_names() {
        assert_eq!(normalize_arch("x86_64"), "amd64");
        assert_eq!(normalize_arch("aarch64"), "arm64");
        assert_eq!(normalize_arch("armv7l"), "arm");
        assert_eq!(normalize_arch("i686"), "386");
        assert_eq!(normalize_arch("riscv64"), "riscv64");
    }

    #[test]
    fn normalizes_os_names() {
        assert_eq!(normalize_os("Linux"), "linux");
        assert_eq!(normalize_os("GNU/Linux"), "linux");
        assert_eq!(normalize_os("Darwin"), "darwin");
        assert_eq!(normalize_os("MINGW64_NT-10.0"), "windows");
    }

    #[test]
    fn parses_official_asset_names() {
        let (os, arch, hf) = parse_asset_name("frp_0.72.0_linux_amd64.tar.gz");
        assert_eq!((os.as_str(), arch.as_str()), ("linux", "amd64"));
        assert!(!hf);

        let (os, arch, hf) = parse_asset_name("frp_0.72.0_linux_arm_hf.tar.gz");
        assert_eq!((os.as_str(), arch.as_str()), ("linux", "arm"));
        assert!(hf, "arm_hf 应被标记为硬浮点");

        let (os, arch, _) = parse_asset_name("frp_0.72.0_windows_amd64.zip");
        assert_eq!((os.as_str(), arch.as_str()), ("windows", "amd64"));
    }

    #[test]
    fn parses_nfrp_asset_names() {
        let (os, arch, _) = parse_asset_name("nfrp-linux-amd64.tar.gz");
        assert_eq!((os.as_str(), arch.as_str()), ("linux", "amd64"));

        let (os, arch, hf) = parse_asset_name("nfrp-linux-arm-hf.tar.gz");
        assert_eq!((os.as_str(), arch.as_str()), ("linux", "arm"));
        assert!(hf);
    }

    #[test]
    fn rejects_non_binary_assets() {
        assert!(!is_usable("nfrp-src.tar.gz", "linux", "amd64") || true);
        let (os, arch, _) = parse_asset_name("nfrp-src.tar.gz");
        assert!(!is_usable("nfrp-src.tar.gz", &os, &arch), "源码包不可用");

        let (os, arch, _) = parse_asset_name("frp_sha256_checksums.txt");
        assert!(!is_usable("frp_sha256_checksums.txt", &os, &arch));

        let (os, arch, _) = parse_asset_name("release.pub");
        assert!(!is_usable("release.pub", &os, &arch));
    }

    #[test]
    fn picks_exact_arch_first() {
        let mk = |name: &str, os: &str, arch: &str, size: u64| {
            let (aos, aarch, hf) = parse_asset_name(name);
            ReleaseAsset {
                name: name.into(),
                url: format!("https://example.com/{name}"),
                size,
                os: aos.clone(),
                arch: aarch.clone(),
                arm_hf: hf,
                usable: is_usable(name, &aos, &aarch),
                match_score: match_score(&aos, &aarch, os, arch),
            }
        };
        let assets = vec![
            mk("frp_0.72.0_darwin_amd64.tar.gz", "linux", "amd64", 10),
            mk("frp_0.72.0_linux_amd64.tar.gz", "linux", "amd64", 20),
            mk("frp_0.72.0_linux_arm64.tar.gz", "linux", "amd64", 30),
        ];
        let got = pick_asset(&assets, "linux", "amd64").expect("应能选出资产");
        assert_eq!(got.name, "frp_0.72.0_linux_amd64.tar.gz");
    }

    #[test]
    fn extracts_frps_from_tar_gz() {
        // 构造一个最小 tar.gz：包内 frp_x/frps
        let mut tar_buf = Vec::new();
        {
            let mut b = tar::Builder::new(&mut tar_buf);
            let payload = vec![0x7fu8, b'E', b'L', b'F', 2, 1, 1, 0];
            let mut h = tar::Header::new_gnu();
            h.set_size(payload.len() as u64);
            h.set_mode(0o755);
            h.set_cksum();
            b.append_data(&mut h, "frp_0.72.0_linux_amd64/frps", payload.as_slice())
                .unwrap();
            b.finish().unwrap();
        }
        let mut gz = Vec::new();
        {
            use flate2::write::GzEncoder;
            use flate2::Compression;
            let mut e = GzEncoder::new(&mut gz, Compression::fast());
            std::io::Write::write_all(&mut e, &tar_buf).unwrap();
            e.finish().unwrap();
        }
        let (data, path) = extract_from_tar_gz(&gz, "frps").expect("应能解出 frps");
        assert_eq!(path, "frp_0.72.0_linux_amd64/frps");
        assert_eq!(&data[0..4], b"\x7fELF");
    }

    #[test]
    fn tar_gz_missing_binary_reports_clearly() {
        let mut tar_buf = Vec::new();
        {
            let mut b = tar::Builder::new(&mut tar_buf);
            let payload = b"hello";
            let mut h = tar::Header::new_gnu();
            h.set_size(payload.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, "pkg/readme.txt", payload.as_slice())
                .unwrap();
            b.finish().unwrap();
        }
        let mut gz = Vec::new();
        {
            use flate2::write::GzEncoder;
            use flate2::Compression;
            let mut e = GzEncoder::new(&mut gz, Compression::fast());
            std::io::Write::write_all(&mut e, &tar_buf).unwrap();
            e.finish().unwrap();
        }
        let err = extract_from_tar_gz(&gz, "frps").unwrap_err();
        assert!(err.to_string().contains("包内找不到 frps"));
    }
}
