//! Script de build para enlazar contra pjproject (PJSIP) compilado en ~/.local.
//!
//! Solo enlaza pjproject cuando la feature `pjsip` está activa
//! (`CARGO_FEATURE_PJSIP`); sin ella el binario mock no necesita pjproject.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_PJSIP");

    let pjsip_enabled = std::env::var("CARGO_FEATURE_PJSIP").is_ok();
    if !pjsip_enabled {
        println!("cargo:warning=backend mock (sin pjproject)");
        return;
    }

    let local_lib = "/home/jesus/.local/lib";
    let local_include = "/home/jesus/.local/include";

    // Librerías estáticas de pjproject (en orden de dependencia).
    // Se envuelven en --start-group/--end-group para resolver referencias
    // circulares entre libs estáticas de terceros (speex, gsm, etc.).
    println!("cargo:rustc-link-arg=-Wl,--start-group");
    let libs = [
        "pjsua2-x86_64-unknown-linux-gnu",
        "pjsua-x86_64-unknown-linux-gnu",
        "pjsip-ua-x86_64-unknown-linux-gnu",
        "pjsip-simple-x86_64-unknown-linux-gnu",
        "pjsip-x86_64-unknown-linux-gnu",
        "pjmedia-codec-x86_64-unknown-linux-gnu",
        "pjmedia-videodev-x86_64-unknown-linux-gnu",
        "pjmedia-audiodev-x86_64-unknown-linux-gnu",
        "pjmedia-x86_64-unknown-linux-gnu",
        "pjnath-x86_64-unknown-linux-gnu",
        "pjlib-util-x86_64-unknown-linux-gnu",
        "pj-x86_64-unknown-linux-gnu",
        "gsmcodec-x86_64-unknown-linux-gnu",
        "speex-x86_64-unknown-linux-gnu",
        "ilbccodec-x86_64-unknown-linux-gnu",
        "g7221codec-x86_64-unknown-linux-gnu",
        "yuv-x86_64-unknown-linux-gnu",
        "webrtc-x86_64-unknown-linux-gnu",
    ];
    for libname in libs {
        println!("cargo:rustc-link-lib=static={libname}");
    }
    println!("cargo:rustc-link-arg=-Wl,--end-group");

    // Dependencias del sistema (dinámicas).
    // `asound`: pjproject 2.15 se compiló con el backend ALSA de
    // pjmedia-audiodev (Libs.private de libpjproject.pc termina en -lasound).
    for syslib in [
        "ssl", "crypto", "uuid", "m", "rt", "pthread", "dl", "asound",
    ] {
        println!("cargo:rustc-link-lib=dylib={syslib}");
    }
    println!("cargo:rustc-link-lib=stdc++");
    println!("cargo:rustc-link-search=native={local_lib}");
    println!("cargo:rerun-if-changed={local_lib}");
    println!("cargo:rerun-if-changed={local_include}");

    match pkg_config::Config::new()
        .cargo_metadata(false)
        .probe("libpjproject")
    {
        Ok(_) => println!("cargo:warning=PJSIP backend enabled via pkg-config"),
        Err(_) => println!("cargo:warning=PJSIP backend enabled via hardcoded ~/.local paths"),
    }
}
