#!/usr/bin/env bash
# Compila pjproject (PJSIP) para la arquitectura actual o para un host de
# cross-compilación y lo instala en $PJ_PREFIX (por defecto ~/.local).
#
# Uso:
#   ./scripts/build-pjproject.sh                    # build nativo
#   CROSS_HOST=arm-linux-gnueabihf ./scripts/build-pjproject.sh   # cross
#
# variables opcionales:
#   PJ_PREFIX  prefijo de instalación (default: $HOME/.local)
#   PJ_SOURCE  directorio fuente  (default: $HOME/.local/src/pjproject)
#   PJ_VERSION etiqueta de pjproject (default: 2.15)
set -euo pipefail

PJ_PREFIX="${PJ_PREFIX:-$HOME/.local}"
PJ_SOURCE="${PJ_SOURCE:-$HOME/.local/src/pjproject}"
PJ_VERSION="${PJ_VERSION:-2.15}"
JOBS="$(nproc)"

# --- ¿ya compilado? -----------------------------------------------------
# Si ya hay un .pc instalado y no se pide forzar, se reutiliza (acelera la
# caché de CI y evita romper un build local que ya funciona).
if [ -f "$PJ_PREFIX/lib/pkgconfig/libpjproject.pc" ] && [ -z "${PJ_FORCE:-}" ]; then
  echo "==> pjproject ya instalado en $PJ_PREFIX (usa PJ_FORCE=1 para reconstruir)"
  PKG_CONFIG_PATH="$PJ_PREFIX/lib/pkgconfig" pkg-config --modversion libpjproject
  exit 0
fi

# --- fuente -------------------------------------------------------------
if [ ! -f "$PJ_SOURCE/configure" ] && [ ! -f "$PJ_SOURCE/aconfigure" ]; then
  echo "==> descargando pjproject $PJ_VERSION"
  tmp="$(mktemp -d)"
  curl -fsSL -o "$tmp/pj.tar.gz" \
    "https://github.com/pjsip/pjproject/archive/refs/tags/${PJ_VERSION}.tar.gz"
  tar -xzf "$tmp/pj.tar.gz" -C "$tmp"
  mkdir -p "$(dirname "$PJ_SOURCE")"
  rm -rf "$PJ_SOURCE"
  mv "$tmp/pjproject-${PJ_VERSION}" "$PJ_SOURCE"
  rm -rf "$tmp"
fi

cd "$PJ_SOURCE"
[ -f config.status ] || [ -f Makefile ] && make distclean >/dev/null 2>&1 || true

# --- flags comunes ------------------------------------------------------
# - enable-sound: backend ALSA (audio bidireccional)
# - disable-shared: solo libs estáticas, el .deb no depende de libpjproject
CONF=(
  --prefix="$PJ_PREFIX"
  --disable-shared
  --enable-sound
  --disable-sdl --disable-v4l2 --disable-portaudio --disable-resample
  --disable-ffmpeg --disable-vpx --disable-libtheora --disable-libsrtp
  --disable-httpd --disable-nginx
)

# --- cross-compilación --------------------------------------------------
if [ -n "${CROSS_HOST:-}" ]; then
  echo "==> cross-compilación para $CROSS_HOST"
  CONF+=(--host="$CROSS_HOST")
  export CC="${CROSS_HOST}-gcc"
  export CXX="${CROSS_HOST}-g++"
  export AR="${CROSS_HOST}-ar"
  export RANLIB="${CROSS_HOST}-ranlib"
  export STRIP="${CROSS_HOST}-strip"
  # pkg-config debe ver los .pc del target, no los del host.
  export PKG_CONFIG_LIBDIR="/usr/lib/${CROSS_HOST}/pkgconfig:/usr/share/pkgconfig"
  export PKG_CONFIG_ALLOW_CROSS=1
fi

# ALSA headers pueden vivir en un prefijo local sin sudo (máquina de dev).
if [ -d "$HOME/.local/deps/usr/include" ]; then
  export CFLAGS="${CFLAGS:-} -I$HOME/.local/deps/usr/include"
  export LDFLAGS="${LDFLAGS:-} -L$HOME/.local/deps/usr/lib/x86_64-linux-gnu"
fi

echo "==> configure"
./configure "${CONF[@]}"

echo "==> make (-j$JOBS)"
make -j"$JOBS"

echo "==> make install -> $PJ_PREFIX"
make install

echo "==> pkg-config"
PKG_CONFIG_PATH="$PJ_PREFIX/lib/pkgconfig" pkg-config --modversion libpjproject
PKG_CONFIG_PATH="$PJ_PREFIX/lib/pkgconfig" pkg-config --static --libs libpjproject | head -c 400
echo
echo "pjproject listo en $PJ_PREFIX"
