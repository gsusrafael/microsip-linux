//! Ventana principal de MicroSIP (GTK4).
//!
//! Recibe el backend SIP como parámetro: nunca crea otro backend. Los eventos
//! SIP llegan desde `app.rs` mediante `handle_event`.

use crate::config::{self, AccountCfg};
use crate::sip::{
    AccountId, AccountInfo, CallInfo, CallState, RegistrationState, SipBackend, SipCommand,
    SipEvent,
};
use glib::signal::Inhibit;
use gtk4::prelude::*;
use gtk4::Box as GtkBox;
use gtk4::{
    Align, Application, ApplicationWindow, Button, Entry, HeaderBar, Label, ListBox, ListBoxRow,
    Orientation, Stack,
};
use std::cell::RefCell;
use std::rc::Rc;
use tracing::info;

#[derive(Clone)]
pub struct MainWindow {
    app: Application,
    window: ApplicationWindow,
    stack: Stack,
    status: Label,
    accounts_list: ListBox,
    calls_list: ListBox,
    sip: Rc<RefCell<Box<dyn SipBackend>>>,
}

impl MainWindow {
    pub fn new(app: &Application, sip: Rc<RefCell<Box<dyn SipBackend>>>) -> Self {
        let window = ApplicationWindow::builder()
            .application(app)
            .title("MicroSIP")
            .default_width(760)
            .default_height(560)
            .build();

        let stack = Stack::new();
        let status = Label::new(Some("Listo"));
        status.add_css_class("dim-label");
        status.set_xalign(0.0);

        let accounts_list = ListBox::new();
        accounts_list.set_selection_mode(gtk4::SelectionMode::None);
        let calls_list = ListBox::new();
        calls_list.set_selection_mode(gtk4::SelectionMode::None);

        let accounts_page = accounts_page(&accounts_list, Rc::clone(&sip));
        stack.add_titled(&accounts_page, Some("cuentas"), "Cuentas");

        let dialer_page = dialer_page(Rc::clone(&sip), status.clone());
        stack.add_titled(&dialer_page, Some("marcador"), "Marcador");

        let calls_page = calls_page(&calls_list, Rc::clone(&sip));
        stack.add_titled(&calls_page, Some("llamadas"), "Llamadas");

        let header = HeaderBar::new();
        header.set_title_widget(Some(&Label::new(Some("MicroSIP"))));
        header.pack_start(&status);

        let account_btn = Button::with_label("＋ Cuenta");
        account_btn.add_css_class("suggested-action");
        header.pack_end(&account_btn);

        let quit_btn = Button::with_label("Salir");
        header.pack_end(&quit_btn);
        window.set_titlebar(Some(&header));

        let content = GtkBox::new(Orientation::Vertical, 0);
        content.append(&stack);
        window.set_child(Some(&content));

        let this = Self {
            app: app.clone(),
            window,
            stack,
            status,
            accounts_list,
            calls_list,
            sip,
        };

        let account_win = this.clone();
        account_btn.connect_clicked(move |_| account_win.show_account_dialog());

        let app_quit = this.app.clone();
        quit_btn.connect_clicked(move |_| app_quit.quit());

        // Minimizar a bandeja: ocultar en lugar de destruir.
        let hide_win = this.window.clone();
        this.window.connect_close_request(move |_win| {
            info!(target: "ui", "close-request: ventana oculta (minimizar a bandeja)");
            hide_win.set_visible(false);
            Inhibit(true)
        });

        this.refresh_accounts();
        this.refresh_calls();
        this
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn show_page(&self, name: &str) {
        self.stack.set_visible_child_name(name);
    }

    /// Hook de prueba (feature `test-hooks`): si `MICROSIP_TEST_DIAL` está
    /// definido, marca ese destino con la cuenta por defecto al arrancar.
    #[cfg(feature = "test-hooks")]
    pub fn test_dial_if_configured(&self) {
        let Ok(to) = std::env::var("MICROSIP_TEST_DIAL") else {
            return;
        };
        let backend = self.sip.borrow();
        let Some(account) = backend.default_account() else {
            self.status
                .set_text("test-hooks: no hay cuenta para marcar");
            return;
        };
        info!(target: "test", "auto-maracando {to}");
        backend.send_command(SipCommand::Dial { account, to });
        self.status
            .set_text("test-hooks: llamada automática enviada");
    }

    pub fn handle_event(&self, event: &SipEvent) {
        match event {
            SipEvent::RegistrationChanged {
                state, status_text, ..
            } => {
                self.status
                    .set_text(&format!("{} · {status_text}", registration_label(*state)));
                self.refresh_accounts();
            }
            SipEvent::CallStateChanged { state, .. } => {
                if *state == CallState::Early || *state == CallState::Incoming {
                    self.show_page("llamadas");
                }
                self.refresh_calls();
            }
            SipEvent::CallIncoming { call, .. } => {
                #[cfg(feature = "test-hooks")]
                if std::env::var("MICROSIP_TEST_AUTO_ANSWER").is_ok() {
                    info!(target: "test", "auto-contestando {:?}", call);
                    self.sip
                        .borrow()
                        .send_command(SipCommand::Answer { call: call.clone() });
                }
                self.show_page("llamadas");
                self.refresh_calls();
            }
            SipEvent::AccountListChanged => self.refresh_accounts(),
            SipEvent::CallListChanged => self.refresh_calls(),
            SipEvent::Log { message, .. } => {
                self.status.set_text(message);
            }
        }
    }

    fn refresh_accounts(&self) {
        clear_list(&self.accounts_list);
        let accounts = self.sip.borrow().accounts();
        if accounts.is_empty() {
            let row = simple_row("No hay cuentas. Pulsa «＋ Cuenta» para añadir una.");
            self.accounts_list.append(&row);
            return;
        }
        for account in accounts {
            let row = account_row(&account, self.clone());
            self.accounts_list.append(&row);
        }
    }

    fn refresh_calls(&self) {
        clear_list(&self.calls_list);
        let calls = self.sip.borrow().calls();
        if calls.is_empty() {
            let row = simple_row("Sin llamadas activas.");
            self.calls_list.append(&row);
            return;
        }
        for call in calls {
            let row = call_row(&call, Rc::clone(&self.sip));
            self.calls_list.append(&row);
        }
    }

    fn show_account_dialog(&self) {
        self.show_account_dialog_for(None);
    }

    fn show_account_dialog_for(&self, existing: Option<AccountCfg>) {
        let editing = existing.is_some();
        let dialog = gtk4::Window::builder()
            .title(if editing {
                "Editar cuenta SIP"
            } else {
                "Nueva cuenta SIP"
            })
            .default_width(440)
            .default_height(360)
            .modal(true)
            .build();
        dialog.set_transient_for(Some(&self.window));

        let grid = gtk4::Grid::new();
        grid.set_row_spacing(8);
        grid.set_column_spacing(12);
        grid.set_margin_top(18);
        grid.set_margin_start(18);
        grid.set_margin_end(18);
        grid.set_margin_bottom(18);

        let user_entry = Entry::new();
        user_entry.set_placeholder_text(Some("Usuario"));
        let pass_entry = Entry::new();
        pass_entry.set_visibility(false);
        pass_entry.set_placeholder_text(Some("Contraseña"));
        let domain_entry = Entry::new();
        domain_entry.set_placeholder_text(Some("Dominio o proxy (ej. pbx.local)"));
        let display_entry = Entry::new();
        display_entry.set_placeholder_text(Some("Nombre visible (opcional)"));
        let proxy_entry = Entry::new();
        proxy_entry.set_placeholder_text(Some("Proxy SIP (opcional)"));

        if let Some(account) = existing.as_ref() {
            user_entry.set_text(&account.username);
            pass_entry.set_text(&account.password);
            domain_entry.set_text(&account.domain);
            display_entry.set_text(&account.display_name);
            proxy_entry.set_text(&account.proxy);
        }

        let fields = [
            ("Usuario", &user_entry),
            ("Contraseña", &pass_entry),
            ("Dominio", &domain_entry),
            ("Nombre visible", &display_entry),
            ("Proxy", &proxy_entry),
        ];
        for (row, (label_text, entry)) in fields.iter().enumerate() {
            let label = Label::new(Some(label_text));
            label.set_xalign(0.0);
            entry.set_hexpand(true);
            grid.attach(&label, 0, row as i32, 1, 1);
            grid.attach(*entry, 1, row as i32, 1, 1);
        }

        let button_box = GtkBox::new(Orientation::Horizontal, 8);
        button_box.set_halign(Align::End);
        let cancel = Button::with_label("Cancelar");
        let save = Button::with_label("Guardar");
        save.add_css_class("suggested-action");
        button_box.append(&cancel);
        button_box.append(&save);
        grid.attach(&button_box, 0, fields.len() as i32, 2, 1);

        let dialog_close = dialog.clone();
        cancel.connect_clicked(move |_| dialog_close.close());

        let dialog_save = dialog.clone();
        let status = self.status.clone();
        let sip = Rc::clone(&self.sip);
        let original_id = existing.as_ref().map(|account| account.id.clone());
        save.connect_clicked(move |_| {
            let username = user_entry.text().trim().to_string();
            let domain = domain_entry.text().trim().to_string();
            if username.is_empty() || domain.is_empty() {
                status.set_text("Usuario y dominio son obligatorios");
                return;
            }
            let id = format!("{username}@{domain}");
            if let Some(old_id) = original_id.as_ref() {
                if *old_id != id {
                    let _ = config::remove(old_id);
                    sip.borrow().send_command(SipCommand::DeleteAccount {
                        id: AccountId(old_id.clone()),
                    });
                }
            }
            let account = AccountCfg {
                id,
                username,
                password: pass_entry.text().to_string(),
                domain,
                display_name: display_entry.text().trim().to_string(),
                proxy: proxy_entry.text().trim().to_string(),
            };
            if let Err(err) = config::upsert(account.clone()) {
                status.set_text(&format!("No se pudo guardar: {err}"));
                return;
            }
            sip.borrow().send_command(SipCommand::ConfigureAccount {
                id: AccountId(account.id.clone()),
                username: account.username,
                password: account.password,
                domain: account.domain,
                display_name: account.display_name,
                proxy: account.proxy,
            });
            status.set_text("Cuenta guardada");
            dialog_save.close();
        });

        dialog.set_child(Some(&grid));
        dialog.present();
    }
}

/// Construye la ventana y la devuelve (sin presentarla aún).
pub fn build(app: &Application, sip: Rc<RefCell<Box<dyn SipBackend>>>) -> MainWindow {
    MainWindow::new(app, sip)
}

// ---------------------------------------------------------------------------
// Páginas
// ---------------------------------------------------------------------------

fn accounts_page(list: &ListBox, sip: Rc<RefCell<Box<dyn SipBackend>>>) -> GtkBox {
    let box_ = GtkBox::new(Orientation::Vertical, 12);
    box_.set_margin_top(24);
    box_.set_margin_start(24);
    box_.set_margin_end(24);
    box_.set_margin_bottom(24);

    let title = Label::new(Some("Cuentas SIP"));
    title.set_halign(Align::Start);
    title.add_css_class("title-1");
    box_.append(&title);
    box_.append(list);
    let _ = sip;
    box_
}

fn dialer_page(sip: Rc<RefCell<Box<dyn SipBackend>>>, status: Label) -> GtkBox {
    let box_ = GtkBox::new(Orientation::Vertical, 12);
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
    grid.set_halign(Align::Center);
    grid.set_valign(Align::Center);

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
    let sip_clone = Rc::clone(&sip);
    let status_clone = status.clone();
    call_btn.connect_clicked(move |_| {
        let to = entry_clone.text().trim().to_string();
        if to.is_empty() {
            status_clone.set_text("Introduce un número o URI SIP");
            return;
        }
        let backend = sip_clone.borrow();
        let Some(account) = backend.default_account() else {
            status_clone.set_text("Configura una cuenta SIP primero");
            return;
        };
        backend.send_command(SipCommand::Dial { account, to });
        status_clone.set_text("Llamada iniciada…");
    });

    box_.append(&grid);
    box_.append(&call_btn);
    box_
}

fn calls_page(list: &ListBox, _sip: Rc<RefCell<Box<dyn SipBackend>>>) -> GtkBox {
    let box_ = GtkBox::new(Orientation::Vertical, 12);
    box_.set_margin_top(24);
    box_.set_margin_start(24);
    box_.set_margin_end(24);
    box_.set_margin_bottom(24);

    let title = Label::new(Some("Llamadas activas"));
    title.set_halign(Align::Start);
    title.add_css_class("title-1");
    box_.append(&title);
    box_.append(list);
    box_
}

// ---------------------------------------------------------------------------
// Filas
// ---------------------------------------------------------------------------

fn simple_row(text: &str) -> ListBoxRow {
    let row = ListBoxRow::new();
    let label = Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_margin_top(8);
    label.set_margin_bottom(8);
    label.set_margin_start(12);
    label.set_margin_end(12);
    row.set_child(Some(&label));
    row
}

fn account_row(account: &AccountInfo, owner: MainWindow) -> ListBoxRow {
    let row = ListBoxRow::new();
    let row_box = GtkBox::new(Orientation::Horizontal, 12);
    row_box.set_margin_top(6);
    row_box.set_margin_bottom(6);
    row_box.set_margin_start(12);
    row_box.set_margin_end(12);

    let first = account
        .display_name
        .chars()
        .next()
        .or_else(|| account.username.chars().next())
        .unwrap_or('?');
    let avatar = Label::new(Some(&first.to_uppercase().to_string()));
    avatar.add_css_class("caps-label");

    let info = GtkBox::new(Orientation::Vertical, 2);
    let name = Label::new(Some(&account.id.0));
    name.set_halign(Align::Start);
    let state = Label::new(Some(&format!(
        "{} · {}",
        registration_label(account.state),
        account.status_text
    )));
    state.set_halign(Align::Start);
    state.add_css_class(match account.state {
        RegistrationState::Ok => "success-label",
        RegistrationState::Failed => "error-label",
        _ => "dim-label",
    });
    info.append(&name);
    info.append(&state);

    let edit = Button::with_label("Editar");
    let owner_edit = owner.clone();
    let id = account.id.clone();
    edit.connect_clicked(move |_| {
        let stored = config::load().into_iter().find(|item| item.id == id.0);
        owner_edit.show_account_dialog_for(stored);
    });

    let unregister = Button::with_label("Desregistrar");
    let sip_clone = Rc::clone(&owner.sip);
    let id = account.id.clone();
    unregister.connect_clicked(move |_| {
        sip_clone
            .borrow()
            .send_command(SipCommand::UnregisterAccount { id: id.clone() });
    });

    let delete = Button::with_label("Eliminar");
    delete.add_css_class("destructive-action");
    let owner_delete = owner.clone();
    let id = account.id.clone();
    delete.connect_clicked(move |_| {
        let _ = config::remove(&id.0);
        owner_delete
            .sip
            .borrow()
            .send_command(SipCommand::DeleteAccount {
                id: AccountId(id.0.clone()),
            });
        owner_delete.status.set_text("Cuenta eliminada");
    });

    row_box.append(&avatar);
    row_box.append(&info);
    row_box.append(&edit);
    row_box.append(&unregister);
    row_box.append(&delete);
    row.set_child(Some(&row_box));
    row
}

fn call_row(call: &CallInfo, sip: Rc<RefCell<Box<dyn SipBackend>>>) -> ListBoxRow {
    let row = ListBoxRow::new();
    let row_box = GtkBox::new(Orientation::Horizontal, 12);
    row_box.set_margin_top(6);
    row_box.set_margin_bottom(6);
    row_box.set_margin_start(12);
    row_box.set_margin_end(12);

    let info = GtkBox::new(Orientation::Vertical, 2);
    let peer = Label::new(Some(&call.peer));
    peer.set_halign(Align::Start);
    let state = Label::new(Some(call_label(call.state)));
    state.set_halign(Align::Start);
    state.add_css_class("dim-label");
    info.append(&peer);
    info.append(&state);
    row_box.append(&info);

    let can_answer = call.incoming && matches!(call.state, CallState::Incoming | CallState::Early);
    let active = !matches!(
        call.state,
        CallState::Disconnected | CallState::Failed | CallState::Idle
    );

    if can_answer {
        let answer = Button::with_label("Contestar");
        answer.add_css_class("suggested-action");
        let sip_clone = Rc::clone(&sip);
        let id = call.id.clone();
        answer.connect_clicked(move |_| {
            sip_clone
                .borrow()
                .send_command(SipCommand::Answer { call: id.clone() });
        });
        row_box.append(&answer);

        let reject = Button::with_label("Rechazar");
        reject.add_css_class("destructive-action");
        let sip_clone = Rc::clone(&sip);
        let id = call.id.clone();
        reject.connect_clicked(move |_| {
            sip_clone
                .borrow()
                .send_command(SipCommand::Reject { call: id.clone() });
        });
        row_box.append(&reject);
    } else if active {
        let hangup = Button::with_label("Colgar");
        hangup.add_css_class("destructive-action");
        let sip_clone = Rc::clone(&sip);
        let id = call.id.clone();
        hangup.connect_clicked(move |_| {
            sip_clone
                .borrow()
                .send_command(SipCommand::Hangup { call: id.clone() });
        });
        row_box.append(&hangup);

        if call.state == CallState::Connected {
            let dtmf = Entry::new();
            dtmf.set_placeholder_text(Some("DTMF"));
            dtmf.set_width_request(70);
            let dtmf_for_send = dtmf.clone();
            let send = Button::with_label("Enviar");
            let sip_clone = Rc::clone(&sip);
            let id = call.id.clone();
            send.connect_clicked(move |_| {
                let text = dtmf_for_send.text();
                if let Some(digit) = text.chars().next() {
                    sip_clone.borrow().send_command(SipCommand::SendDtmf {
                        call: id.clone(),
                        digit,
                    });
                    dtmf_for_send.set_text("");
                }
            });
            row_box.append(&dtmf);
            row_box.append(&send);
        }
    }

    row.set_child(Some(&row_box));
    row
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn clear_list(list: &ListBox) {
    while let Some(child) = list.first_child() {
        list.remove(&child);
    }
}

fn registration_label(state: RegistrationState) -> &'static str {
    match state {
        RegistrationState::Unknown => "Sin registrar",
        RegistrationState::Registering => "Registrando",
        RegistrationState::Ok => "Registrado",
        RegistrationState::Failed => "Error de registro",
        RegistrationState::Expired => "Expirado",
    }
}

fn call_label(state: CallState) -> &'static str {
    match state {
        CallState::Idle => "Inactiva",
        CallState::Outgoing => "Llamando…",
        CallState::Incoming => "Entrante",
        CallState::Early => "Timbrando…",
        CallState::Connected => "Conectada",
        CallState::Disconnecting => "Colgando…",
        CallState::Disconnected => "Finalizada",
        CallState::Failed => "Fallida",
    }
}
