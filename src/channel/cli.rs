use crate::channel::Channel;
use crate::error::Error;
use crate::message::OutboundMessage;

pub struct CliChannel;

impl Channel for CliChannel {
    fn send(&self, message: OutboundMessage) -> Result<(), Error> {
        println!("{}", message.content);
        Ok(())
    }
}
