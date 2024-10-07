use blaming_diff_filter::annotate::DiffAnnotator;
use clap::{command, Parser};
use std::io;

/// git diffFilter annotating each line with originating commit-id.
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    /// Inner diff filter to run.
    inner: Option<Vec<String>>,
}

fn main() -> io::Result<()> {
    let args = Args::parse();
    let mut annotator = DiffAnnotator::new(args.inner);
    annotator.annotate_diff(io::stdin().lock(), io::stdout())
}
