# MicroSIP para Linux

Port de [MicroSIP](https://www.microsip.org/) (softphone SIP basado en PJSIP) a
Linux, escrito en **Rust + GTK4**, con soporte de **bandeja del sistema** y
empaquetado como **.deb** para Debian/Ubuntu.

> **Estado:** v0.1.0 — PJSIP FFI **funcional**. Los structs C se generan con
> **bindgen** desde `~/.local/include/pjsua-lib/pjsua.h` (layout exacto de
> pjproject 2.15). `pjsua_init` se ejecuta correctamente: PJSUA pasa a estado
> `CREATED` y registra todos los módulos SIP (mod-ua, mod-invite, etc.).
> La UI (cuentas, marcador, llamadas, bandeja) funciona con el backend
> seleccionable: **mock** (por defecto, sin dependencias) o **PJSIP real**
> (`--features pjsip`).

---

## Estructura del proyecto

```
microsip-linux/
├── Cargo.toml            # manifiesto + metadata cargo-deb (.deb)
├── build.rs              # enlaza estáticamente pjproject (feature pjsip)
├── .cargo/config.toml    # rutas de ~/.local (sin sudo)
├── src/
│   ├── main.rs           # punto de entrada
│   ├── app.rs            # aplicación GTK4 + selección de backend SIP
│   ├── main_window.rs    # ventana principal (stack: cuentas/marcador/llamadas)
│   ├── tray.rs           # bandeja del sistema (tray-icon / appindicator)
│   └── sip/
│       ├── mod.rs        # trait SipBackend + backend mock
│       └── pjsip.rs      # bindings FFI contra PJSUA (feature pjsip)
├── assets/
│   ├── microsip-linux.desktop
│   └── icon.svg
└── debian/
    ├── control
    └── changelog
```

---

## Requisitos del sistema

### Para ejecutar (runtime)
- `libgtk-4-0`
- `libayatana-appindicator3-1` (bandeja del sistema)
- `libglib2.0-0`

### Para compilar
- `rustc` + `cargo` (1.70+)
- `libgtk-4-dev`, `libayatana-appindicator3-dev`, `libglib2.0-dev`, `pkg-config`

> En tu máquina no hay `sudo` disponible, así que las dev-libs se bajaron con
> `apt download` y se extrajeron a `~/.local/deps/`; `.cargo/config.toml` ya
> apunta a ese prefijo.

---

## Compilar y ejecutar

### 1. Con backend mock (por defecto, funcional inmediatamente)

```bash
cd /home/jesus/Workspace/microsip-linux
cargo build --release          # compila sin PJSIP
./target/release/microsip-linux
```

La app abre una ventana GTK4 con:
- **Cuentas**: cuenta de ejemplo `demo@example.com` (registrado, mock)
- **Marcador**: teclado numérico y botón "Llamar"
- **Llamadas**: lista de llamadas activas
- **Bandeja**: al cerrar la ventana, se oculta a la bandeja del sistema
  (clic en el icono para mostrarla de nuevo)

### 2. Con PJSIP real vía FFI

pjproject 2.15 ya está compilado en `/home/jesus/.local` (ver abajo).
Compilar con la feature `pjsip`:

```bash
cargo build --release --features pjsip
./target/release/microsip-linux
```

> **PJSIP FFI funciona:** los structs C vienen de `bindgen` (layout exacto de
> pjproject 2.15) y las funciones externas se declaran a mano. `pjsua_init`
> ejecuta correctamente: PJSUA pasa a `CREATED` y registra todos los módulos
> SIP. El mock sigue siendo el backend por defecto (sin feature `pjsip`).

---

## Empaquetado .deb (Debian/Ubuntu)

```bash
cargo install cargo-deb        # si no está instalado
cargo deb                      # genera target/debian/microsip-linux_0.1.0-1_amd64.deb
```

Instalar:

```bash
sudo apt install ./target/debian/microsip-linux_0.1.0-1_amd64.deb
```

El `.deb` incluye:
- `/usr/bin/microsip-linux`
- `/usr/share/applications/microsip-linux.desktop`
- `/usr/share/icons/hicolor/scalable/apps/microsip-linux.svg`

---

## Compilar pjproject (PJSIP) para Linux

Si quieres trabajar el backend FFI real, pjproject 2.15 ya está compilado en
`/home/jesus/.local` (librerías estáticas + headers + pkg-config). Para
recompilarlo desde cero:

```bash
cd /home/jesus/.local/src/pjsip/pjproject
./configure --prefix=$HOME/.local --disable-shared --enable-static \
    --disable-sdl --disable-v4l2 --disable-opencore-amr --disable-speex \
    --disable-speexdsp --disable-l16 --disable-gsm --disable-speex-aec \
    --disable-webrtc --disable-sound --disable-portaudio --disable-alsa \
    --disable-resample --disable-ffmpeg --disable-vpx --disable-libtheora \
    --disable-libsrtp --disable-openssl --disable-gnutls --disable-bearssl \
    --disable-mbedtls --disable-quic --disable-httpd --disable-nginx
make -j$(nproc) && make install
```

Luego compilar MicroSIP con `--features pjsip`.

---

## Cómo probar

| Qué probar | Comando | Qué debería ver |
|---|---|---|
| UI se abre | `cargo run --release` | Ventana "MicroSIP" con 3 pestañas |
| Cuenta mock registrada | `cargo run --release` → pestaña "Cuentas" | `demo@example.com` · "Registrado" |
| Marcador numérico | `cargo run --release` → "Marcador" | Teclado 0-9, *, #, botón "Llamar" |
| Bandeja del sistema | `cargo run --release` → cerrar ventana | Icono en bandeja; clic para mostrar |
| Compilar .deb | `cargo deb` | `target/debian/*.deb` |
| PJSIP vinculado | `cargo build --features pjsip` | Compila sin errores de enlace |
| PJSIP inicializa | `cargo build --features pjsip && ./target/debug/microsip-linux` | Log: `PJSUA state changed: NULL --> CREATED`, módulos SIP registrados |

Para probar sin servidor gráfico (entorno headless/CI):

```bash
xvfb-run -a cargo run --release   # necesita Xvfb
```

---

## Roadmap (v1 → v1.1)

1. Conectar comandos SIP (Dial/Answer/Hangup) desde la UI al backend real.
2. Audio: integrar `pjmedia` con ALSA/PulseAudio para voz real.
3. Cuentas persistentes (archivo de config) en vez de cuenta mock hardcodeada.
4. Mejorar la gestión de memoria de `pj_str_t` (pool de pjsua en vez de
   `CString::forget`).

---

## Licencia

GPL-2.0-only (misma licencia que MicroSIP original, basado en PJSIP).
