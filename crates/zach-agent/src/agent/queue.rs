//! 排队消息：插队（steer）与追加（follow_up）消息的取用方式与队列
//!
//! 队列只属于有状态的 [`crate::Agent`]，低层循环通过 [`crate::LoopHost`] 的
//! `poll_steering` / `poll_follow_up` 读取，不感知队列本身。

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use zach_ai_core::Message;

/// 插队与追加消息在注入点的取用方式
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueMode {
    /// 一次取出全部排队消息
    All,
    /// 每个注入点只取最早的一条
    #[default]
    OneAtATime,
}

/// 排队消息
#[derive(Debug, Default)]
pub(super) struct PendingQueue {
    messages: VecDeque<Message>,
    pub(super) mode: QueueMode,
}

impl PendingQueue {
    pub(super) fn new(mode: QueueMode) -> Self {
        Self {
            messages: VecDeque::new(),
            mode,
        }
    }

    pub(super) fn push(&mut self, message: Message) {
        self.messages.push_back(message);
    }

    pub(super) fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// 按模式预览下一个注入点会取出的消息
    pub(super) fn peek(&self) -> Vec<Message> {
        match self.mode {
            QueueMode::All => self.messages.iter().cloned().collect(),
            QueueMode::OneAtATime => self.messages.front().cloned().into_iter().collect(),
        }
    }

    /// 按模式取出消息
    pub(super) fn drain(&mut self) -> Vec<Message> {
        let count = match self.mode {
            QueueMode::All => self.messages.len(),
            QueueMode::OneAtATime => self.messages.len().min(1),
        };
        self.messages.drain(..count).collect()
    }

    pub(super) fn clear(&mut self) {
        self.messages.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn queue(mode: QueueMode, texts: &[&str]) -> PendingQueue {
        let mut queue = PendingQueue::new(mode);
        for text in texts {
            queue.push(Message::user(*text));
        }
        queue
    }

    #[test]
    fn one_at_a_time_takes_only_the_oldest_message() {
        let mut queue = queue(QueueMode::OneAtATime, &["一", "二", "三"]);

        assert_eq!(queue.peek(), vec![Message::user("一")]);
        assert_eq!(queue.drain(), vec![Message::user("一")]);
        assert_eq!(queue.drain(), vec![Message::user("二")]);
        assert_eq!(queue.peek(), vec![Message::user("三")]);
        assert!(!queue.is_empty());
    }

    #[test]
    fn all_mode_takes_every_message_in_order() {
        let mut queue = queue(QueueMode::All, &["一", "二"]);

        assert_eq!(queue.peek(), vec![Message::user("一"), Message::user("二")]);
        assert_eq!(
            queue.drain(),
            vec![Message::user("一"), Message::user("二")]
        );
        assert!(queue.is_empty());
    }

    #[test]
    fn peek_does_not_consume_and_empty_queue_drains_nothing() {
        let mut queue = queue(QueueMode::OneAtATime, &["一"]);
        queue.peek();
        queue.peek();
        assert_eq!(queue.drain().len(), 1);

        assert!(queue.drain().is_empty());
        assert!(queue.peek().is_empty());
        assert!(queue.is_empty());
    }

    #[test]
    fn clear_drops_everything_but_keeps_mode() {
        let mut queue = queue(QueueMode::All, &["一", "二"]);
        queue.clear();
        assert!(queue.is_empty());
        assert_eq!(queue.mode, QueueMode::All);
    }

    #[test]
    fn default_mode_is_one_at_a_time() {
        assert_eq!(PendingQueue::default().mode, QueueMode::OneAtATime);
    }
}
