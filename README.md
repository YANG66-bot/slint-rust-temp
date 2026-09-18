# LumaWave

基于 **Rust + Slint** 的桌面实时音乐频谱可视化应用，让音乐以光波的形态浮在桌面上。

底部细线型频谱：88 根柱体（粉 → 浅粉 → 天蓝 → 浅紫 → 青五色渐变）、
轻微辉光、Peak 亮点、倒影与底部基线，整体轻盈融入背景；
无信号时柱体以呼吸动画待机。

## 功能

- **多种音源**
  - 内置测试信号（多正弦波模拟音乐，无需任何音频设备）
  - 本地音频文件播放（WAV / MP3 / FLAC / OGG，symphonia 解码）
  - 麦克风实时采集（cpal / WASAPI）
  - 控制条"音源"按钮循环切换，退出时自动写回配置
- **实时频谱分析**：Hann 加窗 FFT(2048) → 对数频段映射（20Hz~20kHz）
  → dB 归一化 → 上升快 / 下降慢平滑 → Peak Hold（每帧 ×0.985 衰减）
- **流畅渲染**：分析线程 40Hz、UI 刷新 60Hz，帧率无关指数插值
  （时间常数 18ms），绝不瞬间归零、无锯齿
- **播放控制**：打开文件（多选）、播放 / 暂停、上一首 / 下一首、
  播放结束自动进入待机呼吸态，按播放键重新播放
- **视觉细节**：逐柱五色渐变、底层辉光柱、Peak 高亮点、
  倒影（比例 0.30 / 透明度 0.22）、底部细基线（3px / 透明度 0.25）

## 构建

```sh
cargo build --release
cargo run
```

推荐使用支持 Slint 扩展的 IDE（VS Code + [Slint extension](https://marketplace.visualstudio.com/items?itemName=Slint.slint)）。

## 架构

线程模型：音频生产者线程 / 频谱分析线程 / Slint UI 线程，
通过无锁 SPSC 环形缓冲与 `crossbeam-channel` 解耦；
音频线程绝不直接操作 Slint。

| 模块 | 职责 |
| --- | --- |
| `error` | 统一错误类型（thiserror） |
| `config` | 用户配置加载 / 保存 / 校验（JSON） |
| `audio` | 音频源抽象、无锁环形缓冲、cpal 采集、混缩重采样 |
| `player` | 文件解码（symphonia）、播放源、播放列表、播放器组合 |
| `spectrum` | FFT、窗函数、频段映射、归一化、平滑、Peak Hold、分析线程 |
| `visualizer` | 纯 Rust 视觉状态模型（插值 / idle 呼吸）与调色 |
| `ui` | Slint 桥接（VecModel 增量写入、60Hz 定时器） |
| `app` | 应用生命周期与装配 |

```
PCM → 单声道 → 加窗 → FFT(2048) → 幅度 → 对数频段 → dB 归一化
    → attack 0.65 / release 0.12 平滑 → Peak 0.985 → UI（60Hz）
```

## 配置

配置文件位于 `%APPDATA%\lumawave\settings.json`（Windows），
首次运行自动生成；所有 DSP / 视觉参数均可调整并在加载时校验夹取：

- `audio`：音源类型、音量
- `spectrum`：柱数、FFT 尺寸、频段范围、dB 范围、平滑系数、
  Peak 衰减、分析帧率、柱增益、高频倾斜补偿、窗函数
- `visual`：调色板（`#RRGGBB` 列表）、各元素不透明度、
  倒影 / 基线 / idle 动画开关

## 日志

默认级别 `info`，通过 `RUST_LOG` 覆盖：

```sh
RUST_LOG=lumawave=debug cargo run
```

## 许可证

MIT
