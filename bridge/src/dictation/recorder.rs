//! The thread that owns the microphone and the recognizer. The tray's event
//! loop sends it start and stop; it publishes each dictation (see `set_take`).

use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use super::{Access, FINISH_TIMEOUT, MAX_LISTEN, Take, TakeState, set_take, update_status};
use crate::log;
use crate::oauth::now_ms;

#[cfg(target_os = "macos")]
use super::mac as platform;
#[cfg(windows)]
use super::win as platform;

/// The microphone and the recognizer of one platform.
pub(super) trait Engine {
    /// Ask for microphone and speech recognition access (prompting the first
    /// time); blocks until answered.
    fn request_access(&mut self) -> Access;
    /// Start listening.
    fn start(&mut self, language: Option<&str>) -> Result<(), String>;
    /// Stop listening and return what was said, waiting up to `timeout` for
    /// the final words.
    fn finish(&mut self, timeout: Duration) -> Result<String, String>;
}

enum Command {
    Prepare,
    Start(Option<String>),
    Stop,
}

/// Sends commands to the recorder thread.
pub struct Recorder {
    commands: Sender<Command>,
}

impl Recorder {
    /// `stopped` runs when listening ends without the shortcut (time limit, error).
    pub fn spawn(stopped: impl Fn() + Send + 'static) -> Self {
        let (commands, receiver) = mpsc::channel();
        std::thread::spawn(move || run(receiver, &stopped));
        Recorder { commands }
    }

    /// Ask for access now (Settings turned dictation on), so the first
    /// dictation does not wait on permission prompts.
    pub fn prepare(&self) {
        let _ = self.commands.send(Command::Prepare);
    }

    pub fn start(&self, language: Option<String>) {
        let _ = self.commands.send(Command::Start(language));
    }

    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }
}

fn run(commands: Receiver<Command>, stopped: &dyn Fn()) {
    // Without an engine every dictation fails with the reason.
    let mut engine = platform::Engine::new().inspect_err(|error| {
        log!("dictation is not available: {error}");
        update_status(|status| status.access = Access::Denied(error.clone()));
    });
    // The dictation in progress and when it must stop at the latest.
    let mut current: Option<(i64, Instant)> = None;
    loop {
        let command = match current {
            Some((_, deadline)) => match commands.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                Ok(command) => command,
                Err(RecvTimeoutError::Timeout) => {
                    log!("dictation reached its time limit");
                    stopped();
                    Command::Stop
                }
                Err(RecvTimeoutError::Disconnected) => return,
            },
            None => match commands.recv() {
                Ok(command) => command,
                Err(_) => return,
            },
        };
        match command {
            Command::Prepare => {
                if let Ok(engine) = &mut engine {
                    ask(engine);
                }
            }
            Command::Start(language) => {
                if current.is_some() {
                    continue;
                }
                let id = now_ms();
                let engine = match &mut engine {
                    Ok(engine) => engine,
                    Err(error) => {
                        fail(id, error, stopped);
                        continue;
                    }
                };
                if ask(engine) != Access::Granted {
                    fail(id, &super::status().access_problem(), stopped);
                    continue;
                }
                platform::sound(platform::Sound::Start);
                match engine.start(language.as_deref()) {
                    Ok(()) => {
                        log!("dictation started");
                        set_take(Take { id, state: TakeState::Listening });
                        current = Some((id, Instant::now() + MAX_LISTEN));
                    }
                    Err(error) => fail(id, &error, stopped),
                }
            }
            Command::Stop => {
                let (Some((id, _)), Ok(engine)) = (current.take(), &mut engine) else { continue };
                let result = engine.finish(FINISH_TIMEOUT);
                match result.map(|text| text.trim().to_string()) {
                    Ok(text) if !text.is_empty() => {
                        platform::sound(platform::Sound::Stop);
                        log!("dictation done ({} characters)", text.chars().count());
                        update_status(|status| status.last_error = None);
                        set_take(Take { id, state: TakeState::Done(text) });
                    }
                    Ok(_) => fail(id, "Didn't catch that. Speak while you hold the shortcut, then try again.", &|| {}),
                    Err(error) => fail(id, &error, &|| {}),
                }
            }
        }
    }
}

/// The current access, asking first if it was never asked.
fn ask(engine: &mut platform::Engine) -> Access {
    let known = super::status().access;
    if known == Access::Granted {
        return known;
    }
    update_status(|status| status.access = Access::Asking);
    let access = engine.request_access();
    log!("dictation access: {access:?}");
    update_status(|status| status.access = access.clone());
    access
}

fn fail(id: i64, message: &str, stopped: &dyn Fn()) {
    platform::sound(platform::Sound::Error);
    log!("dictation failed: {message}");
    update_status(|status| status.last_error = Some(message.to_string()));
    set_take(Take { id, state: TakeState::Failed(message.to_string()) });
    stopped();
}
