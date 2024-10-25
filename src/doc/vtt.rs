use std::{fmt::Debug, ops::Range};

use anyhow::anyhow;
use axum::async_trait;
use subtp::vtt::{VttBlock, WebVtt};
use tera::Tera;

use crate::Result;
use super::{Document, Parser, Renderer, SnippetRenderer};

pub struct VttParser;

#[derive(Clone, Debug)]
struct CueTime {
    seconds: u32,
    millis: u16,
}

#[derive(Debug)]
struct Cue {
    start: CueTime,
    end: CueTime,
    text_range: Range<usize>,
}

fn is_word_start(s: &str, i: usize) -> bool {
    if !s.is_char_boundary(i) {
        return false;
    }
    let Some(next) = s[i..].chars().next() else {
        return false;
    };
    if next.is_whitespace() {
        return false;
    }
    if i == 0 {
        return true;
    }
    let mut j = i;
    loop {
        j -= 1;
        if s.is_char_boundary(j) {
            let Some(prev) = s[j..].chars().next() else {
                return false;
            };
            return prev.is_whitespace();
        }
        if j == 0 {
            return false;
        }
    }
}

fn longest_overlap(a: &str, b: &str) -> usize {
    let n = a.len().min(b.len());
    let mut i = a.len() - n;
    while !is_word_start(a, i) && i < a.len() {
        i += 1;
    }
    let a = &a[i..];
    let mut i = n;
    while !is_word_start(b, i) && i > 0 {
        i -= 1;
    }
    let b = &b[..i];

    let n = a.len();
    for (i, _) in a.char_indices() {
        let j = n - i;
        if is_word_start(b, j) && a[i..] == b[..j] {
            return j;
        }
    }
    0
}

fn remove_tags(s: String) -> String {
    let mut result = String::new();
    let mut tag = false;
    for c in s.chars() {
        match c {
            '<' => tag = true,
            '>' => tag = false,
            _ if tag => (),
            _ => result.push(c),
        }
    }
    result
}

impl Parser for VttParser {
    fn parse_document(&self, input: &str) -> Result<Document> {
        let mut text = String::new();
        let mut cues = vec![];
        let vtt = WebVtt::parse(input)
            .map_err(|err| anyhow!("invalid VTT content: {err}"))?;
        for cue in vtt.blocks.into_iter().filter_map(|b| match b {
            VttBlock::Que(cue) => Some(cue),
            _ => None,
        }) {
            let seconds =
                (cue.timings.start.hours as u32) * 3600 +
                (cue.timings.start.minutes as u32) * 60 +
                (cue.timings.start.seconds as u32);
            let millis = cue.timings.start.milliseconds;
            let start = CueTime { seconds, millis };
            let seconds =
                (cue.timings.end.hours as u32) * 3600 +
                (cue.timings.end.minutes as u32) * 60 +
                (cue.timings.end.seconds as u32);
            let millis = cue.timings.end.milliseconds;
            let end = CueTime { seconds, millis };
            let cue_text = remove_tags(cue.payload.join("\n"));
            let overlap = longest_overlap(&text, &cue_text);
            if overlap == 0 && !text.is_empty() && !text.ends_with("\n") {
                text += "\n";
            }
            let from = text.len() - overlap;
            text += &cue_text[overlap..];
            let to = text.len();
            cues.push(Cue { start, end, text_range: from..to });
        }

        let text = text.into_boxed_str();
        #[allow(clippy::single_range_in_vec_init)]
        let spans = vec![0..text.len()];
        Ok(Document::new(text, spans).with(cues))
    }
}

pub struct VttHtmlRenderer<'a, T: SnippetRenderer + Send> {
    pub tera: &'a Tera,
    pub snippets: T,
    pub cue_template: String,
}

fn fmt_cue_time(t: &CueTime) -> String {
    let mut x = t.seconds;
    let s = x % 60;
    x /= 60;
    let m = x % 60;
    x /= 60;
    let h = x;
    let mut str = String::new();
    if h > 0 {
        str = format!("{h:0>2}:");
    }
    let ms = t.millis;
    str += &format!("{m:0>2}:{s:0>2}.{ms:0>3}");
    str
}

fn cue_seconds(t: &CueTime) -> f64 {
    t.seconds as f64 + t.millis as f64 / 1000.0
}

#[async_trait]
impl<'a, T: SnippetRenderer + Send + Sync> Renderer for VttHtmlRenderer<'a, T> {
    async fn render_html(&self, doc: &Document) -> Result<String> {
        let Some(cues) = doc.info::<Vec<Cue>>() else {
            return Err(anyhow!("not a VTT document").into());
        };

        let mut prev_end = CueTime { seconds: 0, millis: 0 };
        let mut html = String::new();
        for cue in cues {
            let mut ctx = tera::Context::new();
            ctx.insert("prev_end", &cue_seconds(&prev_end));
            ctx.insert("start", &cue_seconds(&cue.start));
            ctx.insert("end", &cue_seconds(&cue.end));
            ctx.insert("prev_end_fmt", &fmt_cue_time(&prev_end));
            ctx.insert("start_fmt", &fmt_cue_time(&cue.start));
            ctx.insert("end_fmt", &fmt_cue_time(&cue.end));
            ctx.insert("content", &self.snippets.render_snippet(doc, cue.text_range.clone()).await?);
            html += &self.tera.render(&self.cue_template, &ctx)?;
            prev_end = cue.end.clone();
        }
        Ok(html)
    }
}
