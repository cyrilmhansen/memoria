//! Isolated Windows IFilter helper for PDF attachment text extraction.
//!
//! This binary is deliberately separate from mail-archive-app. It resolves
//! the registered PDF handler at runtime and loads it only in this process.

#[cfg(not(windows))]
fn main() {
    eprintln!("memoria-ifilter-helper: Windows only");
    std::process::exit(3);
}

#[cfg(windows)]
mod windows_helper {
    use std::env;
    use std::ffi::OsStr;
    use std::io::{self, Write};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::core::{Interface, PCWSTR, PWSTR};
    use windows::Win32::Storage::IndexServer::{
        IFilter, CHUNK_TEXT, FILTER_E_END_OF_CHUNKS, FILTER_E_NO_MORE_TEXT, FILTER_S_LAST_TEXT,
        IFILTER_INIT_CANON_PARAGRAPHS, IFILTER_INIT_CANON_SPACES, IFILTER_INIT_INDEXING_ONLY,
        STAT_CHUNK,
    };
    use windows::Win32::System::Com::{
        CLSIDFromString, CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistStream,
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, STGM_READ,
    };
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegQueryValueExW, HKEY, HKEY_CLASSES_ROOT, KEY_READ,
        REG_VALUE_TYPE,
    };
    use windows::Win32::UI::Shell::SHCreateStreamOnFileEx;

    const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
    const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
    const IID_IFILTER: &str = "{89BCB740-6119-101A-BCB7-00DD010655AF}";

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn registry_value(path: &str, value_name: Option<&str>) -> Option<String> {
        let path = wide(OsStr::new(path));
        let value_name = value_name.map(|value| wide(OsStr::new(value)));
        let value_name = value_name
            .as_ref()
            .map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr()));
        let mut key = HKEY::default();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CLASSES_ROOT,
                PCWSTR(path.as_ptr()),
                None,
                KEY_READ,
                &mut key,
            )
        };
        if status.0 != 0 {
            return None;
        }
        let mut value_type = REG_VALUE_TYPE(0);
        let mut byte_len = 0u32;
        let status = unsafe {
            RegQueryValueExW(
                key,
                value_name,
                None,
                Some(&mut value_type),
                None,
                Some(&mut byte_len),
            )
        };
        if status.0 != 0 || byte_len == 0 {
            unsafe {
                let _ = RegCloseKey(key);
            }
            return None;
        }
        let mut bytes = vec![0u8; byte_len as usize];
        let status = unsafe {
            RegQueryValueExW(
                key,
                value_name,
                None,
                Some(&mut value_type),
                Some(bytes.as_mut_ptr()),
                Some(&mut byte_len),
            )
        };
        unsafe {
            let _ = RegCloseKey(key);
        }
        if status.0 != 0 {
            return None;
        }
        let units = bytes[..byte_len as usize]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|unit| *unit != 0)
            .collect::<Vec<_>>();
        Some(String::from_utf16_lossy(&units))
    }

    fn normalize_extension(extension: &str) -> String {
        let extension = extension.trim();
        if extension.starts_with('.') {
            extension.to_ascii_lowercase()
        } else {
            format!(".{}", extension.to_ascii_lowercase())
        }
    }

    fn registered_ifilter_clsid(extension: &str) -> Result<String, String> {
        let extension = normalize_extension(extension);
        let progid = registry_value(&extension, None)
            .or_else(|| registry_value(&format!("SystemFileAssociations\\{extension}"), None));
        let paths = [
            progid
                .as_deref()
                .map(|value| format!("{value}\\PersistentHandler")),
            Some(format!("{extension}\\PersistentHandler")),
            Some(format!(
                "SystemFileAssociations\\{extension}\\PersistentHandler"
            )),
        ];
        let persistent = paths
            .iter()
            .flatten()
            .find_map(|path| registry_value(path, None))
            .unwrap_or_default();
        [
            format!("CLSID\\{persistent}\\PersistentAddinsRegistered\\{IID_IFILTER}"),
            format!("{persistent}\\PersistentAddinsRegistered\\{IID_IFILTER}"),
            format!(
                "CLSID\\{}\\PersistentAddinsRegistered\\{IID_IFILTER}",
                progid.as_deref().unwrap_or("")
            ),
            format!(
                "SystemFileAssociations\\{extension}\\PersistentAddinsRegistered\\{IID_IFILTER}"
            ),
        ]
        .iter()
        .find_map(|path| registry_value(path, None))
        .ok_or_else(|| "ifilter-unavailable".into())
    }

    struct ComGuard;
    impl ComGuard {
        fn new() -> Result<Self, String> {
            let status = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            if status.is_ok() {
                Ok(Self)
            } else {
                Err(format!("com-init: {status}"))
            }
        }
    }
    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    fn extract(path: &Path) -> Result<String, String> {
        let metadata = std::fs::metadata(path).map_err(|error| format!("input: {error}"))?;
        if metadata.len() > MAX_INPUT_BYTES {
            return Err("input-too-large".into());
        }
        let _com = ComGuard::new()?;
        let extension = path
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| format!(".{value}"))
            .ok_or_else(|| "missing-extension".to_string())?;
        let clsid = registered_ifilter_clsid(&extension)?;
        let clsid = wide(OsStr::new(&clsid));
        let filter: IFilter = unsafe {
            CoCreateInstance(
                &CLSIDFromString(PCWSTR(clsid.as_ptr()))
                    .map_err(|error| format!("clsid-parse: {error}"))?,
                None,
                CLSCTX_INPROC_SERVER,
            )
        }
        .map_err(|error| format!("cocreate: {error}"))?;
        let persist: IPersistStream = filter
            .cast()
            .map_err(|error| format!("ipersiststream: {error}"))?;
        let path = wide(path.as_os_str());
        let stream =
            unsafe { SHCreateStreamOnFileEx(PCWSTR(path.as_ptr()), STGM_READ.0, 0, false, None) }
                .map_err(|error| format!("stream-open: {error}"))?;
        unsafe { persist.Load(&stream) }.map_err(|error| format!("persist-load: {error}"))?;

        let flags = IFILTER_INIT_INDEXING_ONLY.0
            | IFILTER_INIT_CANON_PARAGRAPHS.0
            | IFILTER_INIT_CANON_SPACES.0;
        let mut status_flags = 0;
        let status = unsafe { filter.Init(flags as u32, &[], &mut status_flags) };
        if status < 0 {
            return Err(format!("filter-init-hresult=0x{status:08x}"));
        }
        let mut output = String::new();
        loop {
            let mut chunk = STAT_CHUNK::default();
            let status = unsafe { filter.GetChunk(&mut chunk) };
            if status == FILTER_E_END_OF_CHUNKS.0 {
                break;
            }
            if status < 0 {
                return Err(format!("get-chunk-hresult=0x{status:08x}"));
            }
            if chunk.flags.0 & CHUNK_TEXT.0 == 0 {
                continue;
            }
            loop {
                let mut buffer = vec![0u16; 4096];
                let mut count = buffer.len() as u32;
                let status = unsafe { filter.GetText(&mut count, PWSTR(buffer.as_mut_ptr())) };
                if status == FILTER_E_NO_MORE_TEXT.0 {
                    break;
                }
                if status < 0 && status != FILTER_S_LAST_TEXT.0 {
                    return Err(format!("get-text-hresult=0x{status:08x}"));
                }
                let count = (count as usize).min(buffer.len());
                output.push_str(&String::from_utf16_lossy(&buffer[..count]));
                if output.len() > MAX_OUTPUT_BYTES {
                    return Err("output-too-large".into());
                }
                if status == FILTER_S_LAST_TEXT.0 {
                    break;
                }
            }
        }
        Ok(output)
    }

    pub fn run() -> ! {
        let Some(argument) = env::args_os().nth(1) else {
            eprintln!("status=failed reason=missing-input");
            std::process::exit(4);
        };
        if argument == "discover" {
            let extension = env::args_os()
                .nth(2)
                .ok_or_else(|| "missing-extension".to_string());
            match extension
                .and_then(|extension| registered_ifilter_clsid(&extension.to_string_lossy()))
            {
                Ok(_) => std::process::exit(0),
                Err(_) => std::process::exit(3),
            }
        }
        let path = argument;
        match extract(Path::new(&path)) {
            Ok(text) => {
                let _ = io::stdout().write_all(text.as_bytes());
                let _ = io::stdout().flush();
                std::process::exit(0);
            }
            Err(error) if error == "ifilter-unavailable" => {
                eprintln!("status=unsupported reason={error}");
                std::process::exit(3);
            }
            Err(error) => {
                eprintln!("status=failed reason={error}");
                std::process::exit(4);
            }
        }
    }
}

#[cfg(windows)]
fn main() -> ! {
    windows_helper::run()
}
