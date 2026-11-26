// 在 Windows Release 构建中隐藏额外的控制台黑框窗口
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// xiaorui_FTP 桌面程序入口
fn main() {
    xiaorui_ftp_lib::run()
}
