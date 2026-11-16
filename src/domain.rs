//! Website domains for browser windows.
//!
//! Nothing here touches the network: a domain is worked out from text the
//! browser already shows (its address bar, or its window title). Only the
//! host is ever returned, never a path, a query or a user name.

/// Schemes whose addresses are websites.
const WEB_SCHEMES: [&str; 3] = ["http", "https", "ftp"];

/// Characters some browsers put into titles that cannot be seen.
fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{200b}' | '\u{200c}' | '\u{200d}' | '\u{200e}' | '\u{200f}' | '\u{2060}' | '\u{feff}'
    )
}

fn valid_host(host: &str) -> bool {
    if host.is_empty()
        || host.len() > 253
        || host.starts_with(['.', '-'])
        || host.ends_with('-')
        || host.contains("..")
    {
        return false;
    }
    host.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || !c.is_ascii())
}

fn is_ipv4(host: &str) -> bool {
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.len() <= 3 && p.parse::<u8>().is_ok())
}

/// The host of an address-bar text (`https://www.example.org/a?b` ->
/// `example.org`). `None` for things that are not websites: search terms,
/// `about:` / `chrome://` / `edge://` / `file:` addresses, empty text.
/// Only the domain is ever returned, never a path or query.
pub fn domain_from_url(text: &str) -> Option<String> {
    let text: String = text.trim().chars().filter(|c| !is_invisible(*c)).collect();
    if text.is_empty() || text.chars().any(char::is_whitespace) {
        return None;
    }
    let lower = text.to_lowercase();
    let mut rest = lower.as_str();
    if let Some(i) = rest.find("://") {
        if !WEB_SCHEMES.contains(&&rest[..i]) {
            return None;
        }
        rest = &rest[i + 3..];
    } else if let Some(i) = rest.find(':') {
        // `about:blank`, `mailto:...`, `view-source:...` are schemes;
        // `localhost:3000/x` is a host with a port.
        let after = &rest[i + 1..];
        let port_end = after.find(['/', '?', '#']).unwrap_or(after.len());
        let port = &after[..port_end];
        let is_port = !port.is_empty() && port.chars().all(|c| c.is_ascii_digit());
        if !is_port && !rest.starts_with('[') {
            return None;
        }
    }
    if let Some(i) = rest.find(['/', '?', '#']) {
        rest = &rest[..i];
    }
    if let Some(i) = rest.rfind('@') {
        rest = &rest[i + 1..];
    }
    // IPv6 literal: keep it as written, without the port.
    if rest.starts_with('[') {
        let end = rest.find(']')?;
        let inner = &rest[1..end];
        let ok = inner.contains(':')
            && inner
                .chars()
                .all(|c| c.is_ascii_hexdigit() || c == ':' || c == '.');
        return ok.then(|| rest[..=end].to_string());
    }
    if let Some(i) = rest.rfind(':') {
        if rest[i + 1..].chars().all(|c| c.is_ascii_digit()) {
            rest = &rest[..i];
        } else {
            return None;
        }
    }
    let host = rest.trim_end_matches('.');
    let host = host.strip_prefix("www.").unwrap_or(host);
    if !valid_host(host) {
        return None;
    }
    if host == "localhost" || is_ipv4(host) {
        return Some(host.to_string());
    }
    // A single word is a search, not a site.
    let (_, tld) = host.rsplit_once('.')?;
    let tld_ok = tld.len() >= 2
        && (tld.chars().all(|c| c.is_ascii_alphabetic())
            || tld.starts_with("xn--")
            || tld.chars().all(|c| !c.is_ascii()));
    tld_ok.then(|| host.to_string())
}

/// Top-level domains accepted when a domain is only *mentioned* in a title.
/// Deliberately short, and without endings that are common file types
/// (`.rs`, `.py`, `.md`, `.sh`, `.pl`, `.ai`, `.app`, `.zip`, ...).
const TITLE_TLDS: [&str; 46] = [
    "com", "org", "net", "edu", "gov", "io", "dev", "co", "uk", "de", "fr", "nl", "eu", "ch", "at",
    "be", "se", "no", "dk", "fi", "es", "it", "ie", "ca", "us", "au", "nz", "jp", "cn", "in", "br",
    "ru", "tr", "iq", "ae", "info", "tv", "xyz", "tech", "site", "online", "store", "blog",
    "cloud", "example", "test",
];

/// Well-known site names as they appear at the end of page titles.
const SITE_NAMES: [(&str, &str); 38] = [
    ("youtube", "youtube.com"),
    ("youtube music", "music.youtube.com"),
    ("github", "github.com"),
    ("gitlab", "gitlab.com"),
    ("stack overflow", "stackoverflow.com"),
    ("stack exchange", "stackexchange.com"),
    ("gmail", "mail.google.com"),
    ("google search", "google.com"),
    ("google docs", "docs.google.com"),
    ("google sheets", "docs.google.com"),
    ("google slides", "docs.google.com"),
    ("google drive", "drive.google.com"),
    ("google calendar", "calendar.google.com"),
    ("google maps", "maps.google.com"),
    ("google meet", "meet.google.com"),
    ("google translate", "translate.google.com"),
    ("wikipedia", "wikipedia.org"),
    ("reddit", "reddit.com"),
    ("x", "x.com"),
    ("twitter", "twitter.com"),
    ("facebook", "facebook.com"),
    ("instagram", "instagram.com"),
    ("linkedin", "linkedin.com"),
    ("netflix", "netflix.com"),
    ("twitch", "twitch.tv"),
    ("whatsapp", "web.whatsapp.com"),
    ("whatsapp web", "web.whatsapp.com"),
    ("telegram web", "web.telegram.org"),
    ("spotify", "open.spotify.com"),
    ("figma", "figma.com"),
    ("canva", "canva.com"),
    ("trello", "trello.com"),
    ("chatgpt", "chatgpt.com"),
    ("claude", "claude.ai"),
    ("mdn", "developer.mozilla.org"),
    ("mdn web docs", "developer.mozilla.org"),
    ("docs.rs", "docs.rs"),
    ("microsoft teams", "teams.microsoft.com"),
];

const SEPARATORS: [&str; 5] = [" - ", " | ", " · ", " — ", " – "];

/// Splits off the part after the last separator: `("Page", "Site")`.
fn split_last<'a>(title: &'a str, separators: &[&str]) -> Option<(&'a str, &'a str)> {
    separators
        .iter()
        .filter_map(|sep| title.rfind(sep).map(|i| (i, sep.len())))
        .max_by_key(|(i, _)| *i)
        .map(|(i, len)| (&title[..i], &title[i + len..]))
}

fn normalized(text: &str) -> String {
    text.chars()
        .filter(|c| !is_invisible(*c))
        .collect::<String>()
        .trim()
        .to_lowercase()
}

/// A domain written out in running text, if the token looks like one.
fn domain_token(token: &str) -> Option<String> {
    let token = token.trim_matches(|c: char| {
        matches!(
            c,
            '.' | ','
                | ':'
                | ';'
                | '!'
                | '?'
                | '('
                | ')'
                | '['
                | ']'
                | '<'
                | '>'
                | '"'
                | '\''
                | '«'
                | '»'
                | '“'
                | '”'
        )
    });
    if token.contains("://") {
        return domain_from_url(token);
    }
    if token.contains(['\\', '@', '_', '=', '%']) {
        return None;
    }
    let host = token.split(['/', '?', '#']).next().unwrap_or("");
    let lower = host.to_lowercase();
    let lower = lower.strip_prefix("www.").unwrap_or(&lower);
    if !lower.is_ascii() || !valid_host(lower) {
        return None;
    }
    let labels: Vec<&str> = lower.split('.').collect();
    if labels.len() < 2 || labels.iter().any(|l| l.is_empty()) {
        return None;
    }
    let tld = labels[labels.len() - 1];
    // The name before the ending must be a word, not a number (`v1.2.com`).
    let name = labels[labels.len() - 2];
    if !TITLE_TLDS.contains(&tld) || !name.chars().any(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    // Product names that happen to look like domains.
    if ["asp.net", "vb.net", "ado.net", "ml.net"].contains(&lower) {
        return None;
    }
    Some(lower.to_string())
}

/// A domain guessed from a browser window title alone: a domain written in
/// the title, or a well-known site name at the end of the page title.
/// `None` when unsure: a wrong domain is worse than none.
pub fn domain_from_title(title: &str) -> Option<String> {
    let page = strip_browser_suffix(title);
    let page: String = page.chars().filter(|c| !is_invisible(*c)).collect();
    let page = page.trim();
    if page.is_empty() {
        return None;
    }
    // 1. A domain written in the title; the one nearest the end is usually the site.
    if let Some(found) = page.split_whitespace().rev().find_map(domain_token) {
        return Some(found);
    }
    // 2. A well-known site name as the last part of the title.
    let lookup = |name: &str| {
        let name = normalized(name);
        SITE_NAMES
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, d)| d.to_string())
    };
    if let Some(d) = lookup(page) {
        return Some(d);
    }
    if let Some((_, last)) = split_last(page, &SEPARATORS) {
        if let Some(d) = lookup(last) {
            return Some(d);
        }
    }
    if let Some((_, last)) = split_last(page, &[" / "]) {
        if normalized(last) == "x" {
            return Some("x.com".into());
        }
    }
    if normalized(page).ends_with(" at duckduckgo") {
        return Some("duckduckgo.com".into());
    }
    None
}

/// Browser names as they appear at the end of window titles.
const BROWSERS: [&str; 18] = [
    "google chrome",
    "google chrome beta",
    "google chrome dev",
    "chromium",
    "microsoft edge",
    "microsoft edge beta",
    "microsoft edge dev",
    "mozilla firefox",
    "mozilla firefox private browsing",
    "firefox",
    "firefox developer edition",
    "brave",
    "opera",
    "opera gx",
    "vivaldi",
    "tor browser",
    "librewolf",
    "waterfox",
];

const TITLE_DASHES: [&str; 3] = [" - ", " — ", " – "];

fn is_profile_part(text: &str) -> bool {
    let t = normalized(text);
    if matches!(
        t.as_str(),
        "personal" | "work" | "guest" | "[inprivate]" | "inprivate" | "(incognito)" | "incognito"
    ) {
        return true;
    }
    t.strip_prefix("profile ")
        .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
}

/// Edge adds ` and 3 more pages` when a window has several tabs.
fn strip_more_pages(title: &str) -> &str {
    let Some(i) = title.rfind(" and ") else {
        return title;
    };
    let tail = normalized(&title[i + 5..]);
    let mut words = tail.split(' ');
    let number = words.next().unwrap_or("");
    let rest: Vec<&str> = words.collect();
    let counted = !number.is_empty() && number.chars().all(|c| c.is_ascii_digit());
    if counted && (rest == ["more", "pages"] || rest == ["more", "page"]) {
        &title[..i]
    } else {
        title
    }
}

/// The page title without the browser's own suffix
/// (`Inbox - Some Browser` -> `Inbox`).
pub fn strip_browser_suffix(title: &str) -> &str {
    let trimmed = title.trim();
    if BROWSERS.contains(&normalized(trimmed).as_str()) {
        return "";
    }
    let Some((mut page, browser)) = split_last(trimmed, &TITLE_DASHES) else {
        return trimmed;
    };
    let browser = normalized(browser);
    if !BROWSERS.contains(&browser.as_str()) {
        return trimmed;
    }
    // Edge (and Chrome with several profiles) put the profile before the name.
    for _ in 0..2 {
        match split_last(page, &TITLE_DASHES) {
            Some((before, part)) if is_profile_part(part) => page = before,
            _ => break,
        }
    }
    if is_profile_part(page) {
        return "";
    }
    strip_more_pages(page).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        let cases: &[(&str, Option<&str>)] = &[
            ("https://www.example.org/a/b?c=d#e", Some("example.org")),
            ("http://example.org", Some("example.org")),
            ("example.org", Some("example.org")),
            ("example.org/some/path", Some("example.org")),
            ("www.example.org", Some("example.org")),
            ("WWW.Example.ORG/Path", Some("example.org")),
            ("docs.example.co.uk/x", Some("docs.example.co.uk")),
            (
                "https://user:secret@mail.example.org:8443/inbox",
                Some("mail.example.org"),
            ),
            ("example.org:8080", Some("example.org")),
            ("example.org:8080/admin", Some("example.org")),
            ("example.org.", Some("example.org")),
            ("https://example.org?q=1", Some("example.org")),
            ("https://example.org#top", Some("example.org")),
            ("ftp://files.example.net/pub", Some("files.example.net")),
            ("localhost", Some("localhost")),
            ("localhost:3000/app", Some("localhost")),
            ("http://localhost:5173", Some("localhost")),
            ("192.168.1.10", Some("192.168.1.10")),
            ("http://192.168.1.10:8080/status", Some("192.168.1.10")),
            ("https://[::1]:8080/x", Some("[::1]")),
            ("bücher.example", Some("bücher.example")),
            (
                "https://xn--bcher-kva.example/a",
                Some("xn--bcher-kva.example"),
            ),
            ("main.rs", Some("main.rs")),
            ("  https://example.org/padded  ", Some("example.org")),
            ("", None),
            ("   ", None),
            ("how to bake bread", None),
            ("bread", None),
            ("about:blank", None),
            ("about:newtab", None),
            ("chrome://settings/privacy", None),
            ("edge://newtab", None),
            ("brave://rewards", None),
            ("file:///C:/Users/Sam/report.html", None),
            ("view-source:https://example.org/", None),
            ("mailto:sam@example.org", None),
            ("javascript:void(0)", None),
            ("data:text/html,hello", None),
            ("chrome-extension://abcdefgh/page.html", None),
            ("1.2.3", None),
        ];
        for &(input, want) in cases {
            assert_eq!(domain_from_url(input).as_deref(), want, "{input:?}");
        }
        // Never a path, query or user name.
        for input in [
            "https://a.example.org/secret/path?token=abc",
            "https://sam:pw@example.org/x",
        ] {
            let d = domain_from_url(input).unwrap();
            assert!(!d.contains('/') && !d.contains('?') && !d.contains('@') && !d.contains("sam"));
        }
        assert_eq!(domain_from_url("ex ample.org"), None);
        assert_eq!(domain_from_url("-bad.example.org"), None);
        assert_eq!(domain_from_url("a..example.org"), None);
        assert_eq!(domain_from_url("example.org:port"), None);
    }

    #[test]
    fn browser_suffixes() {
        let cases: &[(&str, &str)] = &[
            ("Quarterly plan - Google Chrome", "Quarterly plan"),
            ("Quarterly plan - Chromium", "Quarterly plan"),
            ("Quarterly plan — Mozilla Firefox", "Quarterly plan"),
            ("Quarterly plan - Mozilla Firefox", "Quarterly plan"),
            (
                "Quarterly plan — Mozilla Firefox Private Browsing",
                "Quarterly plan",
            ),
            ("Quarterly plan - Brave", "Quarterly plan"),
            ("Quarterly plan - Opera", "Quarterly plan"),
            ("Quarterly plan - Vivaldi", "Quarterly plan"),
            ("Quarterly plan - Microsoft Edge", "Quarterly plan"),
            ("Quarterly plan - Microsoft\u{200b} Edge", "Quarterly plan"),
            (
                "Quarterly plan - Personal - Microsoft\u{200b} Edge",
                "Quarterly plan",
            ),
            (
                "Quarterly plan - Profile 1 - Microsoft Edge",
                "Quarterly plan",
            ),
            ("Quarterly plan - Work - Microsoft Edge", "Quarterly plan"),
            (
                "Quarterly plan - [InPrivate] - Microsoft Edge",
                "Quarterly plan",
            ),
            (
                "Quarterly plan and 3 more pages - Personal - Microsoft Edge",
                "Quarterly plan",
            ),
            (
                "Quarterly plan and 1 more page - Work - Microsoft Edge",
                "Quarterly plan",
            ),
            (
                "Salt and pepper - Recipes - Google Chrome",
                "Salt and pepper - Recipes",
            ),
            ("Budget - Sheets - Google Chrome", "Budget - Sheets"),
            ("Google Chrome", ""),
            ("Mozilla Firefox", ""),
            ("Personal - Microsoft Edge", ""),
            // Titles that only mention a browser are left alone.
            (
                "How to install Google Chrome on a laptop",
                "How to install Google Chrome on a laptop",
            ),
            (
                "Opera - tickets for Saturday",
                "Opera - tickets for Saturday",
            ),
            ("Brave new world - notes.txt", "Brave new world - notes.txt"),
            ("Plain title", "Plain title"),
            ("  Padded - Google Chrome  ", "Padded"),
        ];
        for &(input, want) in cases {
            assert_eq!(strip_browser_suffix(input), want, "{input:?}");
        }
        assert_eq!(strip_browser_suffix(""), "");
        // A window that shows only profile names has no page title left.
        assert_eq!(strip_browser_suffix("Work - Work - Microsoft Edge"), "");
        assert_eq!(
            strip_browser_suffix("Rota - Profile 12 - Microsoft Edge"),
            "Rota"
        );
        assert_eq!(
            strip_browser_suffix("Rota - Profile x - Microsoft Edge"),
            "Rota - Profile x"
        );
    }

    #[test]
    fn domains_from_titles() {
        let cases: &[(&str, Option<&str>)] = &[
            // A domain written in the title.
            (
                "Welcome to shop.example.com - Google Chrome",
                Some("shop.example.com"),
            ),
            (
                "example.org: news and weather — Mozilla Firefox",
                Some("example.org"),
            ),
            (
                "Sign in | portal.example.co.uk - Microsoft Edge",
                Some("portal.example.co.uk"),
            ),
            ("(3) inbox - mail.example.net", Some("mail.example.net")),
            (
                "Read https://www.example.org/articles/42 later",
                Some("example.org"),
            ),
            ("WWW.EXAMPLE.COM - Brave", Some("example.com")),
            ("Results for example.dev/docs - Opera", Some("example.dev")),
            // Well-known site names at the end of the page title.
            (
                "Ten minute bread recipe - YouTube - Google Chrome",
                Some("youtube.com"),
            ),
            ("(12) Lo-fi mix - YouTube", Some("youtube.com")),
            ("YouTube - Google Chrome", Some("youtube.com")),
            (
                "alexdev/toolbox: handy scripts · GitHub - Brave",
                Some("github.com"),
            ),
            (
                "rust - How do I sort a vector? - Stack Overflow - Vivaldi",
                Some("stackoverflow.com"),
            ),
            (
                "Inbox (4) - sam.lee@example.org - Gmail - Google Chrome",
                Some("mail.google.com"),
            ),
            (
                "bread recipe - Google Search - Google Chrome",
                Some("google.com"),
            ),
            (
                "Trip budget - Google Sheets - Microsoft Edge",
                Some("docs.google.com"),
            ),
            ("Bread - Wikipedia — Mozilla Firefox", Some("wikipedia.org")),
            ("Sam Lee on X: \"hello\" / X - Google Chrome", Some("x.com")),
            (
                "Array.prototype.map() - JavaScript | MDN - Google Chrome",
                Some("developer.mozilla.org"),
            ),
            (
                "bread recipe at DuckDuckGo — Mozilla Firefox",
                Some("duckduckgo.com"),
            ),
            ("Landing page – Figma - Google Chrome", Some("figma.com")),
            // File names and other look-alikes are not domains.
            ("main.rs - toolbox - Editor", None),
            ("report.pdf - Google Chrome", None),
            ("notes.md", None),
            ("build.sh — Mozilla Firefox", None),
            ("logo.ai - Google Chrome", None),
            ("setup.exe downloaded - Microsoft Edge", None),
            ("C:\\Work\\site\\index.html - Google Chrome", None),
            ("Version 1.2.com released", None),
            (
                "Getting started with ASP.NET Core - Docs - Google Chrome",
                None,
            ),
            ("sam.lee@example.org - Mail - Google Chrome", None),
            ("Meeting at 10.30 - Calendar - Google Chrome", None),
            ("my_file.com.txt - Google Chrome", None),
            // Site names only count at the end, as a whole part.
            ("YouTube tips for beginners - Blog - Google Chrome", None),
            ("Why I left GitHub", None),
            ("New Tab - Google Chrome", None),
            ("Google Chrome", None),
            ("", None),
        ];
        for &(input, want) in cases {
            assert_eq!(domain_from_title(input).as_deref(), want, "{input:?}");
        }
    }

    #[test]
    fn tables_are_consistent() {
        for (name, domain) in SITE_NAMES {
            assert_eq!(name, name.to_lowercase());
            assert_eq!(domain_from_url(domain).as_deref(), Some(domain), "{domain}");
        }
        for b in BROWSERS {
            assert_eq!(b, b.to_lowercase());
        }
        for tld in TITLE_TLDS {
            for ext in [
                "rs", "py", "md", "sh", "pl", "ai", "app", "zip", "js", "ts", "txt", "pdf", "exe",
            ] {
                assert_ne!(tld, ext);
            }
        }
    }
}
