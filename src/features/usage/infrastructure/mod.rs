//! Filesystem adapter over the agents' log directories, and the local UTC
//! offset. Read-only: no network, no credentials.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::features::usage::application::{LogAgent, LogLine, UsageLogs};
use crate::features::usage::domain::parse_utc_offset;
use crate::shared::domain::command::CommandRunner;

/// Longer lines (huge tool outputs) are skipped without being buffered.
pub const MAX_LINE: usize = 16 * 1024 * 1024;
const MAX_DEPTH: usize = 8;

/// `~/.claude/projects` and `~/.codex/sessions`. Symlinks inside them are
/// never followed, so a scan cannot leave those directories.
pub struct FsUsageLogs {
    pub claude: PathBuf,
    pub codex: PathBuf,
    pub max_line: usize,
}

impl FsUsageLogs {
    pub fn new(home: &Path) -> Self {
        Self {
            claude: home.join(".claude/projects"),
            codex: home.join(".codex/sessions"),
            max_line: MAX_LINE,
        }
    }
}

impl UsageLogs for FsUsageLogs {
    fn scan(
        &self,
        agent: LogAgent,
        modified_since: u64,
        visit: &mut dyn FnMut(LogLine<'_>),
    ) -> bool {
        let root = match agent {
            LogAgent::Claude => &self.claude,
            LogAgent::Codex => &self.codex,
        };
        if !root.is_dir() {
            return false;
        }
        let mut files = Vec::new();
        collect(root, 0, modified_since, &mut files);
        files.sort();
        for f in files {
            let Ok(file) = fs::File::open(&f) else {
                continue;
            };
            visit(LogLine::File);
            for_each_line(BufReader::new(file), self.max_line, &mut |l| {
                visit(LogLine::Line(l))
            });
        }
        true
    }
}

/// `*.jsonl` regular files under `dir` modified at or after `since`.
fn collect(dir: &Path, depth: usize, since: u64, out: &mut Vec<PathBuf>) {
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
                out.push(path);
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

    fn lines_of(logs: &FsUsageLogs, agent: LogAgent, since: u64) -> (bool, Vec<String>) {
        let mut v = Vec::new();
        let found = logs.scan(agent, since, &mut |l| match l {
            LogLine::File => v.push("<file>".into()),
            LogLine::Line(s) => v.push(s.to_string()),
        });
        (found, v)
    }

    fn touch(p: &Path, body: &str, age_days: u64) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
        let t = SystemTime::now() - Duration::from_secs(age_days * 86_400);
        fs::File::options()
            .write(true)
            .open(p)
            .unwrap()
            .set_modified(t)
            .unwrap();
    }

    #[test]
    fn scans_only_recent_jsonl_files_and_never_follows_symlinks() {
        let home = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let logs = FsUsageLogs::new(home.path());
        let root = &logs.claude;
        touch(&root.join("p/new.jsonl"), "a\nb", 0);
        touch(&root.join("p/s/subagents/agent.jsonl"), "c\n", 1);
        touch(&root.join("p/old.jsonl"), "old\n", 30);
        touch(&root.join("p/notes.txt"), "txt\n", 0);
        touch(&outside.path().join("x.jsonl"), "outside\n", 0);
        std::os::unix::fs::symlink(outside.path().join("x.jsonl"), root.join("p/link.jsonl"))
            .unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("linkdir")).unwrap();
        let since = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - 7 * 86_400;
        let (found, lines) = lines_of(&logs, LogAgent::Claude, since);
        assert!(found);
        assert_eq!(lines, ["<file>", "a", "b", "<file>", "c"]);
        let (found, lines) = lines_of(&logs, LogAgent::Codex, since);
        assert!(!found && lines.is_empty());
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
