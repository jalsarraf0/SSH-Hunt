#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use sqlx::{PgPool, Row};
use uuid::Uuid;

#[derive(Debug, Parser)]
#[command(name = "admin")]
#[command(about = "SSH-Hunt admin CLI")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Migrate,
    Seed,
    Ban { username: String },
    Broadcast { message: String },
    Stats,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    let db_url = std::env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let pool = PgPool::connect(&db_url).await?;

    match cli.command {
        Commands::Migrate => migrate(&pool).await?,
        Commands::Seed => seed(&pool).await?,
        Commands::Ban { username } => ban(&pool, &username).await?,
        Commands::Broadcast { message } => broadcast(&pool, &message).await?,
        Commands::Stats => stats(&pool).await?,
    }

    Ok(())
}

async fn migrate(pool: &PgPool) -> Result<()> {
    sqlx::migrate!("../../migrations").run(pool).await?;
    println!("migrations applied");
    Ok(())
}

async fn seed(pool: &PgPool) -> Result<()> {
    let lore_rows = vec![
        (
            "corp.memo.001",
            "Welcome to CorpSim onboarding. Only some recruits reach NetCity.",
        ),
        (
            "corp.memo.ghost",
            "Ghost nodes are not myths. Watch for hidden prompts in terminal output.",
        ),
        (
            "market.neon",
            "Neon Bazaar prices spike during Black Ice Storm world events.",
        ),
    ];

    for (code, text) in lore_rows {
        sqlx::query(
            r#"
            INSERT INTO lore_entries(code, body)
            VALUES ($1, $2)
            ON CONFLICT (code) DO UPDATE SET body = EXCLUDED.body
            "#,
        )
        .bind(code)
        .bind(text)
        .execute(pool)
        .await?;
    }

    let shop_rows = vec![
        ("script.gremlin.grep", 150),
        ("script.pipe.chain", 230),
        ("consumable.focus_boost", 90),
    ];

    for (sku, price) in shop_rows {
        sqlx::query(
            r#"
            INSERT INTO shop_catalog(sku, price)
            VALUES ($1, $2)
            ON CONFLICT (sku) DO UPDATE SET price = EXCLUDED.price
            "#,
        )
        .bind(sku)
        .bind(price)
        .execute(pool)
        .await?;
    }

    println!("seed content upserted");
    Ok(())
}

async fn ban(pool: &PgPool, username: &str) -> Result<()> {
    let result =
        sqlx::query("UPDATE players SET banned = true, updated_at = now() WHERE username = $1")
            .bind(username)
            .execute(pool)
            .await?;
    println!("banned players: {}", result.rows_affected());
    Ok(())
}

async fn broadcast(pool: &PgPool, message: &str) -> Result<()> {
    sqlx::query("INSERT INTO admin_broadcasts(id, message, created_at) VALUES ($1, $2, now())")
        .bind(Uuid::new_v4())
        .bind(message)
        .execute(pool)
        .await?;
    println!("broadcast queued");
    Ok(())
}

async fn stats(pool: &PgPool) -> Result<()> {
    let players = sqlx::query("SELECT COUNT(*) AS count FROM players")
        .fetch_one(pool)
        .await?
        .try_get::<i64, _>("count")?;

    let auctions = sqlx::query("SELECT COUNT(*) AS count FROM auctions WHERE closed_at IS NULL")
        .fetch_one(pool)
        .await?
        .try_get::<i64, _>("count")?;

    let chats = sqlx::query("SELECT COUNT(*) AS count FROM chats")
        .fetch_one(pool)
        .await?
        .try_get::<i64, _>("count")?;

    println!("players={players} open_auctions={auctions} chat_messages={chats}");
    Ok(())
}
