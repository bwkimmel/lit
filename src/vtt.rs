use clap::Parser;
use serde::{Deserialize, Serialize};
use core::time::Duration;
use itertools::Itertools;
use subtp::vtt::{VttBlock, VttCue, VttTimings, WebVtt};

#[derive(Clone, Debug, Parser, Serialize, Deserialize)]
pub struct TruncateOptions {
    #[arg(long, help="maximum number of repetitions to allow without truncating", default_value_t=5)]
    pub max_repetitions: usize,
    #[arg(long, help="number of repetitions to preserve when truncating", default_value_t=3)]
    pub truncated_repetitions: usize,
    #[arg(long, help="string to append to truncated substring", default_value="...")]
    pub truncation_suffix: String,
}

#[derive(Clone, Debug, Parser, Serialize, Deserialize)]
pub struct MergeOptions {
    #[arg(long, help="maximum duration (in millis) between cue blocks for which to consider merging", default_value_t=500)]
    pub max_merge_gap_millis: u32,
}

#[derive(Clone, Debug, Parser, Serialize, Deserialize)]
pub struct CleanVttOptions {
    #[clap(flatten)]
    pub merge: MergeOptions,
    #[clap(flatten)]
    pub truncate: TruncateOptions,
}

fn truncate_repetitions(mut s: &[char], opts: &TruncateOptions) -> String {
    let mut result = String::new();
    while !s.is_empty() {
        let max = s.len() / opts.max_repetitions;
        let mut truncated = false;
        for n in 1..=max {
            let mut count = 0;
            for (i, j, k) in (0..).tuple_windows() {
                if k*n > s.len() || s[i*n..j*n] != s[j*n..k*n] {
                    count = j;
                    break;
                }
            }
            if count <= opts.max_repetitions {
                continue;
            }
            for _ in 0..opts.truncated_repetitions {
                result += &String::from_iter(s[0..n].iter());
            }
            result += &opts.truncation_suffix;
            s = &s[n*count..];
            truncated = true;
        }
        if truncated {
            continue;
        }
        result.push(s[0]);
        s = &s[1..];
    }
    result
}

fn canonicalize_cue(cue: VttCue, opts: &TruncateOptions) -> VttCue {
    let start = cue.timings.start.min(cue.timings.end);
    let end = cue.timings.start.max(cue.timings.end);
    let payload = cue.payload.into_iter()
        .map(|line| truncate_repetitions(&line.chars().collect_vec(), opts))
        .collect_vec();
    VttCue {
        timings: VttTimings { start, end },
        payload,
        ..cue
    }
}

fn canonicalize_block(block: VttBlock, opts: &TruncateOptions) -> VttBlock {
    match block {
        VttBlock::Que(cue) => VttBlock::Que(canonicalize_cue(cue, opts)),
        _ => block,
    }
}

fn merge_cues(a: &VttCue, b: &VttCue, opts: &MergeOptions) -> Option<VttCue> {
    if a.timings.start > b.timings.start {
        return merge_cues(b, a, opts);
    }
    if a.identifier != b.identifier {
        return None;
    }
    if a.settings != b.settings {
        return None;
    }

    let a_end: Duration = a.timings.end.into();
    let b_start: Duration = b.timings.start.into();
    if b_start > a_end && (b_start - a_end).as_millis() > opts.max_merge_gap_millis.into() {
        return None;
    }

    let start = a.timings.start;
    let end = a.timings.end.max(b.timings.end);
    let timings = VttTimings { start, end };

    let a_str = a.payload.iter().join("\n");
    let b_str = b.payload.iter().join("\n");
    if a_str.starts_with(&b_str) {
        return Some(VttCue { timings, ..a.clone() });
    }
    if b_str.starts_with(&a_str) {
        return Some(VttCue { timings, ..b.clone() });
    }

    None
}

fn merge_blocks(a: &VttBlock, b: &VttBlock, opts: &MergeOptions) -> Option<VttBlock> {
    match (a, b) {
        (VttBlock::Que(a), VttBlock::Que(b)) => merge_cues(a, b, opts).map(VttBlock::Que),
        _ => None,
    }
}

pub fn clean(vtt: WebVtt, opts: &CleanVttOptions) -> WebVtt {
    let mut blocks = vec![];
    for block in vtt.blocks.into_iter() {
        let block = canonicalize_block(block, &opts.truncate);
        let Some(last_block) = blocks.last() else {
            blocks.push(block);
            continue;
        };
        if let Some(merged) = merge_blocks(last_block, &block, &opts.merge) {
            blocks.pop();
            blocks.push(merged);
            continue;
        }
        blocks.push(block);
    }
    WebVtt { blocks, ..vtt }
}
