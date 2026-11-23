# Changelog

What changed in each version of HourForge, in plain words.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/); versions follow [Semantic Versioning](https://semver.org/).

## [1.1.0] - 2026-11-24

Your recorded time and your settings from 1.0.0 are used as they are. Nothing needs converting.

### Added

- **Kurdish, Arabic, Persian and Hebrew text is shown properly.** Window titles, project and client names, notes, tags and categories in these scripts now appear joined and in the right reading order everywhere: the timeline, Today, reports, the Projects page, menus and drop-down lists. You can also type them in every text box (search, notes, project names, the timer task) and see them correctly while typing.
- **Review page.** A new page lists the programs and websites that no rule has sorted yet, the ones with the most time first. Pick a category once and HourForge remembers it. Websites that only count as "Browsing" because of the browser they were opened in are listed too. A number next to "Review" shows how many are waiting.
- **Invoice.** On the Projects page, "Invoice…" makes a clean page to print or save as PDF: your name, the client, one line per project with hours, rate and amount, the total due, and the hours per day behind it. Also from the command line: `HourForge.exe --export last-month --invoice`.
- **Billable switch for projects.** Mark a project as not billable to keep tracking it without ever charging it: it shows no amount on timesheets and is left off invoices.
- **Calendar export.** The activity report can be saved as a calendar file (.ics) for Outlook, Google Calendar and others, with one event per sitting, named after its project or category. Window titles and websites are left out of the file on purpose. Command line: `--format ics`.
- **Weekly summary.** At the start of each week a notice near the clock tells you last week's hours, how that compares with the week before, and your busiest category and project. Clicking it opens that week in the reports. It can be turned off in Settings.
- **Search in the timeline.** Type a word to narrow the day to matching titles, programs, websites, notes, tags, categories or projects. `#word` looks at tags only.
- **Tags for many blocks at once.** Select blocks in the timeline and choose "# Tags" to add tags to all of them, or replace theirs.
- **Time per tag** in the HTML and JSON reports.
- **Keyboard shortcuts.** Ctrl+1 to Ctrl+9 open the pages, Alt+Left and Alt+Right change the day, Alt+Home goes back to today, Ctrl+F searches the timeline, Ctrl+N adds time, Ctrl+E exports the day in view, Ctrl+T starts or stops the timer, Enter confirms a form. Press F1 for the list.

### Changed

- Timesheets have a new last column, `billable` (CSV) and a `billable` field (JSON). The other columns are where they were.
- Time added by hand may run past midnight: "22:30 to 01:15" now means until 01:15 the next day.
- On the Projects page the keywords of all projects now line up in one column.
- The portable ZIP now includes this changelog.
- For people who build from source: `scripts\package.ps1` follows `CARGO_TARGET_DIR`.

### Fixed

- **Settings could be lost after a damaged settings file.** If the settings file could not be read (for example after a crash while the disk was full), HourForge started with the defaults and the next change replaced your categories, rules and projects for good. The unreadable file is now kept next to it as `settings.damaged.json`.
- On the Today page, "Next day" could walk into days that have not happened yet. It now stops at today, as the timeline already did.

## [1.0.0] - 2026-11-17

First release.

- Automatic tracking of the program, window and website you work in, with away detection and the "what were you doing?" question when you come back.
- Today page, timeline with editing (add, split, merge, delete, notes), and reports by day, week, month or any range.
- Categories with rules, projects with keywords, timesheets with rounding and hourly rates.
- Daily goals and limits, break reminders, a focus timer and a start/stop timer with a global hotkey.
- Privacy tools: pause, private programs, title redaction, delete any range. No network requests at all.
- CSV, JSON and HTML export, backup and import, and a command line.

[1.1.0]: https://github.com/zheerkhalid/hourforge/releases/tag/v1.1.0
[1.0.0]: https://github.com/zheerkhalid/hourforge/releases/tag/v1.0.0
