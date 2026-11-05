//! macOS: the microphone through AVAudioEngine, recognized by the Speech
//! framework (on the device when the language supports it).
//!
//! Both need the usage descriptions in `T3 WoW.app`'s Info.plist (see
//! `app.rs`); without them macOS kills the process on the first request, so
//! dictation only runs from the app bundle.

use std::ptr::NonNull;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool, NSObjectProtocol};
use objc2::{AnyThread, msg_send, sel};
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_avf_audio::{AVAudioEngine, AVAudioPCMBuffer, AVAudioTime};
use objc2_foundation::{NSBundle, NSError, NSLocale, NSOperationQueue, NSString};
use objc2_speech::{
    SFSpeechAudioBufferRecognitionRequest, SFSpeechRecognitionResult, SFSpeechRecognitionTask, SFSpeechRecognizer,
    SFSpeechRecognizerAuthorizationStatus,
};

use super::{Access, first_line};

const PRIVACY: &str = "System Settings > Privacy & Security";

#[derive(Default)]
struct Transcript {
    text: String,
    finished: bool,
    error: Option<String>,
}

type Shared = Arc<(Mutex<Transcript>, Condvar)>;

struct Session {
    audio: Retained<AVAudioEngine>,
    request: Retained<SFSpeechAudioBufferRecognitionRequest>,
    task: Retained<SFSpeechRecognitionTask>,
    transcript: Shared,
    // Kept alive while the session runs.
    _tap: RcBlock<dyn Fn(NonNull<AVAudioPCMBuffer>, NonNull<AVAudioTime>)>,
    _handler: RcBlock<dyn Fn(*mut SFSpeechRecognitionResult, *mut NSError)>,
}

pub struct Engine {
    session: Option<Session>,
    /// Results arrive here instead of on the main thread, which runs the tray.
    queue: Retained<NSOperationQueue>,
}

impl Engine {
    pub fn new() -> Result<Self, String> {
        let bundle = NSBundle::mainBundle();
        for key in ["NSMicrophoneUsageDescription", "NSSpeechRecognitionUsageDescription"] {
            if bundle.objectForInfoDictionaryKey(&NSString::from_str(key)).is_none() {
                return Err("Dictation works in the T3 WoW app only (`t3wow app install`, then open it).".into());
            }
        }
        Ok(Engine { session: None, queue: NSOperationQueue::new() })
    }
}

/// Wait for a callback-style answer.
fn wait_for<T: Send + 'static>(ask: impl FnOnce(Box<dyn Fn(T) + Send>)) -> Option<T> {
    let answer: Arc<(Mutex<Option<T>>, Condvar)> = Arc::default();
    let reply = answer.clone();
    ask(Box::new(move |value| {
        *reply.0.lock().unwrap() = Some(value);
        reply.1.notify_all();
    }));
    let (lock, ready) = &*answer;
    // Long enough for the person to read the prompt.
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut value = lock.lock().unwrap();
    while value.is_none() && Instant::now() < deadline {
        value = ready.wait_timeout(value, Duration::from_millis(500)).unwrap().0;
    }
    value.take()
}

fn speech_access() -> Access {
    let mut status = unsafe { SFSpeechRecognizer::authorizationStatus() };
    if status == SFSpeechRecognizerAuthorizationStatus::NotDetermined {
        status = wait_for(|reply| {
            let block = RcBlock::new(move |status: SFSpeechRecognizerAuthorizationStatus| reply(status));
            unsafe { SFSpeechRecognizer::requestAuthorization(&block) };
        })
        .unwrap_or(SFSpeechRecognizerAuthorizationStatus::NotDetermined);
    }
    match status {
        SFSpeechRecognizerAuthorizationStatus::Authorized => Access::Granted,
        SFSpeechRecognizerAuthorizationStatus::NotDetermined => Access::Unknown,
        SFSpeechRecognizerAuthorizationStatus::Restricted => {
            Access::Denied("Speech recognition is restricted on this Mac.".into())
        }
        _ => Access::Denied(format!("Allow T3 WoW under {PRIVACY} > Speech Recognition, then try again.")),
    }
}

fn microphone_access() -> Access {
    let Some(audio) = (unsafe { AVMediaTypeAudio }) else {
        return Access::Denied("This Mac has no audio capture support.".into());
    };
    let mut status = unsafe { AVCaptureDevice::authorizationStatusForMediaType(audio) };
    if status == AVAuthorizationStatus::NotDetermined {
        let granted = wait_for(|reply| {
            let block = RcBlock::new(move |granted: Bool| reply(granted.as_bool()));
            unsafe { AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &block) };
        });
        status = match granted {
            Some(true) => AVAuthorizationStatus::Authorized,
            Some(false) => AVAuthorizationStatus::Denied,
            None => AVAuthorizationStatus::NotDetermined,
        };
    }
    match status {
        AVAuthorizationStatus::Authorized => Access::Granted,
        AVAuthorizationStatus::NotDetermined => Access::Unknown,
        AVAuthorizationStatus::Restricted => Access::Denied("Microphone access is restricted on this Mac.".into()),
        _ => Access::Denied(format!("Allow T3 WoW under {PRIVACY} > Microphone, then try again.")),
    }
}

fn describe(error: &NSError) -> String {
    first_line(&error.localizedDescription().to_string())
}

impl super::recorder::Engine for Engine {
    fn request_access(&mut self) -> Access {
        match microphone_access() {
            Access::Granted => speech_access(),
            other => other,
        }
    }

    fn start(&mut self, language: Option<&str>) -> Result<(), String> {
        if let Some(session) = self.session.take() {
            stop(&session);
            unsafe { session.task.cancel() };
        }
        let recognizer = unsafe {
            match language {
                Some(tag) => {
                    let locale = NSLocale::initWithLocaleIdentifier(NSLocale::alloc(), &NSString::from_str(tag));
                    SFSpeechRecognizer::initWithLocale(SFSpeechRecognizer::alloc(), &locale)
                }
                None => SFSpeechRecognizer::init(SFSpeechRecognizer::alloc()),
            }
        }
        .ok_or_else(|| match language {
            Some(tag) => format!("Speech recognition does not support the language {tag}."),
            None => "Speech recognition does not support the system language: pick one in Settings.".into(),
        })?;
        unsafe {
            if !recognizer.isAvailable() {
                return Err("Speech recognition is not available right now. Check the internet connection.".into());
            }
            recognizer.setQueue(&self.queue);

            let request = SFSpeechAudioBufferRecognitionRequest::new();
            request.setShouldReportPartialResults(true);
            // Private and works offline, where the language allows it.
            if recognizer.supportsOnDeviceRecognition() {
                request.setRequiresOnDeviceRecognition(true);
            }
            // macOS 13 and later.
            if request.respondsToSelector(sel!(setAddsPunctuation:)) {
                request.setAddsPunctuation(true);
            }

            let audio = AVAudioEngine::new();
            let input = audio.inputNode();
            let format = input.outputFormatForBus(0);
            if format.sampleRate() <= 0.0 || format.channelCount() == 0 {
                return Err("No microphone found.".into());
            }
            let feed = request.clone();
            let tap = RcBlock::new(move |buffer: NonNull<AVAudioPCMBuffer>, _when: NonNull<AVAudioTime>| {
                feed.appendAudioPCMBuffer(buffer.as_ref());
            });
            let tap_ptr: *const _ = &*tap;
            input.installTapOnBus_bufferSize_format_block(0, 1024, Some(&format), tap_ptr as *mut _);
            audio.prepare();
            if let Err(error) = audio.startAndReturnError() {
                input.removeTapOnBus(0);
                return Err(format!("Could not start the microphone: {}", describe(&error)));
            }

            let transcript: Shared = Arc::default();
            let results = transcript.clone();
            let handler = RcBlock::new(move |result: *mut SFSpeechRecognitionResult, error: *mut NSError| {
                let (lock, changed) = &*results;
                let mut transcript = lock.lock().unwrap();
                if let Some(result) = result.as_ref() {
                    transcript.text = result.bestTranscription().formattedString().to_string();
                    transcript.finished |= result.isFinal();
                }
                if let Some(error) = error.as_ref() {
                    transcript.error = Some(describe(error));
                    transcript.finished = true;
                }
                changed.notify_all();
            });
            let task = recognizer.recognitionTaskWithRequest_resultHandler(&request, &handler);
            self.session = Some(Session { audio, request, task, transcript, _tap: tap, _handler: handler });
        }
        Ok(())
    }

    fn finish(&mut self, timeout: Duration) -> Result<String, String> {
        let Some(session) = self.session.take() else { return Ok(String::new()) };
        stop(&session);
        unsafe { session.request.endAudio() };
        let (lock, changed) = &*session.transcript;
        let deadline = Instant::now() + timeout;
        let mut transcript = lock.lock().unwrap();
        while !transcript.finished && Instant::now() < deadline {
            transcript = changed.wait_timeout(transcript, Duration::from_millis(100)).unwrap().0;
        }
        let text = std::mem::take(&mut transcript.text);
        let error = transcript.error.take();
        drop(transcript);
        unsafe { session.task.cancel() };
        // "No speech detected" and the like still count as nothing said.
        match error {
            Some(error) if text.is_empty() && !error.to_lowercase().contains("no speech") => Err(error),
            _ => Ok(text),
        }
    }
}

fn stop(session: &Session) {
    unsafe {
        session.audio.stop();
        session.audio.inputNode().removeTapOnBus(0);
    }
}

pub enum Sound {
    Start,
    Stop,
    Error,
}

/// A short system sound, so the player knows without looking that the
/// microphone is on or off.
pub fn sound(sound: Sound) {
    let name = match sound {
        Sound::Start => "Tink",
        Sound::Stop => "Pop",
        Sound::Error => "Basso",
    };
    let Some(class) = AnyClass::get(c"NSSound") else { return };
    unsafe {
        let sound: Option<Retained<AnyObject>> = msg_send![class, soundNamed: &*NSString::from_str(name)];
        if let Some(sound) = sound {
            let _: Bool = msg_send![&sound, play];
        }
    }
}
