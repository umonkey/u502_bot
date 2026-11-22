//! The `export` command: read one local day of stored messages and write one Markdown file per chat.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use jiff::tz::TimeZone;

use crate::domains::message::{ExportedMessage, MessageRepository};
use crate::infra::config;
use crate::infra::database::Database;
use crate::shared::day::Day;
use crate::shared::result::{Error, Result};

/// Runs the command and maps its outcome to the process exit code.
pub async fn run(day: Option<Day>, chat_id: Option<i64>, out: PathBuf) -> ExitCode {
    match export_all(day, chat_id, &out).await {
        Ok((day, paths)) if paths.is_empty() => {
            match chat_id {
                Some(chat_id) => {
                    eprintln!("No messages to export for chat {chat_id} on {}", day.iso());
                }
                None => eprintln!("No messages to export on {}", day.iso()),
            }

            ExitCode::SUCCESS
        }
        Ok((_, paths)) => {
            for path in paths {
                println!("{}", path.display());
            }

            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("error: {error}");

            ExitCode::FAILURE
        }
    }
}

/// Resolves the database and the day, then closes the pool whatever the export did.
async fn export_all(
    day: Option<Day>,
    chat_id: Option<i64>,
    out: &Path,
) -> Result<(Day, Vec<PathBuf>)> {
    let path = config::database_path().await?;
    let database = open_existing(&path).await?;
    let outcome = export_within(&database, day, chat_id, out).await;
    database.close().await;

    outcome
}

/// Opens a database that must already exist: a typo in the path is not an empty export.
async fn open_existing(path: &Path) -> Result<Database> {
    if !tokio::fs::try_exists(path).await.unwrap_or(true) {
        return Err(Error::DatabaseNotFound(path.to_path_buf()));
    }

    Database::connect(path).await
}

async fn export_within(
    database: &Database,
    day: Option<Day>,
    chat_id: Option<i64>,
    out: &Path,
) -> Result<(Day, Vec<PathBuf>)> {
    let tz = TimeZone::try_system()?;
    let day = match day {
        Some(day) => day,
        None => Day::yesterday_from(jiff::ZonedDateTime::now().date())?,
    };
    let repository = MessageRepository::new(database.pool().clone());
    let paths = export_day(&repository, day, chat_id, out, &tz).await?;

    Ok((day, paths))
}

/// Writes one file per chat that has renderable text, in ascending chat order.
async fn export_day(
    repository: &MessageRepository,
    day: Day,
    chat_id: Option<i64>,
    out: &Path,
    tz: &TimeZone,
) -> Result<Vec<PathBuf>> {
    let (start, end) = day.bounds(tz)?;
    let chats = match chat_id {
        Some(chat_id) => vec![chat_id],
        None => repository.chat_ids_between(start, end).await?,
    };

    tokio::fs::create_dir_all(out)
        .await
        .map_err(|source| Error::CreateExportDirectory {
            path: out.to_path_buf(),
            source,
        })?;

    let mut paths = Vec::new();
    for chat_id in chats {
        let messages = repository.messages_between(chat_id, start, end).await?;
        if messages.is_empty() {
            continue;
        }

        let contents = render(&day, tz, &messages)?;
        let file = out.join(format!("{}-{chat_id}.md", day.compact()));
        tokio::fs::write(&file, contents)
            .await
            .map_err(|source| Error::WriteFile {
                path: file.clone(),
                source,
            })?;
        paths.push(file);
    }

    Ok(paths)
}

fn render(day: &Day, tz: &TimeZone, messages: &[ExportedMessage]) -> Result<String> {
    let mut output = format!("# Chat Log: {}\n\n", day.iso());

    for message in messages {
        let text = message.text.replace("\r\n", "\n");
        let text = text.trim_end_matches(&['\n', '\r'][..]);
        let text = text.replace('\n', "\n    ");
        output.push_str(&format!(
            "[{}] (#{}) {}: {}\n",
            day.clock(message.received_at, tz)?,
            message.message_id,
            message.sender_name.as_deref().unwrap_or("Unknown"),
            text,
        ));
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use jiff::tz;

    use crate::domains::message::NewMessage;

    use super::*;

    fn day(raw: &str) -> Day {
        raw.parse().expect("the day should parse")
    }

    async fn in_memory_repository() -> (Database, MessageRepository) {
        let database = Database::connect_in_memory()
            .await
            .expect("in-memory database should open");
        database.apply_schema().await.expect("schema should apply");
        let repository = MessageRepository::new(database.pool().clone());

        (database, repository)
    }

    async fn store(
        repository: &MessageRepository,
        chat_id: i64,
        message_id: i64,
        received_at: i64,
        sender_name: Option<&str>,
        text: Option<&str>,
    ) {
        repository
            .store(
                &NewMessage {
                    chat_id,
                    message_id,
                    received_at,
                    sender_id: None,
                    sender_name: sender_name.map(str::to_owned),
                    text: text.map(str::to_owned),
                },
                None,
            )
            .await
            .expect("the message should store");
    }

    #[test]
    fn render_writes_the_header_and_pads_the_clock() {
        let tz = TimeZone::fixed(tz::offset(3));
        let messages = vec![
            ExportedMessage {
                message_id: 10421,
                received_at: 1_795_323_840,
                sender_name: Some("Alexey".to_owned()),
                text: "Guys, white smoke on cold start.".to_owned(),
            },
            ExportedMessage {
                message_id: 10422,
                received_at: 1_795_324_020,
                sender_name: None,
                text: "Check your HPFP.".to_owned(),
            },
        ];

        let contents = render(&day("20261122"), &tz, &messages).expect("the day should render");

        assert_eq!(
            contents,
            "# Chat Log: 2026-11-22\n\n\
             [08:04] (#10421) Alexey: Guys, white smoke on cold start.\n\
             [08:07] (#10422) Unknown: Check your HPFP.\n"
        );
    }

    #[test]
    fn render_indents_every_continuation_line_by_four_spaces() {
        let tz = TimeZone::fixed(tz::offset(3));
        let messages = vec![
            ExportedMessage {
                message_id: 1,
                received_at: 1_795_323_840,
                sender_name: None,
                text: "first\nsecond\n\nfourth".to_owned(),
            },
            ExportedMessage {
                message_id: 2,
                received_at: 1_795_323_900,
                sender_name: None,
                text: "ends in a newline\n".to_owned(),
            },
            ExportedMessage {
                message_id: 3,
                received_at: 1_795_323_960,
                sender_name: None,
                text: "windows\r\nstyle\r\n".to_owned(),
            },
        ];

        let contents = render(&day("20261122"), &tz, &messages).expect("the day should render");

        assert_eq!(
            contents,
            "# Chat Log: 2026-11-22\n\n\
             [08:04] (#1) Unknown: first\n    second\n    \n    fourth\n\
             [08:05] (#2) Unknown: ends in a newline\n\
             [08:06] (#3) Unknown: windows\n    style\n"
        );
    }

    #[tokio::test]
    async fn export_day_writes_one_file_per_chat() {
        let (_database, repository) = in_memory_repository().await;
        let tz = TimeZone::fixed(tz::offset(3));
        let day = day("20261122");
        let (start, end) = day.bounds(&tz).expect("bounds should resolve");
        store(
            &repository,
            42,
            1,
            start + 3_600,
            Some("Ada"),
            Some("first"),
        )
        .await;
        store(&repository, 42, 2, start + 7_200, None, Some("second")).await;
        store(
            &repository,
            -1001,
            5,
            start + 60,
            Some("Bob"),
            Some("group"),
        )
        .await;
        store(&repository, 7, 3, start + 120, Some("Ada"), None).await;
        store(&repository, 99, 4, end, Some("Ada"), Some("the next day")).await;
        let directory = tempfile::tempdir().expect("a temporary directory should be created");

        let paths = export_day(&repository, day, None, directory.path(), &tz)
            .await
            .expect("the day should export");

        assert_eq!(
            paths,
            vec![
                directory.path().join("20261122--1001.md"),
                directory.path().join("20261122-42.md"),
            ]
        );
        assert_eq!(
            std::fs::read_to_string(&paths[0]).expect("the file should be readable"),
            "# Chat Log: 2026-11-22\n\n[00:01] (#5) Bob: group\n"
        );
        assert_eq!(
            std::fs::read_to_string(&paths[1]).expect("the file should be readable"),
            "# Chat Log: 2026-11-22\n\n\
             [01:00] (#1) Ada: first\n\
             [02:00] (#2) Unknown: second\n"
        );
        assert!(!directory.path().join("20261122-7.md").exists());
        assert!(!directory.path().join("20261122-99.md").exists());
    }

    #[tokio::test]
    async fn export_day_restricts_an_explicit_chat() {
        let (_database, repository) = in_memory_repository().await;
        let tz = TimeZone::fixed(tz::offset(3));
        let day = day("20261122");
        let (start, _) = day.bounds(&tz).expect("bounds should resolve");
        store(&repository, 42, 1, start + 60, Some("Ada"), Some("first")).await;
        store(&repository, 7, 2, start + 60, Some("Bob"), Some("second")).await;
        let directory = tempfile::tempdir().expect("a temporary directory should be created");

        let paths = export_day(&repository, day, Some(7), directory.path(), &tz)
            .await
            .expect("the chat should export");

        assert_eq!(paths, vec![directory.path().join("20261122-7.md")]);
    }

    #[tokio::test]
    async fn export_day_creates_a_missing_directory() {
        let (_database, repository) = in_memory_repository().await;
        let tz = TimeZone::fixed(tz::offset(3));
        let day = day("20261122");
        let (start, _) = day.bounds(&tz).expect("bounds should resolve");
        store(&repository, 42, 1, start + 60, Some("Ada"), Some("first")).await;
        let directory = tempfile::tempdir().expect("a temporary directory should be created");
        let out = directory.path().join("nested").join("export");

        let paths = export_day(&repository, day, None, &out, &tz)
            .await
            .expect("the day should export");

        assert_eq!(paths, vec![out.join("20261122-42.md")]);
        assert!(out.is_dir());
    }

    #[tokio::test]
    async fn a_missing_database_is_not_created() {
        let directory = tempfile::tempdir().expect("a temporary directory should be created");
        let path = directory.path().join("absent.sqlite");

        let error = match open_existing(&path).await {
            Ok(_) => panic!("a missing database should fail"),
            Err(error) => error,
        };

        assert!(matches!(
            error,
            Error::DatabaseNotFound(reported) if reported == path
        ));
        assert!(!path.exists());
    }
}
