# sip-lab — PBX SIP local de prueba para validar MicroSIP

Laboratorio **loopback**, sin sudo, sin tocar la configuración del escritorio,
sin credenciales reales. Vive en `tests/sip-lab/` dentro de este repo.

```
MicroSIP / pjsua  ──UDP 5060──▶  pbx.py (registrar + proxy)  ──▶  otro softphone
                                 (Python stdlib, sin dependencias)
```

## Qué hay aquí

| Archivo | Qué hace |
|---|---|
| `pbx.py` | El PBX: registrar/desregistrar con digest MD5, proxy de INVITE, relay de 100/180/200, intercambio SDP, ACK/BYE/CANCEL, 404/486/405/483 |
| `siplib.py` | Parser/serializador SIP + digest (compartido) |
| `gen_accounts.py` | Genera `accounts.json` (cuentas de prueba obvias, modo 600) |
| `accounts.json` | Cuentas de prueba — **no son credenciales reales** |
| `sip_client.py` | UA SIP en Python (el lado "scripted" de las pruebas) |
| `run_tests.py` | Suite de 36 aserciones de protocolo → `evidencia/test-results.json` |
| `run_pjsua_test.sh` | Empareja dos `pjsua` reales (el stack PJSIP de MicroSIP) contra el PBX |

### Binarios disponibles en la máquina (investigado, [VERIFIED 2026-11-26])

- **No hay** `asterisk` ni `freeswitch` instalados → por eso el PBX propio en Python.
- **`pjsua` de pjproject 2.15 YA compilado** (sin sudo, en `~/.local`):
  `/home/jesus/.local/src/pjsip/pjproject/pjsip-apps/bin/pjsua-x86_64-unknown-linux-gnu`
  (compilado en una sesión anterior; logs `~/…/pj-install.log`, `pj-make.log`).
  Mismo library PJSIP que usa MicroSIP → harness de softphone real.
- `python3` 3.12, `node` 24, `nc`, `socat`, `ss` disponibles.
- MicroSIP: port Linux en `~/Workspace/microsip-linux` (**no se toca**, es de
  otro trabajo) y fuente en `~/.local/src/pjsip/microsip-src`.

## Arranque rápido (comandos exactos)

```sh
cd tests/sip-lab

# 1) cuentas de prueba (ya generadas; --force para regenerar)
python3 gen_accounts.py

# 2) arrancar el PBX (loopback, UDP 5060)
python3 pbx.py
#    → "listening on udp://127.0.0.1:5060 (advertise=127.0.0.1) realm=sip-lab accounts=3"

# 3) en otra terminal: suite de protocolo (36 aserciones)
python3 run_tests.py
#    → "==== 36 passed, 0 failed ===="   exit 0

# 4) en otra terminal: llamada real entre dos pjsua (PJSIP)
./run_pjsua_test.sh
#    → "RESULT: PASS — both pjsua registered and the call reached CONFIRMED"
```

El PBX corre en foreground; `Ctrl-C` lo detiene limpiamente. Para background:

```sh
setsid nohup python3 pbx.py > logs/pbx.stdout.log 2>&1 &
# detener (por pid exacto, nunca pkill):
kill "$(ss -ulnp | grep 127.0.0.1:5060 | grep -oP 'pid=\K[0-9]+')"
```

## Configurar MicroSIP contra este PBX

Campos de cuenta en MicroSIP (los tres valores son de `accounts.json`):

| Campo MicroSIP | Valor |
|---|---|
| Account name | `lab-1001` (o el que quieras) |
| SIP Server / Domain | `127.0.0.1` (o `127.0.0.1:5060`) |
| Username | `1001` |
| Password | `lab-1001-test` |

- **Dos cuentas para llamar entre sí**: `1001`/`lab-1001-test` y
  `1002`/`lab-1002-test` (y `1003`/`lab-1003-test` como tercero).
- Cada MicroSIP necesita su propio puerto SIP si corren en la misma máquina
  (MicroSIP → *Settings* → *Advanced* → *SIP Port*): usa `5061` y `5062` para
  no chocar con el PBX en `5060`.
- Tras registrar (aparece el indicador de cuenta activa), marca `1002` y
  llamar: verás **180 Ringing → CONFIRMED**, y en `logs/pbx.log` el flujo
  completo con el intercambio SDP.
- Si MicroSIP corre en **otra máquina** (p. ej. Windows), arranca el PBX con
  `python3 pbx.py --host 0.0.0.0` y usa la IP de esta máquina como servidor.
  (Con `--host 0.0.0.0` el PBX auto-detecta la IP a anunciar; si quieres
  fijarla: `--advertise <IP>`.)

## E2E automatizado con nuestra app (feature `test-hooks`)

La app puede marcar y contestar sola para validar el camino completo sin
clics (no entra en el `.deb`):

```sh
# desde la raíz del repo, con el PBX arriba (python3 pbx.py)
mkdir -p /tmp/ms-config/microsip-linux
cat > /tmp/ms-config/microsip-linux/accounts.conf <<'EOF'
id=1001@127.0.0.1
username=1001
password=lab-1001-test
domain=127.0.0.1
display_name=Alice
proxy=

EOF
chmod 600 /tmp/ms-config/microsip-linux/accounts.conf

cargo build --features pjsip,test-hooks

# saliente: la app registra (401→200) y marca a 1002
XDG_CONFIG_HOME=/tmp/ms-config MICROSIP_TEST_DIAL=sip:1002@127.0.0.1 \
  RUST_LOG=info xvfb-run -a ./target/debug/microsip-linux

# entrante: la app registra y contesta sola cualquier INVITE
XDG_CONFIG_HOME=/tmp/ms-config MICROSIP_TEST_AUTO_ANSWER=1 \
  RUST_LOG=info xvfb-run -a ./target/debug/microsip-linux
```

Evidencia observada (2026-11-26): registro `401 → digest → 200 OK`
(`registration success, status=200`), INVITE saliente `200 OK → ACK` con
`Encoder/Decoder stream started` y `Conf connect 1↔0`, y entrante
`180 Ringing → 200 OK` con audio conectado en ambos extremos.

> Nota: el parser `uri_user()` de este lab extrae mal el user part cuando el
> AOR lleva display name (`"Alice" <sip:1001@...>`); corregido en
> `siplib.py` para aceptar name-addr.

## Ver la señal "en vivo"

```sh
tail -f logs/pbx.log
# RX INVITE ... | SDP: c=IN IP4 127.0.0.1, m=audio 4000 RTP/AVP 0 8
# FWD  INVITE 1001=>1002 -> 127.0.0.1:5062 (branch=..., call-id=...)
# TX  180 Ringing -> ...
# TX  200 OK -> ... | SDP: m=audio 4002 ...
# CALL answered (200 OK) 1001 => 1002
# FWD  ACK / FWD BYE → CALL ended (BYE)
```

## Qué valida la suite (`run_tests.py`, 36 aserciones)

1. REGISTER sin credenciales → **401** con challenge `Digest` (nonce, qop).
2. REGISTER con contraseña equivocada → **401** (digest falla).
3. REGISTER de 1001/1002/1003 → **200** con `Contact`+`expires`.
4. OPTIONS keepalive → **200** con `Allow`.
5. **Llamada completa 1001→1002**: INVITE con oferta SDP → 100 → 180 → 200
   con respuesta SDP → ACK (sin SDP) → BYE → 200. Verifica que el PBX añade
   `Record-Route` (se queda en la señalización) y que la SDP viaja intacta
   (puerto 4000 en la oferta, 4002 en la respuesta).
6. Segunda INVITE a una línea timbrando → **486 Busy Here**.
7. **CANCEL** de un diálogo temprano → **200 (CANCEL)** + **487 (INVITE)**,
   con el CANCEL reutilizando branch y CSeq del INVITE (RFC 3261 §9.1).
8. INVITE a extensión inexistente → **404**.
9. **Desregistro** (`Expires: 0`) → 200 y luego INVITE → **404**.
10. Nonce de digest desconocido/caducado → **401** con nonce nuevo.

Y `run_pjsua_test.sh` valida contra **dos softphones PJSIP reales**: registro
con digest (`registration success, status=200`) y llamada `CONFIRMED` en ambos
extremos, colgada con `200 OK`.

## Límites conocidos (diseño, no bugs)

- **Solo UDP/IPv4**, loopback por defecto. Sin TCP, TLS ni SRTP.
- **No toca RTP**: el PBX es solo señalización; el audio va directo entre
  softphones (por eso en la SDP de pjsua se ve la IP local real
  `100.112.160.199`, no127.0.0.1 — correcto, es la IP que el softphone anuncia).
- **No reenvía ni valida `Require`/extensiones** (no100rel/PRACK, no
  transferencias). Si un softphone pide algo que no entendemos, se ignora.
- **Digest solo MD5 + qop=auth** (lo que usan MicroSIP y pjsua). Sin MD5-sess,
  sin SHA-256, sin IPsec.
- **Una llamada por extensión** (`486` si la línea está ocupada); sin fork.
- El estado de llamada "ringing" sin BYE se **reapa a los 120 s** (el caller
  desapareció) y una llamada confirmada sin BYE a los 600 s.
- `--auth-invite` existe como flag pero está **apagado**: el INVITE no pide
  credenciales (como muchos PBX de lab). Para exigirlo: `python3 pbx.py --auth-invite`.

## Evidencia

- `evidencia/test-results.json` — 36 aserciones con detalle.
- `logs/pbx.log` — traza completa de señalización (RX/TX/FWD/CALL).
- `logs/pjsua-caller.log`, `logs/pjsua-callee.log` — softphones PJSIP reales.

> **Cuentas de prueba**: `lab-1001-test` etc. son deliberadamente obvias y solo
> valen para este lab loopback. No usar en ningún otro sitio.
