//! Aplicación MicroSIP: aplicación GTK4, ventana principal, backend SIP y bandeja.

use crate::{main_window, sip, tray};
use gtk4::prelude::*;
use gtk4::{Application, ApplicationWindow};
use glib::MainContext;
use std::rc::Rc;
use std::cell::RefCell;
use std::sync::Arc;
use tracing::{info, warn};

const APP_ID: &str = "org.microsip.linux";

pub struct MicroSipApp {
    app: Application,
    #[cfg(feature = "pjsip")]
    sip: sip::PjsipBackend,
    #[cfg(not(feature = "pjsip"))]
    sip: sip::MockBackend,
    tray: tray::TrayManager,
}

impl MicroSipApp {
    pub fn new() -> Self {
        let app = Application::builder()
            .application_id(APP_ID)
            .build();

        #[cfg(feature = "pjsip")]
        {
            use crate::sip::SipBackend;
            let mut backend = sip::PjsipBackend::new();
            if let Err(e) = backend.init() {
                warn!(target: "sip::pjsip", "no se pudo inicializar PJSUA: {e}");
            }
            // Cuenta de ejemplo (demo) — configurar contra un servidor SIP real.
            // Descomentar y ajustar para registrar una cuenta real:
            // backend.configure_account(
            //     sip::AccountId("demo@example.com".into()), "demo", "sip.example.com");
            let tray = tray::TrayManager::default();
            Self { app, sip: backend, tray }
        }

        #[cfg(not(feature = "pjsip"))]
        {
            let mut backend = sip::MockBackend::new();
            // Cuenta de ejemplo (mock) para que la UI tenga algo que mostrar.
            let demo = sip::AccountId("demo@example.com".into());
            backend.simulate_registered(demo.clone());

            let app_weak = app.downgrade();
            let tray_cb = Arc::new(move |action| match action {
                tray::TrayAction::ShowWindow => {
                    if let Some(app) = app_weak.upgrade() {
                        info!(target: "tray", "acción: mostrar ventana");
                    }
                }
                tray::TrayAction::Quit => {
                    info!(target: "tray", "acción: salir");
                    std::process::exit(0);
                }
            });

            let tray = tray::TrayManager::new(tray_cb).unwrap_or_else(|e| {
                warn!(target: "tray", "bandeja no disponible: {e}");
                tray::TrayManager::default()
            });

            Self { app, sip: backend, tray }
        }
    }

    pub fn run(&self) {
        let app = &self.app;

        // Celda compartida (solo hilo principal) para guardar la ventana y
        // poder mostrarla desde el hilo de bandeja vía MainContext::channel.
        let window_cell = Rc::new(RefCell::new(None::<ApplicationWindow>));
        let window_cell_activate = window_cell.clone();

        let (show_tx, show_rx) = MainContext::channel(glib::Priority::default());
        let window_cell_show = window_cell.clone();
        show_rx.attach(None, move |_| {
            if let Some(win) = window_cell_show.borrow().as_ref() {
                win.present();
            }
            glib::Continue(true)
        });

        let tray_cb = Arc::new(move |action| match action {
            tray::TrayAction::ShowWindow => {
                let _ = show_tx.send(());
            }
            tray::TrayAction::Quit => {
                info!(target: "tray", "salir desde bandeja");
                std::process::exit(0);
            }
        });

        let tray = tray::TrayManager::new(tray_cb).unwrap_or_else(|e| {
            warn!(target: "tray", "bandeja no disponible: {e}");
            tray::TrayManager::default()
        });

        app.connect_activate(move |app| {
            let win = main_window::build(app);
            *window_cell_activate.borrow_mut() = Some(win.clone());
            win.present();
        });

        info!(target: "microsip", "aplicación iniciada ({APP_ID})");
        app.run();
        let _ = tray;
    }
}
