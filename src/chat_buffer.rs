//! per-chat ring buffer for recent messages.
//! shared between the telegram producer (writes) and the agent (reads).

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// a single buffered message.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct BufferedMessage {
    pub user_name: String,
    pub user_id: Option<i64>,
    pub text: String,
    pub received_at: Instant,
}

/// per-chat ring buffer configuration.
const MAX_MESSAGES_PER_CHAT: usize = 50;
const MAX_AGE: Duration = Duration::from_secs(30 * 60); // 30 minutes

/// shared buffer holding recent messages for all chats.
pub struct ChatBuffer {
    chats: Mutex<HashMap<i64, VecDeque<BufferedMessage>>>,
}

impl ChatBuffer {
    pub fn new() -> Self {
        Self {
            chats: Mutex::new(HashMap::new()),
        }
    }

    /// add a message to a chat's ring buffer. prunes stale entries.
    pub fn push(&self, chat_id: i64, msg: BufferedMessage) {
        let mut chats = self.chats.lock().unwrap();
        let buf = chats.entry(chat_id).or_default();

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
    }

    /// get a snapshot of recent messages for a chat.
    pub fn snapshot(&self, chat_id: i64) -> Vec<BufferedMessage> {
        let mut chats = self.chats.lock().unwrap();
        let Some(buf) = chats.get_mut(&chat_id) else {
            return Vec::new();
        };

        // prune stale before returning
        let cutoff = Instant::now() - MAX_AGE;
        while buf.front().is_some_and(|m| m.received_at < cutoff) {
            buf.pop_front();
        }

        buf.iter().cloned().collect()
    }

    /// list all chat_ids that have buffered messages (with count).
    pub fn active_chats(&self) -> Vec<(i64, usize)> {
        let chats = self.chats.lock().unwrap();
        chats
            .iter()
            .filter(|(_, buf)| !buf.is_empty())
            .map(|(&chat_id, buf)| (chat_id, buf.len()))
            .collect()
    }

    /// format a chat's buffer as readable text for the agent.
    pub fn format_context(&self, chat_id: i64) -> Option<String> {
        let messages = self.snapshot(chat_id);
        if messages.is_empty() {
            return None;
        }

        let mut lines = Vec::with_capacity(messages.len());
        for msg in &messages {
            lines.push(format!("{}: {}", msg.user_name, msg.text));
        }
        Some(lines.join("\n"))
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
            received_at: Instant::now(),
        }
    }

    #[test]
    fn test_push_and_snapshot() {
        let buf = ChatBuffer::new();
        buf.push(100, msg("alice", "hello"));
        buf.push(100, msg("bob", "hi"));
        buf.push(200, msg("charlie", "hey"));

        let snap = buf.snapshot(100);
        assert_eq!(snap.len(), 2);
        assert_eq!(snap[0].user_name, "alice");
        assert_eq!(snap[1].user_name, "bob");

        let snap2 = buf.snapshot(200);
        assert_eq!(snap2.len(), 1);

        // nonexistent chat
        assert!(buf.snapshot(999).is_empty());
    }

    #[test]
    fn test_max_size_enforcement() {
        let buf = ChatBuffer::new();
        for i in 0..60 {
            buf.push(100, msg("user", &format!("msg {i}")));
        }
        let snap = buf.snapshot(100);
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
            let deque = chats.entry(100).or_default();
            deque.push_back(BufferedMessage {
                user_name: "old".into(),
                user_id: Some(1),
                text: "ancient".into(),
                received_at: Instant::now() - Duration::from_secs(31 * 60),
            });
        }

        buf.push(100, msg("new", "recent"));
        let snap = buf.snapshot(100);
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].user_name, "new");
    }

    #[test]
    fn test_active_chats() {
        let buf = ChatBuffer::new();
        buf.push(100, msg("a", "x"));
        buf.push(200, msg("b", "y"));
        buf.push(200, msg("c", "z"));

        let mut active = buf.active_chats();
        active.sort_by_key(|(id, _)| *id);
        assert_eq!(active, vec![(100, 1), (200, 2)]);
    }

    #[test]
    fn test_format_context() {
        let buf = ChatBuffer::new();
        buf.push(100, msg("alice", "hello"));
        buf.push(100, msg("bob", "hi there"));

        let ctx = buf.format_context(100).unwrap();
        assert_eq!(ctx, "alice: hello\nbob: hi there");

        assert!(buf.format_context(999).is_none());
    }
}
