//! 单实例守护。
//!
//! 为什么需要：选择「退到后台」后窗口从任务栏消失，用户往往以为程序没开，
//! 于是再从开始菜单点一次。不拦的话会起第二个进程 —— 两个托盘图标，
//! 而且两个进程会同时遍历同一块磁盘（扫描锁是进程内的，管不了跨进程）。
//!
//! 实现用 **命名事件** 而非文件锁/端口/FindWindow：
//! - 不引入新依赖（windows-sys 已在依赖里）
//! - 第二个实例只需 SetEvent 一次，首实例的后台线程就会把窗口调回前台
//! - 没有「上一次异常退出留下锁文件」这类脏状态（内核对象随进程消失自动回收）

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows_sys::Win32::System::Threading::{
    CreateEventW, OpenEventW, SetEvent, WaitForSingleObject,
};

/// 放在 Local\ 命名空间：同一登录会话内唯一，不与其他用户互相干扰
const EVENT_NAME: &str = "Local\\WorkBuddy.DiskManager.Activate";
const EVENT_MODIFY_STATE: u32 = 0x0002;
/// 表示「创建失败、不做单实例保护」的哨兵句柄
const NO_HANDLE: usize = 0;

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// 尝试成为首实例。
/// - `Some(handle)`：本进程是首实例，handle 交给监听线程等待唤醒信号
/// - `None`：已有实例在运行，且已发出「请把窗口调回前台」的信号
pub fn acquire() -> Option<usize> {
    let name = wide(EVENT_NAME);
    unsafe {
        let existing = OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr());
        if !existing.is_null() {
            let _ = SetEvent(existing);
            let _ = CloseHandle(existing);
            return None;
        }
        // 自动重置事件（bManualReset = 0）：每次 SetEvent 只唤醒一次等待，不会粘住
        let handle = CreateEventW(std::ptr::null(), 0, 0, name.as_ptr());
        if handle.is_null() {
            // 创建失败极罕见（资源耗尽等）：宁可不做保护，也要保证程序能启动
            return Some(NO_HANDLE);
        }
        Some(handle as usize)
    }
}

/// 后台等待唤醒信号；收到就把主窗口调回前台
pub fn spawn_listener(handle: usize, app: tauri::AppHandle) {
    if handle == NO_HANDLE {
        return;
    }
    std::thread::spawn(move || unsafe {
        // 句柄以 usize 跨线程传递：裸指针不是 Send，而内核句柄本身是进程内全局有效的
        let h = handle as *mut c_void;
        loop {
            // INFINITE：没有信号就一直阻塞，不占 CPU
            if WaitForSingleObject(h, u32::MAX) == WAIT_OBJECT_0 {
                super::commands::settings::show_main(&app);
            } else {
                break;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证命名事件的真实语义：同一时刻只允许一个「首实例」，
    /// 且上一个实例退出（句柄关闭 → 内核对象销毁）后可以重新成为首实例。
    #[test]
    fn detects_running_instance() {
        // 若开发机上正开着本程序，事件已存在，此时跳过而不是误报失败
        let Some(first) = acquire() else {
            eprintln!("skip: 本机已有实例在运行，跳过单实例测试");
            return;
        };
        assert_ne!(first, NO_HANDLE, "事件创建意外失败，测试无法继续");

        assert!(acquire().is_none(), "已有实例在运行时，第二个实例必须被拒绝");

        // 关闭句柄 = 模拟程序退出，内核对象随之销毁
        unsafe { CloseHandle(first as *mut c_void) };

        let again = acquire().expect("上一个实例退出后应可重新成为首实例");
        assert_ne!(again, NO_HANDLE);
        unsafe { CloseHandle(again as *mut c_void) };
    }
}
