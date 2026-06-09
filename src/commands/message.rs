use std::sync::Arc;

use crate::agent::Agent;
use crate::approver::{AnyApprover, CliApprover};
use crate::channel::Channel;
use crate::cli::{
    handle_rules_command, handle_switch_command, parse_slash_command, parse_steer_command,
    provider_for_session,
};
use crate::db::Database;
use crate::error;
use crate::message::{ChannelKind, InboundMessage};

const STEER_REJECTION: &str = "no active turn to steer. send a normal message instead.";

pub(crate) async fn run_message(content: String) -> Result<(), error::Error> {
    let client = reqwest::Client::new();
    let db = Arc::new(Database::open()?);

    if let Some(("switch", args)) = parse_slash_command(&content) {
        let msg = handle_switch_command(args, client, &db);
        println!("{msg}");
        return Ok(());
    }

    if let Some(("rules", args)) = parse_slash_command(&content) {
        let msg = handle_rules_command(args, &db);
        println!("{msg}");
        return Ok(());
    }

    let content = if let Some(steer) = parse_steer_command(&content, None) {
        let steer = steer.trim();
        if steer.is_empty() {
            return Ok(());
        }
        println!("{STEER_REJECTION}");
        return Ok(());
    } else {
        content
    };

    let provider = provider_for_session(client.clone(), &db)?;
    let agent = Agent::new(provider, AnyApprover::Cli(CliApprover), db, client);

    let inbound = InboundMessage {
        channel: ChannelKind::Cli,
        content,
        images: Vec::new(),
    };

    if let Some(outbound) = agent.process(&inbound).await? {
        crate::channel::CliChannel.send(outbound)?;
    }
    Ok(())
}
