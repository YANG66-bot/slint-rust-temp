//! 播放列表：文件队列与曲内导航。

use std::path::{Path, PathBuf};

/// 线性播放列表（v1：不做随机 / 循环模式）。
#[derive(Debug, Default, Clone)]
pub struct Playlist {
    items: Vec<PathBuf>,
    index: Option<usize>,
}

impl Playlist {
    /// 追加文件（已存在的路径也会追加，保持用户意图）。
    pub fn add(&mut self, path: PathBuf) -> usize {
        self.items.push(path);
        self.items.len() - 1
    }

    /// 批量追加。
    pub fn add_many(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        for p in paths {
            self.add(p);
        }
    }

    /// 列表长度。
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 是否为空。
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 当前选中项（若已选择）。
    pub fn current(&self) -> Option<&Path> {
        self.index
            .and_then(|i| self.items.get(i).map(|p| p.as_path()))
    }

    /// 选中第 `index` 项。
    pub fn select(&mut self, index: usize) -> Option<&Path> {
        if index < self.items.len() {
            self.index = Some(index);
            self.items.get(index).map(|p| p.as_path())
        } else {
            None
        }
    }

    /// 下一首（到列表末尾返回 `None`，不循环）。
    // 刻意命名 `next` 表达“曲内导航”语义；播放列表不是迭代器，
    // 不应因此被建议实现 `std::iter::Iterator`
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<&Path> {
        let next = self.index.map_or(0, |i| i + 1);
        if next < self.items.len() {
            self.index = Some(next);
            self.items.get(next).map(|p| p.as_path())
        } else {
            None
        }
    }

    /// 上一首（在列表开头返回 `None`）。
    pub fn prev(&mut self) -> Option<&Path> {
        let prev = self.index?.checked_sub(1)?;
        self.index = Some(prev);
        self.items.get(prev).map(|p| p.as_path())
    }

    /// 清空列表与选中状态。
    pub fn clear(&mut self) {
        self.items.clear();
        self.index = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Vec<PathBuf> {
        vec![
            PathBuf::from("a.mp3"),
            PathBuf::from("b.wav"),
            PathBuf::from("c.flac"),
        ]
    }

    #[test]
    fn add_and_select() {
        let mut list = Playlist::default();
        list.add_many(paths());
        assert_eq!(list.len(), 3);
        assert_eq!(list.select(1), Some(Path::new("b.wav")));
        assert_eq!(list.current(), Some(Path::new("b.wav")));
        assert_eq!(list.select(9), None);
    }

    #[test]
    fn next_and_prev_navigate() {
        let mut list = Playlist::default();
        list.add_many(paths());
        assert!(list.current().is_none());
        assert_eq!(list.next(), Some(Path::new("a.mp3")));
        assert_eq!(list.next(), Some(Path::new("b.wav")));
        assert_eq!(list.next(), Some(Path::new("c.flac")));
        assert_eq!(list.next(), None, "到末尾不再前进");
        assert_eq!(list.prev(), Some(Path::new("b.wav")));
        assert_eq!(list.prev(), Some(Path::new("a.mp3")));
        assert_eq!(list.prev(), None, "到开头不再后退");
    }

    #[test]
    fn next_on_empty_playlist_returns_none() {
        let mut list = Playlist::default();
        assert!(list.is_empty());
        assert!(list.next().is_none());
        assert!(list.prev().is_none());
        assert!(list.current().is_none());
    }

    #[test]
    fn clear_resets_everything() {
        let mut list = Playlist::default();
        list.add_many(paths());
        list.select(2);
        list.clear();
        assert!(list.is_empty());
        assert!(list.current().is_none());
        assert!(list.next().is_none());
    }
}
