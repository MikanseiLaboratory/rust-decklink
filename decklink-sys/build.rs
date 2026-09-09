use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=cxx/decklink_c.h");
    println!("cargo:rerun-if-changed=cxx/shim.cpp");
    println!("cargo:rerun-if-env-changed=DECKLINK_SDK_DIR");
    println!("cargo:rerun-if-env-changed=DECKLINK_FORCE_STUB");

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
            println!("cargo:rustc-link-lib=ole32");
            println!("cargo:rustc-link-lib=oleaut32");
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

    let midl = find_midl().ok_or("midl.exe not found; install Visual Studio C++ tools")?;
    let status = Command::new(midl)
        .args([
            "/nologo",
            "/W1",
            "/char",
            "signed",
            "/env",
            "x64",
            "/h",
            "DeckLinkAPI_h.h",
            "/iid",
            "DeckLinkAPI_i.c",
        ])
        .arg(&idl)
        .current_dir(&out)
        .status()
        .map_err(|err| err.to_string())?;
    if !status.success() {
        return Err("midl failed to compile DeckLinkAPI.idl".into());
    }
    Ok(out)
}

fn find_midl() -> Option<PathBuf> {
    if let Ok(path) = env::var("MIDL") {
        return Some(PathBuf::from(path));
    }
    which("midl.exe").or_else(|| which("midl"))
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
    candidates.push(PathBuf::from("/usr/src/decklink-sdk"));
    candidates.push(PathBuf::from("/opt/blackmagic/DeckLinkSDK"));

    candidates.into_iter().find(|path| looks_like_sdk(path))
}

fn looks_like_sdk(path: &Path) -> bool {
    path.join("Linux").join("include").join("DeckLinkAPI.h").exists()
        || path.join("Win").join("include").join("DeckLinkAPI.idl").exists()
        || path.join("Mac").join("include").join("DeckLinkAPI.h").exists()
}
