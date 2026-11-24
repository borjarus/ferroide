//! Embeds the built-in Language Packs (`languages/` in the repository):
//! every `language.toml` and `.scm` file, so the binary needs no data files.

use std::{env, fmt::Write as _, fs, path::PathBuf};

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let languages = manifest_dir.join("../../languages");
    println!("cargo:rerun-if-changed={}", languages.display());

    let mut packs: Vec<PathBuf> = fs::read_dir(&languages)
        .expect("languages/ folder")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("language.toml").is_file())
        .collect();
    packs.sort();

    let mut out =
        String::from("/// `(pack id, [(file name, contents)])` of every built-in pack.\n");
    out.push_str("pub fn packs() -> &'static [(&'static str, &'static [(&'static str, &'static str)])] {\n    &[\n");
    for pack in packs {
        println!("cargo:rerun-if-changed={}", pack.display());
        let id = pack.file_name().unwrap().to_string_lossy().into_owned();
        let mut files: Vec<PathBuf> = fs::read_dir(&pack)
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e == "scm" || e == "toml"))
            .collect();
        files.sort();
        writeln!(out, "        ({id:?}, &[").unwrap();
        for file in files {
            println!("cargo:rerun-if-changed={}", file.display());
            let name = file.file_name().unwrap().to_string_lossy().into_owned();
            writeln!(
                out,
                "            ({name:?}, include_str!({:?})),",
                file.display().to_string()
            )
            .unwrap();
        }
        out.push_str("        ]),\n");
    }
    out.push_str("    ]\n}\n");
    let dest = PathBuf::from(env::var("OUT_DIR").unwrap()).join("builtin_packs.rs");
    fs::write(dest, out).unwrap();
}
