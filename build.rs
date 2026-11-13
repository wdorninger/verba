//! Copies the OpenVINO GenAI runtime (GPU speech recognition) and the VC++ runtime it needs into
//! target\<profile>\openvino, so every built verba.exe can use the GPU, not only the packaged one.
//! The runtime comes from setup-openvino.ps1 (%LOCALAPPDATA%\transcribe-build); without it the build still
//! works and the app runs on the CPU backend.

use std::fs;
use std::path::{Path, PathBuf};

const OPENVINO_DLLS: &[&str] = &[
    "openvino_genai_c.dll",
    "openvino_genai.dll",
    "openvino_c.dll",
    "openvino.dll",
    "openvino_intel_gpu_plugin.dll",
    "openvino_intel_cpu_plugin.dll",
    "openvino_ir_frontend.dll",
    "openvino_tokenizers.dll",
    "cache.json",
];
const CRT_DLLS: &[&str] = &[
    "msvcp140.dll",
    "msvcp140_1.dll",
    "msvcp140_2.dll",
    "msvcp140_codecvt_ids.dll",
    "vcruntime140.dll",
    "vcruntime140_1.dll",
];

/// Subdirectories of `dir` whose names start with `prefix`, sorted (so the last one is the newest version).
fn dirs(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|r| r.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    v.retain(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(prefix)));
    v.sort();
    v
}

/// ...\VC\Redist\MSVC\<version>\x64\Microsoft.VC14x.CRT of the newest Visual Studio found.
fn crt_dir() -> Option<PathBuf> {
    let mut found = Vec::new();
    for root in [r"C:\Program Files\Microsoft Visual Studio", r"C:\Program Files (x86)\Microsoft Visual Studio"] {
        for version in dirs(Path::new(root), "") {
            for edition in dirs(&version, "") {
                for msvc in dirs(&edition.join(r"VC\Redist\MSVC"), "") {
                    found.extend(dirs(&msvc.join("x64"), "Microsoft.VC14").into_iter().filter(|d| d.to_string_lossy().ends_with(".CRT")));
                }
            }
        }
    }
    found.sort();
    found.pop()
}

/// Copies a file unless an identical-size copy is already there (keeps incremental builds fast).
fn copy(from: &Path, to: &Path) {
    let same = fs::metadata(from).ok().zip(fs::metadata(to).ok()).is_some_and(|(a, b)| a.len() == b.len());
    if !same {
        if let Err(e) = fs::copy(from, to) {
            println!("cargo:warning=could not copy {}: {e}", from.display());
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let Some(local) = std::env::var_os("LOCALAPPDATA") else { return };
    let build = PathBuf::from(local).join("transcribe-build");
    println!("cargo:rerun-if-changed={}", build.display());
    let Some(genai) = dirs(&build, "openvino_genai_windows_").pop() else {
        println!("cargo:warning=OpenVINO GenAI not set up (run setup-openvino.ps1): the app will only use the CPU backend");
        return;
    };
    // OUT_DIR = target\<profile>\build\transcribe-<hash>\out
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let Some(profile_dir) = out.ancestors().nth(3) else { return };
    let dest = profile_dir.join("openvino");
    let _ = fs::create_dir_all(&dest);
    let bin = genai.join(r"runtime\bin\intel64\Release");
    for f in OPENVINO_DLLS {
        copy(&bin.join(f), &dest.join(f));
    }
    copy(&genai.join(r"runtime\3rdparty\tbb\bin\tbb12.dll"), &dest.join("tbb12.dll"));
    match crt_dir() {
        Some(crt) => {
            for f in CRT_DLLS {
                copy(&crt.join(f), &dest.join(f));
            }
        }
        None => println!("cargo:warning=Visual C++ redistributable folder not found; OpenVINO may not load"),
    }
}
