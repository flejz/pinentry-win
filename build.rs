use embed_manifest::{embed_manifest, new_manifest};
use embed_manifest::manifest::DpiAwareness;

fn main() {
    if std::env::var_os("CARGO_CFG_TARGET_OS").as_deref() == Some(std::ffi::OsStr::new("windows")) {
        embed_manifest(
            new_manifest("pinentry-windows")
                .dpi_awareness(DpiAwareness::PerMonitorV2)
                // activeCodePage UTF-8 and ComCtl32 v6 visual styles are
                // included by the embed-manifest crate's default manifest.
        )
        .expect("unable to embed manifest");
        println!("cargo:rerun-if-changed=build.rs");
    }
}
