// Embeds the app manifest. Release builds demand admin rights (needed for the
// PawnIO CPU temperature/power driver); debug builds run as the invoking user
// so `cargo run` works from a normal terminal.
fn main() {
    println!("cargo:rerun-if-env-changed=TASKBARHWM_ASINVOKER");
    // TASKBARHWM_ASINVOKER=1 lets a release build run unelevated for local profiling.
    let release = std::env::var("PROFILE").map(|p| p == "release").unwrap_or(false)
        && std::env::var_os("TASKBARHWM_ASINVOKER").is_none();
    let level = if release { "requireAdministrator" } else { "asInvoker" };
    let manifest = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges>
      <requestedExecutionLevel level="{level}" uiAccess="false"/>
    </requestedPrivileges></security>
  </trustInfo>
  <compatibility xmlns="urn:schemas-microsoft-com:compatibility.v1">
    <application>
      <supportedOS Id="{{8e0f7a12-bfb3-4fe8-b9a5-48fd50a15a9a}}"/>
    </application>
  </compatibility>
  <application xmlns="urn:schemas-microsoft-com:asm.v3">
    <windowsSettings>
      <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
      <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
    </windowsSettings>
  </application>
</assembly>"#
    );
    println!("cargo:rerun-if-changed=res/app.ico");
    let mut res = winresource::WindowsResource::new();
    res.set_manifest(&manifest);
    // Icon (generate with scripts/make-icon.ps1) and version info shown in Explorer / Task Manager.
    if std::path::Path::new("res/app.ico").exists() {
        res.set_icon("res/app.ico");
    }
    res.set("ProductName", "TaskbarHWM");
    res.set("FileDescription", "TaskbarHWM - hardware monitor for the Windows taskbar");
    res.set("InternalName", "TaskbarHWM");
    res.set("OriginalFilename", "TaskbarHWM.exe");
    res.compile().expect("failed to embed manifest");
}
