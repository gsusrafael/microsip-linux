//! Punto de entrada de MicroSIP para Linux.
//!
//! Arreglos:
//!   cargo run                  -> GTK4 + bandeja (feature `tray` por defecto)
//!   cargo run --no-default-features -> sin bandeja (útil si faltan dev-libs C)

mod app;
mod main_window;
mod sip;
mod tray;

fn main() {
    // Traces por consola; en el .deb se enviará al journal/syslog.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    // GTK4 application
    let app = app::MicroSipApp::new();
    app.run();
}
