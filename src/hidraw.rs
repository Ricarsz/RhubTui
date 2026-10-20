//! Thin wrapper around Linux `hidraw`.
//!
//! No libhidapi dependency: hidraw nodes are plain character devices. Reports are
//! written with `write(2)` and read with `read(2)`; feature reports need one
//! `ioctl(2)`.

use std::ffi::{c_int, c_ulong};
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;

const O_NONBLOCK: i32 = 0o4000;
const O_CLOEXEC: i32 = 0o2000000;

unsafe extern "C" {
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}

const fn ioc(dir: u64, ty: u8, nr: u8, size: usize) -> u64 {
    (dir << 30) | ((size as u64) << 16) | ((ty as u64) << 8) | (nr as u64)
}

/// `HIDIOCGFEATURE(len)` — `_IOWR('H', 0x07, len)`.
fn hid_get_feature_ioctl(len: usize) -> c_ulong {
    ioc(3, b'H', 0x07, len) as c_ulong
}

/// Open a hidraw node non-blocking. `write` requires permissions on the node.
pub fn open(path: &str, write: bool) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(write)
        .custom_flags(O_NONBLOCK | O_CLOEXEC)
        .open(path)
}

/// Read one report. Returns `Ok(None)` when no report is queued.
///
/// For devices with numbered reports the first returned byte is the report ID.
pub fn read_packet(file: &File, buf: &mut [u8]) -> io::Result<Option<usize>> {
    let mut f = file;
    match f.read(buf) {
        Ok(0) => Ok(None),
        Ok(n) => Ok(Some(n)),
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(None),
        Err(e) => Err(e),
    }
}

/// Write one report. The buffer must already contain the report ID (if any).
pub fn write_packet(file: &File, packet: &[u8]) -> io::Result<()> {
    let mut f = file;
    f.write_all(packet)
}

/// Fetch a feature report payload (report ID excluded from the result).
pub fn get_feature(file: &File, report_id: u8, payload_len: usize) -> io::Result<Vec<u8>> {
    let len = payload_len + 1;
    let mut buf = vec![0u8; len];
    buf[0] = report_id;

    let ret = unsafe {
        ioctl(
            file.as_raw_fd(),
            hid_get_feature_ioctl(len),
            buf.as_mut_ptr(),
        )
    };
    if ret < 0 {
        return Err(io::Error::last_os_error());
    }
    let n = (ret as usize).clamp(1, len);
    Ok(buf[1..n].to_vec())
}
