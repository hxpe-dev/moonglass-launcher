//! Interface sounds and background music. Everything audible runs on one
//! thread that owns the sound device; the UI sends it commands and never
//! waits. Without a sound device (or with one that cannot be opened) the
//! commands are dropped and Moonglass stays silent.
//!
//! The interface sounds are synthesized at startup: short soft tones, so
//! there are no sound files to ship. The music is the player's own: the
//! audio files of a folder chosen in Settings, shuffled.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rodio::buffer::SamplesBuffer;
use rodio::{Decoder, MixerDeviceSink, Player, Source};

/// An interface sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sfx {
    /// The focus moved (cards, rows, keys).
    Move,
    /// Another filter pill.
    Filter,
    /// Typed a key on the on-screen keyboard.
    Key,
    Confirm,
    Back,
    /// A setting changed.
    Toggle,
    /// A menu, sheet or Settings opened.
    Open,
    Close,
    /// A game is starting.
    Launch,
    /// Something could not be done.
    Error,
}

impl Sfx {
    const ALL: [Sfx; 10] = [
        Sfx::Move,
        Sfx::Filter,
        Sfx::Key,
        Sfx::Confirm,
        Sfx::Back,
        Sfx::Toggle,
        Sfx::Open,
        Sfx::Close,
        Sfx::Launch,
        Sfx::Error,
    ];

    /// Of several sounds asked for by one action, the highest is played.
    pub fn priority(self) -> u8 {
        match self {
            Sfx::Key => 0,
            Sfx::Move => 1,
            Sfx::Filter => 2,
            Sfx::Toggle | Sfx::Confirm | Sfx::Back => 3,
            Sfx::Open | Sfx::Close => 4,
            Sfx::Error => 5,
            Sfx::Launch => 6,
        }
    }
}

/// Music files Moonglass can play.
pub const MUSIC_EXTENSIONS: [&str; 5] = ["mp3", "flac", "ogg", "oga", "wav"];

/// How deep into subfolders the music folder is searched.
const MUSIC_DEPTH: usize = 4;

/// The audio files under `dir`, in no particular order.
pub fn music_files(dir: &Path) -> Vec<PathBuf> {
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return;
        };
        for e in rd.flatten() {
            let path = e.path();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                if depth < MUSIC_DEPTH {
                    walk(&path, depth + 1, out);
                }
            } else if path
                .extension()
                .is_some_and(|x| MUSIC_EXTENSIONS.iter().any(|m| x.eq_ignore_ascii_case(m)))
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, 0, &mut out);
    out
}

/// A volume setting (0 to 100) as a gain. Squared, so the steps sound even.
pub fn gain(percent: u8) -> f32 {
    let p = f32::from(percent.min(100)) / 100.0;
    p * p
}

enum Cmd {
    Sfx(Sfx),
    SfxVolume(f32),
    MusicVolume(f32),
    MusicFolder(Option<PathBuf>),
    /// Whether music may play now (not during a game, not while Moonglass
    /// is in the background).
    MusicActive(bool),
    NextTrack,
    /// The previous song, or the start of this one once it has played a
    /// few seconds.
    PreviousTrack,
    TogglePause,
}

/// The song shown on the home screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowPlaying {
    pub title: String,
    /// Paused by the player (not because a game is running).
    pub paused: bool,
    /// How far into the song.
    pub position: Duration,
    /// How long the song is, when its file says.
    pub length: Option<Duration>,
}

impl NowPlaying {
    /// 0 to 1 through the song, if its length is known.
    pub fn progress(&self) -> Option<f32> {
        let len = self.length?.as_secs_f32();
        (len > 0.0).then(|| (self.position.as_secs_f32() / len).clamp(0.0, 1.0))
    }
}

/// "1:05", or "1:02:05" past an hour.
pub fn clock(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// A song's name from its file: "03 - Artist - Song.mp3" is
/// "Artist - Song".
pub fn track_title(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().trim().to_string())
        .unwrap_or_default();
    let rest = stem.trim_start_matches(|c: char| c.is_ascii_digit());
    if rest.len() < stem.len() {
        if let Some(r) = rest.trim_start().strip_prefix(['-', '.', '_']) {
            if !r.trim().is_empty() {
                return r.trim().to_string();
            }
        }
    }
    stem
}

/// The handle the UI keeps. Cheap to clone.
#[derive(Clone)]
pub struct Audio {
    tx: Sender<Cmd>,
    now: Arc<Mutex<Option<NowPlaying>>>,
}

impl Audio {
    /// Starts the audio thread. Volumes start at zero and music inactive
    /// until the UI says otherwise.
    pub fn start() -> Self {
        let (tx, rx) = channel();
        let now = Arc::new(Mutex::new(None));
        let shared = now.clone();
        if let Err(e) = std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || Engine::new(shared).run(rx))
        {
            tracing::warn!(error = %e, "audio thread did not start");
        }
        Self { tx, now }
    }

    pub fn previous_track(&self) {
        self.send(Cmd::PreviousTrack);
    }

    pub fn toggle_pause(&self) {
        self.send(Cmd::TogglePause);
    }

    /// The current song, when there is music.
    pub fn now_playing(&self) -> Option<NowPlaying> {
        self.now.lock().ok().and_then(|n| n.clone())
    }

    fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }

    pub fn play(&self, s: Sfx) {
        self.send(Cmd::Sfx(s));
    }

    pub fn set_sfx_volume(&self, percent: u8) {
        self.send(Cmd::SfxVolume(gain(percent)));
    }

    pub fn set_music_volume(&self, percent: u8) {
        self.send(Cmd::MusicVolume(gain(percent)));
    }

    pub fn set_music_folder(&self, dir: Option<PathBuf>) {
        self.send(Cmd::MusicFolder(dir));
    }

    pub fn set_music_active(&self, active: bool) {
        self.send(Cmd::MusicActive(active));
    }

    pub fn next_track(&self) {
        self.send(Cmd::NextTrack);
    }
}

/// How often the audio thread wakes to fade and queue music.
const STEP: Duration = Duration::from_millis(30);
/// Wait before trying a sound device that failed to open again.
const DEVICE_RETRY: Duration = Duration::from_secs(10);
/// Share of the remaining distance the music gain moves each step: about
/// half a second to fade in or out.
const FADE: f32 = 0.12;
/// The same for a pause from the button: about a fifth of a second.
const FADE_PAUSE: f32 = 0.4;

struct Engine {
    device: Option<MixerDeviceSink>,
    device_failed_at: Option<Instant>,
    sounds: Vec<(Sfx, SamplesBuffer)>,
    sfx_volume: f32,

    music: Option<Player>,
    music_volume: f32,
    music_active: bool,
    /// The music player's volume right now, fading toward its target.
    music_gain: f32,
    tracks: Vec<PathBuf>,
    /// The current song in `tracks`, which is reshuffled after each pass.
    pos: usize,
    /// The player holds the current song (it has been opened). When the
    /// player then runs empty, the song has ended.
    loaded: bool,
    /// Paused with the home screen's button.
    user_paused: bool,
    /// The open song's length, when its file says.
    length: Option<Duration>,
    /// Where the song was when the button paused it.
    paused_at: Option<Duration>,
    /// Tracks that failed to open in a row: all of them failing stops the
    /// music until the folder changes.
    failures: usize,
    rng: u64,
    /// What the UI shows, and what was last put there.
    now: Arc<Mutex<Option<NowPlaying>>>,
    shown: Option<NowPlaying>,
}

/// Previous restarts the song instead once it has played this long.
const RESTART_AFTER: Duration = Duration::from_secs(3);

impl Engine {
    fn new(now: Arc<Mutex<Option<NowPlaying>>>) -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0x9E37_79B9_7F4A_7C15, |d| d.as_nanos() as u64);
        Self {
            device: None,
            device_failed_at: None,
            sounds: Sfx::ALL.iter().map(|&s| (s, synth::render(s))).collect(),
            sfx_volume: 0.0,
            music: None,
            music_volume: 0.0,
            music_active: false,
            music_gain: 0.0,
            tracks: Vec::new(),
            pos: 0,
            loaded: false,
            user_paused: false,
            length: None,
            paused_at: None,
            failures: 0,
            rng: seed | 1,
            now,
            shown: None,
        }
    }

    /// Moves to another song: `delta` 1 is the next one (reshuffling after
    /// the last), -1 the previous one.
    fn step_track(&mut self, delta: isize) {
        let n = self.tracks.len();
        if n == 0 {
            return;
        }
        self.loaded = false;
        if delta >= 0 {
            self.pos += 1;
            if self.pos >= n {
                self.pos = 0;
                self.shuffle();
            }
        } else {
            self.pos = self.pos.checked_sub(1).unwrap_or(n - 1);
        }
    }

    /// Drops what the player holds, so a later step opens `pos`. Skipping
    /// (rather than `Player::clear`, which waits for the sound card) keeps
    /// this thread from blocking.
    fn reload(&mut self) {
        self.loaded = false;
        self.paused_at = None;
        if let Some(p) = &self.music {
            for _ in 0..p.len() {
                p.skip_one();
            }
        }
    }

    fn publish(&mut self) {
        // Until the song is opened, it is at its start.
        // Paused: where it was when the button was pressed, though the
        // music takes a moment more to fade out.
        let (position, length) = match &self.music {
            Some(p) if self.loaded => (
                self.paused_at
                    .filter(|_| self.user_paused)
                    .unwrap_or_else(|| p.get_pos()),
                self.length,
            ),
            _ => (Duration::ZERO, None),
        };
        let now = self.tracks.get(self.pos).map(|p| NowPlaying {
            title: track_title(p),
            paused: self.user_paused,
            position,
            length,
        });
        if now != self.shown {
            self.shown = now.clone();
            if let Ok(mut n) = self.now.lock() {
                *n = now;
            }
        }
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        loop {
            match rx.recv_timeout(STEP) {
                Ok(c) => self.command(c),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            // Everything queued, then one music step.
            while let Ok(c) = rx.try_recv() {
                self.command(c);
            }
            self.music_step();
        }
    }

    /// The sound device, opened on first use.
    fn device(&mut self) -> Option<&MixerDeviceSink> {
        if self.device.is_none()
            && self
                .device_failed_at
                .is_none_or(|t| t.elapsed() >= DEVICE_RETRY)
        {
            match rodio::DeviceSinkBuilder::open_default_sink() {
                Ok(mut d) => {
                    d.log_on_drop(false);
                    self.device = Some(d);
                    self.device_failed_at = None;
                }
                Err(e) => {
                    if self.device_failed_at.is_none() {
                        tracing::warn!(error = %e, "no sound device");
                    }
                    self.device_failed_at = Some(Instant::now());
                }
            }
        }
        self.device.as_ref()
    }

    fn command(&mut self, c: Cmd) {
        match c {
            Cmd::Sfx(s) => {
                if self.sfx_volume <= 0.0 {
                    return;
                }
                let volume = self.sfx_volume;
                let Some(buf) = self
                    .sounds
                    .iter()
                    .find(|(x, _)| *x == s)
                    .map(|(_, b)| b.clone())
                else {
                    return;
                };
                if let Some(d) = self.device() {
                    d.mixer().add(buf.amplify(volume));
                }
            }
            Cmd::SfxVolume(v) => self.sfx_volume = v,
            Cmd::MusicVolume(v) => self.music_volume = v,
            Cmd::MusicActive(a) => self.music_active = a,
            Cmd::MusicFolder(dir) => {
                if let Some(p) = self.music.take() {
                    p.stop();
                }
                self.music_gain = 0.0;
                self.failures = 0;
                self.pos = 0;
                self.loaded = false;
                self.user_paused = false;
                self.tracks = dir.as_deref().map(music_files).unwrap_or_default();
                self.shuffle();
                tracing::info!(tracks = self.tracks.len(), "music folder set");
            }
            Cmd::NextTrack => {
                self.step_track(1);
                self.reload();
            }
            Cmd::PreviousTrack => {
                let played = self.music.as_ref().map_or(Duration::ZERO, Player::get_pos);
                if !(self.loaded && played >= RESTART_AFTER) {
                    self.step_track(-1);
                }
                self.reload();
            }
            Cmd::TogglePause => {
                self.user_paused = !self.user_paused;
                self.paused_at = self
                    .music
                    .as_ref()
                    .filter(|_| self.user_paused && self.loaded)
                    .map(Player::get_pos);
            }
        }
        self.publish();
    }

    fn shuffle(&mut self) {
        for i in (1..self.tracks.len()).rev() {
            // xorshift64
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            let j = (self.rng % (i as u64 + 1)) as usize;
            self.tracks.swap(i, j);
        }
    }

    fn music_step(&mut self) {
        self.fade_and_queue();
        self.publish();
    }

    /// Fades the music toward its volume, and opens the next song when one
    /// ends.
    fn fade_and_queue(&mut self) {
        let target = if self.music_active && !self.user_paused && !self.tracks.is_empty() {
            self.music_volume
        } else {
            0.0
        };
        // Nothing playing and nothing to play: leave the device alone.
        if self.music.is_none() && target <= 0.0 {
            return;
        }
        if self.music.is_none() {
            let Some(d) = self.device() else { return };
            let p = Player::connect_new(d.mixer());
            p.set_volume(0.0);
            self.music = Some(p);
        }
        // A pause from the button is quick, so the song stops about where
        // its bar did.
        let rate = if self.user_paused { FADE_PAUSE } else { FADE };
        self.music_gain += (target - self.music_gain) * rate;
        if (target - self.music_gain).abs() < 0.002 {
            self.music_gain = target;
        }
        let gain = self.music_gain;
        let Some(player) = &self.music else { return };
        player.set_volume(gain);
        if gain <= 0.0 && target <= 0.0 {
            if !player.is_paused() {
                player.pause();
            }
            return;
        }
        if player.is_paused() {
            player.play();
        }
        // The song ended: on to the next, which goes in at once.
        if player.empty() && self.failures < self.tracks.len() {
            if self.loaded {
                self.step_track(1);
            }
            let path = self.tracks[self.pos].clone();
            match std::fs::File::open(&path)
                .map_err(|e| e.to_string())
                .and_then(|f| Decoder::try_from(f).map_err(|e| e.to_string()))
            {
                Ok(src) => {
                    self.failures = 0;
                    self.loaded = true;
                    self.length = src.total_duration();
                    tracing::debug!(track = %path.display(), "music");
                    // A short fade in hides a hard start.
                    if let Some(player) = &self.music {
                        player.append(src.fade_in(Duration::from_millis(400)));
                    }
                }
                Err(e) => {
                    self.failures += 1;
                    self.step_track(1);
                    tracing::warn!(track = %path.display(), error = %e, "music file not played");
                }
            }
        }
    }
}

/// The interface sounds, as soft bell-like tones.
mod synth {
    use super::Sfx;
    use rodio::buffer::SamplesBuffer;
    use std::num::NonZero;

    const RATE: u32 = 48_000;

    /// Adds one tone to `out`, starting `at` seconds in: a pitch gliding
    /// from `f0` to `f1` Hz over `dur` seconds, at peak `amp`, dying away
    /// at `decay` per second.
    fn tone(out: &mut Vec<f32>, at: f32, f0: f32, f1: f32, dur: f32, amp: f32, decay: f32) {
        let rate = RATE as f32;
        let start = (at * rate) as usize;
        let n = (dur * rate) as usize;
        if out.len() < start + n {
            out.resize(start + n, 0.0);
        }
        let attack = 0.004 * rate;
        let release = (0.012 * rate).min(n as f32);
        let mut phase = 0.0f32;
        for i in 0..n {
            let t = i as f32 / rate;
            let f = f0 + (f1 - f0) * (i as f32 / n as f32);
            phase += std::f32::consts::TAU * f / rate;
            // A little of the second and third harmonics: a soft bell.
            let wave = phase.sin() + 0.22 * (2.0 * phase).sin() + 0.06 * (3.0 * phase).sin();
            let mut env = (-decay * t).exp();
            if (i as f32) < attack {
                env *= i as f32 / attack;
            }
            let left = (n - i) as f32;
            if left < release {
                env *= left / release;
            }
            out[start + i] += wave * env * amp;
        }
    }

    pub fn samples(s: Sfx) -> Vec<f32> {
        let mut o = Vec::new();
        match s {
            Sfx::Move => tone(&mut o, 0.0, 1250.0, 1180.0, 0.05, 0.30, 70.0),
            Sfx::Filter => tone(&mut o, 0.0, 830.0, 760.0, 0.07, 0.32, 48.0),
            Sfx::Key => tone(&mut o, 0.0, 1900.0, 1800.0, 0.03, 0.18, 120.0),
            Sfx::Confirm => {
                tone(&mut o, 0.0, 880.0, 880.0, 0.10, 0.30, 30.0);
                tone(&mut o, 0.06, 1320.0, 1320.0, 0.16, 0.30, 24.0);
            }
            Sfx::Back => {
                tone(&mut o, 0.0, 1175.0, 1175.0, 0.09, 0.26, 34.0);
                tone(&mut o, 0.055, 784.0, 784.0, 0.14, 0.26, 26.0);
            }
            Sfx::Toggle => tone(&mut o, 0.0, 990.0, 1120.0, 0.06, 0.30, 55.0),
            Sfx::Open => {
                tone(&mut o, 0.0, 520.0, 780.0, 0.18, 0.26, 14.0);
                tone(&mut o, 0.03, 1040.0, 1560.0, 0.15, 0.08, 20.0);
            }
            Sfx::Close => {
                tone(&mut o, 0.0, 780.0, 520.0, 0.16, 0.24, 16.0);
                tone(&mut o, 0.0, 1560.0, 1040.0, 0.12, 0.06, 22.0);
            }
            Sfx::Launch => {
                // A rising C major arpeggio over a low C.
                tone(&mut o, 0.0, 261.6, 261.6, 1.1, 0.16, 3.0);
                for (k, f) in [523.3, 659.3, 784.0, 1046.5].into_iter().enumerate() {
                    tone(&mut o, 0.07 * k as f32, f, f, 0.95, 0.20, 4.5);
                }
            }
            Sfx::Error => {
                tone(&mut o, 0.0, 233.1, 233.1, 0.08, 0.32, 24.0);
                tone(&mut o, 0.10, 196.0, 196.0, 0.12, 0.32, 24.0);
            }
        }
        // Never louder than half scale, whatever the tones add up to.
        let peak = o.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        if peak > 0.5 {
            o.iter_mut().for_each(|x| *x *= 0.5 / peak);
        }
        o
    }

    pub fn render(s: Sfx) -> SamplesBuffer {
        SamplesBuffer::new(
            NonZero::new(1).unwrap(),
            NonZero::new(RATE).unwrap(),
            samples(s)
                .into_iter()
                .map(|x| x as rodio::Sample)
                .collect::<Vec<_>>(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_is_short_and_not_too_loud() {
        for s in Sfx::ALL {
            let o = synth::samples(s);
            assert!(!o.is_empty(), "{s:?} is silent");
            assert!(o.len() < 48_000 * 2, "{s:?} is too long");
            assert!(o.iter().all(|x| x.abs() <= 0.5 + 1e-4), "{s:?} clips");
            // Fades out: no click at the end.
            assert!(o.last().unwrap().abs() < 0.01, "{s:?} ends abruptly");
        }
    }

    #[test]
    fn song_titles_drop_track_numbers() {
        let t = |s: &str| track_title(Path::new(s));
        assert_eq!(t("music/03 - Artist - Song.mp3"), "Artist - Song");
        assert_eq!(t("07. Intro.flac"), "Intro");
        assert_eq!(t("1979.mp3"), "1979");
        assert_eq!(t("Song.ogg"), "Song");
    }

    #[test]
    fn previous_and_next_wrap_around() {
        let mut e = Engine::new(Arc::default());
        e.tracks = vec!["a.mp3".into(), "b.mp3".into(), "c.mp3".into()];
        e.step_track(-1);
        assert_eq!(e.pos, 2);
        e.step_track(1);
        assert_eq!(e.pos, 0, "past the last song: back to the first");
        e.publish();
        assert!(e.now.lock().unwrap().is_some());
        e.command(Cmd::TogglePause);
        assert!(e.now.lock().unwrap().as_ref().unwrap().paused);
    }

    #[test]
    fn song_progress_and_clock() {
        let mut n = NowPlaying {
            title: "x".into(),
            paused: false,
            position: Duration::from_secs(30),
            length: Some(Duration::from_secs(120)),
        };
        assert_eq!(n.progress(), Some(0.25));
        n.length = None;
        assert_eq!(n.progress(), None);
        assert_eq!(clock(Duration::from_secs(65)), "1:05");
        assert_eq!(clock(Duration::from_secs(3725)), "1:02:05");
    }

    #[test]
    fn volume_curve() {
        assert_eq!(gain(0), 0.0);
        assert_eq!(gain(100), 1.0);
        assert!((gain(50) - 0.25).abs() < 1e-6);
        assert_eq!(gain(200), 1.0);
    }

    #[test]
    fn finds_music_in_subfolders() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("album")).unwrap();
        for f in ["a.mp3", "album/b.FLAC", "notes.txt", "album/cover.jpg"] {
            std::fs::write(dir.path().join(f), b"").unwrap();
        }
        let mut found: Vec<String> = music_files(dir.path())
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        found.sort();
        assert_eq!(found, ["a.mp3", "b.FLAC"]);
    }
}

#[cfg(test)]
mod device_tests {
    use super::*;

    /// Plays every sound on the default device. Needs speakers:
    /// `cargo test -p moonglass-launcher plays_every_sound -- --ignored`.
    #[test]
    #[ignore]
    fn plays_every_sound() {
        rodio::DeviceSinkBuilder::open_default_sink().expect("a sound device");
        let a = Audio::start();
        a.set_sfx_volume(70);
        for s in Sfx::ALL {
            a.play(s);
            std::thread::sleep(Duration::from_millis(if s == Sfx::Launch {
                1300
            } else {
                450
            }));
        }
    }
}
