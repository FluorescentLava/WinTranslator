use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        compile_resources();
    }
    #[cfg(feature = "portable")]
    windows_reactor_setup::as_self_contained();
    #[cfg(not(feature = "portable"))]
    {
        let manifest = std::path::Path::new(&std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("app.manifest");
        println!("cargo:rustc-link-arg-bins=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg-bins=/MANIFESTINPUT:{}",
            manifest.display()
        );
        println!("cargo:rerun-if-changed=app.manifest");
    }
}

fn compile_resources() {
    println!("cargo:rerun-if-changed=app.rc");
    println!("cargo:rerun-if-changed=assets/icons/app.ico");
    for name in [
        "RC",
        "PATH",
        "WindowsSdkDir",
        "WindowsSdkBinPath",
        "WindowsSdkVerBinPath",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let resource = PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("WinTranslater.res");
    let compiler = resource_compiler().expect(
        "Windows SDK rc.exe was not found. Install the Windows SDK or add rc.exe to PATH (or set RC).",
    );
    let output = Command::new(&compiler)
        .current_dir(&root)
        .arg("/nologo")
        .arg("/fo")
        .arg(&resource)
        .arg("app.rc")
        .output()
        .unwrap_or_else(|error| panic!("Unable to run {}: {error}", compiler.display()));
    assert!(
        output.status.success(),
        "Compiling WinTranslater resources failed with {}:\n{}{}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    // Only icon/version resources live here. The existing manifest path above
    // remains responsible for packaged and self-contained runtime activation.
    println!("cargo:rustc-link-arg-bins={}", resource.display());
}

fn resource_compiler() -> Option<PathBuf> {
    if let Some(path) = env::var_os("RC") {
        return Some(PathBuf::from(path));
    }
    if let Some(path) = env::var_os("PATH")
        && let Some(compiler) = env::split_paths(&path)
            .map(|directory| directory.join("rc.exe"))
            .find(|path| path.is_file())
    {
        return Some(compiler);
    }
    let host = env::var("HOST").unwrap_or_default();
    let architecture = if host.starts_with("aarch64") {
        "arm64"
    } else if host.starts_with("i686") {
        "x86"
    } else {
        "x64"
    };
    let mut directories: Vec<PathBuf> = ["WindowsSdkVerBinPath", "WindowsSdkBinPath"]
        .into_iter()
        .filter_map(env::var_os)
        .map(PathBuf::from)
        .collect();
    let mut sdk_roots: Vec<PathBuf> = env::var_os("WindowsSdkDir")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    for variable in ["ProgramFiles(x86)", "ProgramFiles"] {
        if let Some(root) = env::var_os(variable) {
            sdk_roots.push(PathBuf::from(root).join("Windows Kits").join("10"));
        }
    }
    for sdk in sdk_roots {
        let bin = sdk.join("bin");
        if let Ok(entries) = std::fs::read_dir(&bin) {
            let mut versions: Vec<_> = entries
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| {
                    path.is_dir()
                        && path
                            .file_name()
                            .is_some_and(|name| name.to_string_lossy().starts_with("10."))
                })
                .collect();
            versions.sort_by(|left, right| sdk_version(right).cmp(&sdk_version(left)));
            directories.extend(versions);
        }
        directories.push(bin);
    }
    for directory in directories {
        for candidate in [
            directory.join(architecture).join("rc.exe"),
            directory.join("rc.exe"),
            directory.join("x64").join("rc.exe"),
            directory.join("x86").join("rc.exe"),
        ] {
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

fn sdk_version(path: &Path) -> Vec<u32> {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .split('.')
        .filter_map(|part| part.parse().ok())
        .collect()
}
