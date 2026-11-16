//! Builds blocks of time from the stream of events the tracker observes.
//!
//! The builder is pure: it never looks at the clock or the system, so every
//! behaviour can be tested by feeding it events.
//!
//! Rules, in short:
//! - Ticks with the same [`Sample`] grow one `Active` block; a different
//!   sample closes it and opens the next; no sample leaves a gap.
//! - When a tick reports no input for `idle_secs` or longer, the active block
//!   is cut back to the last input and an `Away` block runs from there. When
//!   input returns the away block closes and [`Out::Returned`] is emitted.
//! - Lock and sleep close the active block and open an away block; unlock and
//!   wake close it (also with `Returned`). While locked, ticks only extend the
//!   away block: tracking continues at `Unlock`.
//! - Pause closes everything and records nothing until `Unpause`.
//! - Two events further apart than `max_gap_secs` mean nobody was watching:
//!   the active block ends at the earlier event and the gap becomes an away
//!   block (so the user can still say what it was).
//! - If the clock is set back, the open block ends at the last time seen and
//!   nothing is recorded until the clock passes the end of what was already
//!   recorded: output never overlaps earlier output.
//! - No block crosses a local midnight: it is closed at midnight and a new
//!   block (new id) continues. `Returned` then names the last piece.
//! - The open block is emitted once it has a duration, again at most every
//!   `checkpoint_secs`, and when it closes.

use crate::model::{new_id, Block, BlockId, Kind, TrackState};
use crate::timeutil::Tz;

/// The foreground window, after the privacy rules were applied.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Sample {
    /// Lower-case program file name.
    pub exe: String,
    /// Friendly program name.
    pub app: String,
    pub title: String,
    pub domain: String,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Event {
    /// What is in front right now. Sent on every foreground change and on
    /// every sampling tick (a few seconds apart).
    /// - `fg`: `None` when there is nothing to record (no window, or a
    ///   program on the never-record list): time passes but leaves a gap.
    /// - `idle_secs`: seconds since the last keyboard or mouse input.
    /// - `media`: something is keeping the screen on (video, a call) or a
    ///   fullscreen media program is in front.
    Tick {
        t: i64,
        fg: Option<Sample>,
        idle_secs: u32,
        media: bool,
    },
    /// The session was locked / unlocked.
    Lock {
        t: i64,
    },
    Unlock {
        t: i64,
    },
    /// The PC is going to sleep / woke up.
    Suspend {
        t: i64,
    },
    Resume {
        t: i64,
    },
    /// The user paused / resumed tracking. Paused time is not recorded at all.
    Pause {
        t: i64,
    },
    Unpause {
        t: i64,
    },
    /// Sign-out, shutdown or the tracker exiting: close everything open.
    Stop {
        t: i64,
    },
}

impl Event {
    pub fn time(&self) -> i64 {
        match self {
            Event::Tick { t, .. }
            | Event::Lock { t }
            | Event::Unlock { t }
            | Event::Suspend { t }
            | Event::Resume { t }
            | Event::Pause { t }
            | Event::Unpause { t }
            | Event::Stop { t } => *t,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Config {
    /// Seconds without input before time counts as away. The away block
    /// starts at the last input, not when the threshold is crossed.
    pub idle_secs: u32,
    /// When set, a `Tick` with `media == true` is never idle.
    pub media_counts_active: bool,
    /// Two events further apart than this mean the tracker saw nothing in
    /// between (sleep without notice, a hang, the clock was set forward):
    /// the open block ends at the earlier event.
    pub max_gap_secs: u32,
    /// The open block is re-emitted at least this often so that a crash
    /// loses little.
    pub checkpoint_secs: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            idle_secs: 300,
            media_counts_active: true,
            max_gap_secs: 120,
            checkpoint_secs: 60,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub enum Out {
    /// Insert this block, or replace the stored block with the same id (an
    /// open block that grew, or was closed).
    Put(Block),
    /// A block emitted earlier must be removed again.
    Delete(BlockId),
    /// The user is back after being away for `[start, end)`; `id` is the
    /// away block, so the answer to "what were you doing?" can be stored.
    Returned { id: BlockId, start: i64, end: i64 },
}

/// The block that is still growing.
struct Open {
    block: Block,
    /// The sample of an active block (`None` for away blocks).
    sample: Option<Sample>,
    /// The end that was last emitted, if the block was emitted at all.
    emitted_end: Option<i64>,
    /// Event time of the last emit.
    emitted_at: i64,
}

pub struct SessionBuilder {
    config: Config,
    tz: Tz,
    open: Option<Open>,
    /// Time of the previous event.
    last_t: Option<i64>,
    /// End of the latest block that was closed: nothing new starts before it.
    high_water: i64,
    running: bool,
    paused: bool,
    locked: bool,
    asleep: bool,
    idle: bool,
}

impl SessionBuilder {
    pub fn new(config: Config, tz: Tz) -> SessionBuilder {
        SessionBuilder {
            config,
            tz,
            open: None,
            last_t: None,
            high_water: i64::MIN,
            running: false,
            paused: false,
            locked: false,
            asleep: false,
            idle: false,
        }
    }

    pub fn set_config(&mut self, config: Config) {
        self.config = config;
    }

    /// What the tracker is doing as of the last event.
    pub fn state(&self) -> TrackState {
        if self.paused {
            TrackState::Paused
        } else if !self.running {
            TrackState::Off
        } else if self.asleep {
            TrackState::Asleep
        } else if self.locked {
            TrackState::Locked
        } else if self.idle {
            TrackState::Idle
        } else {
            TrackState::Active
        }
    }

    /// The block that is still growing, if any.
    pub fn current(&self) -> Option<&Block> {
        self.open.as_ref().map(|o| &o.block)
    }

    fn open_is(&self, kind: Kind) -> bool {
        self.open.as_ref().is_some_and(|o| o.block.kind == kind)
    }

    /// Opens a block at `start` (never before what was already recorded).
    /// Nothing is opened while the clock is behind earlier output.
    fn open_block(&mut self, kind: Kind, sample: Option<Sample>, start: i64, now: i64) {
        let start = start.max(self.high_water);
        if start > now {
            return;
        }
        let s = sample.clone().unwrap_or_default();
        self.open = Some(Open {
            block: Block {
                id: new_id(),
                start,
                end: start,
                kind,
                exe: s.exe,
                app: s.app,
                title: s.title,
                domain: s.domain,
                ..Default::default()
            },
            sample,
            emitted_end: None,
            emitted_at: now,
        });
    }

    fn open_away(&mut self, start: i64, now: i64) {
        self.open_block(Kind::Away, None, start, now);
    }

    /// Closes the open block as it stands. Returns it if it has a duration.
    fn close(&mut self, out: &mut Vec<Out>) -> Option<Block> {
        let o = self.open.take()?;
        if o.block.end > o.block.start {
            if o.emitted_end != Some(o.block.end) {
                out.push(Out::Put(o.block.clone()));
            }
            self.high_water = self.high_water.max(o.block.end);
            Some(o.block)
        } else {
            if o.emitted_end.is_some() {
                out.push(Out::Delete(o.block.id));
            }
            None
        }
    }

    /// Closes an away block because the user is back.
    fn close_returned(&mut self, out: &mut Vec<Out>) {
        if let Some(b) = self.close(out) {
            out.push(Out::Returned {
                id: b.id,
                start: b.start,
                end: b.end,
            });
        }
    }

    /// Grows the open block to `t`, cutting it at every local midnight on the way.
    fn extend(&mut self, t: i64, out: &mut Vec<Out>) {
        loop {
            let Some(o) = self.open.as_mut() else { return };
            if t <= o.block.end {
                return;
            }
            let midnight = self.tz.day_start(self.tz.day_of(o.block.start) + 1);
            if t <= midnight || midnight <= o.block.start {
                o.block.end = t;
                return;
            }
            o.block.end = midnight;
            let (kind, sample) = (o.block.kind, o.sample.clone());
            self.close(out);
            self.open_block(kind, sample, midnight, t);
        }
    }

    /// Emits the open block if it is due.
    fn checkpoint(&mut self, t: i64, out: &mut Vec<Out>) {
        let every = self.config.checkpoint_secs as i64;
        if let Some(o) = self.open.as_mut() {
            let due = match o.emitted_end {
                None => true,
                Some(end) => end != o.block.end && t - o.emitted_at >= every,
            };
            if due && o.block.end > o.block.start {
                out.push(Out::Put(o.block.clone()));
                o.emitted_end = Some(o.block.end);
                o.emitted_at = t;
            }
        }
    }

    /// Lock and sleep: the active block ends, an away block runs.
    fn go_away(&mut self, t: i64, out: &mut Vec<Out>) {
        if self.paused {
            return;
        }
        if self.open_is(Kind::Active) {
            self.extend(t, out);
            self.close(out);
        }
        if self.open.is_none() {
            self.open_away(t, t);
        } else {
            self.extend(t, out);
        }
    }

    /// Unlock and wake: the away block ends, unless something else still
    /// keeps the user away.
    fn come_back(&mut self, t: i64, out: &mut Vec<Out>) {
        if self.paused || self.locked || self.asleep {
            return;
        }
        if self.open_is(Kind::Away) {
            self.extend(t, out);
            self.close_returned(out);
        }
        self.idle = false;
    }

    fn tick(
        &mut self,
        t: i64,
        fg: Option<Sample>,
        idle_secs: u32,
        media: bool,
        out: &mut Vec<Out>,
    ) {
        if self.paused {
            return;
        }
        // A tick can only arrive from a PC that is awake.
        self.asleep = false;
        if self.locked {
            if self.open.is_none() {
                self.open_away(t, t);
            }
            self.extend(t, out);
            return;
        }
        let last_input = t - idle_secs as i64;
        let is_idle =
            idle_secs >= self.config.idle_secs && !(media && self.config.media_counts_active);
        if is_idle {
            if self.open_is(Kind::Active) {
                // The time since the last input was not work: take it back.
                let (start, end) = self
                    .open
                    .as_ref()
                    .map(|o| (o.block.start, o.block.end))
                    .unwrap_or((t, t));
                let cut = last_input.clamp(start, t);
                if cut > end {
                    self.extend(cut, out);
                } else if let Some(o) = self.open.as_mut() {
                    o.block.end = cut;
                }
                let away_from = self.open.as_ref().map(|o| o.block.end).unwrap_or(cut);
                self.close(out);
                self.open_away(away_from, t);
            } else if self.open.is_none() {
                self.open_away(last_input.max(self.last_t.unwrap_or(t)), t);
            }
            self.idle = true;
            self.extend(t, out);
            return;
        }
        let mut start_at = t;
        if self.open_is(Kind::Away) {
            // Back: the absence ended when the input came.
            let seen = self.open.as_ref().map(|o| o.block.end).unwrap_or(t);
            let back = last_input.max(seen).min(t);
            self.extend(back, out);
            self.close_returned(out);
            start_at = back;
        }
        self.idle = false;
        match fg {
            None => {
                if self.open.is_some() {
                    self.extend(t, out);
                    self.close(out);
                }
            }
            Some(sample) => {
                let same = self
                    .open
                    .as_ref()
                    .is_some_and(|o| o.sample.as_ref() == Some(&sample));
                if !same {
                    if self.open.is_some() {
                        self.extend(t, out);
                        self.close(out);
                    }
                    self.open_block(Kind::Active, Some(sample), start_at, t);
                }
                self.extend(t, out);
            }
        }
    }

    /// Feeds one event; returns what to store. Events must be pushed in the
    /// order they happened; an event whose time lies before the previous one
    /// (the clock was set back) must not corrupt anything.
    pub fn push(&mut self, event: Event) -> Vec<Out> {
        let mut out = Vec::new();
        let t = event.time();

        if let Some(prev) = self.last_t {
            if t < prev {
                // The clock was set back: end what is open at the last time seen.
                self.close(&mut out);
                self.idle = false;
            } else if t - prev > self.config.max_gap_secs as i64 && !self.paused {
                // Nobody was watching in between.
                if self.open_is(Kind::Active) {
                    self.close(&mut out);
                }
                if self.open.is_none() && self.running {
                    self.open_away(prev, t);
                    self.idle = true;
                }
            }
        }

        match event {
            Event::Tick {
                fg,
                idle_secs,
                media,
                ..
            } => {
                self.running = true;
                self.tick(t, fg, idle_secs, media, &mut out);
            }
            Event::Lock { .. } => {
                self.running = true;
                self.locked = true;
                self.go_away(t, &mut out);
            }
            Event::Suspend { .. } => {
                self.running = true;
                self.asleep = true;
                self.go_away(t, &mut out);
            }
            Event::Unlock { .. } => {
                if self.locked {
                    self.locked = false;
                    self.come_back(t, &mut out);
                }
            }
            Event::Resume { .. } => {
                if self.asleep {
                    self.asleep = false;
                    self.come_back(t, &mut out);
                }
            }
            Event::Pause { .. } => {
                if !self.paused {
                    self.extend(t, &mut out);
                    if self.open_is(Kind::Away) {
                        // Pausing is something the user does: they are back.
                        self.close_returned(&mut out);
                    } else {
                        self.close(&mut out);
                    }
                    self.idle = false;
                    self.paused = true;
                }
            }
            Event::Unpause { .. } => {
                if self.paused {
                    self.paused = false;
                    self.running = true;
                    if self.locked || self.asleep {
                        self.open_away(t, t);
                    }
                }
            }
            Event::Stop { .. } => {
                self.extend(t, &mut out);
                self.close(&mut out);
                self.running = false;
                self.locked = false;
                self.asleep = false;
                self.idle = false;
                self.last_t = None;
                return out;
            }
        }

        self.checkpoint(t, &mut out);
        self.last_t = Some(t);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::AwayLabel;
    use crate::timeutil::days_from_civil;
    use std::collections::HashMap;

    /// Applies outputs the way the store would.
    #[derive(Default)]
    struct Sink {
        blocks: HashMap<BlockId, Block>,
        returned: Vec<(BlockId, i64, i64)>,
        puts: usize,
    }

    impl Sink {
        fn apply(&mut self, outs: Vec<Out>) {
            for o in outs {
                match o {
                    Out::Put(b) => {
                        self.puts += 1;
                        self.blocks.insert(b.id, b);
                    }
                    Out::Delete(id) => {
                        self.blocks.remove(&id);
                    }
                    Out::Returned { id, start, end } => self.returned.push((id, start, end)),
                }
            }
        }

        fn sorted(&self) -> Vec<Block> {
            let mut v: Vec<Block> = self.blocks.values().cloned().collect();
            v.sort_by_key(|b| (b.start, b.id));
            v
        }

        /// (kind letter, title, start, end) relative to `base`.
        fn shape(&self, base: i64) -> Vec<(char, String, i64, i64)> {
            self.sorted()
                .iter()
                .map(|b| {
                    let k = if b.kind == Kind::Active { 'a' } else { 'w' };
                    (k, b.title.clone(), b.start - base, b.end - base)
                })
                .collect()
        }
    }

    fn sample(title: &str) -> Sample {
        Sample {
            exe: "editor.exe".into(),
            app: "Editor".into(),
            title: title.into(),
            domain: String::new(),
        }
    }

    fn tick(t: i64, title: &str) -> Event {
        Event::Tick {
            t,
            fg: Some(sample(title)),
            idle_secs: 0,
            media: false,
        }
    }

    fn idle_tick(t: i64, title: &str, idle_secs: u32) -> Event {
        Event::Tick {
            t,
            fg: Some(sample(title)),
            idle_secs,
            media: false,
        }
    }

    fn row(k: char, title: &str, s: i64, e: i64) -> (char, String, i64, i64) {
        (k, title.to_string(), s, e)
    }

    /// Noon UTC on an ordinary day, far from any midnight in UTC.
    const BASE: i64 = 1_800_000_000 - 1_800_000_000 % 86_400 + 12 * 3600;

    fn builder() -> SessionBuilder {
        SessionBuilder::new(Config::default(), Tz::Fixed(0))
    }

    /// A builder that tolerates long silences between ticks.
    fn patient() -> SessionBuilder {
        SessionBuilder::new(
            Config {
                max_gap_secs: 2000,
                ..Config::default()
            },
            Tz::Fixed(0),
        )
    }

    fn run(b: &mut SessionBuilder, sink: &mut Sink, events: Vec<Event>) {
        for e in events {
            sink.apply(b.push(e));
        }
    }

    #[test]
    fn same_window_merges_and_focus_changes_split() {
        let mut b = builder();
        let mut sink = Sink::default();
        assert_eq!(b.state(), TrackState::Off);
        assert!(b.current().is_none());
        run(
            &mut b,
            &mut sink,
            vec![
                tick(BASE, "one"),
                tick(BASE + 5, "one"),
                tick(BASE + 10, "one"),
                tick(BASE + 12, "two"),
                tick(BASE + 15, "two"),
                tick(BASE + 20, "one"),
                tick(BASE + 25, "one"),
            ],
        );
        assert_eq!(b.state(), TrackState::Active);
        assert_eq!(
            sink.shape(BASE),
            vec![
                row('a', "one", 0, 12),
                row('a', "two", 12, 20),
                row('a', "one", 20, 25)
            ]
        );
        let cur = b.current().unwrap();
        assert_eq!(
            (cur.title.as_str(), cur.exe.as_str(), cur.app.as_str()),
            ("one", "editor.exe", "Editor")
        );
        assert_eq!(cur.kind, Kind::Active);
        // Three distinct ids.
        assert_eq!(sink.blocks.len(), 3);
        // A change of domain alone is a change too.
        let mut with_domain = sample("one");
        with_domain.domain = "docs.example".into();
        sink.apply(b.push(Event::Tick {
            t: BASE + 30,
            fg: Some(with_domain),
            idle_secs: 0,
            media: false,
        }));
        sink.apply(b.push(Event::Stop { t: BASE + 40 }));
        let v = sink.sorted();
        assert_eq!(v.len(), 4);
        assert_eq!(v[3].domain, "docs.example");
        assert_eq!(
            (v[2].end, v[3].start, v[3].end),
            (BASE + 30, BASE + 30, BASE + 40)
        );
    }

    #[test]
    fn nothing_in_front_leaves_a_gap() {
        let mut b = builder();
        let mut sink = Sink::default();
        let none = |t| Event::Tick {
            t,
            fg: None,
            idle_secs: 0,
            media: false,
        };
        run(
            &mut b,
            &mut sink,
            vec![
                tick(BASE, "one"),
                tick(BASE + 10, "one"),
                none(BASE + 20),
                none(BASE + 30),
                tick(BASE + 40, "one"),
                tick(BASE + 50, "one"),
            ],
        );
        assert_eq!(
            sink.shape(BASE),
            vec![row('a', "one", 0, 20), row('a', "one", 40, 50)]
        );
        assert!(sink.returned.is_empty());
    }

    #[test]
    fn idle_is_backdated_to_the_last_input_and_return_is_reported() {
        let mut b = builder();
        let mut sink = Sink::default();
        // Input stops at BASE+100; ticks keep coming every 10 s.
        let mut events = vec![tick(BASE, "doc")];
        for t in (10..=100).step_by(10) {
            events.push(tick(BASE + t, "doc"));
        }
        for t in (110..=390).step_by(10) {
            events.push(idle_tick(BASE + t, "doc", (t - 100) as u32));
        }
        run(&mut b, &mut sink, events);
        // Not yet 300 s without input: still one active block.
        assert_eq!(b.state(), TrackState::Active);
        assert_eq!(sink.blocks.len(), 1);
        assert_eq!(b.current().map(|c| c.end), Some(BASE + 390));
        // The threshold is crossed: the active block shrinks back.
        sink.apply(b.push(idle_tick(BASE + 400, "doc", 300)));
        assert_eq!(b.state(), TrackState::Idle);
        assert_eq!(b.current().unwrap().kind, Kind::Away);
        assert_eq!(b.current().unwrap().away, AwayLabel::None);
        assert_eq!(
            sink.shape(BASE),
            vec![row('a', "doc", 0, 100), row('w', "", 100, 400)]
        );
        run(
            &mut b,
            &mut sink,
            vec![
                idle_tick(BASE + 500, "doc", 400),
                idle_tick(BASE + 900, "doc", 800),
                // Input came back 3 s before this tick.
                idle_tick(BASE + 1000, "doc", 3),
                tick(BASE + 1010, "doc"),
            ],
        );
        assert_eq!(b.state(), TrackState::Active);
        sink.apply(b.push(Event::Stop { t: BASE + 1010 }));
        let shape = sink.shape(BASE);
        assert_eq!(
            shape,
            vec![
                row('a', "doc", 0, 100),
                row('w', "", 100, 997),
                row('a', "doc", 997, 1010)
            ]
        );
        let away = sink.sorted()[1].clone();
        assert_eq!(sink.returned, vec![(away.id, BASE + 100, BASE + 997)]);
    }

    #[test]
    fn a_window_that_appeared_while_idle_is_not_work() {
        let mut b = patient();
        let mut sink = Sink::default();
        run(
            &mut b,
            &mut sink,
            vec![
                tick(BASE, "doc"),
                idle_tick(BASE + 200, "doc", 100),
                // A pop-up takes the foreground without any input.
                idle_tick(BASE + 250, "popup", 150),
                idle_tick(BASE + 290, "popup", 190),
            ],
        );
        assert_eq!(sink.blocks.len(), 2);
        sink.apply(b.push(idle_tick(BASE + 400, "popup", 300)));
        // The pop-up block is taken back entirely; away starts where it began.
        assert_eq!(
            sink.shape(BASE),
            vec![row('a', "doc", 0, 250), row('w', "", 250, 400)]
        );
    }

    #[test]
    fn media_counts_as_active_when_enabled() {
        let watch = |t: i64, idle: u32| Event::Tick {
            t,
            fg: Some(sample("film")),
            idle_secs: idle,
            media: true,
        };
        let mut b = patient();
        let mut sink = Sink::default();
        run(
            &mut b,
            &mut sink,
            vec![
                watch(BASE, 0),
                watch(BASE + 100, 100),
                watch(BASE + 1000, 1000),
            ],
        );
        assert_eq!(b.state(), TrackState::Active);
        assert_eq!(sink.shape(BASE), vec![row('a', "film", 0, 1000)]);

        let mut b = SessionBuilder::new(
            Config {
                media_counts_active: false,
                max_gap_secs: 2000,
                ..Config::default()
            },
            Tz::Fixed(0),
        );
        let mut sink = Sink::default();
        run(
            &mut b,
            &mut sink,
            vec![
                watch(BASE, 0),
                watch(BASE + 100, 100),
                watch(BASE + 1000, 1000),
            ],
        );
        assert_eq!(b.state(), TrackState::Idle);
        assert_eq!(
            sink.shape(BASE),
            vec![row('w', "", 0, 1000)],
            "the whole stretch had no input"
        );
    }

    #[test]
    fn lock_and_unlock() {
        let mut b = builder();
        let mut sink = Sink::default();
        run(
            &mut b,
            &mut sink,
            vec![
                tick(BASE, "doc"),
                tick(BASE + 50, "doc"),
                Event::Lock { t: BASE + 60 },
            ],
        );
        assert_eq!(b.state(), TrackState::Locked);
        run(
            &mut b,
            &mut sink,
            vec![
                // Ticks while locked (the lock screen is in front) change nothing.
                tick(BASE + 100, "lock screen"),
                tick(BASE + 200, "lock screen"),
                Event::Lock { t: BASE + 210 },
                Event::Unlock { t: BASE + 300 },
            ],
        );
        assert_eq!(b.state(), TrackState::Active);
        run(
            &mut b,
            &mut sink,
            vec![tick(BASE + 305, "doc"), tick(BASE + 320, "doc")],
        );
        assert_eq!(
            sink.shape(BASE),
            vec![
                row('a', "doc", 0, 60),
                row('w', "", 60, 300),
                row('a', "doc", 305, 320)
            ]
        );
        assert_eq!(sink.returned.len(), 1);
        assert_eq!(
            (sink.returned[0].1, sink.returned[0].2),
            (BASE + 60, BASE + 300)
        );
        // Unlock without lock, resume without suspend: harmless.
        let before = sink.shape(BASE);
        run(
            &mut b,
            &mut sink,
            vec![
                Event::Unlock { t: BASE + 325 },
                Event::Resume { t: BASE + 326 },
                tick(BASE + 330, "doc"),
            ],
        );
        assert_eq!(sink.blocks.len(), before.len());
        assert_eq!(sink.returned.len(), 1);
    }

    #[test]
    fn locking_while_idle_keeps_one_away_block() {
        let mut b = builder();
        let mut sink = Sink::default();
        run(
            &mut b,
            &mut sink,
            vec![
                tick(BASE, "doc"),
                idle_tick(BASE + 100, "doc", 100),
                idle_tick(BASE + 200, "doc", 200),
                idle_tick(BASE + 300, "doc", 300),
                // The screen saver locks the session.
                Event::Lock { t: BASE + 600 },
                tick(BASE + 700, "lock screen"),
                Event::Unlock { t: BASE + 900 },
                tick(BASE + 905, "doc"),
                tick(BASE + 910, "doc"),
            ],
        );
        assert_eq!(
            sink.shape(BASE),
            vec![row('w', "", 0, 900), row('a', "doc", 905, 910)]
        );
        assert_eq!(sink.returned.len(), 1);
    }

    #[test]
    fn suspend_and_resume() {
        let mut b = builder();
        let mut sink = Sink::default();
        run(
            &mut b,
            &mut sink,
            vec![
                tick(BASE, "doc"),
                tick(BASE + 30, "doc"),
                Event::Suspend { t: BASE + 40 },
            ],
        );
        assert_eq!(b.state(), TrackState::Asleep);
        run(
            &mut b,
            &mut sink,
            vec![
                // Hours later; Windows locks on wake.
                Event::Resume { t: BASE + 7200 },
                Event::Lock { t: BASE + 7201 },
            ],
        );
        assert_eq!(b.state(), TrackState::Locked);
        run(
            &mut b,
            &mut sink,
            vec![
                Event::Unlock { t: BASE + 7260 },
                tick(BASE + 7262, "doc"),
                tick(BASE + 7270, "doc"),
            ],
        );
        // The resume closed the sleep block; the lock right after opened the next.
        assert_eq!(
            sink.shape(BASE),
            vec![
                row('a', "doc", 0, 40),
                row('w', "", 40, 7200),
                row('w', "", 7201, 7260),
                row('a', "doc", 7262, 7270)
            ]
        );
        assert_eq!(sink.returned.len(), 2);

        // Lock first, then sleep, wake, unlock: one away block.
        let mut b = builder();
        let mut sink = Sink::default();
        run(
            &mut b,
            &mut sink,
            vec![
                tick(BASE, "doc"),
                Event::Lock { t: BASE + 10 },
                Event::Suspend { t: BASE + 20 },
                Event::Resume { t: BASE + 5000 },
                Event::Unlock { t: BASE + 5010 },
                tick(BASE + 5012, "doc"),
                // A suspend that is never followed by a resume event.
                Event::Suspend { t: BASE + 5020 },
                tick(BASE + 9000, "doc"),
                tick(BASE + 9010, "doc"),
            ],
        );
        assert_eq!(b.state(), TrackState::Active);
        assert_eq!(
            sink.shape(BASE),
            vec![
                row('a', "doc", 0, 10),
                row('w', "", 10, 5010),
                row('a', "doc", 5012, 5020),
                row('w', "", 5020, 9000),
                row('a', "doc", 9000, 9010)
            ]
        );
    }

    #[test]
    fn sleep_without_notice_and_clock_set_forward_become_away() {
        let mut b = builder();
        let mut sink = Sink::default();
        run(
            &mut b,
            &mut sink,
            vec![
                tick(BASE, "doc"),
                tick(BASE + 5, "doc"),
                tick(BASE + 10, "doc"),
                // Nothing for an hour (lid closed, a hang, or the clock jumped ahead).
                tick(BASE + 3610, "doc"),
                tick(BASE + 3620, "doc"),
            ],
        );
        assert_eq!(
            sink.shape(BASE),
            vec![
                row('a', "doc", 0, 10),
                row('w', "", 10, 3610),
                row('a', "doc", 3610, 3620)
            ]
        );
        assert_eq!(sink.returned.len(), 1);
        assert_eq!(
            (sink.returned[0].1, sink.returned[0].2),
            (BASE + 10, BASE + 3610)
        );
        // A gap just inside the limit is ordinary tracked time.
        let mut b = builder();
        let mut sink = Sink::default();
        run(
            &mut b,
            &mut sink,
            vec![tick(BASE, "doc"), tick(BASE + 120, "doc")],
        );
        assert_eq!(sink.shape(BASE), vec![row('a', "doc", 0, 120)]);
        // A stop long after the last event does not stretch the active block.
        let mut b = builder();
        let mut sink = Sink::default();
        run(
            &mut b,
            &mut sink,
            vec![
                tick(BASE, "doc"),
                tick(BASE + 10, "doc"),
                Event::Stop { t: BASE + 5000 },
            ],
        );
        assert_eq!(
            sink.shape(BASE),
            vec![row('a', "doc", 0, 10), row('w', "", 10, 5000)]
        );
        assert!(sink.returned.is_empty());
    }

    #[test]
    fn pause_records_nothing() {
        let mut b = builder();
        let mut sink = Sink::default();
        run(
            &mut b,
            &mut sink,
            vec![
                tick(BASE, "doc"),
                tick(BASE + 20, "doc"),
                Event::Pause { t: BASE + 30 },
            ],
        );
        assert_eq!(b.state(), TrackState::Paused);
        assert!(b.current().is_none());
        run(
            &mut b,
            &mut sink,
            vec![
                tick(BASE + 40, "secret"),
                tick(BASE + 500, "secret"),
                Event::Lock { t: BASE + 600 },
                Event::Unlock { t: BASE + 700 },
                Event::Pause { t: BASE + 800 },
                tick(BASE + 2000, "secret"),
                Event::Unpause { t: BASE + 3000 },
            ],
        );
        assert_eq!(b.state(), TrackState::Active);
        run(
            &mut b,
            &mut sink,
            vec![tick(BASE + 3005, "doc"), tick(BASE + 3010, "doc")],
        );
        assert_eq!(
            sink.shape(BASE),
            vec![row('a', "doc", 0, 30), row('a', "doc", 3005, 3010)]
        );
        assert!(sink.returned.is_empty());
        // Unpausing while the session is locked goes straight to away.
        run(
            &mut b,
            &mut sink,
            vec![
                Event::Pause { t: BASE + 3020 },
                Event::Lock { t: BASE + 3030 },
                Event::Unpause { t: BASE + 3040 },
            ],
        );
        assert_eq!(b.state(), TrackState::Locked);
        run(&mut b, &mut sink, vec![Event::Unlock { t: BASE + 3100 }]);
        let shape = sink.shape(BASE);
        assert_eq!(shape[1], row('a', "doc", 3005, 3020));
        assert_eq!(shape[2], row('w', "", 3040, 3100));
    }

    #[test]
    fn stop_closes_everything_and_a_restart_is_clean() {
        let mut b = builder();
        let mut sink = Sink::default();
        run(
            &mut b,
            &mut sink,
            vec![
                tick(BASE, "doc"),
                tick(BASE + 7, "doc"),
                Event::Stop { t: BASE + 9 },
            ],
        );
        assert_eq!(b.state(), TrackState::Off);
        assert!(b.current().is_none());
        assert_eq!(sink.shape(BASE), vec![row('a', "doc", 0, 9)]);
        // Started again much later: no gap block, no merge with the old block.
        run(
            &mut b,
            &mut sink,
            vec![
                tick(BASE + 50_000, "doc"),
                tick(BASE + 50_005, "doc"),
                Event::Stop { t: BASE + 50_005 },
            ],
        );
        assert_eq!(
            sink.shape(BASE),
            vec![row('a', "doc", 0, 9), row('a', "doc", 50_000, 50_005)]
        );
        // A block that never got a duration leaves nothing behind.
        let mut b = builder();
        let mut sink = Sink::default();
        run(
            &mut b,
            &mut sink,
            vec![tick(BASE, "doc"), Event::Stop { t: BASE }],
        );
        assert!(sink.blocks.is_empty());
    }

    #[test]
    fn blocks_are_split_at_local_midnight() {
        // Local time is UTC+3: local midnight is 21:00 UTC.
        let tz = Tz::Fixed(3 * 3600);
        let day = days_from_civil(2026, 11, 12);
        let midnight = tz.day_start(day + 1);
        let mut b = SessionBuilder::new(Config::default(), tz.clone());
        let mut sink = Sink::default();
        let mut events = Vec::new();
        for t in (-60..=60).step_by(7) {
            events.push(tick(midnight + t, "late night"));
        }
        events.push(Event::Lock { t: midnight + 100 });
        // Locked across the next midnight too.
        events.push(Event::Unlock {
            t: midnight + 86_400 + 500,
        });
        run(&mut b, &mut sink, events);
        let shape = sink.shape(midnight);
        assert_eq!(
            shape,
            vec![
                row('a', "late night", -60, 0),
                row('a', "late night", 0, 100),
                row('w', "", 100, 86_400),
                row('w', "", 86_400, 86_900),
            ]
        );
        for blk in sink.sorted() {
            assert_eq!(tz.day_of(blk.start), tz.day_of(blk.end - 1));
        }
        // The question is about the last piece.
        assert_eq!(sink.returned.len(), 1);
        assert_eq!(sink.returned[0].1, midnight + 86_400);
    }

    #[test]
    fn daylight_saving_days() {
        // Central European rules for 2026.
        let fwd = days_from_civil(2026, 3, 29) as i64 * 86_400 + 3600;
        let back = days_from_civil(2026, 10, 25) as i64 * 86_400 + 3600;
        let tz = Tz::with_dst(3600, 7200, fwd, back);
        for (y, m, d, hours) in [(2026, 3, 29, 23), (2026, 10, 25, 25)] {
            let day = days_from_civil(y, m, d);
            let (start, end) = tz.day_bounds(day);
            assert_eq!(end - start, hours * 3600);
            let mut b = SessionBuilder::new(
                Config {
                    max_gap_secs: 4000,
                    ..Config::default()
                },
                tz.clone(),
            );
            let mut sink = Sink::default();
            // Work from an hour before the day starts until an hour after it ends.
            let mut t = start - 3600;
            while t <= end + 3600 {
                sink.apply(b.push(tick(t, "all day")));
                t += 1800;
            }
            sink.apply(b.push(Event::Stop { t: end + 3600 }));
            let v = sink.sorted();
            assert_eq!(v.len(), 3);
            assert_eq!((v[0].start, v[0].end), (start - 3600, start));
            // The middle block is exactly the 23- or 25-hour day.
            assert_eq!((v[1].start, v[1].end), (start, end));
            assert_eq!((v[2].start, v[2].end), (end, end + 3600));
            assert_eq!(tz.day_of(v[1].start), day);
            assert_eq!(tz.day_of(v[1].end - 1), day);
        }
    }

    #[test]
    fn clock_set_back_never_overlaps_earlier_output() {
        let mut b = builder();
        let mut sink = Sink::default();
        run(
            &mut b,
            &mut sink,
            vec![
                tick(BASE, "doc"),
                tick(BASE + 50, "doc"),
                tick(BASE + 100, "doc"),
                // The clock is set back by 70 s.
                tick(BASE + 30, "doc"),
                tick(BASE + 60, "doc"),
                tick(BASE + 90, "doc"),
            ],
        );
        // Nothing new until the clock passes what was already recorded.
        assert_eq!(sink.shape(BASE), vec![row('a', "doc", 0, 100)]);
        assert!(b.current().is_none());
        run(
            &mut b,
            &mut sink,
            vec![tick(BASE + 120, "doc"), tick(BASE + 150, "doc")],
        );
        assert_eq!(
            sink.shape(BASE),
            vec![row('a', "doc", 0, 100), row('a', "doc", 120, 150)]
        );
        assert_eq!(sink.blocks.len(), 2);
        // Set back while away: the away block ends at the last time seen.
        let mut b = builder();
        let mut sink = Sink::default();
        run(
            &mut b,
            &mut sink,
            vec![
                tick(BASE, "doc"),
                Event::Lock { t: BASE + 10 },
                tick(BASE + 100, "x"),
                Event::Unlock { t: BASE + 40 },
                tick(BASE + 50, "doc"),
                tick(BASE + 130, "doc"),
                tick(BASE + 160, "doc"),
            ],
        );
        for blk in sink.sorted() {
            assert!(blk.start < blk.end);
        }
        assert_eq!(
            sink.shape(BASE),
            vec![
                row('a', "doc", 0, 10),
                row('w', "", 10, 100),
                row('a', "doc", 130, 160)
            ]
        );
    }

    #[test]
    fn checkpoints_come_at_the_configured_pace() {
        let mut b = builder();
        let mut sink = Sink::default();
        let mut puts_at = Vec::new();
        for t in (0..=300).step_by(5) {
            let outs = b.push(tick(BASE + t, "doc"));
            if outs.iter().any(|o| matches!(o, Out::Put(_))) {
                puts_at.push(t);
            }
            sink.apply(outs);
        }
        // First emit as soon as the block has a duration, then once a minute.
        assert_eq!(puts_at, vec![5, 65, 125, 185, 245]);
        assert_eq!(sink.blocks.len(), 1);
        assert_eq!(sink.sorted()[0].end, BASE + 245);
        // Closing emits the final state exactly once.
        let outs = b.push(Event::Stop { t: BASE + 300 });
        assert_eq!(outs.len(), 1);
        sink.apply(outs);
        assert_eq!(sink.sorted()[0].end, BASE + 300);
        // A faster pace through set_config.
        let mut b = builder();
        b.set_config(Config {
            checkpoint_secs: 10,
            ..Config::default()
        });
        let mut puts = 0;
        for t in (0..=100).step_by(5) {
            puts += b
                .push(tick(BASE + t, "doc"))
                .iter()
                .filter(|o| matches!(o, Out::Put(_)))
                .count();
        }
        assert_eq!(puts, 10);
    }

    struct Rng(u64);

    impl Rng {
        fn below(&mut self, n: u64) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0 % n
        }
    }

    #[test]
    fn invariants_hold_for_random_event_streams() {
        let fwd = days_from_civil(2026, 3, 29) as i64 * 86_400 + 3600;
        let back = days_from_civil(2026, 10, 25) as i64 * 86_400 + 3600;
        let tz = Tz::with_dst(3600, 7200, fwd, back);
        for seed in 1..=40u64 {
            let mut rng = Rng(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) | 1);
            let mut b = SessionBuilder::new(
                Config {
                    idle_secs: 60 + rng.below(600) as u32,
                    media_counts_active: rng.below(2) == 0,
                    max_gap_secs: 30 + rng.below(300) as u32,
                    checkpoint_secs: 1 + rng.below(120) as u32,
                },
                tz.clone(),
            );
            // Start shortly before the spring change, some seeds before the autumn one.
            let origin = if seed % 2 == 0 { fwd } else { back } - 20 * 3600;
            let mut t = origin;
            let (mut lo, mut hi) = (t, t);
            let mut sink = Sink::default();
            let mut idle = 0u32;
            for _ in 0..1500 {
                // Mostly small steps; sometimes a long silence; rarely the clock goes back.
                t += match rng.below(100) {
                    0..=84 => 1 + rng.below(20) as i64,
                    85..=95 => 60 + rng.below(4000) as i64,
                    96..=97 => 20_000 + rng.below(60_000) as i64,
                    _ => -(1 + rng.below(900) as i64),
                };
                lo = lo.min(t);
                hi = hi.max(t);
                let event = match rng.below(100) {
                    0..=74 => {
                        idle = if rng.below(4) == 0 {
                            0
                        } else {
                            idle + rng.below(200) as u32
                        };
                        Event::Tick {
                            t,
                            fg: (rng.below(12) != 0)
                                .then(|| sample(["a", "b", "c"][rng.below(3) as usize])),
                            idle_secs: idle,
                            media: rng.below(10) == 0,
                        }
                    }
                    75..=79 => Event::Lock { t },
                    80..=84 => Event::Unlock { t },
                    85..=87 => Event::Suspend { t },
                    88..=90 => Event::Resume { t },
                    91..=93 => Event::Pause { t },
                    94..=97 => Event::Unpause { t },
                    _ => Event::Stop { t },
                };
                sink.apply(b.push(event));
                if let Some(cur) = b.current() {
                    assert!(cur.start <= cur.end, "seed {seed}");
                }
            }
            sink.apply(b.push(Event::Stop { t }));
            let blocks = sink.sorted();
            assert!(blocks.len() > 20, "seed {seed}: the stream produced blocks");
            let mut total = 0;
            for (i, blk) in blocks.iter().enumerate() {
                assert!(blk.start < blk.end, "seed {seed}: empty block");
                assert!(matches!(blk.kind, Kind::Active | Kind::Away));
                assert_eq!(
                    tz.day_of(blk.start),
                    tz.day_of(blk.end - 1),
                    "seed {seed}: block crosses a local midnight"
                );
                assert!(
                    blk.start >= lo && blk.end <= hi,
                    "seed {seed}: outside the stream"
                );
                if i > 0 {
                    assert!(
                        blocks[i - 1].end <= blk.start,
                        "seed {seed}: blocks overlap"
                    );
                }
                assert_eq!(blk.kind == Kind::Active, !blk.exe.is_empty());
                total += blk.end - blk.start;
            }
            assert!(total <= hi - lo, "seed {seed}: more time than passed");
            // Every reported absence is a stored away block with those times.
            for (id, start, end) in &sink.returned {
                let blk = sink.blocks.get(id).expect("returned block exists");
                assert_eq!((blk.kind, blk.start, blk.end), (Kind::Away, *start, *end));
            }
        }
    }
}
