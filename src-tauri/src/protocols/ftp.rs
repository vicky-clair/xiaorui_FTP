//! 现代 FTP / FTPS 协议适配器（基于 `suppaftp` + 纯 Rust `rustls`）
//!
//! 特性：
//! - 显式 FTPS（`AUTH TLS`）与隐式 FTPS（默认 990 端口），原生支持 TLS 1.2 / TLS 1.3；
//! - 支持可选跳过自签名证书校验（仅限内网环境）；
//! - 自动优先使用扩展被动模式（EPSV），异常时自动回退标准被动模式（PASV）；
//! - 通过阻塞线程池隔离同步流操作，不阻塞 Tokio 异步运行时。

use std::io::{Cursor, Read, Write};
use std::path::Path;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::UNIX_EPOCH;

use async_trait::async_trait;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{ClientConfig, DigitallySignedStruct, RootCertStore, SignatureScheme};
use suppaftp::list::File as FtpListFile;
use suppaftp::types::FileType;
use suppaftp::{Mode, RustlsConnector, RustlsFtpStream};

use crate::error::{AppError, AppResult};
use crate::models::{join_remote_path, normalize_remote_path, FileEntry, Protocol};
use crate::protocols::{ProgressCallback, RemoteFs};

/// 允许跳过证书校验的自定义验证器（仅在用户主动勾选「忽略自签名证书」时启用）
#[derive(Debug)]
struct NoCertVerifier;

impl ServerCertVerifier for NoCertVerifier {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::RSA_PKCS1_SHA384,
            SignatureScheme::RSA_PKCS1_SHA512,
            SignatureScheme::ECDSA_NISTP256_SHA256,
            SignatureScheme::ECDSA_NISTP384_SHA384,
            SignatureScheme::RSA_PSS_SHA256,
            SignatureScheme::RSA_PSS_SHA384,
            SignatureScheme::RSA_PSS_SHA512,
            SignatureScheme::ED25519,
        ]
    }
}

/// 构造 `rustls` TLS 1.2/1.3 连接器
fn build_rustls_connector(skip_verify: bool) -> AppResult<RustlsConnector> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = if skip_verify {
        ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| AppError::Connection(format!("初始化 TLS 配置失败: {}", e)))?
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoCertVerifier))
            .with_no_client_auth()
    } else {
        let mut root_store = RootCertStore::empty();
        root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        ClientConfig::builder_with_provider(provider)
            .with_safe_default_protocol_versions()
            .map_err(|e| AppError::Connection(format!("初始化 TLS 配置失败: {}", e)))?
            .with_root_certificates(root_store)
            .with_no_client_auth()
    };

    Ok(RustlsConnector::from(Arc::new(config)))
}

/// FTP / FTPS 客户端实例
pub struct FtpClient {
    stream: Arc<Mutex<RustlsFtpStream>>,
}

impl FtpClient {
    /// 连接并登录 FTP / FTPS 服务器
    pub async fn connect(
        host: &str,
        port: u16,
        username: &str,
        password: Option<&str>,
        protocol: Protocol,
        tls_skip_verify: bool,
    ) -> AppResult<Self> {
        let host = host.trim().to_string();
        if host.is_empty() {
            return Err(AppError::InvalidInput("FTP 主机地址不能为空".into()));
        }
        let user = if username.trim().is_empty() {
            "anonymous".to_string()
        } else {
            username.to_string()
        };
        let pass = password
            .unwrap_or("anonymous@xiaorui-ftp.local")
            .to_string();

        let stream = tokio::task::spawn_blocking(move || -> AppResult<RustlsFtpStream> {
            let addr = format!("{}:{}", host, port);
            let mut ftp = match protocol {
                Protocol::FtpsImplicit => {
                    let connector = build_rustls_connector(tls_skip_verify)?;
                    RustlsFtpStream::connect_secure_implicit(&addr, connector, &host)
                        .map_err(|e| AppError::Connection(format!("隐式 FTPS 连接失败: {}", e)))?
                }
                Protocol::FtpsExplicit => {
                    let connector = build_rustls_connector(tls_skip_verify)?;
                    let plain = RustlsFtpStream::connect(&addr).map_err(|e| {
                        AppError::Connection(format!("连接 FTP {} 失败: {}", addr, e))
                    })?;
                    plain.into_secure(connector, &host).map_err(|e| {
                        AppError::Connection(format!("FTPS AUTH TLS 握手失败: {}", e))
                    })?
                }
                _ => RustlsFtpStream::connect(&addr)
                    .map_err(|e| AppError::Connection(format!("连接 FTP {} 失败: {}", addr, e)))?,
            };

            ftp.login(&user, &pass)
                .map_err(|e| AppError::Auth(format!("FTP 登录失败: {}", e)))?;
            // 强制二进制传输模式，避免文本换行符被服务端篡改
            let _ = ftp.transfer_type(FileType::Binary);
            ftp.set_mode(Mode::ExtendedPassive);
            Ok(ftp)
        })
        .await
        .map_err(|e| AppError::Connection(format!("FTP 连接任务异常: {}", e)))??;

        Ok(Self {
            stream: Arc::new(Mutex::new(stream)),
        })
    }
}

#[async_trait]
impl RemoteFs for FtpClient {
    async fn list_dir(&self, path: &str) -> AppResult<Vec<FileEntry>> {
        let norm = normalize_remote_path(path);
        let stream = self.stream.clone();

        tokio::task::spawn_blocking(move || -> AppResult<Vec<FileEntry>> {
            let mut ftp = stream
                .lock()
                .map_err(|_| AppError::Protocol("FTP 会话锁损坏".into()))?;
            ftp.cwd(&norm)
                .map_err(|e| AppError::Protocol(format!("切换目录 '{}' 失败: {}", norm, e)))?;

            // 若服务器不支持 EPSV，则自动回退到标准 PASV 模式重试
            let lines = match ftp.list(None) {
                Ok(l) => l,
                Err(_) => {
                    ftp.set_mode(Mode::Passive);
                    ftp.list(None)
                        .map_err(|e| AppError::Protocol(format!("列举目录失败: {}", e)))?
                }
            };

            let mut entries = Vec::new();
            for line in lines {
                if let Ok(parsed) = FtpListFile::from_str(&line) {
                    let name = parsed.name().to_string();
                    if name == "." || name == ".." || name.is_empty() {
                        continue;
                    }
                    let is_dir = parsed.is_directory();
                    let is_symlink = parsed.is_symlink();
                    let modified = parsed
                        .modified()
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_secs() as i64);

                    entries.push(FileEntry {
                        path: join_remote_path(&norm, &name),
                        name,
                        is_dir,
                        size: if is_dir { 0 } else { parsed.size() as u64 },
                        modified,
                        permissions: None,
                        mode: None,
                        is_symlink,
                    });
                }
            }

            entries.sort_by(|a, b| {
                b.is_dir
                    .cmp(&a.is_dir)
                    .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            });
            Ok(entries)
        })
        .await
        .map_err(|e| AppError::Protocol(format!("FTP 列举任务异常: {}", e)))?
    }

    async fn mkdir(&self, path: &str) -> AppResult<()> {
        let norm = normalize_remote_path(path);
        let stream = self.stream.clone();
        tokio::task::spawn_blocking(move || -> AppResult<()> {
            let mut ftp = stream
                .lock()
                .map_err(|_| AppError::Protocol("锁损坏".into()))?;
            ftp.mkdir(&norm)
                .map_err(|e| AppError::Protocol(format!("创建目录失败: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| AppError::Protocol(e.to_string()))?
    }

    async fn remove(&self, path: &str, is_dir: bool) -> AppResult<()> {
        let norm = normalize_remote_path(path);
        let stream = self.stream.clone();
        tokio::task::spawn_blocking(move || -> AppResult<()> {
            let mut ftp = stream
                .lock()
                .map_err(|_| AppError::Protocol("锁损坏".into()))?;
            if is_dir {
                ftp.rmdir(&norm).map_err(|e| {
                    AppError::Protocol(format!("删除目录失败（请先清空子项）: {}", e))
                })?;
            } else {
                ftp.rm(&norm)
                    .map_err(|e| AppError::Protocol(format!("删除文件失败: {}", e)))?;
            }
            Ok(())
        })
        .await
        .map_err(|e| AppError::Protocol(e.to_string()))?
    }

    async fn rename(&self, from: &str, to: &str) -> AppResult<()> {
        let src = normalize_remote_path(from);
        let dst = normalize_remote_path(to);
        let stream = self.stream.clone();
        tokio::task::spawn_blocking(move || -> AppResult<()> {
            let mut ftp = stream
                .lock()
                .map_err(|_| AppError::Protocol("锁损坏".into()))?;
            ftp.rename(&src, &dst)
                .map_err(|e| AppError::Protocol(format!("重命名失败: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| AppError::Protocol(e.to_string()))?
    }

    async fn chmod(&self, path: &str, mode: u32) -> AppResult<()> {
        let norm = normalize_remote_path(path);
        let stream = self.stream.clone();
        tokio::task::spawn_blocking(move || -> AppResult<()> {
            let mut ftp = stream
                .lock()
                .map_err(|_| AppError::Protocol("锁损坏".into()))?;
            let cmd = format!("CHMOD {:03o} {}", mode & 0o777, norm);
            ftp.site(&cmd)
                .map_err(|e| AppError::Protocol(format!("SITE CHMOD 失败: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| AppError::Protocol(e.to_string()))?
    }

    async fn read_bytes(&self, path: &str, max_bytes: usize) -> AppResult<Vec<u8>> {
        let norm = normalize_remote_path(path);
        let stream = self.stream.clone();
        tokio::task::spawn_blocking(move || -> AppResult<Vec<u8>> {
            let mut ftp = stream
                .lock()
                .map_err(|_| AppError::Protocol("锁损坏".into()))?;
            let cursor = ftp
                .retr_as_buffer(&norm)
                .map_err(|e| AppError::Protocol(format!("读取远程文件失败: {}", e)))?;
            let data = cursor.into_inner();
            if data.len() > max_bytes {
                return Err(AppError::InvalidInput(format!(
                    "文件超过在线编辑限制 ({} KB)",
                    max_bytes / 1024
                )));
            }
            Ok(data)
        })
        .await
        .map_err(|e| AppError::Protocol(e.to_string()))?
    }

    async fn write_bytes(&self, path: &str, data: &[u8]) -> AppResult<()> {
        let norm = normalize_remote_path(path);
        let bytes = data.to_vec();
        let stream = self.stream.clone();
        tokio::task::spawn_blocking(move || -> AppResult<()> {
            let mut ftp = stream
                .lock()
                .map_err(|_| AppError::Protocol("锁损坏".into()))?;
            let mut cursor = Cursor::new(bytes);
            ftp.put_file(&norm, &mut cursor)
                .map_err(|e| AppError::Protocol(format!("保存远程文件失败: {}", e)))?;
            Ok(())
        })
        .await
        .map_err(|e| AppError::Protocol(e.to_string()))?
    }

    async fn download_file(
        &self,
        remote_path: &str,
        local_path: &Path,
        progress: ProgressCallback,
        cancel: Arc<AtomicBool>,
    ) -> AppResult<u64> {
        let norm = normalize_remote_path(remote_path);
        let local_pb = local_path.to_path_buf();
        let stream = self.stream.clone();

        tokio::task::spawn_blocking(move || -> AppResult<u64> {
            if let Some(parent) = local_pb.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut ftp = stream
                .lock()
                .map_err(|_| AppError::Protocol("锁损坏".into()))?;
            let total = ftp.size(&norm).unwrap_or(0) as u64;

            let mut transferred: u64 = 0;
            ftp.retr(&norm, |reader| {
                let mut out = std::fs::File::create(&local_pb)
                    .map_err(suppaftp::FtpError::ConnectionError)?;
                let mut buf = [0u8; 64 * 1024];
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(suppaftp::FtpError::ConnectionError(std::io::Error::new(
                            std::io::ErrorKind::Interrupted,
                            "cancelled",
                        )));
                    }
                    let n = reader
                        .read(&mut buf)
                        .map_err(suppaftp::FtpError::ConnectionError)?;
                    if n == 0 {
                        break;
                    }
                    out.write_all(&buf[..n])
                        .map_err(suppaftp::FtpError::ConnectionError)?;
                    transferred += n as u64;
                    progress(transferred, total.max(transferred));
                }
                out.flush().map_err(suppaftp::FtpError::ConnectionError)?;
                Ok(())
            })
            .map_err(|e| {
                if cancel.load(Ordering::Relaxed) {
                    AppError::Cancelled
                } else {
                    AppError::Protocol(format!("FTP 下载失败: {}", e))
                }
            })?;
            Ok(transferred)
        })
        .await
        .map_err(|e| AppError::Protocol(e.to_string()))?
    }

    async fn upload_file(
        &self,
        local_path: &Path,
        remote_path: &str,
        progress: ProgressCallback,
        cancel: Arc<AtomicBool>,
    ) -> AppResult<u64> {
        let norm = normalize_remote_path(remote_path);
        let local_pb = local_path.to_path_buf();
        let stream = self.stream.clone();

        tokio::task::spawn_blocking(move || -> AppResult<u64> {
            let meta = std::fs::metadata(&local_pb)?;
            let total = meta.len();
            let file = std::fs::File::open(&local_pb)?;
            let mut reader = ProgressReader {
                inner: file,
                transferred: 0,
                total,
                progress,
                cancel: cancel.clone(),
            };
            let mut ftp = stream
                .lock()
                .map_err(|_| AppError::Protocol("锁损坏".into()))?;
            ftp.put_file(&norm, &mut reader).map_err(|e| {
                if cancel.load(Ordering::Relaxed) {
                    AppError::Cancelled
                } else {
                    AppError::Protocol(format!("FTP 上传失败: {}", e))
                }
            })?;
            Ok(reader.transferred)
        })
        .await
        .map_err(|e| AppError::Protocol(e.to_string()))?
    }
}

/// 包装本地文件 `Read` 实现，用于上传过程中上报实时进度与响应取消中断
struct ProgressReader<R> {
    inner: R,
    transferred: u64,
    total: u64,
    progress: ProgressCallback,
    cancel: Arc<AtomicBool>,
}

impl<R: Read> Read for ProgressReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "cancelled",
            ));
        }
        let n = self.inner.read(buf)?;
        if n > 0 {
            self.transferred += n as u64;
            (self.progress)(self.transferred, self.total.max(self.transferred));
        }
        Ok(n)
    }
}
