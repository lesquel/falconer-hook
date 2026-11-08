//! Filesystem adapter over Codex's session rollouts, and the local UTC
//! offset. Read-only: no network, no credentials.

use std::fs;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::features::usage::application::{LogLine, UsageLogs};
use crate::features::usage::domain::parse_utc_offset;
use crate::shared::domain::command::CommandRunner;

/// Longer lines are skipped without being buffered.
pub const MAX_LINE: usize = 16 * 1024 * 1024;
/// Bytes read from the start and from the end of a rollout.
pub const CHUNK: usize = 256 * 1024;
/// Rollouts read per run, newest first.
pub const MAX_FILES: usize = 200;
const MAX_DEPTH: usize = 8;

/// `~/.codex/sessions`. Symlinks inside it are never followed, so a scan
/// cannot leave that directory. A rollout reads at most `2 * chunk` bytes:
/// its head (session metadata, model) and its tail (the last running
/// total and rate limits), never its middle.
pub struct FsUsageLogs {
    pub codex: PathBuf,
    pub max_line: usize,
    pub chunk: usize,
    pub max_files: usize,
}

impl FsUsageLogs {
    pub fn new(home: &Path) -> Self {
        Self {
            codex: home.join(".codex/sessions"),
            max_line: MAX_LINE,
            chunk: CHUNK,
            max_files: MAX_FILES,
        }
    }

    fn visit_file(&self, f: &Path, visit: &mut dyn FnMut(LogLine<'_>)) {
        let Ok(mut file) = fs::File::open(f) else {
            return;
        };
        let Ok(len) = file.metadata().map(|m| m.len()) else {
            return;
        };
        visit(LogLine::File);
        let emit = &mut |l: &str| visit(LogLine::Line(l));
        let chunk = self.chunk as u64;
        if len <= 2 * chunk {
            for_each_line(BufReader::new(file.take(2 * chunk)), self.max_line, emit);
            return;
        }
        let mut buf = Vec::with_capacity(self.chunk);
        if (&mut file).take(chunk).read_to_end(&mut buf).is_ok() {
            // Only whole lines: drop what the chunk cut.
            let end = buf.iter().rposition(|&b| b == b'\n').map_or(0, |i| i + 1);
            for_each_line(&buf[..end], self.max_line, emit);
        }
        buf.clear();
        if file.seek(SeekFrom::End(-(chunk as i64))).is_ok()
            && file.take(chunk).read_to_end(&mut buf).is_ok()
        {
            let start = buf
                .iter()
                .position(|&b| b == b'\n')
                .map_or(buf.len(), |i| i + 1);
            for_each_line(&buf[start..], self.max_line, emit);
        }
    }
}

impl UsageLogs for FsUsageLogs {
    fn codex(&self, modified_since: u64, visit: &mut dyn FnMut(LogLine<'_>)) -> bool {
        if !self.codex.is_dir() {
            return false;
        }
        let mut files = Vec::new();
        collect(&self.codex, 0, modified_since, &mut files);
        files.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
        files.truncate(self.max_files);
        for (_, f) in files {
            self.visit_file(&f, visit);
        }
        true
    }
}

/// `*.jsonl` regular files under `dir` modified at or after `since`, with
/// their modification time.
fn collect(dir: &Path, depth: usize, since: u64, out: &mut Vec<(u64, PathBuf)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        // `file_type` does not follow symlinks; symlinks are skipped.
        let Ok(t) = e.file_type() else { continue };
        let path = e.path();
        if t.is_dir() && depth < MAX_DEPTH {
            collect(&path, depth + 1, since, out);
        } else if t.is_file() && path.extension().is_some_and(|x| x == "jsonl") {
            let modified = e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs());
            if modified >= since {
                out.push((modified, path));
            }
        }
    }
}

/// Calls `f` with every UTF-8 line of at most `max` bytes; longer and
/// non-UTF-8 lines are dropped. Read errors end the file.
fn for_each_line(mut r: impl BufRead, max: usize, f: &mut dyn FnMut(&str)) {
    let mut buf: Vec<u8> = Vec::new();
    let mut too_long = false;
    let mut emit = |buf: &mut Vec<u8>, too_long: bool| {
        if !too_long {
            if let Ok(s) = std::str::from_utf8(buf) {
                f(s.trim_end_matches('\r'));
            }
        }
        buf.clear();
    };
    loop {
        let chunk = match r.fill_buf() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        if chunk.is_empty() {
            if !buf.is_empty() {
                emit(&mut buf, too_long);
            }
            break;
        }
        let (take, end) = match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => (i, true),
            None => (chunk.len(), false),
        };
        if !too_long {
            if buf.len() + take > max {
                too_long = true;
                buf.clear();
            } else {
                buf.extend_from_slice(&chunk[..take]);
            }
        }
        r.consume(take + usize::from(end));
        if end {
            emit(&mut buf, too_long);
            too_long = false;
        }
    }
}

/// Seconds east of UTC from `date +%z`; 0 (UTC) when it cannot be read.
pub fn local_utc_offset(runner: &dyn CommandRunner) -> i64 {
    runner
        .run("date", &["+%z".to_string()])
        .ok()
        .filter(|o| o.success())
        .and_then(|o| parse_utc_offset(&o.stdout))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn lines_of(logs: &FsUsageLogs, since: u64) -> (bool, Vec<String>) {
        let mut v = Vec::new();
        let found = logs.codex(since, &mut |l| match l {
            LogLine::File => v.push("<file>".into()),
            LogLine::Line(s) => v.push(s.to_string()),
        });
        (found, v)
    }

    fn touch(p: &Path, body: &str, age_secs: u64) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
        let t = SystemTime::now() - Duration::from_secs(age_secs);
        fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    fn week_ago() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 7 * 86_400
    }

    #[test]
    fn scans_only_recent_jsonl_files_and_never_follows_symlinks() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let logs = FsUsageLogs::new(home.path());
        let root = &logs.codex;
        touch(&root.join("2026/new.jsonl"), "a\nb", 0);
        touch(&root.join("2026/11/08/older.jsonl"), "c\n", 86_400);
        touch(&root.join("2026/old.jsonl"), "old\n", 30 * 86_400);
        touch(&root.join("2026/notes.txt"), "txt\n", 0);
        touch(&outside.path().join("x.jsonl"), "outside\n", 0);
        std::os::unix::fs::symlink(outside.path().join("x.jsonl"), root.join("2026/link.jsonl"))
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("linkdir")).unwrap();
        let (found, lines) = lines_of(&logs, week_ago());
        assert!(found);
        assert_eq!(lines, ["<file>", "a", "b", "<file>", "c"]);
        let empty = tempfile::tempdir().unwrap();
        let (found, lines) = lines_of(&FsUsageLogs::new(empty.path()), 0);
        assert!(!found && lines.is_empty());
    }

    #[test]
    fn a_big_rollout_is_read_by_its_head_and_tail_only() {
        let home = tempfile::tempdir().unwrap();
        let mut logs = FsUsageLogs::new(home.path());
        logs.chunk = 16;
        // 0..=16 head (cut at "headcut"), 17.. middle, last 16 bytes tail.
        let body =
            "head1\nhead2\nheadcut-middle\nMIDDLE-1\nMIDDLE-2\nMIDDLE-3\ncut-t1\ntail2\ntail3";
        touch(&logs.codex.join("rollout-big.jsonl"), body, 0);
        let (_, lines) = lines_of(&logs, week_ago());
        assert_eq!(lines, ["<file>", "head1", "head2", "tail2", "tail3"]);
        // At most two chunks: read whole.
        let body = "a\nbbbbbbbbbbbbbbbbbbbbbbbbb\nc";
        touch(&logs.codex.join("rollout-big.jsonl"), body, 0);
        let (_, lines) = lines_of(&logs, week_ago());
        assert_eq!(lines, ["<file>", "a", "bbbbbbbbbbbbbbbbbbbbbbbbb", "c"]);
    }

    #[test]
    fn only_the_newest_files_up_to_the_cap_are_read() {
        let home = tempfile::tempdir().unwrap();
        let mut logs = FsUsageLogs::new(home.path());
        logs.max_files = 2;
        touch(&logs.codex.join("a/oldest.jsonl"), "oldest", 300);
        touch(&logs.codex.join("b/newest.jsonl"), "newest", 0);
        touch(&logs.codex.join("c/middle.jsonl"), "middle", 100);
        let (_, lines) = lines_of(&logs, week_ago());
        assert_eq!(lines, ["<file>", "newest", "<file>", "middle"]);
    }

    #[test]
    fn overlong_and_non_utf8_lines_are_dropped() {
        let mut out = Vec::new();
        let input: &[u8] = b"short\r\n0123456789abc\nok\n\xff\xfe\nlast";
        let r = BufReader::with_capacity(4, input);
        for_each_line(r, 10, &mut |l| out.push(l.to_string()));
        assert_eq!(out, ["short", "ok", "last"]);
    }

    #[test]
    fn utc_offset_comes_from_date_or_defaults_to_utc() {
        use crate::shared::domain::command::fakes::RecordingRunner;
        use crate::shared::domain::command::CommandError;
        let r = RecordingRunner::default();
        r.reply_ok("-0300\n");
        assert_eq!(local_utc_offset(&r), -10_800);
        assert_eq!(r.argv(), [["date", "+%z"]]);
        r.reply(Err(CommandError::NotFound("date".into())));
        assert_eq!(local_utc_offset(&r), 0);
    }
}
