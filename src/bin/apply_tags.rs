use std::collections::HashMap;

use anyhow::{anyhow, Result};
use clap::Parser;
use dialoguer::Confirm;
use lit::{config::Config, dict::Dictionary};
use tokio::{fs::File, io::{AsyncBufReadExt, BufReader}};

#[derive(Parser, Debug)]
struct Args {
    #[arg(help="file containing the list of words to apply tags to")]
    words: String,

    #[arg(long, short, help="tag to apply")]
    tag: String,

    #[arg(long, help="path to config file")]
    config: String,

    #[arg(long, short='x', help="exclude words having this tag")]
    exclude_tag: Option<String>,

    #[arg(long, help="apply exclusion tag to words when not applying tag")]
    apply_exclusion_tag: bool,

    #[arg(long, short, help="don't attempt to apply the tag to words already having these tags", value_delimiter=',')]
    conflicting_tags: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config = Config::load(&args.config)?;
    let pool = config.database.open().await?;
    let dict = Dictionary::new(pool.clone(), config.word_images_path());

    let mut tags = HashMap::new();

    let rdr = BufReader::new(File::open(&args.words).await?);
    let mut lines = rdr.lines();

    while let Some(line) = lines.next_line().await? {
        for word in dict.find_words_by_text(&line).await.map_err(|e| anyhow!("{e}"))? {
            let Some(id) = word.id else {
                continue;
            };
            if word.tags.contains(&args.tag) {
                continue;
            }
            if let Some(ref exclude_tag) = args.exclude_tag {
                if word.tags.contains(exclude_tag) {
                    continue;
                }
            }
            if args.conflicting_tags.iter().any(|tag| word.tags.contains(tag)) {
                continue;
            }

            println!();
            println!();
            println!("ID  : {id}");
            println!("Text: {}", word.text);
            println!("Tags: {}", word.tags.join(", "));
            println!("Desc: {}", word.translation);
            println!();

            let apply = Confirm::new()
                .with_prompt(format!("Apply tag '{}'?", args.tag))
                .interact_opt()?;
            let Some(apply) = apply else {
                println!("Operation cancelled");
                return Ok(());
            };

            if apply {
                println!("Applying tag '{}'", args.tag);
                tags.insert(id, args.tag.clone());
                continue;
            }
            let Some(exclude_tag) = args.exclude_tag.clone() else {
                continue;
            };
            if args.apply_exclusion_tag {
                println!("Applying tag '{exclude_tag}'");
                tags.insert(id, exclude_tag);
            }
        }
    }

    if tags.is_empty() {
        println!("No tags to apply");
        return Ok(());
    }

    let mut tag_counts: HashMap<String, usize> = HashMap::new();
    for tag in tags.values() {
        *tag_counts.entry(tag.clone()).or_default() += 1;
    }
    
    println!();
    println!();
    println!("DONE!");
    println!();
    println!("Will apply tags: ");
    for (tag, count) in tag_counts {
        println!("  {tag}: {count} words");
    }

    println!();
    if !Confirm::new().with_prompt("Commit?").wait_for_newline(true).interact()? {
        println!("Not committing changes");
        return Ok(());
    }

    let mut txn = pool.begin().await?;
    for (id, tag) in tags {
        sqlx::query("INSERT INTO word_tag (word_id, tag) VALUES (?, ?)")
            .bind(id)
            .bind(tag)
            .execute(&mut *txn)
            .await?;
    }
    txn.commit().await?;

    Ok(())
}
