# MicroSIP para Linux

Port de [MicroSIP](https://www.microsip.org/) (softphone SIP basado en PJSIP) a
Linux, escrito en **Rust + GTK4**, con soporte de **bandeja del sistema** y
empaquetado como **.deb** para Debian/Ubuntu.

> **Estado:** v0.1.1 — UI navegable (Cuentas / Marcador / Llamadas / Sonido),
> selección de micrófono, altavoz y timbre, backend PJSIP real y paquetes
> `.deb` multi-arquitectura.
> Los structs C se generan con **bindgen** desde
> `~/.local/include/pjsua-lib/pjsua.h` (layout exacto de pjproject 2.15).
> La UI GTK4 usa **un único backend SIP** (mock o PJSIP real con
> `--features pjsip`), con cuentas persistentes en
> `~/.config/microsip-linux/accounts.conf` (permisos 600), llamadas
> entrantes/salientes, contestar/rechazar/colgar, DTMF y eventos SIP
> enlazados al hilo GTK4. PJSUA se inicia con transporte UDP y worker thread;
> el audio usa el backend ALSA de pjmedia. La bandeja usa **ksni**
> (StatusNotifierItem/D-Bus, sin GTK3).

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
│   ├── config.rs         # cuentas persistentes (~/.config/microsip-linux)
│   ├── main_window.rs    # ventana principal (stack: cuentas/marcador/llamadas)
│   ├── tray.rs           # bandeja del sistema (ksni / StatusNotifierItem D-Bus)
│   └── sip/
│       ├── mod.rs        # trait SipBackend + backend mock
│       └── pjsip.rs      # bindings FFI contra PJSUA (feature pjsip)
├── assets/
│   ├── microsip-linux.desktop
│   └── icon.svg
├── tests/
│   └── sip-lab/          # PBX local de prueba (Python) + suite de 36 tests
└── debian/
    ├── control
    └── changelog
```

---

## Requisitos del sistema

### Para ejecutar (runtime)
- `libgtk-4-1` (GTK4) y `libadwaita-1-0` (paleta GTK4 moderna: `accent_bg_color`,
  esquema oscuro/claro vía `AdwStyleManager`) — la bandeja usa **ksni**
  (StatusNotifierItem sobre D-Bus) y **no** necesita GTK3 ni
  libayatana-appindicator. El escritorio debe alojar un StatusNotifierItem
  (KDE lo trae; GNOME requiere la extensión
  *AppIndicator and KStatusNotifierItem support*).
- `libglib2.0-0t64`, `libssl3t64`, `libxdo3`, `libuuid1`, `libgdk-pixbuf-2.0-0`,
  `libpango-1.0-0` (enlaces directos del binario)
- `libasound2t64` (backend ALSA de `pjmedia-audiodev`)
- `libasound2-plugins` (recomendado: puente ALSA→PulseAudio; pjproject no
  tiene backend Pulse propio)

> Los nombres de paquete son los de Ubuntu 24.04/Debian 13 (t64): en
> Ubuntu 22.04/Debian 12 serían `libgtk-4-1`, `libglib2.0-0`, `libssl3`,
> `libasound2`. El `.deb` se genera con `Depends: $auto` →
> `dpkg-shlibdeps` resuelve los nombres según la máquina de build.

### Para compilar
- `rustc` + `cargo` (1.70+)
- `libgtk-4-dev`, `libglib2.0-dev`, `libasound2-dev`, `pkg-config`

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
- **Cuentas**: cuentas guardadas o vacía (botón «＋ Cuenta»)
- **Marcador**: teclado numérico y botón "Llamar"
- **Llamadas**: lista de llamadas activas (contestar/rechazar/colgar/DTMF)
- **Bandeja**: al cerrar la ventana, se oculta a la bandeja del sistema
  (clic en el icono para mostrarla; menú «Salir»)

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

### Tema y esquema de color (GTK3 ↔ GTK4)

La app **no impone CSS propio**: usa las clases estándar de GTK4
(`suggested-action`, `destructive-action`, `title-1`, `dim-label`…).

Dos problemas reales que tenía una app GTK4 «pura» en un escritorio GTK3:

1. **Oscuro/claro.** GTK3 lo lee de `~/.config/gtk-3.0/settings.ini`, pero
   GTK4 **no lee ese fichero**, así que la app se quedaba en claro mientras el
   escritorio estaba oscuro. Se resuelve leyendo
   `org.gnome.desktop.interface color-scheme` y aplicándolo con
   **`AdwStyleManager::color_scheme`** (libadwaita ignora
   `gtk-application-prefer-dark-theme` y lo avisa como no soportado).

2. **Color de acento.** Sin libadwaita, `accent_bg_color` queda sin definir y
   los botones salen **grises**. Añadiendo `libadwaita` (0.4, emparejado con
   gtk4 0.6) y llamando a `adw_init()` al arranque, los botones reciben su
   acento.

> **Por qué no se fuerza `gtk-theme-name`:** el `gtk-4.0/gtk.css` de
> elementary **no define fondo para `button.suggested-action`** (0 reglas sin
> `.linked`), así que al cargarlo GTK4 pierde su Adwaita embebido y la app sale
> **entera en grises** (medido: 0 píxeles de acento frente a 11 247 sin
> forzar). Por eso el stylesheet del escritorio se registra solo para
> diagnóstico; se puede forzar con `MICROSIP_FORCE_THEME_NAME=1` para
> experimentar.

Log esperado con `RUST_LOG=info`:

```
theme: esquema del escritorio aplicado: tema="io.elementary.stylesheet.slate" oscuro=sí
```

Verificado en el escritorio real: fondo oscuro `#242424`, acento `#3584e4`,
texto visible y sin avisos `Adwaita-WARNING`.

---

## Empaquetado .deb (Debian/Ubuntu)

```bash
cargo install cargo-deb        # si no está instalado
cargo deb                      # genera target/debian/microsip-linux_0.1.1-1_amd64.deb
```

Instalar:

```bash
sudo apt install ./target/debian/microsip-linux_0.1.1-1_amd64.deb
```

El `.deb` incluye:
- `/usr/bin/microsip-linux`
- `/usr/share/applications/microsip-linux.desktop`
- `/usr/share/icons/hicolor/scalable/apps/microsip-linux.svg`
- `/usr/share/doc/microsip-linux/{copyright,changelog.Debian.gz}`

**Dependencias del paquete.** `Depends` se arma con `$auto`
(`dpkg-shlibdeps` sobre el binario → `libgtk-4-1`, `libglib2.0-0t64`,
`libssl3t64`, `libxdo3`, `libuuid1`… con su versión mínima) **más**
`libasound2t64` (backend ALSA de `pjmedia-audiodev`, enlazado en `build.rs`).
La bandeja usa **ksni** (SNI/D-Bus), sin libayatana-appindicator.

`Recommends: libasound2-plugins` aporta el puente ALSA→PulseAudio (pjproject
no tiene backend Pulse nativo). **pjproject se enlaza estático** en el binario
→ el `.deb` **no** depende de `libpjproject`/`libpj-*`.

> Ojo con `dest` en `[package.metadata.deb] assets`: sin `/` final y sin
> nombre de fichero, cargo-deb escribe el archivo **exactamente** en esa ruta
> (creaba un fichero llamado `/usr/bin`, dejando el `.deb` ininstalable).

---

## Release automático (.deb multi-arquitectura)

El workflow `.github/workflows/release.yml` genera paquetes para **tres
arquitecturas** y los publica como release al pushear una etiqueta `v*`
(también admite `workflow_dispatch` para probar sin etiqueta):

| Arch | Runner | Cómo se construye |
|---|---|---|
| `amd64` | `ubuntu-24.04` | nativo |
| `arm64` | `ubuntu-24.04-arm` | nativo (runner ARM64 de GitHub) |
| `armhf` | `ubuntu-24.04` | cross con `gcc-arm-linux-gnueabihf` + multiarch `armhf` |

Cada job:

1. instala GTK4/Adwaita/ALSA dev (y los `:armhf` en el job cruzado);
2. compila **pjproject 2.15** para la arquitectura objetivo
   (`scripts/build-pjproject.sh`, cacheado en `~/.local`);
3. `cargo deb --target <triple>` — `build.rs` obtiene las librerías de
   `libpjproject.pc` vía pkg-config, así que sirve para cualquier arquitectura;
4. **verifica** el resultado con `scripts/verify-deb.sh`: `Architecture`,
   `Machine` del ELF, `Depends` resueltos por `$auto` (GTK4/Adwaita/ALSA) y
   que **no** dependa de `libpjproject` (enlace estático);
5. sube el `.deb` como artefacto.

El job `release` descarga los tres y los adjunta a la release con
`softprops/action-gh-release`.

**v0.1.1:** corrige la navegación de la ventana (la v0.1.0 abría solo en
Cuentas, sin selector visible para llegar al marcador). Los jobs nativos
ejecutan además una prueba de regresión GTK bajo Xvfb.

Para publicar una versión nueva:

```bash
git tag v0.1.1 && git push origin v0.1.1
```

---

## Compilar pjproject (PJSIP) para Linux

Si quieres trabajar el backend FFI real, pjproject 2.15 ya está compilado en
`/home/jesus/.local` (librerías estáticas + headers + pkg-config). Para
recompilarlo desde cero:

```bash
cd /home/jesus/.local/src/pjsip/pjproject
./configure --prefix=$HOME/.local --disable-shared \
    CFLAGS="-I$HOME/.local/deps/usr/include" \
    LDFLAGS="-L$HOME/.local/deps/usr/lib/x86_64-linux-gnu" \
    --disable-sdl --disable-v4l2 --disable-opencore-amr --disable-speex \
    --disable-speexdsp --disable-l16 --disable-gsm --disable-speex-aec \
    --disable-webrtc --enable-sound --disable-portaudio --disable-resample \
    --disable-ffmpeg --disable-vpx --disable-libtheora --disable-libsrtp \
    --disable-httpd --disable-nginx
make dep && make -j$(nproc) && make install
```

Con `--enable-sound` pjmedia usa el backend **ALSA** (`-lasound`); los headers
de ALSA pueden extraerse sin sudo con `apt download libasound2-dev` +
`dpkg-deb -x` a `~/.local/deps` (ver `.cargo/config.toml`).
Luego compilar MicroSIP con `--features pjsip`.

---

## Cómo probar

| Qué probar | Comando | Qué debería ver |
|---|---|---|
| UI se abre | `cargo run --release` | Ventana "MicroSIP" abre en "Marcador", con Cuentas/Marcador/Llamadas/Sonido visibles |
| Cuentas | `cargo run --release` → pestaña "Cuentas" | Lista vacía o cuentas guardadas; botón «＋ Cuenta» |
| Marcador numérico | `cargo run --release` → "Marcador" | Teclado 0-9, *, #, botón "Llamar" |
| Regresión de navegación | `xvfb-run -a cargo test --features pjsip ui_exposes_navigation_and_dialer -- --ignored` | 1 passed: selector visible, apertura en Marcador, salto a Llamadas y controles de Sonido |
| Sonido | pestaña "Sonido" | Micrófono, altavoz de llamadas, timbre WAV, probar/detener prueba y guardar |
| Bandeja del sistema | `cargo run --release` → cerrar ventana | Icono SNI; clic para mostrar, menú «Salir» |
| Cuenta persistida | Guardar una cuenta y reiniciar | La cuenta reaparece en «Cuentas» (`~/.config/microsip-linux/accounts.conf`, 600) |
| Registro SIP real | Cuenta apuntando a un PBX y `RUST_LOG=info` | Log PJSUA: `Adding account`, `Registration sent` y `TX ... REGISTER` |
| Compilar .deb | `cargo deb` | `target/debian/*.deb` |
| PJSIP vinculado | `cargo build --features pjsip` | Compila sin errores de enlace |
| PJSIP inicializa | `cargo build --features pjsip && ./target/debug/microsip-linux` | Log: `PJSUA state changed: NULL --> CREATED --> RUNNING`, módulos SIP registrados |

Para probar sin servidor gráfico (entorno headless/CI):

```bash
xvfb-run -a cargo run --release   # necesita Xvfb
```

### Prueba end-to-end con PBX local (recomendada)

El repo incluye un PBX SIP de prueba en Python (`tests/sip-lab/`, sin sudo,
loopback) con una suite de 36 aserciones de protocolo:

```bash
cd tests/sip-lab
python3 pbx.py                 # terminal 1: PBX en 127.0.0.1:5060
python3 run_tests.py           # terminal 2: → "36 passed, 0 failed"
cd ../..
cargo build --features pjsip,test-hooks
# registrar 1001 y marcar a 1002 (ver tests/sip-lab/README.md para el config)
XDG_CONFIG_HOME=/tmp/ms-config MICROSIP_TEST_DIAL=sip:1002@127.0.0.1 \
  RUST_LOG=info xvfb-run -a ./target/debug/microsip-linux
```

Evidencia obtenida (2026-11-26): registro `401 → digest → 200 OK`
(`registration success, status=200`); llamada saliente `INVITE → 200 OK → ACK`
con `Encoder/Decoder stream started` y `Conf connect 1↔0` (audio bidireccional
ALSA); llamada entrante `180 Ringing → 200 OK` con audio conectado en ambos
extremos. La feature `test-hooks` (auto-marcar/auto-contestar) no se incluye en
el `.deb`.

### Audio y timbre

La pestaña **Sonido** enumera los dispositivos PJSUA/ALSA con capacidad de
captura o reproducción. La selección se guarda en
`~/.config/microsip-linux/audio.conf` (600), usando driver + nombre para
resolverla aunque cambie el ID numérico al reiniciar.

El timbre admite **WAV PCM de 16 bit mono a 16000 Hz** (el puente de audio de
este build no remuestrea). «Probar timbre» reproduce el WAV
seleccionado (o el incluido en el paquete) por el altavoz de llamadas;
«Detener prueba» cancela la reproducción. Al recibir una llamada suena
automáticamente y se detiene al contestar, rechazar o colgar. El `.deb`
incluye `assets/ringtone.wav` en `/usr/share/sounds/microsip-linux/`.

---

## Roadmap (v1 → v1.1)

1. Transferencia de llamada, hold y mute.
2. MWI/buzón de voz y llamadas perdidas.
3. Selector de códecs y puertos SIP/RTP.
4. Historial de llamadas.

---

## Licencia

GPL-2.0-only (misma licencia que MicroSIP original, basado en PJSIP).
