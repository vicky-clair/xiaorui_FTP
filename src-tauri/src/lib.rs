//! xiaorui_FTP Rust 后端核心库
//!
//! 负责初始化 Tauri 运行时、注册插件、管理全局应用状态（站点、会话、传输队列）
//! 以及向前端暴露所有类型安全的 IPC 命令。

mod commands;
mod error;
mod local_fs;
mod models;
mod protocols;
mod storage;
mod transfer;

use tauri::Manager;

use crate::commands::AppState;

/// 启动 Tauri 桌面应用程序
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 注册系统文件/目录选择对话框插件与外部链接打开插件
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // 获取操作系统标准应用数据目录用于持久化站点列表
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("xiaorui_FTP"));
            let state =
                AppState::new(data_dir).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
            app.manage(state);
            Ok(())
        })
        // 注册前端可调用的 Tauri IPC 命令清单
        .invoke_handler(tauri::generate_handler![
            // 站点管理与本地备份导入导出
            commands::list_sites,
            commands::save_site,
            commands::delete_site,
            commands::export_sites_to_file,
            commands::import_sites_from_file,
            commands::check_ssh_agent_status,
            // 会话连接与生命周期管理
            commands::connect_saved_site,
            commands::connect_adhoc,
            commands::quick_connect_uri,
            commands::disconnect_session,
            commands::list_sessions,
            // 远程文件系统操作
            commands::remote_list_dir,
            commands::remote_mkdir,
            commands::remote_delete,
            commands::remote_rename,
            commands::remote_chmod,
            commands::remote_read_text,
            commands::remote_write_text,
            // 本地文件系统操作
            commands::local_home_dir,
            commands::local_list_roots,
            commands::local_list_dir,
            commands::local_mkdir,
            commands::local_delete,
            commands::local_rename,
            // 并发传输队列操作
            commands::enqueue_upload,
            commands::enqueue_upload_paths,
            commands::enqueue_download,
            commands::list_transfers,
            commands::cancel_transfer,
            commands::clear_finished_transfers,
            // SFTP 内置远程命令终端
            commands::ssh_exec_command,
        ])
        .run(tauri::generate_context!())
        .expect("启动 xiaorui_FTP 应用程序失败");
}
