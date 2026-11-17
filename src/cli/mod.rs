pub mod start;

use clap::Parser;

#[derive(Debug, clap::Parser)]
#[command(name = "u502", version, about = "u502 car community Telegram bot")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Start the Telegram bot
    Start,
}

pub fn run() {
    match Cli::parse().command {
        Command::Start => start::run(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_start_subcommand() {
        let cli = Cli::try_parse_from(["u502", "start"]).expect("start should parse");
        assert!(matches!(cli.command, Command::Start));
    }

    #[test]
    fn rejects_unknown_subcommand() {
        assert!(Cli::try_parse_from(["u502", "bogus"]).is_err());
    }
}
