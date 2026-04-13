//! per-chat ring buffer for recent messages.
//! shared between the telegram producer (writes) and the agent (reads).
//! keyed by (chat_id, thread_id) so supergroup topics are tracked separately.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::message::ImageSource;

/// composite key: (chat_id, thread_id). thread_id is None for non-topic chats.
type BufferKey = (i64, Option<i64>);

/// a single buffered message.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct BufferedMessage {
    pub user_name: String,
    pub user_id: Option<i64>,
    pub text: String,
    pub images: Vec<ImageSource>,
    pub received_at: Instant,
}

/// per-chat ring buffer configuration.
const MAX_MESSAGES_PER_CHAT: usize = 50;
const MAX_AGE: Duration = Duration::from_secs(30 * 60); // 30 minutes
const SIZE_WARNING_BYTES: usize = 100 * 1024 * 1024; // 100 MiB

/// shared buffer holding recent messages for all chats/threads.
pub struct ChatBuffer {
    chats: Mutex<HashMap<BufferKey, VecDeque<BufferedMessage>>>,
}

impl ChatBuffer {
    pub fn new() -> Self {
        Self {
            chats: Mutex::new(HashMap::new()),
        }
    }

    /// add a message to a chat/thread's ring buffer. prunes stale entries.
    pub fn push(&self, chat_id: i64, thread_id: Option<i64>, msg: BufferedMessage) {
        let mut chats = self.chats.lock().unwrap();
        let buf = chats.entry((chat_id, thread_id)).or_default();

        // prune old messages
        let cutoff = Instant::now() - MAX_AGE;
        while buf.front().is_some_and(|m| m.received_at < cutoff) {
            buf.pop_front();
        }

        // enforce size limit
        while buf.len() >= MAX_MESSAGES_PER_CHAT {
            buf.pop_front();
        }

        buf.push_back(msg);

        // warn if total buffer size is getting large (images can be heavy)
        let total_bytes: usize = chats
            .values()
            .flat_map(|buf| buf.iter())
            .map(|m| m.text.len() + m.images.iter().map(|i| i.data.len()).sum::<usize>())
            .sum();
        if total_bytes > SIZE_WARNING_BYTES {
            tracing::warn!(
                total_mib = total_bytes / (1024 * 1024),
                "chat buffer size exceeds {}MiB",
                SIZE_WARNING_BYTES / (1024 * 1024),
            );
        }
    }

    /// get a snapshot of recent messages for a chat/thread.
    pub fn snapshot(&self, chat_id: i64, thread_id: Option<i64>) -> Vec<BufferedMessage> {
        let mut chats = self.chats.lock().unwrap();
        let Some(buf) = chats.get_mut(&(chat_id, thread_id)) else {
            return Vec::new();
        };

        // prune stale before returning
        let cutoff = Instant::now() - MAX_AGE;
        while buf.front().is_some_and(|m| m.received_at < cutoff) {
            buf.pop_front();
        }

        buf.iter().cloned().collect()
    }

    /// list all (chat_id, thread_id) pairs that have buffered messages (with count).
    pub fn active_chats(&self) -> Vec<(BufferKey, usize)> {
        let chats = self.chats.lock().unwrap();
        chats
            .iter()
            .filter(|(_, buf)| !buf.is_empty())
            .map(|(key, buf)| (*key, buf.len()))
            .collect()
    }

    /// format a chat/thread's buffer as readable text + images for the agent.
    /// returns (text_context, images) where images are from recent buffer messages.
    pub fn format_context(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
    ) -> Option<(String, Vec<ImageSource>)> {
        let messages = self.snapshot(chat_id, thread_id);
        if messages.is_empty() {
            return None;
        }

        let mut lines = Vec::with_capacity(messages.len());
        let mut images = Vec::new();
        for msg in &messages {
            let text = if msg.images.is_empty() {
                msg.text.clone()
            } else {
                let label = if msg.text.is_empty() {
                    "[photo]".to_string()
                } else {
                    format!("{} [+photo]", msg.text)
                };
                images.extend(msg.images.iter().cloned());
                label
            };
            lines.push(format!("{}: {text}", msg.user_name));
        }
        Some((lines.join("\n"), images))
    }

    /// drain a chat/thread's buffer, returning formatted text and clearing the
    /// buffer. messages are only injected into the agent's context once — new
    /// messages that arrive after the drain accumulate for the next trigger.
    /// drain and format a chat/thread's buffer as text + images for the agent.
    /// clears the buffer so messages are only injected once.
    pub fn drain_context(
        &self,
        chat_id: i64,
        thread_id: Option<i64>,
    ) -> Option<(String, Vec<ImageSource>)> {
        let mut chats = self.chats.lock().unwrap();
        let buf = chats.get_mut(&(chat_id, thread_id))?;

        // prune stale
        let cutoff = Instant::now() - MAX_AGE;
        while buf.front().is_some_and(|m| m.received_at < cutoff) {
            buf.pop_front();
        }

        if buf.is_empty() {
            return None;
        }

        let messages: Vec<_> = buf.drain(..).collect();
        let mut lines = Vec::with_capacity(messages.len());
        let mut images = Vec::new();
        for msg in &messages {
            let text = if msg.images.is_empty() {
                msg.text.clone()
            } else {
                let label = if msg.text.is_empty() {
                    "[photo]".to_string()
                } else {
                    format!("{} [+photo]", msg.text)
                };
                images.extend(msg.images.iter().cloned());
                label
            };
            lines.push(format!("{}: {text}", msg.user_name));
        }
        Some((lines.join("\n"), images))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(name: &str, text: &str) -> BufferedMessage {
        BufferedMessage {
            user_name: name.into(),
            user_id: Some(1),
            text: text.into(),
            images: vec![],
            received_at: Instant::now(),
        }
    }

    #[test]
    fn test_push_and_snapshot() {
        let buf = ChatBuffer::new();
        buf.push(100, None, msg("alice", "hello"));
        buf.push(100, None, msg("bob", "hi"));
        buf.push(200, None, msg("charlie", "hey"));

        let snap = buf.snapshot(100, None);
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].user_name, "alice");
        assert_eq!(snap[1].user_name, "bob");

        let snap2 = buf.snapshot(200, None);
        assert_eq!(snap2.len(), 1);

        // nonexistent chat
        assert!(buf.snapshot(999, None).is_empty());
    }

    #[test]
    fn test_thread_isolation() {
        let buf = ChatBuffer::new();
        buf.push(100, Some(1), msg("alice", "in thread 1"));
        buf.push(100, Some(2), msg("bob", "in thread 2"));
        buf.push(100, None, msg("charlie", "no thread"));

        assert_eq!(buf.snapshot(100, Some(1)).len(), 1);
        assert_eq!(buf.snapshot(100, Some(2)).len(), 1);
        assert_eq!(buf.snapshot(100, None).len(), 1);
        assert_eq!(buf.snapshot(100, Some(1))[0].text, "in thread 1");
        assert_eq!(buf.snapshot(100, Some(2))[0].text, "in thread 2");
    }

    #[test]
    fn test_max_size_enforcement() {
        let buf = ChatBuffer::new();
        for i in 0..60 {
            buf.push(100, None, msg("user", &format!("msg {i}")));
        }
        let snap = buf.snapshot(100, None);
        assert_eq!(snap.len(), MAX_MESSAGES_PER_CHAT);
        // oldest should be pruned, newest kept
        assert_eq!(snap.last().unwrap().text, "msg 59");
    }

    #[test]
    fn test_age_pruning() {
        let buf = ChatBuffer::new();

        // manually insert an old message
        {
            let mut chats = buf.chats.lock().unwrap();
            let deque = chats.entry((100, None)).or_default();
            deque.push_back(BufferedMessage {
                user_name: "old".into(),
                user_id: Some(1),
                text: "ancient".into(),
                images: vec![],
                received_at: Instant::now() - Duration::from_secs(31 * 60),
            });
        }

        buf.push(100, None, msg("new", "recent"));
        let snap = buf.snapshot(100, None);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].user_name, "new");
    }

    #[test]
    fn test_active_chats() {
        let buf = ChatBuffer::new();
        buf.push(100, None, msg("a", "x"));
        buf.push(200, Some(5), msg("b", "y"));
        buf.push(200, Some(5), msg("c", "z"));

        let mut active = buf.active_chats();
        active.sort_by_key(|((chat_id, _), _)| *chat_id);
        assert_eq!(active.len(), 2);
        assert_eq!(active[0].0, (100, None));
        assert_eq!(active[0].1, 1);
        assert_eq!(active[1].0, (200, Some(5)));
        assert_eq!(active[1].1, 2);
    }

    #[test]
    fn test_format_context() {
        let buf = ChatBuffer::new();
        buf.push(100, None, msg("alice", "hello"));
        buf.push(100, None, msg("bob", "hi there"));

        let (ctx, imgs) = buf.format_context(100, None).unwrap();
        assert_eq!(ctx, "alice: hello\nbob: hi there");
        assert!(imgs.is_empty());

        assert!(buf.format_context(999, None).is_none());
    }

    #[test]
    fn test_drain_context_clears_buffer() {
        let buf = ChatBuffer::new();
        buf.push(100, None, msg("alice", "hello"));
        buf.push(100, None, msg("bob", "hi"));

        let (ctx, imgs) = buf.drain_context(100, None).unwrap();
        assert_eq!(ctx, "alice: hello\nbob: hi");
        assert!(imgs.is_empty());

        // buffer is now empty
        assert!(buf.drain_context(100, None).is_none());
        assert!(buf.snapshot(100, None).is_empty());

        // new messages still accumulate
        buf.push(100, None, msg("charlie", "hey"));
        let (ctx2, imgs2) = buf.drain_context(100, None).unwrap();
        assert_eq!(ctx2, "charlie: hey");
        assert!(imgs2.is_empty());
    }
}
