mod compaction;
pub(crate) mod context;
mod prompt;

use std::collections::BTreeSet;
use std::sync::{Arc, RwLock};

use chrono::Utc;

use crate::approver::AnyApprover;
use crate::cold_resume::{ColdResumeDecision, ColdResumePrompter, QUIET_THRESHOLD_TOKENS};
use crate::db::Database;
use crate::error::Error;
use crate::mcp::manager::McpManager;
use crate::message::{
    ChannelKind, ContentBlock, InboundMessage, Message, MessageContent, MessageKind,
    OutboundMessage, Role, ToolResultContent,
};
use crate::pricing;
use crate::provider::{
    AnyProvider, DEFAULT_SYSTEM_PROMPT, Provider, ProviderResponse, SETUP_SYSTEM_PROMPT,
    StopReason, Usage,
};
use crate::runtime::RuntimeState;
use crate::skill::Skill;
use crate::tool::{self, ApprovalDecision, Approver, ToolCall, ToolDefinition};

const MAX_TOOL_ROUNDS: u32 = 40;
const WARNING_ROUND: u32 = 32;
const TELEGRAM_MAX_MESSAGE_LEN: usize = 4096;
const MAX_LENGTH_RETRIES: u32 = 2;

pub struct Agent {
    provider: AnyProvider,
    approver: AnyApprover,
    db: Arc<Database>,
    client: reqwest::Client,
    mcp: Option<Arc<McpManager>>,
    skills: Arc<Vec<Skill>>,
    vault_secrets: Arc<RwLock<Vec<String>>>,
    chat_buffer: Option<Arc<crate::chat_buffer::ChatBuffer>>,
    runtime: Option<Arc<RuntimeState>>,
    cold_resume: Option<ColdResumePrompter>,
}

impl Agent {
    pub fn new(
        provider: AnyProvider,
        approver: AnyApprover,
        db: Arc<Database>,
        client: reqwest::Client,
    ) -> Self {
        Self {
            provider,
            approver,
            db,
            client,
            mcp: None,
            skills: Arc::new(Vec::new()),
            vault_secrets: Arc::new(RwLock::new(tool::load_vault_secrets())),
            chat_buffer: None,
            runtime: None,
            cold_resume: None,
        }
    }

    pub fn with_mcp(mut self, mcp: Arc<McpManager>) -> Self {
        self.mcp = Some(mcp);
        self
    }

    pub fn with_skills(mut self, skills: Arc<Vec<Skill>>) -> Self {
        self.skills = skills;
        self
    }

    pub fn with_chat_buffer(mut self, buf: Arc<crate::chat_buffer::ChatBuffer>) -> Self {
        self.chat_buffer = Some(buf);
        self
    }

    pub fn with_runtime(mut self, runtime: Arc<RuntimeState>) -> Self {
        self.runtime = Some(runtime);
        self
    }

    pub fn with_cold_resume_prompter(mut self, prompter: ColdResumePrompter) -> Self {
        self.cold_resume = Some(prompter);
        self
    }

    /// if the message starts with /skill-name and a matching user-invocable skill exists,
    /// prepend the skill body (with $ARGUMENTS substituted) wrapped in [skill: name] tags.
    fn expand_skill(&self, content: &str) -> String {
        let trimmed = content.trim();
        if !trimmed.starts_with('/') {
            return content.to_string();
        }

        let (cmd, args) = match trimmed[1..].split_once(char::is_whitespace) {
            Some((c, a)) => (c, a.trim()),
            None => (&trimmed[1..], ""),
        };

        let skill = self
            .skills
            .iter()
            .find(|s| s.name == cmd && s.user_invocable);

        let Some(skill) = skill else {
            return content.to_string();
        };

        tracing::info!(skill = %skill.name, "expanding user-invoked skill");

        let body = skill.body.replace("$ARGUMENTS", args);
        format!("[skill: {}]\n{}\n[/skill]\n\n{}", skill.name, body, args)
    }

    /// if the prompt cache has gone cold since the last completion and the
    /// conversation is large enough to be worth flagging, ask the user whether
    /// to keep or clear the context. silently noops when:
    /// - no telegram prompter is wired (cli / autonomous mode)
    /// - this is a brand-new session (no last_completion_at recorded)
    /// - the elapsed time is still within the provider's cache ttl
    /// - the persisted input_tokens are below the quiet threshold (~10k)
    async fn maybe_prompt_cold_resume(
        &self,
        session_id: i64,
        preserve_from_message_id: i64,
    ) -> Result<(), Error> {
        let Some(prompter) = self.cold_resume.as_ref() else {
            return Ok(());
        };

        let last_completion = match self.db.session_last_completion_at(session_id)? {
            Some(secs) => secs,
            None => return Ok(()),
        };

        let now_secs = chrono::Utc::now().timestamp();
        let elapsed_secs = now_secs.saturating_sub(last_completion).max(0) as u64;
        let elapsed = std::time::Duration::from_secs(elapsed_secs);

        let cache_ttl = self.provider.cache_ttl();
        if elapsed <= cache_ttl {
            return Ok(());
        }

        let (input_tokens, _ctx_window) = match self.db.session_usage(session_id)? {
            Some(pair) => pair,
            None => return Ok(()),
        };
        if input_tokens < QUIET_THRESHOLD_TOKENS {
            return Ok(());
        }

        let model_id = self.provider.model_id();
        let cost_estimate = pricing::format_replay_cost(&model_id, input_tokens);

        tracing::info!(
            elapsed_secs = elapsed.as_secs(),
            cache_ttl_secs = cache_ttl.as_secs(),
            input_tokens,
            model = %model_id,
            cost = %cost_estimate,
            "cache likely cold, prompting user"
        );

        match prompter
            .prompt(input_tokens, elapsed, &cost_estimate, &model_id)
            .await
        {
            Ok(ColdResumeDecision::Keep) => {
                tracing::info!("cold-resume: user kept context");
            }
            Ok(ColdResumeDecision::Clear) => {
                let cleared = self
                    .db
                    .clear_session_context_before(session_id, Some(preserve_from_message_id))?;
                tracing::info!(
                    cleared_messages = cleared,
                    preserve_from_message_id,
                    "cold-resume: user cleared context"
                );
            }
            Err(e) => {
                // soft-fail: log and continue with full context. better than
                // breaking the conversation when telegram is flaky.
                tracing::warn!(%e, "cold-resume prompt failed, defaulting to keep");
            }
        }

        Ok(())
    }

    fn drain_pending_steers(
        &self,
        session_id: i64,
        messages: &mut Vec<Message>,
    ) -> Result<(), Error> {
        let Some(runtime) = self.runtime.as_ref() else {
            return Ok(());
        };

        let steers = runtime.drain_steers();
        let steer_count = steers.len();
        if let Err(e) = self.inject_steers(session_id, messages, steers) {
            tracing::error!(%e, steer_count, "failed to inject pending steers");
            return Err(Error::Provider(format!(
                "failed to persist /steer injection ({steer_count} steer message{})",
                if steer_count == 1 { "" } else { "s" }
            )));
        }
        Ok(())
    }

    fn inject_steers(
        &self,
        session_id: i64,
        messages: &mut Vec<Message>,
        steers: Vec<String>,
    ) -> Result<bool, Error> {
        let steer_texts: Vec<String> = steers
            .into_iter()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if steer_texts.is_empty() {
            return Ok(false);
        }

        let note = steering_note(&steer_texts);
        self.db.append_message_with_kind(
            session_id,
            "system",
            MessageKind::Steer,
            &[MessageContent::text(&note)],
            None,
        )?;
        messages.push(Message::user(note));
        Ok(true)
    }

    fn finish_turn_or_drain_steers(&self) -> Vec<String> {
        self.runtime
            .as_ref()
            .map(|runtime| runtime.finish_turn_or_drain_steers())
            .unwrap_or_default()
    }

    #[tracing::instrument(skip(self, inbound), fields(channel = ?inbound.channel))]
    pub async fn process(
        &self,
        inbound: &InboundMessage,
    ) -> Result<Option<OutboundMessage>, Error> {
        let session_id = self.db.active_session()?;
        let channel_str = inbound.channel.as_str();

        // repair orphaned tool_use blocks left by a previous crash/interruption
        // before appending the new user turn, so the orphan remains the tail.
        let mut existing_messages = self.db.load_messages(session_id)?;
        self.repair_orphaned_tool_calls(session_id, &mut existing_messages)?;
        drop(existing_messages);

        // expand /skill-name invocations before sending to the LLM
        let content = self.expand_skill(&inbound.content);

        // build user content: text (if non-empty) + any attached images
        let mut user_content = Vec::new();
        if !content.is_empty() {
            user_content.push(MessageContent::text(&content));
        }
        for image in &inbound.images {
            user_content.push(MessageContent::Image {
                source: image.clone(),
            });
        }

        // persist the new user message before the cold-resume prompt. if the
        // user chooses "clear", older history is elided while this trigger
        // message stays in the fresh context.
        let user_message_id =
            self.db
                .append_message(session_id, "user", &user_content, Some(channel_str))?;

        // cold-resume check: if the prompt cache has expired since the last
        // completion *and* the conversation is large enough that replaying it
        // uncached would be visible on the bill, ask the user how to proceed.
        // runs before load_messages so the [clear] action's db updates take
        // effect on this turn's loaded history.
        self.maybe_prompt_cold_resume(session_id, user_message_id)
            .await?;

        // load conversation history (growing window for prompt cache efficiency)
        let mut messages = self.db.load_messages(session_id)?;

        // inject current timestamp as a system note so the model knows the time
        // without putting it in the system prompt (which would bust the cache).
        // only inject if the last time note is >5 minutes old (or absent).
        let now = Utc::now();
        let should_inject_time = {
            let last_time = messages.iter().rev().find_map(|m| {
                m.content.iter().find_map(|c| {
                    if let MessageContent::Text { text } = c {
                        text.strip_prefix("[system: current date and time is ")
                            .and_then(|s| s.strip_suffix(']'))
                            .and_then(|s| {
                                chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M UTC").ok()
                            })
                            .map(|naive| naive.and_utc())
                    } else {
                        None
                    }
                })
            });
            match last_time {
                Some(t) => now.signed_duration_since(t).num_seconds() >= 300,
                None => true,
            }
        };
        if should_inject_time {
            let time_note = format!(
                "[system: current date and time is {}]",
                now.format("%Y-%m-%d %H:%M UTC")
            );
            let time_content = vec![MessageContent::text(&time_note)];
            self.db
                .append_message(session_id, "system", &time_content, None)?;
            messages.push(Message::user(&time_note));
        }

        let system_prompt = self.system_prompt(session_id)?;
        let tools = self.all_tool_definitions().await;
        let mut tool_rounds = 0;
        let mut switched_provider: Option<AnyProvider> = None;
        let mut last_input_tokens: Option<u32> = None;
        let mut compaction_count: u32 = 0;
        // track which one-shot context warning thresholds (20, 40, 60) have fired.
        // derive initial state from DB so we don't re-fire on session resume.
        let mut fired_thresholds: BTreeSet<u32> = {
            let initial_pct = match self.db.session_usage(session_id)? {
                Some((input_tokens, context_window)) if context_window > 0 => {
                    input_tokens as f64 / context_window as f64 * 100.0
                }
                _ => 0.0,
            };
            let mut set = BTreeSet::new();
            for &t in &[20, 40, 60] {
                if initial_pct >= t as f64 {
                    set.insert(t);
                }
            }
            set
        };
        let mut pending_voice: Option<Vec<u8>> = None;
        let mut pending_attachments: Vec<crate::tool::FileAttachment> = Vec::new();
        let mut length_retries: u32 = 0;

        loop {
            self.drain_pending_steers(session_id, &mut messages)?;

            // compact context if approaching the model's limit
            let context_window = switched_provider
                .as_ref()
                .unwrap_or(&self.provider)
                .context_window();

            if compaction::needs_compaction(&messages, last_input_tokens, context_window) {
                let prior_summary = self.db.get_session_summary(session_id)?;
                let provider = switched_provider.as_ref().unwrap_or(&self.provider);
                match compaction::compact_messages(provider, messages.clone(), prior_summary).await
                {
                    Ok((compacted, summary, split_at)) => {
                        self.persist_compaction_cursor(session_id, split_at)?;
                        self.db.set_session_summary(session_id, &summary)?;
                        messages = compacted;
                        compaction_count += 1;
                        tracing::info!(compaction_count, "compacted context");
                    }
                    Err(e) => {
                        tracing::warn!(%e, "compaction failed, continuing with full context");
                    }
                }
            }

            let active_provider = switched_provider.as_ref().unwrap_or(&self.provider);
            let response = match active_provider
                .complete(&system_prompt, &messages, &tools)
                .await
            {
                Ok(r) => r,
                Err(Error::ContextOverflow) => {
                    return Ok(Some(OutboundMessage {
                        content: "conversation context is full. key facts have been preserved \
                            — please start a new session."
                            .into(),
                        voice: None,
                        attachments: vec![],
                    }));
                }
                Err(Error::RateLimited(ref msg)) => {
                    let provider_name = active_provider.provider_name();
                    tracing::warn!(provider = provider_name, "rate limited");
                    return Ok(Some(OutboundMessage {
                        content: format!(
                            "rate limited by {provider_name}. please try again in a moment.\n\n\
                             details: {msg}"
                        ),
                        voice: None,
                        attachments: vec![],
                    }));
                }
                Err(Error::BudgetExhausted(ref msg)) => {
                    let current_name = active_provider.provider_name();
                    let fallback_name = match current_name {
                        "anthropic" => "openai",
                        "deepseek" => "anthropic",
                        "openai" => "anthropic",
                        "openrouter" => "anthropic",
                        _ => return Err(Error::BudgetExhausted(msg.clone())),
                    };
                    tracing::warn!(
                        provider = current_name,
                        fallback = fallback_name,
                        "budget exhausted, attempting fallback"
                    );
                    match AnyProvider::from_name(self.client.clone(), fallback_name, None) {
                        Ok(new_provider) => {
                            let model_id = new_provider.model_id();
                            let reasoning_effort = new_provider.reasoning_effort();
                            if let Err(e) = self.db.set_session_model_reasoning(
                                session_id,
                                &model_id,
                                reasoning_effort,
                            ) {
                                tracing::warn!(%e, "failed to persist fallback model");
                            }
                            let note = format!(
                                "the {current_name} provider's budget is exhausted. \
                                 automatically switched to {fallback_name}. \
                                 send `/switch {current_name}` to switch back."
                            );
                            messages.push(Message::user(&note));
                            self.db.append_message(
                                session_id,
                                "user",
                                &[MessageContent::text(&note)],
                                None,
                            )?;
                            switched_provider = Some(new_provider);
                            continue;
                        }
                        Err(fallback_err) => {
                            tracing::error!(
                                %fallback_err,
                                "fallback provider also failed"
                            );
                            return Err(Error::BudgetExhausted(msg.clone()));
                        }
                    }
                }
                Err(e) => return Err(e),
            };

            let response_model_id = active_provider.model_id();
            let usage = &response.usage;
            last_input_tokens = Some(usage.input_tokens);

            let ctx = context::ContextUsage::compute(usage, context_window, compaction_count);
            let context_pct = format!("{:.0}%", ctx.usage_percent);
            let context_tokens = format!("{}/{}", ctx.input_tokens, ctx.context_window);

            // unified context-aware log line. WARN when above 80% to surface
            // approaching limits before compaction kicks in at 90%.
            if ctx.usage_percent > 80.0 {
                tracing::warn!(
                    context = %context_pct,
                    tokens = %context_tokens,
                    output = ctx.output_tokens,
                    cache_created = usage.cache_creation_tokens,
                    cache_read = usage.cache_read_tokens,
                    reasoning = usage.reasoning_tokens,
                    compactions = ctx.compaction_count,
                    "context usage"
                );
            } else {
                tracing::info!(
                    context = %context_pct,
                    tokens = %context_tokens,
                    output = ctx.output_tokens,
                    cache_created = usage.cache_creation_tokens,
                    cache_read = usage.cache_read_tokens,
                    reasoning = usage.reasoning_tokens,
                    compactions = ctx.compaction_count,
                    "context usage"
                );
            }

            if let Err(e) =
                self.db
                    .set_session_usage(session_id, ctx.input_tokens, ctx.context_window)
            {
                tracing::warn!(%e, "failed to persist context usage");
            }

            // record completion timestamp so the next user turn can detect
            // whether the prompt cache has expired in the gap.
            if let Err(e) = self
                .db
                .set_session_last_completion_at(session_id, chrono::Utc::now().timestamp())
            {
                tracing::warn!(%e, "failed to persist last_completion_at");
            }

            if response.tool_calls.is_empty() {
                // check telegram message length limit before persisting
                if inbound.channel == ChannelKind::Telegram
                    && !response.content.is_empty()
                    && length_retries < MAX_LENGTH_RETRIES
                {
                    let html = crate::telegram_fmt::markdown_to_telegram_html(&response.content);
                    if html.len() > TELEGRAM_MAX_MESSAGE_LEN {
                        length_retries += 1;
                        tracing::info!(
                            html_len = html.len(),
                            retry = length_retries,
                            "response too long for telegram, asking agent to retry"
                        );
                        // add the too-long response to in-memory context (not persisted)
                        // so the agent can see what it wrote and rework it
                        let mut retry_content = response.hidden_content.clone();
                        retry_content.push(MessageContent::text(&response.content));
                        messages.push(Message::assistant_with_content(retry_content));
                        let feedback = format!(
                            "[system: your response is {} characters after formatting, \
                             but telegram's limit is {}. either rewrite it much shorter, \
                             or write the full content to a file (e.g. /tmp/response.md) \
                             and use the send_file tool to share it with the user \
                             alongside a brief summary message.]",
                            html.len(),
                            TELEGRAM_MAX_MESSAGE_LEN,
                        );
                        messages.push(Message::user(&feedback));
                        continue;
                    }
                }

                let last_chance_steers = self.finish_turn_or_drain_steers();
                if !last_chance_steers.is_empty() {
                    let mut provisional_content = response.hidden_content.clone();
                    if !response.content.is_empty() {
                        provisional_content.push(MessageContent::text(&response.content));
                    }
                    if !provisional_content.is_empty() {
                        messages.push(Message::assistant_with_content(provisional_content));
                    }
                    self.inject_steers(session_id, &mut messages, last_chance_steers)?;
                    continue;
                }

                // persist the final assistant response (skip empty content to avoid API rejection)
                if !response.content.is_empty() {
                    let mut assistant_content = response.hidden_content.clone();
                    assistant_content.push(MessageContent::text(&response.content));
                    let message_id = self.db.append_message(
                        session_id,
                        "assistant",
                        &assistant_content,
                        None,
                    )?;
                    self.db
                        .set_message_usage(message_id, usage, &response_model_id)?;
                }

                return Ok(Some(OutboundMessage {
                    content: response.content,
                    voice: pending_voice.take(),
                    attachments: std::mem::take(&mut pending_attachments),
                }));
            }

            tool_rounds += 1;

            tracing::debug!(
                tool_round = tool_rounds,
                count = response.tool_calls.len(),
                "executing tool calls"
            );

            // graceful final turn: budget exhausted
            if tool_rounds > MAX_TOOL_ROUNDS {
                tracing::warn!(
                    tool_rounds,
                    "tool budget exhausted, requesting final summary"
                );

                // persist assistant message with its tool_use blocks (unanswered)
                let mut assistant_blocks = response.hidden_content.clone();
                if !response.content.is_empty() {
                    assistant_blocks.push(MessageContent::text(&response.content));
                }
                for call in &response.tool_calls {
                    assistant_blocks.push(tool_use_content(call));
                }
                let assistant_message_id =
                    self.db
                        .append_message(session_id, "assistant", &assistant_blocks, None)?;
                self.db
                    .set_message_usage(assistant_message_id, usage, &response_model_id)?;
                messages.push(Message::assistant_with_content(assistant_blocks));

                // synthetic tool results telling the model to wrap up
                let synthetic_results: Vec<MessageContent> = response
                    .tool_calls
                    .iter()
                    .map(|call| {
                        MessageContent::tool_result(
                            &call.id,
                            "tool budget exhausted (40 rounds). you must respond now. \
                             summarize progress and explain remaining work.",
                        )
                    })
                    .collect();
                self.db
                    .append_message(session_id, "user", &synthetic_results, None)?;
                messages.push(Message::user_with_content(synthetic_results));

                loop {
                    // final text-only turn (no tools)
                    self.drain_pending_steers(session_id, &mut messages)?;
                    let active_provider = switched_provider.as_ref().unwrap_or(&self.provider);
                    let final_model_id = active_provider.model_id();
                    let final_response = match active_provider
                        .complete(&system_prompt, &messages, &[])
                        .await
                    {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::warn!(%e, "final summary call failed, using fallback");
                            ProviderResponse {
                                content:
                                    "i used all 40 tool rounds. send a follow-up message and i'll continue."
                                        .to_string(),
                                stop_reason: StopReason::EndTurn,
                                tool_calls: vec![],
                                hidden_content: vec![],
                                usage: Usage::default(),
                            }
                        }
                    };

                    let last_chance_steers = self.finish_turn_or_drain_steers();
                    if !last_chance_steers.is_empty() {
                        let mut provisional_content = final_response.hidden_content.clone();
                        if !final_response.content.is_empty() {
                            provisional_content.push(MessageContent::text(&final_response.content));
                        }
                        if !provisional_content.is_empty() {
                            messages.push(Message::assistant_with_content(provisional_content));
                        }
                        self.inject_steers(session_id, &mut messages, last_chance_steers)?;
                        continue;
                    }

                    let final_content = final_response.content.clone();

                    // check length — if too long and on telegram, send error instead
                    let send_content = if inbound.channel == ChannelKind::Telegram {
                        let html = crate::telegram_fmt::markdown_to_telegram_html(&final_content);
                        if html.len() > TELEGRAM_MAX_MESSAGE_LEN {
                            format!(
                                "sorry, my response was too long for telegram ({} chars, \
                                 limit is {}). send a follow-up and i'll try to be more \
                                 concise, or ask me to write it to a file.",
                                html.len(),
                                TELEGRAM_MAX_MESSAGE_LEN,
                            )
                        } else {
                            final_content.clone()
                        }
                    } else {
                        final_content.clone()
                    };

                    let final_blocks = vec![MessageContent::text(&final_content)];
                    let message_id =
                        self.db
                            .append_message(session_id, "assistant", &final_blocks, None)?;
                    self.db.set_message_usage(
                        message_id,
                        &final_response.usage,
                        &final_model_id,
                    )?;

                    return Ok(Some(OutboundMessage {
                        content: send_content,
                        voice: pending_voice.take(),
                        attachments: std::mem::take(&mut pending_attachments),
                    }));
                }
            }

            let mut assistant_blocks = response.hidden_content.clone();
            if !response.content.is_empty() {
                assistant_blocks.push(MessageContent::text(response.content));
            }

            for call in &response.tool_calls {
                tracing::debug!(tool = %call.name, "invoking tool");
                assistant_blocks.push(tool_use_content(call));
            }

            // persist the assistant message (including tool_use blocks)
            let assistant_message_id =
                self.db
                    .append_message(session_id, "assistant", &assistant_blocks, None)?;
            self.db
                .set_message_usage(assistant_message_id, usage, &response_model_id)?;
            messages.push(Message::assistant_with_content(assistant_blocks));

            // execute tool calls concurrently
            let results = futures::future::join_all(
                response
                    .tool_calls
                    .iter()
                    .map(|call| self.handle_tool_call_with_approval(call)),
            )
            .await;

            let mut tool_results = Vec::new();
            let mut saw_complete = false;
            let mut saw_compact = false;
            for result in results {
                let result = result?;
                if result.complete {
                    saw_complete = true;
                }
                if result.compact {
                    saw_compact = true;
                }
                if let Some(new_provider) = result.switch_provider {
                    let model_id = new_provider.model_id();
                    let reasoning_effort = new_provider.reasoning_effort();
                    tracing::info!(%model_id, reasoning = %reasoning_effort, "switching provider mid-conversation");
                    if let Err(e) =
                        self.db
                            .set_session_model_reasoning(session_id, &model_id, reasoning_effort)
                    {
                        tracing::warn!(%e, "failed to persist model selection");
                    }
                    switched_provider = Some(new_provider);
                }
                if let Some(voice_bytes) = result.voice {
                    pending_voice = Some(voice_bytes);
                }
                if let Some(attachment) = result.attachment {
                    pending_attachments.push(attachment);
                }
                tool_results.push(result.content);
            }

            if saw_complete {
                self.db
                    .append_message(session_id, "user", &tool_results, None)?;
                return Ok(None);
            }

            // persist tool results (clean, no injections)
            self.db
                .append_message(session_id, "user", &tool_results, None)?;
            messages.push(Message::user_with_content(tool_results));

            // collect system injections (budget warnings, context usage) into a
            // separate message so they don't interfere with structured tool output.
            let mut system_notes = Vec::new();

            if tool_rounds == WARNING_ROUND {
                system_notes.push(MessageContent::text(format!(
                    "[system: you have used {WARNING_ROUND} of {MAX_TOOL_ROUNDS} tool rounds. \
                     {} remain before you must produce a final response. plan accordingly.]",
                    MAX_TOOL_ROUNDS - WARNING_ROUND
                )));
            }

            // tiered context warnings: one-shot at 20/40/60%, every round at 80%+.
            // round 1 gets a bare info line unless a threshold applies.
            let pct = ctx.usage_percent;
            let tokens_info = format!("{}/{}", ctx.input_tokens, ctx.context_window);

            let context_msg: Option<String> = if pct >= 80.0 {
                fired_thresholds.extend(&[20, 40, 60]);
                Some(format!(
                    "[context: {pct:.0}% used ({tokens_info} tokens). \
                     approaching auto-compaction at 90%. consider triggering \
                     compaction at the earliest opportunity.]"
                ))
            } else if pct >= 60.0 && !fired_thresholds.contains(&60) {
                fired_thresholds.extend(&[20, 40, 60]);
                Some(format!(
                    "[context: {pct:.0}% used ({tokens_info} tokens). \
                     significant context in use. unless the current task absolutely \
                     requires all prior context, consider compacting now before continuing.]"
                ))
            } else if pct >= 40.0 && !fired_thresholds.contains(&40) {
                fired_thresholds.extend(&[20, 40]);
                Some(format!(
                    "[context: {pct:.0}% used ({tokens_info} tokens). \
                     if there are multiple disjoint tasks in context, consider \
                     compacting now to reduce costs.]"
                ))
            } else if pct >= 20.0 && !fired_thresholds.contains(&20) {
                fired_thresholds.insert(20);
                Some(format!(
                    "[context: {pct:.0}% used ({tokens_info} tokens). \
                     token costs increase with context size. if all context is relevant \
                     to the current task, continue freely. if context spans multiple \
                     completed tasks, consider running compaction when the current task finishes.]"
                ))
            } else if tool_rounds == 1 {
                Some(format!("[context: {pct:.0}% used ({tokens_info} tokens)]"))
            } else {
                None
            };

            if let Some(msg) = context_msg {
                system_notes.push(MessageContent::text(msg));
            }

            if !system_notes.is_empty() {
                self.db
                    .append_message(session_id, "system", &system_notes, None)?;
                // sent to the API as a user message
                messages.push(Message::user_with_content(system_notes));
            }

            // agent-requested compaction (via compact_context tool)
            if saw_compact && ctx.usage_percent >= 20.0 {
                let prior_summary = self.db.get_session_summary(session_id)?;
                let provider = switched_provider.as_ref().unwrap_or(&self.provider);
                match compaction::compact_messages(provider, messages.clone(), prior_summary).await
                {
                    Ok((compacted, summary, split_at)) => {
                        self.persist_compaction_cursor(session_id, split_at)?;
                        self.db.set_session_summary(session_id, &summary)?;
                        messages = compacted;
                        compaction_count += 1;
                        tracing::info!(compaction_count, "agent-requested compaction");
                    }
                    Err(e) => {
                        tracing::warn!(%e, "agent-requested compaction failed");
                    }
                }
            }
        }
    }

    async fn handle_tool_call_with_approval(
        &self,
        call: &ToolCall,
    ) -> Result<tool::ToolCallResult, Error> {
        // hard deny vault access before anything else — no approval can override
        if let Some(deny) = tool::check_vault_deny(call) {
            return Ok(deny);
        }

        if tool::requires_approval(call) {
            // log a concise summary of what's being requested
            let approval_summary = tool::approval_summary(call);
            tracing::info!(
                tool = %call.name,
                detail = %approval_summary,
                "requesting approval"
            );
            let decision = match self.approver.request_approval(call).await {
                Ok(d) => d,
                Err(Error::ApprovalTimeout) => {
                    tracing::warn!(tool = %call.name, "approval timed out, treating as deny");
                    return Ok(tool::ToolCallResult {
                        content: MessageContent::tool_result(
                            &call.id,
                            "approval timed out — the user did not respond in time",
                        ),
                        switch_provider: None,
                        complete: false,
                        compact: false,
                        voice: None,
                        attachment: None,
                    });
                }
                Err(e) => return Err(e),
            };
            match decision {
                ApprovalDecision::AllowOnce => {
                    tracing::info!(tool = %call.name, "approved (once)");
                }
                ApprovalDecision::AutoApproved => {
                    tracing::debug!(tool = %call.name, "auto-approved");
                }
                ApprovalDecision::AllowAlways { ref pattern } => {
                    for p in pattern.split('\n') {
                        let p = p.trim();
                        if !p.is_empty() {
                            tracing::info!(p, "saving approval rule");
                            self.db.save_approval_rule(p)?;
                        }
                    }
                }
                ApprovalDecision::AllowTimed {
                    ref pattern,
                    duration_secs,
                } => {
                    let expires_at =
                        chrono::Utc::now() + chrono::Duration::seconds(duration_secs as i64);
                    let expires_str = expires_at.format("%Y-%m-%d %H:%M:%S").to_string();
                    for p in pattern.split('\n') {
                        let p = p.trim();
                        if !p.is_empty() {
                            tracing::info!(p, %expires_str, "saving timed approval rule");
                            self.db
                                .save_approval_rule_with_expiry(p, Some(&expires_str))?;
                        }
                    }
                }
                ApprovalDecision::Deny => {
                    return Ok(tool::ToolCallResult {
                        content: MessageContent::tool_result(&call.id, "command denied by user"),
                        switch_provider: None,
                        complete: false,
                        compact: false,
                        voice: None,
                        attachment: None,
                    });
                }
            }
        }

        // refresh vault secrets when a skill is activated (user may have added secrets)
        if call.name == tool::ACTIVATE_SKILL_TOOL_NAME
            && let Ok(mut secrets) = self.vault_secrets.write()
        {
            *secrets = tool::load_vault_secrets();
        }

        let mut result = tool::handle_tool_call(
            &self.client,
            &self.db,
            self.mcp.as_deref(),
            &self.skills,
            call,
            self.chat_buffer.as_deref(),
            self.runtime.as_deref(),
        )
        .await?;

        // scrub vault secrets from all tool output
        if let Ok(secrets) = self.vault_secrets.read()
            && !secrets.is_empty()
        {
            result.content = scrub_tool_result(result.content, &secrets);
        }

        Ok(result)
    }

    /// build the full list of tool definitions (built-in + MCP).
    async fn all_tool_definitions(&self) -> Vec<ToolDefinition> {
        let setup_mode = self.is_setup_mode().unwrap_or(false);
        let mut tools = tool::tool_definitions(setup_mode);

        if setup_mode {
            return tools;
        }

        if let Some(ref mcp) = self.mcp {
            let builtin_names: std::collections::HashSet<String> =
                tools.iter().map(|t| t.name().to_string()).collect();

            for entry in mcp.list_all_tools().await {
                let namespaced = format!("mcp__{}_{}", entry.server_name, entry.tool.name);
                if builtin_names.contains(&namespaced) {
                    tracing::warn!(
                        tool = %namespaced,
                        "MCP tool name conflicts with built-in, skipping"
                    );
                    continue;
                }
                tools.push(ToolDefinition::Dynamic {
                    name: namespaced,
                    description: entry.tool.description.unwrap_or_default(),
                    input_schema: entry.tool.input_schema,
                });
            }
        }

        tools
    }

    /// check if the last message is an assistant message with tool_use blocks
    /// that lack matching tool_results. this happens when a crash/OOM/kill
    /// interrupts the tool loop before results are persisted.
    ///
    /// for mid-history orphans, use `ava doctor` instead.
    fn repair_orphaned_tool_calls(
        &self,
        session_id: i64,
        messages: &mut Vec<Message>,
    ) -> Result<(), Error> {
        let last = match messages.last() {
            Some(msg) if msg.role == Role::Assistant => msg,
            _ => return Ok(()),
        };

        let tool_use_ids: Vec<String> = last
            .content
            .iter()
            .filter_map(|c| match c {
                MessageContent::ToolUse { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();

        if tool_use_ids.is_empty() {
            return Ok(());
        }

        tracing::warn!(
            count = tool_use_ids.len(),
            "repairing orphaned tool_use blocks from interrupted session"
        );

        let synthetic: Vec<MessageContent> = tool_use_ids
            .iter()
            .map(|id| {
                MessageContent::tool_result(
                    id,
                    "the session was interrupted and it is unknown whether this tool call completed.",
                )
            })
            .collect();

        self.db
            .append_message(session_id, "user", &synthetic, None)?;
        messages.push(Message::user_with_content(synthetic));

        Ok(())
    }

    /// advance the compaction cursor after a successful compaction.
    /// `split_at` is the number of messages consumed from the in-memory vec.
    /// if a cursor already existed, the first message was the synthetic summary
    /// (not in DB), so we offset by 1.
    fn persist_compaction_cursor(&self, session_id: i64, split_at: usize) -> Result<(), Error> {
        if split_at == 0 {
            return Ok(());
        }
        let current_cursor = self.db.get_compaction_cursor(session_id)?;
        // if cursor was set, the first in-memory message was the synthetic summary,
        // which has no DB row. so the DB messages consumed = split_at - 1.
        let db_consumed = if current_cursor.is_some() {
            split_at.saturating_sub(1)
        } else {
            split_at
        };
        if db_consumed == 0 {
            return Ok(());
        }
        let after_id = current_cursor.unwrap_or(0);
        if let Some(new_cursor) = self.db.nth_message_id(session_id, after_id, db_consumed)? {
            self.db.set_compaction_cursor(session_id, new_cursor)?;
        }
        Ok(())
    }

    fn is_setup_mode(&self) -> Result<bool, Error> {
        Ok(!self.db.is_setup_complete()?)
    }

    fn system_prompt(&self, session_id: i64) -> Result<String, Error> {
        // in setup mode, use a dedicated prompt that guides the user through initialization
        if self.is_setup_mode()? {
            let mut prompt = SETUP_SYSTEM_PROMPT.to_string();
            prompt.push_str(&format!(
                "\n\ncurrent date and time: {}",
                Utc::now().format("%Y-%m-%d %H:%M UTC")
            ));
            return Ok(prompt);
        }

        let traits = self.db.identity_traits()?;
        let facts = self.db.recent_facts()?;
        let episodes = self.db.recent_episodes()?;
        let pending_tasks = self.db.pending_task_titles()?;

        // build base prompt, incorporating agent name from identity traits if available
        let name = traits
            .iter()
            .find(|t| t.key.as_deref() == Some("name"))
            .map(|t| t.content.as_str());

        let mut prompt = match name {
            Some(n) => format!("you are {n}, an ai assistant."),
            None => DEFAULT_SYSTEM_PROMPT.to_string(),
        };

        // use session creation date (stable) instead of current time (changes every minute)
        // to avoid busting the prompt cache. current time is injected as a system note
        // in the message flow instead.
        let session_date = self.db.session_created_at(session_id)?;
        // extract just the date portion (YYYY-MM-DD) from "YYYY-MM-DD HH:MM:SS"
        let date_only = session_date.split(' ').next().unwrap_or(&session_date);
        prompt.push_str(&format!("\n\nsession started: {date_only}"));

        if !traits.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&prompt::format_identity_traits(&traits));
        }

        if !facts.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&prompt::format_known_facts(&facts));
        }

        if !episodes.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&prompt::format_recent_episodes(&episodes));
        }

        if !pending_tasks.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&prompt::format_pending_tasks(&pending_tasks));
        }

        let model_skills: Vec<_> = self
            .skills
            .iter()
            .filter(|s| !s.disable_model_invocation)
            .cloned()
            .collect();
        if !model_skills.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&prompt::format_available_skills(&model_skills));
        }

        // show active channels (only those active in the last 7 days)
        let channels = self.db.list_channels().unwrap_or_default();
        let cutoff = Utc::now() - chrono::Duration::days(7);
        let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S").to_string();
        let recent_channels: Vec<_> = channels
            .into_iter()
            .filter(|ch| ch.last_seen_at >= cutoff_str)
            .collect();
        if !recent_channels.is_empty() {
            prompt.push_str("\n\n");
            prompt.push_str(&prompt::format_active_channels(&recent_channels));
        }

        // environment context so the model knows where it's running
        {
            let hostname = hostname::get()
                .map(|h| h.to_string_lossy().into_owned())
                .unwrap_or_else(|_| "unknown".to_string());
            let workspace = crate::config::workspace_root().display();
            prompt.push_str(&format!(
                "\n\n## environment\nhostname: {hostname}\nworkspace: {workspace}\n\
                 use \".\" or relative paths for the exec tool cwd, not absolute paths from \
                 other machines."
            ));
        }

        prompt.push_str(&format!(
            "\n\n## tool budget\nyou have a budget of {MAX_TOOL_ROUNDS} tool rounds per user \
             message. after exhausting the budget you get one final text-only turn. if you need \
             more rounds, tell the user to send a follow-up message."
        ));

        prompt.push_str(&format!(
            "\n\n## message length\nresponses are delivered via telegram which has a \
             {TELEGRAM_MAX_MESSAGE_LEN}-character message limit. for long content, write \
             it to a file (e.g. /tmp/response.md) and use the send_file tool to share it \
             with the user alongside a brief summary."
        ));

        Ok(prompt)
    }
}

/// scrub vault secret values from a tool result's text content.
/// images pass through unchanged.
fn scrub_tool_result(content: MessageContent, secrets: &[String]) -> MessageContent {
    match content {
        MessageContent::ToolResult {
            tool_use_id,
            content: trc,
        } => {
            let scrubbed = match trc {
                ToolResultContent::Text(text) => {
                    ToolResultContent::Text(tool::scrub_vault_secrets(&text, secrets))
                }
                ToolResultContent::Blocks(blocks) => ToolResultContent::Blocks(
                    blocks
                        .into_iter()
                        .map(|b| match b {
                            ContentBlock::Text { text } => ContentBlock::Text {
                                text: tool::scrub_vault_secrets(&text, secrets),
                            },
                            other => other,
                        })
                        .collect(),
                ),
            };
            MessageContent::ToolResult {
                tool_use_id,
                content: scrubbed,
            }
        }
        other => other,
    }
}

fn tool_use_content(call: &ToolCall) -> MessageContent {
    MessageContent::tool_use(call.id.clone(), call.name.clone(), call.input.clone())
}

fn steering_note(messages: &[String]) -> String {
    format!(
        "[system: the user sent /steer before this response:\n{}]",
        messages.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::prompt::*;
    use super::*;
    use crate::approver::CliApprover;
    use crate::db::{Memory, MemoryKind};
    use crate::message::ChannelKind;
    use crate::provider::{ProviderResponse, StopReason, TestProvider, Usage};

    fn make_test_provider(response: &str) -> AnyProvider {
        let response = response.to_string();
        AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                Ok(ProviderResponse {
                    content: response.clone(),
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    hidden_content: Vec::new(),
                    usage: Usage::default(),
                })
            }),
        })
    }

    fn make_failing_provider() -> AnyProvider {
        AnyProvider::Test(TestProvider {
            handler: Box::new(|_, _| Err(Error::Provider("provider failed".into()))),
        })
    }

    fn test_client() -> reqwest::Client {
        reqwest::Client::builder()
            .no_proxy()
            .build()
            .expect("test HTTP client")
    }

    fn steer_origin() -> crate::runtime::SteerOrigin {
        crate::runtime::SteerOrigin {
            chat_id: 1,
            thread_id: Some(2),
        }
    }

    #[tokio::test]
    async fn test_agent_processes_message() {
        let provider = make_test_provider("hi");
        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "hello".into(),
        };

        let outbound = agent.process(&inbound).await.unwrap().unwrap();
        assert_eq!(outbound.content, "hi");
    }

    #[tokio::test]
    async fn test_provider_error_propagates() {
        let provider = make_failing_provider();
        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "hello".into(),
        };

        let result = agent.process(&inbound).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, Error::Provider(msg) if msg == "provider failed"));
    }

    #[tokio::test]
    async fn test_agent_injects_facts_into_system_prompt() {
        use std::sync::{Arc as StdArc, Mutex};

        crate::config::init_workspace_root();
        let seen_prompt = StdArc::new(Mutex::new(None));
        let seen_prompt_clone = seen_prompt.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |system, _msgs| {
                *seen_prompt_clone.lock().unwrap() = Some(system.to_string());
                Ok(ProviderResponse {
                    content: "hi".into(),
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    hidden_content: Vec::new(),
                    usage: Usage::default(),
                })
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        db.mark_setup_complete().unwrap();
        db.remember(MemoryKind::Fact, "alex", Some("user"), Some("name"))
            .unwrap();
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "hello".into(),
        };

        agent.process(&inbound).await.unwrap().unwrap();

        let prompt = seen_prompt.lock().unwrap().clone().unwrap();
        assert!(prompt.contains("## known facts"));
        assert!(prompt.contains("### user"));
        assert!(prompt.contains("- name: alex"));
    }

    #[tokio::test]
    async fn test_agent_loads_history_from_session() {
        use std::sync::{Arc as StdArc, Mutex};

        let db = Arc::new(Database::open_in_memory().unwrap());
        let sid = db.active_session().unwrap();

        // seed conversation history in the db
        db.append_message(
            sid,
            "user",
            &[MessageContent::text("my name is alex")],
            Some("cli"),
        )
        .unwrap();
        db.append_message(
            sid,
            "assistant",
            &[MessageContent::text("nice to meet you alex")],
            None,
        )
        .unwrap();

        // track what messages the provider sees
        let seen_msgs = StdArc::new(Mutex::new(None));
        let seen_clone = seen_msgs.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, msgs| {
                *seen_clone.lock().unwrap() = Some(msgs.len());
                Ok(ProviderResponse {
                    content: "your name is alex".into(),
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    hidden_content: Vec::new(),
                    usage: Usage::default(),
                })
            }),
        });

        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            test_client(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "what is my name?".into(),
        };
        agent.process(&inbound).await.unwrap().unwrap();

        // provider should have seen 4 messages: 2 history + 1 new + 1 time note
        let msg_count = seen_msgs.lock().unwrap().unwrap();
        assert_eq!(msg_count, 4);
    }

    fn make_memory(
        kind: MemoryKind,
        content: &str,
        category: Option<&str>,
        key: Option<&str>,
    ) -> Memory {
        Memory {
            id: 0,
            kind,
            content: content.into(),
            category: category.map(|s| s.into()),
            key: key.map(|s| s.into()),
            created_at: "2024-01-15 12:00:00".into(),
        }
    }

    #[test]
    fn test_format_known_facts_groups_by_category() {
        let facts = vec![
            make_memory(MemoryKind::Fact, "alex", Some("user"), Some("name")),
            make_memory(
                MemoryKind::Fact,
                "concise",
                Some("preferences"),
                Some("response_style"),
            ),
            make_memory(
                MemoryKind::Fact,
                "Europe/Amsterdam",
                Some("user"),
                Some("timezone"),
            ),
        ];

        let formatted = format_known_facts(&facts);

        assert_eq!(
            formatted,
            "## known facts\n\n### user\n- name: alex\n- timezone: Europe/Amsterdam\n\n### preferences\n- response_style: concise"
        );
    }

    #[test]
    fn test_format_known_facts_truncates_values() {
        let facts = vec![make_memory(
            MemoryKind::Fact,
            &"x".repeat(MAX_FACT_VALUE_CHARS + 10),
            Some("user"),
            Some("bio"),
        )];

        let formatted = format_known_facts(&facts);
        let expected = format!("- bio: {}", "x".repeat(MAX_FACT_VALUE_CHARS));

        assert!(formatted.contains(&expected));
        assert!(!formatted.contains(&"x".repeat(MAX_FACT_VALUE_CHARS + 1)));
    }

    #[test]
    fn test_format_identity_traits() {
        let traits = vec![
            make_memory(
                MemoryKind::Identity,
                "formal and precise",
                None,
                Some("tone"),
            ),
            make_memory(
                MemoryKind::Identity,
                "dry wit, concise",
                None,
                Some("personality"),
            ),
        ];

        let formatted = format_identity_traits(&traits);
        assert!(formatted.contains("## identity"));
        assert!(formatted.contains("- tone: formal and precise"));
        assert!(formatted.contains("- personality: dry wit, concise"));
    }

    #[test]
    fn test_format_recent_episodes() {
        let episodes = vec![
            make_memory(MemoryKind::Episode, "discussed migration plan", None, None),
            make_memory(MemoryKind::Episode, "user mentioned traveling", None, None),
        ];

        let formatted = format_recent_episodes(&episodes);
        assert!(formatted.contains("## recent memories"));
        assert!(formatted.contains("[2024-01-15] discussed migration plan"));
        assert!(formatted.contains("[2024-01-15] user mentioned traveling"));
    }

    #[tokio::test]
    async fn test_agent_tool_loop_executes_and_returns() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    // first call: return a tool call (remember a fact)
                    Ok(ProviderResponse {
                        content: "let me remember that".into(),
                        stop_reason: StopReason::ToolUse,
                        tool_calls: vec![tool::ToolCall {
                            id: "call_1".into(),
                            name: "remember".into(),
                            input: serde_json::json!({
                                "content": "alex",
                                "kind": "fact",
                                "category": "user",
                                "key": "name"
                            }),
                        }],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                } else {
                    // second call: final text response
                    Ok(ProviderResponse {
                        content: "done, i remembered that".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                }
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            test_client(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "my name is alex".into(),
        };

        let outbound = agent.process(&inbound).await.unwrap().unwrap();
        assert_eq!(outbound.content, "done, i remembered that");

        // provider was called twice (tool call round + final response)
        assert_eq!(call_count.load(Ordering::SeqCst), 2);

        // tool actually executed — fact was persisted
        let facts = db.recent_facts().unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "alex");

        // messages were persisted (user + system[time] + assistant[tool_use] + user[tool_result] + system[context] + assistant[final])
        let sid = db.active_session().unwrap();
        let count = db.session_message_count(sid).unwrap();
        assert_eq!(count, 6);
    }

    #[tokio::test]
    async fn test_agent_injects_pending_steer_between_tool_rounds() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc as StdArc, Mutex};

        let db = Arc::new(Database::open_in_memory().unwrap());
        let call_count = Arc::new(AtomicUsize::new(0));
        let seen_steer = StdArc::new(Mutex::new(false));
        let runtime = Arc::new(RuntimeState::new(String::new()));
        let call_count_clone = call_count.clone();
        let seen_steer_clone = seen_steer.clone();
        let runtime_for_provider = Arc::clone(&runtime);

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    assert!(
                        runtime_for_provider
                            .try_push_steer("answer in one sentence", steer_origin())
                    );
                    Ok(ProviderResponse {
                        content: "checking".into(),
                        stop_reason: StopReason::ToolUse,
                        tool_calls: vec![tool::ToolCall {
                            id: "call_1".into(),
                            name: "remember".into(),
                            input: serde_json::json!({
                                "content": "steer test",
                                "kind": "fact"
                            }),
                        }],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                } else {
                    let found = msgs.iter().any(|msg| {
                        msg.content.iter().any(|block| {
                            matches!(
                                block,
                                MessageContent::Text { text }
                                    if text.contains("the user sent /steer")
                                        && text.contains("answer in one sentence")
                            )
                        })
                    });
                    *seen_steer_clone.lock().unwrap() = found;
                    Ok(ProviderResponse {
                        content: "done".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                }
            }),
        });

        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            test_client(),
        )
        .with_runtime(Arc::clone(&runtime));
        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "please remember this".into(),
        };

        runtime.begin_turn();
        let outbound = agent.process(&inbound).await.unwrap().unwrap();
        assert!(runtime.close_turn().is_empty());
        assert_eq!(outbound.content, "done");
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
        assert!(*seen_steer.lock().unwrap());

        let sid = db.active_session().unwrap();
        let history = db.load_recent_messages(sid, 20).unwrap();
        assert!(history.iter().any(|m| m.kind == MessageKind::Steer));
    }

    #[tokio::test]
    async fn test_agent_extends_turn_for_steer_arriving_during_final_call() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc as StdArc, Mutex};

        let db = Arc::new(Database::open_in_memory().unwrap());
        let runtime = Arc::new(RuntimeState::new(String::new()));
        let call_count = Arc::new(AtomicUsize::new(0));
        let seen_context = StdArc::new(Mutex::new(false));
        let runtime_for_provider = Arc::clone(&runtime);
        let call_count_clone = Arc::clone(&call_count);
        let seen_context_clone = Arc::clone(&seen_context);

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    assert!(
                        runtime_for_provider
                            .try_push_steer("make the answer corrected", steer_origin())
                    );
                    Ok(ProviderResponse {
                        content: "initial answer".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                } else {
                    let saw_initial = msgs.iter().any(|msg| {
                        msg.content.iter().any(|block| {
                            matches!(block, MessageContent::Text { text } if text == "initial answer")
                        })
                    });
                    let saw_steer = msgs.iter().any(|msg| {
                        msg.content.iter().any(|block| {
                            matches!(
                                block,
                                MessageContent::Text { text }
                                    if text.contains("the user sent /steer")
                                        && text.contains("make the answer corrected")
                            )
                        })
                    });
                    *seen_context_clone.lock().unwrap() = saw_initial && saw_steer;
                    Ok(ProviderResponse {
                        content: "corrected answer".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                }
            }),
        });

        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            test_client(),
        )
        .with_runtime(Arc::clone(&runtime));
        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "please answer".into(),
        };

        runtime.begin_turn();
        let outbound = agent.process(&inbound).await.unwrap().unwrap();

        assert_eq!(outbound.content, "corrected answer");
        assert_eq!(call_count.load(Ordering::SeqCst), 2);
        assert!(*seen_context.lock().unwrap());
        assert!(runtime.close_turn().is_empty());
        assert!(!runtime.try_push_steer("too late", steer_origin()));

        let sid = db.active_session().unwrap();
        let history = db.load_recent_messages(sid, 20).unwrap();
        assert!(history.iter().any(|m| m.kind == MessageKind::Steer));
        assert!(!history.iter().any(|m| {
            m.role == Role::Assistant
                && m.content.iter().any(|block| {
                    matches!(block, MessageContent::Text { text } if text == "initial answer")
                })
        }));
        assert!(history.iter().any(|m| {
            m.role == Role::Assistant
                && m.content.iter().any(|block| {
                    matches!(block, MessageContent::Text { text } if text == "corrected answer")
                })
        }));
    }

    #[tokio::test]
    async fn test_agent_returns_pending_steer_for_rejection_after_complete_tool_exit() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let runtime = Arc::new(RuntimeState::new(String::new()));
        let runtime_for_provider = Arc::clone(&runtime);

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                assert!(
                    runtime_for_provider.try_push_steer("too late for complete", steer_origin())
                );
                Ok(ProviderResponse {
                    content: "finishing silently".into(),
                    stop_reason: StopReason::ToolUse,
                    tool_calls: vec![tool::ToolCall {
                        id: "call_1".into(),
                        name: tool::COMPLETE_TOOL_NAME.into(),
                        input: serde_json::json!({"reason": "done"}),
                    }],
                    hidden_content: Vec::new(),
                    usage: Usage::default(),
                })
            }),
        });

        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            test_client(),
        )
        .with_runtime(Arc::clone(&runtime));
        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "finish silently".into(),
        };

        runtime.begin_turn();
        let result = agent.process(&inbound).await.unwrap();
        assert!(result.is_none());
        let rejected = runtime.close_turn();

        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].text, "too late for complete");
        assert_eq!(rejected[0].origin, steer_origin());

        let sid = db.active_session().unwrap();
        let history = db.load_recent_messages(sid, 20).unwrap();
        assert!(!history.iter().any(|m| m.kind == MessageKind::Steer));
    }

    #[tokio::test]
    async fn test_agent_returns_pending_steer_for_rejection_after_provider_error() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let runtime = Arc::new(RuntimeState::new(String::new()));
        let runtime_for_provider = Arc::clone(&runtime);

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                assert!(runtime_for_provider.try_push_steer("too late for error", steer_origin()));
                Err(Error::Provider("provider failed".into()))
            }),
        });

        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            test_client(),
        )
        .with_runtime(Arc::clone(&runtime));
        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "please fail".into(),
        };

        runtime.begin_turn();
        let result = agent.process(&inbound).await;
        assert!(result.is_err());
        let rejected = runtime.close_turn();

        assert_eq!(rejected.len(), 1);
        assert_eq!(rejected[0].text, "too late for error");
        assert_eq!(rejected[0].origin, steer_origin());

        let sid = db.active_session().unwrap();
        let history = db.load_recent_messages(sid, 20).unwrap();
        assert!(!history.iter().any(|m| m.kind == MessageKind::Steer));
    }

    #[tokio::test]
    async fn test_agent_multiple_tool_calls_execute_concurrently() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    // first call: return multiple tool calls at once
                    Ok(ProviderResponse {
                        content: "let me remember several things".into(),
                        stop_reason: StopReason::ToolUse,
                        tool_calls: vec![
                            tool::ToolCall {
                                id: "call_1".into(),
                                name: "remember".into(),
                                input: serde_json::json!({
                                    "content": "alex",
                                    "kind": "fact",
                                    "category": "user",
                                    "key": "name"
                                }),
                            },
                            tool::ToolCall {
                                id: "call_2".into(),
                                name: "remember".into(),
                                input: serde_json::json!({
                                    "content": "rust",
                                    "kind": "fact",
                                    "category": "user",
                                    "key": "language"
                                }),
                            },
                            tool::ToolCall {
                                id: "call_3".into(),
                                name: "remember".into(),
                                input: serde_json::json!({
                                    "content": "met at a conference",
                                    "kind": "episode"
                                }),
                            },
                        ],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                } else {
                    Ok(ProviderResponse {
                        content: "remembered everything".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                }
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            test_client(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "remember these things".into(),
        };

        let outbound = agent.process(&inbound).await.unwrap().unwrap();
        assert_eq!(outbound.content, "remembered everything");

        // all three tools executed
        let facts = db.recent_facts().unwrap();
        assert_eq!(facts.len(), 2);
        let episodes = db.recent_episodes().unwrap();
        assert_eq!(episodes.len(), 1);

        // tool results message is clean; context usage is a separate system message
        let sid = db.active_session().unwrap();
        let msgs = db.load_messages(sid).unwrap();
        // user + system[time] + assistant[3 tool_use] + user[3 tool_result] + user[context] + assistant[final]
        assert_eq!(msgs.len(), 6);
        assert_eq!(msgs[3].content.len(), 3);
    }

    #[tokio::test]
    async fn test_agent_tool_loop_limit_graceful() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = std::sync::Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        // provider returns tool calls for the first 41 calls, then a final
        // text response on the 42nd (the text-only summary turn).
        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n <= MAX_TOOL_ROUNDS as usize {
                    Ok(ProviderResponse {
                        content: format!("round {n}"),
                        stop_reason: StopReason::ToolUse,
                        tool_calls: vec![tool::ToolCall {
                            id: format!("call_{n}"),
                            name: "remember".into(),
                            input: serde_json::json!({
                                "content": format!("event {n}"),
                                "kind": "episode"
                            }),
                        }],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                } else {
                    // final text-only turn
                    Ok(ProviderResponse {
                        content: "i've completed 40 rounds of work. here's a summary.".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                }
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "loop forever".into(),
        };

        let result = agent.process(&inbound).await;
        assert!(result.is_ok(), "should return Ok, not Err");
        let outbound = result.unwrap().unwrap();
        assert!(outbound.content.contains("40 rounds"));
    }

    #[tokio::test]
    async fn test_agent_tool_budget_final_turn_fallback() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = std::sync::Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        // provider returns tool calls, and when the final text-only call
        // happens it returns an error — agent should use the static fallback.
        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n <= MAX_TOOL_ROUNDS as usize {
                    Ok(ProviderResponse {
                        content: format!("round {n}"),
                        stop_reason: StopReason::ToolUse,
                        tool_calls: vec![tool::ToolCall {
                            id: format!("call_{n}"),
                            name: "remember".into(),
                            input: serde_json::json!({
                                "content": format!("event {n}"),
                                "kind": "episode"
                            }),
                        }],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                } else {
                    Err(Error::Provider("api error".into()))
                }
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "loop forever".into(),
        };

        let result = agent.process(&inbound).await;
        assert!(
            result.is_ok(),
            "should return Ok even when final call fails"
        );
        let outbound = result.unwrap().unwrap();
        assert!(outbound.content.contains("40 tool rounds"));
    }

    #[tokio::test]
    async fn test_agent_tool_budget_warning_injected() {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = std::sync::Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();
        let seen_warning = Arc::new(Mutex::new(false));
        let seen_warning_clone = seen_warning.clone();

        // provider returns tool calls for WARNING_ROUND rounds, then check
        // that the messages contain the budget warning on the next call.
        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);

                // after the warning round, check if the last user message
                // contains the budget warning text
                if n == WARNING_ROUND as usize {
                    if let Some(last_msg) = msgs.last() {
                        for block in &last_msg.content {
                            if let MessageContent::Text { text } = block
                                && text.contains("[system: you have used")
                            {
                                *seen_warning_clone.lock().unwrap() = true;
                            }
                        }
                    }
                    // return a final text response to end the loop
                    return Ok(ProviderResponse {
                        content: "done".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    });
                }

                Ok(ProviderResponse {
                    content: format!("round {n}"),
                    stop_reason: StopReason::ToolUse,
                    tool_calls: vec![tool::ToolCall {
                        id: format!("call_{n}"),
                        name: "remember".into(),
                        input: serde_json::json!({
                            "content": format!("event {n}"),
                            "kind": "episode"
                        }),
                    }],
                    hidden_content: Vec::new(),
                    usage: Usage::default(),
                })
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "do many things".into(),
        };

        agent.process(&inbound).await.unwrap();
        assert!(
            *seen_warning.lock().unwrap(),
            "budget warning should have been injected at round {WARNING_ROUND}"
        );
    }

    #[tokio::test]
    async fn test_agent_approval_deny_returns_denied() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = std::sync::Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Ok(ProviderResponse {
                        content: "let me run a command".into(),
                        stop_reason: StopReason::ToolUse,
                        tool_calls: vec![tool::ToolCall {
                            id: "call_1".into(),
                            name: "exec".into(),
                            input: serde_json::json!({"command": "echo hi"}),
                        }],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                } else {
                    Ok(ProviderResponse {
                        content: "ok, command was denied".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                }
            }),
        });

        // we can't use AnyApprover with a custom approver, so we need to
        // build the agent differently. since Agent takes AnyApprover, we'll
        // test the routing by checking the tool result message instead.
        // use CliApprover (auto-approves) and verify exec actually runs.
        // for denial testing, we check handle_tool_call_with_approval indirectly.
        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            test_client(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "run echo hi".into(),
        };

        let outbound = agent.process(&inbound).await.unwrap().unwrap();
        // CliApprover auto-approves, so the command executed
        assert_eq!(outbound.content, "ok, command was denied");

        // verify exec tool actually ran (check persisted messages contain tool result)
        let sid = db.active_session().unwrap();
        let msgs = db.load_messages(sid).unwrap();
        // should have: user, system(time), assistant(tool_use), user(tool_result), user(system/context), assistant(final)
        assert_eq!(msgs.len(), 6);
    }

    #[test]
    fn test_system_prompt_includes_all_sections() {
        crate::config::init_workspace_root();
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.mark_setup_complete().unwrap();
        db.remember(MemoryKind::Identity, "formal", None, Some("tone"))
            .unwrap();
        db.remember(MemoryKind::Fact, "alex", Some("user"), Some("name"))
            .unwrap();
        db.remember(MemoryKind::Episode, "discussed rust", None, None)
            .unwrap();

        let provider = make_test_provider("hi");
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());
        let prompt = agent
            .system_prompt(agent.db.active_session().unwrap())
            .unwrap();

        assert!(prompt.contains("session started:"));
        assert!(prompt.contains("## identity"));
        assert!(prompt.contains("- tone: formal"));
        assert!(prompt.contains("## known facts"));
        assert!(prompt.contains("- name: alex"));
        assert!(prompt.contains("## recent memories"));
        assert!(prompt.contains("discussed rust"));
        assert!(prompt.contains("## tool budget"));
        assert!(prompt.contains("40 tool rounds"));
    }

    #[test]
    fn test_system_prompt_includes_pending_tasks() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.mark_setup_complete().unwrap();
        db.add_task("fix CI failure", None).unwrap();
        db.add_task("review PR #42", None).unwrap();

        let provider = make_test_provider("hi");
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());
        let prompt = agent
            .system_prompt(agent.db.active_session().unwrap())
            .unwrap();

        assert!(prompt.contains("## pending tasks"));
        assert!(prompt.contains("fix CI failure"));
        assert!(prompt.contains("review PR #42"));
    }

    #[test]
    fn test_system_prompt_omits_empty_tasks() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.mark_setup_complete().unwrap();

        let provider = make_test_provider("hi");
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());
        let prompt = agent
            .system_prompt(agent.db.active_session().unwrap())
            .unwrap();

        assert!(!prompt.contains("## pending tasks"));
    }

    #[tokio::test]
    async fn test_agent_repairs_orphaned_tool_use() {
        use std::sync::{Arc as StdArc, Mutex};

        let db = Arc::new(Database::open_in_memory().unwrap());
        let sid = db.active_session().unwrap();

        // seed an assistant message with a tool_use block but no tool_result
        let orphaned_assistant = vec![
            MessageContent::text("let me run that"),
            MessageContent::tool_use(
                "orphan_1",
                "exec",
                serde_json::json!({"command": "echo hi"}),
            ),
        ];
        db.append_message(sid, "assistant", &orphaned_assistant, None)
            .unwrap();

        // track messages the provider sees
        let seen_msgs = StdArc::new(Mutex::new(Vec::new()));
        let seen_clone = seen_msgs.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, msgs| {
                *seen_clone.lock().unwrap() = msgs.to_vec();
                Ok(ProviderResponse {
                    content: "recovered from crash".into(),
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    hidden_content: Vec::new(),
                    usage: Usage::default(),
                })
            }),
        });

        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            test_client(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "hello after crash".into(),
        };

        let result = agent.process(&inbound).await;
        assert!(result.is_ok(), "should not error on orphaned tool_use");
        let outbound = result.unwrap().unwrap();
        assert_eq!(outbound.content, "recovered from crash");

        // verify the provider saw the synthetic tool_result
        let msgs = seen_msgs.lock().unwrap();
        // expected: assistant(tool_use) + user(synthetic tool_result) + user(new message) + system(time)
        assert_eq!(msgs.len(), 4);

        // second message should be the synthetic tool_result
        let repair_msg = &msgs[1];
        assert_eq!(repair_msg.role, Role::User);
        assert!(matches!(
            &repair_msg.content[0],
            MessageContent::ToolResult {
                tool_use_id,
                content,
            } if tool_use_id == "orphan_1" && content.as_display_str().contains("interrupted")
        ));
    }

    #[test]
    fn test_system_prompt_excludes_context_usage() {
        // context usage is injected as a message, not in the system prompt,
        // to avoid busting the prompt cache on every tool round.
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.mark_setup_complete().unwrap();
        let provider = make_test_provider("hi");
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());

        let prompt = agent
            .system_prompt(agent.db.active_session().unwrap())
            .unwrap();
        assert!(!prompt.contains("context usage"));
    }

    #[test]
    fn test_system_prompt_excludes_completed_tasks() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.mark_setup_complete().unwrap();
        let id = db.add_task("done task", None).unwrap();
        db.complete_task(id).unwrap();
        db.add_task("pending task", None).unwrap();

        let provider = make_test_provider("hi");
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());
        let prompt = agent
            .system_prompt(agent.db.active_session().unwrap())
            .unwrap();

        assert!(prompt.contains("## pending tasks"));
        assert!(prompt.contains("pending task"));
        assert!(!prompt.contains("done task"));
    }

    #[test]
    fn test_format_pending_tasks() {
        let tasks = vec![
            (3, "investigate CI failure".into()),
            (7, "fix test_session_persistence".into()),
        ];
        let formatted = format_pending_tasks(&tasks);
        assert_eq!(
            formatted,
            "## pending tasks\n- [id:3] investigate CI failure\n- [id:7] fix test_session_persistence"
        );
    }

    #[tokio::test]
    async fn test_agent_complete_tool_returns_none() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                call_count_clone.fetch_add(1, Ordering::SeqCst);
                Ok(ProviderResponse {
                    content: "".into(),
                    stop_reason: StopReason::ToolUse,
                    tool_calls: vec![tool::ToolCall {
                        id: "call_1".into(),
                        name: "complete".into(),
                        input: serde_json::json!({"reason": "memory distillation done"}),
                    }],
                    hidden_content: Vec::new(),
                    usage: Usage::default(),
                })
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            provider,
            AnyApprover::Cli(CliApprover),
            Arc::clone(&db),
            test_client(),
        );

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "distill memories".into(),
        };

        let result = agent.process(&inbound).await.unwrap();
        assert!(result.is_none(), "expected None for complete tool");

        // provider was called once
        assert_eq!(call_count.load(Ordering::SeqCst), 1);

        // messages were persisted (user + system[time] + assistant[tool_use] + user[tool_result])
        let sid = db.active_session().unwrap();
        let count = db.session_message_count(sid).unwrap();
        assert_eq!(count, 4);
    }

    #[test]
    fn test_expand_skill_matches() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let skills = Arc::new(vec![Skill {
            name: "summarize".into(),
            description: "summarize text".into(),
            user_invocable: true,
            disable_model_invocation: false,

            body: "please summarize: $ARGUMENTS".into(),
        }]);
        let agent = Agent::new(
            make_test_provider("hi"),
            AnyApprover::Cli(CliApprover),
            db,
            test_client(),
        )
        .with_skills(skills);

        let expanded = agent.expand_skill("/summarize this is my text");
        assert!(expanded.contains("[skill: summarize]"));
        assert!(expanded.contains("please summarize: this is my text"));
        assert!(expanded.contains("[/skill]"));
        // args are also appended after the skill block
        assert!(expanded.ends_with("this is my text"));
    }

    #[test]
    fn test_expand_skill_no_match() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(
            make_test_provider("hi"),
            AnyApprover::Cli(CliApprover),
            db,
            test_client(),
        );

        // no skills loaded — should pass through unchanged
        assert_eq!(agent.expand_skill("/unknown foo"), "/unknown foo");
        assert_eq!(agent.expand_skill("plain message"), "plain message");
    }

    #[test]
    fn test_expand_skill_not_user_invocable() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let skills = Arc::new(vec![Skill {
            name: "internal".into(),
            description: "internal only".into(),
            user_invocable: false,
            disable_model_invocation: false,

            body: "secret stuff".into(),
        }]);
        let agent = Agent::new(
            make_test_provider("hi"),
            AnyApprover::Cli(CliApprover),
            db,
            test_client(),
        )
        .with_skills(skills);

        // not user-invocable — should not expand
        assert_eq!(agent.expand_skill("/internal test"), "/internal test");
    }

    #[test]
    fn test_expand_skill_no_arguments() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        let skills = Arc::new(vec![Skill {
            name: "status".into(),
            description: "check status".into(),
            user_invocable: true,
            disable_model_invocation: false,

            body: "report the current status. args: $ARGUMENTS".into(),
        }]);
        let agent = Agent::new(
            make_test_provider("hi"),
            AnyApprover::Cli(CliApprover),
            db,
            test_client(),
        )
        .with_skills(skills);

        let expanded = agent.expand_skill("/status");
        assert!(expanded.contains("[skill: status]"));
        assert!(expanded.contains("report the current status. args: "));
    }

    #[test]
    fn test_system_prompt_includes_skills() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.mark_setup_complete().unwrap();
        let skills = Arc::new(vec![
            Skill {
                name: "summarize".into(),
                description: "summarize text".into(),
                user_invocable: true,
                disable_model_invocation: false,

                body: "summarize this".into(),
            },
            Skill {
                name: "hidden".into(),
                description: "hidden from model".into(),
                user_invocable: true,
                disable_model_invocation: true,

                body: "secret".into(),
            },
        ]);
        let agent = Agent::new(
            make_test_provider("hi"),
            AnyApprover::Cli(CliApprover),
            db,
            test_client(),
        )
        .with_skills(skills);

        let prompt = agent
            .system_prompt(agent.db.active_session().unwrap())
            .unwrap();
        assert!(prompt.contains("## available skills"));
        assert!(prompt.contains("**summarize**: summarize text"));
        // hidden skill should not appear
        assert!(!prompt.contains("hidden from model"));
    }

    #[test]
    fn test_system_prompt_omits_skills_when_none() {
        let db = Arc::new(Database::open_in_memory().unwrap());
        db.mark_setup_complete().unwrap();
        let agent = Agent::new(
            make_test_provider("hi"),
            AnyApprover::Cli(CliApprover),
            db,
            test_client(),
        );

        let prompt = agent
            .system_prompt(agent.db.active_session().unwrap())
            .unwrap();
        assert!(!prompt.contains("## available skills"));
    }

    #[tokio::test]
    async fn test_telegram_too_long_response_triggers_retry() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();
        let seen_feedback = Arc::new(std::sync::Mutex::new(false));
        let seen_feedback_clone = seen_feedback.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    // first call: return response that's way over 4096 chars
                    Ok(ProviderResponse {
                        content: "a]".repeat(3000), // ~6000 chars, >4096 after HTML
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                } else {
                    // second call: check that feedback was injected
                    let last_msg = msgs.last().unwrap();
                    for block in &last_msg.content {
                        if let MessageContent::Text { text } = block
                            && text.contains("[system: your response is")
                        {
                            *seen_feedback_clone.lock().unwrap() = true;
                        }
                    }
                    Ok(ProviderResponse {
                        content: "short response".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                }
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());

        let inbound = InboundMessage {
            channel: ChannelKind::Telegram, // must be telegram for length check
            images: Vec::new(),
            content: "write something long".into(),
        };

        let outbound = agent.process(&inbound).await.unwrap().unwrap();
        assert_eq!(outbound.content, "short response");
        assert_eq!(
            call_count.load(Ordering::SeqCst),
            2,
            "provider should be called twice"
        );
        assert!(
            *seen_feedback.lock().unwrap(),
            "feedback message about length should have been injected"
        );
    }

    #[tokio::test]
    async fn test_telegram_too_long_no_retry_on_cli() {
        // the length check should NOT trigger on CLI channel
        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                Ok(ProviderResponse {
                    content: "x".repeat(5000),
                    stop_reason: StopReason::EndTurn,
                    tool_calls: vec![],
                    hidden_content: Vec::new(),
                    usage: Usage::default(),
                })
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());

        let inbound = InboundMessage {
            channel: ChannelKind::Cli, // CLI — no length limit
            images: Vec::new(),
            content: "write something long".into(),
        };

        let outbound = agent.process(&inbound).await.unwrap().unwrap();
        assert_eq!(
            outbound.content.len(),
            5000,
            "CLI should return full response without retry"
        );
    }

    #[tokio::test]
    async fn test_send_file_produces_attachment() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        // create a temp file to send
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let tmp_path = tmp.path().to_str().unwrap().to_string();
        std::fs::write(&tmp_path, b"hello from send_file").unwrap();
        let tmp_path_clone = tmp_path.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Ok(ProviderResponse {
                        content: "sending file".into(),
                        stop_reason: StopReason::ToolUse,
                        tool_calls: vec![tool::ToolCall {
                            id: "call_sf".into(),
                            name: "send_file".into(),
                            input: serde_json::json!({
                                "path": tmp_path_clone,
                                "caption": "test attachment"
                            }),
                        }],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                } else {
                    Ok(ProviderResponse {
                        content: "file sent".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                }
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "send me a file".into(),
        };

        let outbound = agent.process(&inbound).await.unwrap().unwrap();
        assert_eq!(outbound.content, "file sent");
        assert_eq!(outbound.attachments.len(), 1);
        assert_eq!(outbound.attachments[0].bytes, b"hello from send_file");
        assert_eq!(
            outbound.attachments[0].caption.as_deref(),
            Some("test attachment")
        );
        assert_eq!(outbound.attachments[0].kind, tool::AttachmentKind::Document);
    }

    #[tokio::test]
    async fn test_send_photo_produces_photo_attachment() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        // create a temp .png file so the extension check passes
        let tmp_dir = tempfile::tempdir().unwrap();
        let tmp_path = tmp_dir.path().join("shot.png");
        std::fs::write(&tmp_path, b"fake-png-bytes").unwrap();
        let tmp_path_str = tmp_path.to_str().unwrap().to_string();
        let tmp_path_clone = tmp_path_str.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n == 0 {
                    Ok(ProviderResponse {
                        content: "sending photo".into(),
                        stop_reason: StopReason::ToolUse,
                        tool_calls: vec![tool::ToolCall {
                            id: "call_sp".into(),
                            name: "send_photo".into(),
                            input: serde_json::json!({
                                "path": tmp_path_clone,
                                "caption": "a screenshot"
                            }),
                        }],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                } else {
                    Ok(ProviderResponse {
                        content: "photo sent".into(),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                }
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());

        let inbound = InboundMessage {
            channel: ChannelKind::Cli,
            images: Vec::new(),
            content: "send me a photo".into(),
        };

        let outbound = agent.process(&inbound).await.unwrap().unwrap();
        assert_eq!(outbound.content, "photo sent");
        assert_eq!(outbound.attachments.len(), 1);
        assert_eq!(outbound.attachments[0].bytes, b"fake-png-bytes");
        assert_eq!(
            outbound.attachments[0].caption.as_deref(),
            Some("a screenshot")
        );
        assert_eq!(outbound.attachments[0].kind, tool::AttachmentKind::Photo);
    }

    #[tokio::test]
    async fn test_budget_exhausted_too_long_sends_error() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = call_count.clone();

        let provider = AnyProvider::Test(TestProvider {
            handler: Box::new(move |_system, _msgs| {
                let n = call_count_clone.fetch_add(1, Ordering::SeqCst);
                if n <= MAX_TOOL_ROUNDS as usize {
                    // exhaust budget with tool calls
                    Ok(ProviderResponse {
                        content: format!("round {n}"),
                        stop_reason: StopReason::ToolUse,
                        tool_calls: vec![tool::ToolCall {
                            id: format!("call_{n}"),
                            name: "remember".into(),
                            input: serde_json::json!({
                                "content": format!("event {n}"),
                                "kind": "episode"
                            }),
                        }],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                } else {
                    // final text-only turn: return very long response
                    Ok(ProviderResponse {
                        content: "x".repeat(5000),
                        stop_reason: StopReason::EndTurn,
                        tool_calls: vec![],
                        hidden_content: Vec::new(),
                        usage: Usage::default(),
                    })
                }
            }),
        });

        let db = Arc::new(Database::open_in_memory().unwrap());
        let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, test_client());

        let inbound = InboundMessage {
            channel: ChannelKind::Telegram, // must be telegram
            images: Vec::new(),
            content: "loop forever".into(),
        };

        let outbound = agent.process(&inbound).await.unwrap().unwrap();
        // should get error message, not the 5000-char response
        assert!(
            outbound.content.contains("sorry, my response was too long"),
            "expected error message, got: {}",
            &outbound.content[..100.min(outbound.content.len())]
        );
        assert!(outbound.content.len() < TELEGRAM_MAX_MESSAGE_LEN);
    }
}
