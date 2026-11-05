//! Voice dictation: hold a global shortcut (or tap it to start and again to
//! stop), speak, and the words land in the addon's composer to review and
//! send. Addons cannot use the microphone, so the tray app listens and
//! recognizes speech with the system recognizer (Speech framework on macOS,
//! Windows.Media.SpeechRecognition on Windows). The result reaches the game
//! like any other data: the bridge publishes it in the snapshot (`dictation`)
//! and the addon inserts it into the composer once.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

#[cfg(target_os = "macos")]
mod mac;
#[cfg(any(target_os = "macos", windows))]
mod recorder;
#[cfg(windows)]
mod win;

#[cfg(any(target_os = "macos", windows))]
pub use recorder::Recorder;

/// Holding the shortcut at least this long is push-to-talk: releasing it
/// stops. A shorter tap starts listening until the next press.
pub const HOLD: Duration = Duration::from_millis(400);
/// Recognizers stop on their own after about a minute.
pub const MAX_LISTEN: Duration = Duration::from_secs(55);
/// How long to wait for the final words after the microphone stops.
pub const FINISH_TIMEOUT: Duration = Duration::from_secs(5);

/// One dictation, as the addon sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Take {
    /// Unique across bridge restarts, so the addon inserts each one once.
    pub id: i64,
    pub state: TakeState,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TakeState {
    Listening,
    Done(String),
    Failed(String),
}

static TAKE: Mutex<Option<Take>> = Mutex::new(None);
/// The shortcut while it is registered, for the addon's hint.
static SHORTCUT: Mutex<Option<String>> = Mutex::new(None);
/// Bumped on every change above, so the bridge loop republishes right away.
static VERSION: AtomicU64 = AtomicU64::new(0);

pub fn set_take(take: Take) {
    *TAKE.lock().unwrap() = Some(take);
    VERSION.fetch_add(1, Ordering::Relaxed);
}

pub fn set_shortcut(label: Option<String>) {
    let mut shortcut = SHORTCUT.lock().unwrap();
    if *shortcut != label {
        *shortcut = label;
        VERSION.fetch_add(1, Ordering::Relaxed);
    }
}

pub fn version() -> u64 {
    VERSION.load(Ordering::Relaxed)
}

/// Add the current dictation and the shortcut to a snapshot.
pub fn add_to_snapshot(snapshot: &mut Value) {
    let take = TAKE.lock().unwrap().clone();
    let shortcut = SHORTCUT.lock().unwrap().clone();
    if let Some(bridge) = snapshot.get_mut("bridge").and_then(Value::as_object_mut)
        && let Some(shortcut) = shortcut
    {
        bridge.insert("dictation".into(), json!({ "shortcut": shortcut }));
    }
    if let (Some(take), Some(map)) = (take, snapshot.as_object_mut()) {
        map.insert("dictation".into(), take_json(&take));
    }
}

fn take_json(take: &Take) -> Value {
    match &take.state {
        TakeState::Listening => json!({ "id": take.id, "state": "listening" }),
        TakeState::Done(text) => json!({ "id": take.id, "state": "done", "text": text }),
        TakeState::Failed(message) => json!({ "id": take.id, "state": "error", "message": message }),
    }
}

/// Whether the microphone and speech recognition may be used, for Settings.
#[derive(Debug, Clone, PartialEq)]
pub enum Access {
    Unknown,
    Asking,
    Granted,
    Denied(String),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Status {
    pub access: Access,
    /// The shortcut could not be registered (another app has it, or it is not a valid key).
    pub shortcut_error: Option<String>,
    /// The last dictation failed.
    pub last_error: Option<String>,
}

impl Status {
    /// Why dictation cannot listen when access is not granted.
    pub fn access_problem(&self) -> String {
        match &self.access {
            Access::Denied(message) => message.clone(),
            _ => "T3 WoW may not use the microphone or speech recognition yet.".into(),
        }
    }
}

static STATUS: Mutex<Status> = Mutex::new(Status { access: Access::Unknown, shortcut_error: None, last_error: None });

pub fn status() -> Status {
    STATUS.lock().unwrap().clone()
}

pub fn update_status(change: impl FnOnce(&mut Status)) {
    change(&mut STATUS.lock().unwrap());
}

/// What a press or release of the shortcut does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    Start,
    Stop,
}

/// Turns shortcut presses and releases into start and stop: hold to talk, or
/// tap to start and tap again to stop.
#[derive(Debug, Default)]
pub struct Trigger {
    listening: bool,
    /// When the key went down, while it is down.
    down_since: Option<Instant>,
    /// This press stopped a tapped dictation; its release does nothing.
    stopping: bool,
}

impl Trigger {
    pub fn press(&mut self, now: Instant) -> Option<Gesture> {
        if self.down_since.is_some() {
            // Key repeat.
            return None;
        }
        self.down_since = Some(now);
        if self.listening {
            self.listening = false;
            self.stopping = true;
            Some(Gesture::Stop)
        } else {
            self.listening = true;
            Some(Gesture::Start)
        }
    }

    pub fn release(&mut self, now: Instant) -> Option<Gesture> {
        let since = self.down_since.take()?;
        if std::mem::take(&mut self.stopping) || !self.listening {
            return None;
        }
        if now.duration_since(since) >= HOLD {
            self.listening = false;
            return Some(Gesture::Stop);
        }
        None
    }

    /// Listening stopped on its own (time limit, error).
    pub fn reset(&mut self) {
        self.listening = false;
    }

    /// Stop listening because the shortcut goes away; true if it was listening.
    pub fn cancel(&mut self) -> bool {
        self.down_since = None;
        self.stopping = false;
        std::mem::take(&mut self.listening)
    }
}

/// A readable form of a shortcut: `control+shift+Space` -> `Ctrl+Shift+Space`
/// (`Ctrl+Option+Space` style names on macOS). None when it is not valid.
pub fn shortcut_label(shortcut: &str) -> Option<String> {
    let mut modifiers = Vec::new();
    let mut key = None;
    for token in shortcut.split('+').map(str::trim) {
        if key.is_some() || token.is_empty() {
            return None;
        }
        let modifier = match token.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => Some(0),
            "alt" | "option" => Some(1),
            "shift" => Some(2),
            "cmd" | "command" | "super" | "meta" | "win" => Some(3),
            _ => None,
        };
        match modifier {
            Some(m) if !modifiers.contains(&m) => modifiers.push(m),
            Some(_) => return None,
            None => key = Some(key_label(token)?),
        }
    }
    let key = key?;
    modifiers.sort_unstable();
    let names: &[&str] =
        if cfg!(target_os = "macos") { &["Ctrl", "Option", "Shift", "Cmd"] } else { &["Ctrl", "Alt", "Shift", "Win"] };
    let mut parts: Vec<String> = modifiers.iter().map(|&m| names[m].to_string()).collect();
    parts.push(key);
    Some(parts.join("+"))
}

/// `KeyD` -> `D`, `Digit5` -> `5`, `F5`, `Space`, `Backquote` and other key
/// codes as they are. None for keys a shortcut cannot use on its own.
fn key_label(token: &str) -> Option<String> {
    if let Some(letter) =
        token.strip_prefix("Key").filter(|l| l.len() == 1 && l.chars().all(|c| c.is_ascii_alphabetic()))
    {
        return Some(letter.to_ascii_uppercase());
    }
    if let Some(digit) = token.strip_prefix("Digit").filter(|d| d.len() == 1 && d.chars().all(|c| c.is_ascii_digit())) {
        return Some(digit.to_string());
    }
    if token.len() == 1 && token.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Some(token.to_ascii_uppercase());
    }
    let function = token.strip_prefix(['F', 'f']).and_then(|n| n.parse::<u8>().ok()).filter(|n| (1..=24).contains(n));
    if let Some(n) = function {
        return Some(format!("F{n}"));
    }
    const NAMED: &[&str] = &[
        "Space",
        "Backquote",
        "Backslash",
        "BracketLeft",
        "BracketRight",
        "Comma",
        "Equal",
        "Minus",
        "Period",
        "Quote",
        "Semicolon",
        "Slash",
        "Enter",
        "Tab",
        "Backspace",
        "Delete",
        "Insert",
        "Home",
        "End",
        "PageUp",
        "PageDown",
        "ArrowUp",
        "ArrowDown",
        "ArrowLeft",
        "ArrowRight",
        "Pause",
        "ScrollLock",
        "Numpad0",
        "Numpad1",
        "Numpad2",
        "Numpad3",
        "Numpad4",
        "Numpad5",
        "Numpad6",
        "Numpad7",
        "Numpad8",
        "Numpad9",
        "NumpadAdd",
        "NumpadSubtract",
        "NumpadMultiply",
        "NumpadDivide",
        "NumpadDecimal",
        "NumpadEnter",
    ];
    NAMED.iter().find(|name| name.eq_ignore_ascii_case(token)).map(|name| name.to_string())
}

/// The shortcut in the form `global-hotkey` parses (`control+shift+Space`).
pub fn shortcut_spec(label: &str) -> Option<String> {
    let label = shortcut_label(label)?;
    let parts: Vec<String> = label
        .split('+')
        .map(|part| match part {
            "Ctrl" => "control".to_string(),
            "Alt" | "Option" => "alt".to_string(),
            "Shift" => "shift".to_string(),
            "Cmd" | "Win" => "super".to_string(),
            key if key.len() == 1 && key.chars().all(|c| c.is_ascii_alphabetic()) => format!("Key{key}"),
            key if key.len() == 1 && key.chars().all(|c| c.is_ascii_digit()) => format!("Digit{key}"),
            key => key.to_string(),
        })
        .collect();
    Some(parts.join("+"))
}

/// A short, readable reason for a recognizer error.
pub fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or_default().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hold_to_talk_stops_on_release() {
        let mut trigger = Trigger::default();
        let t0 = Instant::now();
        assert_eq!(trigger.press(t0), Some(Gesture::Start));
        // Key repeat while held.
        assert_eq!(trigger.press(t0 + Duration::from_millis(300)), None);
        assert_eq!(trigger.release(t0 + Duration::from_secs(2)), Some(Gesture::Stop));
        assert_eq!(trigger.press(t0 + Duration::from_secs(3)), Some(Gesture::Start));
    }

    #[test]
    fn a_tap_listens_until_the_next_press() {
        let mut trigger = Trigger::default();
        let t0 = Instant::now();
        assert_eq!(trigger.press(t0), Some(Gesture::Start));
        assert_eq!(trigger.release(t0 + Duration::from_millis(150)), None);
        assert_eq!(trigger.press(t0 + Duration::from_secs(4)), Some(Gesture::Stop));
        // Holding that second press does not start again.
        assert_eq!(trigger.release(t0 + Duration::from_secs(6)), None);
        assert_eq!(trigger.press(t0 + Duration::from_secs(7)), Some(Gesture::Start));
    }

    #[test]
    fn starts_again_after_stopping_on_its_own() {
        let mut trigger = Trigger::default();
        let t0 = Instant::now();
        trigger.press(t0);
        trigger.release(t0 + Duration::from_millis(100));
        trigger.reset();
        assert_eq!(trigger.press(t0 + Duration::from_secs(60)), Some(Gesture::Start));
    }

    #[test]
    fn names_shortcuts() {
        assert_eq!(shortcut_label("control+shift+Space").as_deref(), Some("Ctrl+Shift+Space"));
        assert_eq!(shortcut_label("Shift+ctrl+KeyD").as_deref(), Some("Ctrl+Shift+D"));
        assert_eq!(shortcut_label("F8").as_deref(), Some("F8"));
        assert_eq!(shortcut_label("ctrl+Digit5").as_deref(), Some("Ctrl+5"));
        assert_eq!(shortcut_label("ctrl+ctrl+Space"), None);
        assert_eq!(shortcut_label("ctrl+"), None);
        assert_eq!(shortcut_label("ctrl+shift"), None);
        assert_eq!(shortcut_label("Space+ctrl"), None);
        assert_eq!(shortcut_label("ctrl+F25"), None);
        assert_eq!(shortcut_spec("Ctrl+Shift+D").as_deref(), Some("control+shift+KeyD"));
        assert_eq!(shortcut_spec("Alt+Backquote").as_deref(), Some("alt+Backquote"));
    }

    #[cfg(any(target_os = "macos", windows))]
    #[test]
    fn every_shortcut_spec_parses() {
        use std::str::FromStr;
        for shortcut in ["Ctrl+Shift+Space", "F8", "Alt+Backquote", "Ctrl+Shift+D", "Cmd+5", "Shift+Numpad0"] {
            let spec = shortcut_spec(shortcut).unwrap();
            assert!(global_hotkey::hotkey::HotKey::from_str(&spec).is_ok(), "{spec}");
        }
    }

    #[test]
    fn puts_the_take_into_the_snapshot() {
        let mut snapshot = json!({ "bridge": { "status": "ok" } });
        set_shortcut(Some("Ctrl+Shift+Space".into()));
        set_take(Take { id: 7, state: TakeState::Done("hello there".into()) });
        add_to_snapshot(&mut snapshot);
        assert_eq!(snapshot["bridge"]["dictation"]["shortcut"], "Ctrl+Shift+Space");
        assert_eq!(snapshot["dictation"], json!({ "id": 7, "state": "done", "text": "hello there" }));
        let failed = take_json(&Take { id: 8, state: TakeState::Failed("No microphone".into()) });
        assert_eq!(failed["state"], "error");
    }
}
