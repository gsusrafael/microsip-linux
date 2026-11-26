#!/usr/bin/env bash
# Verifica un .deb generado por cargo-deb antes de publicarlo.
#   scripts/verify-deb.sh <fichero.deb> <amd64|arm64|armhf>
#
# LC_ALL=C es obligatorio: readelf localiza sus etiquetas ("Máquina:" en
# español) y entonces el parseo deja de reconocer "Machine:".
export LC_ALL=C
set -euo pipefail

DEB="${1:?falta el .deb}"
WANT_ARCH="${2:?falta la arquitectura esperada}"

echo "==> verificando $DEB (esperado: $WANT_ARCH)"
dpkg-deb -I "$DEB"

# 1) Architecture correcta
arch="$(dpkg-deb -f "$DEB" Architecture)"
if [ "$arch" != "$WANT_ARCH" ]; then
  echo "FAIL: Architecture '$arch' != '$WANT_ARCH'"; exit 1
fi

# 2) el binario debe ser de la arquitectura esperada
tmp="$(mktemp -d)"
dpkg-deb -x "$DEB" "$tmp"
bin="$tmp/usr/bin/microsip-linux"
[ -x "$bin" ] || { echo "FAIL: no hay /usr/bin/microsip-linux"; exit 1; }
machine="$(readelf -h "$bin" | awk -F: '/Machine/{gsub(/^[ \t]+/,"",$2); print $2}')"
case "$WANT_ARCH" in
  amd64) want="Advanced Micro Devices X86-64" ;;
  arm64) want="AArch64" ;;
  armhf) want="ARM" ;;
  *)     want="" ;;
esac
echo "Machine: $machine"
if [ -n "$want" ] && ! echo "$machine" | grep -qi "$want"; then
  echo "FAIL: Machine '$machine' no parece $want"; exit 1
fi

# 3) $auto debe haber resuelto las dependencias de runtime
deps="$(dpkg-deb -f "$DEB" Depends)"
echo "Depends: $deps"
for d in libgtk-4-1 libadwaita-1-0 libasound2; do
  if ! echo "$deps" | grep -q "$d"; then
    echo "FAIL: falta Depends '$d' (dpkg-shlibdeps no resolvió \$auto)"; exit 1
  fi
done

# 4) NO debe depender de libpjproject: se enlaza estático
if echo "$deps" | grep -q "libpjproject"; then
  echo "FAIL: depende de libpjproject pese al enlace estático"; exit 1
fi

# 5) contenido mínimo
for f in usr/share/applications/microsip-linux.desktop \
         usr/share/icons/hicolor/scalable/apps/microsip-linux.svg; do
  [ -e "$tmp/$f" ] || { echo "FAIL: falta $f"; exit 1; }
done

rm -rf "$tmp"
echo "OK: $DEB"
