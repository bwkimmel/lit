use std::io::{read_to_string, stdin};

use anyhow::Result;
use clap::Parser;
use lit::vtt::CleanVttOptions;
use subtp::vtt::WebVtt;

#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Args {
    #[clap(flatten)]
    clean_vtt_opts: CleanVttOptions,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let input = read_to_string(stdin())?;
    let vtt = WebVtt::parse(&input)?;
    let vtt = lit::vtt::clean(vtt, &args.clean_vtt_opts);
    println!("{}", vtt.render());
    Ok(())
}
