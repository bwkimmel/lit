use anyhow::{anyhow, Result};
use clap::Parser;
use notmecab::{Blob, Dict};

#[derive(Parser, Debug)]
struct Args {
    #[arg(help="the text to parse")]
    text: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let sysdic = Blob::open("data/mecab_ko_dic/sys.dic")?;
    let unkdic = Blob::open("data/mecab_ko_dic/unk.dic")?;
    let matrix = Blob::open("data/mecab_ko_dic/matrix.bin")?;
    let unkdef = Blob::open("data/mecab_ko_dic/char.bin")?;
    let dict = Dict::load(sysdic, unkdic, matrix, unkdef)
        .map_err(|e| anyhow!("failed to load dictionary: {e}"))?;
    let result = dict.tokenize(&args.text)?;
    for token in &result.0 {
        println!("{}", token.get_feature(&dict));
    }
    Ok(())
}
