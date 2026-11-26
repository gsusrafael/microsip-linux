//! Capa de red SIP.
//!
//! La implementación real de MicroSIP usa PJSIP (C). Aquí definimos un
//! `SipBackend` trait para poder desarrollar la UI en Rust sin depender de
//! librerías C, y dejamos un backend `mock` listo para desarrollar sin red.
//! Cuando la feature `pjsip` está activa, se compila además el backend real
//! vía FFI contra pjproject (ver `src/sip/pjsip.rs`).

#[cfg(feature = "pjsip")]
mod pjsip;

#[cfg(feature = "pjsip")]
pub use pjsip::PjsipBackend;

use std::collections::HashMap;
use tokio::sync::mpsc;

/// Identificador de una cuenta SIP configurada.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccountId(pub String);

/// Identificador de una llamada activa.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CallId(pub String);

/// Estado de registro de una cuenta.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegistrationState {
    Unknown,
    Registering,
    Ok,
    Failed,
    Expired,
}

/// Estado de una llamada.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallState {
    Idle,
    Outgoing,
    Incoming,
    Early,       // ringing far end
    Connected,
    Disconnecting,
    Disconnected,
    Failed,
}

/// Evento que emite el backend SIP hacia la UI.
#[derive(Debug, Clone)]
pub enum SipEvent {
    RegistrationChanged { account: AccountId, state: RegistrationState },
    CallStateChanged { call: CallId, state: CallState },
    CallIncoming { call: CallId, from: String, to: String },
    Log { level: tracing::Level, message: String },
}

/// Comando que la UI envía al backend SIP.
#[derive(Debug)]
pub enum SipCommand {
    ConfigureAccount {
        id: AccountId,
        username: String,
        password: String,
        domain: String,
        display_name: String,
    },
    UnregisterAccount { id: AccountId },
    Dial { account: AccountId, to: String },
    Answer { call: CallId },
    Reject { call: CallId },
    Hangup { call: CallId },
    SendDtmf { call: CallId, digit: char },
}

/// Resultado de un comando SIP.
#[derive(Debug)]
pub enum SipResult {
    Ok,
    Error { code: u16, message: String },
}

/// Canal de comandos UI -> backend.
pub type SipCommandSender = mpsc::UnboundedSender<SipCommand>;
/// Canal de eventos backend -> UI.
pub type SipEventStream = mpsc::UnboundedReceiver<SipEvent>;

/// Backend SIP abstracto. Permite intercambiar mock <-> PJSIP sin tocar la UI.
pub trait SipBackend: Send + Sync + 'static {
    fn init(&mut self) -> Result<(), String>;
    fn event_stream(&mut self) -> SipEventStream;
    fn send_command(&self, cmd: SipCommand);
    fn registration_state(&self, id: &AccountId) -> RegistrationState;
    fn calls(&self) -> Vec<(CallId, CallState, String)>;
}

// ---------------------------------------------------------------------------
// Backend mock (desarrollo / demostración sin PJSIP)
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct MockBackend {
    /// Comandos UI -> backend (no usados por el mock v1, listos para FFI).
    _cmd_tx: SipCommandSender,
    /// Eventos backend -> UI.
    event_tx: mpsc::UnboundedSender<SipEvent>,
    rx: mpsc::UnboundedReceiver<SipEvent>,
    accounts: HashMap<AccountId, RegistrationState>,
    calls: HashMap<CallId, (CallState, String)>,
    next_call: u64,
}

impl MockBackend {
    pub fn new() -> Self {
        let (cmd_tx, _cmd_rx) = mpsc::unbounded_channel::<SipCommand>();
        let (event_tx, rx) = mpsc::unbounded_channel::<SipEvent>();
        Self {
            _cmd_tx: cmd_tx,
            event_tx,
            rx,
            accounts: HashMap::new(),
            calls: HashMap::new(),
            next_call: 0,
        }
    }

    fn next_call_id(&mut self) -> CallId {
        self.next_call += 1;
        CallId(format!("call-{}", self.next_call))
    }
}

impl SipBackend for MockBackend {
    fn init(&mut self) -> Result<(), String> {
        tracing::info!(target: "sip::mock", "backend mock inicializado");
        Ok(())
    }

    fn event_stream(&mut self) -> SipEventStream {
        // UnboundedReceiver no es Clone: se transfiere y se deja uno nuevo
        // (un solo consumidor: la UI).
        std::mem::replace(&mut self.rx, mpsc::unbounded_channel().1)
    }

    fn send_command(&self, _cmd: SipCommand) {
        // El mock se impulsa directamente con simulate_*; en FFI este canal
        // se conectará al endpoint PJSIP.
        let _ = (_cmd,);
    }

    fn registration_state(&self, id: &AccountId) -> RegistrationState {
        self.accounts.get(id).copied().unwrap_or(RegistrationState::Unknown)
    }

    fn calls(&self) -> Vec<(CallId, CallState, String)> {
        self.calls
            .iter()
            .map(|(id, (state, peer))| (id.clone(), *state, peer.clone()))
            .collect()
    }
}

impl MockBackend {
    /// Simula el registro OK de una cuenta (pruebas de UI).
    pub fn simulate_registered(&mut self, id: AccountId) {
        self.accounts.insert(id.clone(), RegistrationState::Ok);
        let _ = self.event_tx.send(SipEvent::RegistrationChanged {
            account: id,
            state: RegistrationState::Ok,
        });
    }

    /// Simula una llamada entrante.
    pub fn simulate_incoming(&mut self, from: &str, to: &str) -> CallId {
        let id = self.next_call_id();
        self.calls.insert(id.clone(), (CallState::Incoming, from.to_string()));
        let _ = self.event_tx.send(SipEvent::CallIncoming {
            call: id.clone(),
            from: from.to_string(),
            to: to.to_string(),
        });
        id
    }
}
