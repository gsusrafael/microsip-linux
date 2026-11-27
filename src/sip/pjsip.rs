//! Backend SIP real: bindings FFI contra PJSUA (PJSIP, C API).
//!
//! Los structs C vienen de bindgen (`pjsip_bindings.rs`); las funciones
//! externas se declaran a mano porque bindgen no expande `PJ_DECL`.
//!
//! Requiere pjproject compilado en `~/.local` (ver `build.rs`).
//! Compilar: `cargo build --features pjsip`.

use std::collections::{HashMap, HashSet};
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::raw::{c_char, c_uint, c_void};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tokio::sync::mpsc;

use super::{
    AccountId, AccountInfo, AudioDeviceInfo, CallId, CallInfo, CallState, RegistrationState,
    SipBackend, SipCommand, SipEvent,
};

mod bindings {
    #![allow(unused_imports)]
    #![allow(unused)]
    #![allow(dead_code)]
    #![allow(non_upper_case_globals)]
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    #![allow(clippy::all)]
    include!(concat!(env!("OUT_DIR"), "/pjsip_bindings.rs"));
}

use bindings::{
    pj_ssize_t, pj_status_t, pj_str_t, pjmedia_aud_dev_info, pjsip_cred_info, pjsip_status_code,
    pjsip_transport_type_e, pjsua_acc_config, pjsua_acc_id, pjsua_acc_info, pjsua_call_id,
    pjsua_call_info, pjsua_call_setting, pjsua_config, pjsua_logging_config, pjsua_media_config,
    pjsua_msg_data, pjsua_player_id, pjsua_transport_config, pjsua_transport_id,
};

// ---------------------------------------------------------------------------
// Declaraciones FFI (bindgen no expande PJ_DECL)
// ---------------------------------------------------------------------------

extern "C" {
    fn pjsua_create() -> pj_status_t;
    fn pjsua_init(
        ua_cfg: *const pjsua_config,
        log_cfg: *const pjsua_logging_config,
        media_cfg: *const pjsua_media_config,
    ) -> pj_status_t;
    fn pjsua_start() -> pj_status_t;
    fn pjsua_destroy() -> pj_status_t;
    fn pjsua_config_default(cfg: *mut pjsua_config);
    fn pjsua_logging_config_default(cfg: *mut pjsua_logging_config);
    fn pjsua_media_config_default(cfg: *mut pjsua_media_config);
    fn pjsua_transport_config_default(cfg: *mut pjsua_transport_config);
    fn pjsua_transport_create(
        type_: pjsip_transport_type_e,
        cfg: *const pjsua_transport_config,
        p_id: *mut pjsua_transport_id,
    ) -> pj_status_t;
    fn pjsua_acc_config_default(cfg: *mut pjsua_acc_config);
    fn pjsua_acc_add(
        acc_cfg: *const pjsua_acc_config,
        is_default: i32,
        p_acc_id: *mut pjsua_acc_id,
    ) -> pj_status_t;
    fn pjsua_acc_del(acc_id: pjsua_acc_id) -> pj_status_t;
    fn pjsua_acc_get_info(acc_id: pjsua_acc_id, info: *mut pjsua_acc_info) -> pj_status_t;
    fn pjsua_acc_set_registration(acc_id: pjsua_acc_id, renew: i32) -> pj_status_t;
    fn pjsua_call_make_call(
        acc_id: pjsua_acc_id,
        dst_uri: *const pj_str_t,
        opt: *const pjsua_call_setting,
        user_data: *mut c_void,
        msg_data: *const pjsua_msg_data,
        p_call_id: *mut pjsua_call_id,
    ) -> pj_status_t;
    fn pjsua_call_get_info(call_id: pjsua_call_id, info: *mut pjsua_call_info) -> pj_status_t;
    fn pjsua_call_answer(
        call_id: pjsua_call_id,
        code: u32,
        reason: *const pj_str_t,
        msg_data: *const pjsua_msg_data,
    ) -> pj_status_t;
    fn pjsua_call_hangup(
        call_id: pjsua_call_id,
        code: u32,
        reason: *const pj_str_t,
        msg_data: *const pjsua_msg_data,
    ) -> pj_status_t;
    fn pjsua_call_dial_dtmf(call_id: pjsua_call_id, digits: *const pj_str_t) -> pj_status_t;
    fn pjsua_conf_connect(source: i32, sink: i32) -> pj_status_t;
    fn pjsua_conf_disconnect(source: i32, sink: i32) -> pj_status_t;
    fn pjsua_enum_aud_devs(info: *mut pjmedia_aud_dev_info, count: *mut c_uint) -> pj_status_t;
    fn pjsua_set_snd_dev(capture_dev: i32, playback_dev: i32) -> pj_status_t;
    fn pjsua_player_create(
        filename: *const pj_str_t,
        options: c_uint,
        p_id: *mut pjsua_player_id,
    ) -> pj_status_t;
    fn pjsua_player_get_conf_port(id: pjsua_player_id) -> i32;
    fn pjsua_player_destroy(id: pjsua_player_id) -> pj_status_t;
}

const PJ_SUCCESS: i32 = 0;
const PJSIP_TRANSPORT_UDP: u32 = bindings::pjsip_transport_type_e_PJSIP_TRANSPORT_UDP;
const PJSIP_ROLE_UAS: u32 = bindings::pjsip_role_e_PJSIP_ROLE_UAS;
const PJSIP_INV_STATE_CALLING: u32 = bindings::pjsip_inv_state_PJSIP_INV_STATE_CALLING;
const PJSIP_INV_STATE_INCOMING: u32 = bindings::pjsip_inv_state_PJSIP_INV_STATE_INCOMING;
const PJSIP_INV_STATE_EARLY: u32 = bindings::pjsip_inv_state_PJSIP_INV_STATE_EARLY;
const PJSIP_INV_STATE_CONNECTING: u32 = bindings::pjsip_inv_state_PJSIP_INV_STATE_CONNECTING;
const PJSIP_INV_STATE_CONFIRMED: u32 = bindings::pjsip_inv_state_PJSIP_INV_STATE_CONFIRMED;
const PJSIP_INV_STATE_DISCONNECTED: u32 = bindings::pjsip_inv_state_PJSIP_INV_STATE_DISCONNECTED;
const PJSUA_CALL_MEDIA_ACTIVE: u32 = bindings::pjsua_call_media_status_PJSUA_CALL_MEDIA_ACTIVE;
const PJSIP_SC_OK: u32 = bindings::pjsip_status_code_PJSIP_SC_OK;
const PJSIP_SC_TRYING: u32 = bindings::pjsip_status_code_PJSIP_SC_TRYING;
const PJSIP_SC_BUSY_HERE: u32 = bindings::pjsip_status_code_PJSIP_SC_BUSY_HERE;
const PJSIP_CRED_DATA_PLAIN_PASSWD: i32 = 0;

// ---------------------------------------------------------------------------
// Estado global de callbacks (pjsua es singleton)
// ---------------------------------------------------------------------------

static EVENT_TX: Mutex<Option<mpsc::UnboundedSender<SipEvent>>> = Mutex::new(None);
static ACCOUNT_INFOS: OnceLock<Mutex<HashMap<AccountId, AccountInfo>>> = OnceLock::new();
static CALL_INFOS: OnceLock<Mutex<HashMap<CallId, CallInfo>>> = OnceLock::new();
static ACCOUNT_IDS: OnceLock<Mutex<HashMap<pjsua_acc_id, AccountId>>> = OnceLock::new();
static RINGTONE_PATH: OnceLock<Mutex<Option<std::path::PathBuf>>> = OnceLock::new();
static RINGTONE_PLAYER: OnceLock<Mutex<Option<pjsua_player_id>>> = OnceLock::new();
static RINGTONE_PREVIEW: OnceLock<Mutex<bool>> = OnceLock::new();
static RINGING_CALLS: OnceLock<Mutex<HashSet<pjsua_call_id>>> = OnceLock::new();

fn ringtone_path() -> &'static Mutex<Option<std::path::PathBuf>> {
    RINGTONE_PATH.get_or_init(|| Mutex::new(None))
}

fn ringtone_player() -> &'static Mutex<Option<pjsua_player_id>> {
    RINGTONE_PLAYER.get_or_init(|| Mutex::new(None))
}

fn ringtone_preview() -> &'static Mutex<bool> {
    RINGTONE_PREVIEW.get_or_init(|| Mutex::new(false))
}

fn ringing_calls() -> &'static Mutex<HashSet<pjsua_call_id>> {
    RINGING_CALLS.get_or_init(|| Mutex::new(HashSet::new()))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}

fn account_infos() -> &'static Mutex<HashMap<AccountId, AccountInfo>> {
    ACCOUNT_INFOS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn call_infos() -> &'static Mutex<HashMap<CallId, CallInfo>> {
    CALL_INFOS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn account_ids() -> &'static Mutex<HashMap<pjsua_acc_id, AccountId>> {
    ACCOUNT_IDS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn emit(event: SipEvent) {
    if let Some(tx) = lock(&EVENT_TX).as_ref() {
        let _ = tx.send(event);
    }
}

fn pj_to_string(value: &pj_str_t) -> String {
    if value.ptr.is_null() || value.slen <= 0 {
        return String::new();
    }
    unsafe {
        let bytes = std::slice::from_raw_parts(value.ptr as *const u8, value.slen as usize);
        String::from_utf8_lossy(bytes).into_owned()
    }
}

fn fixed_c_string(ptr: *const c_char) -> String {
    unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() }
}

fn default_ringtone() -> Option<std::path::PathBuf> {
    [
        "/usr/share/sounds/microsip-linux/ringtone.wav",
        concat!(env!("CARGO_MANIFEST_DIR"), "/assets/ringtone.wav"),
        "/usr/share/sounds/elementary/stereo/bell.wav",
        "/usr/share/sounds/freedesktop/stereo/phone.wav",
    ]
    .iter()
    .map(std::path::PathBuf::from)
    .find(|path| path.is_file())
}

/// PJSUA solo reproduce WAV PCM de 16 bit mono. Validar el contenedor evita
/// guardar un timbre que solo fallaría al recibir la primera llamada.
fn validate_ringtone_wav(path: &std::path::Path) -> Result<(), String> {
    let mut file = File::open(path).map_err(|err| format!("No se pudo abrir el timbre: {err}"))?;
    let mut header = [0u8; 12];
    file.read_exact(&mut header)
        .map_err(|err| format!("WAV inválido: {err}"))?;
    if &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err("El timbre debe ser un WAV válido".into());
    }

    let file_len = file.metadata().map_err(|err| err.to_string())?.len();
    let mut offset = 12u64;
    let mut found_format = false;
    let mut found_data = false;
    while offset.saturating_add(8) <= file_len {
        file.seek(SeekFrom::Start(offset))
            .map_err(|err| err.to_string())?;
        let mut chunk = [0u8; 8];
        file.read_exact(&mut chunk)
            .map_err(|err| format!("WAV inválido: {err}"))?;
        let size = u32::from_le_bytes(chunk[4..8].try_into().unwrap()) as u64;
        let data_offset = offset + 8;
        if &chunk[0..4] == b"fmt " {
            found_format = true;
            if size < 16 || data_offset.saturating_add(size) > file_len {
                return Err("WAV inválido: bloque fmt incompleto".into());
            }
            let mut fmt = [0u8; 16];
            file.read_exact(&mut fmt)
                .map_err(|err| format!("WAV inválido: {err}"))?;
            let encoding = u16::from_le_bytes(fmt[0..2].try_into().unwrap());
            let channels = u16::from_le_bytes(fmt[2..4].try_into().unwrap());
            let sample_rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap());
            let bits = u16::from_le_bytes(fmt[14..16].try_into().unwrap());
            if encoding != 1 || channels != 1 || bits != 16 {
                return Err("El WAV debe ser PCM de 16 bit y mono".into());
            }
            // pjproject se compila con --disable-resample y el puente usa
            // PJSUA_DEFAULT_CLOCK_RATE (16 kHz). Otro reloj no se puede
            // conectar al conference bridge.
            if sample_rate != 16_000 {
                return Err("El WAV debe muestrearse a 16000 Hz".into());
            }
        } else if &chunk[0..4] == b"data" {
            if data_offset.saturating_add(size) > file_len {
                return Err("WAV inválido: bloque data incompleto".into());
            }
            found_data = true;
        }
        offset = data_offset.saturating_add(size).saturating_add(size % 2);
    }
    if !found_format || !found_data {
        return Err("WAV inválido: faltan bloques fmt o data".into());
    }
    Ok(())
}

fn start_ringtone_player() -> Result<(), String> {
    let mut player = lock(ringtone_player());
    if player.is_some() {
        return Ok(());
    }
    let configured = lock(ringtone_path()).clone();
    let path = configured
        .or_else(default_ringtone)
        .ok_or_else(|| "No hay timbre WAV disponible: selecciona uno en Sonido".to_string())?;
    let path_c = CString::new(path.to_string_lossy().as_bytes()).map_err(|e| e.to_string())?;
    let path_pj = pj_str_from_cstring(&path_c);
    let mut id = 0;
    let status = unsafe { pjsua_player_create(&path_pj, 0, &mut id) };
    if status != PJ_SUCCESS {
        return Err(format!(
            "No se pudo abrir el timbre {}: {status}",
            path.display()
        ));
    }
    let slot = unsafe { pjsua_player_get_conf_port(id) };
    let connected = unsafe { pjsua_conf_connect(slot, 0) };
    if connected != PJ_SUCCESS {
        unsafe {
            let _ = pjsua_player_destroy(id);
        }
        return Err(format!(
            "No se pudo conectar el timbre al altavoz: {connected}"
        ));
    }
    *player = Some(id);
    tracing::info!(target: "sip::pjsip", "timbre reproduciéndose: {}", path.display());
    Ok(())
}

fn stop_ringtone_player() {
    let id = lock(ringtone_player()).take();
    if let Some(id) = id {
        unsafe {
            let slot = pjsua_player_get_conf_port(id);
            let _ = pjsua_conf_disconnect(slot, 0);
            let _ = pjsua_player_destroy(id);
        }
        tracing::info!(target: "sip::pjsip", "timbre detenido");
    }
}

fn sync_ringtone() {
    // Tomar los guards por separado: en una expresión con `||` el primer
    // MutexGuard puede seguir vivo mientras se toma el segundo, en orden
    // inverso al de update_call_info (ringing -> preview).
    let preview = *lock(ringtone_preview());
    let ringing = !lock(ringing_calls()).is_empty();
    if !preview && !ringing {
        stop_ringtone_player();
    } else if let Err(err) = start_ringtone_player() {
        tracing::warn!(target: "sip::pjsip", "timbre: {err}");
    }
}

fn stop_ringing_call(call_id: pjsua_call_id) {
    lock(ringing_calls()).remove(&call_id);
    *lock(ringtone_preview()) = false;
    sync_ringtone();
}

fn map_reg_state(status: pjsip_status_code) -> RegistrationState {
    match status {
        0 => RegistrationState::Unknown,
        PJSIP_SC_TRYING | 401 | 407 => RegistrationState::Registering,
        PJSIP_SC_OK => RegistrationState::Ok,
        _ => RegistrationState::Failed,
    }
}

fn map_call_state(state: u32) -> CallState {
    match state {
        PJSIP_INV_STATE_CALLING => CallState::Outgoing,
        PJSIP_INV_STATE_INCOMING => CallState::Incoming,
        PJSIP_INV_STATE_EARLY => CallState::Early,
        PJSIP_INV_STATE_CONNECTING => CallState::Outgoing,
        PJSIP_INV_STATE_CONFIRMED => CallState::Connected,
        PJSIP_INV_STATE_DISCONNECTED => CallState::Disconnected,
        _ => CallState::Idle,
    }
}

fn update_account_info(acc_id: pjsua_acc_id) {
    let mut info: pjsua_acc_info = unsafe { std::mem::zeroed() };
    if unsafe { pjsua_acc_get_info(acc_id, &mut info as *mut _) } != PJ_SUCCESS {
        return;
    }
    let status_text = pj_to_string(&info.status_text);
    let state = map_reg_state(info.status);
    let Some(account_id) = lock(account_ids()).get(&acc_id).cloned() else {
        return;
    };
    let username = account_id.0.split('@').next().unwrap_or("").to_string();
    let domain = account_id.0.split('@').nth(1).unwrap_or("").to_string();
    lock(account_infos()).insert(
        account_id.clone(),
        AccountInfo {
            id: account_id.clone(),
            username,
            domain,
            display_name: String::new(),
            state,
            status_text: status_text.clone(),
        },
    );
    emit(SipEvent::RegistrationChanged {
        account: account_id,
        state,
        status_text,
    });
    emit(SipEvent::AccountListChanged);
}

fn update_call_info(call_id: pjsua_call_id) -> Option<CallInfo> {
    let mut info: pjsua_call_info = unsafe { std::mem::zeroed() };
    if unsafe { pjsua_call_get_info(call_id, &mut info as *mut _) } != PJ_SUCCESS {
        return None;
    }
    let call = CallId(call_id.to_string());
    let peer = pj_to_string(&info.remote_info);
    let state = map_call_state(info.state);
    let incoming = info.role == PJSIP_ROLE_UAS;
    let call_info = CallInfo {
        id: call.clone(),
        peer,
        state,
        incoming,
    };
    if state == CallState::Disconnected {
        lock(call_infos()).remove(&call);
    } else {
        lock(call_infos()).insert(call.clone(), call_info.clone());
    }
    if incoming {
        if matches!(state, CallState::Incoming | CallState::Early) {
            lock(ringing_calls()).insert(call_id);
        } else {
            lock(ringing_calls()).remove(&call_id);
            *lock(ringtone_preview()) = false;
        }
    }
    emit(SipEvent::CallStateChanged {
        call,
        state,
        peer: call_info.peer.clone(),
        incoming,
    });
    emit(SipEvent::CallListChanged);
    Some(call_info)
}

// ---------------------------------------------------------------------------
// Callbacks C
// ---------------------------------------------------------------------------

extern "C" fn cb_on_reg_state(acc_id: pjsua_acc_id) {
    let _ = catch_unwind(AssertUnwindSafe(|| update_account_info(acc_id)));
}

extern "C" fn cb_on_call_state(call_id: pjsua_call_id, _event: *mut bindings::pjsip_event) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let _ = update_call_info(call_id);
    }));
}

extern "C" fn cb_on_incoming_call(
    _acc_id: pjsua_acc_id,
    call_id: pjsua_call_id,
    _rdata: *mut bindings::pjsip_rx_data,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        // Una llamada real sustituye a la prueba de timbre: al contestar debe
        // detenerse, no quedar sonando por la bandera de preview.
        *lock(ringtone_preview()) = false;
        if let Some(info) = update_call_info(call_id) {
            emit(SipEvent::CallIncoming {
                call: info.id,
                from: info.peer.clone(),
                to: String::new(),
            });
        }
        // 180 Ringing: el usuario aún decide si contestar.
        unsafe {
            let _ = pjsua_call_answer(call_id, 180, std::ptr::null(), std::ptr::null());
        }
    }));
}

extern "C" fn cb_on_call_media_state(call_id: pjsua_call_id) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let mut info: pjsua_call_info = unsafe { std::mem::zeroed() };
        if unsafe { pjsua_call_get_info(call_id, &mut info as *mut _) } != PJ_SUCCESS {
            return;
        }
        if info.media_status != PJSUA_CALL_MEDIA_ACTIVE {
            return;
        }
        let slot = info.conf_slot;
        if slot >= 0 {
            unsafe {
                let _ = pjsua_conf_connect(slot, 0);
                let _ = pjsua_conf_connect(0, slot);
            }
        }
    }));
}

// ---------------------------------------------------------------------------
// Backend PJSIP
// ---------------------------------------------------------------------------

pub struct PjsipBackend {
    initialized: bool,
    accounts: Mutex<HashMap<AccountId, pjsua_acc_id>>,
    event_rx: mpsc::UnboundedReceiver<SipEvent>,
}

impl PjsipBackend {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        *lock(&EVENT_TX) = Some(event_tx);
        *lock(account_infos()) = HashMap::new();
        *lock(call_infos()) = HashMap::new();
        *lock(account_ids()) = HashMap::new();
        *lock(ringing_calls()) = HashSet::new();
        *lock(ringtone_preview()) = false;
        *lock(ringtone_player()) = None;
        Self {
            initialized: false,
            accounts: Mutex::new(HashMap::new()),
            event_rx,
        }
    }

    fn configure_account(
        &self,
        id: AccountId,
        username: &str,
        password: &str,
        domain: &str,
        display_name: &str,
        proxy: &str,
    ) -> Result<(), String> {
        if !self.initialized {
            return Err("PJSUA no inicializado".into());
        }

        let domain = domain
            .trim()
            .trim_start_matches("sip://")
            .trim_start_matches("sip:");
        let username = username.trim();
        let password = password.trim();
        let display_name = display_name.trim();
        let proxy = proxy.trim();

        let id_uri = if display_name.is_empty() {
            format!("sip:{username}@{domain}")
        } else {
            let escaped = display_name.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\" <sip:{username}@{domain}>")
        };
        let reg_uri = if domain.starts_with("sip:") {
            domain.to_string()
        } else {
            format!("sip:{domain}")
        };

        let id_c = CString::new(id_uri).map_err(|e| e.to_string())?;
        let reg_c = CString::new(reg_uri).map_err(|e| e.to_string())?;
        let username_c = CString::new(username).map_err(|e| e.to_string())?;
        let password_c = CString::new(password).map_err(|e| e.to_string())?;
        let realm_c = CString::new("*").unwrap();
        let scheme_c = CString::new("digest").unwrap();
        let proxy_c = CString::new(proxy).map_err(|e| e.to_string())?;

        // Si la cuenta ya existe, eliminarla antes de volver a añadirla.
        if let Some(old_acc_id) = self.accounts.lock().unwrap().remove(&id) {
            unsafe {
                let _ = pjsua_acc_del(old_acc_id);
            }
            lock(account_ids()).remove(&old_acc_id);
            lock(account_infos()).remove(&id);
        }

        unsafe {
            let mut acc_cfg: pjsua_acc_config = std::mem::zeroed();
            pjsua_acc_config_default(&mut acc_cfg as *mut _);
            acc_cfg.id = pj_str_from_cstring(&id_c);
            acc_cfg.reg_uri = pj_str_from_cstring(&reg_c);
            acc_cfg.cred_count = 1;
            acc_cfg.cred_info[0] = pjsip_cred_info {
                realm: pj_str_from_cstring(&realm_c),
                scheme: pj_str_from_cstring(&scheme_c),
                username: pj_str_from_cstring(&username_c),
                data_type: PJSIP_CRED_DATA_PLAIN_PASSWD,
                data: pj_str_from_cstring(&password_c),
                algorithm_type: 0,
                ext: std::mem::zeroed(),
            };
            if !proxy.is_empty() {
                acc_cfg.proxy_cnt = 1;
                acc_cfg.proxy[0] = pj_str_from_cstring(&proxy_c);
            }

            let mut acc_id: pjsua_acc_id = 0;
            let st = pjsua_acc_add(&acc_cfg as *const _, 1, &mut acc_id as *mut _);
            if st != PJ_SUCCESS {
                return Err(format!("pjsua_acc_add falló: {st}"));
            }

            self.accounts.lock().unwrap().insert(id.clone(), acc_id);
            lock(account_ids()).insert(acc_id, id.clone());
            lock(account_infos()).insert(
                id.clone(),
                AccountInfo {
                    id: id.clone(),
                    username: username.to_string(),
                    domain: domain.to_string(),
                    display_name: display_name.to_string(),
                    state: RegistrationState::Registering,
                    status_text: "Registering…".into(),
                },
            );
        }

        emit(SipEvent::AccountListChanged);
        emit(SipEvent::RegistrationChanged {
            account: id,
            state: RegistrationState::Registering,
            status_text: "Registering…".into(),
        });
        Ok(())
    }

    fn unregister_account(&self, id: &AccountId) -> Result<(), String> {
        let acc_id = {
            let accounts = self.accounts.lock().unwrap();
            accounts.get(id).copied()
        };
        let Some(acc_id) = acc_id else {
            return Err("Cuenta no encontrada".into());
        };
        unsafe {
            let st = pjsua_acc_set_registration(acc_id, 0);
            if st != PJ_SUCCESS {
                return Err(format!("pjsua_acc_set_registration falló: {st}"));
            }
        }
        Ok(())
    }

    fn delete_account(&self, id: &AccountId) -> Result<(), String> {
        let acc_id = self.accounts.lock().unwrap().remove(id);
        lock(account_ids()).retain(|_, account_id| account_id != id);
        lock(account_infos()).remove(id);
        if let Some(acc_id) = acc_id {
            unsafe {
                let _ = pjsua_acc_del(acc_id);
            }
        }
        emit(SipEvent::AccountListChanged);
        Ok(())
    }

    fn dial(&self, account: AccountId, to: &str) -> Result<(), String> {
        let acc_id = {
            let accounts = self.accounts.lock().unwrap();
            accounts.get(&account).copied()
        };
        let Some(acc_id) = acc_id else {
            return Err("Cuenta no encontrada".into());
        };

        let domain = account.0.split('@').nth(1).unwrap_or("");
        let dst = normalize_destination(to, domain);
        let dst_c = CString::new(dst).map_err(|e| e.to_string())?;
        let dst_pj = pj_str_from_cstring(&dst_c);

        unsafe {
            let mut call_id: pjsua_call_id = 0;
            let st = pjsua_call_make_call(
                acc_id,
                &dst_pj as *const pj_str_t,
                std::ptr::null(),
                std::ptr::null_mut(),
                std::ptr::null(),
                &mut call_id as *mut _,
            );
            if st != PJ_SUCCESS {
                return Err(format!("pjsua_call_make_call falló: {st}"));
            }
        }
        Ok(())
    }

    fn answer(&self, call: &CallId) -> Result<(), String> {
        let call_id = parse_call_id(call)?;
        unsafe {
            let st = pjsua_call_answer(call_id, 200, std::ptr::null(), std::ptr::null());
            if st != PJ_SUCCESS {
                return Err(format!("pjsua_call_answer falló: {st}"));
            }
        }
        stop_ringing_call(call_id);
        Ok(())
    }

    fn reject(&self, call: &CallId) -> Result<(), String> {
        let call_id = parse_call_id(call)?;
        unsafe {
            let st = pjsua_call_hangup(
                call_id,
                PJSIP_SC_BUSY_HERE,
                std::ptr::null(),
                std::ptr::null(),
            );
            if st != PJ_SUCCESS {
                return Err(format!("pjsua_call_hangup falló: {st}"));
            }
        }
        stop_ringing_call(call_id);
        Ok(())
    }

    fn hangup(&self, call: &CallId) -> Result<(), String> {
        let call_id = parse_call_id(call)?;
        unsafe {
            let st = pjsua_call_hangup(call_id, 0, std::ptr::null(), std::ptr::null());
            if st != PJ_SUCCESS {
                return Err(format!("pjsua_call_hangup falló: {st}"));
            }
        }
        stop_ringing_call(call_id);
        Ok(())
    }

    fn send_dtmf(&self, call: &CallId, digit: char) -> Result<(), String> {
        let call_id = parse_call_id(call)?;
        let digits = CString::new(digit.to_string()).map_err(|e| e.to_string())?;
        let digits_pj = pj_str_from_cstring(&digits);
        unsafe {
            let st = pjsua_call_dial_dtmf(call_id, &digits_pj as *const pj_str_t);
            if st != PJ_SUCCESS {
                return Err(format!("pjsua_call_dial_dtmf falló: {st}"));
            }
        }
        Ok(())
    }
}

fn pj_str_from_cstring(c: &CString) -> pj_str_t {
    pj_str_t {
        ptr: c.as_ptr() as *mut c_char,
        slen: c.as_bytes().len() as pj_ssize_t,
    }
}

fn parse_call_id(call: &CallId) -> Result<pjsua_call_id, String> {
    call.0
        .parse::<pjsua_call_id>()
        .map_err(|_| format!("ID de llamada inválido: {}", call.0))
}

fn normalize_destination(to: &str, domain: &str) -> String {
    let to = to.trim();
    if to.starts_with("sip:") || to.starts_with("sips:") {
        return to.to_string();
    }
    if to.contains('@') {
        return format!("sip:{to}");
    }
    if domain.is_empty() {
        format!("sip:{to}")
    } else {
        format!("sip:{to}@{domain}")
    }
}

impl Default for PjsipBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PjsipBackend {
    fn drop(&mut self) {
        *lock(ringtone_preview()) = false;
        stop_ringtone_player();
        if self.initialized {
            unsafe {
                let _ = pjsua_destroy();
            }
            self.initialized = false;
        }
    }
}

impl SipBackend for PjsipBackend {
    fn init(&mut self) -> Result<(), String> {
        unsafe {
            let st = pjsua_create();
            if st != PJ_SUCCESS {
                return Err(format!("pjsua_create falló: {st}"));
            }

            let mut cfg: pjsua_config = std::mem::zeroed();
            pjsua_config_default(&mut cfg as *mut _);
            cfg.thread_cnt = 1;
            cfg.max_calls = 4;
            cfg.cb.on_reg_state = Some(cb_on_reg_state);
            cfg.cb.on_call_state = Some(cb_on_call_state);
            cfg.cb.on_incoming_call = Some(cb_on_incoming_call);
            cfg.cb.on_call_media_state = Some(cb_on_call_media_state);

            let mut log_cfg: pjsua_logging_config = std::mem::zeroed();
            pjsua_logging_config_default(&mut log_cfg as *mut _);
            log_cfg.console_level = 4;

            let mut media_cfg: pjsua_media_config = std::mem::zeroed();
            pjsua_media_config_default(&mut media_cfg as *mut _);

            let st = pjsua_init(
                &cfg as *const _,
                &log_cfg as *const _,
                &media_cfg as *const _,
            );
            if st != PJ_SUCCESS {
                let _ = pjsua_destroy();
                return Err(format!("pjsua_init falló: {st}"));
            }

            let mut transport_cfg: pjsua_transport_config = std::mem::zeroed();
            pjsua_transport_config_default(&mut transport_cfg as *mut _);
            transport_cfg.port = 0;
            let mut transport_id: pjsua_transport_id = 0;
            let st = pjsua_transport_create(
                PJSIP_TRANSPORT_UDP,
                &transport_cfg as *const _,
                &mut transport_id as *mut _,
            );
            if st != PJ_SUCCESS {
                let _ = pjsua_destroy();
                return Err(format!("pjsua_transport_create falló: {st}"));
            }

            let st = pjsua_start();
            if st != PJ_SUCCESS {
                let _ = pjsua_destroy();
                return Err(format!("pjsua_start falló: {st}"));
            }
        }

        self.initialized = true;
        tracing::info!(target: "sip::pjsip", "PJSUA iniciado (UDP, worker thread)");
        Ok(())
    }

    fn event_stream(&mut self) -> mpsc::UnboundedReceiver<SipEvent> {
        std::mem::replace(&mut self.event_rx, mpsc::unbounded_channel().1)
    }

    fn send_command(&self, cmd: SipCommand) {
        let result = match cmd {
            SipCommand::ConfigureAccount {
                id,
                username,
                password,
                domain,
                display_name,
                proxy,
            } => self.configure_account(id, &username, &password, &domain, &display_name, &proxy),
            SipCommand::UnregisterAccount { id } => self.unregister_account(&id),
            SipCommand::DeleteAccount { id } => self.delete_account(&id),
            SipCommand::Dial { account, to } => self.dial(account, &to),
            SipCommand::Answer { call } => self.answer(&call),
            SipCommand::Reject { call } => self.reject(&call),
            SipCommand::Hangup { call } => self.hangup(&call),
            SipCommand::SendDtmf { call, digit } => self.send_dtmf(&call, digit),
        };
        if let Err(message) = result {
            tracing::warn!(target: "sip::pjsip", "comando SIP falló: {message}");
            emit(SipEvent::Log {
                level: tracing::Level::WARN,
                message,
            });
        }
    }

    fn accounts(&self) -> Vec<AccountInfo> {
        lock(account_infos()).values().cloned().collect()
    }

    fn default_account(&self) -> Option<AccountId> {
        lock(account_infos()).keys().next().cloned()
    }

    fn calls(&self) -> Vec<CallInfo> {
        lock(call_infos()).values().cloned().collect()
    }

    fn audio_devices(&self) -> Vec<AudioDeviceInfo> {
        if !self.initialized {
            return Vec::new();
        }
        let mut count: c_uint = 64;
        let mut infos = vec![unsafe { std::mem::zeroed::<pjmedia_aud_dev_info>() }; count as usize];
        let status = unsafe { pjsua_enum_aud_devs(infos.as_mut_ptr(), &mut count) };
        if status != PJ_SUCCESS {
            tracing::warn!(target: "sip::pjsip", "enumerar audio falló: {status}");
            return Vec::new();
        }
        infos.truncate(count as usize);
        tracing::info!(target: "sip::pjsip", "dispositivos de audio enumerados: {count}");
        infos
            .into_iter()
            .map(|info| {
                let device = AudioDeviceInfo {
                    id: info.id,
                    name: fixed_c_string(info.name.as_ptr()),
                    driver: fixed_c_string(info.driver.as_ptr()),
                    input_count: info.input_count,
                    output_count: info.output_count,
                };
                tracing::debug!(
                    target: "sip::pjsip",
                    "audio {}: {} [{}] in={} out={}",
                    device.id,
                    device.name,
                    device.driver,
                    device.input_count,
                    device.output_count
                );
                device
            })
            .collect()
    }

    fn set_audio_devices(&self, capture_id: i32, playback_id: i32) -> Result<(), String> {
        if !self.initialized {
            return Err("PJSUA no inicializado".into());
        }
        let status = unsafe { pjsua_set_snd_dev(capture_id, playback_id) };
        if status == PJ_SUCCESS {
            tracing::info!(
                target: "sip::pjsip",
                "dispositivos de audio: captura={capture_id}, reproducción={playback_id}"
            );
            Ok(())
        } else {
            Err(format!(
                "No se pudieron abrir los dispositivos de audio: {status}"
            ))
        }
    }

    fn set_ringtone(&self, path: Option<std::path::PathBuf>) -> Result<(), String> {
        if let Some(path) = path.as_ref() {
            if !path.is_file() {
                return Err(format!("El timbre no existe: {}", path.display()));
            }
            let is_wav = path
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("wav"))
                .unwrap_or(false);
            if !is_wav {
                return Err("El timbre debe ser un archivo WAV".into());
            }
            validate_ringtone_wav(path)?;
        }
        stop_ringtone_player();
        *lock(ringtone_path()) = path;
        sync_ringtone();
        Ok(())
    }

    fn refresh_ringtone(&self) {
        sync_ringtone();
    }

    fn preview_ringtone(&self) -> Result<(), String> {
        *lock(ringtone_preview()) = true;
        if let Err(err) = start_ringtone_player() {
            *lock(ringtone_preview()) = false;
            return Err(err);
        }
        Ok(())
    }

    fn stop_ringtone(&self) {
        *lock(ringtone_preview()) = false;
        sync_ringtone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_ringtone_is_valid_pcm_mono() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ringtone.wav");
        validate_ringtone_wav(&path).expect("el timbre incluido debe ser WAV PCM 16 bit mono");
    }

    #[test]
    #[ignore = "requiere un dispositivo de audio; reproduce el timbre brevemente"]
    fn pjsip_audio_smoke() {
        let mut backend = PjsipBackend::new();
        backend.init().expect("PJSUA debe iniciar");
        assert!(
            !backend.audio_devices().is_empty(),
            "PJSUA debe enumerar al menos un dispositivo"
        );
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/ringtone.wav");
        backend.set_ringtone(Some(path)).unwrap();
        backend.preview_ringtone().unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        backend.stop_ringtone();
    }
}
