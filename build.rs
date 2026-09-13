use std::path::PathBuf;

fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=az_trainer.ico");
        println!("cargo:rerun-if-changed=build.rs");

        let ver = env!("CARGO_PKG_VERSION");
        let mut parts: Vec<u32> = ver.split('.').filter_map(|p| p.parse().ok()).collect();
        parts.resize(4, 0);
        let quad = format!("{},{},{},{}", parts[0], parts[1], parts[2], parts[3]);

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());

        // copy the icon beside the generated script so the .rc can name it
        // without any path, avoiding Windows backslash escaping entirely
        let ico_dst = out_dir.join("az_trainer.ico");
        std::fs::copy(root.join("az_trainer.ico"), &ico_dst).expect("copy icon");

        // icon + VERSIONINFO, so Explorer's Details tab tracks the crate version
        let rc = format!(
            r#"1 ICON "az_trainer.ico"

1 VERSIONINFO
FILEVERSION {quad}
PRODUCTVERSION {quad}
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904b0"
    BEGIN
      VALUE "ProductName", "AZ Trainer"
      VALUE "FileDescription", "AZ Trainer"
      VALUE "FileVersion", "{ver}"
      VALUE "ProductVersion", "{ver}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#
        );

        let rc_path = out_dir.join("app.rc");
        std::fs::write(&rc_path, rc).expect("write app.rc");
        embed_resource::compile(&rc_path, embed_resource::NONE)
            .manifest_optional()
            .unwrap();
    }
}
