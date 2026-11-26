//! Backend SIP real: bindings FFI contra PJSUA (PJSIP, C API).
//!
//! Los structs C vienen de bindgen (`pjsip_bindings.rs`, generado desde
//! `~/.local/include/pjsua-lib/pjsua.h`); las funciones externas se declaran
//! a mano porque bindgen no expande la macro `PJ_DECL`.
//!
//! Requiere pjproject compilado en `~/.local` (ver build.rs).
//!
//! Compilar: cargo build --features pjsip

use std::collections::HashMap;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::Mutex;
use tokio::sync::mpsc;

use super::{
    AccountId, CallId, CallState, RegistrationState, SipBackend, SipCommand, SipEvent, SipResult,
};

// Bindings generados por bindgen (structs con layout exacto de pjproject 2.15).
mod bindings {
    #![allow(unused_imports)]
    #![allow(non_upper_case_globals)]
    #![allow(non_camel_case_types)]
    #![allow(non_snake_case)]
    #![allow(clippy::all)]
    include!("pjsip_bindings.rs");
}

use bindings::{
    pj_status_t, pj_str_t, pjsua_acc_config, pjsua_acc_id, pjsua_call_id, pjsua_call_setting,
    pjsua_config, pjsua_logging_config, pjsua_media_config,
};

// ---------------------------------------------------------------------------
// Declaraciones FFI (extern "C") — bindgen no expande PJ_DECL
// ---------------------------------------------------------------------------

extern "C" {
    fn pjsua_create() -> pj_status_t;
    fn pjsua_init(
        ua_cfg: *const pjsua_config,
        log_cfg: *const pjsua_logging_config,
        media_cfg: *const pjsua_media_config,
    ) -> pj_status_t;
    fn pjsua_destroy() -> pj_status_t;
    fn pjsua_config_default(cfg: *mut pjsua_config);
    fn pjsua_logging_config_default(cfg: *mut pjsua_logging_config);
    fn pjsua_media_config_default(cfg: *mut pjsua_media_config);
    fn pjsua_acc_config_default(cfg: *mut pjsua_acc_config);
    fn pjsua_acc_add(
        acc_cfg: *const pjsua_acc_config,
        set_default: pj_status_t,
        p_acc_id: *mut pjsua_acc_id,
    ) -> pj_status_t;
    fn pjsua_call_make_call(
        acc_id: pjsua_acc_id,
        dst_uri: *const pj_str_t,
        opt: *const pjsua_call_setting,
        user_data: *mut c_void,
        p_call_id: *mut pjsua_call_id,
    ) -> pj_status_t;
}

// pj_status_t es int; PJ_SUCCESS = 0.
const PJ_SUCCESS: i32 = 0;

// ---------------------------------------------------------------------------
// Helpers para pj_str_t
// ---------------------------------------------------------------------------

/// Crea un pj_str_t. La memoria vive mientras el CString no se libera;
/// para pjsua (que copia los strings al init/add_acc) es aceptable en v1.
fn pj_str(s: &str) -> pj_str_t {
    let c = std::ffi::CString::new(s).unwrap();
    let ptr = c.as_ptr() as *mut c_char;
    let len = s.len() as i64;
    // Conservar el CString para no liberarlo (pjsua puede leerlo después).
    // En v1 esto es una fuga intencional por simplicidad; en producción se
    // usaría un pool de pjsua o un Vec<*mut c_char> con ownership.
    std::mem::forget(c);
    pj_str_t { ptr, slen: len }
}

// ---------------------------------------------------------------------------
// Canal de eventos backend -> UI
// ---------------------------------------------------------------------------

static EVENT_TX: Mutex<Option<mpsc::UnboundedSender<SipEvent>>> = Mutex::new(None);

// Callbacks estáticos (pjsua llama a estos punteros C).
extern "C" fn cb_on_reg_state(acc_id: pjsua_acc_id) {
    tracing::info!(target: "sip::pjsip", "registro cuenta {acc_id} cambió");
    if let Some(guard) = EVENT_TX.lock().ok() {
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(SipEvent::RegistrationChanged {
                account: AccountId(format!("acc-{acc_id}")),
                state: RegistrationState::Ok, // TODO: consultar pjsua_acc_get_info
            });
        }
    }
}

extern "C" fn cb_on_call_state(call_id: pjsua_call_id, _event: *mut bindings::pjsip_event) {
    tracing::info!(target: "sip::pjsip", "llamada {call_id} cambió de estado");
    if let Some(guard) = EVENT_TX.lock().ok() {
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(SipEvent::CallStateChanged {
                call: CallId(format!("call-{call_id}")),
                state: CallState::Connected, // TODO: mapear estado real
            });
        }
    }
}

extern "C" fn cb_on_incoming_call(
    acc_id: pjsua_acc_id,
    call_id: pjsua_call_id,
    _rdata: *mut bindings::pjsip_rx_data,
) {
    tracing::info!(target: "sip::pjsip", "llamada entrante {call_id} en cuenta {acc_id}");
    if let Some(guard) = EVENT_TX.lock().ok() {
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(SipEvent::CallIncoming {
                call: CallId(format!("call-{call_id}")),
                from: format!("sip:peer@{acc_id}"), // TODO: extraer From header
                to: format!("sip:local@{acc_id}"),
            });
        }
    }
}

// ---------------------------------------------------------------------------
// Backend PJSIP real
// ---------------------------------------------------------------------------

/// Backend SIP basado en PJSUA (PJSIP).
pub struct PjsipBackend {
    initialized: bool,
    accounts: HashMap<AccountId, pjsua_acc_id>,
    calls: HashMap<CallId, pjsua_call_id>,
    event_tx: mpsc::UnboundedSender<SipEvent>,
    event_rx: mpsc::UnboundedReceiver<SipEvent>,
    next_call: u64,
}

impl PjsipBackend {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        // Registrar el canal global para los callbacks C.
        let _ = EVENT_TX.lock().ok().map(|mut g| g.replace(event_tx.clone()));

        Self {
            initialized: false,
            accounts: HashMap::new(),
            calls: HashMap::new(),
            event_tx,
            event_rx,
            next_call: 0,
        }
    }

    fn next_call_id(&mut self) -> CallId {
        self.next_call += 1;
        CallId(format!("call-{}", self.next_call))
    }
}

impl Default for PjsipBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl SipBackend for PjsipBackend {
    fn init(&mut self) -> Result<(), String> {
        unsafe {
            let st = pjsua_create();
            if st != PJ_SUCCESS {
                return Err(format!("pjsua_create falló: {st}"));
            }

            // Configurar con valores por defecto de pjproject (layout exacto).
            let mut cfg: pjsua_config = std::mem::zeroed();
            pjsua_config_default(&mut cfg as *mut _);
            cfg.thread_cnt = 0; // sin worker threads, usaremos poll si hace falta
            cfg.max_calls = 8;

            // Callbacks de la aplicación.
            cfg.cb.on_reg_state = Some(cb_on_reg_state);
            cfg.cb.on_call_state = Some(cb_on_call_state);
            cfg.cb.on_incoming_call = Some(cb_on_incoming_call);

            let mut log_cfg: pjsua_logging_config = std::mem::zeroed();
            pjsua_logging_config_default(&mut log_cfg as *mut _);
            log_cfg.console_level = 4;

            let mut media_cfg: pjsua_media_config = std::mem::zeroed();
            pjsua_media_config_default(&mut media_cfg as *mut _);

            let st = pjsua_init(&cfg as *const _, &log_cfg as *const _, &media_cfg as *const _);
            if st != PJ_SUCCESS {
                let _ = pjsua_destroy();
                return Err(format!("pjsua_init falló: {st}"));
            }
        }
        self.initialized = true;
        tracing::info!(target: "sip::pjsip", "PJSUA inicializado");
        Ok(())
    }

    fn event_stream(&mut self) -> mpsc::UnboundedReceiver<SipEvent> {
        std::mem::replace(&mut self.event_rx, mpsc::unbounded_channel().1)
    }

    fn send_command(&self, _cmd: SipCommand) {
        let _ = _cmd;
    }

    fn registration_state(&self, _id: &AccountId) -> RegistrationState {
        RegistrationState::Unknown // TODO: pjsua_acc_get_info
    }

    fn calls(&self) -> Vec<(CallId, CallState, String)> {
        self.calls
            .iter()
            .map(|(id, _call_id)| (id.clone(), CallState::Idle, String::new()))
            .collect()
    }
}

impl PjsipBackend {
    /// Configura una cuenta SIP en PJSUA (con valores por defecto de pjproject).
    pub fn configure_account(&mut self, id: AccountId, username: &str, domain: &str) -> SipResult {
        if !self.initialized {
            return SipResult::Error {
                code: 400,
                message: "PJSUA no inicializado".into(),
            };
        }

        let id_uri = format!("sip:{username}@{domain}");
        let reg_uri = format!("sip:{domain}");

        unsafe {
            let mut acc_cfg: pjsua_acc_config = std::mem::zeroed();
            pjsua_acc_config_default(&mut acc_cfg as *mut _);
            acc_cfg.id = pj_str(&id_uri);
            acc_cfg.reg_uri = pj_str(&reg_uri);

            let mut acc_id: pjsua_acc_id = 0;
            let st = pjsua_acc_add(&acc_cfg as *const _, 1, &mut acc_id as *mut _);
            if st != PJ_SUCCESS {
                return SipResult::Error {
                    code: st as u16,
                    message: format!("pjsua_acc_add falló: {st}"),
                };
            }
            self.accounts.insert(id.clone(), acc_id);
        }

        SipResult::Ok
    }

    /// Realiza una llamada SIP.
    pub fn dial(&mut self, account: AccountId, to: &str) -> SipResult {
        let acc_id = match self.accounts.get(&account) {
            Some(a) => *a,
            None => {
                return SipResult::Error {
                    code: 404,
                    message: "Cuenta no encontrada".into(),
                };
            }
        };

        let dst_uri = pj_str(to);
        let call_id = self.next_call_id();
        let mut pjsua_call_id: pjsua_call_id = 0;

        unsafe {
            let st = pjsua_call_make_call(
                acc_id,
                &dst_uri as *const pj_str_t,
                std::ptr::null(),
                std::ptr::null_mut(),
                &mut pjsua_call_id as *mut _,
            );
            if st != PJ_SUCCESS {
                return SipResult::Error {
                    code: st as u16,
                    message: format!("pjsua_call_make_call falló: {st}"),
                };
            }
        }

        self.calls.insert(call_id.clone(), pjsua_call_id);
        SipResult::Ok
    }

    /// Contesta una llamada entrante (TODO: pjsua_call_answer).
    pub fn answer(&self, _call: CallId) -> SipResult {
        SipResult::Ok
    }

    /// Colgar una llamada (TODO: pjsua_call_hangup).
    pub fn hangup(&self, _call: CallId) -> SipResult {
        SipResult::Ok
    }
}
