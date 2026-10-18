# Recording-video-rs 🎥

基于 Rust 与现代 GUI 架构（Slint）深度复现的 Windows 平台高性能桌面屏幕录制客户端。

参考官方商业录屏软件 Bandicam 的底层设计，采用 Windows 物理硬件时钟锚定、GDI DIBSection 零拷贝显存直取以及 WASAPI 系统级音频回环采集，实现音画 100% 物理时间轴对齐、稳定录制与秒级 FastStart MP4 交付。

---

## ✨ 核心特性

- **🚀 零黑框启动**：原生声明 Windows GUI 子系统（`#![windows_subsystem = "windows"]`），开箱即用。
- **⏱️ 绝对时间轴同步（Wallclock + VFR）**：
  - 彻底解决传统录屏由于固定帧率丢帧/抖动导致的“快进”、“慢放”与音画脱节问题。
  - 每一帧视频根据真实高精度物理时钟锚点（1ms 硬件定时器）赋予呈现时间戳（PTS），录制 N 秒输出就是严格 N 秒。
- **🔊 高保真系统扬声器录制**：基于 Windows 原生 WASAPI Loopback 回环音频采集，自动适配 44.1kHz / 48kHz 重采样，静音期毫秒级填充零电平以锁定音画单调时钟。
- **⚡ 超低延迟实时编码管线**：
  - 环形缓冲队列非阻塞推流，杜绝编码瞬时 I/O 抖动阻塞屏幕采集。
  - 支持自适应智能编码，优先启用零延迟超快编码参数（`libx264 ultrafast zerolatency`）与硬件加速支持。
- **📦 工业级 MP4 容器封装**：支持 FastStart 索引自动前置（`moov` 头部迁移），录制停止即刻封装完毕，视频无需等待二次转码，可直接在浏览器或播放器中秒播、拖动进度条。
- **🎨 现代化 Slint GUI 交互**：轻量级现代化界面，实时展示录制时长、平滑动态帧率（FPS）、已录制帧数及文件体积。

---

## 🏗️ 架构与模块划分

项目采用 Cargo Workspace 多模块架构，职责划分清晰：

```text
bandicam-rs/
├── crates/
│   ├── bandicam-common/    # 共享数据契约（VideoFrame、AudioChunk、RecordingConfig、RuntimeStats）
│   ├── bandicam-capture/   # 屏幕捕获核心（物理分辨率识别、鼠标光标绘制、DIBSection 零拷贝采集、FpsPacer）
│   ├── bandicam-audio/     # 系统声音捕获（WASAPI 渲染设备枚举、Loopback 回环捕获、立体声线性重采样器）
│   ├── bandicam-muxer/     # 容器封装与编码（流式管道推流、VFR 物理时间轴映射、FastStart MP4 生成）
│   └── bandicam-gui/       # Slint 界面、全局热键绑定与录制后台协调服务
├── src/
│   └── main.rs             # 应用程序入口与无界面基准性能压测模式
├── tests/                  # 端到端集成录制测试
├── Cargo.toml
└── README.md
```

---

## 🛠️ 环境依赖与要求

1. **操作系统**：Windows 10 / Windows 11 (64-bit)
2. **构建工具**：[Rust](https://www.rust-lang.org/) (建议 1.75+，稳定版 toolchain)
3. **多媒体后端**：[FFmpeg](https://ffmpeg.org/)
   - 需确保 `ffmpeg.exe` 位于系统环境变量 `PATH` 中，或置于 `C:\my_app\ffmpeg\bin\`。
   - 亦可通过设置环境变量 `FFMPEG_PATH` 指定 ffmpeg 可执行文件绝对路径。

---

## 🚀 编译与运行

### 1. 开发调试运行
在项目根目录下直接运行：
```powershell
cargo run
```
系统将直接唤起图形界面。

### 2. 生产环境单文件编译（推荐）
执行全量 Release 优化编译：
```powershell
cargo build --release
```
编译产物位于：
```text
target/release/bandicam-rs.exe
```
生成的 `bandicam-rs.exe` 为独立 Windows GUI 可执行程序，双击即可无弹窗启动。

### 3. 无 UI 命令行基准压测
用于在 CI 或终端环境中测试硬件编码帧率与音画性能（如全屏录制 5 秒）：
```powershell
cargo run -- --test-record 5
```
录制文件将在 `recordings/benchmark_test.mp4`。

### 4. 运行工程测试
```powershell
cargo test --workspace
```

