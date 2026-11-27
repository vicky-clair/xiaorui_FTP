//! S3 兼容对象存储协议适配器（纯 Rust AWS Signature V4 实现）
//!
//! 兼容 AWS S3、阿里云 OSS、腾讯云 COS、MinIO、Cloudflare R2、Backblaze B2 等服务：
//! - 支持自定义 Endpoint、Virtual-Host 与 Path-Style 访问模式；
//! - 支持标准 AccessKey/SecretKey 及可选 STS 临时安全令牌（`x-amz-security-token`）；
//! - 自动将对象前缀（`CommonPrefixes`）映射为直观的虚拟目录层级。

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use futures_util::StreamExt;
use hmac::{Hmac, Mac};
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use quick_xml::events::Event;
use quick_xml::Reader;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_LENGTH, HOST};
use reqwest::{Body, Client, Method};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio_util::io::ReaderStream;
use url::Url;

use crate::error::{AppError, AppResult};
use crate::models::{normalize_remote_path, FileEntry};
use crate::protocols::{ProgressCallback, RemoteFs};
use crate::security::{commit_staging_file, ensure_not_symlink_target, staging_part_path};

type HmacSha256 = Hmac<Sha256>;

/// AWS SigV4 规范 URI 编码字符集（保留 A-Z、a-z、0-9、'-'、'_'、'.'、'~'）
const AWS_ENCODE_SET: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// 路径编码字符集（在 SigV4 基础上保留路径分隔符 `/`）
const AWS_PATH_ENCODE_SET: &AsciiSet = &AWS_ENCODE_SET.remove(b'/');

/// S3 兼容对象存储客户端（释放时自动覆写清零 SecretKey 与 STS 令牌）
pub struct S3Client {
    http: Client,
    endpoint: Url,
    bucket: String,
    region: String,
    access_key: String,
    secret_key: String,
    session_token: Option<String>,
    path_style: bool,
}

impl Drop for S3Client {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.secret_key.zeroize();
        if let Some(tok) = self.session_token.as_mut() {
            tok.zeroize();
        }
    }
}

impl S3Client {
    /// 初始化 S3 客户端并尝试列举根目录以验证凭据与存储桶可访问性
    #[allow(clippy::too_many_arguments)]
    pub async fn connect(
        endpoint: &str,
        bucket: &str,
        region: &str,
        access_key: &str,
        secret_key: &str,
        session_token: Option<String>,
        path_style: bool,
        tls_skip_verify: bool,
    ) -> AppResult<Self> {
        let ep_str = if endpoint.starts_with("http://") || endpoint.starts_with("https://") {
            endpoint.trim_end_matches('/').to_string()
        } else {
            format!("https://{}", endpoint.trim_end_matches('/'))
        };
        let endpoint_url = Url::parse(&ep_str)
            .map_err(|e| AppError::InvalidInput(format!("S3 Endpoint 格式错误: {}", e)))?;

        if bucket.trim().is_empty() {
            return Err(AppError::InvalidInput(
                "S3 存储桶 (Bucket) 名称不能为空".into(),
            ));
        }

        let http = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(20))
            .danger_accept_invalid_certs(tls_skip_verify)
            .user_agent("xiaorui_FTP/1.0")
            .build()
            .map_err(|e| AppError::Connection(format!("初始化 HTTP 客户端失败: {}", e)))?;

        let client = Self {
            http,
            endpoint: endpoint_url,
            bucket: bucket.trim().to_string(),
            region: if region.trim().is_empty() {
                "us-east-1".to_string()
            } else {
                region.trim().to_string()
            },
            access_key: access_key.to_string(),
            secret_key: secret_key.to_string(),
            session_token,
            path_style,
        };

        // 预检根目录以尽早暴露凭据或 Bucket 名称错误
        let _ = client.list_dir("/").await?;
        Ok(client)
    }

    /// 将前端统一远程路径转换为 S3 对象 Key（去除前导 `/`）
    fn object_key(remote_path: &str) -> String {
        let norm = normalize_remote_path(remote_path);
        norm.trim_start_matches('/').to_string()
    }

    /// 按 Path-Style 或 Virtual-Host 模式构造最终请求 URL、Host 头与规范查询串
    fn build_url_and_host(
        &self,
        key: &str,
        query: &[(String, String)],
    ) -> AppResult<(Url, String, String)> {
        let scheme = self.endpoint.scheme();
        let base_host = self
            .endpoint
            .host_str()
            .ok_or_else(|| AppError::InvalidInput("Endpoint 缺少主机名".into()))?;
        let port_part = self
            .endpoint
            .port()
            .map(|p| format!(":{}", p))
            .unwrap_or_default();

        let (host_header, raw_path) = if self.path_style {
            let h = format!("{}{}", base_host, port_part);
            let p = if key.is_empty() {
                format!("/{}", self.bucket)
            } else {
                format!("/{}/{}", self.bucket, key)
            };
            (h, p)
        } else {
            let h = format!("{}.{}{}", self.bucket, base_host, port_part);
            let p = if key.is_empty() {
                "/".to_string()
            } else {
                format!("/{}", key)
            };
            (h, p)
        };

        let encoded_path = utf8_percent_encode(&raw_path, AWS_PATH_ENCODE_SET).to_string();
        let mut sorted_q = query.to_vec();
        sorted_q.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        let canonical_query = sorted_q
            .iter()
            .map(|(k, v)| {
                format!(
                    "{}={}",
                    utf8_percent_encode(k, AWS_ENCODE_SET),
                    utf8_percent_encode(v, AWS_ENCODE_SET)
                )
            })
            .collect::<Vec<_>>()
            .join("&");

        let full_url = if canonical_query.is_empty() {
            format!("{}://{}{}", scheme, host_header, encoded_path)
        } else {
            format!(
                "{}://{}{}?{}",
                scheme, host_header, encoded_path, canonical_query
            )
        };

        let url = Url::parse(&full_url)
            .map_err(|e| AppError::Protocol(format!("构造 S3 请求 URL 失败: {}", e)))?;

        Ok((url, host_header, canonical_query))
    }

    /// 计算 AWS Signature V4 签名并生成完整请求头
    fn sign_request(
        &self,
        method: &Method,
        canonical_uri: &str,
        canonical_query: &str,
        host_header: &str,
        payload_hash: &str,
        extra_headers: &[(&str, String)],
    ) -> AppResult<HeaderMap> {
        let now = Utc::now();
        let amz_date = now.format("%Y%m%dT%H%M%SZ").to_string();
        let date_stamp = now.format("%Y%m%d").to_string();

        let mut sig_headers: Vec<(String, String)> = vec![
            ("host".to_string(), host_header.to_string()),
            ("x-amz-content-sha256".to_string(), payload_hash.to_string()),
            ("x-amz-date".to_string(), amz_date.clone()),
        ];

        if let Some(token) = &self.session_token {
            if !token.is_empty() {
                sig_headers.push(("x-amz-security-token".to_string(), token.clone()));
            }
        }
        for (k, v) in extra_headers {
            sig_headers.push((k.to_ascii_lowercase(), v.trim().to_string()));
        }

        sig_headers.sort_by(|a, b| a.0.cmp(&b.0));

        let canonical_headers = sig_headers
            .iter()
            .map(|(k, v)| format!("{}:{}\n", k, v))
            .collect::<String>();
        let signed_headers = sig_headers
            .iter()
            .map(|(k, _)| k.as_str())
            .collect::<Vec<_>>()
            .join(";");

        let canonical_request = format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            method.as_str(),
            canonical_uri,
            canonical_query,
            canonical_headers,
            signed_headers,
            payload_hash
        );

        let cr_hash = hex::encode(Sha256::digest(canonical_request.as_bytes()));
        let credential_scope = format!("{}/{}/s3/aws4_request", date_stamp, self.region);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{}\n{}\n{}",
            amz_date, credential_scope, cr_hash
        );

        let k_date = hmac_sha256(
            format!("AWS4{}", self.secret_key).as_bytes(),
            date_stamp.as_bytes(),
        );
        let k_region = hmac_sha256(&k_date, self.region.as_bytes());
        let k_service = hmac_sha256(&k_region, b"s3");
        let k_signing = hmac_sha256(&k_service, b"aws4_request");
        let signature = hex::encode(hmac_sha256(&k_signing, string_to_sign.as_bytes()));

        let auth_header = format!(
            "AWS4-HMAC-SHA256 Credential={}/{}, SignedHeaders={}, Signature={}",
            self.access_key, credential_scope, signed_headers, signature
        );

        let mut headers = HeaderMap::new();
        headers.insert(
            HOST,
            HeaderValue::from_str(host_header)
                .map_err(|e| AppError::Protocol(format!("无效 Host 头: {}", e)))?,
        );
        headers.insert(
            HeaderName::from_static("x-amz-date"),
            HeaderValue::from_str(&amz_date).unwrap(),
        );
        headers.insert(
            HeaderName::from_static("x-amz-content-sha256"),
            HeaderValue::from_str(payload_hash).unwrap(),
        );
        headers.insert(
            HeaderName::from_static("authorization"),
            HeaderValue::from_str(&auth_header)
                .map_err(|e| AppError::Protocol(format!("无效 Authorization 头: {}", e)))?,
        );
        if let Some(token) = &self.session_token {
            if !token.is_empty() {
                headers.insert(
                    HeaderName::from_static("x-amz-security-token"),
                    HeaderValue::from_str(token)
                        .map_err(|e| AppError::Protocol(format!("无效 SessionToken: {}", e)))?,
                );
            }
        }
        for (k, v) in extra_headers {
            let hn = HeaderName::from_bytes(k.as_bytes())
                .map_err(|e| AppError::Protocol(format!("无效请求头名称 {}: {}", k, e)))?;
            let hv = HeaderValue::from_str(v)
                .map_err(|e| AppError::Protocol(format!("无效请求头值 {}: {}", k, e)))?;
            headers.insert(hn, hv);
        }

        Ok(headers)
    }

    /// 发送带 SigV4 签名的 S3 HTTP 请求并统一转换错误状态码
    async fn send_signed(
        &self,
        method: Method,
        key: &str,
        query: &[(String, String)],
        body: Vec<u8>,
        extra_headers: &[(&str, String)],
    ) -> AppResult<reqwest::Response> {
        let (url, host_header, canonical_query) = self.build_url_and_host(key, query)?;
        let payload_hash = hex::encode(Sha256::digest(&body));
        let headers = self.sign_request(
            &method,
            url.path(),
            &canonical_query,
            &host_header,
            &payload_hash,
            extra_headers,
        )?;

        let resp = self
            .http
            .request(method, url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|e| AppError::Connection(format!("S3 请求失败: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 403 || status.as_u16() == 401 {
                return Err(AppError::Auth(format!(
                    "S3 认证被拒绝 ({}): {}",
                    status,
                    extract_s3_error(&err_text)
                )));
            }
            if status.as_u16() == 404 {
                return Err(AppError::NotFound(format!(
                    "S3 对象或存储桶不存在 ({})",
                    status
                )));
            }
            return Err(AppError::Protocol(format!(
                "S3 返回错误 {}: {}",
                status,
                extract_s3_error(&err_text)
            )));
        }

        Ok(resp)
    }
}

/// 计算 HMAC-SHA256 摘要
fn hmac_sha256(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC 密钥长度合法");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// 从 S3 XML 错误响应体中提取可读的 `Code` 与 `Message`
fn extract_s3_error(xml: &str) -> String {
    let mut reader = Reader::from_str(xml);
    let mut in_msg = false;
    let mut in_code = false;
    let mut code = String::new();
    let mut msg = String::new();
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                b"Code" => in_code = true,
                b"Message" => in_msg = true,
                _ => {}
            },
            Ok(Event::Text(t)) => {
                let s = t.unescape().unwrap_or_default().to_string();
                if in_code {
                    code.push_str(&s);
                } else if in_msg {
                    msg.push_str(&s);
                }
            }
            Ok(Event::End(e)) => match e.name().as_ref() {
                b"Code" => in_code = false,
                b"Message" => in_msg = false,
                _ => {}
            },
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }
    if !code.is_empty() || !msg.is_empty() {
        format!("{} - {}", code, msg)
    } else {
        xml.chars().take(200).collect()
    }
}

/// 解析 S3 `ListObjectsV2` 单页 XML 响应，返回本页条目与可选的下一页 `NextContinuationToken`
pub fn parse_list_objects_v2_page(xml: &str, prefix: &str) -> (Vec<FileEntry>, Option<String>) {
    let mut reader = Reader::from_str(xml);
    let mut entries = Vec::new();

    let mut in_contents = false;
    let mut in_common_prefix = false;
    let mut current_tag = String::new();

    let mut key = String::new();
    let mut size: u64 = 0;
    let mut modified: Option<i64> = None;
    let mut is_truncated = false;
    let mut next_token = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                if name == "Contents" {
                    in_contents = true;
                    key.clear();
                    size = 0;
                    modified = None;
                } else if name == "CommonPrefixes" {
                    in_common_prefix = true;
                    key.clear();
                } else {
                    current_tag = name;
                }
            }
            Ok(Event::Text(t)) => {
                let text = t.unescape().unwrap_or_default().to_string();
                if in_contents {
                    match current_tag.as_str() {
                        "Key" => key.push_str(&text),
                        "Size" => size = text.trim().parse().unwrap_or(0),
                        "LastModified" => {
                            modified = chrono::DateTime::parse_from_rfc3339(text.trim())
                                .ok()
                                .map(|dt| dt.timestamp());
                        }
                        _ => {}
                    }
                } else if in_common_prefix && current_tag == "Prefix" {
                    key.push_str(&text);
                } else {
                    match current_tag.as_str() {
                        "IsTruncated" => is_truncated = text.trim().eq_ignore_ascii_case("true"),
                        "NextContinuationToken" => next_token.push_str(text.trim()),
                        _ => {}
                    }
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                if name == "Contents" {
                    in_contents = false;
                    // 跳过与当前前缀相同的目录占位对象
                    if key != prefix && !key.is_empty() {
                        let rel = key.strip_prefix(prefix).unwrap_or(&key);
                        let is_dir_marker = rel.ends_with('/');
                        let clean_name = rel.trim_end_matches('/').to_string();
                        if !clean_name.is_empty() && !clean_name.contains('/') {
                            entries.push(FileEntry {
                                name: clean_name,
                                path: format!("/{}", key.trim_end_matches('/')),
                                is_dir: is_dir_marker,
                                size: if is_dir_marker { 0 } else { size },
                                modified,
                                permissions: None,
                                mode: None,
                                is_symlink: false,
                            });
                        }
                    }
                } else if name == "CommonPrefixes" {
                    in_common_prefix = false;
                    let rel = key.strip_prefix(prefix).unwrap_or(&key);
                    let clean_name = rel.trim_end_matches('/').to_string();
                    if !clean_name.is_empty() {
                        entries.push(FileEntry {
                            name: clean_name,
                            path: format!("/{}", key.trim_end_matches('/')),
                            is_dir: true,
                            size: 0,
                            modified: None,
                            permissions: None,
                            mode: None,
                            is_symlink: false,
                        });
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
    let token = if is_truncated && !next_token.is_empty() {
        Some(next_token)
    } else {
        None
    };
    (entries, token)
}

/// 解析 S3 `ListObjectsV2` XML 中的全部对象 Key 与分页续传 Token（用于目录递归删除）
pub fn parse_list_keys_page(xml: &str) -> (Vec<String>, Option<String>) {
    let mut reader = Reader::from_str(xml);
    let mut keys = Vec::new();
    let mut current_tag = String::new();
    let mut key_buf = String::new();
    let mut is_truncated = false;
    let mut next_token = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                current_tag = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                if current_tag == "Key" {
                    key_buf.clear();
                }
            }
            Ok(Event::Text(t)) => {
                let s = t.unescape().unwrap_or_default().to_string();
                match current_tag.as_str() {
                    "Key" => key_buf.push_str(&s),
                    "IsTruncated" => is_truncated = s.trim().eq_ignore_ascii_case("true"),
                    "NextContinuationToken" => next_token.push_str(s.trim()),
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.local_name().as_ref()).to_string();
                if name == "Key" && !key_buf.is_empty() {
                    keys.push(std::mem::take(&mut key_buf));
                }
                current_tag.clear();
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
    }

    let token = if is_truncated && !next_token.is_empty() {
        Some(next_token)
    } else {
        None
    };
    (keys, token)
}

#[async_trait]
impl RemoteFs for S3Client {
    async fn list_dir(&self, path: &str) -> AppResult<Vec<FileEntry>> {
        let raw_key = Self::object_key(path);
        let prefix = if raw_key.is_empty() {
            String::new()
        } else {
            format!("{}/", raw_key.trim_end_matches('/'))
        };

        // 循环拉取所有分页，避免单目录超过 1000 项时显示不全
        let mut all_entries = Vec::new();
        let mut continuation: Option<String> = None;

        loop {
            let mut query = vec![
                ("list-type".to_string(), "2".to_string()),
                ("delimiter".to_string(), "/".to_string()),
                ("max-keys".to_string(), "1000".to_string()),
            ];
            if !prefix.is_empty() {
                query.push(("prefix".to_string(), prefix.clone()));
            }
            if let Some(tok) = &continuation {
                query.push(("continuation-token".to_string(), tok.clone()));
            }

            let resp = self
                .send_signed(Method::GET, "", &query, Vec::new(), &[])
                .await?;
            let xml = resp
                .text()
                .await
                .map_err(|e| AppError::Protocol(format!("读取 S3 响应失败: {}", e)))?;

            let (mut page_entries, next_tok) = parse_list_objects_v2_page(&xml, &prefix);
            all_entries.append(&mut page_entries);
            match next_tok {
                Some(t) => continuation = Some(t),
                None => break,
            }
        }

        // 按 path 去重（防止目录占位对象与 CommonPrefixes 同时返回造成重复条目）
        let mut deduped: std::collections::HashMap<String, FileEntry> =
            std::collections::HashMap::with_capacity(all_entries.len());
        for entry in all_entries {
            deduped
                .entry(entry.path.clone())
                .and_modify(|existing| {
                    if entry.is_dir && !existing.is_dir {
                        *existing = entry.clone();
                    } else if existing.modified.is_none() && entry.modified.is_some() {
                        existing.modified = entry.modified;
                    }
                })
                .or_insert(entry);
        }
        let mut unique_entries: Vec<FileEntry> = deduped.into_values().collect();
        unique_entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        Ok(unique_entries)
    }

    async fn mkdir(&self, path: &str) -> AppResult<()> {
        let obj_key = Self::object_key(path);
        let trimmed = obj_key.trim_end_matches('/');
        if trimmed.is_empty() {
            return Err(AppError::InvalidInput("目录名称不能为空".into()));
        }
        let key = format!("{}/", trimmed);
        let _ = self
            .send_signed(Method::PUT, &key, &[], Vec::new(), &[])
            .await?;
        Ok(())
    }

    async fn remove(&self, path: &str, is_dir: bool) -> AppResult<()> {
        let key = Self::object_key(path);
        if is_dir {
            // 使用 continuation-token 循环分页列举并删除前缀下的全部子对象，显式传播删除错误
            let prefix = format!("{}/", key.trim_end_matches('/'));
            let mut continuation: Option<String> = None;

            loop {
                let mut query = vec![
                    ("list-type".to_string(), "2".to_string()),
                    ("max-keys".to_string(), "1000".to_string()),
                    ("prefix".to_string(), prefix.clone()),
                ];
                if let Some(tok) = &continuation {
                    query.push(("continuation-token".to_string(), tok.clone()));
                }

                let resp = self
                    .send_signed(Method::GET, "", &query, Vec::new(), &[])
                    .await?;
                let xml = resp
                    .text()
                    .await
                    .map_err(|e| AppError::Protocol(format!("读取 S3 列表响应失败: {}", e)))?;

                let (keys, next_tok) = parse_list_keys_page(&xml);
                for k in keys {
                    self.send_signed(Method::DELETE, &k, &[], Vec::new(), &[])
                        .await?;
                }

                match next_tok {
                    Some(t) => continuation = Some(t),
                    None => break,
                }
            }

            // 清理可能存在的目录占位对象（若该占位对象不存在则忽略 404，其他错误照常返回）
            if let Err(e) = self
                .send_signed(Method::DELETE, &prefix, &[], Vec::new(), &[])
                .await
            {
                if !matches!(e, AppError::NotFound(_)) {
                    return Err(e);
                }
            }
        } else {
            let _ = self
                .send_signed(Method::DELETE, &key, &[], Vec::new(), &[])
                .await?;
        }
        Ok(())
    }

    async fn rename(&self, from: &str, to: &str) -> AppResult<()> {
        // S3 通过 CopyObject + DeleteObject 实现对象重命名
        let src_key = Self::object_key(from);
        let dst_key = Self::object_key(to);
        let copy_source = utf8_percent_encode(
            &format!("/{}/{}", self.bucket, src_key),
            AWS_PATH_ENCODE_SET,
        )
        .to_string();

        let _ = self
            .send_signed(
                Method::PUT,
                &dst_key,
                &[],
                Vec::new(),
                &[("x-amz-copy-source", copy_source)],
            )
            .await?;
        let _ = self
            .send_signed(Method::DELETE, &src_key, &[], Vec::new(), &[])
            .await?;
        Ok(())
    }

    async fn read_bytes(&self, path: &str, max_bytes: usize) -> AppResult<Vec<u8>> {
        let key = Self::object_key(path);
        let resp = self
            .send_signed(Method::GET, &key, &[], Vec::new(), &[])
            .await?;
        let mut stream = resp.bytes_stream();
        let mut buf = Vec::new();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(|e| AppError::Protocol(format!("读取对象流失败: {}", e)))?;
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
        let key = Self::object_key(path);
        let _ = self
            .send_signed(Method::PUT, &key, &[], data.to_vec(), &[])
            .await?;
        Ok(())
    }

    async fn download_file(
        &self,
        remote_path: &str,
        local_path: &Path,
        progress: ProgressCallback,
        cancel: Arc<AtomicBool>,
    ) -> AppResult<u64> {
        ensure_not_symlink_target(local_path)?;
        let key = Self::object_key(remote_path);
        let resp = self
            .send_signed(Method::GET, &key, &[], Vec::new(), &[])
            .await?;
        let total = resp.content_length().unwrap_or(0);
        if let Some(parent) = local_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let part_path = staging_part_path(local_path);
        let mut file = tokio::fs::File::create(&part_path).await?;
        let mut stream = resp.bytes_stream();
        let mut transferred: u64 = 0;

        let res: AppResult<()> = async {
            while let Some(item) = stream.next().await {
                if cancel.load(Ordering::Relaxed) {
                    return Err(AppError::Cancelled);
                }
                let chunk = item.map_err(|e| AppError::Protocol(format!("下载流中断: {}", e)))?;
                file.write_all(&chunk).await?;
                transferred += chunk.len() as u64;
                progress(transferred, total.max(transferred));
            }
            file.flush().await?;
            Ok(())
        }
        .await;

        drop(file);
        if let Err(err) = res {
            let _ = tokio::fs::remove_file(&part_path).await;
            return Err(err);
        }
        commit_staging_file(&part_path, local_path)?;
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

        let key = Self::object_key(remote_path);
        let (url, host_header, canonical_query) = self.build_url_and_host(&key, &[])?;

        // 采用 SigV4 UNSIGNED-PAYLOAD + 64 KB 流式上传，恒定低内存占用并支持中途取消与实时进度
        let mut headers = self.sign_request(
            &Method::PUT,
            url.path(),
            &canonical_query,
            &host_header,
            "UNSIGNED-PAYLOAD",
            &[],
        )?;
        headers.insert(CONTENT_LENGTH, HeaderValue::from(total));

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
            .request(Method::PUT, url)
            .headers(headers)
            .body(Body::wrap_stream(stream))
            .send()
            .await
            .map_err(|e| {
                if cancel.load(Ordering::Relaxed) {
                    AppError::Cancelled
                } else {
                    AppError::Connection(format!("S3 流式上传失败: {}", e))
                }
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let err_text = resp.text().await.unwrap_or_default();
            if status.as_u16() == 403 || status.as_u16() == 401 {
                return Err(AppError::Auth(format!(
                    "S3 认证被拒绝 ({}): {}",
                    status,
                    extract_s3_error(&err_text)
                )));
            }
            return Err(AppError::Protocol(format!(
                "S3 上传返回错误 {}: {}",
                status,
                extract_s3_error(&err_text)
            )));
        }

        let final_bytes = sent.load(Ordering::Relaxed).max(total);
        progress(final_bytes, total.max(final_bytes));
        Ok(final_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_s3_list_xml_and_pagination() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <ListBucketResult xmlns="http://s3.amazonaws.com/doc/2006-03-01/">
            <Name>demo</Name>
            <Prefix>docs/</Prefix>
            <IsTruncated>true</IsTruncated>
            <NextContinuationToken>token-page-2</NextContinuationToken>
            <CommonPrefixes><Prefix>docs/images/</Prefix></CommonPrefixes>
            <Contents>
                <Key>docs/readme.md</Key>
                <LastModified>2025-05-01T12:00:00.000Z</LastModified>
                <Size>2048</Size>
            </Contents>
        </ListBucketResult>"#;
        let (entries, token) = parse_list_objects_v2_page(xml, "docs/");
        assert_eq!(entries.len(), 2);
        assert!(entries[0].is_dir);
        assert_eq!(entries[0].name, "images");
        assert!(!entries[1].is_dir);
        assert_eq!(entries[1].name, "readme.md");
        assert_eq!(entries[1].size, 2048);
        assert_eq!(token.as_deref(), Some("token-page-2"));

        let (keys, key_token) = parse_list_keys_page(xml);
        assert_eq!(keys, vec!["docs/readme.md"]);
        assert_eq!(key_token.as_deref(), Some("token-page-2"));
    }

    #[test]
    fn test_extract_s3_error_xml() {
        let err_xml =
            r#"<Error><Code>AccessDenied</Code><Message>Invalid Access Key</Message></Error>"#;
        assert_eq!(
            extract_s3_error(err_xml),
            "AccessDenied - Invalid Access Key"
        );
    }
}
