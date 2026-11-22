pub mod export;
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
    /// Export one day of stored messages to Markdown files
    Export {
        /// Day to export, YYYYMMDD; defaults to yesterday
        #[arg(value_name = "DATE")]
        date: Option<crate::shared::day::Day>,
        /// Chat id; defaults to every chat with messages that day
        #[arg(allow_hyphen_values = true)]
        chat_id: Option<i64>,
        /// Directory to write the files into
        #[arg(long, value_name = "DIR", default_value = ".")]
        out: std::path::PathBuf,
    },
}

pub async fn run() -> std::process::ExitCode {
    match Cli::parse().command {
        Command::Start => start::run().await,
        Command::Export { date, chat_id, out } => export::run(date, chat_id, out).await,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

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

    #[test]
    fn export_defaults_to_yesterday_every_chat_and_the_current_directory() {
        let cli = Cli::try_parse_from(["u502", "export"]).expect("export should parse");

        match cli.command {
            Command::Export { date, chat_id, out } => {
                assert!(date.is_none());
                assert!(chat_id.is_none());
                assert_eq!(out, PathBuf::from("."));
            }
            Command::Start => panic!("expected the export subcommand"),
        }
    }

    #[test]
    fn export_parses_a_date_a_negative_chat_and_an_output_directory() {
        let cli = Cli::try_parse_from(["u502", "export", "20261121", "-100123", "--out", "/tmp/x"])
            .expect("export should parse");

        match cli.command {
            Command::Export { date, chat_id, out } => {
                assert_eq!(
                    date,
                    Some("20261121".parse().expect("the day should parse"))
                );
                assert_eq!(chat_id, Some(-100_123));
                assert_eq!(out, PathBuf::from("/tmp/x"));
            }
            Command::Start => panic!("expected the export subcommand"),
        }
    }

    #[test]
    fn export_rejects_an_invalid_date() {
        for raw in ["2026-11-21", "20260230"] {
            assert!(
                Cli::try_parse_from(["u502", "export", raw]).is_err(),
                "{raw} should not parse"
            );
        }
    }
}
