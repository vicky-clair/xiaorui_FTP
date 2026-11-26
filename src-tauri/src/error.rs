//! 统一错误类型定义模块
//!
//! 将底层 IO、网络、认证、协议与存储异常转换为对用户友好的中文错误描述，
//! 并实现 `serde::Serialize` 以便直接返回给 Tauri 前端提示。

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
