use std::io::BufReader;
use std::io::{self, BufRead, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::ScopedJoinHandle;

pub struct DiffAnnotator {
    inner: Option<Vec<String>>,
    rev: String,
    commits: Vec<String>,
    file: String,
    start: u32,
    offset: u32,
    maxlen: usize,
}

impl DiffAnnotator {
    const ABBREV: usize = 6;

    pub fn new(inner: Option<Vec<String>>, back_to: Option<String>) -> Self {
        DiffAnnotator {
            inner,
            rev: Self::make_blame_rev(back_to),
            commits: Vec::new(),
            file: String::new(),
            start: 0,
            offset: 0,
            maxlen: 0,
        }
    }

    fn rev_parse(rev: &str) -> String {
        let output = Command::new("git")
            .arg("rev-parse")
            .arg(rev)
            .output()
            .expect(format!("git rev-parse for {rev} failed").as_str());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn make_blame_rev(back_to: Option<String>) -> String {
        if let Some(back_to) = back_to {
            if Self::rev_parse(&back_to) == Self::rev_parse("HEAD") {
                // ignore when currently on --back-to branch
                return "HEAD".to_string();
            }
            let output = Command::new("git")
                .arg("merge-base")
                .arg("HEAD")
                .arg(&back_to)
                .output()
                .expect(format!("git merge-base for {back_to} failed").as_str());
            return format!("{}..", String::from_utf8_lossy(&output.stdout).trim());
        }
        "HEAD".to_string()
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
            .arg(&self.rev)
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
                if commit.starts_with('^') || commit.chars().all(|c| c == '0') {
                    Ok(Some(format!("{} ", "·".repeat(self.maxlen))))
                } else {
                    Ok(Some(format!("{} ", commit)))
                }
            } else {
                self.offset += 1;
                Ok(Some(format!("{} ", "?".repeat(self.maxlen))))
            }
        } else if line.starts_with('+') {
            Ok(Some(format!("{} ", "+".repeat(self.maxlen))))
        } else {
            Ok(None)
        }
    }

    fn wrapping_diff<R: BufRead, W: Write + Sync + Send>(
        &mut self,
        reader: R,
        mut writer: W,
    ) -> io::Result<()> {
        if let Some(inner) = &self.inner {
            let cmd = Command::new(&inner[0])
                .args(&inner[1..])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .spawn()
                .map_err(|e| io::Error::new(e.kind(), format!("Inner cmd: {}", inner[0])))?;

            let pfx_write: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
            let pfx_read = pfx_write.clone();

            let stdout = BufReader::new(cmd.stdout.unwrap());
            let mut stdin = cmd.stdin.unwrap();

            std::thread::scope(|s| {
                let t: ScopedJoinHandle<io::Result<()>> = s.spawn(move || {
                    for line in stdout.lines() {
                        if let Some(pfx) = pfx_read.lock().unwrap().remove(0) {
                            write!(writer, "{}", pfx)?;
                        }
                        writeln!(writer, "{}", line?)?;
                    }
                    Ok(())
                });
                for line in reader.lines() {
                    let line = line?;
                    writeln!(stdin, "{}", line)?;
                    pfx_write.lock().unwrap().push(self.process_line(&line)?);
                }
                stdin.flush()?;
                drop(stdin);
                t.join().unwrap()
            })?;
        }
        return Ok(());
    }

    pub fn annotate_diff<R: BufRead, W: Write + Sync + Send>(
        &mut self,
        reader: R,
        mut writer: W,
    ) -> io::Result<()> {
        if self.inner.is_some() {
            return self.wrapping_diff(reader, writer);
        }
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
