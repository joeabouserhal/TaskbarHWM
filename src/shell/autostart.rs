//! "Start with Windows" via a Task Scheduler logon task that runs with the highest privileges.
//! A Run-key entry can't launch an elevated app silently; a scheduled task can, with no UAC prompt at logon.
use windows::core::{Result, BSTR};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
use windows::Win32::System::TaskScheduler::*;
use windows::Win32::System::Variant::VARIANT;

const TASK_NAME: &str = "TaskbarHWM";

/// Initializes COM on the current thread for the lifetime of the guard.
struct Com(bool);

impl Com {
    fn init() -> Com {
        // Err (RPC_E_CHANGED_MODE) means COM is already up on this thread in another mode: usable, not ours to undo.
        Com(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).is_ok() })
    }
}

impl Drop for Com {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

fn root_folder() -> Result<ITaskFolder> {
    unsafe {
        let svc: ITaskService = CoCreateInstance(&TaskScheduler, None, CLSCTX_INPROC_SERVER)?;
        svc.Connect(&VARIANT::default(), &VARIANT::default(), &VARIANT::default(), &VARIANT::default())?;
        svc.GetFolder(&BSTR::from("\\"))
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn task_xml(exe: &str, user: &str) -> String {
    let (exe, user) = (xml_escape(exe), xml_escape(user));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><Description>Starts TaskbarHWM when you sign in.</Description></RegistrationInfo>
  <Triggers>
    <LogonTrigger><Enabled>true</Enabled><UserId>{user}</UserId><Delay>PT3S</Delay></LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author"><UserId>{user}</UserId><LogonType>InteractiveToken</LogonType><RunLevel>HighestAvailable</RunLevel></Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>true</StartWhenAvailable>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author"><Exec><Command>{exe}</Command></Exec></Actions>
</Task>"#
    )
}

fn current_exe() -> Option<String> {
    std::env::current_exe().ok().map(|p| p.to_string_lossy().into_owned())
}

fn current_user() -> String {
    let user = std::env::var("USERNAME").unwrap_or_default();
    match std::env::var("USERDOMAIN") {
        Ok(d) if !d.is_empty() => format!("{d}\\{user}"),
        _ => user,
    }
}

/// True when the logon task exists.
pub fn is_enabled() -> bool {
    let _com = Com::init();
    // Bind the answer first: COM objects must be released before `_com` uninitializes COM. A temporary in a
    // tail expression would be dropped after it, which crashes.
    let exists = root_folder().and_then(|f| unsafe { f.GetTask(&BSTR::from(TASK_NAME)) }).is_ok();
    exists
}

/// Creates (or updates) the logon task for the running exe. Needs administrator rights.
pub fn enable() -> std::result::Result<(), String> {
    let exe = current_exe().ok_or("Couldn't find the app path")?;
    let _com = Com::init();
    let xml = task_xml(&exe, &current_user());
    let r = root_folder().and_then(|f| unsafe {
        f.RegisterTask(
            &BSTR::from(TASK_NAME),
            &BSTR::from(xml),
            TASK_CREATE_OR_UPDATE.0,
            &VARIANT::default(),
            &VARIANT::default(),
            TASK_LOGON_INTERACTIVE_TOKEN,
            &VARIANT::default(),
        )
    });
    r.map(|_| ()).map_err(|e| describe(&e))
}

pub fn disable() -> std::result::Result<(), String> {
    let _com = Com::init();
    let r = root_folder().and_then(|f| unsafe { f.DeleteTask(&BSTR::from(TASK_NAME), 0) });
    r.map_err(|e| describe(&e))
}

fn describe(e: &windows::core::Error) -> String {
    if e.code().0 as u32 == 0x8007_0005 {
        "Needs administrator rights".into()
    } else {
        let m = e.message();
        let m = m.trim();
        if m.is_empty() { format!("Failed ({:#x})", e.code().0) } else { m.chars().take(60).collect() }
    }
}

/// If the task exists but points at an old location (the exe was moved or updated), repoint it.
/// Runs at startup; only does anything when elevated and the task is already enabled.
pub fn heal() {
    let Some(exe) = current_exe() else { return };
    let _com = Com::init();
    let Ok(folder) = root_folder() else { return };
    let Ok(task) = (unsafe { folder.GetTask(&BSTR::from(TASK_NAME)) }) else { return };
    let Ok(xml) = (unsafe { task.Xml() }) else { return };
    if !xml.to_string().contains(&xml_escape(&exe)) {
        let _ = enable();
    }
}
