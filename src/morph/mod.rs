use std::{collections::BTreeMap, ops::Range};

use futures::StreamExt;
use korean::KoreanParser;

use crate::{dict::{Dictionary, Word}, doc::Document, Result};

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

pub async fn analyze_document(doc: Document, parser: &KoreanParser, dict: &Dictionary) -> Result<Document> {
    if doc.info::<BTreeMap<usize, Segment>>().is_some() {
        return Ok(doc);
    }
    let mut segs = vec![];

    for span in doc.spans.iter() {
        let text = &doc.text[span.clone()];
        if text.trim().is_empty() {
            continue;
        }
        let mut stream = Box::pin(parser.parse(text)?);
        while let Some(seg) = stream.next().await {
            let seg = seg?.with_offset(span.start);
            if seg.words.is_empty() {
                continue;
            }

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
