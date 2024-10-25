use axum::async_trait;
use itertools::Itertools;
use pulldown_cmark::{html::push_html, Event, TextMergeWithOffset};

use crate::Result;
use super::{Document, Parser, Renderer, SnippetRenderer};

pub struct MarkdownParser;

impl Parser for MarkdownParser {
    fn parse_document(&self, input: &str) -> Result<Document> {
        let parser = pulldown_cmark::Parser::new(input);
        let parser = TextMergeWithOffset::new(parser.into_offset_iter());
        let spans = parser.filter_map(|(event, range)| match event {
            Event::Text(_) => Some(range),
            _ => None,
        }).collect_vec();
        let text = input.to_string().into_boxed_str();
        Ok(Document::new(text, spans))
    }
}

pub struct MarkdownHtmlRenderer<T: SnippetRenderer + Send>(pub T);

#[async_trait]
impl<T: SnippetRenderer + Send + Sync> Renderer for MarkdownHtmlRenderer<T> {
    async fn render_html(&self, doc: &Document) -> Result<String> {
        let parser = pulldown_cmark::Parser::new(&doc.text);
        let parser = TextMergeWithOffset::new(parser.into_offset_iter());
        let mut events = vec![];
        for (event, range) in parser {
            events.push(match event {
                Event::Text(_) => Event::InlineHtml(self.0.render_snippet(doc, range).await?.into()),
                _ => event,
            });
        }

        let mut html = String::new();
        push_html(&mut html, events.into_iter());
        Ok(html)
    }
}
