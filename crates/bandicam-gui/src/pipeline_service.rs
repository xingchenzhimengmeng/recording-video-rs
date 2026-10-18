use anyhow::Result;
use bandicam_audio::AudioCaptureService;
use bandicam_capture::pacer::FpsPacer;
use bandicam_capture::ScreenCapturer;
use bandicam_common::{AudioChunk, RecordingConfig, RuntimeStats, VideoFrame};
use bandicam_muxer::Mp4Muxer;
use crossbeam_channel::{bounded, Receiver, Sender};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

/// 录制引擎后台协调服务
pub struct ActiveRecordingService {
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    capture_handle: Option<JoinHandle<()>>,
    mux_handle: Option<JoinHandle<()>>,
    audio_service: Option<AudioCaptureService>,
    latest_stats: Arc<Mutex<RuntimeStats>>,
}

impl ActiveRecordingService {
    pub fn start(config: RecordingConfig) -> Result<Self> {
        let running = Arc::new(AtomicBool::new(true));
        let running_capture = running.clone();
        let running_mux = running.clone();
        let paused = Arc::new(AtomicBool::new(false));
        let paused_capture = paused.clone();
        let paused_mux = paused.clone();

        // 环形缓冲队列扩大至 120 帧 (可容纳整整 2 秒的并发缓冲，确保采集绝不阻塞)
        let (video_tx, video_rx): (Sender<VideoFrame>, Receiver<VideoFrame>) = bounded(120);
        let (audio_tx, audio_rx): (Sender<AudioChunk>, Receiver<AudioChunk>) = bounded(512);

        let captured_frames = Arc::new(AtomicU64::new(0));
        let captured_frames_capture = captured_frames.clone();

        let latest_stats = Arc::new(Mutex::new(RuntimeStats::default()));
        let latest_stats_mux = latest_stats.clone();

        let target_fps = config.target_fps;
        let target = config.target;
        let capture_cursor = config.capture_cursor;
        let output_path = config.output_path.clone();
        let has_audio = config.capture_system_audio;

        // 探测采集尺寸
        let temp = ScreenCapturer::new(target, capture_cursor)?;
        let (width, height) = temp.dimensions();
        drop(temp);

        // 1. 同步预先创建并启动 Muxer 进程管道，确保音视频推流第一纳秒即可直接写入
        let mut muxer = Mp4Muxer::create_with_audio(&output_path, width, height, target_fps, has_audio)?;

        let record_start_time = Instant::now();
        let record_start_time_capture = record_start_time;
        let record_start_time_mux = record_start_time;

        // 2. 采集线程 (严格按 60 FPS Pacer 节拍器生产每一帧真实画面)
        let capture_handle = thread::spawn(move || {
            let mut capturer = match ScreenCapturer::new(target, capture_cursor) {
                Ok(c) => c,
                Err(_) => return,
            };
            let mut pacer = FpsPacer::new(target_fps);

            while running_capture.load(Ordering::SeqCst) {
                let frame_id = pacer.wait_next_tick();
                if paused_capture.load(Ordering::SeqCst) {
                    continue;
                }
                if let Ok(mut frame) = capturer.capture_frame(frame_id) {
                    frame.timestamp_us = record_start_time_capture.elapsed().as_micros() as u64;
                    captured_frames_capture.fetch_add(1, Ordering::Relaxed);
                    // 采用安全超时发送 (10ms)，如果编码积压则平滑丢弃超载帧，杜绝主采集时钟抖动
                    let _ = video_tx.try_send(frame);
                }
            }
        });

        // 3. 编码与 Muxer 流水线线程 (严格 1:1 消费每一帧真实视频，不再丢帧或伪补帧)
        let mux_handle = thread::spawn(move || {
            let start_time = record_start_time_mux;
            let mut last_stats_send = Instant::now();
            let mut last_sample_time = Instant::now();
            let mut last_sample_frames = 0u64;
            let mut current_smooth_fps = target_fps as f64;

            let mut total_written_video_frames: u64 = 0;
            let mut total_written_audio_samples: u64 = 0;

            while running_mux.load(Ordering::SeqCst) || !video_rx.is_empty() || !audio_rx.is_empty() {
                // 1. 尽可能读取所有音频采样并写入
                while let Ok(chunk) = audio_rx.try_recv() {
                    total_written_audio_samples += chunk.samples_interleaved.len() as u64;
                    let _ = muxer.write_audio_chunk(&chunk);
                }

                // 2. 真实按顺序消费每一个视频帧，不跳帧、不机械重复最后一帧
                let mut processed_video = false;
                while let Ok(frame) = video_rx.try_recv() {
                    processed_video = true;
                    if !paused_mux.load(Ordering::Relaxed) {
                        if muxer.write_video_frame(&frame).is_ok() {
                            total_written_video_frames += 1;
                        }
                    }
                }

                if !processed_video {
                    thread::sleep(Duration::from_millis(1));
                }

                // 每 200ms 对采样区间帧率进行计算并平滑滤波
                if last_sample_time.elapsed() >= Duration::from_millis(200) {
                    let elapsed_sample = last_sample_time.elapsed().as_secs_f64();
                    let frames_delta = total_written_video_frames.saturating_sub(last_sample_frames);
                    let raw_fps = (frames_delta as f64 / elapsed_sample).min(120.0);
                    current_smooth_fps = current_smooth_fps * 0.3 + raw_fps * 0.7;
                    last_sample_frames = total_written_video_frames;
                    last_sample_time = Instant::now();
                }

                // 每 100ms 更新最新运行时状态
                if last_stats_send.elapsed() >= Duration::from_millis(100) {
                    last_stats_send = Instant::now();
                    let elapsed = start_time.elapsed();
                    let file_size = muxer.current_file_size();

                    let stats = RuntimeStats {
                        is_recording: true,
                        is_paused: paused_mux.load(Ordering::Relaxed),
                        elapsed,
                        current_fps: current_smooth_fps,
                        total_video_frames: total_written_video_frames,
                        total_audio_samples: total_written_audio_samples,
                        dropped_frames: 0,
                        output_file_size: file_size,
                    };
                    if let Ok(mut g) = latest_stats_mux.lock() {
                        *g = stats;
                    }
                }
            }

            // 写入可能残余的最后音频切片
            while let Ok(chunk) = audio_rx.try_recv() {
                let _ = muxer.write_audio_chunk(&chunk);
            }

            let _ = muxer.finish();
        });

        // 4. 音频服务 (传入统一的时钟起点 record_start_time，确保音视频首帧完全同步)
        let audio_service = if has_audio {
            let audio_tx_clone = audio_tx.clone();
            match AudioCaptureService::start(
                config.speaker_device_id.clone(),
                config.audio_sample_rate,
                config.audio_channels,
                move |chunk| {
                    let _ = audio_tx_clone.try_send(chunk);
                },
                record_start_time,
            ) {
                Ok(s) => Some(s),
                Err(e) => {
                    eprintln!("[Audio] 启动扬声器音频采集异常: {e:?}");
                    None
                }
            }
        } else {
            None
        };

        Ok(Self {
            running,
            paused,
            capture_handle: Some(capture_handle),
            mux_handle: Some(mux_handle),
            audio_service,
            latest_stats,
        })
    }

    pub fn toggle_pause(&self) -> bool {
        let current = self.paused.load(Ordering::SeqCst);
        let next = !current;
        self.paused.store(next, Ordering::SeqCst);
        next
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    pub fn get_latest_stats(&self) -> Option<RuntimeStats> {
        self.latest_stats.lock().ok().map(|g| g.clone())
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(mut a) = self.audio_service.take() {
            a.stop();
        }
        if let Some(h) = self.capture_handle.take() {
            let _ = h.join();
        }
        if let Some(h) = self.mux_handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for ActiveRecordingService {
    fn drop(&mut self) {
        self.stop();
    }
}