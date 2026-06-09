use crate::config;
use crate::db::{Database, session::MessageUsageRecord};
use crate::pricing;

pub(crate) fn run_status() {
    println!("ava {}", env!("CARGO_PKG_VERSION"));
    println!("db: {}", config::default_db_path().display());
    if let Ok(db) = Database::open()
        && let Ok(sid) = db.active_session()
    {
        let msg_count = db.session_message_count(sid).unwrap_or(0);
        println!("session: {sid} ({msg_count} messages)");
        let model = db.session_model_reasoning(sid).ok().flatten();
        match db.session_usage(sid) {
            Ok(Some((input_tokens, context_window))) if context_window > 0 => {
                let pct = input_tokens as f64 / context_window as f64 * 100.0;
                if let Some((model_id, _)) = model.as_ref() {
                    let replay_cost = pricing::format_replay_cost(model_id, input_tokens);
                    println!(
                        "context: {pct:.0}% ({input_tokens}/{context_window} tokens, replay input {replay_cost})"
                    );
                } else {
                    println!("context: {pct:.0}% ({input_tokens}/{context_window} tokens)");
                }
            }
            _ => {
                println!("context: unknown");
            }
        }
        if let Ok(records) = db.session_usage_records(sid) {
            println!("session cost: {}", session_cost_label(&records));
        }
        if let Some((model, reasoning)) = model {
            match reasoning {
                Some(effort) => println!("model: {model} (reasoning: {effort})"),
                None => println!("model: {model}"),
            }
        }
    }
}

fn session_cost_label(records: &[MessageUsageRecord]) -> String {
    let mut known_usd = 0.0;
    let mut known = 0_u32;
    let mut unknown = 0_u32;

    for record in records {
        let Some(model_id) = record.model_id.as_deref() else {
            unknown += 1;
            continue;
        };
        match pricing::estimate_usage_cost_usd(model_id, &record.usage) {
            Some(usd) => {
                known += 1;
                known_usd += usd;
            }
            None => unknown += 1,
        }
    }

    match (known > 0, unknown) {
        (true, 0) => pricing::format_cost(Some(known_usd)),
        (true, count) => format!(
            "{} ({count} unknown)",
            pricing::format_cost(Some(known_usd))
        ),
        (false, 0) => "unknown".to_string(),
        (false, _) => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::Usage;

    #[test]
    fn session_cost_label_sums_known_records() {
        let records = vec![
            MessageUsageRecord {
                model_id: Some("anthropic/claude-sonnet-4-6".into()),
                usage: Usage {
                    input_tokens: 100_000,
                    output_tokens: 10_000,
                    ..Default::default()
                },
            },
            MessageUsageRecord {
                model_id: Some("openai/gpt-5.4".into()),
                usage: Usage {
                    input_tokens: 20_000,
                    output_tokens: 2_000,
                    ..Default::default()
                },
            },
        ];

        assert_eq!(session_cost_label(&records), "~$0.53");
    }

    #[test]
    fn session_cost_label_reports_unknown_records() {
        let records = vec![
            MessageUsageRecord {
                model_id: Some("anthropic/claude-sonnet-4-6".into()),
                usage: Usage {
                    input_tokens: 100_000,
                    output_tokens: 10_000,
                    ..Default::default()
                },
            },
            MessageUsageRecord {
                model_id: Some("unknown/model".into()),
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    ..Default::default()
                },
            },
        ];

        assert_eq!(session_cost_label(&records), "~$0.45 (1 unknown)");
    }
}
