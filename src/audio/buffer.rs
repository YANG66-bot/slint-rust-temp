//! 无锁单生产者/单消费者 PCM 环形缓冲。
//!
//! 音频生产者（测试信号线程 / 播放输出回调 / 麦克风回调）与
//! 频谱分析线程之间的唯一数据通道，避免共享可变状态与锁竞争。

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 单生产者/单消费者无锁环形缓冲（单声道 f32 PCM）。
///
/// 约束与行为：
/// - 同一时刻只允许一个生产者线程与一个消费者线程
/// - 生产者调用 [`PcmRingBuffer::write`]，消费者调用 [`PcmRingBuffer::read_into`]
/// - 缓冲已满时丢弃写入溢出的样本（分析线程始终拿到最新数据）
/// - 索引单调递增并对 2 的幂取模，`usize` 回绕亦安全
pub struct PcmRingBuffer {
    buf: UnsafeCell<Box<[f32]>>,
    mask: usize,
    write: AtomicUsize,
    read: AtomicUsize,
}

// SAFETY: `buf` 仅在 `write()`（唯一生产者线程）与 `read_into()`（唯一消费者
// 线程）中访问，且二者操作的索引区间互不重叠；跨线程的数据可见性由
// 索引上的 Release/Acquire 原子序保证（先写数据后发布索引；先读数据后
// 推进索引）。
unsafe impl Sync for PcmRingBuffer {}

impl PcmRingBuffer {
    /// 创建容量至少为 `capacity` 的缓冲（内部向上取整到 2 的幂）。
    pub fn new(capacity: usize) -> Self {
        let cap = capacity.max(4).next_power_of_two();
        Self {
            buf: UnsafeCell::new(vec![0.0; cap].into_boxed_slice()),
            mask: cap - 1,
            write: AtomicUsize::new(0),
            read: AtomicUsize::new(0),
        }
    }

    /// 实际容量。
    pub fn capacity(&self) -> usize {
        self.mask + 1
    }

    /// 生产者写入样本；返回实际接受的样本数（满时丢弃多余部分）。
    pub fn write(&self, samples: &[f32]) -> usize {
        let write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        let free = self.capacity() - (write - read);
        let n = samples.len().min(free);
        // SAFETY: 生产者线程独占 [write, write+n) 区间，消费者不会读取。
        let buf = unsafe { &mut *self.buf.get() };
        for (i, &s) in samples.iter().take(n).enumerate() {
            buf[(write + i) & self.mask] = s;
        }
        self.write.store(write + n, Ordering::Release);
        n
    }

    /// 消费者读取至多 `out.len()` 个样本；返回实际读取数。
    pub fn read_into(&self, out: &mut [f32]) -> usize {
        let read = self.read.load(Ordering::Relaxed);
        let write = self.write.load(Ordering::Acquire);
        let n = out.len().min(write - read);
        // SAFETY: 消费者线程独占 [read, read+n) 区间，生产者在索引推进前
        // 不会覆写该区间。
        let buf = unsafe { &*self.buf.get() };
        for (i, o) in out.iter_mut().enumerate().take(n) {
            *o = buf[(read + i) & self.mask];
        }
        self.read.store(read + n, Ordering::Release);
        n
    }

    /// 消费者视角的可读样本数（近似值，仅供诊断）。
    pub fn available(&self) -> usize {
        let read = self.read.load(Ordering::Relaxed);
        let write = self.write.load(Ordering::Acquire);
        write - read
    }

    /// 丢弃全部待读数据（消费者调用）。
    pub fn clear(&self) {
        let write = self.write.load(Ordering::Acquire);
        self.read.store(write, Ordering::Release);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_roundtrip() {
        let ring = PcmRingBuffer::new(16);
        let written = ring.write(&[1.0, 2.0, 3.0]);
        assert_eq!(written, 3);
        let mut out = [0.0f32; 8];
        assert_eq!(ring.read_into(&mut out), 3);
        assert_eq!(&out[..3], &[1.0, 2.0, 3.0]);
        assert_eq!(ring.available(), 0);
    }

    #[test]
    fn capacity_rounds_up_to_power_of_two() {
        assert_eq!(PcmRingBuffer::new(1).capacity(), 4);
        assert_eq!(PcmRingBuffer::new(100).capacity(), 128);
        assert_eq!(PcmRingBuffer::new(16384).capacity(), 16384);
    }

    #[test]
    fn drops_samples_when_full() {
        let ring = PcmRingBuffer::new(8); // 容量 8
        let data = [0.5f32; 12];
        let accepted = ring.write(&data);
        assert_eq!(accepted, 8);
        let mut out = [0.0f32; 16];
        assert_eq!(ring.read_into(&mut out), 8);
        assert!(out[..8].iter().all(|&v| v == 0.5));
    }

    #[test]
    fn wraps_around_correctly() {
        let ring = PcmRingBuffer::new(4);
        // 多次跨回绕点写入/读取
        let mut expected = 1.0f32;
        for _ in 0..10 {
            let n = ring.write(&[expected, expected + 1.0]);
            assert_eq!(n, 2);
            let mut out = [0.0f32; 2];
            assert_eq!(ring.read_into(&mut out), 2);
            assert_eq!(out[0], expected);
            assert_eq!(out[1], expected + 1.0);
            expected += 2.0;
        }
    }

    #[test]
    fn clear_drops_pending_data() {
        let ring = PcmRingBuffer::new(16);
        ring.write(&[9.0, 9.0]);
        ring.clear();
        assert_eq!(ring.available(), 0);
        let mut out = [0.0f32; 4];
        assert_eq!(ring.read_into(&mut out), 0);
    }
}
