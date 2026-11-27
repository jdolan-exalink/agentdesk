// Shared Windows resource generator for every AgentDesk executable.
//
// Included from each app's build.rs with `include!`. It writes an .rc file and
// an application manifest into OUT_DIR, filling the version from the crate's
// Cargo version, and compiles them with embed-resource. A VERSIONINFO block and
// a manifest give Windows (Defender, SmartScreen, Task Manager) a declared
// publisher, product and version instead of an anonymous binary.
//
// Apps built on gpui (desktop-gpui, quicksupport, server-console) must pass
// `embed_manifest = false`: gpui already embeds its own manifest (asInvoker,
// DPI awareness, Common Controls v6) as resource 1, and a second one would clash.

#[allow(dead_code)]
const AGENTDESK_COMPANY: &str = "Dolan";
#[allow(dead_code)]
const AGENTDESK_PRODUCT: &str = "AgentDesk";

#[allow(dead_code)]
fn embed_windows_resources(icon_rel: &str, exe_name: &str, description: &str, embed_manifest: bool) {
    use std::path::PathBuf;

    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let icon = manifest_dir.join(icon_rel);
    println!("cargo:rerun-if-changed={}", icon.display());
    println!("cargo:rerun-if-changed=../../build-support/winres.rs");
    println!("cargo:rerun-if-changed=build.rs");

    // Only Windows targets get resources (host and target can differ when cross-compiling).
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let major: u32 = env!("CARGO_PKG_VERSION_MAJOR").parse().unwrap_or(0);
    let minor: u32 = env!("CARGO_PKG_VERSION_MINOR").parse().unwrap_or(0);
    let patch: u32 = env!("CARGO_PKG_VERSION_PATCH").parse().unwrap_or(0);
    let ver = format!("{major}.{minor}.{patch}.0");
    let year = 2026;
    let ident = format!("{AGENTDESK_COMPANY}.{AGENTDESK_PRODUCT}.{exe_name}");
    let fwd = |p: &std::path::Path| p.display().to_string().replace('\\', "/");

    let manifest = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="{ident}" version="{ver}" processorArchitecture="*"/>
  <description>{description}</description>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security>
      <requestedPrivileges>
        <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
      </requestedPrivileges>
    </security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
    </application>
  </compatibility>
</assembly>
"#
    );
    let manifest_path = out_dir.join("app.manifest");
    std::fs::write(&manifest_path, manifest).unwrap();

    let rc = format!(
        r#"1 ICON "{icon}"
{manifest_line}

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3fL
FILEFLAGS 0x0L
FILEOS 0x40004L
FILETYPE 0x1L
FILESUBTYPE 0x0L
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "{AGENTDESK_COMPANY}"
      VALUE "FileDescription", "{description}"
      VALUE "FileVersion", "{ver}"
      VALUE "InternalName", "{exe_name}"
      VALUE "LegalCopyright", "Copyright (C) {year} {AGENTDESK_COMPANY}"
      VALUE "OriginalFilename", "{exe_name}.exe"
      VALUE "ProductName", "{AGENTDESK_PRODUCT}"
      VALUE "ProductVersion", "{ver}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = fwd(&icon),
        manifest_line = if embed_manifest {
            format!("1 24 \"{}\"", fwd(&manifest_path))
        } else {
            String::new()
        },
    );
    let rc_path = out_dir.join("agentdesk.rc");
    std::fs::write(&rc_path, rc).unwrap();

    embed_resource::compile(&rc_path, embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
