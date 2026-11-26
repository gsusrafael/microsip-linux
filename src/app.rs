//! Aplicación MicroSIP: una única instancia GTK4, un único backend SIP,
//! una única bandeja y un único puente de eventos hacia el hilo GTK4.

use crate::{config, main_window, sip, tray};
use gtk4::prelude::*;
use gtk4::Application;
use sip::SipBackend;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use tracing::{info, warn};

use tray::TrayAction;

const APP_ID: &str = "org.microsip.linux";

/// Alinea tema y esquema de color (claro/oscuro) con los ajustes del escritorio.
///
/// GTK3 los toma de `~/.config/gtk-3.0/settings.ini`, pero GTK4 **no lee ese
/// fichero**: si el escritorio solo lo tiene para GTK3, la app GTK4 se queda en
/// el esquema por defecto. Aquí leemos directamente GSettings
/// (`org.gnome.desktop.interface`) y lo aplicamos a `GtkSettings`, de forma
/// que la app respeta el mismo esquema que el resto del escritorio **sin**
/// tocar la configuración del operador.
fn apply_system_color_scheme() {
    // libadwaita aporta la paleta GTK4 moderna (`accent_bg_color`,
    // `window_bg_color`, …). Sin ella, un stylesheet que solo define los
    // nombres antiguos (`bg_color`, `accent_color`) deja los botones sin
    // color de acento: la app sale en escala de grises.
    if let Err(err) = libadwaita::init() {
        warn!(target: "theme", "no se pudo inicializar libadwaita: {err}");
    }

    let Some(gtk_settings) = gtk4::Settings::default() else {
        warn!(target: "theme", "GtkSettings no disponible; se usa el tema por defecto");
        return;
    };

    // `Settings::new` aborta si el schema no existe → comprobarlo antes.
    let Some(schema_source) = gtk4::gio::SettingsSchemaSource::default() else {
        return;
    };
    if schema_source
        .lookup("org.gnome.desktop.interface", true)
        .is_none()
    {
        return;
    }
    let desktop = gtk4::gio::Settings::new("org.gnome.desktop.interface");

    let theme = desktop.string("gtk-theme");
    // Forzar `gtk-theme-name` es OPTATIVO y por defecto está desactivado:
    // el `gtk-4.0/gtk.css` de elementary no define fondo para
    // `button.suggested-action` (0 reglas), así que al cargarlo GTK4 pierde su
    // Adwaita embebido y los botones salen grises. Sin forzar, GTK4 usa su
    // stylesheet por defecto + libadwaita → acentos correctos.
    // `MICROSIP_FORCE_THEME_NAME=1` lo reintenta para experimentación.
    if !theme.is_empty() && std::env::var_os("MICROSIP_FORCE_THEME_NAME").is_some() {
        gtk_settings.set_property("gtk-theme-name", theme.as_str());
    }

    // libadwaita ignora `gtk-application-prefer-dark-theme` (lo avisa como
    // no soportado): el oscuro/claro se pide a `AdwStyleManager`.
    let scheme = match desktop.string("color-scheme").as_str() {
        "prefer-dark" => libadwaita::ColorScheme::PreferDark,
        "prefer-light" => libadwaita::ColorScheme::PreferLight,
        _ => libadwaita::ColorScheme::Default,
    };
    libadwaita::StyleManager::default().set_color_scheme(scheme);
    let prefer_dark = scheme == libadwaita::ColorScheme::PreferDark;

    info!(
        target: "theme",
        "esquema del escritorio aplicado: tema=\"{}\" oscuro={}",
        theme,
        if prefer_dark { "sí" } else { "no" }
    );
}

pub struct MicroSipApp {
    app: Application,
    sip: Rc<RefCell<Box<dyn sip::SipBackend>>>,
    events: Rc<RefCell<Option<sip::SipEventStream>>>,
    window: Rc<RefCell<Option<main_window::MainWindow>>>,
}

impl MicroSipApp {
    pub fn new() -> Self {
        let app = Application::builder().application_id(APP_ID).build();

        let (backend, events) = match sip::create_backend() {
            Ok(pair) => pair,
            Err(err) => {
                warn!(target: "sip", "no se pudo iniciar el backend real ({err}); usando mock");
                let mut mock = sip::MockBackend::new();
                let _ = mock.init();
                let events = mock.event_stream();
                (Box::new(mock) as Box<dyn sip::SipBackend>, events)
            }
        };
        let sip = Rc::new(RefCell::new(backend));

        // Cargar cuentas persistidas y registrarlas en el backend.
        for account in config::load() {
            info!(target: "config", "cargando cuenta {}", account.id);
            sip.borrow()
                .send_command(sip::SipCommand::ConfigureAccount {
                    id: sip::AccountId(account.id.clone()),
                    username: account.username,
                    password: account.password,
                    domain: account.domain,
                    display_name: account.display_name,
                    proxy: account.proxy,
                });
        }

        Self {
            app,
            sip,
            events: Rc::new(RefCell::new(Some(events))),
            window: Rc::new(RefCell::new(None)),
        }
    }

    pub fn run(&self) {
        let app = &self.app;
        let sip = Rc::clone(&self.sip);
        let events = Rc::clone(&self.events);
        let window = Rc::clone(&self.window);

        // Bandeja -> hilo GTK4. La bandeja solo captura el Sender (Send).
        let (tray_tx, tray_rx) =
            glib::MainContext::channel::<TrayAction>(glib::Priority::default());
        let app_for_tray = app.clone();
        let window_for_tray = Rc::clone(&window);
        tray_rx.attach(None, move |action| {
            match action {
                TrayAction::ShowWindow => {
                    if let Some(win) = window_for_tray.borrow().as_ref() {
                        win.present();
                    }
                }
                TrayAction::Quit => {
                    info!(target: "tray", "saliendo desde la bandeja");
                    app_for_tray.quit();
                }
            }
            glib::Continue(true)
        });

        let tray_cell = Rc::new(RefCell::new(None::<tray::TrayManager>));
        let tray_cell_activate = Rc::clone(&tray_cell);

        app.connect_startup(|_| {
            // GTK ya está inicializado aquí, pero aún no hay ventanas:
            // es el momento de fijar tema/esquema del escritorio.
            apply_system_color_scheme();
        });

        app.connect_activate(move |app| {
            // Crear la bandeja una sola vez, después de inicializar GTK.
            if tray_cell_activate.borrow().is_none() {
                let tray_tx_for_cb = tray_tx.clone();
                let tray_cb = Arc::new(move |action: TrayAction| {
                    let _ = tray_tx_for_cb.send(action);
                });
                let created = tray::TrayManager::new(tray_cb).unwrap_or_else(|err| {
                    warn!(target: "tray", "bandeja no disponible: {err}");
                    tray::TrayManager::default()
                });
                *tray_cell_activate.borrow_mut() = Some(created);
            }

            // Activación repetida (segundo clic en el .desktop): reutilizar ventana.
            if let Some(existing) = window.borrow().as_ref() {
                existing.present();
                return;
            }

            let win = main_window::build(app, Rc::clone(&sip));

            #[cfg(feature = "test-hooks")]
            win.test_dial_if_configured();

            // Puente de eventos SIP -> hilo GTK4 (solo una vez).
            if let Some(mut stream) = events.borrow_mut().take() {
                let ui = win.clone();
                glib::MainContext::default().spawn_local(async move {
                    while let Some(event) = stream.recv().await {
                        ui.handle_event(&event);
                    }
                });
            }

            *window.borrow_mut() = Some(win.clone());
            win.present();
        });

        info!(target: "microsip", "aplicación iniciada ({APP_ID})");
        app.run();

        // Mantener viva la bandeja hasta que termine el main loop.
        let _ = tray_cell;
    }
}
