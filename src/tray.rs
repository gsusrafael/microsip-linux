//! Bandeja del sistema (system tray).
//!
//! Usa `tray-icon` (appindicator en GNOME/Ubuntu, KSNI en KDE). Está detrás
//! de la feature `tray` porque requiere `libayatana-appindicator3-dev` en el
//! host. Si se compila con `--no-default-features` la app sigue funcionando
//! sin bandeja.

use std::sync::Arc;
use std::thread;

/// Acciones que el bandeja puede solicitar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ShowWindow,
    Quit,
}

/// Callback que invoca el bandeja cuando el usuario interactúa con el icono.
pub type TrayCallback = Arc<dyn Fn(TrayAction) + Send + Sync>;

/// Manejador de bandeja. El icono debe mantenerse vivo (`_icon`) para que el
/// icono persista en el panel.
pub struct TrayManager {
    #[cfg(feature = "tray")]
    _icon: Option<tray_icon::TrayIcon>,
    cb: Option<TrayCallback>,
}

impl TrayManager {
    /// Crea el icono de bandeja. El clic izquierdo sobre el icono emite
    /// `TrayAction::ShowWindow`.
    pub fn new(cb: TrayCallback) -> Result<Self, String> {
        #[cfg(feature = "tray")]
        {
            use tray_icon::menu::{Menu, PredefinedMenuItem};
            use tray_icon::{TrayEvent, TrayIconBuilder};

            let menu = Menu::new();
            // Un menú vacío es suficiente en Linux para que el icono aparezca.
            let _sep = PredefinedMenuItem::separator();

            let cb_click = cb.clone();
            let _icon = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_tooltip("MicroSIP")
                .build()
                .map_err(|e| format!("no se pudo crear el icono de bandeja: {e}"))?;

            // Hilo de eventos: clic izquierdo sobre el icono -> mostrar ventana.
            thread::spawn(move || {
                let rx = TrayEvent::receiver();
                while let Ok(event) = rx.recv() {
                    use tray_icon::ClickEvent;
                    if event.event == ClickEvent::Left {
                        cb_click(TrayAction::ShowWindow);
                    }
                }
            });

            Ok(Self {
                _icon: Some(_icon),
                cb: Some(cb),
            })
        }

        #[cfg(not(feature = "tray"))]
        {
            Ok(Self {
                cb: Some(cb),
            })
        }
    }

    pub fn callback(&self) -> Option<&TrayCallback> {
        self.cb.as_ref()
    }
}

impl Default for TrayManager {
    fn default() -> Self {
        Self {
            #[cfg(feature = "tray")]
            _icon: None,
            cb: None,
        }
    }
}
