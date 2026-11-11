// Platform support for the Core TUI: terminal I/O, input, files, and notifications.
use std::path::Path;

/// Client presentation identity only; never a program/session authority.
pub(crate) fn client_terminal_identity() -> Option<String> {
    let mut name = [0u8; 1024];
    // ttyname_r writes into our buffer and has no borrowed libc global storage.
    if unsafe { libc::ttyname_r(libc::STDIN_FILENO, name.as_mut_ptr().cast(), name.len()) } != 0 {
        return None;
    }
    std::ffi::CStr::from_bytes_until_nul(&name)
        .ok()?
        .to_str()
        .ok()
        .map(str::to_owned)
}

pub(crate) fn read_fd(fd: std::os::fd::RawFd, data: &mut [u8]) -> std::io::Result<usize> {
    let result = unsafe { libc::read(fd, data.as_mut_ptr().cast(), data.len()) };
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(result as usize)
    }
}

/// The tty's erase character (VERASE) on stdin, when one is set.
pub(super) fn terminal_erase_byte() -> Option<u8> {
    let mut termios: libc::termios = unsafe { std::mem::zeroed() };
    if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut termios) } != 0 {
        return None;
    }
    let erase = termios.c_cc[libc::VERASE];
    // 0 (Linux) and 0xff (BSD) disable the special character.
    (erase != 0 && erase != 0xff).then_some(erase)
}

pub(crate) fn poll_fd_readable(fd: std::os::fd::RawFd, timeout_ms: i32) -> std::io::Result<bool> {
    let mut descriptor = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    let result = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
    if result < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(result > 0)
    }
}

pub(super) fn read_terminal_grid_size() -> std::io::Result<(u16, u16)> {
    crossterm::terminal::window_size().map(|size| (size.columns, size.rows))
}

pub(crate) fn create_remote_ssh_config_file(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;

    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

pub(crate) fn create_remote_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;

    std::fs::DirBuilder::new().mode(0o700).create(path)
}

#[cfg(test)]
mod tests {}
