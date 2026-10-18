mod commands;
mod instance;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WindowEvent};

use commands::settings::{CLOSE_BACKGROUND, CLOSE_EXIT, show_main};

/// 创建系统托盘。失败不是致命错误：只是「退到后台」不可用，仍可正常关闭程序。
fn build_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示主界面", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出程序", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .tooltip("存储管理 Disk Manager");

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
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
        let n = commands::update::cleanup_update_leftovers();
        if n > 0 {
          log::info!("已清理 {n} 个更新残留目录");
        }
      });

      if let Err(e) = build_tray(app.handle()) {
        log::warn!("托盘图标创建失败，将禁用「退到后台」选项：{e}");
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
      commands::installer::scan_installers,
      commands::installer::cancel_installer_scan,
      commands::installer::delete_installers,
      commands::emptydir::scan_empty_dirs,
      commands::emptydir::cancel_empty_dir_scan,
      commands::emptydir::delete_empty_dirs,
      commands::uninstall::list_apps,
      commands::uninstall::list_win32_apps,
      commands::uninstall::list_store_apps_only,
      commands::uninstall::uninstall_app,
      commands::uninstall::cancel_uninstall,
      commands::uninstall::check_installed,
      commands::uninstall::scan_app_residuals,
      commands::uninstall::clean_residuals,
      commands::uninstall::get_app_icon,
      commands::uninstall::force_remove_app,
      commands::uninstall::restart_as_admin,
      commands::settings::get_app_settings,
      commands::settings::update_close_action,
      commands::settings::update_autostart,
      commands::settings::resolve_close,
      commands::settings::show_main_window,
      commands::update::update_endpoint,
    ])
    .run(tauri::generate_context!())
    .expect("error while building tauri application");
}
