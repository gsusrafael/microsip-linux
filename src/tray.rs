//! Bandeja del sistema (system tray).
//!
//! Implementa StatusNotifierItem (SNI) con `ksni` sobre D-Bus. A diferencia
//! de `tray-icon`, no usa GTK3, por lo que no entra en conflicto con la GTK4
//! de la aplicación. Está detrás de la feature `tray`.

use std::sync::Arc;

/// Acciones que el bandeja puede solicitar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    ShowWindow,
    Quit,
}

/// Callback que invoca el bandeja cuando el usuario interactúa con el icono.
pub type TrayCallback = Arc<dyn Fn(TrayAction) + Send + Sync>;

/// Manejador de bandeja. El servicio SNI debe mantenerse vivo.
pub struct TrayManager {
    #[cfg(feature = "tray")]
    _handle: Option<ksni::blocking::Handle<SniTray>>,
}

#[cfg(feature = "tray")]
pub struct SniTray {
    cb: TrayCallback,
}

#[cfg(feature = "tray")]
impl ksni::Tray for SniTray {
    fn id(&self) -> String {
        "microsip-linux".into()
    }

    fn title(&self) -> String {
        "MicroSIP".into()
    }

    fn icon_name(&self) -> String {
        "phone".into()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        (self.cb)(TrayAction::ShowWindow);
    }

    fn menu(&self) -> Vec<ksni::menu::MenuItem<Self>> {
        let show_cb = Arc::clone(&self.cb);
        let show = ksni::menu::StandardItem {
            label: "Mostrar MicroSIP".into(),
            activate: Box::new(move |_this: &mut Self| show_cb(TrayAction::ShowWindow)),
            ..Default::default()
        };

        let quit_cb = Arc::clone(&self.cb);
        let quit = ksni::menu::StandardItem {
            label: "Salir".into(),
            activate: Box::new(move |_this: &mut Self| quit_cb(TrayAction::Quit)),
            ..Default::default()
        };

        vec![show.into(), quit.into()]
    }
}

impl TrayManager {
    /// Crea el icono de bandeja (SNI). Clic izquierdo o menú -> mostrar /
    /// salir.
    pub fn new(cb: TrayCallback) -> Result<Self, String> {
        #[cfg(feature = "tray")]
        {
            use ksni::blocking::TrayMethods;

            let tray = SniTray {
                cb: Arc::clone(&cb),
            };
            let handle = tray
                .assume_sni_available(true)
                .spawn()
                .map_err(|e| format!("no se pudo crear el icono de bandeja: {e}"))?;

            Ok(Self {
                _handle: Some(handle),
            })
        }

        #[cfg(not(feature = "tray"))]
        {
            Ok(Self {})
        }
    }
}

impl Default for TrayManager {
    fn default() -> Self {
        Self {
            #[cfg(feature = "tray")]
            _handle: None,
        }
    }
}
