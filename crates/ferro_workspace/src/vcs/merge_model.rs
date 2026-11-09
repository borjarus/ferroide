//! The model behind the 3-way merge view: a conflicted file split into
//! common text and conflicts, each with our side, the base and their side,
//! and the user's resolution.
//!
//! It parses the diff3-style markers git writes (`<<<<<<<`, `|||||||`,
//! `=======`, `>>>>>>>`); the base section is optional (merge style).

use anyhow::{Result, bail};

/// How a conflict was resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Resolution {
    /// Our side (left).
    Ours,
    /// Their side (right).
    Theirs,
    /// Ours followed by theirs.
    Both,
    /// Neither: keep the base text.
    Base,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub ours: Vec<String>,
    pub base: Vec<String>,
    pub theirs: Vec<String>,
    pub resolution: Option<Resolution>,
}

impl Conflict {
    /// The lines the result gets, once resolved.
    pub fn resolved_lines(&self) -> Option<Vec<String>> {
        Some(match self.resolution? {
            Resolution::Ours => self.ours.clone(),
            Resolution::Theirs => self.theirs.clone(),
            Resolution::Both => self.ours.iter().chain(&self.theirs).cloned().collect(),
            Resolution::Base => self.base.clone(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Common(Vec<String>),
    Conflict(Conflict),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MergeModel {
    pub segments: Vec<Segment>,
    pub ours_label: String,
    pub theirs_label: String,
}

#[derive(PartialEq)]
enum Part {
    Common,
    Ours,
    Base,
    Theirs,
}

fn marker<'a>(line: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(prefix)?;
    (rest.is_empty() || rest.starts_with(' ')).then(|| rest.trim())
}

impl MergeModel {
    /// Parse a file with conflict markers.
    pub fn parse(text: &str) -> Result<Self> {
        let mut model = MergeModel::default();
        let mut part = Part::Common;
        let mut common: Vec<String> = Vec::new();
        let mut conflict = Conflict {
            ours: Vec::new(),
            base: Vec::new(),
            theirs: Vec::new(),
            resolution: None,
        };
        for (ix, line) in text.lines().enumerate() {
            let n = ix + 1;
            if let Some(label) = marker(line, "<<<<<<<") {
                if part != Part::Common {
                    bail!("line {n}: nested conflict marker");
                }
                if !common.is_empty() {
                    model
                        .segments
                        .push(Segment::Common(std::mem::take(&mut common)));
                }
                if model.ours_label.is_empty() {
                    model.ours_label = label.to_string();
                }
                part = Part::Ours;
            } else if marker(line, "|||||||").is_some() && part == Part::Ours {
                part = Part::Base;
            } else if marker(line, "=======").is_some()
                && (part == Part::Ours || part == Part::Base)
            {
                part = Part::Theirs;
            } else if let Some(label) = marker(line, ">>>>>>>") {
                if part != Part::Theirs {
                    bail!("line {n}: `>>>>>>>` without `=======`");
                }
                if model.theirs_label.is_empty() {
                    model.theirs_label = label.to_string();
                }
                model.segments.push(Segment::Conflict(std::mem::replace(
                    &mut conflict,
                    Conflict {
                        ours: Vec::new(),
                        base: Vec::new(),
                        theirs: Vec::new(),
                        resolution: None,
                    },
                )));
                part = Part::Common;
            } else {
                let line = line.to_string();
                match part {
                    Part::Common => common.push(line),
                    Part::Ours => conflict.ours.push(line),
                    Part::Base => conflict.base.push(line),
                    Part::Theirs => conflict.theirs.push(line),
                }
            }
        }
        if part != Part::Common {
            bail!("the file ends inside a conflict");
        }
        if !common.is_empty() {
            model.segments.push(Segment::Common(common));
        }
        Ok(model)
    }

    pub fn conflict_count(&self) -> usize {
        self.conflicts().count()
    }

    pub fn unresolved_count(&self) -> usize {
        self.conflicts().filter(|c| c.resolution.is_none()).count()
    }

    pub fn conflicts(&self) -> impl Iterator<Item = &Conflict> {
        self.segments.iter().filter_map(|s| match s {
            Segment::Conflict(c) => Some(c),
            Segment::Common(_) => None,
        })
    }

    pub fn conflict(&self, ix: usize) -> Option<&Conflict> {
        self.conflicts().nth(ix)
    }

    /// The segment index of conflict `ix`.
    pub fn segment_of(&self, ix: usize) -> Option<usize> {
        self.segments
            .iter()
            .enumerate()
            .filter(|(_, s)| matches!(s, Segment::Conflict(_)))
            .nth(ix)
            .map(|(seg, _)| seg)
    }

    /// The conflict index of segment `seg`, if it is a conflict.
    pub fn conflict_of(&self, seg: usize) -> Option<usize> {
        matches!(self.segments.get(seg), Some(Segment::Conflict(_))).then(|| {
            self.segments[..seg]
                .iter()
                .filter(|s| matches!(s, Segment::Conflict(_)))
                .count()
        })
    }

    pub fn resolve(&mut self, ix: usize, resolution: Option<Resolution>) {
        if let Some(seg) = self.segment_of(ix)
            && let Segment::Conflict(c) = &mut self.segments[seg]
        {
            c.resolution = resolution;
        }
    }

    /// The first unresolved conflict at or after `from`, wrapping around.
    pub fn next_unresolved(&self, from: usize) -> Option<usize> {
        let n = self.conflict_count();
        (0..n)
            .map(|k| (from + k) % n)
            .find(|&ix| self.conflict(ix).is_some_and(|c| c.resolution.is_none()))
    }

    fn side_text(&self, pick: impl Fn(&Conflict) -> &Vec<String>) -> String {
        let mut out = String::new();
        for segment in &self.segments {
            let lines = match segment {
                Segment::Common(lines) => lines,
                Segment::Conflict(c) => pick(c),
            };
            for line in lines {
                out.push_str(line);
                out.push('\n');
            }
        }
        out
    }

    pub fn ours_text(&self) -> String {
        self.side_text(|c| &c.ours)
    }

    pub fn theirs_text(&self) -> String {
        self.side_text(|c| &c.theirs)
    }

    pub fn base_text(&self) -> String {
        self.side_text(|c| &c.base)
    }

    /// The merged file, once every conflict is resolved.
    pub fn result_text(&self) -> Option<String> {
        let mut out = String::new();
        for segment in &self.segments {
            let lines = match segment {
                Segment::Common(lines) => lines.clone(),
                Segment::Conflict(c) => c.resolved_lines()?,
            };
            for line in lines {
                out.push_str(&line);
                out.push('\n');
            }
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEXT: &str = "a\n<<<<<<< HEAD\nours\n||||||| base\nb\n=======\ntheirs\n>>>>>>> feature\nc\n<<<<<<< HEAD\nx\n=======\ny\n>>>>>>> feature\nd\n";

    #[test]
    fn parses_diff3_and_merge_style_conflicts() {
        let model = MergeModel::parse(TEXT).unwrap();
        assert_eq!(model.conflict_count(), 2);
        assert_eq!(model.ours_label, "HEAD");
        assert_eq!(model.theirs_label, "feature");
        let first = model.conflict(0).unwrap();
        assert_eq!(first.base, vec!["b"]);
        assert!(model.conflict(1).unwrap().base.is_empty());
        assert_eq!(model.ours_text(), "a\nours\nc\nx\nd\n");
        assert_eq!(model.theirs_text(), "a\ntheirs\nc\ny\nd\n");
        assert_eq!(model.segment_of(1), Some(3));
        assert_eq!(model.conflict_of(3), Some(1));
        assert_eq!(model.conflict_of(0), None);
    }

    #[test]
    fn result_exists_only_when_everything_is_resolved() {
        let mut model = MergeModel::parse(TEXT).unwrap();
        assert_eq!(model.result_text(), None);
        model.resolve(0, Some(Resolution::Both));
        assert_eq!(model.unresolved_count(), 1);
        assert_eq!(model.next_unresolved(0), Some(1));
        model.resolve(1, Some(Resolution::Theirs));
        assert_eq!(model.next_unresolved(0), None);
        assert_eq!(model.result_text().unwrap(), "a\nours\ntheirs\nc\ny\nd\n");
        model.resolve(0, Some(Resolution::Base));
        assert_eq!(model.result_text().unwrap(), "a\nb\nc\ny\nd\n");
        model.resolve(1, None);
        assert_eq!(model.result_text(), None);
    }

    #[test]
    fn broken_markers_are_errors() {
        assert!(MergeModel::parse("<<<<<<< a\nx\n").is_err());
        assert!(MergeModel::parse("x\n>>>>>>> b\n").is_err());
        assert_eq!(MergeModel::parse("plain\n").unwrap().conflict_count(), 0);
    }
}
