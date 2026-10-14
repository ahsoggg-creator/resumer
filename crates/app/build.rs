use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=assets/app.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    println!("cargo:rerun-if-changed=assets/app.manifest");
    println!("cargo:rerun-if-env-changed=RESUMER_CLI_EXE");

    // The installer carries resumer-cli.exe inside itself: take it from RESUMER_CLI_EXE or from the target
    // folder (build the CLI first). Dev builds without it embed nothing and fall back to a sibling file.
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("resumer-cli.bin");
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".into());
    let candidates = [
        std::env::var_os("RESUMER_CLI_EXE").map(PathBuf::from),
        Some(manifest.join("../../target").join(&profile).join("resumer-cli.exe")),
    ];
    let found = candidates.into_iter().flatten().find(|p| p.is_file());
    match found {
        Some(p) => {
            println!("cargo:rerun-if-changed={}", p.display());
            std::fs::copy(&p, &out).unwrap();
        }
        None => std::fs::write(&out, []).unwrap(),
    }

    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let v = |k: &str| {
            format!(
                "{k}={}",
                std::env::var(format!("CARGO_PKG_VERSION_{}", &k[3..])).unwrap()
            )
        };
        let macros = [v("RS_MAJOR"), v("RS_MINOR"), v("RS_PATCH")];
        embed_resource::compile("assets/app.rc", &macros)
            .manifest_optional()
            .unwrap();
    }
}
