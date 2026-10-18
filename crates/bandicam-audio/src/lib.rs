use anyhow::Result;
use bandicam_common::AudioChunk;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::Shell::PropertiesSystem::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

/// 枚举系统中的可用音频输出设备 (扬声器/耳机)
pub fn list_audio_endpoints() -> Vec<AudioDeviceInfo> {
    thread::spawn(move || {
        let mut devices = Vec::new();
        unsafe {
            let co_init_res = CoInitializeEx(None, COINIT_MULTITHREADED);
            if let Ok(enumerator) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                let default_id = enumerator
                    .GetDefaultAudioEndpoint(eRender, eConsole)
                    .ok()
                    .and_then(|d| d.GetId().ok())
                    .map(|p| p.to_string().unwrap_or_default())
                    .unwrap_or_default();

                if let Ok(collection) = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) {
                    let count = collection.GetCount().unwrap_or(0);
                    for i in 0..count {
                        if let Ok(dev) = collection.Item(i) {
                            let id = dev.GetId().ok().map(|p| p.to_string().unwrap_or_default()).unwrap_or_default();
                            let is_default = !id.is_empty() && id == default_id;
                            let mut friendly_name = String::new();
                            if let Ok(store) = dev.OpenPropertyStore(STGM_READ) {
                                if let Ok(prop) = store.GetValue(&PKEY_DEVICE_FRIENDLY_NAME) {
                                    friendly_name = prop.to_string();
                                }
                            }
                            if friendly_name.is_empty() {
                                friendly_name = format!("音频输出设备 [{i}]");
                            }
                            let name = if is_default {
                                format!("【默认扬声器】 {friendly_name}")
                            } else {
                                friendly_name
                            };
                            devices.push(AudioDeviceInfo {
                                id,
                                name,
                                is_default,
                            });
                        }
                    }
                }
            }
            if co_init_res.is_ok() {
                CoUninitialize();
            }
        }
        devices
    })
    .join()
    .unwrap_or_default()
}

/// 获取带有【默认系统输出】选项的完整音频输出设备列表 (支持热插拔实时发现)
pub fn get_speaker_options() -> Vec<AudioDeviceInfo> {
    let endpoints = list_audio_endpoints();
    let mut options = Vec::new();
    let default_name = endpoints
        .iter()
        .find(|d| d.is_default)
        .map(|d| d.name.as_str())
        .unwrap_or("系统主设备");

    options.push(AudioDeviceInfo {
        id: String::new(),
        name: format!("【自动】跟随系统默认输出 (当前: {default_name})"),
        is_default: true,
    });

    options.extend(endpoints);
    options
}

/// 高保真立体声线性重采样器 (可将任意源采样率如 44.1kHz / 96kHz 精准转换为目标 48kHz)
pub fn resample_stereo_interleaved(input: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if src_rate == dst_rate || input.is_empty() {
        return input.to_vec();
    }
    let input_frames = input.len() / 2;
    if input_frames == 0 {
        return Vec::new();
    }
    let ratio = src_rate as f64 / dst_rate as f64;
    let output_frames = ((input_frames as f64) / ratio).round() as usize;
    let mut output = Vec::with_capacity(output_frames * 2);

    for i in 0..output_frames {
        let src_pos = i as f64 * ratio;
        let idx0 = src_pos.floor() as usize;
        let frac = (src_pos - idx0 as f64) as f32;
        let idx1 = (idx0 + 1).min(input_frames - 1);

        let left0 = input[idx0 * 2];
        let right0 = input[idx0 * 2 + 1];
        let left1 = input[idx1 * 2];
        let right1 = input[idx1 * 2 + 1];

        output.push(left0 + frac * (left1 - left0));
        output.push(right0 + frac * (right1 - right0));
    }
    output
}

struct WasapiLoopbackDevice {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    channels: u16,
    sample_rate: u32,
    bits_per_sample: u16,
    is_float: bool,
}

impl WasapiLoopbackDevice {
    pub fn new(device_id: Option<&str>) -> Result<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = if let Some(id_str) = device_id.filter(|s| !s.is_empty()) {
                let wide_id: Vec<u16> = id_str.encode_utf16().chain(std::iter::once(0)).collect();
                enumerator.GetDevice(windows::core::PCWSTR(wide_id.as_ptr()))
                    .or_else(|_| enumerator.GetDefaultAudioEndpoint(eRender, eConsole))?
            } else {
                enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?
            };
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let p_fmt = client.GetMixFormat()?;
            let fmt = *p_fmt;
            let channels = fmt.nChannels;
            let sample_rate = fmt.nSamplesPerSec;
            let bits_per_sample = fmt.wBitsPerSample;

            let mut is_float = fmt.wFormatTag == 3;
            if fmt.wFormatTag == 0xFFFE && fmt.cbSize >= 22 {
                let ext = &*(p_fmt as *const WAVEFORMATEXTENSIBLE);
                if ext.SubFormat.data1 == 3 {
                    is_float = true;
                }
            }

            let base_flags = AUDCLNT_STREAMFLAGS_LOOPBACK;

            // 优先启用 AUTOCONVERTPCM 保证与硬件混音流平滑兼容，若失败则回退基础模式
            let init_res = client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                base_flags | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                1_000_000,
                0,
                p_fmt,
                None,
            ).or_else(|_| {
                client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    base_flags,
                    1_000_000,
                    0,
                    p_fmt,
                    None,
                )
            });

            init_res?;

            let capture: IAudioCaptureClient = client.GetService()?;
            client.Start()?;

            Ok(Self {
                client,
                capture,
                channels,
                sample_rate,
                bits_per_sample,
                is_float,
            })
        }
    }

    pub fn read_available_samples(&mut self) -> Vec<f32> {
        let mut raw_frames = Vec::new();
        unsafe {
            loop {
                let packet_size = match self.capture.GetNextPacketSize() {
                    Ok(sz) => sz,
                    Err(_) => break,
                };
                if packet_size == 0 {
                    break;
                }

                let mut p_data: *mut u8 = std::ptr::null_mut();
                let mut num_frames: u32 = 0;
                let mut flags: u32 = 0;

                if self.capture.GetBuffer(&mut p_data, &mut num_frames, &mut flags, None, None).is_err() {
                    break;
                }

                let ch = self.channels as usize;
                if flags & 2 != 0 || p_data.is_null() {
                    raw_frames.resize(raw_frames.len() + (num_frames as usize * ch), 0.0f32);
                } else if self.is_float && self.bits_per_sample == 32 {
                    let float_slice = std::slice::from_raw_parts(p_data as *const f32, num_frames as usize * ch);
                    raw_frames.extend_from_slice(float_slice);
                } else if self.bits_per_sample == 16 {
                    let i16_slice = std::slice::from_raw_parts(p_data as *const i16, num_frames as usize * ch);
                    for &s in i16_slice {
                        raw_frames.push(s as f32 / 32768.0);
                    }
                } else if self.bits_per_sample == 32 {
                    let i32_slice = std::slice::from_raw_parts(p_data as *const i32, num_frames as usize * ch);
                    for &s in i32_slice {
                        raw_frames.push((s >> 8) as f32 / 8388608.0);
                    }
                } else {
                    raw_frames.resize(raw_frames.len() + (num_frames as usize * ch), 0.0f32);
                }

                let _ = self.capture.ReleaseBuffer(num_frames);
            }
        }

        // 声道统一定制为 2 声道 Stereo
        let mut stereo_samples = Vec::with_capacity(raw_frames.len() * 2 / self.channels.max(1) as usize);
        if self.channels == 1 {
            for &s in &raw_frames {
                stereo_samples.push(s);
                stereo_samples.push(s);
            }
        } else if self.channels == 2 {
            stereo_samples = raw_frames;
        } else if self.channels > 2 {
            let ch = self.channels as usize;
            for chunk in raw_frames.chunks_exact(ch) {
                stereo_samples.push(chunk[0]);
                stereo_samples.push(chunk[1]);
            }
        } else {
            stereo_samples = raw_frames;
        }

        stereo_samples
    }

    pub fn read_available_samples_resampled(&mut self, target_sample_rate: u32) -> Vec<f32> {
        let stereo = self.read_available_samples();
        if stereo.is_empty() || self.sample_rate == target_sample_rate {
            stereo
        } else {
            resample_stereo_interleaved(&stereo, self.sample_rate, target_sample_rate)
        }
    }
}

impl Drop for WasapiLoopbackDevice {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
        }
    }
}

/// 音频采集管理器 (对应 Bandicam 的 CAudioCaptureWASAPI)
/// 专注于系统声音回环捕获，以绝对时间轴实现样本级严格同步对齐
pub struct AudioCaptureService {
    running: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl AudioCaptureService {
    pub fn start<F>(
        system_device_id: Option<String>,
        target_sample_rate: u32,
        target_channels: u16,
        mut on_chunk: F,
        record_start_time: Instant,
    ) -> Result<Self>
    where
        F: FnMut(AudioChunk) + Send + 'static,
    {
        let running = Arc::new(AtomicBool::new(true));
        let running_flag = running.clone();

        let handle = thread::spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }

            let mut sys_device = match WasapiLoopbackDevice::new(system_device_id.as_deref()) {
                Ok(dev) => Some(dev),
                Err(e) => {
                    eprintln!("[Audio] 扬声器音频捕获启动失败: {e:?}");
                    None
                }
            };

            let chunk_interval = Duration::from_millis(10);
            let channels = target_channels as usize;
            let mut total_emitted_frames: u64 = 0;

            while running_flag.load(Ordering::SeqCst) {
                thread::sleep(chunk_interval);

                let available_samples = sys_device
                    .as_mut()
                    .map(|d| d.read_available_samples_resampled(target_sample_rate))
                    .unwrap_or_default();

                let elapsed = record_start_time.elapsed();
                let expected_frames = (elapsed.as_secs_f64() * target_sample_rate as f64) as u64;
                let needed_frames = expected_frames.saturating_sub(total_emitted_frames) as usize;

                if needed_frames > 0 {
                    let mut chunk_samples = available_samples;
                    let available_frames = chunk_samples.len() / channels;

                    if available_frames < needed_frames {
                        // 系统处于静音或音频未就绪时，用纯零静音补齐，绝对锁定音视频时间轴
                        chunk_samples.resize(needed_frames * channels, 0.0f32);
                    } else if available_frames > needed_frames + (target_sample_rate as usize / 5) {
                        // 出现偶尔突发抖动时，截断至当前时钟基准
                        chunk_samples.truncate(needed_frames * channels);
                    }

                    let frames_in_chunk = chunk_samples.len() / channels;
                    if frames_in_chunk > 0 {
                        let chunk_timestamp_us = (total_emitted_frames as f64 / target_sample_rate as f64 * 1_000_000.0) as u64;
                        total_emitted_frames += frames_in_chunk as u64;

                        on_chunk(AudioChunk {
                            sample_rate: target_sample_rate,
                            channels: target_channels,
                            timestamp_us: chunk_timestamp_us,
                            samples_interleaved: chunk_samples,
                        });
                    }
                }
            }

            drop(sys_device);
            unsafe {
                CoUninitialize();
            }
        });

        Ok(Self {
            running,
            handle: Some(handle),
        })
    }

    pub fn stop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for AudioCaptureService {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resampler() {
        let input = vec![0.0f32, 0.0, 1.0, 1.0];
        let out = resample_stereo_interleaved(&input, 48000, 48000);
        assert_eq!(out, input);
    }
}