//! Script de build: enlaza pjproject (PJSIP) cuando la feature `pjsip` está
//! activa.
//!
//! Es **multi-arquitectura**: las rutas y los nombres de librería se obtienen
//! de `libpjproject.pc` vía pkg-config, así que sirve igual para amd64, arm64
//! y armhf (lo único que cambia es el `.pc` con el que se compile pjproject).
//!
//! Añade a `PKG_CONFIG_PATH` los prefijos locales sin sudo
//! (`~/.local/lib/pkgconfig`, `~/.local/deps/.../pkgconfig`) para que funcione
//! también en la máquina de desarrollo.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_PJSIP");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH");
    println!("cargo:rerun-if-env-changed=PKG_CONFIG_PATH_FOR_TARGET");

    // Sin la feature `pjsip` no se toca pjproject (build mock sin dependencias C).
    if std::env::var("CARGO_FEATURE_PJSIP").is_err() {
        println!("cargo:warning=backend mock (sin pjproject)");
        return;
    }

    augment_pkg_config_path();

    // `statik(true)` => pkg-config --static, que incluye Libs.private
    // (-lasound, -lssl, -lcrypto, -luuid, …).
    let lib = match pkg_config::Config::new()
        .statik(true)
        .cargo_metadata(false)
        .probe("libpjproject")
    {
        Ok(lib) => lib,
        Err(err) => {
            println!("cargo:warning=no se encontró libpjproject: {err}");
            println!("cargo:warning=compilar pjproject (ver README) o fijar PKG_CONFIG_PATH");
            return;
        }
    };

    // --start-group/--end-group: las libs estáticas de pjproject tienen
    // referencias circulares entre sí y con las de terceros.
    println!("cargo:rustc-link-arg=-Wl,--start-group");
    for name in &lib.libs {
        println!("cargo:rustc-link-lib={name}");
    }
    println!("cargo:rustc-link-arg=-Wl,--end-group");

    for path in &lib.link_paths {
        println!("cargo:rustc-link-search=native={}", path.display());
    }
    // `-ldl` no aparece en el .pc pero algún backend lo necesita.
    println!("cargo:rustc-link-lib=dylib=dl");

    // La ruta -L de ALSA no viene en libpjproject.pc; se resuelve sondeando
    // el .pc de alsa (existe en el prefijo local o en el sistema).
    if let Ok(alsa) = pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("alsa")
    {
        for path in &alsa.link_paths {
            println!("cargo:rustc-link-search=native={}", path.display());
        }
    }

    println!("cargo:warning=PJSIP enlazado vía pkg-config (multi-arquitectura)");
}

/// Prepende los prefijos locales de desarrollo a las variables de búsqueda de
/// pkg-config.
///
/// Ojo con `PKG_CONFIG_PATH_<target>`: el crate `pkg-config` le da **prioridad
/// 1** sobre el `PKG_CONFIG_PATH` plano (prioridad 4), y `cargo deb --target`
/// lo inyecta apuntando solo a `/usr/lib/<triple>/pkgconfig`. Sin ampliarlo
/// aquí, `libpjproject` (que vive en `~/.local/lib/pkgconfig`) no se encuentra
/// y el build de CI falla con "no se encontró libpjproject".
fn augment_pkg_config_path() {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut dirs = vec![
        format!("{home}/.local/lib/pkgconfig"),
        format!("{home}/.local/deps/usr/lib/x86_64-linux-gnu/pkgconfig"),
    ];
    for var in ["PKG_CONFIG_PATH", "PKG_CONFIG_PATH_FOR_TARGET"] {
        if let Ok(existing) = std::env::var(var) {
            if !existing.is_empty() {
                dirs.push(existing);
            }
        }
    }
    // `TARGET` lo define cargo para los build scripts.
    if let Ok(target) = std::env::var("TARGET") {
        let var = format!("PKG_CONFIG_PATH_{target}");
        if let Ok(existing) = std::env::var(&var) {
            if !existing.is_empty() {
                dirs.push(existing);
            }
        }
        std::env::set_var(&var, dirs.join(":"));
    }
    // pkg-config ignora los directorios que no existen, así que es seguro
    // incluir rutas que solo existen en la máquina de desarrollo.
    std::env::set_var("PKG_CONFIG_PATH", dirs.join(":"));
}
