use std::{cmp::Ordering, collections::{BTreeMap, VecDeque}, pin::Pin, sync::Arc};

use anyhow::anyhow;
use async_stream::try_stream;
use clap::Parser;
use futures::{Stream, StreamExt};
use indicatif::ProgressBar;
use itertools::Itertools;
use lit::{bad_req, books::{Book, Books}, config::Config, dict::{Dictionary, WordStatus}, doc::{self, Document}, morph::{KoreanParser, Segment}, Result};
use tokio::task::JoinSet;
use unicode_segmentation::UnicodeSegmentation;

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

    #[arg(short, long, help="word to search for")]
    target_word: String,

    #[arg(short='M', long, help="maximum number of words in target sentences", default_value_t=21)]
    max_sentence_words: usize,

    #[arg(short='m', long, help="minimum number of words in target sentences", default_value_t=5)]
    min_sentence_words: usize,

    #[arg(short='r', long, help="include only already-read books")]
    only_read_books: bool,
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
        if args.only_read_books && book.last_read.is_none() {
            continue;
        }
        books.push(book);
    }

    println!("Found {} books...", books.len());

    let mut tasks = JoinSet::new();
    let progress = ProgressBar::new(books.len() as u64);
    let mut sentences: Vec<(DocumentInfo, Sentence)> = vec![];
    for book in books {
        while tasks.len() >= args.concurrency {
            let doc: Document = tasks.join_next().await.unwrap()??;
            let book: &Book = doc.info().ok_or(anyhow!("no book"))?;
            let doc_sentences: &Vec<Sentence> = doc.info().ok_or(anyhow!("no sentences"))?;
            let doc_info = DocumentInfo {
                book_id: book.id,
                title: book.title.clone(),
                url: book.url.clone(),
                slug: Some(book.slug.clone()),
            };
            sentences.extend(doc_sentences.clone().into_iter().map(|s| (doc_info.clone(), s)));
            progress.inc(1);
        }
        tasks.spawn(analyze_book(args.target_word.clone(), args.min_sentence_words, args.max_sentence_words, book, dict.clone(), lang.clone()));
    }
    while let Some(result) = tasks.join_next().await {
        let doc: Document = result??;
        let book: &Book = doc.info().ok_or(anyhow!("no book"))?;
        let doc_sentences: &Vec<Sentence> = doc.info().ok_or(anyhow!("no sentences"))?;
        let doc_info = DocumentInfo {
            book_id: book.id,
            title: book.title.clone(),
            url: book.url.clone(),
            slug: Some(book.slug.clone()),
        };
        sentences.extend(doc_sentences.clone().into_iter().map(|s| (doc_info.clone(), s)));
        progress.inc(1);
    }
    progress.finish();

    sentences.sort_by(|a, b| compare_readability(&b.1, &a.1));
    for (doc_info, sentence) in sentences.into_iter().take(args.count) {
        print!("http://localhost:5080/read/{} \"{:.30}\":\t", doc_info.book_id, doc_info.title);
        for (i, word) in sentence.words.iter().enumerate() {
            if i > 0 {
                print!(" ");
            }
            if word.is_target {
                print!(">>>{}<<<", word.text);
            } else {
                print!("{}", word.text);
                match word.status {
                    WordStatus::Unknown => print!("[?]"),
                    WordStatus::Ignored => print!("[I]"),
                    WordStatus::WellKnown => (),
                    n => print!("[{}]", n as u8),
                }
            }
        }
        println!();
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

async fn analyze_book(target_word: String, min_sentence_words: usize, max_sentence_words: usize, book: Book, dict: Dictionary, lang: Arc<KoreanParser>) -> Result<Document> {
    let parser: Box<dyn doc::Parser> = match book.content_type.as_str() {
        "text/plain" => Box::new(doc::PlainTextParser),
        "text/vtt" => Box::new(doc::vtt::VttParser),
        "text/markdown" => Box::new(doc::markdown::MarkdownParser),
        t => return bad_req(format!("invalid book content type: {t}").as_str()),
    };

    let document = parser.parse_document(&book.content).map_err(|e| anyhow!("cannot parse book {}: {e}", book.id))?.with(book);
    let document = lang.analyze_document(document).await?;
    let document = compute_document_stats(&target_word, min_sentence_words, max_sentence_words, &dict, document).await?;
    Ok(document)
}

#[derive(Clone, Debug, Default)]
struct DocumentInfo {
    book_id: i64,
    title: String,
    url: Option<String>,
    slug: Option<String>,
}

struct SearchParams<'a> {
    target_word: &'a str,
    max_sentence_words: usize,
}

#[derive(Clone, Debug, Default)]
struct Sentence {
    words: Vec<SentenceWord>,
}

#[derive(Clone, Debug, Default)]
struct SentenceWord {
    text: String,
    status: WordStatus,
    is_target: bool,
}

fn min_non_target_status(s: &Sentence) -> Option<WordStatus> {
    let mut status = None;
    for word in s.words.iter().filter(|w| !w.is_target) {
        status = match (status, word.status) {
            (st, WordStatus::Ignored) => st,
            (None, st) => Some(st),
            (Some(a), b) => Some(a.min(b))
        }
    }
    status
}

fn compare_readability(a: &Sentence, b: &Sentence) -> Ordering {
    match (min_non_target_status(a), min_non_target_status(b)) {
        (None, None) => return Ordering::Equal,
        (Some(_), None) => return Ordering::Greater,
        (None, Some(_)) => return Ordering::Less,
        (Some(x), Some(y)) if x == y => x,
        (Some(x), Some(y)) => return x.cmp(&y),
    };

    for status in [WordStatus::Unknown, WordStatus::New, WordStatus::Level2, WordStatus::Level3, WordStatus::Level4, WordStatus::Level5, WordStatus::WellKnown] {
        let a_count = a.words.iter().filter(|w| !w.is_target && w.status == status).count();
        let b_count = b.words.iter().filter(|w| !w.is_target && w.status == status).count();
        if a_count < b_count {
            return Ordering::Greater;
        } else if a_count > b_count {
            return Ordering::Less;
        }
    }

    a.words.len().cmp(&b.words.len())
}

async fn compute_document_stats(target_word: &str, min_sentence_words: usize, max_sentence_words: usize, dict: &Dictionary, doc: Document) -> Result<Document> {
    let words: &BTreeMap<usize, Segment> = doc.info()
        .ok_or_else(|| anyhow!("could not analyze document"))?;
    let mut sentences = vec![];

    for (start, sentence) in doc.text.split_sentence_bound_indices() {
        let mut context_words = VecDeque::new();
        let end = start + sentence.len();
        for (_, seg) in words.range(start..end) {
            let (_, optimistic_rating) =
                dict.resolve_stati(seg.words.iter()).await
                .unwrap_or((WordStatus::Unknown, WordStatus::Unknown));
            let is_target_word = seg.text == target_word;
            let mut all_parents = vec![];
            for word in seg.words.iter() {
                all_parents.extend(word.parents.clone());
            }
            let derived_from_target =
                dict.find_word_trees_by_text(all_parents).await?
                .contains_key(target_word);
            context_words.push_back(SentenceWord {
                text: seg.text.to_string(),
                status: optimistic_rating,
                is_target: is_target_word || derived_from_target,
            });
            if context_words.len() > max_sentence_words {
                context_words.pop_front();
                let mid = max_sentence_words / 2;
                if context_words[mid].is_target { // FIXME: off-by-one?
                    let words = context_words.iter().cloned().collect_vec();
                    sentences.push(Sentence { words });
                }
            } else if context_words.len() == max_sentence_words {
                let mid = max_sentence_words / 2;
                if context_words.iter().take(mid).any(|w| w.is_target) { // FIXME: off-by-one?
                    let words = context_words.iter().cloned().collect_vec();
                    sentences.push(Sentence { words });
                }
            }
        }

        if context_words.len() < max_sentence_words {
            if context_words.len() >= min_sentence_words {
                if context_words.iter().any(|w| w.is_target) {
                    let words = context_words.iter().cloned().collect_vec();
                    sentences.push(Sentence { words });
                }
            }
        } else {
            let mid = max_sentence_words / 2;
            if context_words.iter().dropping(mid).any(|w| w.is_target) { // FIXME: off-by-one?
                let words = context_words.iter().cloned().collect_vec();
                sentences.push(Sentence { words });
            }
        }


    }

    Ok(doc.with(sentences))
}
