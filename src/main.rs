#![windows_subsystem = "windows"]
use anyhow::Result;
use bandicam_common::{RecordingConfig, RecordingTarget};
use bandicam_gui::pipeline_service::ActiveRecordingService;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

#[cfg(windows)]
mod win_init {
    #[link(name = "user32")]
    extern "system" {
        pub fn SetProcessDpiAwarenessContext(value: isize) -> i32;
    }

    #[link(name = "winmm")]
    extern "system" {
        pub fn timeBeginPeriod(uPeriod: u32) -> u32;
        pub fn timeEndPeriod(uPeriod: u32) -> u32;
    }

    pub const DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2: isize = -4;
}

fn main() -> Result<()> {
    #[cfg(windows)]
    unsafe {
        // 1. 设置 Windows Per-Monitor DPI Awareness v2，确保全屏捕获与窗口分辨率真实对齐物理像素
        let _ = win_init::SetProcessDpiAwarenessContext(win_init::DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // 2. 提升系统调度器时钟精度至 1ms，确保 60 FPS (16.666ms) 节拍器微休眠不发生 15.6ms 抖动
        let _ = win_init::timeBeginPeriod(1);
    }

    println!("======================================================");
    println!("   Bandicam-RS - 60 FPS 现代化桌面屏幕录制客户端        ");
    println!("   - 架构: Slint GUI / 60FPS Pacer / 异步分段 MP4       ");
    println!("   - 优化: Direct DIBSection / 1ms 硬件定时器 / DPI感知 ");
    println!("======================================================");

    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 2 && (args[1] == "--test-record" || args[1] == "--benchmark") {
        let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(3);
        let rec_dir = PathBuf::from("recordings");
        let _ = std::fs::create_dir_all(&rec_dir);
        let output_file = rec_dir.join("benchmark_test.mp4");

        if output_file.exists() {
            let _ = std::fs::remove_file(&output_file);
        }

        println!("[Benchmark] 正在启动真实全屏 60 FPS 录制测试 (时长: {secs} 秒)...");
        println!("[Benchmark] 输出路径: {}", output_file.display());

        let config = RecordingConfig {
            target: RecordingTarget::FullScreen,
            target_fps: 60,
            capture_cursor: true,
            capture_system_audio: true,
            speaker_device_id: None,
            audio_sample_rate: 48000,
            audio_channels: 2,
            output_path: output_file.clone(),
        };

        let mut service = ActiveRecordingService::start(config)?;
        for _ in 0..(secs * 10) {
            thread::sleep(Duration::from_millis(100));
            if let Some(stats) = service.get_latest_stats() {
                print!("\r[Recording] 时长: {:.1}s | 实时帧率: {:.1} FPS | 总帧数: {} | 文件大小: {:.2} MB",
                    stats.elapsed.as_secs_f64(),
                    stats.current_fps,
                    stats.total_video_frames,
                    stats.output_file_size as f64 / (1024.0 * 1024.0)
                );
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
        }
        println!("\n[Benchmark] 正在停止录制并写入 FastStart 索引头...");
        service.stop();

        println!("[Benchmark] 录制完成！生成文件大小: {} bytes", std::fs::metadata(&output_file)?.len());
        return Ok(());
    }

    let res = bandicam_gui::run_gui();

    #[cfg(windows)]
    unsafe {
        let _ = win_init::timeEndPeriod(1);
    }

    res
}
