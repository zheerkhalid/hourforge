//! Editing recorded time by hand: splitting, merging, manual entries, tags.
//! Pure functions; the caller writes the results to the store.

use crate::model::{new_id, Block, Kind};

/// The longest manual entry that is accepted (a guard against typos in dates).
pub const MAX_MANUAL_SECS: i64 = 24 * 3600;

/// Cuts a block in two at `at`. The first half keeps the id; the second gets
/// a new one and the same details. `None` when `at` is not strictly inside.
pub fn split(block: &Block, at: i64) -> Option<(Block, Block)> {
    if at <= block.start || at >= block.end {
        return None;
    }
    let first = Block {
        end: at,
        ..block.clone()
    };
    let second = Block {
        id: new_id(),
        start: at,
        ..block.clone()
    };
    Some((first, second))
}

/// Why blocks cannot be merged.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MergeError {
    /// Fewer than two blocks.
    TooFew,
    /// Tracked, away, manual, timer and focus blocks cannot be mixed.
    MixedKinds,
}

impl MergeError {
    pub fn text(self) -> &'static str {
        match self {
            MergeError::TooFew => "Select at least two blocks to merge.",
            MergeError::MixedKinds => {
                "Only blocks of the same kind can be merged (for example two tracked blocks)."
            }
        }
    }
}

/// Merges blocks into one that runs from the earliest start to the latest
/// end (gaps between them are filled). The earliest block keeps its id and
/// details; notes are joined, tags united, and a project or category set by
/// hand on any of them is kept. Returns the merged block and the ids to delete.
pub fn merge(blocks: &[Block]) -> Result<(Block, Vec<u64>), MergeError> {
    if blocks.len() < 2 {
        return Err(MergeError::TooFew);
    }
    if blocks.iter().any(|b| b.kind != blocks[0].kind) {
        return Err(MergeError::MixedKinds);
    }
    let mut sorted: Vec<&Block> = blocks.iter().collect();
    sorted.sort_by_key(|b| (b.start, b.id));
    let mut merged = sorted[0].clone();
    let mut remove = Vec::new();
    for b in &sorted[1..] {
        merged.end = merged.end.max(b.end);
        if merged.project.is_none() {
            merged.project = b.project.clone();
        }
        if merged.category.is_none() {
            merged.category = b.category.clone();
        }
        if merged.title.is_empty() {
            merged.title = b.title.clone();
        }
        if !b.note.is_empty() && !merged.note.contains(&b.note) {
            if !merged.note.is_empty() {
                merged.note.push_str("; ");
            }
            merged.note.push_str(&b.note);
        }
        for tag in &b.tags {
            if !merged.tags.contains(tag) {
                merged.tags.push(tag.clone());
            }
        }
        remove.push(b.id);
    }
    Ok((merged, remove))
}

/// A manual entry (offline time). Checks the times.
pub fn manual(start: i64, end: i64, title: &str) -> Result<Block, String> {
    if end <= start {
        return Err("The end must be after the start.".into());
    }
    if end - start > MAX_MANUAL_SECS {
        return Err("A manual entry can be at most 24 hours long.".into());
    }
    Ok(Block {
        id: new_id(),
        start,
        end,
        kind: Kind::Manual,
        title: title.trim().to_string(),
        ..Default::default()
    })
}

/// `"client, #urgent  deep work"` -> `["client", "urgent", "deep work"]`.
/// Tags are separated by commas, trimmed, without a leading `#`, unique.
pub fn parse_tags(text: &str) -> Vec<String> {
    let mut tags: Vec<String> = Vec::new();
    for part in text.split([',', ';']) {
        let tag = part.trim().trim_start_matches('#').trim().to_string();
        if !tag.is_empty() && !tags.iter().any(|t| t.eq_ignore_ascii_case(&tag)) {
            tags.push(tag);
        }
    }
    tags
}

pub fn tags_text(tags: &[String]) -> String {
    tags.join(", ")
}

/// `existing` plus the tags of `more` it does not have yet (ignoring case).
pub fn add_tags(existing: &[String], more: &[String]) -> Vec<String> {
    let mut tags = existing.to_vec();
    for tag in more {
        if !tags.iter().any(|t| t.to_lowercase() == tag.to_lowercase()) {
            tags.push(tag.clone());
        }
    }
    tags
}

/// Whether a block matches what was typed in the timeline's search box.
///
/// Every word must be found, ignoring case, in the block's title, program,
/// website, note, tags, category or project. A word that starts with `#`
/// only looks at the tags (`#client` finds the tag `client` and
/// `client-visit`).
pub fn matches_search(block: &Block, query: &str, category: &str, project: &str) -> bool {
    let haystack = [
        block.title.as_str(),
        block.app_label(),
        block.exe.as_str(),
        block.domain.as_str(),
        block.note.as_str(),
        category,
        project,
    ]
    .join("\n")
    .to_lowercase();
    let tags: Vec<String> = block.tags.iter().map(|t| t.to_lowercase()).collect();
    query.split_whitespace().all(|word| {
        let word = word.to_lowercase();
        match word.strip_prefix('#') {
            Some(tag) => tags.iter().any(|t| t.starts_with(tag)),
            None => haystack.contains(&word) || tags.iter().any(|t| t.contains(&word)),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_looks_everywhere_and_tags_can_be_asked_for() {
        let b = Block {
            exe: "code.exe".into(),
            app: "Visual Studio Code".into(),
            title: "cart.rs - atlas-shop".into(),
            domain: "docs.example".into(),
            note: "Pairing with Sam".into(),
            tags: vec!["client".into(), "Deep Work".into()],
            ..Default::default()
        };
        let hit = |q: &str| matches_search(&b, q, "Development", "Atlas Web Shop");
        assert!(hit(""));
        assert!(hit("   "));
        assert!(hit("CART"));
        assert!(hit("studio"));
        assert!(hit("docs.example"));
        assert!(hit("pairing sam"));
        assert!(hit("development"));
        assert!(hit("web shop"));
        assert!(hit("deep"));
        assert!(hit("#client"));
        assert!(hit("#cli cart"));
        assert!(hit("#deep"));
        // Every word has to match, and `#` looks at tags only.
        assert!(!hit("cart missing"));
        assert!(!hit("#cart"));
        assert!(!hit("#work"));
        // Other scripts: Kurdish titles are found, in any letter case rules.
        let k = Block {
            title: "ڕاپۆرتی مانگانە – ماڵپەڕ".into(),
            ..Default::default()
        };
        assert!(matches_search(&k, "ماڵپەڕ", "", ""));
        assert!(!matches_search(&k, "پلان", "", ""));
    }

    #[test]
    fn tags_are_added_once() {
        let have = vec!["client".to_string(), "Billable".to_string()];
        let more = parse_tags("billable, urgent, CLIENT, urgent");
        assert_eq!(add_tags(&have, &more), ["client", "Billable", "urgent"]);
        assert_eq!(add_tags(&[], &more), ["billable", "urgent", "CLIENT"]);
    }
    use crate::model::AwayLabel;

    fn block(id: u64, start: i64, end: i64) -> Block {
        Block {
            id,
            start,
            end,
            exe: "editor.exe".into(),
            title: format!("doc {id}"),
            ..Default::default()
        }
    }

    #[test]
    fn splitting() {
        let b = Block {
            note: "n".into(),
            tags: vec!["t".into()],
            ..block(1, 100, 200)
        };
        let (a, c) = split(&b, 130).unwrap();
        assert_eq!((a.id, a.start, a.end), (1, 100, 130));
        assert_eq!((c.start, c.end), (130, 200));
        assert_ne!(c.id, 1);
        assert_eq!(c.note, "n");
        assert_eq!(a.secs() + c.secs(), b.secs());
        assert!(split(&b, 100).is_none());
        assert!(split(&b, 200).is_none());
        assert!(split(&b, 50).is_none());
    }

    #[test]
    fn merging() {
        let mut second = block(2, 220, 300);
        second.note = "second".into();
        second.tags = vec!["x".into(), "y".into()];
        second.project = Some("apollo".into());
        let mut third = block(3, 300, 320);
        third.note = "second".into();
        third.tags = vec!["y".into()];
        let (m, remove) = merge(&[third, block(1, 100, 200), second]).unwrap();
        assert_eq!((m.id, m.start, m.end), (1, 100, 320));
        assert_eq!(m.title, "doc 1");
        assert_eq!(m.note, "second");
        assert_eq!(m.tags, vec!["x".to_string(), "y".to_string()]);
        assert_eq!(m.project.as_deref(), Some("apollo"));
        assert_eq!(remove, vec![2, 3]);
        assert_eq!(merge(&[block(1, 0, 1)]).unwrap_err(), MergeError::TooFew);
        let away = Block {
            kind: Kind::Away,
            away: AwayLabel::Break,
            ..block(5, 0, 10)
        };
        assert_eq!(
            merge(&[block(1, 0, 1), away]).unwrap_err(),
            MergeError::MixedKinds
        );
    }

    #[test]
    fn manual_entries() {
        let m = manual(1000, 4600, "  Workshop  ").unwrap();
        assert_eq!(m.kind, Kind::Manual);
        assert_eq!(m.title, "Workshop");
        assert!(m.is_work());
        assert!(manual(10, 10, "x").is_err());
        assert!(manual(10, 5, "x").is_err());
        assert!(manual(0, MAX_MANUAL_SECS + 1, "x").is_err());
    }

    #[test]
    fn tags() {
        assert_eq!(
            parse_tags("client, #urgent ;deep work,, Client"),
            vec!["client", "urgent", "deep work"]
        );
        assert!(parse_tags("  ").is_empty());
        assert_eq!(tags_text(&parse_tags("a,b")), "a, b");
    }
}
