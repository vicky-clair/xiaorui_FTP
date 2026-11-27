//! 统一错误类型定义模块
//!
//! 将底层 IO、网络、认证、主机密钥校验、安全策略、协议与存储异常
//! 转换为结构化且对用户友好的中文错误描述，并实现 `serde::Serialize` 供 Tauri 前端识别。

use serde::Serialize;
use thiserror::Error;

/// 应用级统一错误枚举
#[derive(Debug, Error)]
pub enum AppError {
    /// 本地文件或系统 IO 读写异常
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),

    /// 网络连接、DNS 解析或 TLS/SSH 握手失败
    #[error("连接失败: {0}")]
    Connection(String),

    /// 用户名、密码、私钥、SSH Agent 或访问密钥校验未通过
    #[error("认证失败: {0}")]
    Auth(String),

    /// 首次连接未知 SSH 主机，需用户在前端核对算法与 SHA-256 指纹并显式确认
    #[error("HOST_KEY_UNKNOWN::{0}")]
    HostKeyUnknown(String),

    /// SSH 主机公钥指纹发生变更，已立即阻断连接以防范中间人攻击
    #[error("HOST_KEY_MISMATCH::{0}")]
    HostKeyMismatch(String),

    /// 命中安全策略阻断（如路径穿越、控制字符注入、危险删除或已知主机仓库损坏）
    #[error("安全策略拦截: {0}")]
    SecurityPolicy(String),

    /// 权限校验未通过（如跨窗口越权访问）
    #[error("权限拒绝: {0}")]
    PermissionDenied(String),

    /// 远程协议交互错误（SFTP/FTP/S3/WebDAV）
    #[error("协议错误: {0}")]
    Protocol(String),

    /// 当前协议不支持所请求的操作（如对象存储执行 CHMOD）
    #[error("不支持的操作: {0}")]
    Unsupported(String),

    /// 请求的站点、文件或对象不存在
    #[error("未找到目标: {0}")]
    NotFound(String),

    /// 用户输入的参数格式不合法
    #[error("参数无效: {0}")]
    InvalidInput(String),

    /// 指定的远程会话已关闭或不存在
    #[error("会话不存在或已断开: {0}")]
    SessionNotFound(String),

    /// 用户主动取消传输任务
    #[error("传输已取消")]
    Cancelled,

    /// 站点配置读写或操作系统凭据库异常
    #[error("存储错误: {0}")]
    Storage(String),
}

impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

/// 后端统一的 Result 类型别名
pub type AppResult<T> = Result<T, AppError>;
