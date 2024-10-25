use std::{collections::{HashMap, VecDeque}, path::PathBuf, sync::Arc};

use anyhow::anyhow;
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serde_repr::{Deserialize_repr, Serialize_repr};
use serde_with::skip_serializing_none;
use sqlx::{Pool, Sqlite};
use tokio::sync::RwLock;

use crate::{bad_req, check, dt, must, not_found, Result};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Ord, PartialOrd, Serialize_repr, Deserialize_repr, sqlx::Type)]
#[repr(u8)]
pub enum WordStatus {
    #[default]
    Unknown = 0,
    New = 1,
    Level2 = 2,
    Level3 = 3,
    Level4 = 4,
    Level5 = 5,
    Ignored = 98,
    WellKnown = 99,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[skip_serializing_none]
pub struct Word {
    pub id: Option<i64>,
    pub text: String,
    pub status: Option<WordStatus>,
    pub pronunciation: Option<String>,
    pub translation: String,
    pub tags: Vec<String>,
    pub parents: Vec<String>,
    pub image_file: Option<String>,

    #[serde(skip_deserializing)]
    pub resolved_status: Option<(WordStatus, WordStatus)>,
    #[serde(skip_deserializing)]
    pub inherit: bool,
    #[serde(skip_deserializing)]
    pub debug: Option<String>,
}

pub const EMPTY_WORD: Word = Word {
    id: None,
    text: String::new(),
    status: None,
    inherit: false,
    pronunciation: None,
    translation: String::new(),
    tags: vec![],
    parents: vec![],
    image_file: None,
    debug: None,
    resolved_status: None,
};

#[derive(Clone)]
pub struct Dictionary {
    word_images_path: PathBuf,
    db: Pool<Sqlite>,
    cache: Arc<RwLock<Cache>>,
}

#[derive(Debug, Default)]
struct WordIndex {
    surely_exists: bool,
    complete: bool,
    word_ids: Vec<i64>,
}

struct Cache {
    words: HashMap<i64, Word>,
    text_index: HashMap<String, WordIndex>,
    text_index_has_all_words: bool,
}

impl Cache {
    fn new() -> Self {
        Self {
            words: HashMap::new(),
            text_index: HashMap::new(),
            text_index_has_all_words: false,
        }
    }

    fn exists_by_text(&self, text: &str) -> Option<bool> {
        let Some(index) = self.text_index.get(text) else {
            if self.text_index_has_all_words {
                return Some(false)
            } else {
                return None;
            }
        };
        if index.surely_exists || !index.word_ids.is_empty() {
            Some(true)
        } else if index.complete {
            Some(false)
        } else {
            None
        }
    }

    fn find_word_by_id(&self, id: i64) -> Option<Word> {
        self.words.get(&id).cloned()
    }

    fn find_all_words_by_text(&self, text: &str) -> Option<Vec<Word>> {
        let Some(index) = self.text_index.get(text) else {
            if self.text_index_has_all_words {
                return Some(vec![]);
            } else {
                return None;
            }
        };
        if !index.complete {
            return None;
        }
        Some(index.word_ids.iter().flat_map(|id| self.words.get(id)).cloned().collect_vec())
    }

    fn insert_word(&mut self, word: &Word) {
        let Some(id) = word.id else {
            return;
        };
        if let Some(existing_word) = self.words.get_mut(&id) {
            if word.text == existing_word.text {
                *existing_word = word.clone();
                return;
            }
            self.invalidate_by_id(id);
        }
        self.words.insert(id, word.clone());
        let index = self.text_index.entry(word.text.clone()).or_default();
        if !index.word_ids.contains(&id) {
            index.word_ids.push(id);
            index.surely_exists = true;
        }
    }

    fn set_text_index_complete(&mut self, text: &str) {
        self.text_index.entry(text.to_string()).or_default().complete = true;
    }

    fn set_text_index_has_all_words(&mut self) {
        self.text_index_has_all_words = true;
    }

    fn set_word_exists(&mut self, text: &str, exists: bool) {
        if exists {
            self.text_index.entry(text.to_string()).or_default().surely_exists = true;
        } else {
            let index = self.text_index.entry(text.to_string()).or_default();
            for id in index.word_ids.iter() {
                self.words.remove(id);
            }
            index.word_ids.clear();
            index.surely_exists = false;
            index.complete = true;
        }
    }

    fn invalidate_by_id(&mut self, word_id: i64) {
        let Some(word) = self.words.remove(&word_id) else {
            return;
        };
        if let Some(index) = self.text_index.get_mut(&word.text) {
            index.complete = false;
            index.surely_exists = false;
            index.word_ids.retain(|id| *id != word_id);
        }
    }

    fn invalidate_text_index_complete(&mut self, text: &str) {
        if let Some(index) = self.text_index.get_mut(text) {
            index.complete = false;
        }
    }

    fn invalidate_text_index_flags(&mut self, text: &str) {
        if let Some(index) = self.text_index.get_mut(text) {
            index.surely_exists = false;
            index.complete = false;
        }
    }
}

#[derive(sqlx::FromRow)]
struct DbWord {
    id: i64,
    text: String,
    status: Option<WordStatus>,
    pronunciation: Option<String>,
    translation: String,
    image_file: Option<String>,
}

impl Dictionary {
    pub fn new(db: Pool<Sqlite>, word_images_path: PathBuf) -> Self {
        Self {
            db, word_images_path,
            cache: Arc::new(RwLock::new(Cache::new())),
        }
    }

    pub async fn prefetch_all(&self) -> Result<()> {
        let mut txn = self.db.begin().await?;
        let word_recs: Vec<DbWord> = sqlx::query_as("SELECT * FROM word")
            .fetch_all(&mut *txn)
            .await?;
        let tag_recs: Vec<(i64, String)> = sqlx::query_as("SELECT word_id, tag FROM word_tag")
            .fetch_all(&mut *txn)
            .await?;
        let parent_recs: Vec<(i64, String)> = sqlx::query_as("SELECT child_word_id, parent_word_text FROM word_parent")
            .fetch_all(&mut *txn)
            .await?;
        let mut words = vec![];
        for wr in word_recs {
            let tags = tag_recs.iter()
                .filter(|(id, _)| *id == wr.id)
                .map(|(_, tag)| tag.clone())
                .collect();
            let parents = parent_recs.iter()
                .filter(|(id, _)| *id == wr.id)
                .map(|(_, p)| p.clone())
                .collect();
            words.push(Word {
                id: Some(wr.id),
                text: wr.text,
                status: wr.status,
                inherit: wr.status.is_none(),
                pronunciation: wr.pronunciation,
                translation: wr.translation,
                image_file: wr.image_file,
                tags,
                parents,
                debug: None,
                resolved_status: wr.status.map(|s| (s, s)),
            })
        }
        let mut cache = self.cache.write().await;
        words.iter().for_each(|word| cache.insert_word(word));
        for text in words.iter().map(|word| &word.text).sorted().dedup() {
            cache.set_text_index_complete(text);
        }
        cache.set_text_index_has_all_words();
        Ok(())
    }

    pub async fn word_exists(&self, word: &str) -> Result<bool> {
        if let Some(exists) = self.cache.read().await.exists_by_text(word) {
            return Ok(exists);
        }
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM word WHERE text = ?")
                       .bind(word)
                       .fetch_one(&self.db)
                       .await?;
        self.cache.write().await.set_word_exists(word, count > 0);
        Ok(count > 0)
    }

    pub async fn find_words_by_text(&self, text: &str) -> Result<Vec<Word>> {
        if let Some(words) = self.cache.read().await.find_all_words_by_text(text) {
            return Ok(words);
        }
        let mut txn = self.db.begin().await?;
        let word_recs: Vec<DbWord> = sqlx::query_as("SELECT * FROM word WHERE text = ?")
            .bind(text)
            .fetch_all(&mut *txn)
            .await?;
        let tag_recs: Vec<(i64, String)> = sqlx::query_as("
            SELECT word_id, tag
            FROM word_tag INNER JOIN word ON word_id = id
            WHERE text = ?
            ")
            .bind(text)
            .fetch_all(&mut *txn)
            .await?;
        let parent_recs: Vec<(i64, String)> = sqlx::query_as("
            SELECT child_word_id, parent_word_text
            FROM word_parent INNER JOIN word ON child_word_id = id
            WHERE text = ?
            ")
            .bind(text)
            .fetch_all(&mut *txn)
            .await?;
        let mut words = vec![];
        for wr in word_recs {
            let tags = tag_recs.iter()
                .filter(|(id, _)| *id == wr.id)
                .map(|(_, tag)| tag.clone())
                .collect();
            let parents = parent_recs.iter()
                .filter(|(id, _)| *id == wr.id)
                .map(|(_, p)| p.clone())
                .collect();
            words.push(Word {
                id: Some(wr.id),
                text: wr.text,
                status: wr.status,
                inherit: wr.status.is_none(),
                pronunciation: wr.pronunciation,
                translation: wr.translation,
                image_file: wr.image_file,
                tags,
                parents,
                debug: None,
                resolved_status: wr.status.map(|s| (s, s)),
            })
        }
        let mut cache = self.cache.write().await;
        words.iter().for_each(|word| cache.insert_word(word));
        cache.set_text_index_complete(text);
        Ok(words)
    }

    pub async fn find_word_by_id(&self, id: i64) -> Result<Option<Word>> {
        if let Some(word) = self.cache.read().await.find_word_by_id(id) {
            return Ok(Some(word));
        }
        let mut txn = self.db.begin().await?;
        let word_rec: Option<DbWord> = sqlx::query_as("SELECT * FROM word WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *txn)
            .await?;
        let Some(word_rec) = word_rec else {
            return Ok(None);
        };
        let tags: Vec<(String,)> = sqlx::query_as("
            SELECT tag
            FROM word_tag INNER JOIN word ON word_id = id
            WHERE id = ?
            ")
            .bind(id)
            .fetch_all(&mut *txn)
            .await?;
        let tags = tags.into_iter().map(|(s,)| s).collect();
        let parents: Vec<(String,)> = sqlx::query_as("
            SELECT parent_word_text
            FROM word_parent INNER JOIN word ON child_word_id = id
            WHERE child_word_id = ?
            ")
            .bind(id)
            .fetch_all(&mut *txn)
            .await?;
        let parents = parents.into_iter().map(|(s,)| s).collect();
        let word = Word {
            id: Some(word_rec.id),
            text: word_rec.text,
            status: word_rec.status,
            inherit: word_rec.status.is_none(),
            pronunciation: word_rec.pronunciation,
            translation: word_rec.translation,
            image_file: word_rec.image_file,
            tags,
            parents,
            debug: None,
            resolved_status: word_rec.status.map(|s| (s, s)),
        };
        self.cache.write().await.insert_word(&word);
        Ok(Some(word))
    }

    pub async fn find_word_trees_by_text<I>(&self, texts: I) -> Result<HashMap<String, Vec<Word>>>
    where
        I: IntoIterator<Item = String>,
    {
        let mut result = HashMap::new();
        let mut q = VecDeque::from_iter(texts);

        while let Some(text) = q.pop_front() {
            if result.contains_key(&text) {
                continue;
            }
            let words = self.find_words_by_text(text.as_ref()).await?;
            for word in words.iter() {
                for parent in word.parents.iter() {
                    if !result.contains_key(parent) {
                        q.push_back(parent.clone());
                    }
                }
            }
            result.insert(text, words);
        }

        Ok(result)
    }

    pub async fn all_tags(&self) -> Result<Vec<String>> {
        let mut all_tags: Vec<String> = sqlx::query_as("SELECT DISTINCT tag FROM word_tag")
            .fetch_all(&self.db)
            .await?
            .into_iter()
            .map(|(s,)| s)
            .collect();
        all_tags.sort();
        Ok(all_tags)
    }

    pub async fn get_word_image_file(&self, word_id: i64) -> Result<Option<PathBuf>> {
        let image_file = if let Some(word) = self.cache.read().await.find_word_by_id(word_id) {
            word.image_file
        } else {
            let (image_file,): (Option<String>,) = sqlx::query_as("SELECT image_file FROM word WHERE id = ?")
                                .bind(word_id)
                                .fetch_one(&self.db)
                                .await?;
            image_file
        };
        let image_path = image_file.map(|f| self.word_images_path.join(f));
        Ok(image_path)
    }

    pub async fn set_word_image_file(&self, word_id: i64, image_file: &str) -> Result<()> {
        self.cache.write().await.invalidate_by_id(word_id);
        sqlx::query("UPDATE word SET image_file = ? WHERE id = ?")
            .bind(image_file)
            .bind(word_id)
            .execute(&self.db)
            .await?;
        self.cache.write().await.invalidate_by_id(word_id);
        Ok(())
    }

    pub async fn delete_word_image_file(&self, word_id: i64) -> Result<()> {
        self.cache.write().await.invalidate_by_id(word_id);
        sqlx::query("UPDATE word SET image_file = NULL WHERE id = ?")
            .bind(word_id)
            .execute(&self.db)
            .await?;
        self.cache.write().await.invalidate_by_id(word_id);
        Ok(())
    }

    pub async fn delete_word(&self, id: i64) -> Result<()> {
        self.cache.write().await.invalidate_by_id(id);
        let mut txn = self.db.begin().await?;

        let text: Option<(String,)> = sqlx::query_as("SELECT text FROM word WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *txn)
            .await?;
        let Some(text) = text.map(|(s,)| s) else {
            return not_found();
        };
        self.cache.write().await.invalidate_text_index_flags(&text);

        let (siblings,): (i32,) = sqlx::query_as("
            SELECT COUNT(*)
            FROM word a INNER JOIN word b ON a.text = b.text
            WHERE a.id = ?
            ")
            .bind(id)
            .fetch_one(&mut *txn)
            .await?;
        if siblings == 1 {
            let (children,): (i32,) = sqlx::query_as("
                SELECT COUNT(*)
                FROM word_parent wp INNER JOIN word w ON wp.parent_word_text = w.text
                WHERE w.id = ?
                ")
                .bind(id)
                .fetch_one(&mut *txn)
                .await?;
            check(children == 0, "word has children")?;
        }

        let (image_file,): (Option<String>,) = must(sqlx::query_as("SELECT image_file FROM word WHERE id = ?")
            .bind(id)
            .fetch_optional(&mut *txn)
            .await?)?;

        sqlx::query("DELETE FROM word_parent WHERE child_word_id = ?")
            .bind(id)
            .execute(&mut *txn)
            .await?;
        sqlx::query("DELETE FROM word_tag WHERE word_id = ?")
            .bind(id)
            .execute(&mut *txn)
            .await?;
        sqlx::query("DELETE FROM word WHERE id = ?")
            .bind(id)
            .execute(&mut *txn)
            .await?;

        txn.commit().await?;
        self.cache.write().await.invalidate_by_id(id);
        self.cache.write().await.invalidate_text_index_flags(&text);

        if let Some(image_file) = image_file {
            let image_path = self.word_images_path.join(image_file);
            if tokio::fs::try_exists(&image_path).await? {
                tokio::fs::remove_file(image_path).await?;
            }
        }

        Ok(())
    }

    pub async fn insert_or_update_word(&self, word: &Word) -> Result<i64> {
        if let Some(id) = word.id {
            self.update_word(id, word).await?;
            Ok(id)
        } else {
            self.insert_word(word).await
        }
    }

    async fn insert_word(&self, word: &Word) -> Result<i64> {
        self.cache.write().await.invalidate_text_index_complete(&word.text);
        let mut txn = self.db.begin().await?;

        let result = sqlx::query("
            INSERT INTO word (text, pronunciation, translation, status)
            VALUES (?, ?, ?, ?)
            ")
            .bind(&word.text)
            .bind(word.pronunciation.as_ref().filter(|s| !s.is_empty()))
            .bind(&word.translation)
            .bind(word.status)
            .execute(&mut *txn)
            .await?;
        let id = result.last_insert_rowid();

        for tag in word.tags.iter() {
            sqlx::query("INSERT INTO word_tag (word_id, tag) VALUES (?, ?)")
                .bind(id)
                .bind(tag)
                .execute(&mut *txn)
                .await?;
            }

        for parent in word.parents.iter() {
            sqlx::query("INSERT INTO word_parent (child_word_id, parent_word_text) VALUES (?, ?)")
                .bind(id)
                .bind(parent)
                .execute(&mut *txn)
                .await?;
            }

        let (count,): (i32,) = sqlx::query_as("
            SELECT COUNT(*)
            FROM word_parent wp LEFT JOIN word w ON wp.parent_word_text = w.text
            WHERE wp.child_word_id = ? AND w.id IS NULL
            ")
            .bind(id)
            .fetch_one(&mut *txn)
            .await?;
        check(count == 0, "missing parents")?;

        txn.commit().await?;
        self.cache.write().await.invalidate_text_index_complete(&word.text);
        self.cache.write().await.set_word_exists(&word.text, true);
        Ok(id)
    }

    async fn update_word(&self, id: i64, word: &Word) -> Result<()> {
        self.cache.write().await.invalidate_by_id(id);
        let mut txn = self.db.begin().await?;

        let result = sqlx::query("
            UPDATE word
            SET text = ?, status = ?, pronunciation = ?, translation = ?
            WHERE id = ?
            ")
            .bind(&word.text)
            .bind(word.status)
            .bind(word.pronunciation.as_ref().filter(|s| !s.is_empty()))
            .bind(&word.translation)
            .bind(id)
            .execute(&mut *txn)
            .await?;
        if result.rows_affected() != 1 {
            not_found()?;
        }

        sqlx::query("DELETE FROM word_tag WHERE word_id = ?")
            .bind(id)
            .execute(&mut *txn)
            .await?;
        for tag in word.tags.iter() {
            sqlx::query("INSERT INTO word_tag (word_id, tag) VALUES (?, ?)")
                .bind(id)
                .bind(tag)
                .execute(&mut *txn)
                .await?;
        }

        sqlx::query("DELETE FROM word_parent WHERE child_word_id = ?")
            .bind(id)
            .execute(&mut *txn)
            .await?;
        for parent in word.parents.iter() {
            let (count,): (i32,) = sqlx::query_as("SELECT COUNT(*) FROM word WHERE text = ?")
                           .bind(parent)
                           .fetch_one(&mut *txn)
                           .await?;
            check(count != 0, "missing parent")?;
            sqlx::query("INSERT INTO word_parent (child_word_id, parent_word_text) VALUES (?, ?)")
                .bind(id)
                .bind(parent)
                .execute(&mut *txn)
                .await?;
        }

        txn.commit().await?;
        self.cache.write().await.invalidate_by_id(id);
        self.cache.write().await.set_word_exists(&word.text, true);
        Ok(())
    }
}

fn fold_status_range_possibilities(x: (WordStatus, WordStatus), y: (WordStatus, WordStatus)) -> Result<(WordStatus, WordStatus)> {
    use WordStatus::*;
    Ok(match (x, y) {
        ((Unknown, Unknown), y) => y,
        (x, (Unknown, Unknown)) => x,
        ((Ignored, Ignored), y) => y,
        (x, (Ignored, Ignored)) => x,
        ((Ignored, _), _) | ((_, Ignored), _) => Err(anyhow!("invalid word status ranges: {x:?}"))?,
        (_, (Ignored, _)) | (_, (_, Ignored)) => Err(anyhow!("invalid word status ranges: {y:?}"))?,
        ((a, b), (c, d)) => (a.min(c), b.max(d)),
    })
}

fn fold_status_range_parents(x: (WordStatus, WordStatus), y: (WordStatus, WordStatus)) -> Result<(WordStatus, WordStatus)> {
    use WordStatus::*;
    Ok(match (x, y) {
        ((Ignored, Ignored), _) => (Ignored, Ignored),
        (_, (Ignored, Ignored)) => (Ignored, Ignored),
        ((Ignored, _), _) | ((_, Ignored), _) => Err(anyhow!("invalid word status ranges: {x:?}"))?,
        (_, (Ignored, _)) | (_, (_, Ignored)) => Err(anyhow!("invalid word status ranges: {y:?}"))?,
        ((a, b), (c, d)) => (a.min(c), b.min(d)),
    })
}


impl Dictionary {
    pub async fn resolve_status_with_eval(&self, word: &Word, eval: &(dyn Fn(&Word) -> Option<WordStatus> + Sync)) -> Result<(WordStatus, WordStatus)> {
        if let Some(status) = eval(word) {
            return Ok((status, status));
        };
        let mut status_range = (WordStatus::WellKnown, WordStatus::WellKnown);
        for parent in word.parents.iter() {
            let parent_words = self.find_words_by_text(parent).await?;
            let parent_status_range = Box::pin(self.resolve_stati(parent_words.iter())).await?;
            status_range = fold_status_range_parents(status_range, parent_status_range)?;
        }
        Ok(status_range)
    }

    pub async fn resolve_status(&self, word: &Word) -> Result<(WordStatus, WordStatus)> {
        self.resolve_status_with_eval(word, &|w| w.status).await
    }

    pub async fn resolve_stati_with_eval(&self, words: impl Iterator<Item=&Word>, eval: &(dyn Fn(&Word) -> Option<WordStatus> + Sync)) -> Result<(WordStatus, WordStatus)> {
        let mut status_range = (WordStatus::Unknown, WordStatus::Unknown);
        for word in words {
            let word_status_range = self.resolve_status_with_eval(word, eval).await?;
            status_range = fold_status_range_possibilities(status_range, word_status_range)?;
        }
        Ok(status_range)
    }

    pub async fn resolve_stati(&self, words: impl Iterator<Item=&Word>) -> Result<(WordStatus, WordStatus)> {
        self.resolve_stati_with_eval(words, &|w| w.status).await
    }
}

#[derive(Clone, Debug, Serialize, sqlx::FromRow)]
pub struct WordSuggestion {
    value: String,
    translation: String,
}

impl Dictionary {
    pub async fn suggest(&self, q: &str) -> Result<Vec<WordSuggestion>> {
        let q = q.trim();
        if q.is_empty() {
            return Ok(vec![]);
        }
        let mut results: Vec<WordSuggestion> = sqlx::query_as("
            SELECT
                text AS value,
                STRING_AGG(
                    CASE INSTR(translation, CHAR(10) || CHAR(10))
                        WHEN 0 THEN translation
                        ELSE SUBSTR(translation, 1, INSTR(translation, CHAR(10) || CHAR(10)) - 1)
                    END, '; ') AS translation
            FROM word
            WHERE text LIKE ?
            GROUP BY 1
            ORDER BY LENGTH(text) ASC, text ASC
            LIMIT 20
        ")
            .bind(format!("{}%", q))
            .fetch_all(&self.db)
            .await?;
        results.sort_by_key(|ws| ws.value.to_string());
        Ok(results)
    }

}

#[derive(Clone, Debug, Serialize)]
#[derive(sqlx::FromRow)]
#[skip_serializing_none]
pub struct WordRow {
    id: i64,
    text: Option<String>,
    parents: Option<String>,
    translation: Option<String>,
    tags: Option<String>,
    status: Option<WordStatus>,
    added: Option<String>,
}

impl Dictionary {
    pub async fn fetch_dt(&self, req: dt::Request) -> Result<dt::Response<WordRow>> {
        let mut columns: Vec<&str> = vec![];
        let mut filters: Vec<&str> = vec![];
        let mut binds = vec![];
        let mut global_filters = vec![];
        for col in req.columns.iter() {
            match col.data.as_str() {
                "text" => {
                    columns.push(&col.data);
                    if !col.search.value.is_empty() {
                        if col.search.regex {
                            filters.push("text MATCH ?");
                            binds.push(col.search.value.clone());
                        } else {
                            filters.push("text LIKE ?");
                            binds.push(format!("%{}%", col.search.value));
                        }
                    }
                    if !req.search.value.is_empty() && req.search.regex {
                        global_filters.push("text MATCH ?");
                    }
                },
                "translation" => {
                    columns.push(&col.data);
                    if !col.search.value.is_empty() {
                        if col.search.regex {
                            filters.push("translation MATCH ?");
                            binds.push(col.search.value.clone());
                        } else {
                            filters.push("translation LIKE ?");
                            binds.push(format!("%{}%", col.search.value));
                        }
                    }
                    if !req.search.value.is_empty() && req.search.regex {
                        global_filters.push("translation MATCH ?");
                    }
                },
                "tags" => {
                    columns.push("(SELECT COALESCE(STRING_AGG(tag, ', '), '') FROM word_tag WHERE word_id = id) AS tags");
                    if !col.search.value.is_empty() {
                        if col.search.regex {
                            filters.push("EXISTS(SELECT * FROM word_tag WHERE word_id = id AND tag MATCH ?)");
                            binds.push(col.search.value.clone());
                        } else {
                            filters.push("EXISTS(SELECT * FROM word_tag WHERE word_id = id AND tag LIKE ?)");
                            binds.push(format!("%{}%", col.search.value));
                        }
                    }
                    if !req.search.value.is_empty() && req.search.regex {
                        global_filters.push("EXISTS(SELECT * FROM word_tag WHERE word_id = id AND tag MATCH ?)");
                    }
                },
                "parents" => {
                    columns.push("(SELECT COALESCE(STRING_AGG(parent_word_text, ', '), '') FROM word_parent WHERE child_word_id = id) AS parents");
                    if !col.search.value.is_empty() {
                        if col.search.regex {
                            filters.push("EXISTS(SELECT * FROM word_parent WHERE child_word_id = id AND parent_word_text MATCH ?)");
                            binds.push(col.search.value.clone());
                        } else {
                            filters.push("EXISTS(SELECT * FROM word_parent WHERE child_word_id = id AND parent_word_text LIKE ?)");
                            binds.push(format!("%{}%", col.search.value));
                        }
                    }
                    if !req.search.value.is_empty() && req.search.regex {
                        global_filters.push("EXISTS(SELECT * FROM word_parent WHERE child_word_id = id AND parent_word_text MATCH ?)")
                    }
                },
                "status" => {
                    columns.push("status");
                },
                "added" => {
                    columns.push("added");
                }
                name => bad_req(format!("invalid column: {name}").as_str())?,
            }
        }

        if !req.search.value.is_empty() && !req.search.regex {
            global_filters.push("id IN (SELECT rowid FROM word_fts WHERE word_fts MATCH ?)");
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
        clauses.push("FROM word".to_string());
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
        let data: Vec<WordRow> = query.fetch_all(&self.db).await?;

        let (records_total,): (u64,) = sqlx::query_as("SELECT COUNT(*) FROM word").fetch_one(&self.db).await?;
        let records_total = records_total as usize;

        let records_filtered = if filters.is_empty() {
            records_total
        } else {
            let sql = format!(
                "SELECT COUNT(*) FROM word WHERE {}",
                filters.join(" AND "));
            // dbg!(&sql);
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
