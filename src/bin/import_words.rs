use std::io;

use anyhow::Result;
use serde_with::skip_serializing_none;
use sqlx::sqlite::SqlitePoolOptions;

#[derive(Debug, serde::Deserialize)]
#[skip_serializing_none]
struct Record {
    term: String,
    parent: String,
    translation: String,
    tags: String,
    added: Option<String>,
    status: usize,
    link_status: String,
    pronunciation: String,
}

fn non_empty(s: &str) -> Option<String> {
    if s.is_empty() {
        None?
    }
    Some(s.into())
}

#[tokio::main]
async fn main() -> Result<()> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect("lit.db").await?;
    let mut r = csv::Reader::from_reader(io::stdin());
    let mut count = 0;
    let mut txn = pool.begin().await?;
    for result in r.deserialize() {
        count += 1;
        if count % 1000 == 0 {
            println!("Record {count}");
        }

        let rec: Record = result?;
        let text = rec.term.replace('\u{200b}', "");
        let link_status = rec.link_status == "y";

        let result = sqlx::query("
            INSERT INTO word (text, pronunciation, translation, added, status)
            VALUES (?, ?, ?, ?, ?)
        ")
            .bind(text)
            .bind(non_empty(&rec.pronunciation))
            .bind(rec.translation)
            .bind(rec.added)
            .bind(if link_status { None } else { Some(rec.status as i32) })
            .execute(&mut *txn)
            .await?;

        if !rec.tags.trim().is_empty() {
            for tag in rec.tags.split(',').map(|s| s.trim()) {
                sqlx::query("INSERT INTO word_tag (word_id, tag) VALUES (?, ?)")
                    .bind(result.last_insert_rowid())
                    .bind(tag)
                    .execute(&mut *txn)
                    .await?;
            }
        }

        if !rec.parent.trim().is_empty() {
            for parent in rec.parent.split(',').map(|s| s.trim()) {
                sqlx::query("
                    INSERT INTO word_parent (child_word_id, parent_word_text)
                    VALUES (?, ?)
                ")
                    .bind(result.last_insert_rowid())
                    .bind(parent)
                    .execute(&mut *txn)
                    .await?;
            }
        }
    }
    txn.commit().await?;
    Ok(())
}
