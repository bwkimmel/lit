use std::{cmp::Ordering, collections::{BTreeMap, HashMap, HashSet}, pin::Pin, sync::{Arc, LazyLock}};

use anyhow::anyhow;
use async_stream::try_stream;
use clap::Parser;
use futures::{Stream, StreamExt};
use indicatif::ProgressBar;
use lit::{bad_req, books::{Book, Books}, config::Config, dict::{Dictionary, Word, WordStatus}, doc::{self, Document}, morph::{analyze_document, korean::KoreanParser, Segment}, Result};
use tokio::task::JoinSet;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, help="the ID of a single book to analyze")]
    book: Option<i64>,

    #[arg(short, long, help="the number of results to show", default_value_t=5)]
    count: usize,

    #[arg(short, long, help="path to configuration file")]
    config: String,

    #[arg(short, long, help="search string to filter by")]
    filter: Option<String>,

    #[arg(short='j', long, help="max concurrency", default_value_t=1)]
    concurrency: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config = Config::load(&args.config)?;
    let pool = config.database.open().await?;
    let books = Books::new(pool.clone(), config.book_audio_path());
    let dict = Dictionary::new(pool.clone(), config.word_images_path());
    dict.prefetch_all().await?;
    let lang = KoreanParser::load(&config.mecab, dict.clone())?;
    let lang = Arc::new(lang);

    let mut books_stream = book_list(&books, &args);
    let mut books = vec![];
    while let Some(book) = books_stream.next().await {
        let book = book?;
        if args.book.is_none() && book.last_read.is_some() {
            continue;
        }
        books.push(book);
    }

    let mut tasks = JoinSet::new();
    let progress = ProgressBar::new(books.len() as u64);
    let mut stats: Vec<DocumentStats> = vec![];
    for book in books {
        while tasks.len() >= args.concurrency {
            let result = tasks.join_next().await.unwrap()??;
            stats.push(result);
            progress.inc(1);
        }
        tasks.spawn(analyze_book(book, dict.clone(), lang.clone()));
    }
    while let Some(result) = tasks.join_next().await {
        let result = result??;
        stats.push(result);
        progress.inc(1);
    }
    progress.finish();

    // stats.sort_by(|a, b| b.optimistic_score.partial_cmp(&a.optimistic_score).unwrap_or(Ordering::Less));
    stats.retain_mut(|a| a.score.is_finite());
    stats.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(Ordering::Less));
    let (term_width, _) = termion::terminal_size()?;
    let title_width = term_width as usize - 20;
    println!("{0: <5} {3: >3}-{4: >3} {5: >5} {1: <.2$}",
        "id", "title", title_width, "pes", "opt", "score");
    for s in stats.into_iter().take(args.count) {
        println!("{0: <5} {3: >3.0}-{4: >3.0} {5: >5.2} {1: <.2$}",
            s.book_id,
            s.title,
            title_width,
            s.pessimistic_score * 100.0,
            s.optimistic_score * 100.0,
            s.score);
    }

    Ok(())
}

fn book_list<'a>(books: &'a Books, args: &'a Args) -> Pin<Box<dyn Stream<Item = Result<Book>> + 'a>> {
    if let Some(id) = args.book {
        let stream = try_stream! {
            yield books.find_book_by_id(id).await?
        };
        return Box::pin(stream);
    }
    if let Some(ref filter) = args.filter {
        return Box::pin(books.search_books(filter.clone()));
    }
    Box::pin(books.all_books())
}

async fn analyze_book(book: Book, dict: Dictionary, lang: Arc<KoreanParser>) -> Result<DocumentStats> {
    let parser: Box<dyn doc::Parser> = match book.content_type.as_str() {
        "text/plain" => Box::new(doc::PlainTextParser),
        "text/vtt" => Box::new(doc::vtt::VttParser),
        "text/markdown" => Box::new(doc::markdown::MarkdownParser),
        t => return bad_req(format!("invalid book content type: {t}").as_str()),
    };

    let document = parser.parse_document(&book.content).map_err(|e| anyhow!("cannot parse book {}: {e}", book.id))?.with(book);
    let document = analyze_document(document, &lang, &dict).await?;
    let document = compute_document_stats(&dict, document).await?;
    Ok(document.info().cloned().unwrap())
}

#[derive(Clone, Debug, Default)]
struct DocumentStats {
    book_id: i64,
    title: String,
    url: Option<String>,
    slug: Option<String>,
    word_count: usize,
    undefined_words: usize,
    words_with_undefined_roots: usize,
    unique_root_words: usize,
    optimistic_rating_dist: HashMap<WordStatus, usize>,
    pessimistic_rating_dist: HashMap<WordStatus, usize>,
    undefined_words_without_undefined_roots: usize,
    optimistic_score: f64,
    pessimistic_score: f64,
    optimistic_weight: f64,
    pessimistic_weight: f64,
    score: f64,
}

static WEIGHTS: LazyLock<HashMap<WordStatus, f64>> = LazyLock::new(|| {
    HashMap::from_iter([
        (WordStatus::WellKnown, 1.0),
        (WordStatus::Level5,    0.9),
        (WordStatus::Level4,    0.7),
        (WordStatus::Level3,    0.5),
        (WordStatus::Level2,    0.2),
        (WordStatus::New,       0.1),
        (WordStatus::Unknown,   0.0),
    ])
});

fn score(rating_dist: &HashMap<WordStatus, usize>) -> f64 {
    let mut score = 0.0;
    let mut total = 0;
    for (status, count) in rating_dist.iter() {
        let weight = WEIGHTS.get(status).cloned().unwrap_or_default();
        score += weight * (*count as f64);
        total += count;
    }
    score /= total as f64;
    score
}

fn weight(rating_dist: &HashMap<WordStatus, usize>) -> f64 {
    let mut weight = 0.0;
    for (status, count) in rating_dist.iter() {
        weight += (*count as f64) * (1.0 - WEIGHTS.get(status).cloned().unwrap_or_default());
    }
    weight
}

static TAG_MIN_STATUSES: LazyLock<HashMap<String, WordStatus>> = LazyLock::new(|| {
    HashMap::from_iter([
        ("loan".to_string(), WordStatus::WellKnown),
        ("transliteration".to_string(), WordStatus::WellKnown),
        ("topik1".to_string(), WordStatus::WellKnown),
        ("topik1v".to_string(), WordStatus::WellKnown),
        ("topik2".to_string(), WordStatus::Level3),
        ("topik2v".to_string(), WordStatus::Level3),
    ])
});

async fn compute_document_stats(dict: &Dictionary, doc: Document) -> Result<Document> {
    let words: &BTreeMap<usize, Segment> = doc.info()
        .ok_or_else(|| anyhow!("could not analyze document"))?;
    let mut roots = HashSet::new();
    let mut optimistic_rating_dist = HashMap::<WordStatus, usize>::new();
    let mut pessimistic_rating_dist = HashMap::<WordStatus, usize>::new();
    let mut undefined_words = 0usize;
    let mut words_with_undefined_roots = 0usize;
    let mut undefined_words_without_undefined_roots = 0usize;

    let eval_status = |word: &Word| -> Option<WordStatus> {
        if word.status == Some(WordStatus::Ignored) {
            return word.status;
        }
        for tag in word.tags.iter() {
            if let Some(status) = TAG_MIN_STATUSES.get(tag) {
                return word.status.map(|s| s.max(*status)).or(Some(*status));
            }
        }
        word.status
    };

    for seg in words.values() {
        let (pessimistic_rating, optimistic_rating) =
            dict.resolve_stati_with_eval(seg.words.iter(), &eval_status).await
            .unwrap_or((WordStatus::Unknown, WordStatus::Unknown));
        *optimistic_rating_dist.entry(optimistic_rating).or_default() += 1;
        *pessimistic_rating_dist.entry(pessimistic_rating).or_default() += 1;
        let mut q = seg.words.clone();
        while let Some(word) = q.pop() {
            if word.parents.is_empty() {
                if dict.find_words_by_text(&word.text).await?.is_empty() {
                    undefined_words += 1;
                    words_with_undefined_roots += 1;
                }
                roots.insert(word.text);
                continue;
            }
            let mut has_undefined_root = false;
            for parent in word.parents {
                let parent_words = dict.find_words_by_text(&parent).await?;
                if parent_words.is_empty() {
                    has_undefined_root = true;
                }
                q.extend(parent_words.into_iter());
            }
            if has_undefined_root {
                words_with_undefined_roots += 1;
            } else if optimistic_rating == WordStatus::Unknown {
                undefined_words_without_undefined_roots += 1;
            }
        }
    }

    let word_count = words.len();
    let unique_root_words = roots.len();

    let mut roots_optimistic_rating_dist = HashMap::new();
    let mut roots_pessimistic_rating_dist = HashMap::new();
    for root in roots.iter() {
        let words = dict.find_words_by_text(root).await?;
        let (pessimistic_rating, optimistic_rating) = dict.resolve_stati_with_eval(words.iter(), &eval_status).await
            .unwrap_or((WordStatus::Unknown, WordStatus::Unknown));
        *roots_optimistic_rating_dist.entry(optimistic_rating).or_default() += 1;
        *roots_pessimistic_rating_dist.entry(pessimistic_rating).or_default() += 1;
    }
    let roots_optimistic_score = score(&roots_optimistic_rating_dist);
    let roots_pessimistic_score = score(&roots_pessimistic_rating_dist);

    let optimistic_score = score(&optimistic_rating_dist);
    let pessimistic_score = score(&pessimistic_rating_dist);
    let optimistic_weight = weight(&optimistic_rating_dist);
    let pessimistic_weight = weight(&pessimistic_rating_dist);
    let book_id = doc.info::<Book>().map(|b| b.id).unwrap_or_default();
    let url = doc.info::<Book>().map(|b| b.url.clone()).unwrap_or_default();
    let slug = doc.info::<Book>().map(|b| b.slug.clone());
    let title = doc.info::<Book>().map(|b| b.title.clone()).unwrap_or_default();

    let score = (unique_root_words as f64) * roots_optimistic_score * optimistic_score / optimistic_weight;
    // let score = -optimistic_weight / (optimistic_score * optimistic_score);

    Ok(doc.with(DocumentStats {
        book_id,
        title,
        url, slug,
        word_count,
        undefined_words,
        words_with_undefined_roots,
        unique_root_words,
        optimistic_rating_dist,
        pessimistic_rating_dist,
        undefined_words_without_undefined_roots,
        optimistic_score,
        pessimistic_score,
        optimistic_weight,
        pessimistic_weight,
        score,
    }))
}
