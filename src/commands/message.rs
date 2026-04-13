use std::sync::Arc;

use crate::agent::Agent;
use crate::approver::{AnyApprover, CliApprover};
use crate::channel::Channel;
use crate::cli::{
    handle_rules_command, handle_switch_command, parse_slash_command, provider_for_session,
};
use crate::db::Database;
use crate::error;
use crate::message::{ChannelKind, InboundMessage};

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
