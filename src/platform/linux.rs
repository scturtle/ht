#[cfg(test)]
use std::{io::Write, os::fd::RawFd, path::PathBuf};
// Platform support for the Core TUI: terminal I/O, input, files, and notifications.
use std::{
    process::{Command, Stdio},
    sync::OnceLock,
};

use super::ClipboardCommand;

mod clipboard;

/// On-demand, self-only process evidence; never samples other users' programs.
/// Missing procfs permissions are data, not a reason to discard the pane dump.
pub(crate) fn diagnostic_process_snapshot() -> serde_json::Value {
    use serde_json::json;
    let read = |path: &std::path::Path| match std::fs::read_to_string(path) {
        Ok(text) => json!({"text":text}),
        Err(error) => json!({"error":error.to_string()}),
    };
    let threads = match std::fs::read_dir("/proc/self/task") {
        Ok(entries) => serde_json::Value::Array(
            entries
                .filter_map(Result::ok)
                .take(128)
                .map(|entry| {
                    let path = entry.path();
                    json!({"tid":entry.file_name().to_string_lossy(),
                "name":read(&path.join("comm")),"wait_channel":read(&path.join("wchan")),
                "status":read(&path.join("status"))})
                })
                .collect(),
        ),
        Err(error) => json!({"error":error.to_string()}),
    };
    json!({"pid":std::process::id(),"status":read(std::path::Path::new("/proc/self/status")),"threads":threads,"thread_limit":128})
}

pub(crate) use super::unix_common::{create_remote_private_dir, create_remote_ssh_config_file};

#[cfg(test)]
mod clipboard_tests;

#[cfg(test)]
mod config_file_tests;

const WSL_MARKER_ENV_VARS: &[&str] = &["WSL_DISTRO_NAME", "WSL_INTEROP"];

pub(crate) fn should_draw_host_cursor_by_default() -> bool {
    running_inside_wsl()
}

pub(crate) fn should_query_host_terminal_palette() -> bool {
    !running_inside_wsl()
}

fn running_inside_wsl() -> bool {
    static RUNNING_INSIDE_WSL: OnceLock<bool> = OnceLock::new();
    *RUNNING_INSIDE_WSL.get_or_init(detect_running_inside_wsl)
}

fn detect_running_inside_wsl() -> bool {
    proc_file_indicates_wsl("/proc/sys/kernel/osrelease")
        || proc_file_indicates_wsl("/proc/version")
        || WSL_MARKER_ENV_VARS
            .iter()
            .any(|key| std::env::var_os(key).is_some())
        || std::path::Path::new("/run/WSL").exists()
}

fn proc_file_indicates_wsl(path: &str) -> bool {
    std::fs::read_to_string(path)
        .map(|text| text_indicates_wsl(&text))
        .unwrap_or(false)
}

fn text_indicates_wsl(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("microsoft") || text.contains("wsl")
}

pub fn write_clipboard(bytes: &[u8]) -> bool {
    let deadline = std::time::Instant::now() + clipboard::TIMEOUT;
    for command in clipboard_commands() {
        if run_clipboard_command_until(&command, bytes, deadline) {
            return true;
        }
    }
    false
}

pub fn read_clipboard_text() -> Option<String> {
    let deadline = std::time::Instant::now() + clipboard::TIMEOUT;
    for command in read_clipboard_text_commands() {
        if let Some(text) = read_clipboard_text_with_command_until(&command, deadline) {
            return Some(text);
        }
    }
    None
}

pub fn clipboard_text_matches(_bytes: &[u8]) -> Option<bool> {
    None
}

pub fn open_url(url: &str) -> std::io::Result<Option<std::process::Child>> {
    Command::new("xdg-open")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(Some)
}

pub fn show_desktop_notification(title: &str, body: Option<&str>) -> std::io::Result<bool> {
    show_desktop_notification_with_command(title, body, |program| Command::new(program))
}

fn show_desktop_notification_with_command(
    title: &str,
    body: Option<&str>,
    mut command: impl FnMut(&str) -> Command,
) -> std::io::Result<bool> {
    if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return Ok(false);
    }

    let mut cmd = command("notify-send");
    cmd.arg("--app-name").arg("Herdr").arg("--").arg(title);
    if let Some(body) = body.filter(|body| !body.is_empty()) {
        cmd.arg(body);
    }
    run_notification_command(cmd)
}

fn run_notification_command(mut command: Command) -> std::io::Result<bool> {
    let status = match command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
    {
        Ok(status) => status,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };

    Ok(status.success())
}

fn clipboard_commands() -> Vec<ClipboardCommand> {
    let mut commands = Vec::new();

    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        commands.push(ClipboardCommand {
            program: "wl-copy",
            args: &["--type", "text/plain;charset=utf-8"],
        });
    }

    if std::env::var_os("DISPLAY").is_some() {
        commands.push(ClipboardCommand {
            program: "xclip",
            args: &["-selection", "clipboard", "-in"],
        });
        commands.push(ClipboardCommand {
            program: "xsel",
            args: &["--clipboard", "--input"],
        });
    }

    commands
}

fn read_clipboard_text_commands() -> Vec<ClipboardCommand> {
    let mut commands = Vec::new();

    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        commands.push(ClipboardCommand {
            program: "wl-paste",
            args: &["--type", "text/plain;charset=utf-8"],
        });
        commands.push(ClipboardCommand {
            program: "wl-paste",
            args: &["--type", "text/plain"],
        });
    }

    if std::env::var_os("DISPLAY").is_some() {
        commands.push(ClipboardCommand {
            program: "xclip",
            args: &["-selection", "clipboard", "-out"],
        });
        commands.push(ClipboardCommand {
            program: "xsel",
            args: &["--clipboard", "--output"],
        });
    }

    commands
}

fn read_clipboard_text_with_command_until(
    command: &ClipboardCommand,
    deadline: std::time::Instant,
) -> Option<String> {
    const MAX_CLIPBOARD_TEXT_BYTES: usize = 1024 * 1024;
    let mut process = Command::new(command.program);
    process.args(command.args);
    let bytes = clipboard::run(process, None, MAX_CLIPBOARD_TEXT_BYTES, deadline).ok()?;
    if bytes.is_empty() {
        return None;
    }
    String::from_utf8(bytes).ok()
}

fn run_clipboard_command_until(
    command: &ClipboardCommand,
    bytes: &[u8],
    deadline: std::time::Instant,
) -> bool {
    let mut process = Command::new(command.program);
    process.args(command.args);
    clipboard::run(process, Some(bytes), 0, deadline).is_ok()
}

#[cfg(test)]
fn run_clipboard_command(command: &ClipboardCommand, bytes: &[u8]) -> bool {
    run_clipboard_command_until(
        command,
        bytes,
        std::time::Instant::now() + clipboard::TIMEOUT,
    )
}

#[cfg(test)]
pub fn process_exists(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    let result = unsafe { libc::kill(pid as i32, 0) };
    if result == 0 {
        true
    } else {
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
    }
}

#[cfg(test)]
pub(crate) fn create_config_temporary(
    path: &std::path::Path,
    private: bool,
) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(if private { 0o600 } else { 0o666 })
        .open(path)
}

#[cfg(test)]
pub(crate) fn write_config_temporary(
    source: Option<&std::path::Path>,
    temporary: &std::path::Path,
    contents: &[u8],
) -> std::io::Result<()> {
    use std::os::{fd::AsRawFd, unix::fs::MetadataExt};
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(temporary)?;
    if let Some(source) = source {
        let input = std::fs::File::open(source)?;
        let metadata = input.metadata()?;
        let current = output.metadata()?;
        if (metadata.uid(), metadata.gid()) != (current.uid(), current.gid()) {
            // Keep ownership before restoring mode/ACLs; chown can clear mode bits.
            if unsafe { libc::fchown(output.as_raw_fd(), metadata.uid(), metadata.gid()) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        // Replace inherited ACLs before enabling the original mode. Prepare all
        // access controls while the temporary is empty, before writing secrets.
        copy_config_xattrs(input.as_raw_fd(), output.as_raw_fd())?;
        output.set_permissions(metadata.permissions())?;
    }
    output.write_all(contents)?;
    output.sync_all()
}

#[cfg(test)]
// Access ACLs and security labels live in xattrs on Linux. Mode bits alone can
// silently broaden access, especially with a default ACL on the parent directory.
fn copy_config_xattrs(source: RawFd, destination: RawFd) -> std::io::Result<()> {
    use std::ffi::CStr;
    fn names(fd: RawFd) -> std::io::Result<Vec<u8>> {
        let size = unsafe { libc::flistxattr(fd, std::ptr::null_mut(), 0) };
        if size < 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENOTSUP) {
                return Ok(Vec::new());
            }
            return Err(error);
        }
        let mut buffer = vec![0; size as usize];
        let read = unsafe { libc::flistxattr(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if read < 0 {
            return Err(std::io::Error::last_os_error());
        }
        buffer.truncate(read as usize);
        Ok(buffer)
    }
    fn value(fd: RawFd, name: &CStr) -> std::io::Result<Vec<u8>> {
        let size = unsafe { libc::fgetxattr(fd, name.as_ptr(), std::ptr::null_mut(), 0) };
        if size < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let mut buffer = vec![0; size as usize];
        let read =
            unsafe { libc::fgetxattr(fd, name.as_ptr(), buffer.as_mut_ptr().cast(), buffer.len()) };
        if read < 0 {
            return Err(std::io::Error::last_os_error());
        }
        buffer.truncate(read as usize);
        Ok(buffer)
    }
    let source_names = names(source)?;
    for bytes in names(destination)?.split_inclusive(|byte| *byte == 0) {
        if !source_names
            .split_inclusive(|byte| *byte == 0)
            .any(|name| name == bytes)
        {
            let name = CStr::from_bytes_with_nul(bytes).map_err(std::io::Error::other)?;
            if unsafe { libc::fremovexattr(destination, name.as_ptr()) } != 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
    }
    for bytes in source_names.split_inclusive(|byte| *byte == 0) {
        let name = CStr::from_bytes_with_nul(bytes).map_err(std::io::Error::other)?;
        let original = value(source, name)?;
        // Avoid requiring relabel privileges when the inherited label already matches.
        if value(destination, name).is_ok_and(|current| current == original) {
            continue;
        }
        if unsafe {
            libc::fsetxattr(
                destination,
                name.as_ptr(),
                original.as_ptr().cast(),
                original.len(),
                0,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(test)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }
    #[test]
    fn wsl_marker_detection_matches_kernel_release_text() {
        assert!(text_indicates_wsl("5.15.167.4-microsoft-standard-WSL2"));
        assert!(text_indicates_wsl("4.4.0-19041-Microsoft"));
        assert!(!text_indicates_wsl("6.8.0-64-generic"));
        assert!(!text_indicates_wsl(""));
    }
    #[test]
    fn clipboard_commands_prefer_wayland_when_available() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::set_var("WAYLAND_DISPLAY", "wayland-0");
            std::env::remove_var("DISPLAY");
        }
        let commands = clipboard_commands();
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].program, "wl-copy");
    }
    #[test]
    fn failed_wl_copy_uses_x11_fallback() {
        use std::ffi::OsString;
        use std::os::unix::fs::PermissionsExt;
        use std::path::PathBuf;
        use std::time::{SystemTime, UNIX_EPOCH};

        struct Cleanup {
            old_path: Option<OsString>,
            old_wayland_display: Option<OsString>,
            old_display: Option<OsString>,
            temp_dir: PathBuf,
        }

        impl Drop for Cleanup {
            fn drop(&mut self) {
                unsafe {
                    match self.old_path.take() {
                        Some(value) => std::env::set_var("PATH", value),
                        None => std::env::remove_var("PATH"),
                    }
                    match self.old_wayland_display.take() {
                        Some(value) => std::env::set_var("WAYLAND_DISPLAY", value),
                        None => std::env::remove_var("WAYLAND_DISPLAY"),
                    }
                    match self.old_display.take() {
                        Some(value) => std::env::set_var("DISPLAY", value),
                        None => std::env::remove_var("DISPLAY"),
                    }
                    std::env::remove_var("HERDR_TEST_XCLIP_PAYLOAD");
                }
                let _ = std::fs::remove_dir_all(&self.temp_dir);
            }
        }

        let _guard = env_lock().lock().unwrap();
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time should follow unix epoch")
            .as_nanos();
        let temp_dir = std::env::temp_dir().join(format!(
            "herdr-failed-wl-copy-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&temp_dir).expect("temp dir should be created");
        let cleanup = Cleanup {
            old_path: std::env::var_os("PATH"),
            old_wayland_display: std::env::var_os("WAYLAND_DISPLAY"),
            old_display: std::env::var_os("DISPLAY"),
            temp_dir: temp_dir.clone(),
        };
        let payload = temp_dir.join("xclip-payload");
        let fake_wl_copy = temp_dir.join("wl-copy");
        let fake_xclip = temp_dir.join("xclip");
        std::fs::write(&fake_wl_copy, "#!/bin/sh\n/bin/cat >/dev/null\nexit 7\n")
            .expect("fake wl-copy should be written");
        std::fs::write(
            &fake_xclip,
            "#!/bin/sh\n/bin/cat > \"$HERDR_TEST_XCLIP_PAYLOAD\"\n",
        )
        .expect("fake xclip should be written");
        for command in [&fake_wl_copy, &fake_xclip] {
            let mut permissions = std::fs::metadata(command)
                .expect("fake clipboard command metadata")
                .permissions();
            permissions.set_mode(0o700);
            std::fs::set_permissions(command, permissions)
                .expect("fake clipboard command should be executable");
        }

        unsafe {
            std::env::set_var("PATH", &temp_dir);
            std::env::set_var("WAYLAND_DISPLAY", "wayland-0");
            std::env::set_var("DISPLAY", ":0");
            std::env::set_var("HERDR_TEST_XCLIP_PAYLOAD", &payload);
        }

        assert!(write_clipboard(b"clipboard fallback"));
        assert_eq!(
            std::fs::read(&payload).expect("xclip should record stdin"),
            b"clipboard fallback"
        );
        drop(cleanup);
    }
    #[test]
    fn finite_clipboard_commands_report_exit_status() {
        let success = ClipboardCommand {
            program: "sh",
            args: &["-c", "cat >/dev/null"],
        };
        let failure = ClipboardCommand {
            program: "sh",
            args: &["-c", "cat >/dev/null; exit 7"],
        };

        assert!(run_clipboard_command(&success, b"clipboard text"));
        assert!(!run_clipboard_command(&failure, b"clipboard text"));
    }
    #[test]
    fn clipboard_commands_include_x11_fallbacks() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::remove_var("WAYLAND_DISPLAY");
            std::env::set_var("DISPLAY", ":0");
        }
        let commands = clipboard_commands();
        assert_eq!(commands.len(), 2);
        assert_eq!(commands[0].program, "xclip");
        assert_eq!(commands[1].program, "xsel");
    }
    #[test]
    fn read_clipboard_text_commands_include_session_backends() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::set_var("WAYLAND_DISPLAY", "wayland-0");
            std::env::set_var("DISPLAY", ":0");
        }

        let commands = read_clipboard_text_commands();
        assert_eq!(commands[0].program, "wl-paste");
        assert_eq!(commands[1].program, "wl-paste");
        assert_eq!(commands[2].program, "xclip");
        assert_eq!(commands[3].program, "xsel");
    }
    #[test]
    fn desktop_notification_sets_app_name_and_separates_option_like_titles() {
        let _guard = env_lock().lock().unwrap();
        unsafe {
            std::env::remove_var("WAYLAND_DISPLAY");
            std::env::set_var("DISPLAY", ":0");
        }

        let path =
            std::env::temp_dir().join(format!("herdr-notify-send-args-{}", std::process::id()));
        let script = "printf '%s\\n' \"$@\" > \"$HERDR_NOTIFY_ARGS\"";
        let shown = show_desktop_notification_with_command("-danger", Some("body"), |_| {
            let mut cmd = Command::new("sh");
            cmd.arg("-c")
                .arg(script)
                .arg("notify-send")
                .env("HERDR_NOTIFY_ARGS", &path);
            cmd
        })
        .expect("notification command should run");

        assert!(shown);
        let args = std::fs::read_to_string(&path).expect("args file");
        let _ = std::fs::remove_file(&path);
        assert_eq!(args, "--app-name\nHerdr\n--\n-danger\nbody\n");
    }
}
