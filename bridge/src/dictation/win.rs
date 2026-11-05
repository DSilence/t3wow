//! Windows: Windows.Media.SpeechRecognition with the dictation grammar, in
//! a continuous session from press to release. Dictation uses Microsoft's
//! online speech service, so Windows must allow online speech recognition.

use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Globalization::Language;
use windows::Media::SpeechRecognition::{
    ISpeechRecognitionConstraint, SpeechContinuousRecognitionCompletedEventArgs,
    SpeechContinuousRecognitionResultGeneratedEventArgs, SpeechContinuousRecognitionSession,
    SpeechRecognitionResultStatus, SpeechRecognitionScenario, SpeechRecognitionTopicConstraint, SpeechRecognizer,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::System::Diagnostics::Debug::MessageBeep;
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONASTERISK, MB_ICONHAND, MB_OK};
use windows::core::{HRESULT, HSTRING, Interface, Ref};

use super::{Access, first_line};

/// Online speech recognition is off (SPERR_SPEECH_PRIVACY_POLICY_NOT_ACCEPTED).
const PRIVACY_POLICY_NOT_ACCEPTED: HRESULT = HRESULT(0x8004_5509_u32 as i32);
const ACCESS_DENIED: HRESULT = HRESULT(0x8007_0005_u32 as i32);

#[derive(Default)]
struct Transcript {
    /// Finished phrases.
    phrases: Vec<String>,
    completed: Option<SpeechRecognitionResultStatus>,
}

type Shared = Arc<(Mutex<Transcript>, Condvar)>;

struct Prepared {
    language: Option<String>,
    /// Owns the session.
    _recognizer: SpeechRecognizer,
    session: SpeechContinuousRecognitionSession,
    transcript: Shared,
}

pub struct Engine {
    prepared: Option<Prepared>,
    listening: bool,
}

impl Engine {
    pub fn new() -> Result<Self, String> {
        // WinRT on this thread, free-threaded so blocking on results is fine.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok().map_err(|e| e.message())?;
        Ok(Engine { prepared: None, listening: false })
    }

    /// A recognizer with the dictation grammar for `language`, reused while
    /// the language stays the same.
    fn prepare(&mut self, language: Option<&str>) -> Result<&Prepared, String> {
        if self.prepared.as_ref().is_some_and(|p| p.language.as_deref() != language) {
            self.prepared = None;
        }
        if self.prepared.is_none() {
            self.prepared = Some(prepare(language).map_err(|e| describe(&e, language))?);
        }
        Ok(self.prepared.as_ref().unwrap())
    }
}

fn prepare(language: Option<&str>) -> windows::core::Result<Prepared> {
    let recognizer = match language {
        Some(tag) => SpeechRecognizer::Create(&Language::CreateLanguage(&HSTRING::from(tag))?)?,
        None => SpeechRecognizer::new()?,
    };
    let dictation =
        SpeechRecognitionTopicConstraint::Create(SpeechRecognitionScenario::Dictation, &"dictation".into())?;
    recognizer.Constraints()?.Append(&dictation.cast::<ISpeechRecognitionConstraint>()?)?;
    let compiled = recognizer.CompileConstraintsAsync()?.join()?;
    let status = compiled.Status()?;
    if status != SpeechRecognitionResultStatus::Success {
        return Err(windows::core::Error::new(HRESULT(-1), status_message(status)));
    }
    let session = recognizer.ContinuousRecognitionSession()?;
    // Stop on silence only at the time limit; the shortcut decides.
    session.SetAutoStopSilenceTimeout(TimeSpan { Duration: 60 * 10_000_000 })?;
    let transcript: Shared = Arc::default();
    let results = transcript.clone();
    session.ResultGenerated(&TypedEventHandler::new(
        move |_, args: Ref<SpeechContinuousRecognitionResultGeneratedEventArgs>| {
            let result = args.ok()?.Result()?;
            if result.Status()? == SpeechRecognitionResultStatus::Success {
                let text = result.Text()?.to_string();
                if !text.trim().is_empty() {
                    let (lock, changed) = &*results;
                    lock.lock().unwrap().phrases.push(text.trim().to_string());
                    changed.notify_all();
                }
            }
            Ok(())
        },
    ))?;
    let completions = transcript.clone();
    session.Completed(&TypedEventHandler::new(
        move |_, args: Ref<SpeechContinuousRecognitionCompletedEventArgs>| {
            let status = args.ok()?.Status()?;
            let (lock, changed) = &*completions;
            lock.lock().unwrap().completed = Some(status);
            changed.notify_all();
            Ok(())
        },
    ))?;
    Ok(Prepared { language: language.map(String::from), _recognizer: recognizer, session, transcript })
}

fn status_message(status: SpeechRecognitionResultStatus) -> String {
    match status {
        SpeechRecognitionResultStatus::TopicLanguageNotSupported => {
            "Dictation does not support this language. Install its speech pack under Settings > Time & language > Speech."
                .into()
        }
        SpeechRecognitionResultStatus::NetworkFailure => {
            "Dictation could not reach Microsoft's speech service. Check the internet connection.".into()
        }
        SpeechRecognitionResultStatus::MicrophoneUnavailable => "No microphone is available.".into(),
        SpeechRecognitionResultStatus::AudioQualityFailure => "The microphone sound was too poor to recognize.".into(),
        other => format!("Speech recognition failed ({}).", other.0),
    }
}

fn describe(error: &windows::core::Error, language: Option<&str>) -> String {
    match error.code() {
        PRIVACY_POLICY_NOT_ACCEPTED => {
            "Turn on Settings > Privacy & security > Speech > Online speech recognition, then try again.".into()
        }
        ACCESS_DENIED => {
            "Allow microphone access: Settings > Privacy & security > Microphone > Let desktop apps access your microphone."
                .into()
        }
        _ => match (first_line(&error.message()), language) {
            (message, Some(tag)) if message.is_empty() => format!("Speech recognition for {tag} is not available."),
            (message, _) if message.is_empty() => format!("Speech recognition failed ({}).", error.code()),
            (message, _) => message,
        },
    }
}

impl super::recorder::Engine for Engine {
    fn request_access(&mut self) -> Access {
        // Windows has no prompt for desktop apps: compiling the grammar fails
        // when the privacy settings block it, and starting fails without a microphone.
        match self.prepare(None) {
            Ok(_) => Access::Granted,
            Err(error) => Access::Denied(error),
        }
    }

    fn start(&mut self, language: Option<&str>) -> Result<(), String> {
        let prepared = self.prepare(language)?;
        {
            let mut transcript = prepared.transcript.0.lock().unwrap();
            *transcript = Transcript::default();
        }
        let started = prepared.session.StartAsync().and_then(|start| start.join());
        if let Err(error) = started {
            let message = describe(&error, language);
            // A broken recognizer is rebuilt next time.
            self.prepared = None;
            return Err(message);
        }
        self.listening = true;
        Ok(())
    }

    fn finish(&mut self, timeout: Duration) -> Result<String, String> {
        if !std::mem::take(&mut self.listening) {
            return Ok(String::new());
        }
        let Some(prepared) = &self.prepared else { return Ok(String::new()) };
        // Waits for the results of what was already said.
        let stopped = prepared.session.StopAsync().and_then(|stop| stop.join());
        let (lock, changed) = &*prepared.transcript;
        let deadline = Instant::now() + timeout;
        let mut transcript = lock.lock().unwrap();
        while transcript.completed.is_none() && Instant::now() < deadline {
            transcript = changed.wait_timeout(transcript, Duration::from_millis(100)).unwrap().0;
        }
        let text = transcript.phrases.join(" ");
        let completed = transcript.completed;
        drop(transcript);
        if let Err(error) = stopped
            && text.is_empty()
        {
            return Err(describe(&error, None));
        }
        match completed {
            Some(status)
                if text.is_empty()
                    && status != SpeechRecognitionResultStatus::Success
                    && status != SpeechRecognitionResultStatus::UserCanceled
                    && status != SpeechRecognitionResultStatus::TimeoutExceeded =>
            {
                Err(status_message(status))
            }
            _ => Ok(text),
        }
    }
}

pub enum Sound {
    Start,
    Stop,
    Error,
}

pub fn sound(sound: Sound) {
    let style = match sound {
        Sound::Start => MB_ICONASTERISK,
        Sound::Stop => MB_OK,
        Sound::Error => MB_ICONHAND,
    };
    let _ = unsafe { MessageBeep(style) };
}
