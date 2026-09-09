use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=cxx/decklink_c.h");
    println!("cargo:rerun-if-changed=cxx/shim.cpp");
    println!("cargo:rerun-if-env-changed=DECKLINK_SDK_DIR");
    println!("cargo:rerun-if-env-changed=DECKLINK_FORCE_STUB");
    println!("cargo:rerun-if-env-changed=MIDL");

    let hardware = env::var("CARGO_FEATURE_HARDWARE").is_ok();
    if !hardware || env::var("DECKLINK_FORCE_STUB").is_ok() {
        compile_stub();
        return;
    }

    let Some(sdk) = find_sdk() else {
        println!(
            "cargo:warning=DeckLink SDK not found; building stub bindings. Set DECKLINK_SDK_DIR to enable hardware."
        );
        compile_stub();
        return;
    };

    println!("cargo:rerun-if-changed={}", sdk.display());
    if let Err(err) = compile_hardware(&sdk) {
        println!("cargo:warning=DeckLink hardware shim failed ({err}); falling back to stub.");
        compile_stub();
    }
}

fn compile_stub() {
    cc::Build::new()
        .cpp(true)
        .file("cxx/shim.cpp")
        .include("cxx")
        .define("RDL_STUB", None)
        .warnings(false)
        .compile("decklink_shim");
}

fn compile_hardware(sdk: &Path) -> Result<(), String> {
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let mut build = cc::Build::new();
    build
        .cpp(true)
        .file("cxx/shim.cpp")
        .include("cxx")
        .define("RDL_HARDWARE", None)
        .warnings(false);

    match target_os.as_str() {
        "windows" => {
            let generated = generate_windows_headers(sdk)?;
            build.include(&generated);
            build.include(sdk.join("Win").join("include"));
            build.file(generated.join("DeckLinkAPI_i.c"));
            println!("cargo:rustc-link-lib=ole32");
            println!("cargo:rustc-link-lib=oleaut32");
            println!("cargo:rustc-link-lib=rpcrt4");
        }
        "linux" => {
            let include = sdk.join("Linux").join("include");
            if !include.join("DeckLinkAPI.h").exists() {
                return Err("Linux/include/DeckLinkAPI.h missing".into());
            }
            build.include(&include);
            build.file(include.join("DeckLinkAPIDispatch.cpp"));
            println!("cargo:rustc-link-lib=dylib=dl");
            println!("cargo:rustc-link-lib=dylib=pthread");
        }
        "macos" => {
            let include = sdk.join("Mac").join("include");
            if !include.join("DeckLinkAPI.h").exists() {
                return Err("Mac/include/DeckLinkAPI.h missing".into());
            }
            build.include(&include);
            build.file(include.join("DeckLinkAPIDispatch.cpp"));
            println!("cargo:rustc-link-lib=framework=CoreFoundation");
        }
        other => return Err(format!("unsupported target OS {other}")),
    }

    build.compile("decklink_shim");
    println!("cargo:rustc-cfg=decklink_hardware");
    Ok(())
}

fn generate_windows_headers(sdk: &Path) -> Result<PathBuf, String> {
    let idl = sdk.join("Win").join("include").join("DeckLinkAPI.idl");
    if !idl.exists() {
        return Err("Win/include/DeckLinkAPI.idl missing".into());
    }

    let out = PathBuf::from(env::var("OUT_DIR").unwrap());
    let header = out.join("DeckLinkAPI_h.h");
    if header.exists() {
        return Ok(out);
    }

    run_midl(&idl, &out)?;
    if !header.exists() {
        return Err("midl did not produce DeckLinkAPI_h.h".into());
    }
    Ok(out)
}

fn windows_idl_arch() -> &'static str {
    match env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default().as_str() {
        "x86" => "x86",
        "aarch64" => "arm64",
        _ => "x64",
    }
}

fn run_midl(idl: &Path, out: &Path) -> Result<(), String> {
    let arch = windows_idl_arch();
    if let Some(vcvars) = find_vcvarsall() {
        let bat = out.join("run_midl.bat");
        let script = format!(
            "@echo off\r\n\
             call \"{vcvars}\" {arch}\r\n\
             if errorlevel 1 exit /b 1\r\n\
             midl.exe /nologo /W1 /char signed /env {arch} /h DeckLinkAPI_h.h /iid DeckLinkAPI_i.c \"{idl}\"\r\n",
            vcvars = vcvars.display(),
            arch = arch,
            idl = idl.display()
        );
        std::fs::write(&bat, script).map_err(|err| err.to_string())?;
        return finish_midl(Command::new(&bat).current_dir(out).output(), "via vcvarsall");
    }

    let midl = find_midl().ok_or("midl.exe not found; install Visual Studio C++ tools")?;
    let mut cmd = Command::new(&midl);
    cmd.args([
        "/nologo",
        "/W1",
        "/char",
        "signed",
        "/env",
        arch,
        "/h",
        "DeckLinkAPI_h.h",
        "/iid",
        "DeckLinkAPI_i.c",
    ])
    .arg(idl)
    .current_dir(out);
    if let Some(dir) = midl.parent() {
        prepend_path(&mut cmd, dir);
    }
    finish_midl(cmd.output(), "direct")
}

fn finish_midl(output: std::io::Result<std::process::Output>, how: &str) -> Result<(), String> {
    let output = output.map_err(|err| err.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = [stderr.trim(), stdout.trim()]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("no output");
    Err(format!("midl failed to compile DeckLinkAPI.idl ({how}): {detail}"))
}

fn prepend_path(cmd: &mut Command, dir: &Path) {
    let mut dirs = vec![dir.to_path_buf()];
    if let Some(path) = env::var_os("PATH") {
        dirs.extend(env::split_paths(&path));
    }
    if let Ok(joined) = env::join_paths(dirs) {
        cmd.env("PATH", joined);
    }
}

fn find_vcvarsall() -> Option<PathBuf> {
    let vswhere = find_vswhere()?;
    let output = Command::new(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-find",
            r"VC\Auxiliary\Build\vcvarsall.bat",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(PathBuf::from)?;
    path.is_file().then_some(path)
}

fn find_vswhere() -> Option<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(x86) = env::var("ProgramFiles(x86)") {
        roots.push(PathBuf::from(x86));
    }
    if let Ok(pf) = env::var("ProgramFiles") {
        roots.push(PathBuf::from(pf));
    }
    roots.into_iter().find_map(|root| {
        let vswhere = root
            .join("Microsoft Visual Studio")
            .join("Installer")
            .join("vswhere.exe");
        vswhere.is_file().then_some(vswhere)
    })
}

fn find_midl() -> Option<PathBuf> {
    if let Ok(path) = env::var("MIDL") {
        let path = PathBuf::from(path);
        if path.is_file() {
            return Some(path);
        }
    }
    which("midl.exe")
        .or_else(|| which("midl"))
        .or_else(find_midl_in_windows_kits)
}

fn find_midl_in_windows_kits() -> Option<PathBuf> {
    let arch = windows_idl_arch();
    let mut bins = Vec::new();
    if let Ok(x86) = env::var("ProgramFiles(x86)") {
        bins.push(PathBuf::from(x86).join(r"Windows Kits\10\bin"));
    }
    if let Ok(pf) = env::var("ProgramFiles") {
        bins.push(PathBuf::from(pf).join(r"Windows Kits\10\bin"));
    }

    let mut found = Vec::new();
    for bin in bins {
        let Ok(entries) = std::fs::read_dir(&bin) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.starts_with("10.") {
                continue;
            }
            let midl = entry.path().join(arch).join("midl.exe");
            if midl.is_file() {
                found.push((name.into_owned(), midl));
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found.pop().map(|(_, path)| path)
}

fn which(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths).find_map(|dir| {
            let candidate = dir.join(name);
            candidate.exists().then_some(candidate)
        })
    })
}

fn find_sdk() -> Option<PathBuf> {
    if let Ok(dir) = env::var("DECKLINK_SDK_DIR") {
        let path = PathBuf::from(dir);
        if looks_like_sdk(&path) {
            return Some(path);
        }
    }

    let mut candidates = Vec::new();
    if let Ok(home) = env::var("USERPROFILE") {
        candidates.push(PathBuf::from(home).join(r"Downloads\Blackmagic DeckLink SDK 16.0"));
    }
    if let Ok(home) = env::var("HOME") {
        candidates.push(PathBuf::from(&home).join("Downloads/Blackmagic DeckLink SDK 16.0"));
        candidates.push(PathBuf::from(home).join("Blackmagic DeckLink SDK 16.0"));
    }
    candidates.push(PathBuf::from(
        r"C:\Program Files\Blackmagic Design\Blackmagic DeckLink SDK 16.0",
    ));
    candidates.push(PathBuf::from(r"C:\Blackmagic DeckLink SDK 16.0"));
    candidates.push(PathBuf::from("/usr/src/decklink-sdk"));
    candidates.push(PathBuf::from("/opt/blackmagic/DeckLinkSDK"));

    candidates.into_iter().find(|path| looks_like_sdk(path))
}

fn looks_like_sdk(path: &Path) -> bool {
    path.join("Linux").join("include").join("DeckLinkAPI.h").exists()
        || path.join("Win").join("include").join("DeckLinkAPI.idl").exists()
        || path.join("Mac").join("include").join("DeckLinkAPI.h").exists()
}
