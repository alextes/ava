mod cli;
pub mod telegram;

pub use cli::CliChannel;

use crate::error::Error;
use crate::message::OutboundMessage;

pub trait Channel {
    fn send(&self, message: OutboundMessage) -> Result<(), Error>;
}
