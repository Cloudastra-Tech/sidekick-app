//! Call detection: notices when a call app (Zoom, Teams, a browser, ...) starts using the
//! microphone and offers to start coaching. It never starts capture itself; the rep still
//! goes through the consent dialog.
//!
//! Windows records which apps are using the microphone for its privacy settings, under
//! `HKCU\...\CapabilityAccessManager\ConsentStore\microphone`: one subkey per app (packaged
//! apps by package family name, others under `NonPackaged` by exe path with `#` for `\`).
//! An app is using the mic right now when `LastUsedTimeStart` is set and
//! `LastUsedTimeStop` is 0.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};

const POLL_INTERVAL: Duration = Duration::from_secs(2);
/// A call app that stops using the mic this long during a coached call ends the call. Long
/// enough to ride out an app briefly reopening its mic (switching devices, mute on some apps).
const CALL_APP_GONE_GRACE: Duration = Duration::from_secs(60);

/// Exe names (lowercase) of desktop call apps and browsers, with display names.
const DESKTOP_APPS: [(&str, &str); 13] = [
    ("zoom.exe", "Zoom"),
    ("ms-teams.exe", "Microsoft Teams"),
    ("teams.exe", "Microsoft Teams"),
    ("chrome.exe", "Google Chrome"),
    ("msedge.exe", "Microsoft Edge"),
    ("firefox.exe", "Firefox"),
    ("brave.exe", "Brave"),
    ("opera.exe", "Opera"),
    ("slack.exe", "Slack"),
    ("webex.exe", "Webex"),
    ("ciscocollabhost.exe", "Webex"),
    ("skype.exe", "Skype"),
    ("discord.exe", "Discord"),
];

/// Package family name prefixes of Store call apps, with display names.
const PACKAGED_APPS: [(&str, &str); 4] = [
    ("MSTeams_", "Microsoft Teams"),
    ("MicrosoftTeams_", "Microsoft Teams"),
    ("5319275A.WhatsAppDesktop_", "WhatsApp"),
    ("Microsoft.SkypeApp_", "Skype"),
];

/// Bundle ID prefixes (lowercase) of macOS call apps and browsers, with display names. A prefix
/// also matches its helpers (`com.google.chrome.helper`).
const MAC_APPS: [(&str, &str); 19] = [
    ("us.zoom.xos", "Zoom"),
    ("com.microsoft.teams", "Microsoft Teams"),
    ("com.microsoft.teams2", "Microsoft Teams"),
    ("com.google.chrome", "Google Chrome"),
    ("com.microsoft.edgemac", "Microsoft Edge"),
    ("org.mozilla.firefox", "Firefox"),
    ("com.brave.browser", "Brave"),
    ("com.operasoftware.opera", "Opera"),
    ("company.thebrowser.browser", "Arc"),
    ("com.apple.safari", "Safari"),
    ("com.apple.webkit.gpu", "Safari"),
    ("com.apple.facetime", "FaceTime"),
    ("com.apple.avconferenced", "FaceTime"),
    ("com.tinyspeck.slackmacgap", "Slack"),
    ("com.cisco.webexmeetingsapp", "Webex"),
    ("cisco-systems.spark", "Webex"),
    ("com.skype.skype", "Skype"),
    ("com.hnc.discord", "Discord"),
    ("net.whatsapp.whatsapp", "WhatsApp"),
];

fn bundle_matches(bundle: &str, prefix: &str) -> bool {
    bundle == prefix
        || bundle
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// A call app that is using the microphone.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct DetectedCall {
    /// The registry subkey, stable while the app is installed.
    pub key: String,
    pub app: String,
    /// The app's root process, for capturing only its audio. `None` for packaged apps.
    pub pid: Option<u32>,
    /// Whether a call started from this offer captures only this app's audio. False on
    /// Windows builds before 20348 or when the process wasn't found: all system audio then.
    #[serde(default)]
    pub app_audio_only: bool,
}

/// The display name when `key` is a known call app, else `None`. `own_exe` (lowercase file
/// name) is skipped: Sidekick itself uses the mic during a call.
pub fn classify(key: &str, own_exe: &str, extra_exes: &[String]) -> Option<String> {
    if let Some(path) = key.strip_prefix("NonPackaged\\") {
        let exe = path.rsplit('#').next()?.to_lowercase();
        if exe == own_exe {
            return None;
        }
        if let Some((_, name)) = DESKTOP_APPS.iter().find(|(e, _)| *e == exe) {
            return Some(name.to_string());
        }
        return extra_exes
            .iter()
            .any(|e| e.eq_ignore_ascii_case(&exe))
            .then_some(exe);
    }
    if let Some((_, name)) = PACKAGED_APPS
        .iter()
        .find(|(prefix, _)| key.starts_with(prefix))
    {
        return Some(name.to_string());
    }
    // macOS: the key is the bundle ID of a process recording from the mic.
    let bundle = key.to_lowercase();
    if bundle.is_empty() || bundle_matches(&bundle, own_exe) {
        return None;
    }
    if let Some((_, name)) = MAC_APPS.iter().find(|(p, _)| bundle_matches(&bundle, p)) {
        return Some(name.to_string());
    }
    extra_exes
        .iter()
        .any(|e| bundle_matches(&bundle, &e.to_lowercase()))
        .then_some(key.to_string())
}

/// Exe file name of a `NonPackaged\...` key.
fn exe_of(key: &str) -> Option<String> {
    key.strip_prefix("NonPackaged\\")?
        .rsplit('#')
        .next()
        .map(str::to_lowercase)
}

#[derive(Default)]
pub struct CallDetectState {
    inner: Mutex<DetectInner>,
}

#[derive(Default)]
struct DetectInner {
    /// The call being offered to the rep, if any.
    offered: Option<DetectedCall>,
    /// Apps the rep dismissed; offered again only after they stop using the mic.
    dismissed: HashSet<String>,
}

impl CallDetectState {
    pub fn offered(&self) -> Option<DetectedCall> {
        self.inner
            .lock()
            .expect("detect lock poisoned")
            .offered
            .clone()
    }
}

#[derive(Clone, Serialize)]
struct ClearedPayload {
    key: String,
}

/// The call app currently offered, so a window that opens late can show the prompt.
#[tauri::command]
pub fn detected_call(state: tauri::State<'_, CallDetectState>) -> Option<DetectedCall> {
    state.offered()
}

/// The rep doesn't want coaching for this app's current call.
#[tauri::command]
pub fn dismiss_detected_call(
    app: AppHandle,
    state: tauri::State<'_, CallDetectState>,
    key: String,
) {
    let mut inner = state.inner.lock().expect("detect lock poisoned");
    inner.dismissed.insert(key.clone());
    if inner.offered.as_ref().is_some_and(|o| o.key == key) {
        inner.offered = None;
    }
    drop(inner);
    let _ = app.emit("call-detection-cleared", ClearedPayload { key });
    if !app.state::<crate::capture::CaptureState>().is_running() {
        crate::overlay::hide(&app);
    }
}

/// From the overlay's prompt: bring up the main window and open its start-call dialog.
#[tauri::command]
pub fn request_start_call(app: AppHandle) {
    crate::tray::show_main_window(&app);
    let _ = app.emit_to("main", "start-call-requested", ());
}

fn enabled() -> bool {
    !crate::config::env_or_dotenv("CALL_DETECTION").is_some_and(|v| v.eq_ignore_ascii_case("off"))
}

/// Polls in the background for the life of the app.
pub fn spawn(app: AppHandle) {
    // Sidekick records from the mic during a call: skip it by exe name (Windows) or bundle ID.
    #[cfg(target_os = "macos")]
    let own_exe = app.config().identifier.to_lowercase();
    #[cfg(not(target_os = "macos"))]
    let own_exe = std::env::current_exe()
        .ok()
        .and_then(|p| Some(p.file_name()?.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let _ = std::thread::Builder::new()
        .name("call-detect".into())
        .spawn(move || {
            let mut previously: HashMap<String, String> = HashMap::new();
            let mut call_app_gone_since: Option<Instant> = None;
            loop {
                // More call apps by exe name, comma-separated (e.g. a softphone).
                let extra: Vec<String> = crate::config::env_or_dotenv("CALL_DETECTION_EXTRA_APPS")
                    .unwrap_or_default()
                    .split(',')
                    .map(|e| e.trim().to_string())
                    .filter(|e| !e.is_empty())
                    .collect();
                let in_use: HashMap<String, String> = platform::mic_in_use()
                    .into_iter()
                    .filter_map(|key| Some((key.clone(), classify(&key, &own_exe, &extra)?)))
                    .collect();
                tick(&app, &previously, &in_use, &mut call_app_gone_since);
                previously = in_use;
                std::thread::sleep(POLL_INTERVAL);
            }
        });
}

fn tick(
    app: &AppHandle,
    previously: &HashMap<String, String>,
    in_use: &HashMap<String, String>,
    call_app_gone_since: &mut Option<Instant>,
) {
    let capture = app.state::<crate::capture::CaptureState>();
    let state = app.state::<CallDetectState>();

    // Apps that stopped using the mic: withdraw the offer, forget dismissals.
    for key in previously.keys().filter(|k| !in_use.contains_key(*k)) {
        let mut inner = state.inner.lock().expect("detect lock poisoned");
        inner.dismissed.remove(key);
        if inner.offered.as_ref().is_some_and(|o| &o.key == key) {
            inner.offered = None;
            drop(inner);
            let _ = app.emit(
                "call-detection-cleared",
                ClearedPayload { key: key.clone() },
            );
            if !capture.is_running() {
                crate::overlay::hide(app);
            }
        }
    }

    // A coached call whose call app has stopped using the mic for a while is over.
    match capture.running_call_app() {
        Some((session_id, key, name)) if !in_use.contains_key(&key) => {
            let since = *call_app_gone_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= CALL_APP_GONE_GRACE {
                *call_app_gone_since = None;
                crate::capture::end_call(
                    app,
                    Some(session_id),
                    Some(format!(
                        "Call ended automatically: {name} stopped using the microphone."
                    )),
                );
            }
        }
        _ => *call_app_gone_since = None,
    }

    if !enabled() || capture.is_running() {
        return;
    }
    // Offer a call app that just started using the mic.
    let started = in_use
        .iter()
        .find(|(key, _)| !previously.contains_key(*key));
    let Some((key, name)) = started else {
        return;
    };
    let mut inner = state.inner.lock().expect("detect lock poisoned");
    if inner.dismissed.contains(key) {
        return;
    }
    let pid = exe_of(key).and_then(|exe| platform::root_pid(&exe));
    let call = DetectedCall {
        key: key.clone(),
        app: name.clone(),
        pid,
        app_audio_only: pid.is_some() && supports_app_audio_capture(),
    };
    inner.offered = Some(call.clone());
    drop(inner);
    let _ = app.emit("call-detected", call);
    crate::overlay::show(app);
}

/// The Windows build number (e.g. 19045 for Windows 10 22H2), if known.
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub fn windows_build() -> Option<u32> {
    platform::windows_build()
}

/// True when this Windows build can capture a single app's audio (process loopback).
pub fn supports_app_audio_capture() -> bool {
    platform::windows_build().is_some_and(|build| build >= 20348)
}

#[cfg(windows)]
mod platform {
    use windows::core::{PCWSTR, PWSTR};
    use windows::Win32::Foundation::{CloseHandle, ERROR_SUCCESS};
    use windows::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegGetValueW, RegOpenKeyExW, HKEY, HKEY_CURRENT_USER,
        HKEY_LOCAL_MACHINE, KEY_READ, RRF_RT_REG_QWORD, RRF_RT_REG_SZ,
    };

    const MICROPHONE: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    struct Key(HKEY);

    impl Key {
        fn open(parent: HKEY, path: &str) -> Option<Self> {
            let path = wide(path);
            let mut key = HKEY::default();
            let status =
                unsafe { RegOpenKeyExW(parent, PCWSTR(path.as_ptr()), None, KEY_READ, &mut key) };
            (status == ERROR_SUCCESS).then_some(Self(key))
        }

        fn subkeys(&self) -> Vec<String> {
            let mut names = Vec::new();
            for index in 0.. {
                let mut buffer = [0u16; 512];
                let mut len = buffer.len() as u32;
                let status = unsafe {
                    RegEnumKeyExW(
                        self.0,
                        index,
                        Some(PWSTR(buffer.as_mut_ptr())),
                        &mut len,
                        None,
                        None,
                        None,
                        None,
                    )
                };
                if status != ERROR_SUCCESS {
                    break;
                }
                names.push(String::from_utf16_lossy(&buffer[..len as usize]));
            }
            names
        }

        fn qword(&self, subkey: &str, value: &str) -> Option<u64> {
            let (subkey, value) = (wide(subkey), wide(value));
            let mut data = 0u64;
            let mut size = std::mem::size_of::<u64>() as u32;
            let status = unsafe {
                RegGetValueW(
                    self.0,
                    PCWSTR(subkey.as_ptr()),
                    PCWSTR(value.as_ptr()),
                    RRF_RT_REG_QWORD,
                    None,
                    Some(&mut data as *mut u64 as *mut _),
                    Some(&mut size),
                )
            };
            (status == ERROR_SUCCESS).then_some(data)
        }

        fn string(&self, value: &str) -> Option<String> {
            let value = wide(value);
            let mut buffer = [0u16; 64];
            let mut size = std::mem::size_of_val(&buffer) as u32;
            let status = unsafe {
                RegGetValueW(
                    self.0,
                    PCWSTR::null(),
                    PCWSTR(value.as_ptr()),
                    RRF_RT_REG_SZ,
                    None,
                    Some(buffer.as_mut_ptr() as *mut _),
                    Some(&mut size),
                )
            };
            if status != ERROR_SUCCESS {
                return None;
            }
            let len = (size as usize / 2).saturating_sub(1);
            Some(String::from_utf16_lossy(&buffer[..len]))
        }
    }

    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }

    /// Registry keys (`<package>` or `NonPackaged\<path>`) of apps using the mic right now.
    pub fn mic_in_use() -> Vec<String> {
        let Some(root) = Key::open(HKEY_CURRENT_USER, MICROPHONE) else {
            return Vec::new();
        };
        let in_use = |key: &Key, subkey: &str| {
            key.qword(subkey, "LastUsedTimeStart")
                .is_some_and(|t| t > 0)
                && key.qword(subkey, "LastUsedTimeStop") == Some(0)
        };
        let mut keys: Vec<String> = root
            .subkeys()
            .into_iter()
            .filter(|name| name != "NonPackaged" && in_use(&root, name))
            .collect();
        if let Some(non_packaged) = Key::open(root.0, "NonPackaged") {
            keys.extend(
                non_packaged
                    .subkeys()
                    .into_iter()
                    .filter(|name| in_use(&non_packaged, name))
                    .map(|name| format!("NonPackaged\\{name}")),
            );
        }
        keys
    }

    /// The oldest process of `exe` whose parent is a different program: the app's root, so
    /// capturing its process tree includes all of its audio.
    pub fn root_pid(exe: &str) -> Option<u32> {
        let snapshot = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.ok()?;
        let mut processes: Vec<(u32, u32, String)> = Vec::new();
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut more = unsafe { Process32FirstW(snapshot, &mut entry) }.is_ok();
        while more {
            let len = entry.szExeFile.iter().position(|&c| c == 0).unwrap_or(0);
            let name = String::from_utf16_lossy(&entry.szExeFile[..len]).to_lowercase();
            processes.push((entry.th32ProcessID, entry.th32ParentProcessID, name));
            more = unsafe { Process32NextW(snapshot, &mut entry) }.is_ok();
        }
        unsafe {
            let _ = CloseHandle(snapshot);
        }
        let same_exe: HashSetU32 = processes
            .iter()
            .filter(|(_, _, name)| name == exe)
            .map(|(pid, _, _)| *pid)
            .collect();
        processes
            .iter()
            .find(|(pid, parent, name)| {
                name == exe && same_exe.contains(pid) && !same_exe.contains(parent)
            })
            .map(|(pid, _, _)| *pid)
    }

    type HashSetU32 = std::collections::HashSet<u32>;

    pub fn windows_build() -> Option<u32> {
        Key::open(
            HKEY_LOCAL_MACHINE,
            r"SOFTWARE\Microsoft\Windows NT\CurrentVersion",
        )?
        .string("CurrentBuildNumber")?
        .parse()
        .ok()
    }
}

#[cfg(target_os = "macos")]
mod platform {
    /// Bundle IDs of processes recording from the mic right now.
    pub fn mic_in_use() -> Vec<String> {
        crate::audio::coreaudio_capture::processes_using_mic()
    }

    pub fn root_pid(_exe: &str) -> Option<u32> {
        None
    }

    pub fn windows_build() -> Option<u32> {
        None
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    pub fn mic_in_use() -> Vec<String> {
        Vec::new()
    }

    pub fn root_pid(_exe: &str) -> Option<u32> {
        None
    }

    pub fn windows_build() -> Option<u32> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_call_apps_and_skips_others() {
        let own = "sidekick.exe";
        assert_eq!(
            classify(r"NonPackaged\C:#Program Files#Zoom#bin#Zoom.exe", own, &[]).as_deref(),
            Some("Zoom")
        );
        assert_eq!(
            classify(r"MSTeams_8wekyb3d8bbwe", own, &[]).as_deref(),
            Some("Microsoft Teams")
        );
        assert_eq!(
            classify(r"NonPackaged\C:#Windows#System32#rundll32.exe", own, &[]),
            None
        );
        assert_eq!(
            classify(r"NonPackaged\C:#x#target#debug#sidekick.exe", own, &[]),
            None
        );
        assert_eq!(
            classify(r"NonPackaged\C:#x#probe.exe", own, &["PROBE.exe".into()]).as_deref(),
            Some("probe.exe")
        );
    }

    #[test]
    fn recognizes_mac_call_apps() {
        let own = "com.sidekick.callcoach";
        assert_eq!(classify("us.zoom.xos", own, &[]).as_deref(), Some("Zoom"));
        assert_eq!(
            classify("com.google.Chrome.helper", own, &[]).as_deref(),
            Some("Google Chrome")
        );
        assert_eq!(classify("com.sidekick.callcoach", own, &[]), None);
        assert_eq!(classify("com.apple.VoiceMemos", own, &[]), None);
        assert_eq!(classify("com.google.chromex", own, &[]), None);
        assert_eq!(
            classify("com.acme.Phone", own, &["com.acme.phone".into()]).as_deref(),
            Some("com.acme.Phone")
        );
    }

    /// Test helper for the call detection flow: holds the microphone open for
    /// `HOLD_MIC_SECS` (default 60), like a call app would.
    /// `cargo test hold_mic_open -- --ignored --nocapture`
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn hold_mic_open() {
        use crate::audio::{CaptureEvent, Channel, LoopbackTarget};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{mpsc, Arc};

        let secs: u64 = std::env::var("HOLD_MIC_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(60);
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<CaptureEvent>();
        let handle = crate::audio::wasapi_capture::spawn(
            Channel::Me,
            LoopbackTarget::DefaultOutput,
            stop.clone(),
            tx,
        )
        .unwrap();
        println!("holding the mic for {secs} s");
        let until = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < until {
            let _ = rx.recv_timeout(Duration::from_millis(200));
        }
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
    }

    /// Live: opens the microphone from this test process and checks the detector sees it in
    /// use, then released. `cargo test mic_use_is_detected -- --ignored --nocapture`
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn mic_use_is_detected() {
        use crate::audio::{CaptureEvent, Channel, LoopbackTarget};
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::{mpsc, Arc};

        let own_exe = std::env::current_exe().unwrap();
        let own_exe = own_exe
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_lowercase();
        let mine = |keys: Vec<String>| {
            keys.into_iter()
                .filter(|k| classify(k, "", std::slice::from_ref(&own_exe)).is_some())
                .collect::<Vec<_>>()
        };
        assert!(
            mine(platform::mic_in_use()).is_empty(),
            "mic already in use by this test"
        );

        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<CaptureEvent>();
        let handle = crate::audio::wasapi_capture::spawn(
            Channel::Me,
            LoopbackTarget::DefaultOutput,
            stop.clone(),
            tx,
        )
        .unwrap();
        let _ = rx.recv_timeout(Duration::from_secs(3));
        std::thread::sleep(Duration::from_millis(500));
        let during = mine(platform::mic_in_use());
        println!("in use while capturing: {during:?}");
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        std::thread::sleep(Duration::from_millis(500));
        let after = mine(platform::mic_in_use());
        println!("in use after stopping: {after:?}");
        println!(
            "windows build {:?}, app audio capture supported: {}",
            platform::windows_build(),
            supports_app_audio_capture()
        );
        println!(
            "root chrome.exe pid: {:?}",
            platform::root_pid("chrome.exe")
        );
        assert_eq!(during.len(), 1);
        assert!(after.is_empty());
    }
}
