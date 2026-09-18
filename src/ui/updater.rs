//! UI 刷新定时器：60Hz 驱动展示状态插值与模型更新。
//!
//! Slint 的 `Timer` 运行在主事件循环内，回调中执行的插值与
//! 模型写入都是轻量纯计算（无锁、无分配），保证不拖累渲染。

use std::time::Duration;

/// 重复执行的 UI 刷新定时器。
///
/// 持有期间定时器持续触发；drop 后停止。
pub struct UiUpdater {
    timer: slint::Timer,
}

impl UiUpdater {
    /// 以 `interval` 间隔启动定时器，每帧调用 `tick`。
    ///
    /// `tick` 内只应做状态插值 + 模型增量写入；
    /// 禁止在回调中做阻塞 IO 或音频操作。
    pub fn start(interval: Duration, tick: impl FnMut() + 'static) -> Self {
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, interval, tick);
        Self { timer }
    }
}

impl Drop for UiUpdater {
    fn drop(&mut self) {
        // 显式停止定时器，防止事件循环销毁后仍触发
        self.timer.stop();
    }
}
