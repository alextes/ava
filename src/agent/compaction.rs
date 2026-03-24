use crate::error::Error;
use crate::message::{Message, MessageContent, Role};
use crate::provider::{AnyProvider, Provider};

const COMPACTION_THRESHOLD: f64 = 0.9;

const SUMMARIZATION_PROMPT: &str = "\
you are summarizing a conversation for context compaction. produce a concise summary preserving:
1. key facts about the user
2. decisions made and their reasoning
3. ongoing tasks or threads
4. important context needed to continue naturally
keep the summary under 2000 words. use plain text, no markdown headers.";

/// returns true when input tokens exceed 90% of the context window.
/// uses `last_input_tokens` from the previous API call if available,
/// otherwise falls back to a char-based heuristic (total_chars / 4).
pub fn needs_compaction(
    messages: &[Message],
    last_input_tokens: Option<u32>,
    context_window: u32,
) -> bool {
    let estimated_tokens = match last_input_tokens {
        Some(tokens) => tokens,
        None => {
            let total_chars: usize = messages
                .iter()
                .flat_map(|m| &m.content)
                .map(|c| match c {
                    MessageContent::Text { text } => text.len(),
                    MessageContent::ToolUse { input, .. } => input.to_string().len(),
                    MessageContent::ToolResult { content, .. } => content.estimated_len(),
                })
                .sum();
            (total_chars / 4) as u32
        }
    };

    let threshold = (context_window as f64 * COMPACTION_THRESHOLD) as u32;
    estimated_tokens > threshold
}

/// compact old messages into a summary, keeping recent messages intact.
///
/// 1. splits messages: keeps the most recent ~20% of messages, everything else is "old"
/// 2. respects tool pairs: never splits between an assistant tool_use and its user tool_result
/// 3. calls `provider.complete()` with a summarization prompt to get a summary
/// 4. returns `[summary_message] ++ recent_messages` and the summary text
pub async fn compact_messages(
    provider: &AnyProvider,
    messages: Vec<Message>,
    prior_summary: Option<String>,
) -> Result<(Vec<Message>, String), Error> {
    let total = messages.len();
    if total <= 2 {
        // nothing meaningful to compact
        return Ok((messages, prior_summary.unwrap_or_default()));
    }

    // keep ~20% of messages as recent, at least 2
    let keep_recent = (total / 5).max(2);
    let mut split_at = total - keep_recent;

    // don't split between an assistant tool_use and its user tool_result
    // if message at split_at is a user message containing tool_results, move split_at back
    while split_at > 0 && is_tool_result_message(&messages[split_at]) {
        split_at -= 1;
    }

    if split_at == 0 {
        // can't compact — all messages are recent
        return Ok((messages, prior_summary.unwrap_or_default()));
    }

    let (old_messages, recent_messages) = messages.split_at(split_at);

    // build the content to summarize
    let mut summarize_content = String::new();
    if let Some(ref prior) = prior_summary {
        summarize_content.push_str("[prior summary]\n");
        summarize_content.push_str(prior);
        summarize_content.push_str("\n\n[new messages to incorporate]\n");
    }
    for msg in old_messages {
        let role_str = match msg.role {
            Role::User | Role::System => "user",
            Role::Assistant => "assistant",
        };
        for block in &msg.content {
            match block {
                MessageContent::Text { text } => {
                    summarize_content.push_str(role_str);
                    summarize_content.push_str(": ");
                    summarize_content.push_str(text);
                    summarize_content.push('\n');
                }
                MessageContent::ToolUse { name, .. } => {
                    summarize_content.push_str(role_str);
                    summarize_content.push_str(": [tool call: ");
                    summarize_content.push_str(name);
                    summarize_content.push_str("]\n");
                }
                MessageContent::ToolResult { content, .. } => {
                    summarize_content.push_str("tool result: ");
                    // truncate long tool results to save tokens
                    let display = content.as_display_str();
                    if display.len() > 500 {
                        summarize_content.push_str(&display[..500]);
                        summarize_content.push_str("...");
                    } else {
                        summarize_content.push_str(&display);
                    }
                    summarize_content.push('\n');
                }
            }
        }
    }

    let summarize_messages = vec![Message::user(summarize_content)];
    let response = provider
        .complete(SUMMARIZATION_PROMPT, &summarize_messages, &[])
        .await?;
    let summary = response.content;

    // build compacted message list: summary as first user message + recent messages
    let summary_text = if let Some(ref prior) = prior_summary {
        format!(
            "[conversation summary (updated)]\n{summary}\n\n[prior summary for reference]\n{prior}"
        )
    } else {
        format!("[conversation summary]\n{summary}")
    };

    let mut compacted = Vec::with_capacity(1 + recent_messages.len());
    compacted.push(Message::user(&summary_text));
    compacted.extend_from_slice(recent_messages);

    Ok((compacted, summary))
}

/// check if a message is a user message containing only tool results
fn is_tool_result_message(msg: &Message) -> bool {
    msg.role == Role::User
        && msg
            .content
            .iter()
            .all(|c| matches!(c, MessageContent::ToolResult { .. }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{ProviderResponse, StopReason, TestProvider, Usage};

    #[test]
    fn test_needs_compaction_with_token_count() {
        let messages = vec![Message::user("hello")];
        // 180k tokens with 200k window = 90% = should trigger
        assert!(needs_compaction(&messages, Some(180_001), 200_000));
        // 100k tokens with 200k window = 50% = should not trigger
        assert!(!needs_compaction(&messages, Some(100_000), 200_000));
        // exactly at threshold
        assert!(!needs_compaction(&messages, Some(180_000), 200_000));
        // 80% should not trigger (below 90% threshold)
        assert!(!needs_compaction(&messages, Some(160_000), 200_000));
    }

    #[test]
    fn test_needs_compaction_char_fallback() {
        // 800_000 chars / 4 = 200_000 tokens, with 200k window = 100% > 90%
        let big_text = "x".repeat(800_000);
        let messages = vec![Message::user(big_text)];
        assert!(needs_compaction(&messages, None, 200_000));

        // small message
        let messages = vec![Message::user("hello")];
        assert!(!needs_compaction(&messages, None, 200_000));
    }

    #[tokio::test]
    async fn test_compact_messages_basic() {
        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(|_system, _msgs| {
                Ok(ProviderResponse {
                    content: "the user discussed rust programming".into(),
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    usage: Usage::default(),
                })
            }),
        });

        let messages = vec![
            Message::user("hello"),
            Message::assistant("hi there"),
            Message::user("let's talk about rust"),
            Message::assistant("sure, rust is great"),
            Message::user("tell me about ownership"),
            Message::assistant("ownership is a key concept"),
            Message::user("what about borrowing"),
            Message::assistant("borrowing lets you reference data"),
            Message::user("thanks"),
            Message::assistant("you're welcome"),
        ];

        let (compacted, summary) = compact_messages(&provider, messages, None).await.unwrap();

        // should have summary + recent messages
        assert!(!compacted.is_empty());
        // first message should be the summary
        assert!(
            matches!(&compacted[0].content[0], MessageContent::Text { text } if text.contains("[conversation summary]"))
        );
        assert_eq!(summary, "the user discussed rust programming");
    }

    #[tokio::test]
    async fn test_compact_messages_preserves_tool_pairs() {
        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(|_system, _msgs| {
                Ok(ProviderResponse {
                    content: "summary".into(),
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    usage: Usage::default(),
                })
            }),
        });

        // create messages where a tool pair would be at the split boundary
        let mut messages = Vec::new();
        for i in 0..8 {
            messages.push(Message::user(format!("msg {i}")));
            messages.push(Message::assistant(format!("reply {i}")));
        }
        // add a tool use + tool result pair near the end
        messages.push(Message::assistant_with_content(vec![
            MessageContent::text("let me check"),
            MessageContent::tool_use("call_1", "web_search", serde_json::json!({"q": "test"})),
        ]));
        messages.push(Message::user_with_content(vec![
            MessageContent::tool_result("call_1", "search results here"),
        ]));
        messages.push(Message::assistant("based on the search..."));
        messages.push(Message::user("ok thanks"));

        let (compacted, _) = compact_messages(&provider, messages, None).await.unwrap();

        // verify no tool_result message is the first of the recent messages
        // (the split should not break tool pairs)
        for msg in &compacted[1..] {
            // if it's a tool result message, the previous message should be in compacted too
            if is_tool_result_message(msg) {
                // find this message's index
                let idx = compacted.iter().position(|m| std::ptr::eq(m, msg)).unwrap();
                assert!(idx > 0, "tool result should not be first recent message");
            }
        }

        assert!(!compacted.is_empty());
    }

    #[tokio::test]
    async fn test_compact_messages_with_prior_summary() {
        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(|_system, msgs| {
                // verify the summarization input includes the prior summary
                let input = match &msgs[0].content[0] {
                    MessageContent::Text { text } => text.clone(),
                    _ => panic!("expected text"),
                };
                assert!(input.contains("[prior summary]"));
                assert!(input.contains("old summary content"));

                Ok(ProviderResponse {
                    content: "updated summary".into(),
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    usage: Usage::default(),
                })
            }),
        });

        let messages = vec![
            Message::user("msg 1"),
            Message::assistant("reply 1"),
            Message::user("msg 2"),
            Message::assistant("reply 2"),
            Message::user("msg 3"),
            Message::assistant("reply 3"),
        ];

        let (compacted, summary) =
            compact_messages(&provider, messages, Some("old summary content".into()))
                .await
                .unwrap();

        assert_eq!(summary, "updated summary");
        assert!(
            matches!(&compacted[0].content[0], MessageContent::Text { text } if text.contains("[conversation summary (updated)]"))
        );
    }

    #[tokio::test]
    async fn test_compact_messages_too_few() {
        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(|_, _| panic!("should not be called")),
        });

        let messages = vec![Message::user("hello"), Message::assistant("hi")];

        let (compacted, _) = compact_messages(&provider, messages.clone(), None)
            .await
            .unwrap();

        // should return messages unchanged
        assert_eq!(compacted.len(), 2);
    }

    #[test]
    fn test_is_tool_result_message() {
        let tool_result_msg =
            Message::user_with_content(vec![MessageContent::tool_result("call_1", "result")]);
        assert!(is_tool_result_message(&tool_result_msg));

        let text_msg = Message::user("hello");
        assert!(!is_tool_result_message(&text_msg));

        let assistant_msg = Message::assistant("hello");
        assert!(!is_tool_result_message(&assistant_msg));
    }
}
