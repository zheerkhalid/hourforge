//! Turns stored blocks into classified, clipped spans: the common input of
//! every total, chart, goal and export.
//!
//! [`flatten`] does four things:
//! 1. clips every block to the asked range;
//! 2. makes worked time non-overlapping: tracked app time wins over manual
//!    entries and labelled away time that overlap it, earlier manual entries
//!    win over later ones (so nothing is counted twice);
//! 3. classifies each piece (category and project, see `rules`);
//! 4. lets a running or finished timer with a project claim the worked time
//!    under it, unless the block has a project set by hand.

use crate::model::{Block, Kind};
use crate::rules::Classifier;

/// A piece of one block inside the asked range.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Span {
    pub start: i64,
    pub end: i64,
    /// Index of the block in the slice given to [`flatten`].
    pub src: u32,
    pub kind: Kind,
    /// Counted as worked time (see `Block::is_work`).
    pub work: bool,
    /// Index into `Classifier::categories`.
    pub category: u16,
    /// Index into `Classifier::projects`.
    pub project: Option<u16>,
}

impl Span {
    pub fn secs(&self) -> i64 {
        self.end - self.start
    }
}

/// Removes the sorted, disjoint intervals `taken` from `[start, end)`.
fn subtract(start: i64, end: i64, taken: &[(i64, i64)], out: &mut Vec<(i64, i64)>) {
    let mut cursor = start;
    let first = taken.partition_point(|(_, e)| *e <= start);
    for &(s, e) in &taken[first..] {
        if s >= end {
            break;
        }
        if s > cursor {
            out.push((cursor, s));
        }
        cursor = cursor.max(e);
        if cursor >= end {
            break;
        }
    }
    if cursor < end {
        out.push((cursor, end));
    }
}

/// Adds `[start, end)` to sorted, disjoint intervals, merging neighbours.
fn insert(taken: &mut Vec<(i64, i64)>, start: i64, end: i64) {
    let at = taken.partition_point(|(s, _)| *s < start);
    taken.insert(at, (start, end));
    let mut i = at.saturating_sub(1);
    while i + 1 < taken.len() {
        if taken[i].1 >= taken[i + 1].0 {
            taken[i].1 = taken[i].1.max(taken[i + 1].1);
            taken.remove(i + 1);
        } else if i > at {
            break;
        } else {
            i += 1;
        }
    }
}

/// See the module documentation. The result is sorted by start time.
pub fn flatten(blocks: &[Block], from: i64, to: i64, classifier: &Classifier) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::with_capacity(blocks.len());
    let clip = |b: &Block| {
        let (s, e) = (b.start.max(from), b.end.min(to));
        (e > s).then_some((s, e))
    };

    // Tracked app time first: it is the ground truth.
    let mut taken: Vec<(i64, i64)> = Vec::new();
    let mut order: Vec<usize> = (0..blocks.len()).collect();
    order.sort_by_key(|&i| (blocks[i].start, blocks[i].id));
    for &i in &order {
        let b = &blocks[i];
        if b.kind != Kind::Active {
            continue;
        }
        let Some((s, e)) = clip(b) else { continue };
        let mut pieces = Vec::new();
        subtract(s, e, &taken, &mut pieces);
        for (ps, pe) in pieces {
            push(&mut spans, blocks, i, ps, pe, classifier);
            insert(&mut taken, ps, pe);
        }
    }
    // Then manual entries and labelled away time, minus what is already counted.
    for &i in &order {
        let b = &blocks[i];
        if b.kind == Kind::Active {
            continue;
        }
        let Some((s, e)) = clip(b) else { continue };
        if b.is_work() {
            let mut pieces = Vec::new();
            subtract(s, e, &taken, &mut pieces);
            for (ps, pe) in pieces {
                push(&mut spans, blocks, i, ps, pe, classifier);
                insert(&mut taken, ps, pe);
            }
        } else {
            push(&mut spans, blocks, i, s, e, classifier);
        }
    }

    // Timers with a project claim the worked time under them.
    let timers: Vec<(i64, i64, u16)> = blocks
        .iter()
        .filter(|b| b.kind == Kind::Timer)
        .filter_map(|b| {
            let id = b.project.as_deref().filter(|p| !p.is_empty())?;
            let index = classifier.project_index(id)?;
            let (s, e) = clip(b)?;
            Some((s, e, index as u16))
        })
        .collect();
    if !timers.is_empty() {
        let mut out = Vec::with_capacity(spans.len() + timers.len() * 2);
        for span in spans {
            if !span.work || blocks[span.src as usize].project.is_some() {
                out.push(span);
                continue;
            }
            let mut cursor = span.start;
            let mut hits: Vec<(i64, i64, u16)> = timers
                .iter()
                .filter(|(s, e, _)| *s < span.end && *e > span.start)
                .copied()
                .collect();
            hits.sort();
            for (s, e, project) in hits {
                let (s, e) = (s.max(cursor), e.min(span.end));
                if e <= s {
                    continue;
                }
                if s > cursor {
                    out.push(Span {
                        start: cursor,
                        end: s,
                        ..span
                    });
                }
                out.push(Span {
                    start: s,
                    end: e,
                    project: Some(project),
                    ..span
                });
                cursor = e;
            }
            if cursor < span.end {
                out.push(Span {
                    start: cursor,
                    ..span
                });
            }
        }
        spans = out;
    }

    spans.sort_by_key(|s| (s.start, s.src));
    spans
}

fn push(
    spans: &mut Vec<Span>,
    blocks: &[Block],
    index: usize,
    start: i64,
    end: i64,
    classifier: &Classifier,
) {
    let b = &blocks[index];
    let class = classifier.classify(b);
    spans.push(Span {
        start,
        end,
        src: index as u32,
        kind: b.kind,
        work: b.is_work(),
        category: class.category as u16,
        project: class.project.map(|p| p as u16),
    });
}

/// Worked seconds in a list of spans.
pub fn worked(spans: &[Span]) -> i64 {
    spans.iter().filter(|s| s.work).map(Span::secs).sum()
}

/// Seconds away that were not labelled as work (idle, locked, breaks).
pub fn away(spans: &[Span]) -> i64 {
    spans
        .iter()
        .filter(|s| s.kind == Kind::Away && !s.work)
        .map(Span::secs)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AwayLabel;
    use crate::model::Project;
    use crate::settings::Settings;

    fn block(id: u64, kind: Kind, start: i64, end: i64) -> Block {
        Block {
            id,
            kind,
            start,
            end,
            exe: if kind == Kind::Active {
                "code.exe".into()
            } else {
                String::new()
            },
            ..Default::default()
        }
    }

    fn classifier() -> Classifier {
        let mut s = Settings::default();
        s.projects.push(Project {
            id: "apollo".into(),
            name: "Apollo".into(),
            keywords: vec!["apollo".into()],
            ..Default::default()
        });
        s.projects.push(Project {
            id: "zeus".into(),
            name: "Zeus".into(),
            ..Default::default()
        });
        Classifier::new(&s)
    }

    #[test]
    fn interval_helpers() {
        let taken = vec![(10, 20), (30, 40)];
        let mut out = Vec::new();
        subtract(0, 50, &taken, &mut out);
        assert_eq!(out, vec![(0, 10), (20, 30), (40, 50)]);
        out.clear();
        subtract(12, 18, &taken, &mut out);
        assert!(out.is_empty());
        out.clear();
        subtract(15, 35, &taken, &mut out);
        assert_eq!(out, vec![(20, 30)]);
        let mut t = vec![(10, 20), (30, 40)];
        insert(&mut t, 20, 30);
        assert_eq!(t, vec![(10, 40)]);
        insert(&mut t, 50, 60);
        insert(&mut t, 0, 5);
        assert_eq!(t, vec![(0, 5), (10, 40), (50, 60)]);
        insert(&mut t, 4, 55);
        assert_eq!(t, vec![(0, 60)]);
    }

    #[test]
    fn clips_to_the_range() {
        let c = classifier();
        let blocks = vec![
            block(1, Kind::Active, 0, 100),
            block(2, Kind::Active, 100, 300),
            block(3, Kind::Away, 300, 400),
        ];
        let spans = flatten(&blocks, 50, 350, &c);
        assert_eq!(spans.len(), 3);
        assert_eq!((spans[0].start, spans[0].end), (50, 100));
        assert_eq!((spans[2].start, spans[2].end), (300, 350));
        assert_eq!(worked(&spans), 250);
        assert_eq!(away(&spans), 50);
        assert!(flatten(&blocks, 400, 500, &c).is_empty());
    }

    #[test]
    fn worked_time_is_never_counted_twice() {
        let c = classifier();
        let mut meeting = block(3, Kind::Away, 150, 400);
        meeting.away = AwayLabel::Meeting;
        let blocks = vec![
            block(1, Kind::Active, 0, 100),
            // A manual entry laid over tracked time and beyond it.
            block(2, Kind::Manual, 50, 200),
            meeting,
            block(4, Kind::Active, 300, 350),
        ];
        let spans = flatten(&blocks, 0, 1000, &c);
        // Active 0-100, manual 100-200, meeting 200-300 and 350-400, active 300-350.
        assert_eq!(worked(&spans), 400);
        let work: Vec<(i64, i64, u32)> = spans
            .iter()
            .filter(|s| s.work)
            .map(|s| (s.start, s.end, s.src))
            .collect();
        assert_eq!(
            work,
            vec![
                (0, 100, 0),
                (100, 200, 1),
                (200, 300, 2),
                (300, 350, 3),
                (350, 400, 2)
            ]
        );
    }

    #[test]
    fn timers_assign_their_project() {
        let c = classifier();
        let mut timer = block(9, Kind::Timer, 100, 200);
        timer.project = Some("zeus".into());
        let mut by_rule = block(2, Kind::Active, 150, 250);
        by_rule.title = "apollo notes".into();
        let mut by_hand = block(3, Kind::Active, 250, 300);
        by_hand.project = Some("apollo".into());
        let mut timer2 = block(10, Kind::Timer, 240, 400);
        timer2.project = Some("zeus".into());
        let blocks = vec![
            block(1, Kind::Active, 0, 150),
            by_rule,
            by_hand,
            timer,
            timer2,
        ];
        let spans = flatten(&blocks, 0, 1000, &c);
        let work: Vec<(i64, i64, Option<u16>)> = spans
            .iter()
            .filter(|s| s.work)
            .map(|s| (s.start, s.end, s.project))
            .collect();
        assert_eq!(
            work,
            vec![
                (0, 100, None),
                (100, 150, Some(1)),
                // The timer beats the keyword rule...
                (150, 200, Some(1)),
                (200, 240, Some(0)),
                (240, 250, Some(1)),
                // ...but not a project chosen by hand.
                (250, 300, Some(0)),
            ]
        );
        // Timers themselves are overlays, never worked time.
        assert_eq!(worked(&spans), 300);
        assert_eq!(spans.iter().filter(|s| s.kind == Kind::Timer).count(), 2);
    }
}
