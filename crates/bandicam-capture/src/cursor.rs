use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::UI::WindowsAndMessaging::*;

static LAST_CURSOR_HANDLE: AtomicUsize = AtomicUsize::new(0);
static CACHED_HOTSPOT_X: AtomicI32 = AtomicI32::new(0);
static CACHED_HOTSPOT_Y: AtomicI32 = AtomicI32::new(0);

/// 对应 Bandicam 的 AVCDrawMouseEffect
/// 将系统真实鼠标指针及点击特效实时合成到内存位图中，并处理原点相对位移
pub struct MouseCursorDrawer;

impl MouseCursorDrawer {
    /// 将当前系统的光标渲染至目标内存设备上下文 (h_mem_dc)
    /// origin_x, origin_y 用于处理区域或窗口录制时的局部坐标系对齐
    pub fn draw_cursor(hdc: HDC, origin_x: i32, origin_y: i32) {
        unsafe {
            let mut ci = CURSORINFO {
                cbSize: std::mem::size_of::<CURSORINFO>() as u32,
                ..Default::default()
            };

            if GetCursorInfo(&mut ci).is_ok() && ci.flags == CURSOR_SHOWING && !ci.hCursor.0.is_null() {
                let cur_handle = ci.hCursor.0 as usize;
                let (hot_x, hot_y) = if cur_handle == LAST_CURSOR_HANDLE.load(Ordering::Relaxed) {
                    (CACHED_HOTSPOT_X.load(Ordering::Relaxed), CACHED_HOTSPOT_Y.load(Ordering::Relaxed))
                } else {
                    let mut ii = ICONINFO::default();
                    if GetIconInfo(HICON(ci.hCursor.0), &mut ii).is_ok() {
                        let hx = ii.xHotspot as i32;
                        let hy = ii.yHotspot as i32;
                        if !ii.hbmMask.0.is_null() {
                            let _ = DeleteObject(HGDIOBJ(ii.hbmMask.0));
                        }
                        if !ii.hbmColor.0.is_null() {
                            let _ = DeleteObject(HGDIOBJ(ii.hbmColor.0));
                        }
                        LAST_CURSOR_HANDLE.store(cur_handle, Ordering::Relaxed);
                        CACHED_HOTSPOT_X.store(hx, Ordering::Relaxed);
                        CACHED_HOTSPOT_Y.store(hy, Ordering::Relaxed);
                        (hx, hy)
                    } else {
                        (0, 0)
                    }
                };

                let screen_x = ci.ptScreenPos.x;
                let screen_y = ci.ptScreenPos.y;
                let target_x = screen_x - origin_x - hot_x;
                let target_y = screen_y - origin_y - hot_y;

                let _ = DrawIconEx(
                    hdc,
                    target_x,
                    target_y,
                    HICON(ci.hCursor.0),
                    0,
                    0,
                    0,
                    HBRUSH(std::ptr::null_mut()),
                    DI_NORMAL,
                );
            }
        }
    }
}
