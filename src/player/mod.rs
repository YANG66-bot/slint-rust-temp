//! 音频播放模块：文件解码、播放源与播放列表。
//!
//! 职责划分：
//! - [`decoder::AudioDecoder`]：symphonia 封装，输出交错 f32
//! - [`source::FileAudioSource`]：解码 → 单声道混缩 → 重采样 → 环形缓冲
//! - [`playlist::Playlist`]：文件队列与曲内导航
//! - [`Player`]：组合器——管理播放列表 / 暂停状态 / 当前曲目，
//!   创建 [`FileAudioSource`] 交给 [`crate::audio::engine::AudioEngine`] 切换
//!
//! 暂停通过共享 `Arc<AtomicBool>` 实现：`Player` 与当前文件源持有
//! 同一实例；切源后暂停状态复位（新曲目从头播放）。

pub mod decoder;
pub mod playlist;
pub mod source;

pub use decoder::AudioDecoder;
pub use playlist::Playlist;
pub use source::{FileAudioSource, TARGET_SAMPLE_RATE};

use crate::audio::MicrophoneCapture;
use crate::audio::engine::AudioEngine;
use crate::audio::source::TestSignalSource;
use crate::error::AppResult;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// 播放器：组合播放列表、暂停控制与音频引擎。
///
/// `Player` 不直接驱动解码线程；它负责"播放什么"，把
/// [`FileAudioSource`] 交给引擎的 [`AudioEngine::switch_source`]。
/// 播放结束（EOF）通过共享标志暴露给 UI（重新播放由 UI 侧发起）。
pub struct Player {
    engine: AudioEngine,
    playlist: Playlist,
    /// 共享暂停标志（与当前文件源共用同一实例）。
    pause: Arc<AtomicBool>,
    /// 当前播放文件。
    current: Option<PathBuf>,
    /// 当前源播放结束标志快照（EOF 检测供 UI 使用）。
    finished: Arc<AtomicBool>,
}

impl Player {
    /// 创建播放器并接管音频引擎。
    pub fn new(engine: AudioEngine) -> Self {
        Self {
            engine,
            playlist: Playlist::default(),
            pause: Arc::new(AtomicBool::new(false)),
            current: None,
            finished: Arc::new(AtomicBool::new(false)),
        }
    }

    /// 批量加入播放列表，返回本次加入数量。
    pub fn add_files(&mut self, files: impl IntoIterator<Item = PathBuf>) -> usize {
        let before = self.playlist.len();
        self.playlist.add_many(files);
        self.playlist.len() - before
    }

    /// 播放指定文件：创建文件源并切换引擎（自动复位暂停）。
    ///
    /// 文件不可解码时立即返回错误，引擎与播放状态保持不变。
    pub fn play(&mut self, path: &Path) -> AppResult<()> {
        let source = FileAudioSource::new(
            Arc::clone(self.engine.ring()),
            path.to_path_buf(),
            Arc::clone(&self.pause),
        )?;
        self.finished = source.finished_handle();
        self.engine.switch_source(Box::new(source))?;
        self.pause.store(false, Ordering::Release);
        self.current = Some(path.to_path_buf());
        Ok(())
    }

    /// 翻转暂停状态，返回翻转后的状态（`true` = 暂停中）。
    pub fn toggle_pause(&self) -> bool {
        let next = !self.pause.load(Ordering::Acquire);
        self.pause.store(next, Ordering::Release);
        next
    }

    /// 是否处于暂停状态。
    pub fn is_paused(&self) -> bool {
        self.pause.load(Ordering::Acquire)
    }

    /// 是否有当前曲目。
    pub fn has_track(&self) -> bool {
        self.current.is_some()
    }

    /// 当前曲目是否已播放完毕（EOF）。
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// 下一首（到列表末尾返回 `None`，不循环）。
    pub fn next_track(&mut self) -> Option<PathBuf> {
        self.playlist.next().map(Path::to_path_buf)
    }

    /// 上一首（在列表开头返回 `None`）。
    pub fn prev_track(&mut self) -> Option<PathBuf> {
        self.playlist.prev().map(Path::to_path_buf)
    }

    /// 切换到测试信号源（清除当前曲目）。
    pub fn use_test_signal(&mut self) -> AppResult<()> {
        let source = TestSignalSource::new(Arc::clone(self.engine.ring()));
        self.engine.switch_source(Box::new(source))?;
        self.reset_playback_state();
        Ok(())
    }

    /// 切换到麦克风采集源（设备不可用时返回错误，不改变当前源）。
    pub fn use_microphone(&mut self, device_name: Option<&str>) -> AppResult<()> {
        let source = MicrophoneCapture::new(Arc::clone(self.engine.ring()), device_name)?;
        self.engine.switch_source(Box::new(source))?;
        self.reset_playback_state();
        Ok(())
    }

    /// 复位播放状态（切换到非文件源时：无曲目、未暂停、未结束）。
    fn reset_playback_state(&mut self) {
        self.pause.store(false, Ordering::Release);
        self.current = None;
        self.finished = Arc::new(AtomicBool::new(false));
    }

    /// 当前播放文件路径。
    pub fn current_path(&self) -> Option<PathBuf> {
        self.current.clone()
    }

    /// 当前播放文件名（无曲目返回 `None`）。
    pub fn current_file_name(&self) -> Option<String> {
        self.current
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|n| n.to_string_lossy().to_string())
    }

    /// 播放列表长度。
    pub fn track_count(&self) -> usize {
        self.playlist.len()
    }

    /// 当前音频源描述（用于状态栏）。
    pub fn source_description(&self) -> String {
        self.engine.source_description()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::buffer::PcmRingBuffer;
    use std::time::Duration;

    /// 生成最小合法的 16bit 单声道 PCM WAV 文件字节（44 字节标准头）。
    fn make_wav_bytes(sample_rate: u32, samples: &[f32]) -> Vec<u8> {
        let data_len = (samples.len() * 2) as u32;
        let mut out = Vec::with_capacity(44 + data_len as usize);
        out.extend_from_slice(b"RIFF");
        out.extend_from_slice(&(36 + data_len).to_le_bytes());
        out.extend_from_slice(b"WAVE");
        out.extend_from_slice(b"fmt ");
        out.extend_from_slice(&16u32.to_le_bytes());
        out.extend_from_slice(&1u16.to_le_bytes()); // PCM
        out.extend_from_slice(&1u16.to_le_bytes()); // 单声道
        out.extend_from_slice(&sample_rate.to_le_bytes());
        out.extend_from_slice(&(sample_rate * 2).to_le_bytes()); // byte rate
        out.extend_from_slice(&2u16.to_le_bytes()); // block align
        out.extend_from_slice(&16u16.to_le_bytes()); // bits
        out.extend_from_slice(b"data");
        out.extend_from_slice(&data_len.to_le_bytes());
        for &s in samples {
            let v = (s.clamp(-1.0, 1.0) * 32767.0) as i16;
            out.extend_from_slice(&v.to_le_bytes());
        }
        out
    }

    #[test]
    fn pause_and_playlist_states_without_track() {
        let ring = Arc::new(PcmRingBuffer::new(8192));
        let mut player = Player::new(AudioEngine::new(ring));
        assert!(!player.has_track());
        assert!(!player.is_paused());
        assert!(!player.is_finished());
        assert!(player.current_file_name().is_none());
        assert_eq!(player.track_count(), 0);
        assert!(player.next_track().is_none());
        assert!(player.prev_track().is_none());
        // 暂停标志仍然可翻转（无曲目时无害）
        assert!(player.toggle_pause());
        assert!(player.is_paused());
        assert!(!player.toggle_pause());
    }

    #[test]
    fn add_files_and_navigate() {
        let ring = Arc::new(PcmRingBuffer::new(8192));
        let mut player = Player::new(AudioEngine::new(ring));
        let files = vec![
            PathBuf::from("a.mp3"),
            PathBuf::from("b.wav"),
            PathBuf::from("c.flac"),
        ];
        assert_eq!(player.add_files(files), 3);
        assert_eq!(player.track_count(), 3);
        assert_eq!(player.next_track(), Some(PathBuf::from("a.mp3")));
        assert_eq!(player.next_track(), Some(PathBuf::from("b.wav")));
        assert_eq!(player.prev_track(), Some(PathBuf::from("a.mp3")));
        assert_eq!(player.prev_track(), None);
    }

    #[test]
    fn play_wav_file_end_to_end() {
        // 0.1 秒 440Hz 正弦 WAV，走完整链路：play → 解码线程 → 环形缓冲 → EOF
        let dir = std::env::temp_dir().join("lumawave_player_test");
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        let path = dir.join("tone_440.wav");
        let samples: Vec<f32> = (0..4410)
            .map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / 44100.0).sin())
            .collect();
        std::fs::write(&path, make_wav_bytes(44100, &samples)).expect("写入测试 WAV");

        let ring = Arc::new(PcmRingBuffer::new(8192));
        let mut player = Player::new(AudioEngine::new(Arc::clone(&ring)));
        assert_eq!(player.add_files([path.clone()]), 1);
        player.play(&path).expect("播放测试 WAV");

        assert!(player.has_track());
        assert!(!player.is_paused(), "新曲目从头播放（暂停复位）");
        assert_eq!(player.current_file_name().as_deref(), Some("tone_440.wav"));
        assert!(player.source_description().contains("tone_440.wav"));

        std::thread::sleep(Duration::from_millis(150));
        assert!(ring.available() > 0, "播放线程应已写入 PCM 数据");

        // 0.1 秒文件应自然播放结束
        std::thread::sleep(Duration::from_millis(600));
        assert!(player.is_finished(), "短文件应触发 EOF");

        // EOF 后重新播放：finished 标志复位
        player.play(&path).expect("重新播放");
        assert!(!player.is_finished());
        assert!(!player.is_paused());
        // 不清理临时文件（temp 目录，避免与其他并发测试争用文件锁）
    }
}
