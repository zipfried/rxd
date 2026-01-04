use std::path::Path;

use sha2::{Digest, Sha256};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};
use tokio::fs;
use tracing::{info, instrument};

/// Initialize database connection pool and create tables
#[instrument(skip_all)]
pub async fn init_db(
    db_path: &Path,
) -> Result<SqlitePool, Box<dyn std::error::Error + Send + Sync>> {
    let options = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(true);

    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await?;

    // Create tweets table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS tweets (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tweet_id TEXT NOT NULL UNIQUE,
            screen_name TEXT NOT NULL,
            tweet_time TEXT NOT NULL,
            full_text TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )
        "#,
    )
    .execute(&pool)
    .await?;

    // Create media table
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS media (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            tweet_id TEXT NOT NULL,
            media_url TEXT NOT NULL UNIQUE,
            filename TEXT,
            file_hash TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            FOREIGN KEY (tweet_id) REFERENCES tweets(tweet_id)
        )
        "#,
    )
    .execute(&pool)
    .await?;

    // Create indexes
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_tweets_screen_name ON tweets(screen_name)")
        .execute(&pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_media_tweet_id ON media(tweet_id)")
        .execute(&pool)
        .await?;

    info!("database initialized at {}", db_path.display());
    Ok(pool)
}

/// Insert or update a tweet record
#[instrument(skip_all)]
pub async fn upsert_tweet(
    pool: &SqlitePool,
    tweet_id: &str,
    screen_name: &str,
    tweet_time: &str,
    full_text: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query(
        r#"
        INSERT INTO tweets (tweet_id, screen_name, tweet_time, full_text)
        VALUES (?, ?, ?, ?)
        ON CONFLICT(tweet_id) DO UPDATE SET
            full_text = excluded.full_text
        "#,
    )
    .bind(tweet_id)
    .bind(screen_name)
    .bind(tweet_time)
    .bind(full_text)
    .execute(pool)
    .await?;

    Ok(())
}

/// Insert or update a media record
#[instrument(skip_all)]
pub async fn upsert_media(
    pool: &SqlitePool,
    tweet_id: &str,
    media_url: &str,
    filename: Option<&str>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query(
        r#"
        INSERT INTO media (tweet_id, media_url, filename)
        VALUES (?, ?, ?)
        ON CONFLICT(media_url) DO UPDATE SET
            filename = excluded.filename
        "#,
    )
    .bind(tweet_id)
    .bind(media_url)
    .bind(filename)
    .execute(pool)
    .await?;

    Ok(())
}

/// Update file hash after download
#[instrument(skip_all)]
pub async fn update_hash(
    pool: &SqlitePool,
    media_url: &str,
    file_hash: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    sqlx::query("UPDATE media SET file_hash = ? WHERE media_url = ?")
        .bind(file_hash)
        .bind(media_url)
        .execute(pool)
        .await?;

    Ok(())
}

/// Media record from database
#[derive(Debug)]
pub struct MediaRecord {
    pub filename: Option<String>,
    pub file_hash: Option<String>,
}

/// Get media record by URL
#[instrument(skip_all)]
pub async fn get_media_by_url(
    pool: &SqlitePool,
    media_url: &str,
) -> Result<Option<MediaRecord>, Box<dyn std::error::Error + Send + Sync>> {
    let row = sqlx::query("SELECT filename, file_hash FROM media WHERE media_url = ?")
        .bind(media_url)
        .fetch_optional(pool)
        .await?;

    Ok(row.map(|r| MediaRecord {
        filename: r.get("filename"),
        file_hash: r.get("file_hash"),
    }))
}

/// Calculate SHA-256 hash of file content
pub fn calculate_hash(data: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(data);
    format!("{:x}", hasher.finalize())
}

/// Verify if file exists and has matching hash
#[instrument(skip_all)]
pub async fn verify_file(
    pool: &SqlitePool,
    media_url: &str,
    save_path: &Path,
) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
    let record = match get_media_by_url(pool, media_url).await? {
        Some(r) => r,
        None => return Ok(false),
    };

    let (filename, expected_hash) = match (record.filename, record.file_hash) {
        (Some(f), Some(h)) => (f, h),
        _ => return Ok(false),
    };

    let filepath = save_path.join(&filename);
    if !filepath.exists() {
        return Ok(false);
    }

    let content = fs::read(&filepath).await?;
    let actual_hash = calculate_hash(&content);

    Ok(actual_hash == expected_hash)
}
