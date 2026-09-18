//! 音频引擎：管理音频源生命周期，为频谱分析提供 PCM 数据流。
//!
//! 引擎持有共享环形缓冲与当前音频源，负责源的启动/停止/切换。
//! 频谱分析线程由 `spectrum::analyzer::AnalysisPipeline` 管理，
//! 二者通过 [`PcmRingBuffer`] 解耦（引擎不知道分析器的存在）。

use crate::audio::buffer::PcmRingBuffer;
use crate::audio::source::AudioSource;
use std::sync::Arc;

/// 音频引擎。
pub struct AudioEngine {
    ring: Arc<PcmRingBuffer>,
    source: Option<Box<dyn AudioSource>>,
}

impl AudioEngine {
    /// 创建引擎（暂无音频源，调用 [`AudioEngine::switch_source`] 启动）。
    pub fn new(ring: Arc<PcmRingBuffer>) -> Self {
        Self { ring, source: None }
    }

    /// 共享环形缓冲（分析线程与各音频源共用）。
    pub fn ring(&self) -> &Arc<PcmRingBuffer> {
        &self.ring
    }

    /// 切换音频源：先停止旧源，再启动新源。
    pub fn switch_source(
        &mut self,
        mut source: Box<dyn AudioSource>,
    ) -> crate::error::AppResult<()> {
        if let Some(mut old) = self.source.take()
            && let Err(e) = old.stop()
        {
            tracing::warn!("停止旧音频源失败: {e}");
        }
        tracing::info!("启动音频源: {}", source.describe());
        source.start()?;
        self.source = Some(source);
        Ok(())
    }

    /// 当前音频源的 (采样率, 声道数)。
    pub fn source_format(&self) -> Option<(u32, u16)> {
        self.source
            .as_ref()
            .map(|s| (s.sample_rate(), s.channels()))
    }

    /// 当前音频源描述（用于状态栏）。
    pub fn source_description(&self) -> String {
        self.source
            .as_ref()
            .map(|s| s.describe())
            .unwrap_or_else(|| "无音频源".to_string())
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        if let Some(mut source) = self.source.take()
            && let Err(e) = source.stop()
        {
            tracing::warn!("关闭音频源失败: {e}");
        }
    }
}
