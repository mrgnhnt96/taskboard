//! Splitting a file several tasks edit in the shared checkout by who made each of its hunks, for
//! `tb slot claim --bring`. The hooks keep each edit's patch in `hunks/<task>` (Claude's own
//! `structuredPatch`, one `{"file", "patch"}` line per edit); a hunk of the file against HEAD is a
//! task's when lines that task's patches wrote, found again in the file, fall in it. A file both
//! sides are built from is HEAD with some of those hunks put in, so the others are never touched.

use std::collections::HashSet;
use std::path::Path;

use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;

static HEADER_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^@@ -(\d+)(?:,(\d+))? \+(\d+)(?:,(\d+))? @@").unwrap());

/// One uncommitted hunk of a file: the lines it replaces in HEAD and the ones it has instead in the
/// working copy, each as (first line's index, count), and a short id, unique in the file, that stays
/// the same while the hunk does.
#[derive(Debug, Clone, PartialEq)]
pub struct Hunk {
    pub old: (usize, usize),
    pub new: (usize, usize),
    pub id: String,
}

impl Hunk {
    /// The working copy's line it starts at, from 1.
    pub fn line(&self) -> usize {
        self.new.0 + 1
    }
}

/// A text's lines, each with its line end.
pub fn lines(text: &str) -> Vec<&str> {
    text.split_inclusive('\n').collect()
}

fn bare(l: &str) -> &str {
    l.trim_end_matches('\n').trim_end_matches('\r')
}

/// The hunks of `git diff -U0` for `file`, from HEAD's text (`head`) to the working copy's (`work`).
pub fn parse(file: &str, head: &[&str], work: &[&str], diff: &str) -> Vec<Hunk> {
    let n = |m: Option<regex::Match>| m.map_or(1, |m| m.as_str().parse().unwrap_or(1));
    let at = |start: usize, count: usize| if count == 0 { start } else { start.saturating_sub(1) };
    HEADER_RE
        .captures_iter(diff)
        .map(|c| {
            let (a, b, x, y) = (n(c.get(1)), n(c.get(2)), n(c.get(3)), n(c.get(4)));
            let (old, new) = ((at(a, b), b), (at(x, y), y));
            // Where it sits in HEAD keeps two hunks with the same lines apart; edits elsewhere don't move it.
            let mut sum = crc32fast::Hasher::new();
            sum.update(format!("{file}:{}", old.0).as_bytes());
            for l in head.iter().skip(old.0).take(old.1) {
                sum.update(format!("-{l}").as_bytes());
            }
            for l in work.iter().skip(new.0).take(new.1) {
                sum.update(format!("+{l}").as_bytes());
            }
            Hunk { old, new, id: format!("{:08x}", sum.finalize())[..6].to_string() }
        })
        .collect()
}

/// Where a task's recorded edits sit in the working copy: the lines they wrote, and the places
/// between lines where they took some out.
#[derive(Debug, Default)]
pub struct Marks {
    lines: HashSet<usize>,
    points: HashSet<usize>,
}

impl Marks {
    /// The patches recorded for `file` (absolute, through the real path of its folder) in a task's
    /// `hunks/<task>` text, found in `work`. None when the log has none for it: a task from before
    /// patches were kept, or one whose edits the log never saw.
    pub fn of(log: &str, file: &str, work: &[&str]) -> Option<Marks> {
        let mut m = Marks::default();
        let mut any = false;
        for rec in log.lines().filter_map(|l| serde_json::from_str::<Value>(l).ok()) {
            if rec["file"].as_str().map(real_file).as_deref() != Some(file) {
                continue;
            }
            any = true;
            for h in rec["patch"].as_array().into_iter().flatten() {
                m.find(h, work);
            }
        }
        any.then_some(m)
    }

    /// Finds one recorded hunk's new side (its context and added lines) in `work`, nearest where it
    /// was written, and marks its added lines and the places it removed lines from.
    fn find(&mut self, h: &Value, work: &[&str]) {
        let mut block: Vec<&str> = vec![];
        let (mut added, mut removed) = (vec![], vec![]);
        for l in h["lines"].as_array().into_iter().flatten().filter_map(|l| l.as_str()) {
            match l.chars().next() {
                Some('+') => {
                    added.push(block.len());
                    block.push(&l[1..]);
                }
                Some('-') => removed.push(block.len()),
                Some(' ') => block.push(&l[1..]),
                _ => {}
            }
        }
        let want = h["newStart"].as_u64().unwrap_or(1).saturating_sub(1) as usize;
        if let Some(s) = nearest(&block, work, want) {
            self.lines.extend(added.iter().map(|i| s + i));
            self.points.extend(removed.iter().map(|i| s + i));
            return;
        }
        // A later edit (another task's, say) changed its context: find each run of lines it added
        // on its own, so a hunk both made is still seen as this one's too.
        for run in added.chunk_by(|a, b| a + 1 == *b) {
            let lines: Vec<&str> = run.iter().map(|i| block[*i]).collect();
            if let Some(s) = nearest(&lines, work, want + run[0]) {
                self.lines.extend(s..s + run.len());
            }
        }
    }

    /// Whether these edits made (some of) `h`.
    pub fn made(&self, h: &Hunk) -> bool {
        let (start, len) = h.new;
        (start..start + len).any(|i| self.lines.contains(&i)) || (start..=start + len).any(|i| self.points.contains(&i))
    }
}

/// Where `block` sits in `work`, nearest line `want`.
fn nearest(block: &[&str], work: &[&str], want: usize) -> Option<usize> {
    if block.is_empty() || block.len() > work.len() {
        return None;
    }
    (0..=work.len() - block.len()).filter(|s| block.iter().enumerate().all(|(i, b)| bare(b) == bare(work[s + i]))).min_by_key(|s| s.abs_diff(want))
}

/// `file` through the real path of its folder, as `rel_in` names the checkout's files.
fn real_file(file: &str) -> String {
    let p = Path::new(file);
    match (p.parent().and_then(|d| d.canonicalize().ok()), p.file_name()) {
        (Some(d), Some(n)) => d.join(n).to_string_lossy().to_string(),
        _ => file.to_string(),
    }
}

/// HEAD's text with the hunks `take` picks put in, and every other line as HEAD has it.
pub fn build(head: &[&str], work: &[&str], hunks: &[Hunk], take: impl Fn(&Hunk) -> bool) -> String {
    let mut out = String::new();
    let mut at = 0;
    for h in hunks {
        out.extend(head[at.min(h.old.0)..h.old.0].iter().copied());
        let from = if take(h) { &work[h.new.0..h.new.0 + h.new.1] } else { &head[h.old.0..h.old.0 + h.old.1] };
        out.extend(from.iter().copied());
        at = h.old.0 + h.old.1;
    }
    out.extend(head[at.min(head.len())..].iter().copied());
    out
}

/// A hunk's first changed line, short, to tell it by.
pub fn preview(h: &Hunk, head: &[&str], work: &[&str]) -> String {
    let (sign, l) = match work.get(h.new.0).filter(|_| h.new.1 > 0) {
        Some(l) => ('+', *l),
        None => ('-', head.get(h.old.0).copied().unwrap_or("")),
    };
    let l = bare(l).trim();
    let short: String = l.chars().take(50).collect();
    format!("{sign}{short}{}", if short.len() < l.len() { "…" } else { "" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HEAD: &str = "a\nb\nc\nd\ne\nf\ng\nh\n";

    fn split(work: &str, diff: &str) -> (Vec<&'static str>, Vec<Hunk>) {
        let head: Vec<&str> = lines(HEAD);
        let w = lines(work);
        (head.clone(), parse("f.txt", &head, &w, diff))
    }

    #[test]
    fn hunks_read_from_a_zero_context_diff() {
        let work = "a\nB\nc\nd\ne\nf\nnew\ng\nh\n";
        let diff = "@@ -2 +2 @@\n-b\n+B\n@@ -6,0 +7 @@\n+new\n@@ -8,0 +10,0 @@\n";
        let (_, h) = split(work, diff);
        assert_eq!((h[0].old, h[0].new), ((1, 1), (1, 1)));
        assert_eq!((h[1].old, h[1].new), ((6, 0), (6, 1)), "an insertion after line 6");
        assert_eq!(h[0].id.len(), 6);
        assert_ne!(h[0].id, h[1].id);
    }

    #[test]
    fn each_task_keeps_its_own_hunks() {
        let work = "a\nB\nc\nd\ne\nf\nG\nh\n";
        let w = lines(work);
        let (head, hunks) = split(work, "@@ -2 +2 @@\n-b\n+B\n@@ -7 +7 @@\n-g\n+G\n");
        let mine = json!({"file": "/nowhere/f.txt", "patch": [{"newStart": 1, "lines": [" a", "-b", "+B", " c", " d"]}]}).to_string();
        let theirs = json!({"file": "/nowhere/f.txt", "patch": [{"newStart": 6, "lines": [" e", " f", "-g", "+G", " h"]}]}).to_string();
        let me = Marks::of(&mine, "/nowhere/f.txt", &w).unwrap();
        let them = Marks::of(&theirs, "/nowhere/f.txt", &w).unwrap();
        assert_eq!((me.made(&hunks[0]), me.made(&hunks[1])), (true, false));
        assert_eq!((them.made(&hunks[0]), them.made(&hunks[1])), (false, true));
        assert_eq!(build(&head, &w, &hunks, |h| me.made(h)), "a\nB\nc\nd\ne\nf\ng\nh\n");
        assert_eq!(build(&head, &w, &hunks, |h| !me.made(h)), "a\nb\nc\nd\ne\nf\nG\nh\n");
        assert!(Marks::of(&mine, "/nowhere/other.txt", &w).is_none(), "no record for another file");
        assert!(Marks::of("", "/nowhere/f.txt", &w).is_none());
    }

    #[test]
    fn a_removal_marks_where_it_was() {
        let work = "a\nb\nd\ne\nf\ng\nh\n";
        let w = lines(work);
        let (_, hunks) = split(work, "@@ -3 +2,0 @@\n-c\n");
        assert_eq!(hunks[0].new, (2, 0));
        let mine = json!({"file": "/x/f.txt", "patch": [{"newStart": 1, "lines": [" a", " b", "-c", " d", " e"]}]}).to_string();
        assert!(Marks::of(&mine, "/x/f.txt", &w).unwrap().made(&hunks[0]));
    }

    #[test]
    fn added_lines_are_found_when_their_context_changed_since() {
        let work = "a\nB\nC\nd\ne\nf\ng\nh\n";
        let w = lines(work);
        let (_, hunks) = split(work, "@@ -2,2 +2,2 @@\n-b\n-c\n+B\n+C\n");
        // Written when line 3 was still "c".
        let mine = json!({"file": "/x/f.txt", "patch": [{"newStart": 1, "lines": [" a", "-b", "+B", " c", " d"]}]}).to_string();
        assert!(Marks::of(&mine, "/x/f.txt", &w).unwrap().made(&hunks[0]));
    }

    #[test]
    fn a_patch_whose_lines_are_gone_marks_nothing() {
        let w = lines("a\nb\n");
        let gone = json!({"file": "/x/f.txt", "patch": [{"newStart": 1, "lines": [" a", "+zzz"]}]}).to_string();
        let m = Marks::of(&gone, "/x/f.txt", &w).unwrap();
        assert!(!m.made(&Hunk { old: (0, 2), new: (0, 2), id: String::new() }));
    }

    #[test]
    fn a_file_without_a_last_line_end_builds_whole() {
        let head = lines("a\nb");
        let work = lines("a\nb\nc\n");
        let hunks = parse("f", &head, &work, "@@ -2 +2,2 @@\n-b\n\\ No newline at end of file\n+b\n+c\n");
        assert_eq!(build(&head, &work, &hunks, |_| true), "a\nb\nc\n");
        assert_eq!(build(&head, &work, &hunks, |_| false), "a\nb");
    }
}
