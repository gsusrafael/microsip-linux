//! Capa de red SIP.
//!
//! Define un backend abstracto para poder alternar entre un mock de
//! desarrollo y PJSIP real sin tocar la UI. La fábrica `create_backend()`
//! garantiza una sola instancia y un único consumidor de eventos.

#[cfg(feature = "pjsip")]
mod pjsip;

use std::collections::HashMap;
use std::sync::Mutex;
use tokio::sync::mpsc;

/// Identificador estable de una cuenta SIP.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AccountId(pub String);

/// Identificador de una llamada.
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
    Early,
    Connected,
    Disconnecting,
    Disconnected,
    Failed,
}

/// Información de cuenta mostrada por la UI.
#[derive(Debug, Clone)]
pub struct AccountInfo {
    pub id: AccountId,
    pub username: String,
    pub domain: String,
    pub display_name: String,
    pub state: RegistrationState,
    pub status_text: String,
}

/// Información de llamada mostrada por la UI.
#[derive(Debug, Clone)]
pub struct CallInfo {
    pub id: CallId,
    pub peer: String,
    pub state: CallState,
    pub incoming: bool,
}

/// Dispositivo de audio expuesto por PJSUA/ALSA.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    pub id: i32,
    pub name: String,
    pub driver: String,
    pub input_count: u32,
    pub output_count: u32,
}

impl AudioDeviceInfo {
    /// Clave estable para persistir la selección: el ID numérico puede cambiar
    /// al reiniciar o al enchufar otro dispositivo.
    pub fn key(&self) -> String {
        if self.driver.is_empty() {
            self.name.clone()
        } else {
            format!("{}\n{}", self.driver, self.name)
        }
    }
}

/// Evento emitido por el backend hacia la UI.
#[derive(Debug, Clone)]
pub enum SipEvent {
    RegistrationChanged {
        account: AccountId,
        state: RegistrationState,
        status_text: String,
    },
    CallStateChanged {
        call: CallId,
        state: CallState,
        peer: String,
        incoming: bool,
    },
    CallIncoming {
        call: CallId,
        from: String,
        to: String,
    },
    AccountListChanged,
    CallListChanged,
    Log {
        level: tracing::Level,
        message: String,
    },
}

/// Comando enviado por la UI al backend.
#[derive(Debug)]
pub enum SipCommand {
    ConfigureAccount {
        id: AccountId,
        username: String,
        password: String,
        domain: String,
        display_name: String,
        proxy: String,
    },
    UnregisterAccount {
        id: AccountId,
    },
    DeleteAccount {
        id: AccountId,
    },
    Dial {
        account: AccountId,
        to: String,
    },
    Answer {
        call: CallId,
    },
    Reject {
        call: CallId,
    },
    Hangup {
        call: CallId,
    },
    SendDtmf {
        call: CallId,
        digit: char,
    },
}

pub const DEFAULT_CAPTURE_DEVICE: i32 = -1;
pub const DEFAULT_PLAYBACK_DEVICE: i32 = -2;

pub type SipEventStream = mpsc::UnboundedReceiver<SipEvent>;

/// Backend SIP abstracto. Solo vive en el hilo GTK4; los callbacks de PJSIP
/// envían eventos por canal, no comparten el objeto.
pub trait SipBackend: 'static {
    fn init(&mut self) -> Result<(), String>;
    fn event_stream(&mut self) -> SipEventStream;
    fn send_command(&self, cmd: SipCommand);
    fn accounts(&self) -> Vec<AccountInfo>;
    fn default_account(&self) -> Option<AccountId>;
    fn calls(&self) -> Vec<CallInfo>;
    fn audio_devices(&self) -> Vec<AudioDeviceInfo>;
    fn set_audio_devices(&self, capture_id: i32, playback_id: i32) -> Result<(), String>;
    fn set_ringtone(&self, path: Option<std::path::PathBuf>) -> Result<(), String>;
    fn refresh_ringtone(&self);
    fn preview_ringtone(&self) -> Result<(), String>;
    fn stop_ringtone(&self);
}

/// Fábrica única: crea el backend activo, lo inicializa y entrega el stream
/// exactamente una vez.
pub fn create_backend() -> Result<(Box<dyn SipBackend>, SipEventStream), String> {
    #[cfg(feature = "pjsip")]
    let mut backend: Box<dyn SipBackend> = Box::new(pjsip::PjsipBackend::new());
    #[cfg(not(feature = "pjsip"))]
    let mut backend: Box<dyn SipBackend> = Box::new(MockBackend::new());

    backend.init()?;
    let events = backend.event_stream();
    Ok((backend, events))
}

// ---------------------------------------------------------------------------
// Backend mock
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct MockState {
    accounts: HashMap<AccountId, AccountInfo>,
    calls: HashMap<CallId, CallInfo>,
    next_call: u64,
}

pub struct MockBackend {
    event_tx: mpsc::UnboundedSender<SipEvent>,
    event_rx: mpsc::UnboundedReceiver<SipEvent>,
    state: Mutex<MockState>,
}

impl MockBackend {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Self {
            event_tx,
            event_rx,
            state: Mutex::new(MockState::default()),
        }
    }

    fn emit(&self, event: SipEvent) {
        let _ = self.event_tx.send(event);
    }

    fn configure(&self, cmd: SipCommand) {
        if let SipCommand::ConfigureAccount {
            id,
            username,
            password: _,
            domain,
            display_name,
            proxy: _,
        } = cmd
        {
            let mut state = self.state.lock().unwrap();
            state.accounts.insert(
                id.clone(),
                AccountInfo {
                    id: id.clone(),
                    username,
                    domain,
                    display_name,
                    state: RegistrationState::Registering,
                    status_text: "Registering…".into(),
                },
            );
            drop(state);

            self.emit(SipEvent::AccountListChanged);
            self.emit(SipEvent::RegistrationChanged {
                account: id.clone(),
                state: RegistrationState::Ok,
                status_text: "Registered".into(),
            });

            let mut state = self.state.lock().unwrap();
            if let Some(account) = state.accounts.get_mut(&id) {
                account.state = RegistrationState::Ok;
                account.status_text = "Registered".into();
            }
            self.emit(SipEvent::AccountListChanged);
        }
    }

    fn dial(&self, account: AccountId, to: String) {
        let mut state = self.state.lock().unwrap();
        state.next_call += 1;
        let call = CallId(format!("mock-{}", state.next_call));
        state.calls.insert(
            call.clone(),
            CallInfo {
                id: call.clone(),
                peer: to.clone(),
                state: CallState::Outgoing,
                incoming: false,
            },
        );
        drop(state);

        self.emit(SipEvent::CallListChanged);
        self.emit(SipEvent::CallStateChanged {
            call: call.clone(),
            state: CallState::Early,
            peer: to.clone(),
            incoming: false,
        });

        let mut state = self.state.lock().unwrap();
        if let Some(info) = state.calls.get_mut(&call) {
            info.state = CallState::Connected;
        }
        drop(state);
        self.emit(SipEvent::CallStateChanged {
            call,
            state: CallState::Connected,
            peer: to,
            incoming: false,
        });
        let _ = account;
    }

    fn set_call_state(&self, call: &CallId, state: CallState) {
        let mut state_guard = self.state.lock().unwrap();
        let Some(info) = state_guard.calls.get_mut(call) else {
            return;
        };
        info.state = state;
        let peer = info.peer.clone();
        let incoming = info.incoming;
        drop(state_guard);
        self.emit(SipEvent::CallStateChanged {
            call: call.clone(),
            state,
            peer,
            incoming,
        });
        self.emit(SipEvent::CallListChanged);
    }
}

impl SipBackend for MockBackend {
    fn init(&mut self) -> Result<(), String> {
        tracing::info!(target: "sip::mock", "backend mock inicializado");
        Ok(())
    }

    fn event_stream(&mut self) -> SipEventStream {
        std::mem::replace(&mut self.event_rx, mpsc::unbounded_channel().1)
    }

    fn send_command(&self, cmd: SipCommand) {
        match cmd {
            SipCommand::ConfigureAccount { .. } => self.configure(cmd),
            SipCommand::UnregisterAccount { id } => {
                let mut state = self.state.lock().unwrap();
                if let Some(account) = state.accounts.get_mut(&id) {
                    account.state = RegistrationState::Unknown;
                    account.status_text = "Unregistered".into();
                }
                drop(state);
                self.emit(SipEvent::AccountListChanged);
            }
            SipCommand::DeleteAccount { id } => {
                self.state.lock().unwrap().accounts.remove(&id);
                self.emit(SipEvent::AccountListChanged);
            }
            SipCommand::Dial { account, to } => self.dial(account, to),
            SipCommand::Answer { call } => self.set_call_state(&call, CallState::Connected),
            SipCommand::Reject { call } | SipCommand::Hangup { call } => {
                self.set_call_state(&call, CallState::Disconnected)
            }
            SipCommand::SendDtmf { .. } => {}
        }
    }

    fn accounts(&self) -> Vec<AccountInfo> {
        self.state
            .lock()
            .unwrap()
            .accounts
            .values()
            .cloned()
            .collect()
    }

    fn default_account(&self) -> Option<AccountId> {
        self.state.lock().unwrap().accounts.keys().next().cloned()
    }

    fn calls(&self) -> Vec<CallInfo> {
        self.state.lock().unwrap().calls.values().cloned().collect()
    }

    fn audio_devices(&self) -> Vec<AudioDeviceInfo> {
        Vec::new()
    }

    fn set_audio_devices(&self, _capture_id: i32, _playback_id: i32) -> Result<(), String> {
        Ok(())
    }

    fn set_ringtone(&self, _path: Option<std::path::PathBuf>) -> Result<(), String> {
        Ok(())
    }

    fn refresh_ringtone(&self) {}

    fn preview_ringtone(&self) -> Result<(), String> {
        Ok(())
    }

    fn stop_ringtone(&self) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mock_command_emits_registration() {
        let mut backend = MockBackend::new();
        let mut rx = backend.event_stream();
        backend.send_command(SipCommand::ConfigureAccount {
            id: AccountId("a@localhost".into()),
            username: "a".into(),
            password: "x".into(),
            domain: "localhost".into(),
            display_name: "A".into(),
            proxy: String::new(),
        });
        let mut saw_ok = false;
        while let Ok(event) = rx.try_recv() {
            if let SipEvent::RegistrationChanged {
                state: RegistrationState::Ok,
                ..
            } = event
            {
                saw_ok = true;
            }
        }
        assert!(saw_ok);
    }
}
