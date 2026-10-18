use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::Duration;

/// 录制目标模式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RecordingTarget {
    FullScreen,
    Region { x: i32, y: i32, width: i32, height: i32 },
    Window { hwnd: usize },
}

/// 录制配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingConfig {
    pub target: RecordingTarget,
    pub target_fps: u32,
    pub output_path: PathBuf,
    pub capture_cursor: bool,
    pub capture_system_audio: bool,
    #[serde(default)]
    pub speaker_device_id: Option<String>,
    pub audio_sample_rate: u32,
    pub audio_channels: u16,
}

impl Default for RecordingConfig {
    fn default() -> Self {
        Self {
            target: RecordingTarget::FullScreen,
            target_fps: 60,
            output_path: PathBuf::from("output.mp4"),
            capture_cursor: true,
            capture_system_audio: true,
            speaker_device_id: None,
            audio_sample_rate: 48000,
            audio_channels: 2,
        }
    }
}

/// 像素格式定义
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PixelFormat {
    Rgba,
    Bgra,
    Bgr24,
    Nv12,
}

/// 统一视频帧定义
#[derive(Clone)]
pub struct VideoFrame {
    pub frame_id: u64,
    pub width: u32,
    pub height: u32,
    pub format: PixelFormat,
    pub timestamp_us: u64,
    pub data: Vec<u8>,
}

/// 统一音频数据包 (PCM 48kHz 双声道标准格式)
#[derive(Clone)]
pub struct AudioChunk {
    pub sample_rate: u32,
    pub channels: u16,
    pub timestamp_us: u64,
    pub samples_interleaved: Vec<f32>,
}

/// 流水线控制指令 (主控 UI -> 录制服务)
#[derive(Debug, Clone)]
pub enum PipelineCommand {
    Start(RecordingConfig),
    Pause,
    Resume,
    Stop,
}

/// 实时运行统计数据 (录制服务 -> UI 状态展示)
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RuntimeStats {
    pub is_recording: bool,
    pub is_paused: bool,
    pub elapsed: Duration,
    pub current_fps: f64,
    pub total_video_frames: u64,
    pub total_audio_samples: u64,
    pub dropped_frames: u64,
    pub output_file_size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_default() {
        let cfg = RecordingConfig::default();
        assert_eq!(cfg.target_fps, 60);
        assert_eq!(cfg.audio_sample_rate, 48000);
        assert_eq!(cfg.audio_channels, 2);
    }
}
