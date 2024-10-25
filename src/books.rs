use std::path::PathBuf;

use anyhow::anyhow;
use chrono::DateTime;
use futures::{Stream, TryStreamExt};
use itertools::Itertools;
use serde::Serialize;
use serde_with::skip_serializing_none;
use sqlx::{Pool, Sqlite};

use crate::{bad_req, dt, must, not_found, Result};

#[derive(sqlx::FromRow)]
pub struct Book {
    pub id: i64,
    pub slug: String,
    pub title: String,
    pub content_type: String,
    pub content: String,
    pub audio_file: Option<String>,
    pub url: Option<String>,
    pub published: Option<DateTime<chrono::Utc>>,
    pub last_read: Option<DateTime<chrono::Local>>,
}

#[derive(Clone)]
pub struct Books {
    db: Pool<Sqlite>,
    book_audio_path: PathBuf,
}

impl Books {
    pub fn new(db: Pool<Sqlite>, book_audio_path: PathBuf) -> Self {
        Self { db, book_audio_path }
    }

    pub async fn find_book_by_slug(&self, slug: String) -> Result<Book> {
        if let Ok(book_id) = slug.parse() {
            return self.find_book_by_id(book_id).await;
        }
        let book = must(sqlx::query_as("SELECT * FROM book WHERE slug = ?")
            .bind(slug)
            .fetch_optional(&self.db)
            .await?)?;
        Ok(book)
    }

    pub async fn find_book_ids_by_url(&self, url: &str) -> Result<Vec<i64>> {
        let result = sqlx::query_as("SELECT id FROM book WHERE url = ?")
            .bind(url)
            .fetch_all(&self.db)
            .await?;
        Ok(result.into_iter().map(|(id,)| id).collect_vec())
    }

    pub async fn find_book_by_id(&self, id: i64) -> Result<Book> {
        let book = must(sqlx::query_as("SELECT * FROM book WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.db)
            .await?)?;
        Ok(book)
    }

    pub async fn get_book_audio_path(&self, id: i64) -> Result<PathBuf> {
        let (path,): (String,) = must(sqlx::query_as("SELECT audio_file FROM book WHERE id = ?")
            .bind(id)
            .fetch_optional(&self.db)
            .await?)?;
        let path = self.book_audio_path.join(path);
        Ok(path)
    }

    pub async fn mark_book_read(&self, id: i64) -> Result<()> {
        let result = sqlx::query("UPDATE book SET last_read = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(id)
            .execute(&self.db)
            .await?;
        if result.rows_affected() != 1 {
            not_found()?;
        }
        Ok(())
    }

    pub async fn set_book_content(&self, id: i64, content: &str) -> Result<()> {
        let result = sqlx::query("UPDATE book SET content = ? WHERE id = ?")
            .bind(content)
            .bind(id)
            .execute(&self.db)
            .await?;
        if result.rows_affected() != 1 {
            not_found()?;
        }
        Ok(())
    }

    pub fn all_books(&self) -> impl Stream<Item = Result<Book>> + use<'_> {
        sqlx::query_as("SELECT * FROM book")
            .fetch(&self.db)
            .map_err(move |err| anyhow!("failed reading book: {err}").into())
    } 

    pub fn search_books(&self, filter: String) -> impl Stream<Item = Result<Book>> + use<'_> {
        sqlx::query_as("
            SELECT *
            FROM book
            WHERE id IN (SELECT rowid FROM book_fts WHERE book_fts MATCH ?)
        ")
            .bind(filter)
            .fetch(&self.db)
            .map_err(move |err| anyhow!("failed reading book: {err}").into())
    }
}

pub struct NewBook {
    pub slug: Option<String>,
    pub title: String,
    pub content_type: String,
    pub content: String,
    pub audio_file: Option<String>,
    pub url: Option<String>,
    pub published: Option<DateTime<chrono::Utc>>,
    pub tags: Vec<String>,
}

impl Books {
    pub async fn insert_book(&self, book: NewBook) -> Result<i64> {
        let mut txn = self.db.begin().await?;
        let result = sqlx::query("
            INSERT INTO book (slug, title, content_type, content, audio_file, url, published)
            VALUES (?, ?, ?, ?, ?, ?, ?)
        ")
            .bind(book.slug)
            .bind(book.title)
            .bind(book.content_type)
            .bind(book.content)
            .bind(book.audio_file)
            .bind(book.url)
            .bind(book.published)
            .execute(&mut *txn)
            .await?;
        let id = result.last_insert_rowid();
        for tag in book.tags {
            sqlx::query("INSERT INTO book_tag (book_id, tag) VALUES (?, ?)")
                .bind(id)
                .bind(tag)
                .execute(&mut *txn)
                .await?;
        }
        txn.commit().await?;
        Ok(id)
    }
}

#[derive(Clone, Debug, Serialize)]
#[derive(sqlx::FromRow)]
#[skip_serializing_none]
pub struct BookRow {
    id: i64,
    title: Option<String>,
    tags: Option<String>,
    added: Option<String>,
    published: Option<String>,
    last_read: Option<String>,
}

impl Books {
    pub async fn fetch_dt(&self, req: dt::Request) -> Result<dt::Response<BookRow>> {
        let mut columns: Vec<&str> = vec![];
        let mut filters: Vec<&str> = vec![];
        let mut binds = vec![];
        let mut global_filters = vec![];
        for col in req.columns.iter() {
            match col.data.as_str() {
                "title" => {
                    columns.push(&col.data);
                    if !col.search.value.is_empty() {
                        if col.search.regex {
                            filters.push("title MATCH ?");
                            binds.push(col.search.value.clone());
                        } else {
                            filters.push("title LIKE ?");
                            binds.push(format!("%{}%", col.search.value));
                        }
                    }
                    if !req.search.value.is_empty() && req.search.regex {
                        global_filters.push("title MATCH ?");
                    }
                },
                "tags" => {
                    columns.push("(SELECT COALESCE(STRING_AGG(tag, ', '), '') FROM book_tag WHERE book_id = id) AS tags");
                    if !col.search.value.is_empty() {
                        if col.search.regex {
                            filters.push("EXISTS(SELECT * FROM book_tag WHERE book_id = id AND tag MATCH ?)");
                            binds.push(col.search.value.clone());
                        } else {
                            filters.push("EXISTS(SELECT * FROM book_tag WHERE book_id = id AND tag LIKE ?)");
                            binds.push(format!("%{}%", col.search.value));
                        }
                    }
                    if !req.search.value.is_empty() && req.search.regex {
                        global_filters.push("EXISTS(SELECT * FROM book_tag WHERE book_id = id AND tag MATCH ?)");
                    }
                },
                "added" => {
                    columns.push("added");
                },
                "published" => {
                    columns.push("published");
                },
                "last_read" => {
                    columns.push("last_read");
                },
                name => bad_req(format!("invalid column: {name}").as_str())?,
            }
        }

        if !req.search.value.is_empty() && !req.search.regex {
            global_filters.push("id IN (SELECT rowid FROM book_fts WHERE book_fts MATCH ?)");
        }

        let global_filter = format!("({})", global_filters.join(" OR "));
        if !global_filters.is_empty() {
            filters.push(&global_filter);
            for _ in global_filters {
                binds.push(req.search.value.clone());
            }
        }

        let mut orders = vec![];
        for order in req.order.unwrap_or_default() {
            orders.push(match order.dir {
                dt::Dir::Asc => format!("{} ASC", order.column + 2),
                dt::Dir::Desc => format!("{} DESC", order.column + 2),
            })
        }

        let mut clauses: Vec<String> = vec!["SELECT id,".to_string()];
        clauses.push(columns.join(", "));
        clauses.push("FROM book".to_string());
        if !filters.is_empty() {
            clauses.push("WHERE".to_string());
            clauses.push(filters.join(" AND "));
        }
        if !orders.is_empty() {
            clauses.push("ORDER BY".to_string());
            clauses.push(orders.join(", "));
        }

        if req.length >= 0 {
            clauses.push(format!("LIMIT {}", req.length));
        }
        if req.start > 0 {
            clauses.push(format!("OFFSET {}", req.start));
        }

        let sql = clauses.join(" ");

        // dbg!(&sql);

        let mut query = sqlx::query_as(sql.as_str());
        for bind in binds.iter() {
            query = query.bind(bind);
        }
        let data: Vec<BookRow> = query.fetch_all(&self.db).await?;

        let (records_total,): (u64,) = sqlx::query_as("SELECT COUNT(*) FROM book").fetch_one(&self.db).await?;
        let records_total = records_total as usize;

        let records_filtered = if filters.is_empty() {
            records_total
        } else {
            let sql = format!(
                "SELECT COUNT(*) FROM book WHERE {}",
                filters.join(" AND "));
            let mut query = sqlx::query_as(&sql);
            for bind in binds {
                query = query.bind(bind);
            }
            let (count,): (u64,) = query.fetch_one(&self.db).await?;
            count as usize
        };

        Ok(dt::Response{
            draw: req.draw,
            records_total,
            records_filtered,
            data,
            error: None,
        })
    }
}
