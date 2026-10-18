use anyhow::Result;
use bandicam_common::{RecordingConfig, RecordingTarget};
use bandicam_gui::pipeline_service::ActiveRecordingService;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

#[test]
fn test_end_to_end_60fps_recording() -> Result<()> {
    let output_file = PathBuf::from("test_rec_output.mp4");
    if output_file.exists() {
        let _ = std::fs::remove_file(&output_file);
    }

    let config = RecordingConfig {
        target: RecordingTarget::Region {
            x: 0,
            y: 0,
            width: 320,
            height: 240,
        },
        target_fps: 60,
        capture_cursor: true,
        capture_system_audio: false,
        speaker_device_id: None,
        audio_sample_rate: 48000,
        audio_channels: 2,
        output_path: output_file.clone(),
    };

    let mut service = ActiveRecordingService::start(config)?;

    // 录制 1.5 秒 (约 90 帧)
    thread::sleep(Duration::from_millis(1500));

    if let Some(stats) = service.get_latest_stats() {
        assert!(stats.current_fps >= 30.0, "FPS should be smooth, got: {}", stats.current_fps);
        assert!(stats.total_video_frames > 0, "Should have encoded frames");
    }

    service.stop();
    assert!(output_file.exists(), "Output MP4 must exist");

    let file_size = std::fs::metadata(&output_file)?.len();
    assert!(file_size > 1024, "Output MP4 should be non-empty (got {} bytes)", file_size);

    // 清理测试产物
    let _ = std::fs::remove_file(&output_file);
    Ok(())
}

#[test]
fn test_end_to_end_audio_and_video_recording() -> Result<()> {
    let output_file = PathBuf::from("test_rec_audio_video_sync.mp4");
    if output_file.exists() {
        let _ = std::fs::remove_file(&output_file);
    }

    let config = RecordingConfig {
        target: RecordingTarget::Region {
            x: 0,
            y: 0,
            width: 320,
            height: 240,
        },
        target_fps: 60,
        capture_cursor: false,
        capture_system_audio: true,
        speaker_device_id: None,
        audio_sample_rate: 48000,
        audio_channels: 2,
        output_path: output_file.clone(),
    };

    let mut service = ActiveRecordingService::start(config)?;

    // 录制 2.0 秒
    thread::sleep(Duration::from_millis(2000));

    service.stop();
    assert!(output_file.exists(), "Output MP4 must exist");

    let file_size = std::fs::metadata(&output_file)?.len();
    assert!(file_size > 1024, "Output MP4 should be non-empty (got {} bytes)", file_size);

    // 清理测试产物
    let _ = std::fs::remove_file(&output_file);
    Ok(())
}
