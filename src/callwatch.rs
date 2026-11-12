//! Detects calls by asking Windows which apps are using the microphone right now.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use winreg::RegKey;
use winreg::enums::HKEY_CURRENT_USER;

use crate::app::Core;

const KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone";
const POLL: Duration = Duration::from_secs(2);
const END_GRACE: Duration = Duration::from_secs(10); // a call only counts as over after the mic has been free this long

const FRIENDLY: &[(&str, &str)] = &[
    ("msteams", "Teams"),
    ("teams", "Teams"),
    ("zoom", "Zoom"),
    ("chrome", "Chrome"),
    ("msedge", "Edge"),
    ("firefox", "Firefox"),
    ("slack", "Slack"),
    ("discord", "Discord"),
    ("webex", "Webex"),
    ("skype", "Skype"),
    ("whatsapp", "WhatsApp"),
    ("signal", "Signal"),
];

/// NonPackaged entries are exe paths with '#' for '\'; packaged ones are package family names.
fn exe_name(entry: &str) -> String {
    let last = entry.rsplit(['#', '\\']).next().unwrap_or(entry);
    let name = last.split('_').next().unwrap_or(last).to_lowercase();
    name.strip_suffix(".exe").map(str::to_string).unwrap_or(name)
}

fn friendly(entry: &str) -> String {
    let name = exe_name(entry);
    FRIENDLY.iter().find(|(k, _)| name.contains(k)).map(|(_, v)| v.to_string()).unwrap_or(name)
}

fn in_use(key: &RegKey) -> bool {
    let start: u64 = key.get_value("LastUsedTimeStart").unwrap_or(0);
    let stop: u64 = key.get_value("LastUsedTimeStop").unwrap_or(1);
    start > 0 && stop == 0
}

/// Apps holding the mic, without our own process (which holds it all the time).
pub fn apps_using_mic() -> Vec<String> {
    let own = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()))
        .unwrap_or_default();
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let mut found = Vec::new();
    for path in [KEY.to_string(), format!(r"{KEY}\NonPackaged")] {
        let Ok(parent) = hkcu.open_subkey(&path) else { continue };
        for name in parent.enum_keys().flatten() {
            if name == "NonPackaged" || exe_name(&name) == own {
                continue;
            }
            if parent.open_subkey(&name).is_ok_and(|k| in_use(&k)) {
                found.push(friendly(&name));
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

pub fn run(core: Arc<Core>) {
    let mut current: Option<String> = None;
    let mut free_for = Duration::ZERO;
    while !core.stop.load(Ordering::Relaxed) {
        let apps = apps_using_mic();
        if !apps.is_empty() {
            free_for = Duration::ZERO;
            let app = apps.join(", ");
            if current.as_deref() != Some(&app) {
                if current.is_none() {
                    log::info!("Call started ({app})");
                }
                current = Some(app.clone());
                core.on_call(Some(app));
            }
        } else if let Some(app) = &current {
            free_for += POLL;
            if free_for >= END_GRACE {
                log::info!("Call ended ({app})");
                current = None;
                core.on_call(None);
            }
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn friendly_names() {
        assert_eq!(super::friendly(r"C:#Program Files#Zoom#bin#Zoom.exe"), "Zoom");
        assert_eq!(super::friendly("MSTeams_8wekyb3d8bbwe"), "Teams");
        assert_eq!(super::exe_name(r"C:#tools#transcribe.exe"), "transcribe");
    }
}
