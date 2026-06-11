use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=assets/app-icon/windows.ico");
    println!("cargo:rerun-if-env-changed=KNOTQ_GOOGLE_OAUTH_CLIENT_ID");
    println!("cargo:rerun-if-env-changed=KNOTQ_GOOGLE_OAUTH_CLIENT_SECRET");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    if env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc") {
        return;
    }
    if !env::var("HOST").unwrap_or_default().contains("windows") {
        return;
    }

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest_dir
        .join("assets")
        .join("app-icon")
        .join("windows.ico");
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let rc_file = out_dir.join("knotq.rc");
    let res_file = out_dir.join("knotq.res");

    fs::write(&rc_file, format!("1 ICON \"{}\"\n", escape_rc_path(&icon)))
        .expect("write Windows resource file");

    let rc = find_rc_exe().unwrap_or_else(|| PathBuf::from("rc.exe"));
    let status = Command::new(&rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_file)
        .arg(&rc_file)
        .status()
        .expect("run rc.exe to compile Windows resources");

    if !status.success() {
        panic!("rc.exe failed with status {status}");
    }

    println!("cargo:rustc-link-arg-bin=knotq={}", res_file.display());
}

fn escape_rc_path(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

fn find_rc_exe() -> Option<PathBuf> {
    if Command::new("rc.exe").arg("/?").output().is_ok() {
        return Some(PathBuf::from("rc.exe"));
    }

    let roots = [
        env::var_os("ProgramFiles(x86)").map(PathBuf::from),
        env::var_os("ProgramFiles").map(PathBuf::from),
    ];

    for root in roots.into_iter().flatten() {
        let bin = root.join("Windows Kits").join("10").join("bin");
        let Ok(versions) = fs::read_dir(bin) else {
            continue;
        };
        let mut candidates = versions
            .filter_map(Result::ok)
            .map(|entry| entry.path().join("x64").join("rc.exe"))
            .filter(|path| path.exists())
            .collect::<Vec<_>>();
        candidates.sort();
        if let Some(path) = candidates.pop() {
            return Some(path);
        }
    }

    None
}
