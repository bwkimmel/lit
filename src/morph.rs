use std::{collections::{BTreeMap, HashMap, VecDeque}, fmt::{Debug, Display}, mem::swap, ops::{Deref, Range}, str::FromStr, sync::LazyLock};

use anyhow::anyhow;
use async_stream::try_stream;
use figment::value::magic::RelativePathBuf;
use futures::{Stream, StreamExt};
use notmecab::{Blob, Dict, LexerToken};
use serde::{Deserialize, Serialize};
use serde_with::{skip_serializing_none, DeserializeFromStr};

use crate::{dict::{Dictionary, Word, WordStatus, EMPTY_WORD}, doc::Document, time, Error, Result};

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

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Element {
    text: String,
    pos: String,
}

impl Element {
    fn is_word(&self) -> bool {
        self.pos != "UNK" && !self.pos.starts_with('S')
    }
}

static POS_TAGS: LazyLock<HashMap<&'static str, Vec<&'static str>>> = LazyLock::new(|| HashMap::from([
    ("NNG", vec!["noun"]),
    ("NNP", vec!["noun", "proper noun"]),
    ("NNB", vec!["noun", "bound noun"]),
    ("NR", vec!["number"]),
    ("NP", vec!["pronoun"]),
    ("VV", vec!["verb"]),
    ("VA", vec!["adj"]),
    ("MM", vec!["det"]),
    ("MAG", vec!["adv"]),
    ("MAJ", vec!["conjunction"]),
    ("IC", vec!["interjection"]),
    ("ETN", vec!["noun"]),
    ("ETM", vec!["prenoun"]),
    ("XSV", vec!["verb"]),
    ("XSA", vec!["adj"]),
]));

static COMPATIBILITY_JAMO: LazyLock<HashMap<char, char>> = LazyLock::new(|| HashMap::from([
    // Initial consonants
    ('\u{1100}', '\u{3131}'),
    ('\u{1101}', '\u{3132}'),
    ('\u{1102}', '\u{3134}'),
    ('\u{1103}', '\u{3137}'),
    ('\u{1104}', '\u{3138}'),
    ('\u{1105}', '\u{3139}'),
    ('\u{1106}', '\u{3141}'),
    ('\u{1107}', '\u{3142}'),
    ('\u{1108}', '\u{3143}'),
    ('\u{1109}', '\u{3145}'),
    ('\u{110a}', '\u{3146}'),
    ('\u{110b}', '\u{3147}'),
    ('\u{110c}', '\u{3148}'),
    ('\u{110d}', '\u{3149}'),
    ('\u{110e}', '\u{314a}'),
    ('\u{110f}', '\u{314b}'),
    ('\u{1110}', '\u{314c}'),
    ('\u{1111}', '\u{314d}'),
    ('\u{1112}', '\u{314e}'),

    // Vowels
    ('\u{1161}', '\u{314f}'),
    ('\u{1162}', '\u{3150}'),
    ('\u{1163}', '\u{3151}'),
    ('\u{1164}', '\u{3152}'),
    ('\u{1165}', '\u{3153}'),
    ('\u{1166}', '\u{3154}'),
    ('\u{1167}', '\u{3155}'),
    ('\u{1168}', '\u{3156}'),
    ('\u{1169}', '\u{3157}'),
    ('\u{116a}', '\u{3158}'),
    ('\u{116b}', '\u{3159}'),
    ('\u{116c}', '\u{315a}'),
    ('\u{116d}', '\u{315b}'),
    ('\u{116e}', '\u{315c}'),
    ('\u{116f}', '\u{315d}'),
    ('\u{1170}', '\u{315e}'),
    ('\u{1171}', '\u{315f}'),
    ('\u{1172}', '\u{3160}'),
    ('\u{1173}', '\u{3161}'),
    ('\u{1174}', '\u{3162}'),
    ('\u{1175}', '\u{3163}'),

    // Final consonant
    ('\u{11a8}', '\u{3131}'),
    ('\u{11a9}', '\u{3132}'),
    ('\u{11aa}', '\u{3133}'),
    ('\u{11ab}', '\u{3134}'),
    ('\u{11ac}', '\u{3135}'),
    ('\u{11ad}', '\u{3136}'),
    ('\u{11ae}', '\u{3137}'),
    ('\u{11af}', '\u{3139}'),
    ('\u{11b0}', '\u{313a}'),
    ('\u{11b1}', '\u{313b}'),
    ('\u{11b2}', '\u{313c}'),
    ('\u{11b3}', '\u{313d}'),
    ('\u{11b4}', '\u{313e}'),
    ('\u{11b5}', '\u{313f}'),
    ('\u{11b6}', '\u{3140}'),
    ('\u{11b7}', '\u{3141}'),
    ('\u{11b8}', '\u{3142}'),
    ('\u{11b9}', '\u{3144}'),
    ('\u{11ba}', '\u{3145}'),
    ('\u{11bb}', '\u{3146}'),
    ('\u{11bc}', '\u{3147}'),
    ('\u{11bd}', '\u{3148}'),
    ('\u{11be}', '\u{314a}'),
    ('\u{11bf}', '\u{314b}'),
    ('\u{11c0}', '\u{314c}'),
    ('\u{11c1}', '\u{314d}'),
    ('\u{11c2}', '\u{314e}'),
]));

fn normalize_jamo(c: char) -> char {
    COMPATIBILITY_JAMO.get(&c).copied().unwrap_or(c)
}

impl FromStr for Element {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let [text, pos, _] = &s
            .split('/')
            .map(String::from)
            .collect::<Vec<_>>()[..] else {
            return Err(anyhow!("invalid pattern element: {s}").into());
        };
        // if sub != "*" {
        //     Err(anyhow!("invalid pattern element: {s}"))?;
        // }
        let text = text.chars().map(normalize_jamo).collect::<String>();
        let pos = pos.clone();
        Ok(Element { text, pos })
    }
}

impl Display for Element {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.text)?;
        write!(f, "/")?;
        write!(f, "{}", self.pos)?;
        write!(f, "/*")
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, DeserializeFromStr)]
struct Pattern(Vec<Element>);

impl Pattern {
    fn is_word(&self) -> bool {
        self.0.iter().all(|e| e.is_word())
    }
}

impl FromStr for Pattern {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let mut elements = vec![];
        for e in s.split('+') {
            elements.push(e.parse()?);
        }
        Ok(Pattern(elements))
    }
}

impl Display for Pattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mut first = true;
        for e in self.0.iter() {
            if !first {
                write!(f, "+")?;
            }
            first = false;
            write!(f, "{}", e)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug)]
struct Token {
    range: Option<Range<usize>>,
    text: String,
    pattern: Pattern,
}

impl Token {
    fn from_lexer_token(lexer_token: LexerToken, dict: &Dict) -> Result<Self> {
        let feature = lexer_token.get_feature(dict);
        let fields = feature.split(',').collect::<Vec<_>>();
        if fields.len() != 8 {
            Err(anyhow!("token expected to have 8 fields, got {}: {feature}", fields.len()))?;
        }
        let text = fields[3].to_string();
        let pattern = match fields[7] {
            "*" => Pattern(vec![Element {
                text: text.clone(),
                pos: fields[0].to_string(),
            }]),
            p => Pattern::from_str(p)?,
        };

        let range = Some(lexer_token.range);
        Ok(Token { range, text, pattern })
    }
}

#[derive(Clone, Debug, Deserialize)]
#[skip_serializing_none]
struct RuleRow {
    pattern: Pattern,
    parents: Option<String>,
    tags: Option<String>,
    translation: Option<String>,
    output: Option<Pattern>,
}

#[derive(Clone, Debug)]
struct Rule {
    pattern: Pattern,
    parents: Vec<String>,
    tags: Vec<String>,
    translation: Option<String>,
    output: Option<Pattern>,
}

fn maybe_string_to_list(s: Option<String>) -> Vec<String> {
    s
        .filter(|p| !p.is_empty())
        .map(|s| s.split(',').map(|t| t.trim().to_string()).collect())
        .unwrap_or_default()
}

impl From<RuleRow> for Rule {
    fn from(row: RuleRow) -> Self {
        let parents = maybe_string_to_list(row.parents);
        let tags = maybe_string_to_list(row.tags);
        Self {
            pattern: row.pattern,
            parents,
            tags,
            translation: row.translation,
            output: row.output,
        }
    }
}

struct RuleTrie {
    children: HashMap<Element, Box<RuleTrie>>,
    rule: Option<Rule>,
}

impl RuleTrie {
    fn new() -> Self {
        Self {
            children: HashMap::new(),
            rule: None,
        }
    }
}

pub struct KoreanParser {
    dict: Dict,
    rules: RuleTrie,
    lit_dict: Dictionary,
    overrides: HashMap<String, Vec<WordParsing>>,
}

pub struct KoreanParseOutput<'a> {
    parser: &'a KoreanParser,
    tokens: VecDeque<Token>,
    next: Option<Segment>,
}

impl<'a> KoreanParseOutput<'a> {
    async fn next(&mut self) -> Result<Option<Segment>> {
        if let Some(next) = self.next.take() {
            return Ok(Some(next));
        }

        let mut range: Option<Range<usize>> = None;
        let mut text = String::new();
        let mut tokens = vec![];

        while let Some(token) = self.tokens.pop_front() {
            let token_range = token.range.clone()
                .ok_or_else(|| anyhow!("token is missing source range"))?;
            if !token.pattern.is_word() {
                self.next = Some(Segment {
                    range: token_range,
                    text: token.text.to_string(),
                    words: vec![],
                });
                break;
            }
            if let Some(ref mut range) = range {
                range.end = token_range.end;
            } else {
                range = Some(token_range);
            }
            text += token.text.to_string().as_str();
            tokens.push(token);
        }

        let Some(range) = range else {
            return Ok(self.next.take());
        };

        let mut t = std::time::Duration::ZERO;
        let mut words = vec![];
        if let Some(overrides) = self.parser.overrides.get(&text) {
            for ovr in overrides {
                words.push(time!(t, self.parser.tokens_to_word(&text, ovr.tokens.clone()).await)?);
            }
        }

        // FIXME: this could be a duplicate of one of the overrides.
        words.push(time!(t, self.parser.tokens_to_word(&text, tokens).await)?);
        Ok(Some(Segment { range, text, words }))
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct MecabConfig {
    pub sysdic: RelativePathBuf,
    pub unkdic: RelativePathBuf,
    pub matrix: RelativePathBuf,
    pub char: RelativePathBuf,
}

#[derive(Debug, Deserialize)]
struct OverrideRow {
    word: String,
    parsing: WordParsing,
}

#[derive(Debug, DeserializeFromStr)]
struct WordParsing {
    tokens: Vec<Token>,
}

impl FromStr for WordParsing {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self> {
        let mut tokens = vec![];
        for token_str in s.split(';') {
            let Some((text, rest)) = token_str.split_once('(') else {
                return Err(anyhow!("invalid token: {token_str}").into());
            };
            let Some(pattern) = rest.strip_suffix(')') else {
                return Err(anyhow!("invalid token: {token_str}").into());
            };
            let text = text.to_string();
            let pattern = Pattern::from_str(pattern)?;
            let token = Token { range: None, text, pattern };
            tokens.push(token);
        }
        Ok(Self { tokens })
    }
}

impl KoreanParser {
    pub fn load(config: &MecabConfig, lit_dict: Dictionary) -> Result<Self> {
        let sysdic = Blob::open(config.sysdic.relative())?;
        let unkdic = Blob::open(config.unkdic.relative())?;
        let matrix = Blob::open(config.matrix.relative())?;
        let char = Blob::open(config.char.relative())?;
        let dict = Dict::load(sysdic, unkdic, matrix, char)
            .map_err(|e| anyhow!("failed to load mecab dictionary: {e}"))?;

        let mut rules = RuleTrie::new();
        let mut rdr = csv::ReaderBuilder::new()
            .flexible(true)
            .from_path("resources/korean_term_patterns.csv")?;
        for rule in rdr.deserialize::<RuleRow>() {
            let rule: Rule = rule?.into();
            let mut node = &mut rules;
            for element in rule.pattern.0.iter() {
                node = node.children.entry(element.clone())
                    .or_insert_with(|| Box::new(RuleTrie::new()));
            }
            if node.rule.is_some() {
                println!("WARNING: duplicate term pattern rule: {}", rule.pattern);
            }
            let _ = node.rule.insert(rule);
        }
        
        let mut overrides: HashMap<String, Vec<WordParsing>> = HashMap::new();
        let mut rdr = csv::ReaderBuilder::new()
            .from_path("resources/korean_parse_overrides.csv")?;
        for ovr in rdr.deserialize::<OverrideRow>() {
            let ovr = ovr?;
            overrides.entry(ovr.word).or_default().push(ovr.parsing);
        }

        Ok(KoreanParser { dict, rules, lit_dict, overrides })
    }

    pub fn parse<'a>(&'a self, text: &'a str) -> Result<impl Stream<Item = Result<Segment>> + 'a> {
        let mut tokens = VecDeque::new();
        let result = self.dict.tokenize(text)?;
        let mut last_end = 0;
        for token in result.0 {
            let token = Token::from_lexer_token(token, &self.dict)?;
            if token.text == "*" {
                continue;
            }
            let token_range = token.range.clone()
                .ok_or_else(|| anyhow!("token is missing source range"))?;
            let range = last_end..token_range.start;
            let sep: String = text[range.clone()].to_string();
            if !sep.is_empty() {
                let pattern = Pattern(vec![Element {
                    text: sep.clone(),
                    pos: "UNK".to_string(),
                }]);
                let range = Some(range);
                tokens.push_back(Token { range, text: sep, pattern });
            }
            last_end = token_range.end;
            tokens.push_back(token);
        }
        let range = last_end..text.bytes().len();
        let sep: String = text[range.clone()].to_string();
        if !sep.is_empty() {
            let pattern = Pattern(vec![Element {
                text: sep.clone(),
                pos: "UNK".to_string(),
            }]);
            let range = Some(range);
            tokens.push_back(Token { range, text: sep, pattern });
        }

        let mut output = KoreanParseOutput { parser: self, tokens, next: None };
        Ok(try_stream! {
            while let Some(seg) = output.next().await? {
                yield seg;
            }
        })
    }
}

enum Reduction {
    Step,
    Terminal(String),
}

struct RootProposal {
    word: String,
    pos: String,
    tokens_consumed: usize,
    extra_element: bool,
    in_dict: bool,
}

impl RootProposal {
    fn longer_than(&self, other: &Self) -> bool {
        if self.tokens_consumed != other.tokens_consumed {
            self.tokens_consumed > other.tokens_consumed
        } else {
            self.extra_element
        }
    }

    fn preferred_over(&self, other: &Self) -> bool {
        if self.in_dict != other.in_dict {
            self.in_dict
        } else if self.in_dict {
            self.longer_than(other)
        } else if (self.pos == "VX") != (other.pos == "VX") {
            self.pos != "VX" // prefer anything over VX
        } else if self.pos == "VX" {
            other.longer_than(self) // between two VXs, choose the shorter
        } else {
            self.longer_than(other) // otherwise, choose the longer
        }
    }

    fn build(prefix: &[Token], extra: Option<&Element>) -> Option<Self> {
        // If the last token contains only one element, and there is no extra
        // element, then the root proposal where that one element is the extra
        // element (and one less token is consumed) should be preferred.
        if extra.is_none() {
            if let Some(last) = prefix.last() {
                if last.pattern.0.len() == 1 {
                    None?;
                }
            }
        }
        let pos = extra.map(|e| e.pos.clone()).or_else(|| {
            prefix.last()
                .and_then(|t| t.pattern.0.last())
                .map(|e| e.pos.clone())
        })?;
        let is_verb = is_verb_pos(&pos);
        let is_noun = is_noun_pos(&pos);
        if !is_noun && !is_verb {
            None?;
        }
        let mut word = prefix.iter().map(|t| t.text.clone())
            .chain(extra.iter().map(|e| e.text.clone()))
            .collect::<Vec<_>>()
            .join("");
        if is_verb {
            word += "다";
        }
        let word = word;
        Some(RootProposal {
            word,
            pos,
            tokens_consumed: prefix.len(),
            extra_element: extra.is_some(),
            in_dict: false,
        })
    }

    fn normalize_pos(&self) -> String {
        if self.pos == "VX" || self.pos == "XSV" {
            "VV".to_string()
        } else if self.pos == "XSA" {
            "VA".to_string()
        } else if self.pos == "XSN" || self.pos == "ETN" {
            "NN".to_string()
        } else {
            self.pos.clone()
        }
    }
}

fn is_noun_pos(pos: &str) -> bool {
    pos.starts_with("N") || pos == "XSN" || pos == "ETN"
}

fn is_verb_pos(pos: &str) -> bool {
    pos == "VV" || pos == "VA" || pos == "VX" || pos == "XSV" || pos == "XSA"
}

fn set_or_compare<T, F>(opt: &mut Option<T>, new: T, f: F)
where
    F: FnOnce(&T, &T) -> bool
{
    if let Some(old) = opt {
        if f(old, &new) {
            *opt = Some(new);
        }
    } else {
        *opt = Some(new);
    }
}

impl KoreanParser {
    async fn normalize(&self, tokens: Vec<Token>) -> Result<(String, Pattern)> {
        let mut t = std::time::Duration::ZERO;
        let mut root: Option<RootProposal> = None;
        for (i, token) in tokens.iter().enumerate() {
            if let Some(mut proposal) = RootProposal::build(&tokens[0..i], token.pattern.0.first()) {
                proposal.in_dict = time!(t, self.lit_dict.word_exists(&proposal.word).await)?;
                set_or_compare(&mut root, proposal, |old, new| new.preferred_over(old));
            }
            if let Some(mut proposal) = RootProposal::build(&tokens[0..=i], None) {
                proposal.in_dict = time!(t, self.lit_dict.word_exists(&proposal.word).await)?;
                set_or_compare(&mut root, proposal, |old, new| new.preferred_over(old));
            }
        }
        let Some(root) = root else {
            let mut pattern = Pattern(tokens.iter().flat_map(|t| t.pattern.0.iter().cloned()).collect());
            let mut root = "*".to_string();
            swap(&mut root, &mut pattern.0[0].text);
            if pattern.0[0].pos.starts_with('V') {
                root += "다";
            }
            return Ok((root, pattern));
        };

        let mut tokens = tokens.into_iter().skip(root.tokens_consumed);
        let mut elements = vec![Element { text: "*".to_string(), pos: root.normalize_pos() }];
        if root.extra_element {
            elements.extend(tokens.next().unwrap().pattern.0.into_iter().skip(1));
        }
        elements.extend(tokens.flat_map(|t| t.pattern.0));
        let pattern = Pattern(elements);

        Ok((root.word, pattern))
    }

    async fn tokens_to_word(&self, text: &str, tokens: Vec<Token>) -> Result<Word> {
        if tokens.is_empty() {
            Err(anyhow!("word has no tokens"))?;
        }

        let mut debug = String::new();
        debug += &format!("Initial pattern: {}", tokens.iter().map(|t| format!("{}({})", t.text, t.pattern)).collect::<Vec<_>>().join(";"));

        let mut t = std::time::Duration::ZERO;
        let (root, mut pattern) = time!(t, self.normalize(tokens).await)?;

        debug += &format!("\nAfter normalization: {pattern}\nRoot word: {root}");

        let last_pos = pattern.0.last().unwrap().pos.clone();
        let mut tags: Vec<String> = POS_TAGS.get(last_pos.as_str())
            .unwrap_or(&vec![])
            .iter()
            .map(|tag| tag.to_string())
            .collect();

        debug += &format!("\nTags based on POS: {tags:?}");

        let text = text.to_string();

        if pattern.0.len() == 1 || root == text {
            debug += "\nWord is a root word";
            let debug = Some(debug);
            return Ok(Word { text, tags, debug, status: Some(WordStatus::Unknown), ..EMPTY_WORD });
        }

        let mut parents = vec![root];

        let mut t = std::time::Duration::ZERO;
        while let Some(r) = time!(t, self.reduce_pattern_once(&mut pattern, &mut parents, &mut tags, &mut debug)) {
            debug += &format!("\n  ==> {pattern}");
            match r {
                Reduction::Step => (),
                Reduction::Terminal(translation) => {
                    debug += &format!("\nWord translated as '{translation}'");
                    let debug = Some(debug);
                    return Ok(Word { text, parents, tags, debug, translation, inherit: true, ..EMPTY_WORD });
                },
            }
        }

        debug += "\nPattern is irreducible";
        let debug = Some(debug);

        Ok(Word { text, parents, tags, debug, status: Some(WordStatus::Unknown), translation: format!("`{pattern}`"), ..EMPTY_WORD })
    }

    fn reduce_pattern_once(&self, pattern: &mut Pattern, parents: &mut Vec<String>, tags: &mut Vec<String>, debug: &mut String) -> Option<Reduction> {
        if pattern.0.is_empty() {
            None?;
        }
        // dbg!(&pattern);

        let n = pattern.0.len();
        for i in 0..n {
            let mut rule = self.rules.rule.as_ref();
            let mut node = &self.rules;
            for element in pattern.0[i..].iter() {
                let Some(child) = node.children.get(element).map(|b| b.deref()) else {
                    break;
                };
                node = child;
                rule = child.rule.as_ref().or(rule);
            }
            let Some(rule) = rule else {
                continue;
            };
            if rule.translation.is_some() && !(i == 0 && n == rule.pattern.0.len()) {
                continue;
            }
            // dbg!(&rule);
            *debug += &format!("\nApplying rule: {}", rule.pattern);
            if let Some(output) = &rule.output {
                *debug += &format!(" ==> {output}");
            }
            for parent in rule.parents.iter() {
                if !parents.contains(parent) {
                    *debug += &format!("\nAdding parent: {parent}");
                    parents.push(parent.clone());
                }
            }
            for tag in rule.tags.iter() {
                if !tags.contains(tag) {
                    *debug += &format!("\nAdding tag: {tag}");
                    tags.push(tag.clone());
                }
            }
            if let Some(tr) = &rule.translation {
                return Some(Reduction::Terminal(tr.clone()));
            }
            let j = i + rule.pattern.0.len();
            let output = rule.output.clone().unwrap_or(Pattern(vec![]));
            // dbg!((i, j, &output));
            let mut elements = pattern.0.to_vec();
            elements.splice(i..j, output.0.into_iter());
            let mut new_pattern = Pattern(elements);
            swap(pattern, &mut new_pattern);
            return Some(Reduction::Step);
        }

        None
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
