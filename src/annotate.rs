use std::io::{self, BufRead, Write};
use std::process::Command;

pub struct DiffAnnotator {
    commits: Vec<String>,
    file: String,
    start: u32,
    offset: u32,
    maxlen: usize,
}

impl DiffAnnotator {
    const ABBREV: usize = 6;

    pub fn new() -> Self {
        DiffAnnotator {
            commits: Vec::new(),
            file: String::new(),
            start: 0,
            offset: 0,
            maxlen: 0,
        }
    }

    fn parse_hunk(&mut self, line: &str) -> u32 {
        // @@ -36,7 +36,7 @@
        let mut parts = line.split_whitespace();
        let mut old = parts.nth(1).unwrap()[1..].split(',');
        self.start = old.next().unwrap().parse::<u32>().unwrap();
        let count = old.next().unwrap().parse::<u32>().unwrap();
        self.start + count
    }

    fn blame_hunk(&mut self, header: &str) -> io::Result<()> {
        let end = self.parse_hunk(header);
        let output = Command::new("git")
            .arg("blame")
            .arg("HEAD")
            .arg(format!("--abbrev={}", Self::ABBREV - 1))
            .arg("-L")
            .arg(&format!("{},{}", self.start, end))
            .arg(&self.file)
            .output()?;
        let lines = String::from_utf8_lossy(&output.stdout);
        self.commits = lines
            .lines()
            .map(|line| line.split_whitespace().next().unwrap().to_string())
            .collect();
        self.maxlen = self.commits.iter().fold(Self::ABBREV, |acc, commit| {
            if commit.len() > acc {
                commit.len()
            } else {
                acc
            }
        });
        self.offset = self.start;
        Ok(())
    }

    fn lookup_commit(&self) -> Option<String> {
        if self.start <= self.offset && self.offset < self.start + self.commits.len() as u32 {
            return Some(self.commits[(self.offset - self.start) as usize].clone());
        }
        None
    }

    fn process_line(&mut self, line: &str) -> io::Result<Option<String>> {
        let line = strip_ansi_escapes::strip_str(&line);
        if line.starts_with("--- ") {
            self.file = line.split_whitespace().last().unwrap()[2..].to_string();
            Ok(None)
        } else if line.starts_with("+++ ") {
            Ok(None)
        } else if line.starts_with("@@ ") {
            self.blame_hunk(&line)?;
            Ok(None)
        } else if line.starts_with(' ') || line.starts_with('-') {
            if let Some(commit) = self.lookup_commit() {
                self.offset += 1;
                Ok(Some(format!("{} ", commit)))
            } else {
                self.offset += 1;
                Ok(Some(format!("{:0<width$} ", "", width = self.maxlen)))
            }
        } else if line.starts_with('+') {
            Ok(Some(format!("{:0<width$} ", "", width = self.maxlen)))
        } else {
            Ok(None)
        }
    }

    pub fn annotate_diff<R: BufRead, W: Write>(
        &mut self,
        reader: R,
        mut writer: W,
    ) -> io::Result<()> {
        for line in reader.lines() {
            let line = line?;
            if let Some(pfx) = self.process_line(&line)? {
                write!(writer, "{}", pfx)?;
            }
            writeln!(writer, "{}", line)?;
        }
        Ok(())
    }
}
