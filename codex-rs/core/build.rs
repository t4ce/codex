#[path = "build_support/trueos_shell3_catalog.rs"]
mod trueos_shell3_catalog;

fn main() {
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"))
        .join("trueos_shell3_catalog.rs");

    if target_os == "trueos" {
        let manifest_dir = std::path::PathBuf::from(
            std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"),
        );
        let default_registry = manifest_dir.join("../../../TRUEOS/src/shell3/names.rs");
        let registry = std::env::var_os("CODEX_TRUEOS_SHELL3_REGISTRY")
            .map(std::path::PathBuf::from)
            .unwrap_or(default_registry);
        println!("cargo:rerun-if-env-changed=CODEX_TRUEOS_SHELL3_REGISTRY");
        println!("cargo:rerun-if-changed={}", registry.display());
        trueos_shell3_catalog::generate(&registry, &output)
            .unwrap_or_else(|error| panic!("failed to generate TRUEOS Shell3 catalog: {error}"));
    } else {
        std::fs::write(
            &output,
            "pub const SHELL3_GROUPS: &[(&str, &str, &[&str])] = &[];\n",
        )
        .expect("write empty host Shell3 catalog");
    }
}
