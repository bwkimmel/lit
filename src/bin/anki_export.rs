use std::{collections::HashMap, path::{Path, PathBuf}};

use anyhow::{anyhow, Ok, Result};
use clap::Parser;
use figment::{providers::{Format, Toml}, value::magic::RelativePathBuf, Figment};
use genanki_rs::{Deck, Field, Model, Note, Package, Template};
use image::ImageReader;
use indicatif::ProgressIterator;
use itertools::Itertools;
use lit::{config::DatabaseConfig, dict::{Dictionary, Word, WordStatus}};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::sqlite::SqlitePoolOptions;
use tempfile::{tempdir, TempDir};
use tokio::{fs::File, io::{AsyncBufReadExt, BufReader}};

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(long, help="Path to Anki deck config")]
    deck_config: String,

    #[arg(help="file to write to")]
    output: String,

    #[arg(long, help="Print words that would be included if rating was one-level higher")]
    show_close_words: bool

    // #[arg(short, long, help="path to configuration file")]
    // config: String,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct ImageConfig {
    max_width: Option<u32>,
    max_height: Option<u32>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct AnkiConfig {
    hidden_tags: Vec<String>,
    exclude_tags: Vec<String>,
    include_tags: Vec<String>,
    min_status: WordStatus,
    min_status_override: HashMap<String, WordStatus>,
    guids_export: Option<RelativePathBuf>,
    model: ModelConfig,
    deck: DeckConfig,
    database: DatabaseConfig,
    userdata: RelativePathBuf,
    images: Option<ImageConfig>,
}

impl AnkiConfig {
    pub fn userdata(&self) -> PathBuf {
        self.userdata.relative()
    }

    pub fn word_images_path(&self) -> PathBuf {
        self.userdata().join("word_images")
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DeckConfig {
    id: i64,
    name: String,
    description: String,
}

impl DeckConfig {
    fn build(self) -> Deck {
        Deck::new(self.id, &self.name, &self.description)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ModelConfig {
    id: i64,
    name: String,
    fields: Vec<FieldConfig>,
    css: Option<String>,
    templates: Vec<TemplateConfig>,
}

impl ModelConfig {
    fn build(self) -> Model {
        let fields = self.fields.into_iter().map(|cfg| cfg.build()).collect();
        let templates = self.templates.into_iter().map(|cfg| cfg.build()).collect();
        let mut model = Model::new(self.id, &self.name, fields, templates);
        if let Some(css) = self.css {
            model = model.css(&css);
        }
        model
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FieldConfig {
    name: String,
}

impl FieldConfig {
    fn build(self) -> Field {
        Field::new(&self.name)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct TemplateConfig {
    name: String,
    qfmt: String,
    afmt: String,
}

impl TemplateConfig {
    fn build(self) -> Template {
        Template::new(&self.name)
            .qfmt(&self.qfmt)
            .afmt(&self.afmt)
    }
}

async fn read_guids(path: impl AsRef<Path> + Clone) -> Result<HashMap<String, String>> {
    let mut guid_column = None;
    let mut columns = vec![];
    let f = File::open(path.clone()).await?;
    let r = BufReader::new(f);
    let mut lines = r.lines();
    while let Some(line) = lines.next_line().await? {
        let Some(comment) = line.strip_prefix("#") else {
            break;
        };
        let Some((key, value)) = comment.split_once(':') else {
            continue;
        };
        let key = key.trim().to_lowercase();
        if key.ends_with(" column") {
            let column: usize = value.trim().parse()?;
            let Some(column) = column.checked_sub(1) else {
                return Err(anyhow!("invalid {key}: {value}"));
            };
            columns.push(column);
            if key == "guid column" {
                guid_column = Some(column);
            }
        }
    }
    drop(lines);
    let Some(guid_column) = guid_column else {
        return Ok(HashMap::new());
    };
    let front_column = (0..).find(|i| !columns.contains(i)).unwrap();

    let mut csv_reader = csv::ReaderBuilder::new()
        .flexible(true)
        .delimiter(b'\t')
        .comment(Some(b'#'))
        .from_path(path)?;

    let mut guids = HashMap::new();
    for record in csv_reader.records() {
        let record = record?;
        let Some(front) = record.get(front_column) else {
            continue;
        };
        let Some(guid) = record.get(guid_column) else {
            continue;
        };
        guids.insert(front.to_string(), guid.to_string());
    }

    Ok(guids)
}

fn sanitize_tag(tag: &str) -> String {
    tag.replace(" ", "_")
}

fn has_tag(word: &Word, tag: &str) -> bool {
    word.tags.iter().any(|t| t == tag)
}

fn has_any_tags(word: &Word, tags: &[String]) -> bool {
    word.tags.iter().any(|t| tags.contains(t))
}

enum Inclusion {
    Included,
    Excluded,
    StatusClose,
    StatusTooLow,
}

fn is_status_close(status: WordStatus, min_status: WordStatus) -> bool {
    use WordStatus::*;
    matches!((status, min_status),
        (Level5, WellKnown)
        | (Level4, Level5)
        | (Level3, Level4)
        | (Level2, Level3)
        | (New, Level2))
}

async fn include_word(word: &Word, ctx: &Context) -> Result<Inclusion> {
    use Inclusion::*;
    if word.id.is_none() {
        return Ok(Excluded);
    }
    if word.translation.is_empty() {
        return Ok(Excluded);
    }
    if has_tag(word, "noanki") {
        return Ok(Excluded);
    }
    let has_include_tags = has_any_tags(word, &ctx.config.include_tags);
    let has_exclude_tags = has_any_tags(word, &ctx.config.exclude_tags);
    if !has_include_tags && has_exclude_tags {
        return Ok(Excluded);
    }
    if !has_include_tags && !word.parents.is_empty() && !word.parents.iter().any(|p| p == "root") {
        return Ok(Excluded);
    }
    let (_, status) = ctx.dict.resolve_status(word).await
        .map_err(|e| anyhow!("{e}"))?;
    if status == WordStatus::Ignored {
        return Ok(Excluded);
    }
    let mut min_status = ctx.config.min_status;
    for tag in word.tags.iter() {
        if let Some(ovr) = ctx.config.min_status_override.get(tag).copied() {
            if ovr < min_status {
                min_status = ovr;
            }
        }
    }
    if status < min_status {
        if is_status_close(status, min_status) {
            return Ok(StatusClose);
        }
        return Ok(StatusTooLow);
    }
    Ok(Included)
}

struct Context {
    config: AnkiConfig,
    model: Model,
    dict: Dictionary,
    guids: HashMap<String, String>,
    tempdir: TempDir,
}

fn guid_for(text: &str) -> String {
    static BASE91_TABLE: [char; 91] = [
       'a','b','c','d','e','f','g','h','i','j','k','l','m','n','o','p','q','r','s',
       't','u','v','w','x','y','z','A','B','C','D','E','F','G','H','I','J','K','L',
       'M','N','O','P','Q','R','S','T','U','V','W','X','Y','Z','0','1','2','3','4',
       '5','6','7','8','9','!','#','$','%','&','(',')','*','+',',','-','.','/',':',
       ';','<','=','>','?','@','[',']','^','_','`','{','|','}','~',
    ];
    let mut h = Sha256::new();
    h.update(text.bytes().collect_vec()); // TODO: is this UTF-8?
    let mut hash_int: usize = 0;
    for b in h.finalize().into_iter().take(8) {
        hash_int <<= 8;
        hash_int += b as usize;
    }

    let mut rv = vec![];
    while hash_int > 0 {
        rv.push(BASE91_TABLE[hash_int % BASE91_TABLE.len()]);
        hash_int /= BASE91_TABLE.len();
    }
    rv.reverse();
    rv.into_iter().collect()
}

async fn word_image(ctx: &Context, word_id: i64, media_files: &mut Vec<String>) -> Result<Option<String>> {
    let Some(mut image_path) = ctx.dict.get_word_image_file(word_id).await.map_err(|e| anyhow!("{e}"))? else {
        return Ok(None);
    };
    let Some(filename) = image_path.file_name() else {
        return Err(anyhow!("invalid image path for word {word_id}: {}", image_path.to_string_lossy()));
    };
    let Some(filename_str) = filename.to_str() else {
        return Err(anyhow!("invalid image filename for word {word_id}: {}", image_path.to_string_lossy()));
    };
    let filename_str = filename_str.to_string();
    if let Some(config) = &ctx.config.images {
        let images_dir = ctx.tempdir.path().join("images");
        tokio::fs::create_dir_all(&images_dir).await?;
        let img = ImageReader::open(&image_path)?.decode()?;
        let w = config.max_width.unwrap_or(img.width()).min(img.width());
        let h = config.max_height.unwrap_or(img.height()).min(img.height());
        let img = img.resize(w, h, image::imageops::FilterType::Lanczos3);
        image_path = images_dir.join(filename);
        img.save(&image_path)?;
    }
    let Some(image_path_str) = image_path.to_str() else {
        return Err(anyhow!("invalid image filename for word {word_id}: {}", image_path.to_string_lossy()));
    };
    media_files.push(image_path_str.to_string());
    Ok(Some(filename_str))
}

async fn words_to_note(words: &[Word], ctx: &Context, media_files: &mut Vec<String>, close_words_list: &mut Vec<Word>) -> Result<Option<Note>> {
    let mut words_vec = vec![];
    let mut extra_words = vec![];
    let mut close_words = vec![];
    for word in words {
        match include_word(word, ctx).await? {
            Inclusion::Included => words_vec.push(word),
            Inclusion::StatusTooLow => extra_words.push(word),
            Inclusion::StatusClose => {
                close_words.push(word);
                extra_words.push(word);
            },
            Inclusion::Excluded => (),
        }
    }
    let num_scored = words_vec.len();
    if num_scored == 0 {
        close_words_list.extend(close_words.into_iter().cloned());
        return Ok(None);
    }

    words_vec.extend(extra_words.into_iter());
    let words = words_vec;

    let text = match words.iter().map(|w| &w.text).all_equal_value() {
        std::result::Result::Ok(x) => x,
        Err(None) => return Ok(None),
        Err(Some((a, b))) => Err(anyhow!("words have mismatched texts: {a}, {b}"))?,
    };
    let pronunciation = words.iter()
        .map(|w| w.pronunciation.as_ref())
        .all_equal_value().ok()
        .flatten()
        .cloned()
        .unwrap_or_default();
    let tags = words.iter()
        .flat_map(|w| w.tags.iter())
        .filter(|t| !ctx.config.hidden_tags.contains(t))
        .unique()
        .cloned()
        .collect_vec();
    let common_tags = words.iter()
        .map(|w| w.tags.clone())
        .reduce(|a, b| a.into_iter().filter(|x| b.contains(x)).collect_vec())
        .unwrap_or_default()
        .into_iter()
        .filter(|t| !ctx.config.hidden_tags.contains(t))
        .collect_vec();

    let translation_markdown = if words.len() == 1 {
        words[0].translation.clone()
    } else {
        let mut tr = String::new();
        for (i, word) in words.iter().enumerate() {
            if i > 0 {
                tr += "\n";
            }
            if i == num_scored {
                tr += "\n#### Additional meanings:\n\n";
            }
            tr += &format!("{}. ", i + 1);
            let word_tags = word.tags.iter()
                .filter(|t| tags.contains(t))
                .filter(|t| !common_tags.contains(t))
                .cloned().collect_vec();
            if !word_tags.is_empty() {
                tr += &format!("[{}] ", word_tags.join(", "));
            }
            tr += &word.translation.replace("\n", "\n   ");
        }
        tr
    };

    let parser = pulldown_cmark::Parser::new(&translation_markdown);
    let mut translation = String::new();
    pulldown_cmark::html::push_html(&mut translation, parser);

    let mut images = String::new();
    for word in words {
        if let Some(path) = word_image(ctx, word.id.unwrap(), media_files).await? {
            images += &format!(r#"<img src="{path}">"#);
        }
    }

    let model = ctx.model.clone();
    let fields = vec![
        text.as_str(),
        "Korean",
        pronunciation.as_str(),
        translation.as_str(),
        images.as_str(),
    ];
    let guid = ctx.guids.get(text).cloned()
        .unwrap_or_else(|| guid_for(text));
    let guid = Some(guid.as_str());
    let mut tags = common_tags.into_iter()
        .filter(|t| !ctx.config.hidden_tags.contains(t))
        .map(|t| sanitize_tag(&t))
        .collect_vec();
    tags.sort();
    tags.dedup();
    let tags = tags.iter().map(|t| t.as_str()).collect_vec();
    let tags = if tags.is_empty() { None } else { Some(tags) };
    let note = Note::new_with_options(model, fields, None, tags, guid)?;
    Ok(Some(note))
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config: AnkiConfig = Figment::from(Toml::file(&args.deck_config)).extract()?;
    let guids = match config.guids_export.clone() {
        Some(f) => read_guids(f.relative()).await?,
        None => HashMap::new(),
    };
    let model = config.model.clone().build();
    let db_path = config.database.path.relative();
    let Some(db) = db_path.to_str() else {
        return Err(anyhow!("invalid database path: {db_path:?}"));
    };
    let pool = SqlitePoolOptions::new()
        .max_connections(config.database.max_connections)
        .connect(db)
        .await?;
    let dict = Dictionary::new(pool.clone(), config.word_images_path());
    let mut deck = config.deck.clone().build();
    let tempdir = tempdir()?;
    let ctx = Context { config, model, dict, guids, tempdir };
    let texts: Vec<(String,)> = sqlx::query_as("SELECT DISTINCT text FROM word ORDER BY 1").fetch_all(&pool).await?;
    let mut media_files = vec![];
    let mut count = 0;
    let mut close_words = vec![];
    for (text,) in texts.into_iter().progress() {
        let words = ctx.dict.find_words_by_text(&text).await.map_err(|e| anyhow!("{e}"))?;
        if let Some(note) = words_to_note(&words, &ctx, &mut media_files, &mut close_words).await? {
            deck.add_note(note);
            count += 1;
        }
    }
    dbg!(count);
    let mut media_files_str = vec![];
    for f in media_files.iter() {
        media_files_str.push(f.as_str());
    }
    let mut pkg = Package::new(vec![deck], media_files_str)?;
    pkg.write_to_file(&args.output)?;
    if args.show_close_words {
        for word in close_words {
            println!("{},{}", word.text, word.tags.join(","));
        }
    }
    Ok(())
}
