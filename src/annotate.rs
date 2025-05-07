use std::collections::HashSet;
use std::io::BufReader;
use std::io::{self, BufRead, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread::ScopedJoinHandle;

/// Annotate each line of a diff with the commit-id that last touched it.
///
/// The `DiffAnnotator` is used to annotate each line of a diff with the commit-id that last
/// touched it. It can use an inner diff filter to process the diff output before annotating it.
/// The `back_to` option can be used to blame up to a common ancestor.
pub struct DiffAnnotator {
    inner: Option<Vec<String>>,
    rev: String,
    format: Option<String>,
    commits: Vec<String>,
    candidates: HashSet<String>,
    file: Option<String>,
    start: u32,
    offset: u32,
    maxlen: usize,
}

impl DiffAnnotator {
    const ABBREV: usize = 6;

    /// Create a new `DiffAnnotator`.
    ///
    /// * `inner` - An optional inner diff filter to process the diff output before annotating it.
    /// * `back_to` - An optional commit-id to blame up to a common ancestor.
    pub fn new(
        inner: Option<Vec<String>>,
        back_to: Option<Vec<String>>,
        format: Option<String>,
    ) -> io::Result<Self> {
        Ok(DiffAnnotator {
            inner,
            rev: Self::make_blame_rev(back_to)?,
            format,
            commits: Vec::new(),
            candidates: HashSet::new(),
            file: None,
            start: 0,
            offset: 0,
            maxlen: 0,
        })
    }

    fn check_output(cmd: &mut Command) -> io::Result<String> {
        let desc = format!("{cmd:?}");
        let output = cmd
            .output()
            .map_err(|e| io::Error::new(e.kind(), desc.clone()))?;
        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(io::Error::new(
                io::ErrorKind::Other,
                format!("{desc}: {}", String::from_utf8_lossy(&output.stderr)),
            ))
        }
    }

    fn rev_parse(rev: &str) -> io::Result<String> {
        Self::check_output(Command::new("git").arg("rev-parse").arg(rev))
    }

    fn make_blame_rev(back_to: Option<Vec<String>>) -> io::Result<String> {
        if let Some(back_to) = back_to {
            for branch in back_to {
                if let Ok(rev) = Self::rev_parse(&branch) {
                    if rev == Self::rev_parse("HEAD")? {
                        // ignore when currently on --back-to branch
                        break;
                    }
                    return Ok(Self::check_output(
                        Command::new("git")
                            .arg("merge-base")
                            .arg("HEAD")
                            .arg(&branch),
                    )? + "..");
                }
            }
        }
        Ok("HEAD".to_string())
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
        self.commits = Self::check_output(
            Command::new("git")
                .arg("blame")
                .arg(&self.rev)
                .arg(format!("--abbrev={}", Self::ABBREV - 1))
                .arg("-L")
                .arg(&format!("{},{}", self.start, end))
                .arg(self.file.as_deref().unwrap()),
        )?
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
        let line = strip_ansi_escapes::strip_str(line);
        if let Some(path) = line.strip_prefix("--- ") {
            // for new files this can be /dev/null, so ignore anything not starting with "a/"
            self.file = path.strip_prefix("a/").map(str::to_string);
            Ok(None)
        } else if line.starts_with("+++ ") {
            Ok(None)
        } else if line.starts_with("@@ ") {
            if self.file.is_some() {
                self.blame_hunk(&line)?;
            } else {
                self.commits.clear();
            }
            Ok(None)
        } else if line.starts_with(' ') || line.starts_with('-') {
            if let Some(commit) = self.lookup_commit() {
                self.offset += 1;
                if commit.starts_with('^') || commit.chars().all(|c| c == '0') {
                    Ok(Some(format!("{} ", "·".repeat(self.maxlen))))
                } else {
                    self.candidates.insert(commit.clone());
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

            let (tx, rx) = mpsc::channel::<Option<String>>();
            let stdout = BufReader::new(cmd.stdout.unwrap());
            let mut stdin = cmd.stdin.unwrap();

            std::thread::scope(|s| {
                let t: ScopedJoinHandle<io::Result<()>> = s.spawn(move || {
                    for line in stdout.lines() {
                        match rx.recv() {
                            Ok(Some(pfx)) => write!(writer, "{}", pfx)?,
                            Ok(None) => (),
                            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e)),
                        }
                        writeln!(writer, "{}", line?)?;
                    }
                    Ok(())
                });
                for line in reader.lines() {
                    let line = line?;
                    tx.send(self.process_line(&line)?)
                        .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                    writeln!(stdin, "{}", line)?;
                }
                drop(stdin);
                t.join().unwrap()
            })?;
        }
        Ok(())
    }

    fn simple_diff<R: BufRead, W: Write + Sync + Send>(
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

    /// Annotate a diff with the commit-id that last touched each line.
    ///
    /// * `reader` - A reader for the diff to annotate.
    /// * `writer` - A writer for the annotated diff.
    pub fn annotate_diff<R: BufRead, W: Write + Sync + Send, CW: Write>(
        &mut self,
        reader: R,
        writer: W,
        mut cand_writer: CW,
    ) -> io::Result<()> {
        if self.inner.is_some() {
            self.wrapping_diff(reader, writer)?;
        } else {
            self.simple_diff(reader, writer)?;
        }
        if let Some(format) = &self.format {
            let output = Self::check_output(
                Command::new("git")
                    .arg("show")
                    .arg("-s")
                    .arg("--color")
                    .arg(format!("--abbrev={}", Self::ABBREV))
                    .arg(format!("--format=%at {}", format))
                    .args(&self.candidates),
            )?;
            let mut lines: Vec<_> = output.lines().collect();
            lines.sort_by_key(|line| {
                line.split_whitespace()
                    .next()
                    .unwrap_or("0")
                    .parse::<u64>()
                    .unwrap_or(0)
            });
            for line in lines {
                let line = line
                    .split_whitespace()
                    .skip(1)
                    .collect::<Vec<_>>()
                    .join(" ");
                writeln!(cand_writer, "{}", line)?;
            }
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
        let mut annotator = DiffAnnotator::new(None, None, None).unwrap();
        let line = "@@ -36,7 +36,7 @@";
        let end = annotator.parse_hunk(line);
        assert_eq!(annotator.start, 36);
        assert_eq!(end, 43);
    }

    #[test]
    fn test_annotate_diff() {
        let mut annotator = DiffAnnotator::new(None, None, None).unwrap();

        let reader = Cursor::new(PATCH);
        let mut writer = Vec::new();
        let mut cwriter = Vec::new();
        let result = annotator.annotate_diff(reader, &mut writer, &mut cwriter);
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
        let format = "%h %s".to_string();
        let mut annotator = DiffAnnotator::new(Some(inner), None, Some(format)).unwrap();

        let reader = Cursor::new(PATCH);
        let mut writer = Vec::new();
        let mut cwriter = Vec::new();
        let result = annotator.annotate_diff(reader, &mut writer, &mut cwriter);
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
        assert_eq!(
            String::from_utf8(cwriter).unwrap(),
            r"b40c1d tests: Add some test data
6ec7db tests: Add some changes to test files for blame testing
"
        );
    }

    #[test]
    fn test_annotate_backto() {
        let backto = Some(vec!["b40c1dbc28".to_string()]);
        let mut annotator = DiffAnnotator::new(None, backto, None).unwrap();

        let reader = Cursor::new(PATCH);
        let mut writer = Vec::new();
        let mut cwriter = Vec::new();
        let result = annotator.annotate_diff(reader, &mut writer, &mut cwriter);
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
