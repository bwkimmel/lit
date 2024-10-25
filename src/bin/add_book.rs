use std::{path::PathBuf, str::FromStr};

use anyhow::{anyhow, Result};
use clap::Parser;
use lit::config::Config;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{types::chrono::{DateTime, Local, TimeZone, Utc}, Connection, Database, QueryBuilder, SqliteConnection};
use tokio::{fs::File, io::{AsyncReadExt, BufReader}};
use url::Url;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, help="URL that the book originated from")]
    url: Option<Url>,

    #[arg(short, long="type", help="the MIME type of the book contents")]
    typ: Option<String>,

    #[arg(long, help="tags to apply to the book", value_delimiter=',')]
    tags: Vec<String>,

    #[arg(long, help="path to database file")]
    db: String,

    #[arg(long, help="title of the book")]
    title: Option<String>,

    #[arg(short, long, help="slug to use in URLs referring to the book")]
    slug: Option<String>,

    #[arg(short, long, help="date on which the book was added")]
    added: Option<DateTime<Local>>,

    #[arg(short='P', long, help="date on which the book was published")]
    published: Option<DateTime<Utc>>,

    #[arg(short='R', long, help="date on which the book was last read")]
    last_read: Option<DateTime<Local>>,

    #[arg(short='A', long, help="indicates whether the book is archived")]
    archived: bool,

    #[arg(long, help="path to the audio file for the book")]
    audio: Option<String>,

    #[arg(long, help="path to yt-dlp metadata file")]
    metadata: Option<PathBuf>,

    #[arg(help="file containing the text of the book")]
    input: String,

    #[arg(long, help="only add book if one does not already exist with the same URL")]
    unique_url: bool,

    #[arg(long, help="automatically disambiguate title if identical title already exists")]
    allow_duplicate_title: bool,

    #[arg(long, help="overwrite existing book if it already exists")]
    overwrite: bool,

    #[arg(short, long, help="path to configuration file")]
    config: String,

    #[arg(
        long,
        help="include these fields when updating an existing book",
        value_delimiter=',',
        value_parser=[
            "title",
            "slug",
            "url",
            "added",
            "published",
            "last_read",
            "archived",
            "audio_file",
            "content_type",
            "content",
            "tags",
        ],
    )]
    include_fields_on_update: Vec<String>,

    #[arg(
        long,
        help="exclude these fields when updating an existing book",
        value_delimiter=',',
        value_parser=[
            "title",
            "slug",
            "url",
            "added",
            "published",
            "last_read",
            "archived",
            "audio_file",
            "content_type",
            "content",
            "tags",
        ],
    )]
    exclude_fields_on_update: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct VideoMetadata {
    channel: Option<String>,
    title: String,
    timestamp: i64,
    webpage_url: String,
    tags: Option<Vec<String>>,
}

fn should_update_field(field: &str, args: &Args) -> bool {
    let field = field.to_string();
    if !args.include_fields_on_update.is_empty() {
        return args.include_fields_on_update.contains(&field);
    } else if !args.exclude_fields_on_update.is_empty() {
        return !args.exclude_fields_on_update.contains(&field);
    }
    true
}

fn update_field<DB: Database>(query: &mut QueryBuilder<'_, DB>, field: &str, args: &Args) -> bool {
    if !should_update_field(field, args) {
        return false;
    }
    if query.sql() != "UPDATE book SET" {
        query.push(",");
    }
    let field = field.to_string();
    query.push(format!(" {field} = "));
    true
}

fn title_from_metadata(md: &VideoMetadata) -> String {
    let mut title = String::new();
    if let Some(ref channel) = md.channel {
        title += &format!("[{channel}]: ");
    }
    title += &md.title;
    title
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if !args.include_fields_on_update.is_empty() && !args.exclude_fields_on_update.is_empty() {
        Err(anyhow!("cannot specify both --include-fields-on-update and --exclude-fields-on-update"))?;
    }

    let config = Config::load(&args.config)?;
    tokio::fs::create_dir_all(config.word_images_path()).await?;
    tokio::fs::create_dir_all(config.book_audio_path()).await?;

    let md: Option<VideoMetadata> = if let Some(ref path) = args.metadata {
        let json = tokio::fs::read_to_string(path).await?;
        Some(serde_json::from_str(&json)?)
    } else { None };

    let Some(title) = args.title.clone().or(md.as_ref().map(title_from_metadata)) else {
        return Err(anyhow!("either --title or --metadata must be set"))?;
    };
    let mut url = args.url.clone();
    if url.is_none() {
        if let Some(md) = md.as_ref() {
            url = Some(Url::from_str(&md.webpage_url)?)
        }
    }
    let published = args.published.or_else(|| md.as_ref().map(|md| {
        Utc.timestamp_opt(md.timestamp, 0).unwrap()
    }));

    let mut conn = SqliteConnection::connect(&args.db).await?;

    let mut title = title;
    if args.allow_duplicate_title {
        let base_title = title.clone();
        let mut suffix = 1;
        loop {
            let (count,): (i32,) = sqlx::query_as("SELECT COUNT(*) FROM book WHERE title = ?")
               .bind(&title)
               .fetch_one(&mut conn)
               .await?;
            if count == 0 {
                break;
            }
            suffix += 1;
            title = format!("{base_title} [{suffix}]");
        }
    }
    let title = title;

    let content_type = args.typ.clone().unwrap_or_else(|| {
        if args.input.ends_with(".vtt") {
            "text/vtt"
        } else if args.input.ends_with(".md") {
            "text/markdown"
        } else {
            "text/plain"
        }.to_string()
    });

    let mut tags = args.tags.clone();
    if let Some(md) = md {
        for tag in md.tags.clone().unwrap_or_default() {
            if !tags.contains(&tag) {
                tags.push(tag.clone());
            }
        }
    }

    let mut rdr = BufReader::new(File::open(&args.input).await?);
    let mut text = String::new();
    rdr.read_to_string(&mut text).await?;
    drop(rdr);
    let text = text;

    let mut audio = args.audio.clone();
    if let Some(path) = audio {
        let mut hash = Sha256::new();
        let mut file = std::fs::File::open(path)?;
        let _ = std::io::copy(&mut file, &mut hash)?;
        let hash_bytes = hash.finalize();
        audio = Some(format!("{:x}.mp3", hash_bytes));
        tokio::fs::copy(args.audio.clone().unwrap(), config.book_audio_path().join(audio.clone().unwrap())).await?;
    }

    let mut txn = conn.begin().await?;

    let mut existing_id = None;
    if args.overwrite {
        if let Some(ref slug) = args.slug {
            let result: Option<(i64,)> = sqlx::query_as("SELECT id FROM book WHERE slug = ?")
                .bind(slug)
                .fetch_optional(&mut *txn)
                .await?;
            if let Some((id,)) = result {
                existing_id = Some(id);
            }
        }
        if args.unique_url && existing_id.is_none() {
            if let Some(url) = url.as_ref() {
                let result: Option<(i64,)> = sqlx::query_as("SELECT id FROM book WHERE url = ?")
                    .bind(url.to_string())
                    .fetch_optional(&mut *txn)
                    .await?;
                if let Some((id,)) = result {
                    existing_id = Some(id);
                }
            }
        }
    } else if args.unique_url {
        if let Some(url) = url.as_ref() {
            let (count,): (i32,) = sqlx::query_as("SELECT COUNT(*) FROM book WHERE url = ?")
                .bind(url.to_string())
                .fetch_one(&mut *txn)
                .await?;
            if count > 0 {
                println!("A book matching the URL already exists: {url}");
                return Ok(());
            }
        }
    }

    let mut updating = false;
    if let Some(id) = existing_id {
        updating = true;
        if should_update_field("tags", &args) {
            sqlx::query("DELETE FROM book_tag WHERE book_id = ?")
                .bind(id)
                .execute(&mut *txn)
                .await?;
        }
        let mut query = QueryBuilder::new("UPDATE book SET");
        if update_field(&mut query, "title", &args) {
            query.push_bind(title);
        }
        if update_field(&mut query, "slug", &args) {
            query.push_bind(args.slug.clone());
        }
        if update_field(&mut query, "url", &args) {
            query.push_bind(url.map(|u| u.to_string()));
        }
        if update_field(&mut query, "added", &args) {
            query.push_bind(args.added.unwrap_or_else(Local::now));
        }
        if update_field(&mut query, "published", &args) {
            query.push_bind(published);
        }
        if let Some(last_read) = args.last_read {
            if update_field(&mut query, "last_read", &args) {
                query.push("COALESCE(MAX(");
                query.push_bind(last_read);
                query.push(", (SELECT last_read FROM book WHERE id = ");
                query.push_bind(id);
                query.push(")), ");
                query.push(last_read);
                query.push(")");
            }
        }
        if update_field(&mut query, "archived", &args) {
            query.push_bind(args.archived);
        }
        if update_field(&mut query, "audio_file", &args) {
            query.push_bind(audio);
        }
        if update_field(&mut query, "content_type", &args) {
            query.push_bind(content_type);
        }
        if update_field(&mut query, "content", &args) {
            query.push_bind(text);
        }
        query.push(" WHERE id = ");
        query.push_bind(id);
        query.build().execute(&mut *txn).await?;
    } else { // insert
        let result = sqlx::query("
            INSERT INTO book (title, slug, url, added, published, last_read, archived, audio_file, content_type, content)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
        ")
            .bind(title)
            .bind(args.slug.clone())
            .bind(url.map(|u| u.to_string()))
            .bind(args.added.unwrap_or_else(Local::now))
            .bind(published)
            .bind(args.last_read)
            .bind(args.archived)
            .bind(audio)
            .bind(content_type)
            .bind(text)
            .execute(&mut *txn)
            .await?;
        existing_id = Some(result.last_insert_rowid());
    }

    let Some(id) = existing_id else {
        return Err(anyhow!("failed to insert or update book"));
    };

    if !updating || should_update_field("tags", &args) {
        for tag in tags {
            sqlx::query("INSERT INTO book_tag (book_id, tag) VALUES (?, ?)")
                .bind(id)
                .bind(tag)
                .execute(&mut *txn)
                .await?;
        }
    }

    txn.commit().await?;
    Ok(())
}
