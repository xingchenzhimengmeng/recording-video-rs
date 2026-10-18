use std::time::{Duration, Instant};

#[link(name = "winmm")]
extern "system" {
    fn timeBeginPeriod(uPeriod: u32) -> u32;
    fn timeEndPeriod(uPeriod: u32) -> u32;
}

/// 60 FPS 硬件帧节拍器 (Pacer)
/// 严格保证 16.666ms 等间隔输出，防止时钟抖动
pub struct FpsPacer {
    frame_interval: Duration,
    next_frame_time: Instant,
    frame_count: u64,
}

impl FpsPacer {
    pub fn new(target_fps: u32) -> Self {
        unsafe {
            let _ = timeBeginPeriod(1);
        }
        Self {
            frame_interval: Duration::from_nanos((1_000_000_000.0 / target_fps as f64) as u64),
            next_frame_time: Instant::now(),
            frame_count: 0,
        }
    }

    /// 等待下一个目标节拍
    pub fn wait_next_tick(&mut self) -> u64 {
        let now = Instant::now();
        if now < self.next_frame_time {
            let remaining = self.next_frame_time - now;
            if remaining > Duration::from_millis(3) {
                std::thread::sleep(remaining - Duration::from_millis(2));
            }
            while Instant::now() < self.next_frame_time {
                std::hint::spin_loop();
            }
        } else if now > self.next_frame_time + self.frame_interval * 2 {
            // 发生较长卡顿时重新对齐时间基准，防止追帧导致瞬时突发爆卡
            self.next_frame_time = now;
        }

        self.next_frame_time += self.frame_interval;
        self.frame_count += 1;
        self.frame_count
    }

    pub fn frame_count(&self) -> u64 {
        self.frame_count
    }
}

impl Drop for FpsPacer {
    fn drop(&mut self) {
        unsafe {
            let _ = timeEndPeriod(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pacer_60fps() {
        let mut pacer = FpsPacer::new(60);
        let t0 = Instant::now();
        for _ in 0..60 {
            pacer.wait_next_tick();
        }
        let elapsed = t0.elapsed();
        println!("60 frames duration: {:?}, fps: {:.2}", elapsed, 60.0 / elapsed.as_secs_f64());
        assert!(elapsed.as_millis() >= 950 && elapsed.as_millis() <= 1050);
    }
}
