//! Backend SIP real: bindings FFI contra PJSUA (PJSIP, C API).
//!
//! Está detrás de la feature `pjsip`. Requiere pjproject compilado (ver
//! `build.rs` y ~/.local). Compilar pjproject:
//!
//!   ./configure --prefix=$HOME/.local --disable-shared --enable-static \
//!       --disable-sdl --disable-v4l2 --disable-opencore-amr --disable-speex \
//!       --disable-speexdsp --disable-l16 --disable-gsm --disable-speex-aec \
//!       --disable-webrtc --disable-sound --disable-portaudio --disable-alsa \
//!       --disable-resample --disable-ffmpeg --disable-vpx --disable-libtheora \
//!       --disable-libsrtp --disable-openssl --disable-gnutls --disable-bearssl \
//!       --disable-mbedtls --disable-quic --disable-httpd --disable-nginx
//!   make -j$(nproc) && make install
//!
//! Luego: cargo build --features pjsip

use std::collections::HashMap;
use std::os::raw::{c_char, c_int, c_void};
use std::sync::Mutex;
use tokio::sync::mpsc;

use super::{
    AccountId, CallId, CallState, RegistrationState, SipBackend, SipCommand, SipEvent, SipResult,
};

// ---------------------------------------------------------------------------
// Tipos FFI de pjsua (C API)
// ---------------------------------------------------------------------------

/// pjsua_acc_id es un int.
type PjsuaAccId = c_int;
/// pjsua_call_id es un int.
type PjsuaCallId = c_int;
/// pj_status_t es un int (0 = éxito).
type PjStatus = c_int;

const PJ_SUCCESS: PjStatus = 0;

/// pj_str_t: { char *ptr; int slen; }
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PjStr {
    pub ptr: *mut c_char,
    pub slen: c_int,
}

/// pjsua_callbacks (reducido a los callbacks que usamos).
#[repr(C)]
pub struct PjsuaCallbacks {
    pub on_reg_state: Option<unsafe extern "C" fn(PjsuaAccId)>,
    pub on_call_state: Option<unsafe extern "C" fn(PjsuaCallId, *mut c_void)>,
    pub on_incoming_call: Option<unsafe extern "C" fn(PjsuaAccId, PjsuaCallId, *mut c_void)>,
}

/// pjsua_config (campos mínimos, el resto se deja a cero / default).
#[repr(C)]
pub struct PjsuaConfig {
    pub cb: PjsuaCallbacks,
    _pad: [u8; 256], // espacio para los demás campos (versioned, etc.)
}

/// pjsua_logging_config (mínimo).
#[repr(C)]
pub struct PjsuaLoggingConfig {
    _pad: [u8; 128],
}

/// pjsua_media_config (mínimo).
#[repr(C)]
pub struct PjsuaMediaConfig {
    _pad: [u8; 128],
}

/// pjsua_acc_config (campos mínimos).
#[repr(C)]
pub struct PjsuaAccConfig {
    pub id_uri: PjStr,
    pub reg_uri: PjStr,
    _pad: [u8; 512],
}

// ---------------------------------------------------------------------------
// Declaraciones FFI (extern "C")
// ---------------------------------------------------------------------------

extern "C" {
    pub fn pjsua_create() -> PjStatus;
    pub fn pjsua_destroy() -> PjStatus;
    pub fn pjsua_init(
        cfg: *const PjsuaConfig,
        log_cfg: *const PjsuaLoggingConfig,
        media_cfg: *const PjsuaMediaConfig,
    ) -> PjStatus;
    pub fn pjsua_add_acc(
        acc_cfg: *const PjsuaAccConfig,
        set_default: i32,
        acc_id: *mut PjsuaAccId,
    ) -> PjStatus;
    pub fn pjsua_call_make_call(
        acc_id: PjsuaAccId,
        dst_uri: *const PjStr,
        opt: *const c_void,
        user_data: *mut c_void,
        call_id: *mut PjsuaCallId,
    ) -> PjStatus;
    pub fn pjsua_call_answer(call_id: PjsuaCallId, code: c_int, param: *const c_void) -> PjStatus;
    pub fn pjsua_call_hangup_all() -> PjStatus;
    pub fn pjsua_call_hangup(call_id: PjsuaCallId, code: c_int, user_data: *mut c_void) -> PjStatus;
}

// ---------------------------------------------------------------------------
// Helpers para pj_str_t
// ---------------------------------------------------------------------------

fn c_string(s: &str) -> *mut c_char {
    // CString no se puede usar directamente porque pj_str_t no toma ownership;
    // el caller debe mantener la memoria viva mientras pjsua la use.
    // Para simplicidad, usamos una asignación estática (FIXME: fugas de memoria
    // en long-running; aceptable para v1).
    let c = std::ffi::CString::new(s).unwrap();
    c.into_raw()
}

fn pj_str(s: &str) -> PjStr {
    let ptr = c_string(s);
    PjStr {
        ptr,
        slen: s.len() as c_int,
    }
}

// ---------------------------------------------------------------------------
// Canal de eventos backend -> UI
// ---------------------------------------------------------------------------

static EVENT_TX: Mutex<Option<mpsc::UnboundedSender<SipEvent>>> = Mutex::new(None);

// Callbacks estáticos (pjsua llama a estos punteros C)
extern "C" fn cb_on_reg_state(acc_id: PjsuaAccId) {
    tracing::info!(target: "sip::pjsip", "registro cuenta {acc_id} cambió");
    if let Some(guard) = EVENT_TX.lock().ok() {
        if let Some(tx) = guard.as_ref() {
            let _ = tx.send(SipEvent::RegistrationChanged {
                account: AccountId(format!("acc-{acc_id}")),
                state: RegistrationState::Ok, // TODO: leer estado real de pjsua
            });
        }
    }
}

extern "C" fn cb_on_call_state(call_id: PjsuaCallId, _event: *mut c_void) {
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

extern "C" fn cb_on_incoming_call(acc_id: PjsuaAccId, call_id: PjsuaCallId, _rdata: *mut c_void) {
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
    accounts: HashMap<AccountId, PjsuaAccId>,
    calls: HashMap<CallId, PjsuaCallId>,
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

impl SipBackend for PjsipBackend {
    fn init(&mut self) -> Result<(), String> {
        unsafe {
            let st = pjsua_create();
            if st != PJ_SUCCESS {
                return Err(format!("pjsua_create falló: {st}"));
            }

            let cfg = PjsuaConfig {
                cb: PjsuaCallbacks {
                    on_reg_state: Some(cb_on_reg_state),
                    on_call_state: Some(cb_on_call_state),
                    on_incoming_call: Some(cb_on_incoming_call),
                },
                _pad: [0; 256],
            };
            let log_cfg = PjsuaLoggingConfig { _pad: [0; 128] };
            let media_cfg = PjsuaMediaConfig { _pad: [0; 128] };

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
        // UnboundedReceiver no es Clone: se transfiere y se deja uno nuevo.
        std::mem::replace(&mut self.event_rx, mpsc::unbounded_channel().1)
    }

    fn send_command(&self, _cmd: SipCommand) {
        // Los comandos se ejecutan directamente en los métodos de comando públicos
        // (ver abajo). Este trait method se deja para compatibilidad.
        let _ = _cmd;
    }

    fn registration_state(&self, _id: &AccountId) -> RegistrationState {
        RegistrationState::Unknown // TODO: consultar pjsua_acc_get_info
    }

    fn calls(&self) -> Vec<(CallId, CallState, String)> {
        self.calls
            .iter()
            .map(|(id, _call_id)| (id.clone(), CallState::Idle, String::new()))
            .collect()
    }
}

impl PjsipBackend {
    /// Configura una cuenta SIP en PJSUA.
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
            let mut acc_cfg = PjsuaAccConfig {
                id_uri: pj_str(&id_uri),
                reg_uri: pj_str(&reg_uri),
                _pad: [0; 512],
            };
            let mut acc_id: PjsuaAccId = 0;
            let st = pjsua_add_acc(&mut acc_cfg as *mut _, 1, &mut acc_id as *mut _);
            if st != PJ_SUCCESS {
                return SipResult::Error {
                    code: st as u16,
                    message: format!("pjsua_add_acc falló: {st}"),
                };
            }
            self.accounts.insert(id.clone(), acc_id);
        }

        SipResult::Ok
    }

    /// Realiza una llamada SIP.
    pub fn dial(&mut self, account: AccountId, to: &str) -> SipResult {
        let _acc_id = match self.accounts.get(&account) {
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
        let mut pjsua_call_id: PjsuaCallId = 0;

        unsafe {
            let st = pjsua_call_make_call(
                _acc_id,
                &dst_uri as *const PjStr,
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

    /// Contesta una llamada entrante.
    pub fn answer(&self, _call: CallId) -> SipResult {
        SipResult::Ok // TODO: pjsua_call_answer con código 200
    }

    /// Colgar una llamada.
    pub fn hangup(&self, _call: CallId) -> SipResult {
        SipResult::Ok // TODO: pjsua_call_hangup
    }
}

impl Default for PjsipBackend {
    fn default() -> Self {
        Self::new()
    }
}
