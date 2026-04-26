mod config;
mod error;
mod i18n;
mod llm;
mod stats;
mod storage;
mod telegram;
mod verification;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, Subcommand};
use teloxide::Bot;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::config::{Config, default_config_path, default_database_path};
use crate::i18n::LocaleRegistry;
use crate::llm::LlmClient;
use crate::storage::Storage;
use crate::verification::Engine;

#[derive(Debug, Parser)]
#[command(name = "bouncer", version, about = "Telegram join-request gatekeeper")]
struct Cli {
    /// Path to the YAML config file.
    #[arg(short = 'c', long, global = true, default_value_os_t = default_config_path())]
    config: PathBuf,

    /// Path to the SQLite database file.
    #[arg(short = 'd', long = "database", global = true, default_value_os_t = default_database_path())]
    database: PathBuf,

    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the bot (default).
    Run,
    /// Print verification statistics globally and per group.
    Stats {
        /// Restrict to a single group chat id.
        #[arg(short = 'g', long, allow_hyphen_values = true)]
        group: Option<i64>,
        /// Only count verifications completed at or after this unix timestamp
        /// (seconds).
        #[arg(short = 's', long)]
        since: Option<i64>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Run) {
        Command::Run => run(&cli.config, &cli.database).await,
        Command::Stats { group, since } => print_stats(&cli.database, group, since).await,
    }
}

fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("bouncer=info,warn"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(false)
        .init();
}

async fn run(config_path: &std::path::Path, db_path: &std::path::Path) -> anyhow::Result<()> {
    let config = Arc::new(
        Config::load(config_path).with_context(|| format!("loading {}", config_path.display()))?,
    );
    let locales = Arc::new(LocaleRegistry::load(&config.i18n.default_locale)?);
    for group in &config.groups {
        if let Some(locale) = &group.locale
            && !locales.is_known(locale)
        {
            anyhow::bail!("group {} references unknown locale `{locale}`", group.id);
        }
    }
    let storage = Storage::open(db_path)
        .with_context(|| format!("opening database {}", db_path.display()))?;
    let llm = Arc::new(LlmClient::new(&config.llm)?);
    let bot = Bot::new(config.telegram.bot_token.clone());

    let engine = Arc::new(Engine::new(
        storage,
        llm,
        bot.clone(),
        config.clone(),
        locales,
    ));
    engine.recover().await?;

    info!(groups = config.groups.len(), "bouncer starting");
    telegram::run(bot, engine).await;
    Ok(())
}

async fn print_stats(
    db_path: &std::path::Path,
    group: Option<i64>,
    since: Option<i64>,
) -> anyhow::Result<()> {
    let storage = Storage::open(db_path)
        .with_context(|| format!("opening database {}", db_path.display()))?;
    let report = stats::render(&storage, group, since).await?;
    print!("{report}");
    Ok(())
}
