use std::ops::Range;

use anymap::any::Any;
use axum::async_trait;

use crate::Result;

pub mod markdown;
pub mod vtt;

#[derive(Debug)]
pub struct Document {
    pub text: Box<str>,
    pub spans: Vec<Range<usize>>,
    info: anymap::Map<dyn Any + Send + Sync>,
}

impl Document {
    pub fn new(text: Box<str>, spans: Vec<Range<usize>>) -> Self {
        Self { text, spans, info: anymap::Map::new() }
    }

    pub fn with<T: 'static + Send + Sync>(mut self, x: T) -> Self {
        self.info.insert(x);
        self
    }

    pub fn info<T: 'static + Send + Sync>(&self) -> Option<&T> {
        self.info.get::<T>()
    }
}

#[async_trait]
pub trait SnippetRenderer {
    async fn render_snippet(&self, doc: &Document, range: Range<usize>) -> Result<String>;
}

pub trait Parser: Send {
    fn parse_document(&self, input: &str) -> Result<Document>;
}

#[async_trait]
pub trait Renderer: Send {
    async fn render_html(&self, doc: &Document) -> Result<String>;
}

pub struct PlainTextParser;

impl Parser for PlainTextParser {
    fn parse_document(&self, input: &str) -> Result<Document> {
        let text = input.to_string().into_boxed_str();
        #[allow(clippy::single_range_in_vec_init)]
        let spans = vec![0..input.len()];
        Ok(Document::new(text, spans))
    }
}

pub struct DefaultRenderer<T: SnippetRenderer>(pub T);

#[async_trait]
impl<T: SnippetRenderer + Send + Sync> Renderer for DefaultRenderer<T> {
    async fn render_html(&self, doc: &Document) -> Result<String> {
        self.0.render_snippet(doc, 0..doc.text.len()).await
    }
}
