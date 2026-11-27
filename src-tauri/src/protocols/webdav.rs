//! WebDAV / HTTPS 协议适配器（遵循 RFC 4918）
//!
//! 兼容 Nextcloud、OwnCloud、Alist、群晖 NAS、坚果云等常见 WebDAV 服务：
//! - 支持 Basic 用户名密码认证、Bearer Token / OAuth2 访问令牌与匿名访问；
//! - 命名空间无关 XML 解析器，兼容 `D:` / `d:` / 无前缀等不同服务端的 `PROPFIND` 响应；
//! - 支持目录列举、新建目录（MKCOL）、删除（DELETE）、重命名（MOVE）及流式上传下载。

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use futures_util::StreamExt;
use percent_encoding::{percent_decode_str, utf8_percent_encode, AsciiSet, CONTROLS};
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_LENGTH, CONTENT_TYPE};
use reqwest::{Body, Client, Method};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use url::Url;

use crate::error::{AppError, AppResult};
use crate::models::{normalize_remote_path, AuthType, FileEntry};
use crate::protocols::{ProgressCallback, RemoteFs};

/// WebDAV URL 路径段百分号编码集合
const PATH_ENCODE_SET: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'`')
    .add(b'{')
    .add(b'}');

/// WebDAV 客户端实例
pub struct WebDavClient {
    http: Client,
    base_url: Url,
    auth_header: Option<String>,
}

impl WebDavClient {
    /// 初始化 WebDAV 客户端并通过根目录 `PROPFIND` 验证连通性与凭据
    pub async fn connect(
        endpoint: &str,
        username: &str,
        secret: Option<&str>,
        auth_type: AuthType,
        tls_skip_verify: bool,
    ) -> AppResult<Self> {
        let ep_str = if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
            endpoint.trim_end_matches('/').to_string()
        } else {
            format!("https://{}", endpoint.trim_end_matches('/'))
        };
        let base_url = Url::parse(&ep_str)
            .map_err(|e| AppError::InvalidInput(format!("WebDAV URL 格式错误: {}", e)))?;

        // 按认证类型构造 Authorization 请求头
        let auth_header = match auth_type {
            AuthType::Token => secret
                .filter(|s| !s.is_empty())
                .map(|t| format!("Bearer {}", t)),
            AuthType::Anonymous => None,
            _ => {
                if !username.is_empty() || secret.is_some_and(|s| !s.is_empty()) {
                    let raw = format!("{}:{}", username, secret.unwrap_or(""));
                    Some(format!(
                        "Basic {}",
                        base64::engine::general_purpose::STANDARD.encode(raw.as_bytes())
                    ))
                } else {
                    None
                }
            }
        };

        let http = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(20))
            .danger_accept_invalid_certs(tls_skip_verify)
            .user_agent("xiaorui_FTP/1.0")
            .build()
            .map_err(|e| AppError::Connection(format!("初始化 WebDAV 客户端失败: {}", e)))?;

        let client = Self {
            http,
            base_url,
            auth_header,
        };

        let _ = client.list_dir("/").await?;
        Ok(client)
    }

    /// 拼接 WebDAV 基础路径与远程相对路径，并进行安全的 URL 编码
    fn build_url(&self, remote_path: &str, trailing_slash: bool) -> AppResult<Url> {
        let norm = normalize_remote_path(remote_path);
        let base_path = self.base_url.path().trim_end_matches('/');
        let combined = if norm == "/" {
            if base_path.is_empty() {
                "/".to_string()
            } else {
                format!("{}/", base_path)
            }
        } else {
            let mut full = format!("{}{}", base_path, norm);
            if trailing_slash && !full.ends_with('/') {
                full.push('/');
            }
            full
        };

        let encoded = utf8_percent_encode(&combined, PATH_ENCODE_SET).to_string();
        let mut url = self.base_url.clone();
        url.set_path(&encoded);
        Ok(url)
    }

    /// 生成附带认证信息的默认请求头
    fn default_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(auth) = &self.auth_header {
            if let Ok(hv) = HeaderValue::from_str(auth) {
                headers.insert(AUTHORIZATION, hv);
            }
        }
        headers
    }

    /// 校验 WebDAV HTTP 响应状态码（2xx 与 207 Multi-Status 均视为成功）
    fn check_status(status: reqwest::StatusCode) -> AppResult<()> {
        if status.is_success() || status.as_u16() == 207 {
            return Ok(());
        }
        match status.as_u16() {
            401 | 403 => Err(AppError::Auth(format!(
                "WebDAV 认证或权限不足 ({})",
                status
            ))),
            404 => Err(AppError::NotFound(format!(
                "WebDAV 资源不存在 ({})",
                status
            ))),
            _ => Err(AppError::Protocol(format!(
                "WebDAV 服务返回错误: {}",
                status
            ))),
        }
    }
}

/// 解析 WebDAV `PROPFIND` 返回的 Multi-Status XML 文档
pub fn parse_propfind_xml(
    xml: &str,
    base_url_path: &str,
    requested_remote_path: &str,
) -> Vec<FileEntry> {
    let mut reader = Reader::from_str(xml);
    let mut entries = Vec::new();

    let mut in_response = false;
    let mut in_resourcetype = false;
    let mut current_tag = String::new();

    let mut href = String::new();
    let mut size: u64 = 0;
    let mut modified: Option<i64> = None;
    let mut is_dir = false;

    // 先对基础路径做百分号解码，避免含中文/空格的基础 URL 与解码后的 href 前缀失配
    let base_prefix_decoded = percent_decode_str(base_url_path)
        .decode_utf8_lossy()
        .trim_end_matches('/')
        .to_string();
    let req_norm = normalize_remote_path(requested_remote_path);

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_ascii_lowercase();
                match name.as_str() {
                    "response" => {
                        in_response = true;
                        href.clear();
                        size = 0;
                        modified = None;
                        is_dir = false;
                    }
                    "resourcetype" => in_resourcetype = true,
                    "collection" if in_resourcetype => is_dir = true,
                    _ => current_tag = name,
                }
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_ascii_lowercase();
                if name == "collection" && in_resourcetype {
                    is_dir = true;
                }
            }
            Ok(Event::Text(t)) if in_response => {
                let text = t.unescape().unwrap_or_default().to_string();
                match current_tag.as_str() {
                    "href" => href.push_str(&text),
                    "getcontentlength" => size = text.trim().parse().unwrap_or(0),
                    "getlastmodified" => {
                        modified = chrono::DateTime::parse_from_rfc2822(text.trim())
                            .or_else(|_| chrono::DateTime::parse_from_rfc3339(text.trim()))
                            .ok()
                            .map(|dt| dt.timestamp());
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_ascii_lowercase();
                if name == "resourcetype" {
                    in_resourcetype = false;
                } else if name == "response" {
                    in_response = false;
                    let raw_path = if href.starts_with("http://") || href.starts_with("https://") {
                        Url::parse(&href)
                            .map(|u| u.path().to_string())
                            .unwrap_or_else(|_| href.clone())
                    } else {
                        href.clone()
                    };
                    let decoded = percent_decode_str(&raw_path)
                        .decode_utf8_lossy()
                        .to_string();
                    let stripped = if !base_prefix_decoded.is_empty() {
                        decoded
                            .strip_prefix(base_prefix_decoded.as_str())
                            .unwrap_or(decoded.as_str())
                    } else {
                        decoded.as_str()
                    };
                    let entry_path = normalize_remote_path(stripped);
                    // 过滤掉当前目录自身，仅保留子项
                    if entry_path != req_norm && entry_path != "/" {
                        let name_only = entry_path.rsplit('/').next().unwrap_or("").to_string();
                        let entry_is_dir = is_dir || raw_path.ends_with('/');
                        if !name_only.is_empty() {
                            entries.push(FileEntry {
                                name: name_only,
                                path: entry_path,
                                is_dir: entry_is_dir,
                                size: if entry_is_dir { 0 } else { size },
                                modified,
                                permissions: None,
                                mode: None,
                                is_symlink: false,
                            });
                        }
                    }
                }
                current_tag.clear();
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }

    entries.sort_by(|a, b| {
        b.is_dir
            .cmp(&a.is_dir)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    entries
}

#[async_trait]
impl RemoteFs for WebDavClient {
    async fn list_dir(&self, path: &str) -> AppResult<Vec<FileEntry>> {
        let url = self.build_url(path, true)?;
        let method = Method::from_bytes(b"PROPFIND").unwrap();
        let mut headers = self.default_headers();
        headers.insert("Depth", HeaderValue::from_static("1"));
        headers.insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/xml; charset=utf-8"),
        );

        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<D:propfind xmlns:D="DAV:">
  <D:prop>
    <D:resourcetype/>
    <D:getcontentlength/>
    <D:getlastmodified/>
  </D:prop>
</D:propfind>"#;

        let resp = self
            .http
            .request(method, url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|e| AppError::Connection(format!("WebDAV PROPFIND 失败: {}", e)))?;

        Self::check_status(resp.status())?;
        let xml = resp
            .text()
            .await
            .map_err(|e| AppError::Protocol(format!("读取 WebDAV 响应失败: {}", e)))?;

        Ok(parse_propfind_xml(&xml, self.base_url.path(), path))
    }

    async fn mkdir(&self, path: &str) -> AppResult<()> {
        let url = self.build_url(path, true)?;
        let method = Method::from_bytes(b"MKCOL").unwrap();
        let resp = self
            .http
            .request(method, url)
            .headers(self.default_headers())
            .send()
            .await
            .map_err(|e| AppError::Connection(format!("WebDAV MKCOL 失败: {}", e)))?;
        Self::check_status(resp.status())
    }

    async fn remove(&self, path: &str, is_dir: bool) -> AppResult<()> {
        let url = self.build_url(path, is_dir)?;
        let resp = self
            .http
            .delete(url)
            .headers(self.default_headers())
            .send()
            .await
            .map_err(|e| AppError::Connection(format!("WebDAV DELETE 失败: {}", e)))?;
        Self::check_status(resp.status())
    }

    async fn rename(&self, from: &str, to: &str) -> AppResult<()> {
        let src_url = self.build_url(from, false)?;
        let dst_url = self.build_url(to, false)?;
        let method = Method::from_bytes(b"MOVE").unwrap();
        let mut headers = self.default_headers();
        headers.insert(
            "Destination",
            HeaderValue::from_str(dst_url.as_str())
                .map_err(|e| AppError::Protocol(format!("无效目标地址: {}", e)))?,
        );
        headers.insert("Overwrite", HeaderValue::from_static("T"));

        let resp = self
            .http
            .request(method, src_url)
            .headers(headers)
            .send()
            .await
            .map_err(|e| AppError::Connection(format!("WebDAV MOVE 失败: {}", e)))?;
        Self::check_status(resp.status())
    }

    async fn read_bytes(&self, path: &str, max_bytes: usize) -> AppResult<Vec<u8>> {
        let url = self.build_url(path, false)?;
        let resp = self
            .http
            .get(url)
            .headers(self.default_headers())
            .send()
            .await
            .map_err(|e| AppError::Connection(format!("WebDAV GET 失败: {}", e)))?;
        Self::check_status(resp.status())?;

        let mut stream = resp.bytes_stream();
        let mut buf = Vec::new();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| AppError::Protocol(format!("读取流失败: {}", e)))?;
            buf.extend_from_slice(&bytes);
            if buf.len() > max_bytes {
                return Err(AppError::InvalidInput(format!(
                    "文件超过在线预览大小限制 ({} KB)",
                    max_bytes / 1024
                )));
            }
        }
        Ok(buf)
    }

    async fn write_bytes(&self, path: &str, data: &[u8]) -> AppResult<()> {
        let url = self.build_url(path, false)?;
        let resp = self
            .http
            .put(url)
            .headers(self.default_headers())
            .body(data.to_vec())
            .send()
            .await
            .map_err(|e| AppError::Connection(format!("WebDAV PUT 失败: {}", e)))?;
        Self::check_status(resp.status())
    }

    async fn download_file(
        &self,
        remote_path: &str,
        local_path: &Path,
        progress: ProgressCallback,
        cancel: Arc<AtomicBool>,
    ) -> AppResult<u64> {
        let url = self.build_url(remote_path, false)?;
        let resp = self
            .http
            .get(url)
            .headers(self.default_headers())
            .send()
            .await
            .map_err(|e| AppError::Connection(format!("WebDAV 下载失败: {}", e)))?;
        Self::check_status(resp.status())?;

        let total = resp.content_length().unwrap_or(0);
        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let mut file = tokio::fs::File::create(local_path).await?;
        let mut stream = resp.bytes_stream();
        let mut transferred: u64 = 0;

        while let Some(item) = stream.next().await {
            if cancel.load(Ordering::Relaxed) {
                drop(file);
                let _ = tokio::fs::remove_file(local_path).await;
                return Err(AppError::Cancelled);
            }
            let chunk = item.map_err(|e| AppError::Protocol(format!("下载中断: {}", e)))?;
            file.write_all(&chunk).await?;
            transferred += chunk.len() as u64;
            progress(transferred, total.max(transferred));
        }
        file.flush().await?;
        Ok(transferred)
    }

    async fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &str,
        progress: ProgressCallback,
        cancel: Arc<AtomicBool>,
    ) -> AppResult<u64> {
        if cancel.load(Ordering::Relaxed) {
            return Err(AppError::Cancelled);
        }
        let meta = tokio::fs::metadata(local_path).await?;
        let total = meta.len();
        let file = tokio::fs::File::open(local_path).await?;
        let url = self.build_url(remote_path, false)?;

        let mut headers = self.default_headers();
        headers.insert(CONTENT_LENGTH, HeaderValue::from(total));

        // 64 KB 流式分块上传，恒定低内存占用并支持中途取消与实时速率汇报
        let sent = Arc::new(AtomicU64::new(0));
        let sent_cb = sent.clone();
        let cancel_cb = cancel.clone();
        let progress_cb = progress.clone();

        let stream = ReaderStream::with_capacity(file, 64 * 1024).map(move |chunk| {
            if cancel_cb.load(Ordering::Relaxed) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Interrupted,
                    "cancelled",
                ));
            }
            let bytes = chunk?;
            let done =
                sent_cb.fetch_add(bytes.len() as u64, Ordering::Relaxed) + bytes.len() as u64;
            progress_cb(done, total.max(done));
            Ok(bytes)
        });

        let resp = self
            .http
            .put(url)
            .headers(headers)
            .body(Body::wrap_stream(stream))
            .send()
            .await
            .map_err(|e| {
                if cancel.load(Ordering::Relaxed) {
                    AppError::Cancelled
                } else {
                    AppError::Connection(format!("WebDAV 流式上传失败: {}", e))
                }
            })?;
        Self::check_status(resp.status())?;

        let final_bytes = sent.load(Ordering::Relaxed).max(total);
        progress(final_bytes, total.max(final_bytes));
        Ok(final_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_propfind_xml() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
        <D:multistatus xmlns:D="DAV:">
          <D:response>
            <D:href>/dav/</D:href>
            <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat>
          </D:response>
          <D:response>
            <D:href>/dav/Photos/</D:href>
            <D:propstat><D:prop><D:resourcetype><D:collection/></D:resourcetype></D:prop></D:propstat>
          </D:response>
          <D:response>
            <D:href>/dav/hello%20world.txt</D:href>
            <D:propstat><D:prop>
              <D:resourcetype/>
              <D:getcontentlength>512</D:getcontentlength>
              <D:getlastmodified>Thu, 01 May 2025 10:00:00 GMT</D:getlastmodified>
            </D:prop></D:propstat>
          </D:response>
        </D:multistatus>"#;
        let entries = parse_propfind_xml(xml, "/dav", "/");
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].name, "Photos");
        assert!(!entries[1].is_dir);
        assert_eq!(entries[1].name, "hello world.txt");
        assert_eq!(entries[1].size, 512);
    }

    #[test]
    fn test_parse_propfind_xml_encoded_chinese_base_path() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
        <d:multistatus xmlns:d="DAV:">
          <d:response>
            <d:href>/%E6%88%91%E7%9A%84%E7%BD%91%E7%9B%98/</d:href>
            <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat>
          </d:response>
          <d:response>
            <d:href>/%E6%88%91%E7%9A%84%E7%BD%91%E7%9B%98/%E6%96%87%E6%A1%A3.txt</d:href>
            <d:propstat><d:prop>
              <d:resourcetype/>
              <d:getcontentlength>128</d:getcontentlength>
            </d:prop></d:propstat>
          </d:response>
        </d:multistatus>"#;
        let entries = parse_propfind_xml(xml, "/%E6%88%91%E7%9A%84%E7%BD%91%E7%9B%98", "/");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "文档.txt");
        assert_eq!(entries[0].path, "/文档.txt");
        assert_eq!(entries[0].size, 128);
    }

    #[test]
    fn test_webdav_check_status() {
        assert!(WebDavClient::check_status(reqwest::StatusCode::OK).is_ok());
        assert!(WebDavClient::check_status(reqwest::StatusCode::MULTI_STATUS).is_ok());
        assert!(matches!(
            WebDavClient::check_status(reqwest::StatusCode::UNAUTHORIZED),
            Err(AppError::Auth(_))
        ));
        assert!(matches!(
            WebDavClient::check_status(reqwest::StatusCode::NOT_FOUND),
            Err(AppError::NotFound(_))
        ));
    }
}
