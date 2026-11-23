//! The rule engine: which category and project a piece of time belongs to,
//! and what the privacy settings let through before anything is stored.

use crate::model::{Block, Category, Field, Kind, PrivateMode, Score, COMMUNICATION, OTHER};
use crate::settings::Settings;
use regex::{Regex, RegexBuilder};
use std::collections::HashMap;

/// Result of classifying a block: indexes into [`Classifier::categories`] and
/// [`Classifier::projects`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Class {
    pub category: usize,
    pub project: Option<usize>,
}

enum ExePattern {
    Exact(String),
    /// Pieces between `*`.
    Glob(Vec<String>),
}

/// Rules compiled for fast matching. Build one whenever the settings change.
pub struct Classifier {
    pub categories: Vec<Category>,
    /// (id, name, color, keywords lower-case)
    pub projects: Vec<crate::model::Project>,
    cat_index: HashMap<String, usize>,
    project_index: HashMap<String, usize>,
    other: usize,
    communication: usize,
    domain: Vec<(String, usize)>,
    title_regex: Vec<(Regex, usize)>,
    title_contains: Vec<(String, usize)>,
    exe_exact: HashMap<String, usize>,
    exe_glob: Vec<(Vec<String>, usize)>,
    /// (keyword lower-case, project index), longest first.
    keywords: Vec<(String, usize)>,
}

/// `Code.EXE` -> `code.exe`; `code` -> `code.exe`.
pub fn normalize_exe(name: &str) -> String {
    let mut n = name.trim().to_lowercase();
    if let Some(i) = n.rfind(['\\', '/']) {
        n = n[i + 1..].to_string();
    }
    if !n.is_empty() && !n.contains('.') && !n.contains('*') {
        n.push_str(".exe");
    }
    n
}

/// Lower-case host without `www.` and without scheme, port or path.
pub fn normalize_domain(text: &str) -> String {
    let mut d = text.trim().to_lowercase();
    if let Some(i) = d.find("://") {
        d = d[i + 3..].to_string();
    }
    if let Some(i) = d.find(['/', '?', '#']) {
        d.truncate(i);
    }
    if let Some(i) = d.rfind('@') {
        d = d[i + 1..].to_string();
    }
    if let Some(i) = d.rfind(':') {
        if d[i + 1..].chars().all(|c| c.is_ascii_digit()) {
            d.truncate(i);
        }
    }
    d.strip_prefix("www.").map(str::to_string).unwrap_or(d)
}

/// Whether `domain` is `rule` or one of its subdomains.
pub fn domain_matches(domain: &str, rule: &str) -> bool {
    domain == rule
        || (domain.len() > rule.len()
            && domain.ends_with(rule)
            && domain.as_bytes()[domain.len() - rule.len() - 1] == b'.')
}

fn glob_matches(pieces: &[String], text: &str) -> bool {
    // pieces = pattern split on '*'; the first must be a prefix, the last a suffix.
    let Some((first, rest)) = pieces.split_first() else {
        return false;
    };
    if !text.starts_with(first.as_str()) {
        return false;
    }
    let mut pos = first.len();
    let Some((last, middle)) = rest.split_last() else {
        return pos == text.len();
    };
    for piece in middle {
        match text[pos..].find(piece.as_str()) {
            Some(i) => pos += i + piece.len(),
            None => return false,
        }
    }
    text.len() >= pos + last.len() && text[pos..].ends_with(last.as_str())
}

/// Compiles a user-written regular expression: case-insensitive, with a size
/// limit so a hostile pattern cannot eat memory.
pub fn compile_regex(pattern: &str) -> Result<Regex, String> {
    RegexBuilder::new(pattern)
        .case_insensitive(true)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| {
            let text = e.to_string();
            text.lines()
                .last()
                .unwrap_or("invalid pattern")
                .trim()
                .to_string()
        })
}

impl Classifier {
    pub fn new(settings: &Settings) -> Classifier {
        let categories = settings.categories.clone();
        let cat_index: HashMap<String, usize> = categories
            .iter()
            .enumerate()
            .map(|(i, c)| (c.id.clone(), i))
            .collect();
        let other = cat_index.get(OTHER).copied().unwrap_or(0);
        let communication = cat_index.get(COMMUNICATION).copied().unwrap_or(other);
        let mut c = Classifier {
            projects: settings.projects.clone(),
            project_index: settings
                .projects
                .iter()
                .enumerate()
                .map(|(i, p)| (p.id.clone(), i))
                .collect(),
            categories,
            cat_index,
            other,
            communication,
            domain: Vec::new(),
            title_regex: Vec::new(),
            title_contains: Vec::new(),
            exe_exact: HashMap::new(),
            exe_glob: Vec::new(),
            keywords: Vec::new(),
        };
        for rule in settings.rules.iter().filter(|r| r.enabled) {
            let Some(&cat) = c.cat_index.get(&rule.category) else {
                continue;
            };
            let pattern = rule.pattern.trim();
            if pattern.is_empty() {
                continue;
            }
            match rule.field {
                Field::Domain => c.domain.push((normalize_domain(pattern), cat)),
                Field::TitleRegex => {
                    if let Ok(re) = compile_regex(pattern) {
                        c.title_regex.push((re, cat));
                    }
                }
                Field::TitleContains => c.title_contains.push((pattern.to_lowercase(), cat)),
                Field::Exe => match exe_pattern(pattern) {
                    ExePattern::Exact(name) => {
                        // The rule higher in the list wins.
                        c.exe_exact.entry(name).or_insert(cat);
                    }
                    ExePattern::Glob(pieces) => c.exe_glob.push((pieces, cat)),
                },
            }
        }
        for (i, project) in settings.projects.iter().enumerate() {
            if project.archived {
                continue;
            }
            for k in &project.keywords {
                let k = k.trim().to_lowercase();
                if !k.is_empty() {
                    c.keywords.push((k, i));
                }
            }
        }
        // Longest keyword first; equal lengths keep project order (stable sort).
        c.keywords
            .sort_by_key(|(k, _)| std::cmp::Reverse(k.chars().count()));
        c
    }

    pub fn other(&self) -> usize {
        self.other
    }

    /// Whether an enabled program rule covers `exe` (even one for "Other").
    pub fn has_program_rule(&self, exe: &str) -> bool {
        let name = normalize_exe(exe);
        self.exe_exact.contains_key(&name)
            || self.exe_glob.iter().any(|(g, _)| glob_matches(g, &name))
    }

    /// Whether an enabled website rule covers `domain`.
    pub fn has_site_rule(&self, domain: &str) -> bool {
        let d = normalize_domain(domain);
        self.domain.iter().any(|(rule, _)| domain_matches(&d, rule))
    }

    pub fn category_index(&self, id: &str) -> Option<usize> {
        self.cat_index.get(id).copied()
    }

    pub fn project_index(&self, id: &str) -> Option<usize> {
        self.project_index.get(id).copied()
    }

    pub fn score(&self, category: usize) -> Score {
        self.categories
            .get(category)
            .map(|c| c.score)
            .unwrap_or_default()
    }

    /// The category from the rules alone. Website rules beat title rules,
    /// which beat program rules; within a group the first rule in the list wins.
    pub fn category_for(&self, exe: &str, title: &str, domain: &str) -> usize {
        if !domain.is_empty() {
            let d = normalize_domain(domain);
            if let Some((_, cat)) = self.domain.iter().find(|(r, _)| domain_matches(&d, r)) {
                return *cat;
            }
        }
        if !title.is_empty() {
            if let Some((_, cat)) = self.title_regex.iter().find(|(re, _)| re.is_match(title)) {
                return *cat;
            }
            if !self.title_contains.is_empty() {
                let lower = title.to_lowercase();
                if let Some((_, cat)) = self
                    .title_contains
                    .iter()
                    .find(|(needle, _)| lower.contains(needle.as_str()))
                {
                    return *cat;
                }
            }
        }
        if !exe.is_empty() {
            let name = normalize_exe(exe);
            if let Some(cat) = self.exe_exact.get(&name) {
                return *cat;
            }
            if let Some((_, cat)) = self.exe_glob.iter().find(|(g, _)| glob_matches(g, &name)) {
                return *cat;
            }
        }
        self.other
    }

    /// The project from keyword rules alone.
    pub fn project_for(&self, title: &str, domain: &str) -> Option<usize> {
        if self.keywords.is_empty() || (title.is_empty() && domain.is_empty()) {
            return None;
        }
        let title = title.to_lowercase();
        let domain = domain.to_lowercase();
        self.keywords
            .iter()
            .find(|(k, _)| title.contains(k.as_str()) || domain.contains(k.as_str()))
            .map(|(_, p)| *p)
    }

    /// Category and project of a block. Values set by hand on the block win
    /// over every rule.
    pub fn classify(&self, b: &Block) -> Class {
        let category = b
            .category
            .as_deref()
            .and_then(|id| self.category_index(id))
            .unwrap_or_else(|| match b.kind {
                Kind::Active => self.category_for(&b.exe, &b.title, &b.domain),
                Kind::Away if b.away.counts_as_work() => self.communication,
                // Manual entries and timers can still match title rules.
                _ => self.category_for("", &b.title, ""),
            });
        let project = match b.project.as_deref() {
            // An empty override means "no project", even if a rule would match.
            Some("") => None,
            Some(id) => self.project_index(id),
            None => self.project_for(&b.title, &b.domain),
        };
        Class { category, project }
    }
}

fn exe_pattern(pattern: &str) -> ExePattern {
    let name = normalize_exe(pattern);
    if name.contains('*') {
        ExePattern::Glob(name.split('*').map(str::to_string).collect())
    } else {
        ExePattern::Exact(name)
    }
}

/// What may be stored about a window.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Sanitized {
    /// The program is on the never-record list.
    Skip,
    Keep {
        title: String,
        domain: String,
    },
}

/// The privacy settings compiled for use on every sample.
pub struct PrivacyFilter {
    record_titles: bool,
    record_domains: bool,
    hide_private_windows: bool,
    private: HashMap<String, PrivateMode>,
    redactions: Vec<(Regex, String)>,
}

/// Whether a browser window title says it is a private / incognito window.
pub fn is_private_window(title: &str) -> bool {
    let t = title.to_lowercase();
    [
        "[inprivate]",
        "inprivate",
        "(incognito)",
        "- incognito",
        "— private browsing",
        "- private browsing",
        "(private browsing)",
        "private browsing —",
        "(private)",
        "private window",
        "- tor browser",
    ]
    .iter()
    .any(|marker| t.contains(marker))
}

impl PrivacyFilter {
    pub fn new(settings: &Settings) -> PrivacyFilter {
        let p = &settings.privacy;
        PrivacyFilter {
            record_titles: p.record_titles,
            record_domains: p.record_domains,
            hide_private_windows: p.hide_private_windows,
            private: p
                .private_apps
                .iter()
                .map(|a| (normalize_exe(&a.exe), a.mode))
                .collect(),
            redactions: p
                .redactions
                .iter()
                .filter(|r| r.enabled && !r.pattern.trim().is_empty())
                .filter_map(|r| Some((compile_regex(&r.pattern).ok()?, r.replace.clone())))
                .collect(),
        }
    }

    /// Whether this program is never recorded at all.
    pub fn skips(&self, exe: &str) -> bool {
        self.private.get(&normalize_exe(exe)) == Some(&PrivateMode::Skip)
    }

    /// Whether a title may be read for this program at all (used to avoid
    /// even asking a private program's window for its address bar).
    pub fn wants_details(&self, exe: &str) -> bool {
        !self.private.contains_key(&normalize_exe(exe))
    }

    /// Applies every privacy rule. `is_browser` enables private-window detection.
    pub fn apply(&self, exe: &str, title: &str, domain: &str, is_browser: bool) -> Sanitized {
        match self.private.get(&normalize_exe(exe)) {
            Some(PrivateMode::Skip) => return Sanitized::Skip,
            Some(PrivateMode::NoTitle) => {
                return Sanitized::Keep {
                    title: String::new(),
                    domain: String::new(),
                }
            }
            None => {}
        }
        if is_browser && self.hide_private_windows && is_private_window(title) {
            return Sanitized::Keep {
                title: String::new(),
                domain: String::new(),
            };
        }
        let domain = if self.record_domains {
            domain.to_string()
        } else {
            String::new()
        };
        if !self.record_titles {
            return Sanitized::Keep {
                title: String::new(),
                domain,
            };
        }
        Sanitized::Keep {
            title: self.redact(title),
            domain,
        }
    }

    /// Replaces every match of every redaction rule.
    pub fn redact(&self, title: &str) -> String {
        let mut text = title.to_string();
        for (re, replacement) in &self.redactions {
            if re.is_match(&text) {
                // `NoExpand`: the replacement is literal text, `$1` has no meaning.
                text = re
                    .replace_all(&text, regex::NoExpand(replacement))
                    .into_owned();
            }
        }
        text
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{AwayLabel, PrivateApp, Project, Redaction, Rule};

    fn rule(field: Field, pattern: &str, category: &str) -> Rule {
        Rule {
            field,
            pattern: pattern.into(),
            category: category.into(),
            enabled: true,
        }
    }

    fn name(c: &Classifier, i: usize) -> &str {
        &c.categories[i].id
    }

    #[test]
    fn default_rules_classify_common_programs() {
        let c = Classifier::new(&Settings::default());
        assert_eq!(
            name(&c, c.category_for("code.exe", "main.rs", "")),
            "development"
        );
        assert_eq!(
            name(&c, c.category_for("WINWORD.EXE", "Report", "")),
            "writing"
        );
        assert_eq!(
            name(&c, c.category_for("chrome.exe", "News", "")),
            "browsing"
        );
        assert_eq!(name(&c, c.category_for("gimp-3.exe", "", "")), "design");
        assert_eq!(name(&c, c.category_for("mystery.exe", "", "")), OTHER);
        assert_eq!(name(&c, c.category_for("", "", "")), OTHER);
    }

    #[test]
    fn precedence_domain_then_title_then_exe() {
        let mut s = Settings::default();
        s.rules
            .insert(0, rule(Field::TitleContains, "invoice", "writing"));
        s.rules
            .insert(0, rule(Field::TitleRegex, r"^\[draft\]", "design"));
        let c = Classifier::new(&s);
        // Program rule alone.
        assert_eq!(
            name(&c, c.category_for("chrome.exe", "Something", "")),
            "browsing"
        );
        // A title rule beats the program rule.
        assert_eq!(
            name(&c, c.category_for("chrome.exe", "Invoice 12", "")),
            "writing"
        );
        // A regex title rule beats a contains rule.
        assert_eq!(
            name(&c, c.category_for("chrome.exe", "[Draft] invoice", "")),
            "design"
        );
        // A website rule beats them all, including for subdomains.
        assert_eq!(
            name(
                &c,
                c.category_for("chrome.exe", "[Draft] invoice", "gist.github.com")
            ),
            "development"
        );
        // A domain that merely ends with the same letters is not a subdomain.
        assert_eq!(
            name(&c, c.category_for("chrome.exe", "x", "notgithub.com")),
            "browsing"
        );
    }

    #[test]
    fn first_rule_in_a_group_wins_and_disabled_rules_are_ignored() {
        let mut s = Settings::default();
        s.rules.clear();
        s.rules.push(rule(Field::Exe, "tool.exe", "design"));
        s.rules.push(rule(Field::Exe, "tool", "games"));
        s.rules.push(Rule {
            enabled: false,
            ..rule(Field::Domain, "example.org", "media")
        });
        s.rules.push(rule(Field::TitleContains, "alpha", "writing"));
        s.rules
            .push(rule(Field::TitleContains, "alpha beta", "media"));
        s.rules.push(rule(Field::TitleRegex, "(unclosed", "media"));
        let c = Classifier::new(&s);
        assert_eq!(name(&c, c.category_for("Tool.exe", "", "")), "design");
        assert_eq!(
            name(&c, c.category_for("tool.exe", "", "example.org")),
            "design"
        );
        assert_eq!(
            name(&c, c.category_for("x.exe", "ALPHA BETA", "")),
            "writing"
        );
    }

    #[test]
    fn manual_override_beats_rules() {
        let c = Classifier::new(&Settings::default());
        let mut b = Block {
            exe: "code.exe".into(),
            ..Default::default()
        };
        assert_eq!(name(&c, c.classify(&b).category), "development");
        b.category = Some("games".into());
        assert_eq!(name(&c, c.classify(&b).category), "games");
        // An override naming a category that no longer exists falls back to the rules.
        b.category = Some("deleted".into());
        assert_eq!(name(&c, c.classify(&b).category), "development");
        let meeting = Block {
            kind: Kind::Away,
            away: AwayLabel::Meeting,
            ..Default::default()
        };
        assert_eq!(name(&c, c.classify(&meeting).category), COMMUNICATION);
    }

    #[test]
    fn projects_match_keywords_longest_first() {
        let mut s = Settings::default();
        s.projects.push(Project {
            id: "site".into(),
            name: "Website".into(),
            keywords: vec!["acme".into()],
            ..Default::default()
        });
        s.projects.push(Project {
            id: "shop".into(),
            name: "Shop".into(),
            keywords: vec!["acme-shop".into(), "C:\\Work\\Shop".into()],
            ..Default::default()
        });
        s.projects.push(Project {
            id: "old".into(),
            name: "Old".into(),
            keywords: vec!["legacy".into()],
            archived: true,
            ..Default::default()
        });
        let c = Classifier::new(&s);
        let p = |title: &str| c.project_for(title, "").map(|i| c.projects[i].id.clone());
        assert_eq!(p("index.html - ACME"), Some("site".into()));
        assert_eq!(p("cart.rs - acme-shop"), Some("shop".into()));
        assert_eq!(p("c:\\work\\shop\\notes.txt"), Some("shop".into()));
        assert_eq!(p("legacy code"), None);
        assert_eq!(p("unrelated"), None);
        // By hand: a chosen project, or explicitly none.
        let mut b = Block {
            title: "index.html - acme".into(),
            ..Default::default()
        };
        assert_eq!(c.classify(&b).project, Some(0));
        b.project = Some("shop".into());
        assert_eq!(c.classify(&b).project, Some(1));
        b.project = Some(String::new());
        assert_eq!(c.classify(&b).project, None);
    }

    #[test]
    fn exe_and_domain_normalisation() {
        assert_eq!(normalize_exe("C:\\Apps\\Code.EXE"), "code.exe");
        assert_eq!(normalize_exe("code"), "code.exe");
        assert_eq!(normalize_exe("soffice.bin"), "soffice.bin");
        assert_eq!(
            normalize_domain("https://www.Example.org:8080/a?b#c"),
            "example.org"
        );
        assert_eq!(
            normalize_domain("user@mail.example.org"),
            "mail.example.org"
        );
        assert!(domain_matches("a.b.example.org", "example.org"));
        assert!(!domain_matches("badexample.org", "example.org"));
        let g = |p: &str, t: &str| match exe_pattern(p) {
            ExePattern::Glob(pieces) => glob_matches(&pieces, t),
            ExePattern::Exact(n) => n == t,
        };
        assert!(g("gimp*.exe", "gimp-3.2.exe"));
        assert!(g("*studio*", "androidstudio64.exe"));
        assert!(!g("gimp*.exe", "gimp.bin"));
        assert!(g("minecraft*", "minecraft.windows.exe"));
        assert!(!g("a*b*c", "acb"));
    }

    #[test]
    fn privacy_filter() {
        let mut s = Settings::default();
        s.privacy.private_apps = vec![
            PrivateApp {
                exe: "bank.exe".into(),
                mode: PrivateMode::Skip,
            },
            PrivateApp {
                exe: "Diary".into(),
                mode: PrivateMode::NoTitle,
            },
        ];
        s.privacy.redactions = vec![
            Redaction {
                pattern: r"\b\d{4}[ -]?\d{4}[ -]?\d{4}[ -]?\d{4}\b".into(),
                replace: "####".into(),
                enabled: true,
            },
            Redaction {
                pattern: r"[\w.+-]+@[\w-]+\.[\w.]+".into(),
                replace: "$1 <mail>".into(),
                enabled: true,
            },
            Redaction {
                pattern: "secret".into(),
                replace: "x".into(),
                enabled: false,
            },
            Redaction {
                pattern: "(broken".into(),
                replace: "x".into(),
                enabled: true,
            },
        ];
        let f = PrivacyFilter::new(&s);
        assert_eq!(f.apply("Bank.exe", "Balance", "", false), Sanitized::Skip);
        assert!(f.skips("bank.exe") && !f.skips("diary.exe"));
        assert!(!f.wants_details("diary.exe") && f.wants_details("code.exe"));
        let keep = |title: &str, domain: &str| Sanitized::Keep {
            title: title.into(),
            domain: domain.into(),
        };
        assert_eq!(
            f.apply("diary.exe", "Dear diary", "x.org", false),
            keep("", "")
        );
        assert_eq!(
            f.apply(
                "mail.exe",
                "Card 1234 5678 9012 3456 from Sam.Lee@example.org secret",
                "",
                false
            ),
            keep("Card #### from $1 <mail> secret", "")
        );
        // Private browser windows keep the time but nothing else.
        assert_eq!(
            f.apply(
                "msedge.exe",
                "Gift ideas - [InPrivate] - Edge",
                "shop.example",
                true
            ),
            keep("", "")
        );
        assert_eq!(
            f.apply("notepad.exe", "(private) notes", "", false),
            keep("(private) notes", "")
        );
        // The global switches.
        s.privacy.record_titles = false;
        let f = PrivacyFilter::new(&s);
        assert_eq!(
            f.apply("code.exe", "main.rs", "example.org", false),
            keep("", "example.org")
        );
        s.privacy.record_domains = false;
        let f = PrivacyFilter::new(&s);
        assert_eq!(
            f.apply("chrome.exe", "Page", "example.org", true),
            keep("", "")
        );
    }

    #[test]
    fn private_window_markers() {
        for t in [
            "News - [InPrivate] - Microsoft Edge",
            "Search — Mozilla Firefox — Private Browsing",
            "Page - Mozilla Firefox (Private Browsing)",
            "Tab (Incognito)",
        ] {
            assert!(is_private_window(t), "{t}");
        }
        assert!(!is_private_window("Privacy policy - Browser"));
    }
}
