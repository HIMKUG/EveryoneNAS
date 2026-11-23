//! Win32 Volume GUID (`\\?\Volume{GUID}\`) 与当前盘符/挂载路径互查解析模块。
//!
//! 职责：防止外接移动硬盘重插拔或系统重启后盘符漂移（如 `D:\` -> `E:\`），
//! 服务启动时通过 Volume GUID 反查当前挂载盘符并重新定位存储空间。

use std::path::{Path, PathBuf};

use anyhow::{bail, Result};

/// Windows 卷标识与当前挂载路径映射信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeInfo {
    /// Win32 唯一卷名，格式固定为 `\\?\Volume{xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx}\`。
    pub volume_guid: String,
    /// 当前系统分配的全部盘符或 NTFS 挂载点（如 `["E:\\"]`）。
    pub mount_paths: Vec<PathBuf>,
}

/// 校验字符串是否符合 Win32 Volume GUID 规范前缀。
pub fn is_valid_volume_guid(guid: &str) -> bool {
    let Some(inner) = guid
        .strip_prefix(r"\\?\Volume{")
        .and_then(|value| value.strip_suffix(r"}\"))
    else {
        return false;
    };
    inner.len() == 36
        && inner
            .bytes()
            .enumerate()
            .all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => byte == b'-',
                _ => byte.is_ascii_hexdigit(),
            })
}

/// RAII 守卫：托管 `FindFirstVolumeW` 返回的 Win32 句柄，确保任意错误早退路径都自动调用 `FindVolumeClose`。
#[cfg(windows)]
struct VolumeFindHandle(windows_sys::Win32::Foundation::HANDLE);

#[cfg(windows)]
impl Drop for VolumeFindHandle {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Storage::FileSystem::FindVolumeClose(self.0);
        }
    }
}

/// 枚举本机全部在线卷的 Volume GUID 及其当前盘符列表。
#[cfg(windows)]
pub fn enumerate_volumes() -> Result<Vec<VolumeInfo>> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;

    use windows_sys::Win32::Foundation::{ERROR_NO_MORE_FILES, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{FindFirstVolumeW, FindNextVolumeW};

    let mut buffer = vec![0u16; 512];
    let raw_handle = unsafe { FindFirstVolumeW(buffer.as_mut_ptr(), buffer.len() as u32) };
    if raw_handle == INVALID_HANDLE_VALUE {
        return Err(std::io::Error::last_os_error().into());
    }
    let find_handle = VolumeFindHandle(raw_handle);

    let mut volumes = Vec::new();
    loop {
        let nul_pos = buffer.iter().position(|&c| c == 0).unwrap_or(buffer.len());
        let volume_guid = OsString::from_wide(&buffer[..nul_pos])
            .to_string_lossy()
            .into_owned();
        let mount_paths = match mount_paths_for_guid(&volume_guid) {
            Ok(paths) => paths,
            Err(err) => {
                tracing::warn!(
                    volume_guid = %volume_guid,
                    "skipping volume whose mount points cannot be queried: {err}"
                );
                Vec::new()
            }
        };
        volumes.push(VolumeInfo {
            volume_guid,
            mount_paths,
        });

        buffer.fill(0);
        let ok = unsafe { FindNextVolumeW(find_handle.0, buffer.as_mut_ptr(), buffer.len() as u32) };
        if ok == 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                break;
            }
            return Err(err.into());
        }
    }

    Ok(volumes)
}

/// 根据当前盘符或挂载目录（如 `D:\`）查询其绑定的 Win32 Volume GUID。
/// 保留为运维/诊断入口（`lanvault-win-svc volumes` 之外的调用方），不在服务主循环内使用。
#[cfg(windows)]
#[allow(dead_code)]
pub fn guid_for_mount_path(mount_path: &Path) -> Result<String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    use windows_sys::Win32::Storage::FileSystem::GetVolumeNameForVolumeMountPointW;

    // Win32 要求挂载点路径必须以反斜杠 `\` 结尾。
    let mut normalized = mount_path.as_os_str().to_string_lossy().into_owned();
    if !normalized.ends_with('\\') {
        normalized.push('\\');
    }

    let wide_mount: Vec<u16> = OsString::from(&normalized)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut out_buf = vec![0u16; 128];

    let ok = unsafe {
        GetVolumeNameForVolumeMountPointW(
            wide_mount.as_ptr(),
            out_buf.as_mut_ptr(),
            out_buf.len() as u32,
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error().into());
    }

    let nul_pos = out_buf.iter().position(|&c| c == 0).unwrap_or(out_buf.len());
    Ok(OsString::from_wide(&out_buf[..nul_pos])
        .to_string_lossy()
        .into_owned())
}

/// 根据 Win32 Volume GUID（`\\?\Volume{GUID}\`）反查当前系统挂载盘符。
#[cfg(windows)]
pub fn mount_paths_for_guid(volume_guid: &str) -> Result<Vec<PathBuf>> {
    use std::ffi::OsString;
    use std::os::windows::ffi::{OsStrExt, OsStringExt};

    use windows_sys::Win32::Foundation::ERROR_MORE_DATA;
    use windows_sys::Win32::Storage::FileSystem::GetVolumePathNamesForVolumeNameW;

    if !is_valid_volume_guid(volume_guid) {
        bail!("invalid Win32 Volume GUID format: `{volume_guid}`");
    }

    let wide_guid: Vec<u16> = OsString::from(volume_guid)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut buf_len: u32 = 512;
    let mut buf = vec![0u16; buf_len as usize];
    let mut return_len: u32 = 0;

    loop {
        let ok = unsafe {
            GetVolumePathNamesForVolumeNameW(
                wide_guid.as_ptr(),
                buf.as_mut_ptr(),
                buf_len,
                &mut return_len,
            )
        };
        if ok != 0 {
            break;
        }
        let err = std::io::Error::last_os_error();
        if err.raw_os_error() == Some(ERROR_MORE_DATA as i32) && return_len > buf_len {
            buf_len = return_len;
            buf.resize(buf_len as usize, 0);
            continue;
        }
        return Err(err.into());
    }

    // 返回值为双 NUL 结尾的多字符串列表 (`E:\\\0F:\\mount\\\0\0`)。
    let mut paths = Vec::new();
    for segment in buf[..return_len as usize].split(|&c| c == 0) {
        if segment.is_empty() {
            continue;
        }
        paths.push(PathBuf::from(OsString::from_wide(segment)));
    }
    Ok(paths)
}

#[cfg(not(windows))]
pub fn enumerate_volumes() -> Result<Vec<VolumeInfo>> {
    bail!("Win32 Volume GUID enumeration is only supported on Windows")
}

#[cfg(not(windows))]
pub fn guid_for_mount_path(_mount_path: &Path) -> Result<String> {
    bail!("Win32 Volume GUID resolution is only supported on Windows")
}

#[cfg(not(windows))]
pub fn mount_paths_for_guid(_volume_guid: &str) -> Result<Vec<PathBuf>> {
    bail!("Win32 Volume GUID reverse lookup is only supported on Windows")
}
