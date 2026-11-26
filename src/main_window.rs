//! Ventana principal de MicroSIP (GTK4).
//!
//! Páginas del stack: cuentas, marcador y llamadas activas.

use crate::sip::MockBackend;
use gtk4::prelude::*;
use gtk4::{
    Application, ApplicationWindow, Box, Button, Entry, HeaderBar, Label, Orientation, Stack,
};
use glib::signal::Inhibit;
use std::cell::RefCell;
use std::rc::Rc;
use tracing::{info, warn};

pub struct MainWindow {
    window: ApplicationWindow,
    stack: Stack,
    sip: Rc<RefCell<MockBackend>>,
}

impl MainWindow {
    pub fn new(app: &Application, sip: Rc<RefCell<MockBackend>>) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("MicroSIP")
            .default_width(700)
            .default_height(500)
            .build();

        let stack = Stack::new();

        // Barra de título
        let header = HeaderBar::new();
        header.set_title_widget(Some(&Label::new(Some("MicroSIP"))));
        let menu_btn = Button::with_label("☰");
        header.pack_end(&menu_btn);

        window.set_titlebar(Some(&header));

        // Páginas
        let accounts_page = accounts_page(Rc::clone(&sip));
        stack.add_titled(&accounts_page, Some("cuentas"), "Cuentas");

        let dialer_page = dialer_page(Rc::clone(&sip));
        stack.add_titled(&dialer_page, Some("marcador"), "Marcador");

        let calls_page = calls_page(Rc::clone(&sip));
        stack.add_titled(&calls_page, Some("llamadas"), "Llamadas");

        let content = Box::new(Orientation::Vertical, 0);
        content.append(&stack);
        window.set_child(Some(&content));

        // Minimizar a bandeja: al pedir cierre, ocultar la ventana en lugar de
        // destruirla. Cuando se inyecte el TrayManager real desde app.rs, aquí
        // se consultará si hay bandeja; por ahora siempre se oculta.
        let window_clone = window.clone();
        window.connect_close_request(move |_win| {
            info!(target: "ui", "close-request: ventana oculta (no se destruye)");
            window_clone.set_visible(false);
            Inhibit(true)
        });

        Self {
            window,
            stack,
            sip,
        }
    }

    pub fn window(&self) -> &ApplicationWindow {
        &self.window
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn show_page(&self, name: &str) {
        self.stack.set_visible_child_name(name);
    }
}

/// Construye la ventana y la devuelve (sin presentarla aún).
pub fn build(app: &Application) -> ApplicationWindow {
    let sip = Rc::new(RefCell::new(crate::sip::MockBackend::new()));
    let win = MainWindow::new(app, sip);
    win.window
}

// -------- Páginas --------

fn accounts_page(_sip: Rc<RefCell<MockBackend>>) -> Box {
    let box_ = Box::new(Orientation::Vertical, 12);
    box_.set_margin_top(24);
    box_.set_margin_start(24);
    box_.set_margin_end(24);
    box_.set_margin_bottom(24);

    let title = Label::new(Some("Cuentas SIP"));
    title.set_halign(gtk4::Align::Start);
    title.add_css_class("title-1");
    box_.append(&title);

    let list = gtk4::ListBox::new();
    list.set_selection_mode(gtk4::SelectionMode::None);

    // Cuenta de ejemplo (mock)
    let row = gtk4::ListBoxRow::new();
    let row_box = Box::new(Orientation::Horizontal, 12);
    row_box.set_margin_top(6);
    row_box.set_margin_bottom(6);
    row_box.set_margin_start(12);
    row_box.set_margin_end(12);

    let avatar = Label::new(Some("D"));
    avatar.add_css_class("caps-label");
    let info = Box::new(Orientation::Vertical, 2);
    let name = Label::new(Some("demo@example.com"));
    name.set_halign(gtk4::Align::Start);
    name.add_css_class("dim-label");
    let status = Label::new(Some("Registrado"));
    status.set_halign(gtk4::Align::Start);
    status.add_css_class("success-label");
    info.append(&name);
    info.append(&status);
    row_box.append(&avatar);
    row_box.append(&info);
    row.set_child(Some(&row_box));
    list.append(&row);

    box_.append(&list);
    box_
}

fn dialer_page(_sip: Rc<RefCell<MockBackend>>) -> Box {
    let box_ = Box::new(Orientation::Vertical, 12);
    box_.set_margin_top(24);
    box_.set_margin_start(24);
    box_.set_margin_end(24);
    box_.set_margin_bottom(24);

    let entry = Entry::new();
    entry.set_placeholder_text(Some("Número o URI SIP"));
    entry.add_css_class("dialer-entry");
    box_.append(&entry);

    let grid = gtk4::Grid::new();
    grid.set_row_spacing(8);
    grid.set_column_spacing(8);
    grid.set_halign(gtk4::Align::Center);
    grid.set_valign(gtk4::Align::Center);

    let labels = [
        ["1", "2", "3"],
        ["4", "5", "6"],
        ["7", "8", "9"],
        ["*", "0", "#"],
    ];
    for (r, row) in labels.iter().enumerate() {
        for (c, label) in row.iter().enumerate() {
            let btn = Button::with_label(label);
            btn.set_hexpand(true);
            btn.set_vexpand(true);
            btn.set_width_request(64);
            btn.set_height_request(64);
            let entry_clone = entry.clone();
            let label = label.to_string();
            btn.connect_clicked(move |_| {
                let cur = entry_clone.text();
                entry_clone.set_text(&format!("{cur}{label}"));
            });
            grid.attach(&btn, c as i32, r as i32, 1, 1);
        }
    }

    let call_btn = Button::with_label("Llamar");
    call_btn.add_css_class("suggested-action");
    call_btn.set_hexpand(true);
    call_btn.set_margin_top(16);
    let entry_clone = entry.clone();
    call_btn.connect_clicked(move |_| {
        let to = entry_clone.text();
        info!(target: "dialer", "llamando a {to}");
    });
    box_.append(&grid);
    box_.append(&call_btn);

    box_
}

fn calls_page(_sip: Rc<RefCell<MockBackend>>) -> Box {
    let box_ = Box::new(Orientation::Vertical, 12);
    box_.set_margin_top(24);
    box_.set_margin_start(24);
    box_.set_margin_end(24);
    box_.set_margin_bottom(24);

    let title = Label::new(Some("Llamadas activas"));
    title.set_halign(gtk4::Align::Start);
    title.add_css_class("title-1");
    box_.append(&title);

    let empty = Label::new(Some("Sin llamadas activas."));
    empty.set_halign(gtk4::Align::Start);
    empty.add_css_class("dim-label");
    box_.append(&empty);

    warn!(target: "ui", "página de llamadas (mock): sin llamadas");
    box_
}
