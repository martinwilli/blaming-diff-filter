use blaming_diff_filter::annotate::DiffAnnotator;
use std::io;

fn main() -> io::Result<()> {
    let mut annotator = DiffAnnotator::new();
    annotator.annotate_diff(io::stdin().lock(), io::stdout())
}
