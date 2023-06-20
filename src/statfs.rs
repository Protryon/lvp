use std::{ffi::CString, os::unix::prelude::OsStrExt, path::Path};

use libc::statvfs64;

pub struct Stats {
    pub block_size: u64,
    pub fragment_size: u64,
    pub block_count: u64,
    pub blocks_free: u64,
    pub blocks_free_unprivileged: u64,
    pub inodes: u64,
    pub inodes_free: u64,
    pub inodes_free_unprivileged: u64,
    pub file_system_id: u64,
    pub flags: u64,
    pub filename_max_len: u64,
}

pub async fn statfs(path: &Path) -> std::io::Result<Stats> {
    let path: Vec<u8> = path.as_os_str().as_bytes().to_vec();
    tokio::task::spawn_blocking(move || {
        let path = CString::new(path).unwrap();
        let mut out: statvfs64 =
            unsafe { std::mem::transmute([0u8; std::mem::size_of::<statvfs64>()]) };
        let status = unsafe { libc::statvfs64(path.as_ptr(), (&mut out) as *mut statvfs64) };
        if status < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Stats {
            block_size: out.f_bsize,
            fragment_size: out.f_frsize,
            block_count: out.f_blocks,
            blocks_free: out.f_bfree,
            blocks_free_unprivileged: out.f_bavail,
            inodes: out.f_files,
            inodes_free: out.f_ffree,
            inodes_free_unprivileged: out.f_favail,
            file_system_id: out.f_fsid,
            flags: out.f_flag,
            filename_max_len: out.f_flag,
        })
    })
    .await
    .unwrap()
}
