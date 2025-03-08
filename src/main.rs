use std::{cmp::Ordering, collections::{BTreeMap, HashMap}, io::Cursor, net::{Ipv4Addr, SocketAddrV4}, str::FromStr, sync::{Arc, LazyLock}};

use anyhow::anyhow;
use axum::{async_trait, body::{Body, Bytes}, extract::{FromRequestParts, Path, Query, State}, http::{header::CONTENT_TYPE, HeaderMap, StatusCode}, response::{Html, IntoResponse, Redirect}, routing::{get, post}, Form, Json, Router};
use axum_extra::{headers::Range, TypedHeader};
use axum_range::{KnownSize, Ranged};
use chrono::{TimeZone, Utc};
use clap::Parser;
use futures::{executor::block_on, lock::Mutex, StreamExt};
use image::{ImageFormat, ImageReader};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use tera::Tera;
use tokio::{fs::File, net::TcpListener};
use tokio_util::io::ReaderStream;
use tower_http::services::ServeDir;

use lit::{bad_req, books::{Book, Books, NewBook}, check, config::{Config, DisplayConfig}, dict::{Dictionary, Word, WordStatus}, doc::{self, markdown::{MarkdownHtmlRenderer, MarkdownParser}, vtt::{Cue, CueTime, VttHtmlRenderer, VttParser}, DefaultRenderer, Document, Parser as _, PlainTextParser, Renderer, SnippetRenderer}, dt, morph::{KoreanParser, Segment}, must, not_found, status, status_msg, time, Error, Result};
use url::Url;
use youtube_dl::YoutubeDl;

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, help="path to configuration file")]
    config: String,
}

struct Context {
    config: Config,
    korean: Arc<KoreanParser>,
    books: Books,
    dict: Dictionary,
    templates: Arc<Mutex<Tera>>,
    docs: Arc<Mutex<HashMap<i64, Document>>>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config = Config::load(&args.config)?;
    tokio::fs::create_dir_all(config.word_images_path()).await?;
    tokio::fs::create_dir_all(config.book_audio_path()).await?;
    let pool = config.database.open().await?;
    let books = Books::new(pool.clone(), config.book_audio_path());
    let dict = Dictionary::new(pool.clone(), config.word_images_path());
    time!(dict.prefetch_all().await?);
    let korean = Arc::new(KoreanParser::load(&config.mecab, dict.clone())?);
    let port = config.port;
    let mut tera = match Tera::new("templates/**/*.html") {
        Ok(t) => t,
        Err(e) => {
            println!("Template parsing error: {}", e);
            ::std::process::exit(1);
        }
    };
    tera.register_filter("markdown", markdown_filter);
    tera.register_filter("url_domain", url_domain_filter);
    tera.register_filter("firstline", firstline_filter);
    tera.register_function("global_config", config.template.clone());
    let templates = Arc::new(Mutex::new(tera));
    let docs = Arc::new(Mutex::new(HashMap::new()));
    let ctx = Context { config, korean, books, dict, templates, docs };
    let app = Router::new()
        .route("/", get(|| async { "Hello, world!" }))
        .route("/import_video", get(get_import_video).post(post_import_video))
        .route("/video", get(get_video))
        .route("/read/:slug", get(read))
        .route("/edit/:slug", get(get_edit))
        .route("/books/:id/audio", get(get_book_audio))
        .route("/define/:text", get(define))
        .route("/define/:text/edit", get(edit_define))
        .route("/books", get(get_books))
        .route("/words", get(get_words))
        .route("/words/:id/edit", get(edit_word))
        .route("/words/:id/image", get(get_word_image).put(put_word_image).delete(delete_word_image))
        .route("/words/:id/summary", get(get_word_summary))
        .route("/api/words-suggest", get(words_suggest))
        .route("/api/words-dt", get(words_dt))
        .route("/api/books-dt", get(books_dt))
        .route("/api/words", get(list_words).post(post_word))
        .route("/api/words/:id", get(get_word).put(put_word).delete(delete_word))
        .route("/api/books", get(list_books))
        .route("/api/books/:id", get(get_book).patch(patch_book))
        .route("/api/books/:id/read", post(post_book_read))
        .route("/api/books/:id/cues/:ts", get(get_book_cues))
        .route("/api/books/:id/words/:offset", get(get_book_word))
        .nest_service("/static", ServeDir::new("static"))
        .with_state(Arc::new(ctx));
    let addr = SocketAddrV4::new(Ipv4Addr::from_str("0.0.0.0")?, port);
    let listener = TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn markdown_filter(value: &tera::Value, _: &HashMap<String, tera::Value>) -> tera::Result<tera::Value> {
    let Some(value) = value.as_str() else {
        return Ok(value.clone());
    };
    let parser = pulldown_cmark::Parser::new(value);
    let mut output = String::new();
    pulldown_cmark::html::push_html(&mut output, parser);
    Ok(tera::Value::String(output))
}

fn firstline_filter(value: &tera::Value, _: &HashMap<String, tera::Value>) -> tera::Result<tera::Value> {
    let Some(value) = value.as_str() else {
        return tera::Result::Err(tera::Error::msg("input to firstline must be a string"));
    };
    let value = value.lines().next().unwrap_or_default().to_string();
    Ok(tera::Value::String(value))
}

fn url_domain_filter(value: &tera::Value, _: &HashMap<String, tera::Value>) -> tera::Result<tera::Value> {
    let Some(value) = value.as_str() else {
        return tera::Result::Err(tera::Error::msg("input to url_domain must be a string"));
    };
    let url = match Url::try_from(value) {
        Ok(url) => url,
        Err(e) => tera::Result::Err(tera::Error::msg(format!("input to url_domain must be a valid URL: {e}")))?,
    };
    let Some(domain) = url.domain() else {
        return tera::Result::Err(tera::Error::msg("input to url_domain must include domain"));
    };
    Ok(tera::Value::String(domain.to_string()))
}

struct WordInfo {
    seg: Segment,
    dict: HashMap<String, Vec<Word>>,
    deps: Vec<String>,
}

async fn analyze_document(ctx: &Context, doc: Document) -> Result<Document> {
    if doc.info::<BTreeMap<usize, WordInfo>>().is_some() {
        return Ok(doc);
    }
    let mut segs = vec![];

    let mut total_elapsed = std::time::Duration::ZERO;

    for span in doc.spans.iter() {
        let text = &doc.text[span.clone()];
        if text.trim().is_empty() {
            continue;
        }
        let mut stream = Box::pin(time!(ctx.korean.parse(text)?));
        while let Some(seg) = time!(total_elapsed, stream.next().await) {
            let seg = seg?.with_offset(span.start);
            if seg.words.is_empty() {
                continue;
            }

            // result += render_word(seg.text.as_str(), word_map).as_str();
            let s = seg.text.as_str();
            let mut words = ctx.dict.find_words_by_text(s).await?;
            let dict_words_empty = words.is_empty();
            for w in seg.words.iter() {
                if !w.parents.contains(&w.text) && (dict_words_empty || !w.translation.is_empty()) {
                    words.push(w.clone());
                }
            }
            let parents = words.iter().flat_map(|w| w.parents.clone());
            let mut dict = ctx.dict.find_word_trees_by_text(parents).await?;
            dict.insert(seg.text.clone(), words);
            for (_, words) in dict.iter_mut() {
                for word in words.iter_mut() {
                    word.resolved_status = Some(ctx.dict.resolve_status(word).await?);
                }
            }
            // resolve_stati(ctx.dict, &mut dict);

            let words = dict.get(&seg.text).unwrap();
            let mut deps = vec![seg.text.clone()];
            let mut stack: Vec<_> = words.iter().rev().collect();
            while let Some(w) = stack.pop() {
                for parent in w.parents.iter().rev() {
                    if !deps.contains(parent) {
                        deps.push(parent.clone());
                    }
                    for parent_word in dict.get(parent).unwrap() {
                        stack.push(parent_word);
                    }
                }
            }
            let deps = deps;
            let seg = Segment { words: words.clone(), ..seg };

            segs.push(WordInfo { seg, dict, deps });
        }
    }

    dbg!(total_elapsed);

    let segs = BTreeMap::from_iter(
        segs.into_iter().map(|info| (info.seg.range.start, info)));

    Ok(doc.with(segs))
}

struct TextAreaSnippetRenderer<'a> {
    tera: &'a Tera,
}

#[async_trait]
impl<'a> SnippetRenderer for TextAreaSnippetRenderer<'a> {
    async fn render_snippet(&self, doc: &Document, range: std::ops::Range<usize>) -> Result<String> {
        let text = &doc.text[range];
        let mut ctx = tera::Context::new();
        ctx.insert("text", text);
        let result = self.tera.render("textarea.html", &ctx)?.trim().to_string();
        Ok(result)
    }
}

struct TeraSnippetRenderer<'a> {
    timing: Arc<Mutex<std::time::Duration>>,
    tera: &'a Tera,
    dict: Dictionary,
    display: &'a DisplayConfig,
}

impl<'a> TeraSnippetRenderer<'a> {
    async fn add_time(&self, dur: std::time::Duration) {
        *self.timing.lock().await += dur;
    }
}

#[async_trait]
impl<'a> SnippetRenderer for TeraSnippetRenderer<'a> {
    async fn render_snippet(&self, doc: &Document, range: std::ops::Range<usize>) -> Result<String> {
        let mut t = std::time::Duration::ZERO;
        let words: &BTreeMap<usize, WordInfo> = doc.info()
            .ok_or_else(|| anyhow!("document analysis missing"))?;
        let mut pos = range.start;
        let mut result = String::new();
        for (&start, word) in words.range(range.clone()) {
            if start > pos {
                result += &doc.text[pos..start].replace("\n", "<br>"); // FIXME: HTML-escape
            }
            pos = word.seg.range.end;

            let (min_status, max_status) = self.dict.resolve_stati(word.seg.words.iter()).await?;
            let words = word.dict.get(&word.seg.text).unwrap();

            let mut ctx = tera::Context::new();
            ctx.insert("text", &word.seg.text);
            ctx.insert("words", &words);
            ctx.insert("status", &max_status);
            ctx.insert("min_status", &min_status);
            ctx.insert("dict", &word.dict);
            ctx.insert("hide_tags", &self.display.hide_tags);
            ctx.insert("deps", &word.deps);
            result += time!(t, self.tera.render("inline_word.html", &ctx)?).trim();
        }
        let end = range.end;
        if pos < end {
            result += &doc.text[pos..end].replace("\n", "<br>"); // FIXME: HTML-scape
        }
        block_on(self.add_time(t));
        Ok(result)
    }
}

async fn get_edit(
    State(ctx): State<Arc<Context>>,
    Path(slug): Path<String>,
) -> Result<impl IntoResponse> {
    ctx.templates.lock().await.full_reload()?;
    let book = ctx.books.find_book_by_slug(slug).await?;
    let mut content = book.content;

    let template = match book.content_type.as_str() {
        "text/plain" => "edit_book_plain.html",
        "text/markdown" => "edit_book_markdown.html",
        "text/vtt" => {
            let tera = ctx.templates.lock().await;
            let doc = VttParser.parse_document(&content)?;
            let snippets = TextAreaSnippetRenderer{tera: &tera};
            let renderer = VttHtmlRenderer{
                tera: &tera,
                snippets,
                cue_template: "vtt_cue_edit.html".to_string(),
            };
            content = renderer.render_html(&doc).await?;
            
            "edit_book_vtt.html"
        },
        t => return bad_req(format!("invalid book content type: {t}").as_str()),
    };

    let mut tera = tera::Context::new();
    tera.insert("id", &book.id);
    tera.insert("title", &book.title);
    tera.insert("content_type", &book.content_type);
    tera.insert("content", &content);
    tera.insert("audio_format", &book.audio_file.map(|_| "audio/mpeg")); // FIXME
    tera.insert("url", &book.url);
    tera.insert("youtube_video_id", &book.url.and_then(|url| youtube_video_id(url.as_str())));
    let resp = Html(ctx.templates.lock().await.render(template, &tera)?);

    Ok(resp)
}

#[axum::debug_handler]
async fn read(
    State(ctx): State<Arc<Context>>,
    Path(slug): Path<String>,
) -> Result<impl IntoResponse> {
    ctx.templates.lock().await.full_reload()?;
    let now = std::time::Instant::now();
    let book = ctx.books.find_book_by_slug(slug).await?;
    dbg!(now.elapsed());
    let title = book.title;

    let tera = ctx.templates.lock().await;
    let snippets = TeraSnippetRenderer {
        tera: &tera,
        dict: ctx.dict.clone(),
        display: &ctx.config.display,
        timing: Arc::default(),
    };
    
    let (parser, renderer): (Box<dyn doc::Parser>, Box<dyn doc::Renderer>) = match book.content_type.as_str() {
        "text/plain" => (Box::new(PlainTextParser), Box::new(DefaultRenderer(snippets))),
        "text/vtt" => (Box::new(VttParser), Box::new(VttHtmlRenderer { tera: &tera, snippets, cue_template: "vtt_cue.html".to_string() })),
        "text/markdown" => (Box::new(MarkdownParser), Box::new(MarkdownHtmlRenderer(snippets))),
        t => return bad_req(format!("invalid book content type: {t}").as_str()),
    };
    
    dbg!(now.elapsed());
    let document = parser.parse_document(&book.content)?;
    dbg!(now.elapsed());
    let document = analyze_document(&ctx, document).await?;
    dbg!(now.elapsed());
    // let document = compute_document_stats(&ctx, document).await?;
    // dbg!(document.info::<DocumentStats>());
    // dbg!(now.elapsed());
    
    dbg!("rendering html");
    let content = renderer.render_html(&document).await?;
    // dbg!(*snippets.timing.lock().await);
    drop(renderer);
    drop(tera);
    dbg!("done rendering html");
    dbg!(now.elapsed());

    // let content = match book.content_type.as_str() {
    //     "text/plain" => render_text_ko(&ctx, &book.content).await?,
    //     "text/vtt" => render_vtt_book(&ctx, &book.content).await?,
    //     "text/markdown" => render_markdown_book(&ctx, &book.content).await?,
    //     t => return bad_req(format!("invalid book content type: {t}").as_str()),
    // };

    let mut tera = tera::Context::new();
    tera.insert("id", &book.id);
    tera.insert("title", &title);
    tera.insert("content_type", &book.content_type);
    tera.insert("content", &content);
    tera.insert("audio_format", &book.audio_file.map(|_| "audio/mpeg")); // FIXME
    tera.insert("url", &book.url);
    tera.insert("youtube_video_id", &book.url.and_then(|url| youtube_video_id(url.as_str())));
    dbg!("rendering template");
    let resp = Html(ctx.templates.lock().await.render("read.html", &tera)?);
    dbg!(now.elapsed());

    dbg!("done read");

    Ok(resp)
}

fn is_base64_digit(c: char) -> bool {
    c.is_ascii_uppercase() ||
    c.is_ascii_lowercase() ||
    c.is_ascii_digit() ||
    c == '_' || c == '-'
}

fn youtube_video_id(url: &str) -> Option<String> {
    let url: Url = url.parse().ok()?;
    match (url.domain()?, url.path()) {
        ("www.youtube.com", "/watch") => {
            url.query_pairs()
                .find(|(k, _)| k == "v")
                .map(|(_, v)| v.to_string())
        },
        ("youtu.be", path) => {
            let id = path.strip_prefix("/")?;
            if id.chars().all(is_base64_digit) {
                Some(id.to_string())
            } else {
                None
            }
        },
        _ => None,
    }
}

async fn get_book_audio(
    State(ctx): State<Arc<Context>>,
    Path(book_id): Path<i64>,
    range: Option<TypedHeader<Range>>,
) -> Result<impl IntoResponse> {
    let path = ctx.books.get_book_audio_path(book_id).await?;
    let file = File::open(path).await?;
    let body = KnownSize::file(file).await?;
    let range = range.map(|TypedHeader(range)| range);
    Ok(([(CONTENT_TYPE, "audio/mpeg" /* FIXME */)], Ranged::new(range, body)))
}

struct QueryStruct<T>(pub T);

#[async_trait]
impl<S, T> FromRequestParts<S> for QueryStruct<T>
where
    S: Send + Sync,
    T: serde::de::DeserializeOwned,
{
    type Rejection = Error;

    async fn from_request_parts(parts: &mut axum::http::request::Parts, _: &S) -> Result<Self> {
        let q = parts.uri.query().unwrap_or("");
        let x = serde_qs::Config::new(5, false).deserialize_str(q)?;
        Ok(Self(x))
    }
}

#[derive(Clone, Debug, Deserialize)]
struct WordSuggestRequest {
    q: String,
}

async fn words_suggest(
    State(ctx): State<Arc<Context>>,
    Query(req): Query<WordSuggestRequest>,
) -> Result<impl IntoResponse> {
    Ok(Json(ctx.dict.suggest(&req.q).await?))
}

async fn words_dt(
    State(ctx): State<Arc<Context>>,
    QueryStruct(req): QueryStruct<dt::Request>,
) -> Result<impl IntoResponse> {
    Ok(Json(ctx.dict.fetch_dt(req).await?))
}

async fn books_dt(
    State(ctx): State<Arc<Context>>,
    QueryStruct(req): QueryStruct<dt::Request>,
) -> Result<impl IntoResponse> {
    Ok(Json(ctx.books.fetch_dt(req).await?))
}

async fn get_words(State(ctx): State<Arc<Context>>) -> Result<impl IntoResponse> {
    ctx.templates.lock().await.full_reload()?;
    let tera = tera::Context::new();
    Ok(Html(ctx.templates.lock().await.render("words.html", &tera)?))
}

async fn get_books(State(ctx): State<Arc<Context>>) -> Result<impl IntoResponse> {
    ctx.templates.lock().await.full_reload()?;
    let tera = tera::Context::new();
    Ok(Html(ctx.templates.lock().await.render("books.html", &tera)?))
}

async fn edit_define(
    State(ctx): State<Arc<Context>>,
    Path(texts): Path<String>,
) -> Result<impl IntoResponse> {
    let texts = texts.split(',').map(|s| s.to_string()).collect::<Vec<_>>();
    ctx.templates.lock().await.full_reload()?;
    let mut tera = tera::Context::new();
    let dict = ctx.dict.find_word_trees_by_text(texts.clone()).await?;
    tera.insert("texts", &texts);
    tera.insert("dict", &dict);
    tera.insert("all_tags", &ctx.dict.all_tags().await?);
    tera.insert("dictionaries", &ctx.config.dictionaries);
    Ok(Html(ctx.templates.lock().await.render("edit_define.html", &tera)?))
}

async fn edit_word(
    State(ctx): State<Arc<Context>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse> {
    ctx.templates.lock().await.full_reload()?;
    let mut tera = tera::Context::new();
    let word = must(ctx.dict.find_word_by_id(id).await?)?;
    tera.insert("word", &word);
    tera.insert("all_tags", &ctx.dict.all_tags().await?);
    Ok(Html(ctx.templates.lock().await.render("edit_word.html", &tera)?))
}

async fn define(
    State(ctx): State<Arc<Context>>,
    Path(text): Path<String>,
) -> Result<impl IntoResponse> {
    let words = ctx.dict.find_words_by_text(&text).await?;
    let mut tera = tera::Context::new();
    tera.insert("words", &words);
    let html = ctx.templates.lock().await.render("define.html", &tera)?;
    Ok(Html(format!(r#"
        <html>
            <head>
                <link href="/static/style.css" rel="stylesheet">
            </head>
            <body>
                {html}
            </body>
        </html>
    "#)))
}

async fn get_word_summary(
    State(ctx): State<Arc<Context>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse> {
    let word = must(ctx.dict.find_word_by_id(id).await?)?;
    let first_line = word.translation.lines().next().unwrap_or_default();
    let parser = pulldown_cmark::Parser::new(first_line);
    let mut output = String::new();
    pulldown_cmark::html::push_html(&mut output, parser);
    Ok(Html(output))
}

async fn delete_word_image(
    State(ctx): State<Arc<Context>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse> {
    let image_file = must(ctx.dict.get_word_image_file(id).await?)?;
    if tokio::fs::try_exists(&image_file).await? {
        tokio::fs::remove_file(image_file).await?;
    }
    ctx.dict.delete_word_image_file(id).await?;
    Ok(())
}

struct ImageType {
    content_type: &'static str,
    extension: &'static str,
    format: ImageFormat,
}

static ALLOWED_IMAGE_TYPES: LazyLock<Arc<[ImageType]>> = LazyLock::new(|| Arc::new([
    ImageType {
        content_type: "image/gif",
        extension: "gif",
        format: ImageFormat::Gif,
    },
    ImageType {
        content_type: "image/jpeg",
        extension: "jpg",
        format: ImageFormat::Jpeg,
    },
    ImageType {
        content_type: "image/png",
        extension: "png",
        format: ImageFormat::Png,
    },
]));

async fn put_word_image(
    State(ctx): State<Arc<Context>>,
    Path(id): Path<i64>,
    headers: HeaderMap,
    data: Bytes,
) -> Result<impl IntoResponse> {
    let Some(content_type) = headers.get(CONTENT_TYPE) else {
        return status(StatusCode::UNSUPPORTED_MEDIA_TYPE);
    };
    let content_type = content_type.to_str()?;
    let Some(image_type) = ALLOWED_IMAGE_TYPES.iter().find(|it| it.content_type == content_type) else {
        return status(StatusCode::UNSUPPORTED_MEDIA_TYPE)?;
    };
    let image_file = ctx.dict.get_word_image_file(id).await?;
    if let Some(image_file) = image_file {
        if tokio::fs::try_exists(&image_file).await? {
            tokio::fs::remove_file(image_file).await?;
        }
    }
    let image_file = format!("{id}.{}", image_type.extension);
    let image_path = ctx.config.word_images_path().join(&image_file);
    tokio::fs::write(image_path, data).await?;
    ctx.dict.set_word_image_file(id, &image_file).await?;
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
struct ImageSize {
    w: Option<u32>,
    h: Option<u32>,
}

async fn get_word_image(
    State(ctx): State<Arc<Context>>,
    Path(id): Path<i64>,
    Query(size): Query<ImageSize>,
) -> Result<impl IntoResponse> {
    let image_file = must(ctx.dict.get_word_image_file(id).await?)?;
    let Some(ext) = image_file.extension() else {
        return Err(anyhow!("image file has no extension"))?;
    };
    let Some(image_type) = ALLOWED_IMAGE_TYPES.iter().find(|it| it.extension == ext) else {
        return Err(anyhow!("image file has invalid extension: '{ext:?}'"))?;
    };
    match size {
        ImageSize { w: None, h: None } => {
            let file = File::open(image_file).await?;
            let body = Body::from_stream(ReaderStream::new(file));
            Ok(([(CONTENT_TYPE, image_type.content_type)], body).into_response())
        },
        _ => {
            let img = ImageReader::open(image_file)?.decode()?;
            let w = size.w.unwrap_or(img.width());
            let h = size.h.unwrap_or(img.height());
            let img = img.resize(w, h, image::imageops::FilterType::Lanczos3);
            let mut blob = vec![];
            img.write_to(&mut Cursor::new(&mut blob), image_type.format)?;
            Ok(([(CONTENT_TYPE, image_type.content_type)], blob).into_response())
        },
    }
}

#[derive(Clone, Debug, Deserialize)]
struct WordSearch {
    text: String,
}

async fn list_words(
    State(ctx): State<Arc<Context>>,
    Query(search): Query<WordSearch>,
) -> Result<impl IntoResponse> {
    let words = ctx.dict.find_words_by_text(&search.text).await?;
    if words.is_empty() {
        not_found()?;
    }
    Ok(Json(words))
}

fn validate_word(word: &Word) -> Result<()> {
    check(!word.parents.contains(&word.text), "word cannot be its own parent")?;
    let mut parents = word.parents.clone();
    parents.sort();
    parents.dedup();
    check(parents.len() == word.parents.len(), "duplicate parents")?;
    let mut tags = word.tags.clone();
    tags.sort();
    tags.dedup();
    check(tags.len() == word.tags.len(), "duplicate tags")?;
    Ok(())
}

async fn post_word(
    State(ctx): State<Arc<Context>>,
    Json(word): Json<Word>,
) -> Result<impl IntoResponse> {
    validate_word(&word)?;
    if word.id.is_some() {
        bad_req("id must not be specified")?;
    }
    let id = ctx.dict.insert_or_update_word(&word).await?;
    let word = Word{ id: Some(id), ..word };
    Ok(Json(word))
}

async fn get_word(
    State(ctx): State<Arc<Context>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse> {
    must(ctx.dict.find_word_by_id(id).await?).map(Json)
}

async fn put_word(
    State(ctx): State<Arc<Context>>,
    Path(id): Path<i64>,
    Json(word): Json<Word>,
) -> Result<impl IntoResponse> {
    validate_word(&word)?;
    let word = Word { id: Some(id), ..word };
    ctx.dict.insert_or_update_word(&word).await?;
    Ok(())
}

async fn delete_word(
    State(ctx): State<Arc<Context>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse> {
    ctx.dict.delete_word(id).await
}

#[derive(Clone, Debug, Deserialize)]
struct BookSearch {
    url: String,
}

#[derive(Clone, Debug, Serialize)]
struct BookSummary {
    id: i64,
    slug: String,
    title: String,
    content_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    audio_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    published: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_read: Option<String>,
}

impl From<Book> for BookSummary {
    fn from(book: Book) -> Self {
        let id = book.id;
        Self {
            id,
            slug: book.slug,
            title: book.title,
            content_type: book.content_type,
            url: book.url,
            audio_url: book.audio_file.map(|_| format!("/books/{id}/audio")),
            published: book.published.map(|dt| dt.format("%+").to_string()),
            last_read: book.last_read.map(|dt| dt.format("%+").to_string()),
        }
    }
}

async fn list_books(
    State(ctx): State<Arc<Context>>,
    Query(search): Query<BookSearch>,
) -> Result<impl IntoResponse> {
    let ids = ctx.books.find_book_ids_by_url(&search.url).await?;
    let mut books = vec![];
    for id in ids {
        let book = ctx.books.find_book_by_id(id).await?;
        let summary: BookSummary = book.into();
        books.push(summary);
    }
    Ok(Json(books))
}

async fn get_book(
    State(ctx): State<Arc<Context>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse> {
    let book = ctx.books.find_book_by_id(id).await?;
    let summary: BookSummary = book.into();
    Ok(Json(summary))
}

#[derive(Clone, Debug, Serialize)]
struct BookCueWord {
    offset: usize,
    min_status: WordStatus,
    max_status: WordStatus,
}

#[derive(Clone, Debug, Serialize)]
struct BookCueToken {
    text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    word: Option<BookCueWord>,
}

#[derive(Clone, Debug, Serialize)]
struct BookCueLine {
    tokens: Vec<BookCueToken>,
}

#[derive(Clone, Debug, Serialize)]
struct BookCue {
    start: f64,
    end: f64,
    lines: Vec<BookCueLine>,
}

#[derive(Clone, Debug, Serialize)]
struct BookCues {
    cues: Vec<BookCue>,
}

fn resolve_status(wi: &WordInfo) -> (WordStatus, WordStatus) {
    use WordStatus::*;
    let (mut min, mut max) = (Unknown, Unknown);
    for (a, b) in wi.seg.words.iter().flat_map(|w| w.resolved_status) {
        min = match min {
            Unknown => a,
            _ => min.min(a),
        };
        max = match max {
            Unknown => b,
            _ => max.max(b),
        }
    }
    (min, max)
}

fn render_book_cue(doc: &Document, cue: &Cue) -> Result<BookCue> {
    let Some(words) = doc.info::<BTreeMap<usize, WordInfo>>() else {
        return status_msg(StatusCode::INTERNAL_SERVER_ERROR, "no word info");
    };
    let mut lines = vec![];
    let mut tokens = vec![];
    let mut pos = cue.text_range.start;
    for (start, word) in words.range(cue.text_range.clone()) {
        if *start > pos {
            let text = doc.text[pos..*start].to_string() + "\0";
            for (i, line) in text.lines().enumerate() {
                let line = line.strip_suffix("\0").unwrap_or(line);
                if i > 0 {
                    lines.push(BookCueLine { tokens });
                    tokens = vec![];
                }
                if line.is_empty() {
                    continue;
                }
                tokens.push(BookCueToken {
                    text: line.to_string(),
                    word: None,
                });
            }
        }
        pos = word.seg.range.end;
        let text = word.seg.text.clone();
        let (min_status, max_status) = resolve_status(word);
        let offset = *start;
        let word = Some(BookCueWord { offset, min_status, max_status });
        tokens.push(BookCueToken { text, word });
    }
    if pos < cue.text_range.end {
        let text = doc.text[pos..cue.text_range.end].to_string() + "\0";
        for (i, line) in text.lines().enumerate() {
            let line = line.strip_suffix("\0").unwrap_or(line);
            if i > 0 {
                lines.push(BookCueLine { tokens });
                tokens = vec![];
            }
            if line.is_empty() {
                continue;
            }
            tokens.push(BookCueToken {
                text: line.to_string(),
                word: None,
            });
        }
    }
    if !tokens.is_empty() {
        lines.push(BookCueLine { tokens });
    }
    Ok(BookCue {
        start: cue.start.to_seconds(),
        end: cue.end.to_seconds(),
        lines,
    })
}

async fn get_book_cues(
    State(ctx): State<Arc<Context>>,
    Path((id, ts)): Path<(i64, f64)>,
) -> Result<impl IntoResponse> {
    let mut docs = ctx.docs.lock().await;

    let doc = match docs.get(&id) {
        Some(doc) => doc,
        None => {
            let book = ctx.books.find_book_by_id(id).await?;
            if book.content_type != "text/vtt" {
                return bad_req("invalid book type");
            }
            let doc = VttParser.parse_document(&book.content)?;
            let doc = analyze_document(&ctx, doc).await?;
            docs.insert(id, doc);
            docs.get(&id).unwrap()
        },
    };

    let Some(cues) = doc.info::<Vec<Cue>>() else {
        return status_msg(StatusCode::INTERNAL_SERVER_ERROR, "no cues");
    };

    let time = CueTime::from_seconds(ts);
    let result = cues.binary_search_by(|cue| {
        if cue.end < time {
            Ordering::Less
        } else if cue.start > time {
            Ordering::Greater
        } else {
            Ordering::Equal
        }
    });
    let (min, max) = match result {
        Ok(i) => (i.saturating_sub(1), i + 1),
        Err(i) => (i.saturating_sub(1), i),
    };
    let max = max.min(cues.len() - 1);

    let cues = cues[min..=max].iter()
        .map(|cue| render_book_cue(doc, cue))
        .collect::<Result<Vec<_>>>()?;

    Ok(Json(BookCues { cues }))
}

#[derive(Clone, Debug, Serialize)]
struct BookWordDef {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
    pub text: String,
    pub status: Option<WordStatus>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pronunciation: Option<String>,
    pub translation: String,
    pub translation_html: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub parents: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_file: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_status: Option<(WordStatus, WordStatus)>,
    pub inherit: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub debug: Option<String>,
}

impl From<Word> for BookWordDef {
    fn from(value: Word) -> Self {
        let parser = pulldown_cmark::Parser::new(&value.translation);
        let mut translation_html = String::new();
        pulldown_cmark::html::push_html(&mut translation_html, parser);
        Self {
            id: value.id,
            text: value.text,
            status: value.status,
            pronunciation: value.pronunciation,
            translation: value.translation,
            translation_html,
            tags: value.tags,
            parents: value.parents,
            image_file: value.image_file,
            resolved_status: value.resolved_status,
            inherit: value.inherit,
            debug: value.debug,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
struct BookWord {
    offset: usize,
    text: String,
    min_status: WordStatus,
    max_status: WordStatus,
    defs: Vec<BookWordDef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    deps: Vec<BookWordDef>,
}

async fn get_book_word(
    State(ctx): State<Arc<Context>>,
    Path((id, offset)): Path<(i64, usize)>,
) -> Result<impl IntoResponse> {
    let mut docs = ctx.docs.lock().await;

    let doc = match docs.get(&id) {
        Some(doc) => doc,
        None => {
            let book = ctx.books.find_book_by_id(id).await?;
            if book.content_type != "text/vtt" {
                return bad_req("invalid book type");
            }
            let doc = VttParser.parse_document(&book.content)?;
            let doc = analyze_document(&ctx, doc).await?;
            docs.insert(id, doc);
            docs.get(&id).unwrap()
        },
    };

    let Some(words) = doc.info::<BTreeMap<usize, WordInfo>>() else {
        return status_msg(StatusCode::INTERNAL_SERVER_ERROR, "no word info");
    };

    let word_info = must(words.get(&offset))?;
    let (min_status, max_status) = resolve_status(word_info);

    let word_ids = word_info.seg.words.iter().filter_map(|w| w.id).collect_vec();

    let word = BookWord {
        offset,
        text: word_info.seg.text.clone(),
        defs: word_info.seg.words.clone().into_iter().map(|w| w.into()).collect_vec(),
        deps: word_info.dict.values().flatten()
            .filter(|w| w.id.map(|id| !word_ids.contains(&id)).unwrap_or(false))
            .cloned()
            .map(|w| w.into())
            .collect_vec(),
        min_status, max_status,
    };

    Ok(Json(word))
}

#[derive(Clone, Debug, Deserialize)]
struct PatchBookRequest {
    content: Option<String>,
}

async fn patch_book(
    State(ctx): State<Arc<Context>>,
    Path(id): Path<i64>,
    Json(book): Json<PatchBookRequest>,
) -> Result<impl IntoResponse> {
    if let Some(content) = book.content {
        ctx.books.set_book_content(id, content.as_str()).await?;
    }
    Ok(())
}

async fn post_book_read(
    State(ctx): State<Arc<Context>>,
    Path(id): Path<i64>,
) -> Result<impl IntoResponse> {
    ctx.books.mark_book_read(id).await
}

#[derive(Clone, Debug, Deserialize)]
struct VideoRequest {
    url: Option<String>,
}

async fn get_video(
    State(ctx): State<Arc<Context>>,
    Query(req): Query<VideoRequest>,
) -> Result<impl IntoResponse> {
    let Some(url) = req.url else {
        return Ok(Redirect::to("/books"));
    };
    // FIXME: canonicalize URL
    let ids = ctx.books.find_book_ids_by_url(&url).await?;
    // FIXME: handle case where there are multiple matches.
    let Some(id) = ids.first() else {
        return Ok(Redirect::to(&format!("/import_video?url={}", urlencoding::encode(url.as_str()))));
    };
    Ok(Redirect::to(&format!("/read/{id}")))
}

#[derive(Clone, Debug, Deserialize)]
struct GetImportVideoRequest {
    url: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ImportVideoOptions {
    title: String,
    subtitles: Vec<ImportVideoOptionsSubtitles>,
    tags: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct ImportVideoOptionsSubtitles {
    auto: bool,
    lang: String,
    name: String,
    url: String,
}

async fn get_import_video(
    State(ctx): State<Arc<Context>>,
    Query(req): Query<GetImportVideoRequest>,
) -> Result<impl IntoResponse> {
    let Some(url) = req.url else {
        return bad_req("url required");
    };
    let video = YoutubeDl::new(&url)
        .socket_timeout("15")
        .run_async().await?
        .into_single_video();
    let Some(video) = video else {
        return bad_req("not a video page");
    };

    let mut title = String::new();
    if let Some(channel) = video.channel {
        title += &format!("[{channel}]: ");
    }
    if let Some(video_title) = video.title {
        title += &video_title;
    } else {
        title += &url;
    }

    let mut tags = video.tags.unwrap_or_default().into_iter().flatten().collect_vec();
    tags.sort();
    tags.dedup();
    let tags = tags;

    let mut subtitles = vec![];
    let subtitles_iter = video.subtitles.clone().unwrap_or_default().into_iter()
        .map(|(k, v)| (k, v.unwrap_or_default(), false));
    let auto_captions_iter = video.automatic_captions.clone().unwrap_or_default().into_iter()
        .map(|(k, v)| (k, v, true));

    for (lang, list, auto) in subtitles_iter.chain(auto_captions_iter) {
        let prefix = format!("{}-", ctx.config.lang);
        let prefix2 = format!("{}_", ctx.config.lang);
        if lang != ctx.config.lang && !lang.starts_with(&prefix) && !lang.starts_with(&prefix2) {
            continue;
        }
        for st in list {
            let Some(ref ext) = st.ext else {
                continue;
            };
            if ext != "vtt" {
                continue;
            }
            let Some(url) = st.url else {
                continue;
            };
            let lang = lang.clone();
            let name = st.name.unwrap_or_else(|| lang.clone());
            subtitles.push(ImportVideoOptionsSubtitles { lang, name, auto, url });
        }
    }

    let mut tera = tera::Context::new();
    tera.insert("url", &url);
    tera.insert("title", &title);
    tera.insert("subtitles", &subtitles);
    tera.insert("tags", &tags);
    tera.insert("published", &video.timestamp);
    tera.insert("duration", &video.duration);
    Ok(Html(ctx.templates.lock().await.render("import_video.html", &tera)?))
}


#[derive(Clone, Debug, Deserialize)]
struct PostImportVideoRequest {
    url: String,
    title: String,
    subtitles: String,
    tags: String,
    published: Option<i64>,
    duration: Option<f64>,
}

#[derive(Clone, Debug, Deserialize)]
struct ImportVideoTag {
    value: String,
}

async fn post_import_video(
    State(ctx): State<Arc<Context>>,
    Form(req): Form<PostImportVideoRequest>,
) -> Result<impl IntoResponse> {
    let tags: Vec<ImportVideoTag> = serde_json::from_str(&req.tags)?;
    let tags = tags.into_iter().map(|t| t.value).collect_vec();
    let content = if req.subtitles == "manual" {
        let Some(dur) = req.duration else {
            return bad_req("duration required for manual captions");
        };
        let mut dur = (dur * 1000.0) as i64;
        let ms = dur % 1000;
        dur /= 1000;
        let ss = dur % 60;
        dur /= 60;
        let mm = dur % 60;
        dur /= 60;
        let hh = dur;
        format!(r#"WEBVTT
Kind: captions
Language: {}

00:00:00.000 --> {:0>2}:{:0>2}:{:0>2}.{:0>3}
.
"#,
            ctx.config.lang, hh, mm, ss, ms)
    } else {
        reqwest::get(req.subtitles).await?.text().await?
    };
    let id = ctx.books.insert_book(NewBook {
        slug: None,
        title: req.title,
        content_type: "text/vtt".to_string(),
        content,
        audio_file: None,
        url: Some(req.url),
        published: req.published.map(|t| Utc.timestamp_opt(t, 0).unwrap()),
        tags,
    }).await?;
    Ok(Redirect::to(&format!("/read/{id}")))
}
