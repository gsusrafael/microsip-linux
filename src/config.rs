//! Persistencia local de cuentas SIP y ajustes de audio.
//!
//! Formato simple `clave=valor`. Las cuentas se separan por una línea en
//! blanco; el audio vive en `audio.conf`. No se crea ningún fichero al
//! arrancar: solo al guardar. Permisos 600 para cuentas y ajustes.

use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AccountCfg {
    pub id: String,
    pub username: String,
    pub password: String,
    pub domain: String,
    pub display_name: String,
    pub proxy: String,
}

impl AccountCfg {
    pub fn new(username: &str, domain: &str) -> Self {
        Self {
            id: format!("{username}@{domain}"),
            username: username.to_string(),
            password: String::new(),
            domain: domain.to_string(),
            display_name: String::new(),
            proxy: String::new(),
        }
    }
}

/// Claves estables de los dispositivos elegidos para captura y reproducción.
/// Vacío = predeterminado del sistema. El timbre es un WAV PCM de 16 bit mono
/// a 16000 Hz (requisito de este build de PJSUA sin remuestreo).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioSettings {
    pub capture_device: String,
    pub playback_device: String,
    pub ringtone: Option<PathBuf>,
}

fn config_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        if !dir.is_empty() {
            return PathBuf::from(dir).join("microsip-linux");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".config/microsip-linux")
}

fn config_path() -> PathBuf {
    config_dir().join("accounts.conf")
}

fn audio_config_path() -> PathBuf {
    config_dir().join("audio.conf")
}

pub fn load_audio() -> AudioSettings {
    let Ok(content) = fs::read_to_string(audio_config_path()) else {
        return AudioSettings::default();
    };
    let mut settings = AudioSettings::default();
    for raw in content.lines() {
        let Some((key, value)) = raw.split_once('=') else {
            continue;
        };
        let value = unescape(value);
        match key.trim() {
            "capture_device" => settings.capture_device = value,
            "playback_device" => settings.playback_device = value,
            "ringtone" if !value.is_empty() => settings.ringtone = Some(PathBuf::from(value)),
            _ => {}
        }
    }
    settings
}

pub fn save_audio(settings: &AudioSettings) -> io::Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    let path = audio_config_path();
    let ringtone = settings
        .ringtone
        .as_ref()
        .map(|path| escape(&path.to_string_lossy()))
        .unwrap_or_default();
    let content = format!(
        "capture_device={}\nplayback_device={}\nringtone={}\n",
        escape(&settings.capture_device),
        escape(&settings.playback_device),
        ringtone
    );
    fs::write(&path, content)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

pub fn load() -> Vec<AccountCfg> {
    let path = config_path();
    let Ok(content) = fs::read_to_string(&path) else {
        return Vec::new();
    };

    let mut accounts = Vec::new();
    let mut current: Option<AccountCfg> = None;

    for raw in content.lines() {
        let line = raw.trim();
        if line.is_empty() {
            if let Some(account) = current.take() {
                accounts.push(account);
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = unescape(value);
        match key.trim() {
            "id" => {
                if let Some(account) = current.take() {
                    accounts.push(account);
                }
                current = Some(AccountCfg {
                    id: value,
                    username: String::new(),
                    password: String::new(),
                    domain: String::new(),
                    display_name: String::new(),
                    proxy: String::new(),
                });
            }
            "username" => {
                if let Some(account) = current.as_mut() {
                    account.username = value;
                }
            }
            "password" => {
                if let Some(account) = current.as_mut() {
                    account.password = value;
                }
            }
            "domain" => {
                if let Some(account) = current.as_mut() {
                    account.domain = value;
                }
            }
            "display_name" => {
                if let Some(account) = current.as_mut() {
                    account.display_name = value;
                }
            }
            "proxy" => {
                if let Some(account) = current.as_mut() {
                    account.proxy = value;
                }
            }
            _ => {}
        }
    }
    if let Some(account) = current.take() {
        accounts.push(account);
    }
    accounts
}

pub fn save(accounts: &[AccountCfg]) -> io::Result<()> {
    let dir = config_dir();
    fs::create_dir_all(&dir)?;
    let path = config_path();

    let mut content = String::new();
    for account in accounts {
        content.push_str("id=");
        content.push_str(&escape(&account.id));
        content.push_str("\nusername=");
        content.push_str(&escape(&account.username));
        content.push_str("\npassword=");
        content.push_str(&escape(&account.password));
        content.push_str("\ndomain=");
        content.push_str(&escape(&account.domain));
        content.push_str("\ndisplay_name=");
        content.push_str(&escape(&account.display_name));
        content.push_str("\nproxy=");
        content.push_str(&escape(&account.proxy));
        content.push_str("\n\n");
    }

    fs::write(&path, content)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    Ok(())
}

pub fn upsert(account: AccountCfg) -> io::Result<Vec<AccountCfg>> {
    let mut accounts = load();
    match accounts.iter_mut().find(|item| item.id == account.id) {
        Some(existing) => *existing = account,
        None => accounts.push(account),
    }
    save(&accounts)?;
    Ok(accounts)
}

pub fn remove(id: &str) -> io::Result<Vec<AccountCfg>> {
    let mut accounts = load();
    accounts.retain(|item| item.id != id);
    save(&accounts)?;
    Ok(accounts)
}

fn escape(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_path() -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        PathBuf::from(format!("/tmp/microsip-config-test-{nanos}"))
    }

    #[test]
    fn roundtrip_account() {
        let account = AccountCfg {
            id: "alice@localhost".into(),
            username: "alice".into(),
            password: "p\\ass=word".into(),
            domain: "localhost".into(),
            display_name: "Alice".into(),
            proxy: "sip:localhost".into(),
        };
        let accounts = vec![account.clone()];
        let dir = test_path();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        save(&accounts).unwrap();
        let loaded = load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].password, account.password);
        assert_eq!(loaded[0].id, account.id);

        // upsert actualiza la contraseña y añade una segunda cuenta.
        let mut updated = account.clone();
        updated.password = "new-secret".into();
        upsert(updated).unwrap();
        upsert(AccountCfg::new("bob", "localhost")).unwrap();
        let loaded = load();
        assert_eq!(loaded.len(), 2);
        assert!(loaded.iter().any(|item| item.password == "new-secret"));

        // remove elimina la cuenta indicada.
        remove("alice@localhost").unwrap();
        let loaded = load();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "bob@localhost");

        let audio = AudioSettings {
            capture_device: "ALSA\nMicrófono USB".into(),
            playback_device: "ALSA\nAuriculares".into(),
            ringtone: Some(PathBuf::from("/tmp/ring tone.wav")),
        };
        save_audio(&audio).unwrap();
        assert_eq!(load_audio(), audio);
        let mode = fs::metadata(audio_config_path())
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);

        let _ = fs::remove_dir_all(dir);
    }
}
