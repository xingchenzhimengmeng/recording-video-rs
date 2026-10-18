use anyhow::{anyhow, Result};
use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowItem {
    pub hwnd: usize,
    pub title: String,
    pub class_name: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// 枚举当前桌面上所有可见、可录制的主应用窗口
pub fn enumerate_windows() -> Vec<WindowItem> {
    let mut windows_list = Vec::new();

    unsafe extern "system" fn enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let list = &mut *(lparam.0 as *mut Vec<WindowItem>);

        if !IsWindowVisible(hwnd).as_bool() {
            return BOOL(1);
        }

        let mut title_buf = [0u16; 512];
        let len = GetWindowTextW(hwnd, &mut title_buf);
        if len <= 0 {
            return BOOL(1);
        }
        let title = String::from_utf16_lossy(&title_buf[..len as usize]).trim().to_string();
        if title.is_empty() || title == "Program Manager" || title == "Windows 输入体验" {
            return BOOL(1);
        }

        let mut rect = RECT::default();
        if GetWindowRect(hwnd, &mut rect).is_err() {
            return BOOL(1);
        }

        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if w <= 100 || h <= 100 {
            return BOOL(1);
        }

        let mut class_buf = [0u16; 256];
        let clen = GetClassNameW(hwnd, &mut class_buf);
        let class_name = if clen > 0 {
            String::from_utf16_lossy(&class_buf[..clen as usize]).trim().to_string()
        } else {
            String::new()
        };

        list.push(WindowItem {
            hwnd: hwnd.0 as usize,
            title,
            class_name,
            x: rect.left,
            y: rect.top,
            width: w,
            height: h,
        });

        BOOL(1)
    }

    unsafe {
        let _ = EnumWindows(Some(enum_proc), LPARAM(&mut windows_list as *mut _ as isize));
    }

    windows_list
}

/// 获取指定窗口的最新屏幕坐标矩形 (x, y, width, height)
pub fn get_window_rect(hwnd: usize) -> Result<(i32, i32, i32, i32)> {
    unsafe {
        let mut rect = RECT::default();
        GetWindowRect(HWND(hwnd as *mut _), &mut rect)
            .map_err(|e| anyhow!("获取窗口矩形失败: {e}"))?;
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        Ok((rect.left, rect.top, w, h))
    }
}

/// 调整指定窗口的大小和屏幕位置 (对应 Bandicam 的窗口重调)
pub fn resize_and_move_window(hwnd: usize, x: i32, y: i32, width: i32, height: i32) -> Result<()> {
    unsafe {
        let w = width.max(320);
        let h = height.max(240);
        SetWindowPos(
            HWND(hwnd as *mut _),
            HWND(std::ptr::null_mut()),
            x,
            y,
            w,
            h,
            SWP_NOZORDER | SWP_NOACTIVATE,
        ).map_err(|e| anyhow!("调整窗口大小失败: {e}"))?;
        Ok(())
    }
}

/// 将指定窗口居中放置到屏幕中心并设定指定尺寸 (1080P/720P等)
pub fn center_window_on_screen(hwnd: usize, target_w: i32, target_h: i32) -> Result<(i32, i32, i32, i32)> {
    unsafe {
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        let w = target_w.min(screen_w).max(320);
        let h = target_h.min(screen_h).max(240);
        let x = ((screen_w - w) / 2).max(0);
        let y = ((screen_h - h) / 2).max(0);
        resize_and_move_window(hwnd, x, y, w, h)?;
        Ok((x, y, w, h))
    }
}

/// 相对微调窗口坐标 (例如 X/Y 偏移)
pub fn move_window_offset(hwnd: usize, dx: i32, dy: i32) -> Result<(i32, i32, i32, i32)> {
    let (x, y, w, h) = get_window_rect(hwnd)?;
    let new_x = x + dx;
    let new_y = y + dy;
    resize_and_move_window(hwnd, new_x, new_y, w, h)?;
    Ok((new_x, new_y, w, h))
}

/// 相对微调窗口尺寸 (例如 宽度/高度 增减 50px)
pub fn resize_window_delta(hwnd: usize, dw: i32, dh: i32) -> Result<(i32, i32, i32, i32)> {
    let (x, y, w, h) = get_window_rect(hwnd)?;
    let new_w = (w + dw).max(320);
    let new_h = (h + dh).max(240);
    resize_and_move_window(hwnd, x, y, new_w, new_h)?;
    Ok((x, y, new_w, new_h))
}

/// 将窗口激活并置顶到前台显示
pub fn bring_window_to_front(hwnd: usize) -> Result<()> {
    unsafe {
        let h = HWND(hwnd as *mut _);
        let _ = ShowWindow(h, SW_RESTORE);
        let _ = SetForegroundWindow(h);
        Ok(())
    }
}
