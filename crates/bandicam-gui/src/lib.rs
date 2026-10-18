slint::include_modules!();

pub mod pipeline_service;

use anyhow::Result;
use bandicam_audio::{get_speaker_options, AudioDeviceInfo};
use bandicam_capture::{
    bring_window_to_front, center_window_on_screen, enumerate_windows, move_window_offset,
    resize_and_move_window, resize_window_delta, ScreenCapturer, WindowItem,
};
use bandicam_common::{RecordingConfig, RecordingTarget};
use pipeline_service::ActiveRecordingService;
use slint::ComponentHandle;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;
use windows::core::PCWSTR;
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{RegisterHotKey, UnregisterHotKey, MOD_NOREPEAT};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::{GetMessageW, MSG, WM_HOTKEY};

fn get_default_output_dir() -> PathBuf {
    // 优先使用当前程序所在目录下的 recordings 文件夹，避免系统 Videos 目录受权限保护无法写入
    if let Ok(exe_path) = std::env::current_exe() {
        if let Some(exe_dir) = exe_path.parent() {
            let local_rec = exe_dir.join("recordings");
            if std::fs::create_dir_all(&local_rec).is_ok() {
                return local_rec;
            }
        }
    }
    let cwd_rec = PathBuf::from("recordings");
    if std::fs::create_dir_all(&cwd_rec).is_ok() {
        return cwd_rec;
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let videos = PathBuf::from(profile).join("Videos").join("Bandicam-RS");
        let _ = std::fs::create_dir_all(&videos);
        videos
    } else {
        let fallback = PathBuf::from("recordings");
        let _ = std::fs::create_dir_all(&fallback);
        fallback
    }
}

pub fn run_gui() -> Result<()> {
    let main_window = MainWindow::new()?;
    let service_slot: Arc<Mutex<Option<ActiveRecordingService>>> = Arc::new(Mutex::new(None));
    let last_output_file: Arc<Mutex<Option<PathBuf>>> = Arc::new(Mutex::new(None));
    let current_output_dir = Arc::new(Mutex::new(get_default_output_dir()));

    let region_rect = Arc::new(Mutex::new((100i32, 100i32, 1280i32, 720i32)));
    let windows_cache: Arc<Mutex<Vec<WindowItem>>> = Arc::new(Mutex::new(enumerate_windows()));
    let selected_window_idx: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));

    // 音频输入输出设备列表
    let speaker_devices: Arc<Mutex<Vec<AudioDeviceInfo>>> = Arc::new(Mutex::new(get_speaker_options()));
    let selected_speaker_idx: Arc<Mutex<usize>> = Arc::new(Mutex::new(0));

    let dir_display = current_output_dir.lock().unwrap().to_string_lossy().to_string();
    main_window.set_output_dir_text(dir_display.into());

    // 初始化音频设备显示
    {
        let spk_list = speaker_devices.lock().unwrap();
        if let Some(first) = spk_list.first() {
            main_window.set_speaker_device_name(first.name.clone().into());
        }
    }

    // 启动时探测物理全屏真实尺寸 (结合 DPI 感知与物理 GDI 硬件设备上下文)
    if let Ok(capturer) = ScreenCapturer::new(RecordingTarget::FullScreen, false) {
        let (w, h) = capturer.dimensions();
        main_window.set_resolution_text(format!("{w} x {h}").into());
    } else {
        main_window.set_resolution_text("1920 x 1080 (默认)".into());
    }

    // 初始化首个目标窗口信息
    {
        let list = windows_cache.lock().unwrap();
        if let Some(first) = list.first() {
            main_window.set_target_window_title(first.title.clone().into());
            main_window.set_target_window_geometry(format!("{}x{} @ ({}, {})", first.width, first.height, first.x, first.y).into());
        }
    }

    // 注册全局热键监听 (F12 开始/停止，F11 暂停/继续)
    {
        let app_handle = main_window.as_weak();
        thread::spawn(move || {
            unsafe {
                let _ = RegisterHotKey(HWND(std::ptr::null_mut()), 1001, MOD_NOREPEAT, 0x7B);
                let _ = RegisterHotKey(HWND(std::ptr::null_mut()), 1002, MOD_NOREPEAT, 0x7A);

                let mut msg = MSG::default();
                while GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0).as_bool() {
                    if msg.message == WM_HOTKEY {
                        if msg.wParam.0 == 1001 {
                            let _ = app_handle.upgrade_in_event_loop(|mw| {
                                mw.invoke_toggle_record();
                            });
                        } else if msg.wParam.0 == 1002 {
                            let _ = app_handle.upgrade_in_event_loop(|mw| {
                                mw.invoke_toggle_pause();
                            });
                        }
                    }
                }

                let _ = UnregisterHotKey(HWND(std::ptr::null_mut()), 1001);
                let _ = UnregisterHotKey(HWND(std::ptr::null_mut()), 1002);
            }
        });
    }

    // 录制开始/停止回调
    let mw_weak = main_window.as_weak();
    let service_slot_toggle = service_slot.clone();
    let last_file_toggle = last_output_file.clone();
    let output_dir_toggle = current_output_dir.clone();
    let region_rect_toggle = region_rect.clone();
    let windows_toggle = windows_cache.clone();
    let window_idx_toggle = selected_window_idx.clone();
    let spk_list_toggle = speaker_devices.clone();
    let spk_idx_toggle = selected_speaker_idx.clone();
    main_window.on_toggle_record(move || {
        let mut slot = service_slot_toggle.lock().unwrap();
        if let Some(mut service) = slot.take() {
            service.stop();
            let saved_name = last_file_toggle
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|p| p.file_name())
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_else(|| "output.mp4".into());
            if let Some(mw) = mw_weak.upgrade() {
                mw.set_is_recording(false);
                mw.set_is_paused(false);
                let out_dir = output_dir_toggle.lock().unwrap().clone();
                let full_saved_path = out_dir.join(&saved_name);
                mw.set_status_text(format!("录制完成！已保存: {saved_name}").into());
                mw.set_last_saved_file(saved_name.clone().into());
                mw.set_output_dir_text(full_saved_path.to_string_lossy().to_string().into());
                mw.set_time_display("00:00:00".into());
            }
        } else {
            let mode = mw_weak.upgrade().map(|w| w.get_selected_mode()).unwrap_or(0);
            let target = match mode {
                1 => {
                    let (x, y, w, h) = *region_rect_toggle.lock().unwrap();
                    RecordingTarget::Region { x, y, width: w, height: h }
                }
                2 => {
                    let list = windows_toggle.lock().unwrap();
                    let idx = *window_idx_toggle.lock().unwrap();
                    if let Some(wnd) = list.get(idx) {
                        RecordingTarget::Window { hwnd: wnd.hwnd }
                    } else {
                        RecordingTarget::FullScreen
                    }
                }
                _ => RecordingTarget::FullScreen,
            };

            let out_dir = output_dir_toggle.lock().unwrap().clone();
            let _ = std::fs::create_dir_all(&out_dir);
            let timestamp = chrono::Local::now().format("%Y-%m-%d_%H-%M-%S").to_string();
            let filename = format!("bandicam_{timestamp}.mp4");
            let file_path = out_dir.join(&filename);
            *last_file_toggle.lock().unwrap() = Some(file_path.clone());

            let capture_speaker = if let Some(mw) = mw_weak.upgrade() {
                mw.get_capture_speaker()
            } else {
                true
            };

            let speaker_id = if capture_speaker {
                let list = spk_list_toggle.lock().unwrap();
                let idx = *spk_idx_toggle.lock().unwrap();
                list.get(idx).and_then(|d| if d.id.is_empty() { None } else { Some(d.id.clone()) })
            } else { None };

            

            let config = RecordingConfig {
                target,
                target_fps: 60,
                output_path: file_path,
                capture_cursor: true,
                capture_system_audio: capture_speaker,
                speaker_device_id: speaker_id,
                audio_sample_rate: 48000,
                audio_channels: 2,
            };

            if let Ok(svc) = ActiveRecordingService::start(config) {
                *slot = Some(svc);
                if let Some(mw) = mw_weak.upgrade() {
                    mw.set_is_recording(true);
                    mw.set_is_paused(false);
                    mw.set_status_text(format!("正在录制: {filename} (REC / 60 FPS)").into());
                }
            } else if let Some(mw) = mw_weak.upgrade() {
                mw.set_status_text("录制启动失败，请检查麦克风或视频采集设备".into());
            }
        }
    });

    // 暂停/继续录制回调
    let service_slot_pause = service_slot.clone();
    let mw_weak_pause = main_window.as_weak();
    main_window.on_toggle_pause(move || {
        let slot = service_slot_pause.lock().unwrap();
        if let Some(svc) = slot.as_ref() {
            let is_now_paused = svc.toggle_pause();
            if let Some(mw) = mw_weak_pause.upgrade() {
                mw.set_is_paused(is_now_paused);
            }
        }
    });

    // 更改保存文件夹回调 (调用系统原生选择目录对话框)
    let mw_weak_choose = main_window.as_weak();
    let current_dir_choose = current_output_dir.clone();
    main_window.on_choose_output_dir(move || {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            if let Ok(dialog) = CoCreateInstance::<_, IFileOpenDialog>(&FileOpenDialog, None, CLSCTX_ALL) {
                let _ = dialog.SetOptions(FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM);
                let title: Vec<u16> = "选择录屏保存文件夹\0".encode_utf16().collect();
                let _ = dialog.SetTitle(PCWSTR(title.as_ptr()));
                if dialog.Show(None).is_ok() {
                    if let Ok(item) = dialog.GetResult() {
                        if let Ok(psz) = item.GetDisplayName(SIGDN_FILESYSPATH) {
                            if let Ok(path_str) = psz.to_string() {
                                let chosen = PathBuf::from(path_str);
                                *current_dir_choose.lock().unwrap() = chosen.clone();
                                if let Some(mw) = mw_weak_choose.upgrade() {
                                    mw.set_output_dir_text(chosen.to_string_lossy().to_string().into());
                                    mw.set_status_text(format!("已更新保存路径: {}", chosen.display()).into());
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    // 打开保存目录回调 (调用系统资源管理器)
    let output_dir_open = current_output_dir.clone();
    let last_file_open = last_output_file.clone();
    main_window.on_open_output_dir(move || {
        let file_opt = last_file_open.lock().unwrap().clone();
        if let Some(file) = file_opt {
            if file.exists() {
                let _ = std::process::Command::new("explorer")
                    .arg(format!("/select,{}", file.display()))
                    .spawn();
                return;
            }
        }
        let dir = output_dir_open.lock().unwrap().clone();
        let _ = std::process::Command::new("explorer").arg(&dir).spawn();
    });

    // 立即播放最近录制的文件
    let last_file_play = last_output_file.clone();
    main_window.on_play_last_recording(move || {
        let file_opt = last_file_play.lock().unwrap().clone();
        if let Some(file) = file_opt {
            if file.exists() {
                let _ = std::process::Command::new("cmd")
                    .args(["/c", "start", "", &file.to_string_lossy()])
                    .spawn();
            }
        }
    });

    // 在资源管理器中高亮定位最近录制的文件
    let last_file_reveal = last_output_file.clone();
    main_window.on_reveal_last_recording(move || {
        let file_opt = last_file_reveal.lock().unwrap().clone();
        if let Some(file) = file_opt {
            if file.exists() {
                let _ = std::process::Command::new("explorer")
                    .arg(format!("/select,{}", file.display()))
                    .spawn();
            }
        }
    });

    // 区域预设尺寸切换回调
    let mw_weak_preset = main_window.as_weak();
    let region_preset_rect = region_rect.clone();
    main_window.on_set_region_preset(move |preset| {
        let (x, y, w, h) = match preset {
            0 => (0, 0, 1920, 1080),
            1 => (100, 100, 1280, 720),
            2 => (100, 100, 854, 480),
            _ => (100, 100, 1280, 720),
        };
        *region_preset_rect.lock().unwrap() = (x, y, w, h);
        if let Some(mw) = mw_weak_preset.upgrade() {
            mw.set_region_info_text(format!("{w} x {h} @ ({x}, {y})").into());
            mw.set_status_text(format!("已切换录制区域尺寸: {w}x{h}").into());
        }
    });

    // 窗口选择与切换回调
    let mw_weak_win = main_window.as_weak();
    let windows_list_ref = windows_cache.clone();
    let window_idx_ref = selected_window_idx.clone();
    main_window.on_select_next_window(move || {
        let list = windows_list_ref.lock().unwrap();
        if list.is_empty() { return; }
        let mut idx = window_idx_ref.lock().unwrap();
        *idx = (*idx + 1) % list.len();
        let target = &list[*idx];
        if let Some(mw) = mw_weak_win.upgrade() {
            mw.set_target_window_title(target.title.clone().into());
            mw.set_target_window_geometry(format!("{}x{} @ ({}, {})", target.width, target.height, target.x, target.y).into());
            mw.set_status_text(format!("选定窗口: {}", target.title).into());
        }
    });

    // 刷新窗口列表回调
    let mw_weak_refresh = main_window.as_weak();
    let windows_refresh_cache = windows_cache.clone();
    let window_refresh_idx = selected_window_idx.clone();
    main_window.on_refresh_windows(move || {
        let new_list = enumerate_windows();
        let count = new_list.len();
        *windows_refresh_cache.lock().unwrap() = new_list;
        *window_refresh_idx.lock().unwrap() = 0;
        let list = windows_refresh_cache.lock().unwrap();
        if let Some(first) = list.first() {
            if let Some(mw) = mw_weak_refresh.upgrade() {
                mw.set_target_window_title(first.title.clone().into());
                mw.set_target_window_geometry(format!("{}x{} @ ({}, {})", first.width, first.height, first.x, first.y).into());
                mw.set_status_text(format!("已发现 {count} 个活动窗口，已选定首个").into());
            }
        }
    });

    // 前置激活目标窗口
    let windows_activate_cache = windows_cache.clone();
    let window_activate_idx = selected_window_idx.clone();
    main_window.on_activate_target_window(move || {
        let list = windows_activate_cache.lock().unwrap();
        let idx = *window_activate_idx.lock().unwrap();
        if let Some(wnd) = list.get(idx) {
            let _ = bring_window_to_front(wnd.hwnd);
        }
    });

    // 屏幕居中窗口
    let mw_weak_center = main_window.as_weak();
    let windows_center_cache = windows_cache.clone();
    let window_center_idx = selected_window_idx.clone();
    main_window.on_center_target_window(move |_preset| {
        let list = windows_center_cache.lock().unwrap();
        let idx = *window_center_idx.lock().unwrap();
        if let Some(wnd) = list.get(idx) {
            if let Ok((x, y, w, h)) = center_window_on_screen(wnd.hwnd, wnd.width, wnd.height) {
                if let Some(mw) = mw_weak_center.upgrade() {
                    mw.set_target_window_geometry(format!("{w}x{h} @ ({x}, {y})").into());
                    mw.set_status_text(format!("已将窗口居中至 ({x}, {y})").into());
                }
            }
        }
    });

    // 相对移动窗口坐标
    let mw_weak_move = main_window.as_weak();
    let windows_move_cache = windows_cache.clone();
    let window_move_idx = selected_window_idx.clone();
    main_window.on_move_target_window(move |dx, dy| {
        let list = windows_move_cache.lock().unwrap();
        let idx = *window_move_idx.lock().unwrap();
        if let Some(wnd) = list.get(idx) {
            if let Ok((x, y, w, h)) = move_window_offset(wnd.hwnd, dx, dy) {
                if let Some(mw) = mw_weak_move.upgrade() {
                    mw.set_target_window_geometry(format!("{w}x{h} @ ({x}, {y})").into());
                }
            }
        }
    });

    // 相对微调窗口尺寸
    let mw_weak_delta_sz = main_window.as_weak();
    let windows_delta_sz_cache = windows_cache.clone();
    let window_delta_sz_idx = selected_window_idx.clone();
    main_window.on_delta_resize_target_window(move |dw, dh| {
        let list = windows_delta_sz_cache.lock().unwrap();
        let idx = *window_delta_sz_idx.lock().unwrap();
        if let Some(wnd) = list.get(idx) {
            if let Ok((x, y, w, h)) = resize_window_delta(wnd.hwnd, dw, dh) {
                if let Some(mw) = mw_weak_delta_sz.upgrade() {
                    mw.set_target_window_geometry(format!("{w}x{h} @ ({x}, {y})").into());
                }
            }
        }
    });

    // 调整目标窗口预设大小 (1080P 或 720P)
    let mw_weak_resize = main_window.as_weak();
    let windows_resize_cache = windows_cache.clone();
    let window_resize_idx = selected_window_idx.clone();
    main_window.on_resize_target_window(move |preset| {
        let (target_w, target_h) = if preset == 0 { (1920, 1080) } else { (1280, 720) };
        let list = windows_resize_cache.lock().unwrap();
        let idx = *window_resize_idx.lock().unwrap();
        if let Some(wnd) = list.get(idx) {
            if resize_and_move_window(wnd.hwnd, wnd.x, wnd.y, target_w, target_h).is_ok() {
                if let Some(mw) = mw_weak_resize.upgrade() {
                    mw.set_target_window_geometry(format!("{target_w}x{target_h} @ ({}, {})", wnd.x, wnd.y).into());
                    mw.set_status_text(format!("已将窗口调整为 {target_w}x{target_h}").into());
                }
            }
        }
    });

    // 切换扬声器输出设备
    let mw_weak_spk = main_window.as_weak();
    let spk_list_cycle = speaker_devices.clone();
    let spk_idx_cycle = selected_speaker_idx.clone();
    main_window.on_cycle_speaker_device(move || {
        let fresh_list = get_speaker_options();
        let mut list = spk_list_cycle.lock().unwrap();
        *list = fresh_list;
        if list.is_empty() { return; }
        let mut idx = spk_idx_cycle.lock().unwrap();
        *idx = (*idx + 1) % list.len();
        let dev = &list[*idx];
        if let Some(mw) = mw_weak_spk.upgrade() {
            mw.set_speaker_device_name(dev.name.clone().into());
            mw.set_status_text(format!("已切换输出: {} (共 {} 个可选设备)", dev.name, list.len()).into());
        }
    });



    // 定时轮询状态刷新 UI
    let mw_weak_timer = main_window.as_weak();
    let service_slot_timer = service_slot.clone();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(100), move || {
        if let Some(mw) = mw_weak_timer.upgrade() {
            let slot = service_slot_timer.lock().unwrap();
            if let Some(svc) = slot.as_ref() {
                if let Some(stats) = svc.get_latest_stats() {
                    let total_secs = stats.elapsed.as_secs();
                    let hours = total_secs / 3600;
                    let mins = (total_secs % 3600) / 60;
                    let secs = total_secs % 60;
                    mw.set_time_display(format!("{hours:02}:{mins:02}:{secs:02}").into());
                    mw.set_fps_text(format!("{:.1} FPS", stats.current_fps).into());
                    let size_mb = stats.output_file_size as f64 / (1024.0 * 1024.0);
                    mw.set_size_text(format!("{size_mb:.2} MB").into());
                    mw.set_frames_text(format!("{} 帧", stats.total_video_frames).into());
                }
            }
        }
    });

    main_window.run()?;
    Ok(())
}
