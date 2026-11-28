//! xiaorui_FTP Rust 后端核心库
//!
//! 负责初始化 Tauri 运行时、注册插件、系统托盘与关闭最小化行为、
//! 管理全局应用状态（站点、会话、传输队列、偏好设置）以及向前端暴露所有类型安全的 IPC 命令。

mod audit;
mod commands;
mod error;
mod known_hosts;
mod local_fs;
mod models;
mod protocols;
mod security;
mod storage;
mod transfer;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};

use crate::commands::AppState;

/// 恢复并聚焦主窗口
fn show_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    if let Some(win) = app.get_webview_window("main") {
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
    }
}

/// 启动 Tauri 桌面应用程序
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // 遵循最小权限原则：仅注册实际使用的系统文件/目录选择对话框插件（移除未使用的 opener 插件）
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // 获取操作系统标准应用数据目录用于持久化站点列表与轻量偏好
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("xiaorui_FTP"));
            let state =
                AppState::new(data_dir).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;
            app.manage(state);

            // 初始化系统托盘图标与右键菜单（左键单击呼出主窗口，右键支持显示/彻底退出）
            let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出 xiaorui_FTP", true, None::<&str>)?;
            let tray_menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            let mut tray_builder = TrayIconBuilder::with_id("main-tray")
                .menu(&tray_menu)
                .tooltip("xiaorui_FTP — 现代文件传输客户端")
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => show_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    }
                    | TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                });

            if let Some(icon) = app.default_window_icon().cloned() {
                tray_builder = tray_builder.icon(icon);
            }
            let _ = tray_builder.build(app);

            Ok(())
        })
        // 拦截窗口关闭事件：主窗口按托盘偏好最小化；独立终端窗口关闭时回收其会话绑定
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let label = window.label().to_string();
                if label == "main" {
                    let should_hide = window
                        .try_state::<AppState>()
                        .map(|s| s.is_minimize_to_tray())
                        .unwrap_or(false);
                    if should_hide {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                } else if label.starts_with("term-") {
                    let app_handle = window.app_handle().clone();
                    tauri::async_runtime::spawn(async move {
                        if let Some(state) = app_handle.try_state::<AppState>() {
                            state.unregister_terminal_window(&label).await;
                        }
                    });
                }
            }
        })
        // 注册前端可调用的 Tauri IPC 命令清单
        .invoke_handler(tauri::generate_handler![
            // 站点管理、偏好设置与本地备份导入导出
            commands::list_sites,
            commands::save_site,
            commands::delete_site,
            commands::export_sites_to_file,
            commands::import_sites_from_file,
            commands::check_ssh_agent_status,
            commands::list_known_hosts,
            commands::remove_known_host,
            commands::trust_known_host,
            commands::list_audit_logs,
            commands::export_audit_logs,
            commands::get_app_settings,
            commands::set_minimize_to_tray,
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
            // 交互式 SSH PTY、独立终端窗口与服务器硬件监测
            commands::remote_server_metrics,
            commands::ssh_pty_open,
            commands::ssh_pty_write,
            commands::ssh_pty_resize,
            commands::ssh_pty_close,
            commands::open_terminal_window,
            commands::focus_main_window,
            commands::clipboard_read_text,
            commands::clipboard_write_text,
            commands::save_text_file,
        ])
        .run(tauri::generate_context!())
        .expect("启动 xiaorui_FTP 应用程序失败");
}
