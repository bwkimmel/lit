use std::{collections::BTreeMap, ops::Range};

use alpha::AlphabeticParser;
use axum::async_trait;
use korean::{KoreanConfig, KoreanParser};
use serde::{Deserialize, Serialize};

use crate::{dict::{Dictionary, Word}, doc::Document, Result};

pub mod alpha;
pub mod korean;

pub struct Segment {
    pub range: Range<usize>,
    pub text: String,
    pub words: Vec<Word>,
}

impl Segment {
    pub fn with_offset(self, delta: usize) -> Self {
        let range = (self.range.start+delta)..(self.range.end+delta);
        Self { range, ..self }
    }
}

#[async_trait]
pub trait Parser {
    async fn parse(&self, text: &str) -> Result<Vec<Segment>>;
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
#[allow(clippy::large_enum_variant)]
pub enum MorphConfig {
    #[default]
    Alphabetic,
    Korean(KoreanConfig),
}

pub enum Morph {
    Alphabetic,
    Korean(KoreanParser)
}

impl Morph {
    pub fn load(config: &MorphConfig, lit_dict: Dictionary) -> Result<Self> {
        Ok(match config {
            MorphConfig::Alphabetic => Self::Alphabetic,
            MorphConfig::Korean(cfg) => Self::Korean(KoreanParser::load(cfg, lit_dict)?),
        })
    }
}

#[async_trait]
impl Parser for Morph {
    async fn parse(&self, text: &str) -> Result<Vec<Segment>> {
        match self {
            Self::Alphabetic => AlphabeticParser.parse(text).await,
            Self::Korean(p) => p.parse(text).await
        }
    }
}

pub async fn analyze_document<P: Parser + ?Sized>(doc: Document, parser: &P, dict: &Dictionary) -> Result<Document> {
    if doc.info::<BTreeMap<usize, Segment>>().is_some() {
        return Ok(doc);
    }
    let mut segs = vec![];

    for span in doc.spans.iter() {
        let text = &doc.text[span.clone()];
        if text.trim().is_empty() {
            continue;
        }
        for seg in parser.parse(text).await? {
            let seg = seg.with_offset(span.start);
            // if seg.words.is_empty() {
            //     continue;
            // }

            let mut words = dict.find_words_by_text(&seg.text).await?;
            let dict_words_empty = words.is_empty();
            for w in seg.words.iter() {
                if !w.parents.contains(&w.text) && (dict_words_empty || !w.translation.is_empty()) {
                    words.push(w.clone());
                }
            }
            let seg = Segment { words: words.clone(), ..seg };

            segs.push(seg);
        }
    }

    let segs = BTreeMap::from_iter(
        segs.into_iter().map(|seg| (seg.range.start, seg)));

    Ok(doc.with(segs))
}
