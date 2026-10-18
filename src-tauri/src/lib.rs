//! WorkMami 后端库入口：模块装配 + Tauri Builder。

mod accounts;
mod api;
mod commands;
mod data;
mod error;
mod oauth;
mod platform;
mod proxy;
mod scheduler;
mod state;
mod usage;
mod vault;
mod wbauth;

use std::sync::Arc;

use state::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let mut builder = tauri::Builder::default();

    // 单实例
    builder = builder.plugin(tauri_plugin_single_instance::init(|_app, _argv, _cwd| {}));
    builder = builder
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec![]),
        ));

    // 登录窗口被关闭/销毁时终止对应 OAuth 会话，避免前端弹窗卡在 pending。
    builder = builder.on_window_event(|window, event| {
        match event {
            tauri::WindowEvent::Destroyed => oauth::on_window_event(window.label(), true),
            tauri::WindowEvent::CloseRequested { .. } => oauth::on_window_event(window.label(), false),
            _ => {}
        }
    });

    builder = builder.setup(|app| {
        platform::init_paths();
        let app_state = Arc::new(AppState::load()?);
        app.manage(app_state.clone());
    
        // 一次性迁移：旧钥匙串 token → 本地加密 vault（迁移后 keyring 条目删除）
        {
            let uids: Vec<String> = app_state.accounts.all().iter().map(|a| a.uid.clone()).collect();
            if !uids.is_empty() {
                vault::migrate_from_keyring("com.workmami.accounts", &uids);
            }
        }
    
        // 账号统一通过"网页授权"添加（扫描无法取得可用 token，已退役）。
        // 登录窗会话隔离（清掉上一个账号的 WebView 会话、避免"选择账号"弹窗）已下沉到 oauth::begin，
        // 每次开窗前清一次并等清理跑完再导航；不在此处启动时清，避免与快速点击的 begin 清理重叠。
    
        // 启动反代（若配置为启用）
        if app_state.proxy_settings().enabled {
            let st = app_state.clone();
            tauri::async_runtime::spawn(async move {
                proxy::start(st).await;
            });
        }
    
        // 启动调度器（传 AppHandle 用于旅行自动领取通知）
        let handle = app.handle().clone();
        tauri::async_runtime::spawn(async move {
            match scheduler::start(handle, app_state.clone()).await {
                Ok(sched) => {
                    // 持有 scheduler 防止被 drop
                    use std::sync::OnceLock;
                    static SCHED: OnceLock<tokio_cron_scheduler::JobScheduler> = OnceLock::new();
                    let _ = SCHED.set(sched);
                }
                Err(e) => tracing::error!("调度器启动失败: {e}"),
            }
        });
    
        Ok(())
    });

    builder = builder.invoke_handler(tauri::generate_handler![
        commands::app_info,
        commands::get_settings,
        commands::save_settings,
        commands::proxy_status,
        commands::list_accounts,
        commands::credential_status,
        commands::set_account_enabled,
        commands::set_account_tags,
        commands::delete_account,
        commands::refresh_quota,
        commands::credit_ledger,
        commands::list_models,
        commands::oauth_begin,
        commands::oauth_poll,
        commands::checkin_status,
        commands::checkin_all,
        commands::checkin_one,
        commands::travel_overview,
        commands::travel_depart,
        commands::travel_claim,
        commands::travel_records,
        commands::data_list,
        commands::data_read,
        commands::data_delete,
        commands::data_export,
        commands::data_backup,
        commands::data_migrate,
        commands::data_uids,
        commands::dashboard_summary,
        commands::dashboard_trend,
        commands::dashboard_top,
        commands::dashboard_heatmap,
        commands::dashboard_balances,
        commands::dashboard_import_app,
        commands::dashboard_requests,
        commands::dashboard_account_billing,
    ]);

    builder.run(tauri::generate_context!()).expect("运行 WorkMami 失败");
}
