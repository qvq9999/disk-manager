mod app_meta;
mod commands;
mod instance;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WindowEvent};

use commands::settings::{CLOSE_BACKGROUND, CLOSE_EXIT, show_main};

/// 窗口与托盘统一使用的图标（256px 源）。
///
/// 为什么不用 `app.default_window_icon()`：它取的是 `bundle.icon` 列表里第一个可解析的
/// 图标 —— 也就是 `icons/32x32.png`。任务栏/托盘在 150% 缩放下会请求 40~48px，
/// 系统只能把 32px 放大，边缘发虚；用户观感就是「图标好像还是旧的/糊的」。
/// 这里直接用 256px 源，由系统按需缩放。
fn app_icon() -> Option<tauri::image::Image<'static>> {
    tauri::image::Image::from_bytes(include_bytes!("../icons/128x128@2x.png")).ok()
}

/// 创建系统托盘。失败不是致命错误：只是「退到后台」不可用，仍可正常关闭程序。
fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示主界面", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出程序", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("存储管理 Disk Manager");

    if let Some(icon) = app_icon().or_else(|| app.default_window_icon().cloned()) {
        builder = builder.icon(icon);
    }

    builder
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            // 托盘里的「退出程序」是明确意图，不再二次询问
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;

    commands::settings::set_tray_ready(true);
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  // 单实例检查必须在 Tauri 初始化之前：否则第二个实例已经把窗口和托盘建出来了才退出，
  // 用户会看到一闪而过的界面和重复托盘图标。
  let Some(instance_handle) = instance::acquire() else {
    // 已有实例在运行，且已通知它把窗口调回前台；本进程无事可做
    std::process::exit(0);
  };

  tauri::Builder::default()
    // 自动更新：检查 GitHub Release 上的 latest.json，签名校验后运行安装包
    .plugin(tauri_plugin_updater::Builder::new().build())
    .setup(move |app| {
      if cfg!(debug_assertions) {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .build(),
        )?;
      }

      // 设置必须在托盘之前载入（关闭行为依赖它）
      commands::settings::init(app.handle());

      // 清理历史版本的更新器临时目录：tauri 的 updater 用 `.keep()` 建临时目录、
      // 不会自动删除，每更新一次就在 %TEMP% 里留一个（含安装包）。
      // 异步做，绝不阻塞主窗口出现。
      std::thread::spawn(|| {
        // 先记下「从哪运行的」：安装器读它把更新装回同一目录，不再另起一份
        commands::update::write_running_from();
        let n = commands::update::cleanup_update_leftovers();
        if n > 0 {
          log::info!("已清理 {n} 个更新残留目录");
        }
        // 只在安装器打过标记时才会真的动手，平时是空操作
        for line in commands::update::cleanup_other_installs() {
          log::info!("清理多余安装 · {line}");
        }
      });

      if let Err(e) = build_tray(app.handle()) {
        log::warn!("托盘图标创建失败，将禁用「退到后台」选项：{e}");
      }

      // 显式给主窗口设置图标：任务栏与 Alt+Tab 用的是**窗口图标**（WM_SETICON），
      // 不是 exe 里嵌的资源。覆盖安装后若沿用默认解析（32px 源），高 DPI 下会被放大发虚，
      // 看起来像「没换图标」。这里用 256px 源让系统自己挑尺寸。
      if let (Some(window), Some(icon)) = (app.get_webview_window("main"), app_icon()) {
        if let Err(e) = window.set_icon(icon) {
          log::warn!("设置窗口图标失败：{e}");
        }
      }

      // 再次启动本程序时，把窗口调回前台
      instance::spawn_listener(instance_handle, app.handle().clone());
      Ok(())
    })
    .on_window_event(|window, event| {
      // 拦截关闭：按用户设置的关闭行为决定「拦下来」还是「放行」
      if let WindowEvent::CloseRequested { api, .. } = event {
        match commands::settings::close_action().as_str() {
          CLOSE_EXIT => {
            // 阻止默认关闭再显式 exit：托盘存在时仅关窗口并不会结束进程
            api.prevent_close();
            window.app_handle().exit(0);
          }
          CLOSE_BACKGROUND if commands::settings::tray_ready() => {
            api.prevent_close();
            let _ = window.hide();
          }
          // 未设置过 / 未记住 / 托盘不可用 → 交给前端弹窗询问
          _ => {
            api.prevent_close();
            let _ = window.emit("close-requested", ());
          }
        }
      }
    })
    .invoke_handler(tauri::generate_handler![
      commands::volumes::get_volumes,
      commands::volumes::is_elevated,
      commands::disk::get_physical_disks,
      commands::hardware::get_hardware_info,
      commands::scan::scan_drive,
      commands::scan::cancel_scan,
      commands::scan::scan_status,
      commands::clean::get_clean_items,
      commands::clean::run_clean,
      commands::clean::cancel_clean,
      commands::installer::scan_installers,
      commands::installer::cancel_installer_scan,
      commands::installer::delete_installers,
      commands::emptydir::scan_empty_dirs,
      commands::dupes::find_duplicates,
      commands::dupes::cancel_dupes,
      commands::dupes::delete_duplicates,
      commands::emptydir::cancel_empty_dir_scan,
      commands::emptydir::delete_empty_dirs,
      commands::uninstall::list_apps,
      commands::uninstall::list_win32_apps,
      commands::uninstall::list_store_apps_only,
      commands::uninstall::uninstall_app,
      commands::uninstall::cancel_uninstall,
      commands::uninstall::check_installed,
      commands::residual::scan_app_residuals,
      commands::residual::clean_residuals,
      commands::residual::prepare_app_removal,
      commands::uninstall::get_app_icon,
      commands::uninstall::force_remove_app,
      commands::uninstall::restart_as_admin,
      commands::settings::get_app_settings,
      commands::settings::update_close_action,
      commands::settings::update_autostart,
      commands::settings::update_prefs,
      commands::settings::accept_disclaimer,
      commands::settings::resolve_close,
      commands::settings::show_main_window,
      commands::update::update_endpoint,
      commands::update::update_prepare,
      commands::identify::identify_node,
      commands::identify::reveal_in_explorer,
      commands::identify::open_external,
      commands::ai::ai_get_config,
      commands::ai::ai_save_config,
      commands::ai::ai_test_connection,
      commands::ai::ai_analyze,
      commands::ai::ai_chat,
      commands::ai::ai_chat_stream,
      commands::ai::ai_analyze_batch,
      commands::ai::ai_suggest,
    ])
    .run(tauri::generate_context!())
    .expect("error while building tauri application");
}
