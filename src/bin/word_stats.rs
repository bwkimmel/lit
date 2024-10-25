use std::{collections::{BTreeMap, HashMap}, pin::Pin, sync::Arc};

use anyhow::anyhow;
use async_stream::try_stream;
use clap::Parser;
use futures::{Stream, StreamExt};
use indicatif::ProgressBar;
use itertools::Itertools;
use lit::{bad_req, books::{Book, Books}, config::Config, dict::{Dictionary, WordStatus}, doc::{self, Document}, morph::{KoreanParser, Segment}, Result};
use tokio::task::JoinSet;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long, help="the ID of a single book to analyze")]
    book: Option<i64>,

    #[arg(short='n', long, help="the number of results to show")]
    count: Option<usize>,

    #[arg(short, long, help="path to configuration file")]
    config: String,

    #[arg(short, long, help="search string to filter by")]
    filter: Option<String>,
    
    #[arg(long, help="maximum word status to include")]
    max_status: Option<usize>,

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
    let max_status = match args.max_status {
        Some(0) => WordStatus::Unknown, 
        Some(1) => WordStatus::New,
        Some(2) => WordStatus::Level2,
        Some(3) => WordStatus::Level3,
        Some(4) => WordStatus::Level4,
        Some(5) => WordStatus::Level5,
        None => WordStatus::WellKnown,
        _ => Err(anyhow!("--max-status must be in range 0..5"))?,
    };

    let mut books_stream = book_list(&books, &args);
    let mut books = vec![];
    while let Some(book) = books_stream.next().await {
        let book = book?;
        if args.book.is_none() && book.last_read.is_some() {
            continue;
        }
        books.push(book);
    }

    let mut roots = HashMap::new();
    let mut tasks = JoinSet::new();
    let progress = ProgressBar::new(books.len() as u64);
    for book in books {
        while tasks.len() >= args.concurrency {
            let result: DocumentStats = tasks.join_next().await.unwrap()??;
            merge_roots(&mut roots, result.roots);
            progress.inc(1);
        }
        tasks.spawn(analyze_book(max_status, book, dict.clone(), lang.clone()));
    }
    while let Some(result) = tasks.join_next().await {
        let result = result??;
        merge_roots(&mut roots, result.roots);
        progress.inc(1);
    }
    progress.finish();

    let mut roots = roots.into_iter().collect_vec();
    roots.sort_by(|(_, a), (_, b)| b.cmp(a));
    if let Some(count) = args.count {
        roots.truncate(count);
    }

    println!("{: >8} {: <16} word", "count", "status");
    for (text, count) in roots {
        let words = dict.find_words_by_text(&text).await?;
        let (pes, opt) = dict.resolve_stati(words.iter()).await?;
        let status = if pes == opt {
            format!("{:?}", pes)
        } else {
            format!("{:?}-{:?}", pes, opt)
        };
        println!("{: >8} {: <16} {}", count, status, text);
    }

    // println!("{0: <5} {3: >3}-{4: >3} {5: >5} {1: <.2$}",
    //     "id", "title", title_width, "pes", "opt", "score");
    // for s in stats.into_iter().take(args.count) {
    //     println!("{0: <5} {3: >3.0}-{4: >3.0} {5: >5.2} {1: <.2$}",
    //         s.book_id,
    //         s.title,
    //         title_width,
    //         s.pessimistic_score * 100.0,
    //         s.optimistic_score * 100.0,
    //         s.score);
    //
    Ok(())
}

fn merge_roots(acc: &mut HashMap<String, usize>, new: HashMap<String, usize>) {
    for (word, count) in new {
        *acc.entry(word).or_default() += count;
    }
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

async fn analyze_book(max_status: WordStatus, book: Book, dict: Dictionary, lang: Arc<KoreanParser>) -> Result<DocumentStats> {
    let parser: Box<dyn doc::Parser> = match book.content_type.as_str() {
        "text/plain" => Box::new(doc::PlainTextParser),
        "text/vtt" => Box::new(doc::vtt::VttParser),
        "text/markdown" => Box::new(doc::markdown::MarkdownParser),
        t => return bad_req(format!("invalid book content type: {t}").as_str()),
    };

    let document = parser.parse_document(&book.content).map_err(|e| anyhow!("cannot parse book {}: {e}", book.id))?.with(book);
    let document = lang.analyze_document(document).await?;
    let document = compute_document_stats(max_status, &dict, document).await?;
    Ok(document.info().cloned().unwrap())
}

#[derive(Clone, Debug, Default)]
struct DocumentStats {
    roots: HashMap<String, usize>,
}

async fn compute_document_stats(max_status: WordStatus, dict: &Dictionary, doc: Document) -> Result<Document> {
    let words: &BTreeMap<usize, Segment> = doc.info()
        .ok_or_else(|| anyhow!("could not analyze document"))?;
    let mut roots = HashMap::new();

    for seg in words.values() {
        let mut q = seg.words.clone();
        while let Some(word) = q.pop() {
            if word.parents.is_empty() {
                let defs = dict.find_words_by_text(&word.text).await?;
                let (_, st) = dict.resolve_stati(defs.iter()).await?;
                if st == WordStatus::Ignored {
                    continue;
                }
                if st <= max_status {
                    *roots.entry(word.text).or_default() += 1;
                }
                continue;
            }
            for parent in word.parents {
                let parent_words = dict.find_words_by_text(&parent).await?;
                q.extend(parent_words.into_iter());
            }
        }
    }

    Ok(doc.with(DocumentStats { roots }))
}
