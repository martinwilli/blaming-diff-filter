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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    const PATCH: &str = r"diff --git a/tests/bar.txt b/tests/bar.txt
index 6d0a9487a999..5aa46cc774fb 10064
--- a/tests/bar.txt
+++ b/tests/bar.txt
@@ -1,10 +1,10 @@
-bar
+barbara
 0.5
 1
 2
 3
 foobar
 bar ba baz
-a
-b
+A
+B
 C
diff --git a/tests/foo.txt b/tests/foo.txt
index 06259808ba40..482e77c74da8 100644
--- a/tests/foo.txt
+++ b/tests/foo.txt
@@ -1,5 +1,5 @@
 foo
-bar
+baz
 a
 b
 c
@@ -7,7 +7,7 @@ d
 +
 -
 +++
-extra
+wtextra
 bla
 ---
 @@ foo
@@ -17,7 +17,7 @@ bar
 3
 4
 5
-6
+5z
 6a
 7
 8
@@ -25,4 +25,3 @@ bar
 10
 11
 12
-13
";

    #[test]
    fn test_parse_hunk() {
        let mut annotator = DiffAnnotator::new(None, None);
        let line = "@@ -36,7 +36,7 @@";
        let end = annotator.parse_hunk(line);
        assert_eq!(annotator.start, 36);
        assert_eq!(end, 43);
    }

    #[test]
    fn test_annotate_diff() {
        let mut annotator = DiffAnnotator::new(None, None);

        let reader = Cursor::new(PATCH);
        let mut writer = Vec::new();
        let result = annotator.annotate_diff(reader, &mut writer);
        assert!(result.is_ok());
        assert_eq!(
            String::from_utf8(writer).unwrap(),
            r"diff --git a/tests/bar.txt b/tests/bar.txt
index 6d0a9487a999..5aa46cc774fb 10064
--- a/tests/bar.txt
+++ b/tests/bar.txt
@@ -1,10 +1,10 @@
b40c1d -bar
++++++ +barbara
6ec7db  0.5
b40c1d  1
b40c1d  2
b40c1d  3
6ec7db  foobar
6ec7db  bar ba baz
b40c1d -a
b40c1d -b
++++++ +A
++++++ +B
6ec7db  C
diff --git a/tests/foo.txt b/tests/foo.txt
index 06259808ba40..482e77c74da8 100644
--- a/tests/foo.txt
+++ b/tests/foo.txt
@@ -1,5 +1,5 @@
b40c1d  foo
b40c1d -bar
++++++ +baz
b40c1d  a
b40c1d  b
b40c1d  c
@@ -7,7 +7,7 @@ d
b40c1d  +
b40c1d  -
b40c1d  +++
b40c1d -extra
++++++ +wtextra
b40c1d  bla
b40c1d  ---
b40c1d  @@ foo
@@ -17,7 +17,7 @@ bar
b40c1d  3
b40c1d  4
b40c1d  5
b40c1d -6
++++++ +5z
6ec7db  6a
b40c1d  7
b40c1d  8
@@ -25,4 +25,3 @@ bar
b40c1d  10
b40c1d  11
b40c1d  12
6ec7db -13
"
        );
    }

    #[test]
    fn test_annotate_inner() {
        let inner = vec![
            "tr".to_string(),
            "[:lower:]".to_string(),
            "[:upper:]".to_string(),
        ];
        let mut annotator = DiffAnnotator::new(Some(inner), None);

        let reader = Cursor::new(PATCH);
        let mut writer = Vec::new();
        let result = annotator.annotate_diff(reader, &mut writer);
        assert!(result.is_ok());
        assert_eq!(
            String::from_utf8(writer).unwrap(),
            r"DIFF --GIT A/TESTS/BAR.TXT B/TESTS/BAR.TXT
INDEX 6D0A9487A999..5AA46CC774FB 10064
--- A/TESTS/BAR.TXT
+++ B/TESTS/BAR.TXT
@@ -1,10 +1,10 @@
b40c1d -BAR
++++++ +BARBARA
6ec7db  0.5
b40c1d  1
b40c1d  2
b40c1d  3
6ec7db  FOOBAR
6ec7db  BAR BA BAZ
b40c1d -A
b40c1d -B
++++++ +A
++++++ +B
6ec7db  C
DIFF --GIT A/TESTS/FOO.TXT B/TESTS/FOO.TXT
INDEX 06259808BA40..482E77C74DA8 100644
--- A/TESTS/FOO.TXT
+++ B/TESTS/FOO.TXT
@@ -1,5 +1,5 @@
b40c1d  FOO
b40c1d -BAR
++++++ +BAZ
b40c1d  A
b40c1d  B
b40c1d  C
@@ -7,7 +7,7 @@ D
b40c1d  +
b40c1d  -
b40c1d  +++
b40c1d -EXTRA
++++++ +WTEXTRA
b40c1d  BLA
b40c1d  ---
b40c1d  @@ FOO
@@ -17,7 +17,7 @@ BAR
b40c1d  3
b40c1d  4
b40c1d  5
b40c1d -6
++++++ +5Z
6ec7db  6A
b40c1d  7
b40c1d  8
@@ -25,4 +25,3 @@ BAR
b40c1d  10
b40c1d  11
b40c1d  12
6ec7db -13
"
        );
    }

    #[test]
    fn test_annotate_backto() {
        let mut annotator = DiffAnnotator::new(None, Some("b40c1dbc28".to_string()));

        let reader = Cursor::new(PATCH);
        let mut writer = Vec::new();
        let result = annotator.annotate_diff(reader, &mut writer);
        assert!(result.is_ok());
        assert_eq!(
            String::from_utf8(writer).unwrap(),
            r"diff --git a/tests/bar.txt b/tests/bar.txt
index 6d0a9487a999..5aa46cc774fb 10064
--- a/tests/bar.txt
+++ b/tests/bar.txt
@@ -1,10 +1,10 @@
······ -bar
++++++ +barbara
6ec7db  0.5
······  1
······  2
······  3
6ec7db  foobar
6ec7db  bar ba baz
······ -a
······ -b
++++++ +A
++++++ +B
6ec7db  C
diff --git a/tests/foo.txt b/tests/foo.txt
index 06259808ba40..482e77c74da8 100644
--- a/tests/foo.txt
+++ b/tests/foo.txt
@@ -1,5 +1,5 @@
······  foo
······ -bar
++++++ +baz
······  a
······  b
······  c
@@ -7,7 +7,7 @@ d
······  +
······  -
······  +++
······ -extra
++++++ +wtextra
······  bla
······  ---
······  @@ foo
@@ -17,7 +17,7 @@ bar
······  3
······  4
······  5
······ -6
++++++ +5z
6ec7db  6a
······  7
······  8
@@ -25,4 +25,3 @@ bar
······  10
······  11
······  12
6ec7db -13
"
        );
    }
}
