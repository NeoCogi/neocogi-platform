//! Host-native conversion for immutable platform path capabilities.

use crate::platform::{INativePathAbi, PathEncoding};
use neocogi_com::{ComRef, NcOutputBuffer, NcStatus, NC_E_INVALID_ARG};
use std::path::PathBuf;

/// Copies a borrowed platform path into exact host-native owned storage.
pub fn decode(path: ComRef<'_, INativePathAbi>) -> Result<PathBuf, NcStatus> {
    let encoding = path.encoding()?;
    let count = usize::try_from(path.unit_count()?).map_err(|_| NC_E_INVALID_ARG)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        if encoding != PathEncoding::WindowsUtf16 {
            return Err(NC_E_INVALID_ARG);
        }
        let mut units = NcOutputBuffer::<u16>::new();
        path.windows_units(units.output(0, count))?;
        if units.len() != count {
            return Err(NC_E_INVALID_ARG);
        }
        let units = units.into_vec();
        Ok(std::ffi::OsString::from_wide(&units).into())
    }
    #[cfg(not(windows))]
    {
        if encoding != PathEncoding::UnixBytes {
            return Err(NC_E_INVALID_ARG);
        }
        let mut bytes = NcOutputBuffer::<u8>::new();
        path.unix_bytes(bytes.output(0, count))?;
        if bytes.len() != count {
            return Err(NC_E_INVALID_ARG);
        }
        let bytes = bytes.into_vec();
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            Ok(std::ffi::OsString::from_vec(bytes).into())
        }
        #[cfg(not(unix))]
        {
            // Browser targets have no native filesystem picker; a lexical path
            // still must not invent an invalid UTF-8 filesystem identity.
            Ok(std::string::String::from_utf8(bytes).map_err(|_| NC_E_INVALID_ARG)?.into())
        }
    }
}
