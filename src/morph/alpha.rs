use axum::async_trait;

use super::{Parser, Segment};
use crate::Result;

pub struct AlphabeticParser;

#[async_trait]
impl Parser for AlphabeticParser {
    async fn parse(&self, text: &str) -> Result<Vec<Segment>> {
        let mut segs = vec![];
        let mut start = None;
        for (i, ch) in text.char_indices().chain([(text.len(), '.')]) {
            match (start, ch.is_alphabetic()) {
                (None, true) => start = Some(i),
                (Some(j), false) => {
                    start = None;
                    segs.push(Segment {
                        range: j..i,
                        text: text[j..i].to_string(),
                        words: vec![],
                    });
                }
                _ => (),
            }
        }
        Ok(segs)
    }
}
