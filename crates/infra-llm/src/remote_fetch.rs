use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Client;

const MAX_REMOTE_ST_BYTES: usize = 2 * 1024 * 1024;
const ALLOWED_HOSTS: &[&str] = &[
    "raw.githubusercontent.com",
    "gist.githubusercontent.com",
    "img.kookapp.cn",
    "file.kookapp.cn",
    "media.kookapp.cn",
];

fn shared_safe_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(15))
            .build()
            .expect("safe reqwest client")
    })
}

/// Strictly validates a remote SillyTavern import URL against SSRF:
/// HTTPS only, default port 443, no userinfo, exact host allowlist, no `..` / `%2e%2e` traversal.
pub fn validate_allowlisted_import_url(raw: &str) -> Result<reqwest::Url> {
    let trimmed = raw.trim();
    let lower_raw = trimmed.to_ascii_lowercase();
    if lower_raw.contains("..") || lower_raw.contains("%2e") {
        bail!("URL 包含非法路径穿越字符段，已拒绝");
    }
    let url = reqwest::Url::parse(trimmed).context("无效的 URL 格式")?;
    if url.scheme() != "https" {
        bail!("出于安全限制，仅允许 HTTPS 链接（禁止 http/file 等协议）");
    }
    if !url.username().is_empty() || url.password().is_some() {
        bail!("拒绝包含用户名/密码段的 URL");
    }
    if !matches!(url.port(), None | Some(443)) {
        bail!("出于安全限制，仅允许标准 443 端口");
    }
    let host = url
        .host_str()
        .map(|h| h.to_ascii_lowercase())
        .unwrap_or_default();
    if !ALLOWED_HOSTS.contains(&host.as_str()) {
        bail!(
            "域名 `{host}` 不在安全导入白名单内（仅支持 raw.githubusercontent.com / gist.githubusercontent.com / img.kookapp.cn / file.kookapp.cn / media.kookapp.cn）"
        );
    }
    if url.path().trim_matches('/').is_empty()
        || url.path_segments().is_some_and(|mut s| s.any(|seg| seg == ".."))
    {
        bail!("URL 路径无效或包含非法目录穿越段");
    }
    Ok(url)
}

/// Downloads a SillyTavern `.json`/`.png` asset from an allowlisted HTTPS origin with zero redirects
/// (`Policy::none()`) and a strict 2 MiB streaming cap.
pub async fn fetch_allowlisted_st_bytes(raw_url: &str) -> Result<(Vec<u8>, String)> {
    let url = validate_allowlisted_import_url(raw_url)?;
    let mut resp = shared_safe_client()
        .get(url.clone())
        .send()
        .await
        .context("拉取远程 SillyTavern 资源失败")?;
    let status = resp.status();
    if status.is_redirection() {
        bail!("出于防 SSRF 安全策略，已拒绝远程重定向 ({status})，请直接提供最终直链");
    }
    if !status.is_success() {
        bail!("远程资源返回 HTTP 错误 ({status})");
    }
    if let Some(len) = resp.content_length()
        && len > MAX_REMOTE_ST_BYTES as u64
    {
        bail!("远程文件超过 2 MiB 安全上限，已拒绝下载");
    }

    let mut buf = Vec::new();
    while let Some(chunk) = resp.chunk().await.context("读取远程响应流失败")? {
        if buf.len().saturating_add(chunk.len()) > MAX_REMOTE_ST_BYTES {
            bail!("远程文件实际体积超过 2 MiB 安全上限，已中止下载");
        }
        buf.extend_from_slice(&chunk);
    }
    if buf.is_empty() {
        bail!("远程文件为空");
    }
    let stem = url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .and_then(|name| name.rsplit_once('.').map(|(a, _)| a).or(Some(name)))
        .filter(|s| !s.is_empty())
        .unwrap_or("st_remote")
        .to_string();
    Ok((buf, stem))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_blocks_ssrf_and_allows_trusted_hosts() {
        assert!(validate_allowlisted_import_url("http://raw.githubusercontent.com/a/b.json").is_err());
        assert!(validate_allowlisted_import_url("https://127.0.0.1/secret.json").is_err());
        assert!(validate_allowlisted_import_url("https://169.254.169.254/latest/meta-data").is_err());
        assert!(validate_allowlisted_import_url("https://evil.com/raw.githubusercontent.com/a.json").is_err());
        assert!(validate_allowlisted_import_url("https://user@raw.githubusercontent.com/a/b.json").is_err());
        assert!(validate_allowlisted_import_url("https://raw.githubusercontent.com:8443/a/b.json").is_err());
        assert!(validate_allowlisted_import_url("https://raw.githubusercontent.com/a/%2e%2e/b.json").is_err());
        assert!(validate_allowlisted_import_url("https://raw.githubusercontent.com/org/repo/main/wb.json").is_ok());
        assert!(validate_allowlisted_import_url("https://img.kookapp.cn/attachments/2026/card.png").is_ok());
    }
}
