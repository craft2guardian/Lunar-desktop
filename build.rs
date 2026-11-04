use std::{env, path::PathBuf, process::Command};

fn run(command: &mut Command) {
    let status = command.status().expect("could not start native build tool");
    assert!(status.success(), "native build tool failed: {command:?}");
}

fn main() {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is not set"));
    let blur_object = out_dir.join("kwin_blur.o");
    let shell_object = out_dir.join("kde_shell.o");
    let protocol_object = out_dir.join("ext-background-effect-v1-protocol.o");
    let archive = out_dir.join("liblunar_kde_blur.a");
    let protocol_xml = PathBuf::from("protocols/ext-background-effect-v1.xml");
    let protocol_header = out_dir.join("ext-background-effect-v1-client-protocol.h");
    let protocol_code = out_dir.join("ext-background-effect-v1-protocol.c");
    run(Command::new("wayland-scanner")
        .arg("client-header")
        .arg(&protocol_xml)
        .arg(&protocol_header));
    run(Command::new("wayland-scanner")
        .arg("private-code")
        .arg(&protocol_xml)
        .arg(&protocol_code));
    let flags = Command::new("pkg-config")
        .args(["--cflags", "gtk4", "wayland-client"])
        .output()
        .expect("pkg-config is required to build the KDE blur support");
    assert!(flags.status.success(), "could not get GTK4/Wayland C flags");

    let cflags = String::from_utf8(flags.stdout).expect("pkg-config output was not UTF-8");
    for (source, object) in [
        ("src/kwin_blur.c", &blur_object),
        ("src/kde_shell.c", &shell_object),
    ] {
        let mut cc = Command::new(env::var_os("CC").unwrap_or_else(|| "cc".into()));
        cc.args(cflags.split_whitespace())
            .arg("-I")
            .arg(&out_dir)
            .args(["-fPIC", "-c", source, "-o"])
            .arg(object);
        run(&mut cc);
    }

    let mut cc = Command::new(env::var_os("CC").unwrap_or_else(|| "cc".into()));
    cc.args(cflags.split_whitespace())
        .arg("-fPIC")
        .arg("-c")
        .arg(&protocol_code)
        .arg("-o")
        .arg(&protocol_object);
    run(&mut cc);

    run(
        Command::new(env::var_os("AR").unwrap_or_else(|| "ar".into()))
            .arg("crus")
            .arg(&archive)
            .arg(&blur_object)
            .arg(&shell_object)
            .arg(&protocol_object),
    );

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    println!("cargo:rustc-link-lib=static=lunar_kde_blur");
    // GTK4 Layer Shell interposes Wayland surface setup. It must be loaded
    // before libwayland-client or its support probe and init hooks can be
    // bypassed by the already-loaded Wayland symbols.
    println!("cargo:rustc-link-lib=dylib=gtk4-layer-shell");
    println!("cargo:rustc-link-lib=wayland-client");
    println!("cargo:rerun-if-changed=src/kwin_blur.c");
    println!("cargo:rerun-if-changed=src/kde_shell.c");
    println!("cargo:rerun-if-changed=protocols/ext-background-effect-v1.xml");
    println!("cargo:rerun-if-changed=build.rs");
}
