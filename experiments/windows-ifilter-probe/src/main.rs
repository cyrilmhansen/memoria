//! Isolated Windows IFilter probe.
//!
//! This executable is intentionally not a Memoria dependency.  It is the
//! process boundary that would be used by a future Windows backend.

#[cfg(not(windows))]
fn main() {
    eprintln!("windows-ifilter-probe: Windows only");
    std::process::exit(2);
}

#[cfg(windows)]
mod windows_probe {
    use std::env;
    use std::ffi::OsStr;
    use std::io::{self, Read, Write};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::time::Instant;

    use windows::core::{Interface, PCWSTR, PWSTR};
    use windows::Win32::Storage::IndexServer::{
        IFilter, LoadIFilter, CHUNK_TEXT, FILTER_E_END_OF_CHUNKS, FILTER_E_NO_MORE_TEXT,
        FILTER_S_LAST_TEXT, IFILTER_INIT_CANON_PARAGRAPHS, IFILTER_INIT_CANON_SPACES,
        IFILTER_INIT_INDEXING_ONLY, STAT_CHUNK,
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

    #[derive(Debug)]
    struct RegisteredIFilter {
        extension: String,
        clsid: String,
        dll_path: Option<String>,
        threading_model: Option<String>,
    }

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn registry_value(key_path: &str, value_name: Option<&str>) -> Option<String> {
        let key_path = wide(OsStr::new(key_path));
        let value_name = value_name.map(|value| wide(OsStr::new(value)));
        let value_name = value_name
            .as_ref()
            .map_or(PCWSTR::null(), |value| PCWSTR(value.as_ptr()));
        let mut key = HKEY::default();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CLASSES_ROOT,
                PCWSTR(key_path.as_ptr()),
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
        let query = unsafe {
            RegQueryValueExW(
                key,
                value_name,
                None,
                Some(&mut value_type),
                None,
                Some(&mut byte_len),
            )
        };
        if query.0 != 0 || byte_len == 0 {
            unsafe { let _ = RegCloseKey(key); }
            return None;
        }
        let mut bytes = vec![0u8; byte_len as usize];
        let query = unsafe {
            RegQueryValueExW(
                key,
                value_name,
                None,
                Some(&mut value_type),
                Some(bytes.as_mut_ptr()),
                Some(&mut byte_len),
            )
        };
        unsafe { let _ = RegCloseKey(key); }
        if query.0 != 0 {
            return None;
        }
        let units = bytes[..byte_len as usize]
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .take_while(|unit| *unit != 0)
            .collect::<Vec<_>>();
        Some(String::from_utf16_lossy(&units))
    }

    fn discover_registered_ifilter(extension: &str) -> Result<RegisteredIFilter, String> {
        let extension = if extension.starts_with('.') {
            extension.to_ascii_lowercase()
        } else {
            format!(".{extension}").to_ascii_lowercase()
        };
        let progid = registry_value(&extension, None)
            .or_else(|| registry_value(&format!("SystemFileAssociations\\{extension}"), None))
            .ok_or_else(|| "extension-registration-missing".to_string())?;
        let persistent = registry_value(&format!("{progid}\\PersistentHandler"), None)
            .or_else(|| registry_value(&format!("{extension}\\PersistentHandler"), None))
            .or_else(|| {
                registry_value(
                    &format!("SystemFileAssociations\\{extension}\\PersistentHandler"),
                    None,
                )
            })
            .unwrap_or_default();
        let addin_paths = [
            format!("CLSID\\{persistent}\\PersistentAddinsRegistered\\{IID_IFILTER}"),
            format!("{persistent}\\PersistentAddinsRegistered\\{IID_IFILTER}"),
            format!("CLSID\\{progid}\\PersistentAddinsRegistered\\{IID_IFILTER}"),
            format!(
                "SystemFileAssociations\\{extension}\\PersistentAddinsRegistered\\{IID_IFILTER}"
            ),
        ];
        let clsid = addin_paths
            .iter()
            .find_map(|path| registry_value(path, None))
            .ok_or_else(|| {
                if persistent.is_empty() {
                    "persistent-handler-and-ifilter-addin-missing".to_string()
                } else {
                    "ifilter-addin-missing".to_string()
                }
            })?;
        let inproc = format!("CLSID\\{clsid}\\InprocServer32");
        Ok(RegisteredIFilter {
            extension,
            clsid,
            dll_path: registry_value(&inproc, None),
            threading_model: registry_value(&inproc, Some("ThreadingModel")),
        })
    }

    struct Extracted {
        text: String,
        chunks: u32,
        text_chunks: u32,
    }

    fn extract(path: &Path) -> Result<Extracted, String> {
        let metadata = std::fs::metadata(path).map_err(|error| format!("input: {error}"))?;
        if metadata.len() > MAX_INPUT_BYTES {
            return Err("input-too-large".into());
        }

        let _com = ComGuard::new()?;
        let path_wide = wide(path.as_os_str());
        let mut raw_filter = std::ptr::null_mut();
        unsafe {
            LoadIFilter(PCWSTR(path_wide.as_ptr()), None, &mut raw_filter)
                .map_err(|error| format!("load-ifilter: {error}"))?;
        }
        if raw_filter.is_null() {
            return Err("unsupported-null-filter".into());
        }
        let filter = unsafe { IFilter::from_raw(raw_filter as _) };
        read_filter(filter)
    }

    fn extract_direct(clsid: &str, path: &Path) -> Result<Extracted, String> {
        let metadata = std::fs::metadata(path).map_err(|error| format!("input: {error}"))?;
        if metadata.len() > MAX_INPUT_BYTES {
            return Err("input-too-large".into());
        }
        let _com = ComGuard::new()?;
        let clsid_wide = wide(OsStr::new(clsid));
        let clsid = unsafe { CLSIDFromString(PCWSTR(clsid_wide.as_ptr())) }
            .map_err(|error| format!("clsid-parse: {error}"))?;
        let filter: IFilter = unsafe {
            CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER)
                .map_err(|error| format!("cocreate: {error}"))?
        };
        let persist: IPersistStream = filter
            .cast()
            .map_err(|error| format!("ipersiststream: {error}"))?;
        let path_wide = wide(path.as_os_str());
        let stream = unsafe {
            SHCreateStreamOnFileEx(PCWSTR(path_wide.as_ptr()), STGM_READ.0, 0, false, None)
        }
        .map_err(|error| format!("stream-open: {error}"))?;
        unsafe { persist.Load(&stream) }.map_err(|error| format!("persist-load: {error}"))?;
        read_filter(filter)
    }

    fn extract_dynamic(path: &Path) -> Result<(RegisteredIFilter, Extracted), String> {
        let extension = path
            .extension()
            .and_then(OsStr::to_str)
            .ok_or_else(|| "extension-missing".to_string())?;
        let registration = discover_registered_ifilter(extension)?;
        let extracted = extract_direct(&registration.clsid, path)?;
        Ok((registration, extracted))
    }

    fn read_filter(filter: IFilter) -> Result<Extracted, String> {
        let flags = IFILTER_INIT_INDEXING_ONLY.0
            | IFILTER_INIT_CANON_PARAGRAPHS.0
            | IFILTER_INIT_CANON_SPACES.0;
        let mut status_flags = 0;
        let status = unsafe { filter.Init(flags as u32, &[], &mut status_flags) };
        if status < 0 {
            return Err(format!("filter-init-hresult=0x{status:08x}"));
        }

        let mut output = String::new();
        let mut chunks = 0;
        let mut text_chunks = 0;
        loop {
            let mut chunk = STAT_CHUNK::default();
            let status = unsafe { filter.GetChunk(&mut chunk) };
            if status == FILTER_E_END_OF_CHUNKS.0 {
                break;
            }
            if status < 0 {
                return Err(format!("get-chunk-hresult=0x{status:08x}"));
            }
            chunks += 1;
            if chunk.flags.0 & CHUNK_TEXT.0 == 0 {
                continue;
            }
            text_chunks += 1;
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
        Ok(Extracted {
            text: output,
            chunks,
            text_chunks,
        })
    }

    fn controlled_child(mode: &str) -> i32 {
        match mode {
            "test-sleep" => {
                std::thread::sleep(std::time::Duration::from_secs(20));
                0
            }
            "test-output" => {
                let block = vec![b'x'; 64 * 1024];
                for _ in 0..(9 * 1024 * 1024 / block.len()) {
                    let _ = io::stdout().write_all(&block);
                }
                let _ = io::stdout().flush();
                0
            }
            "test-crash" => std::process::exit(97),
            _ => 2,
        }
    }

    fn supervise(mode: &str) -> i32 {
        let executable = match env::current_exe() {
            Ok(path) => path,
            Err(error) => {
                eprintln!("supervisor-error={error}");
                return 2;
            }
        };
        let mut child = match std::process::Command::new(executable)
            .arg(mode)
            .stdout(std::process::Stdio::piped())
            .spawn()
        {
            Ok(child) => child,
            Err(error) => {
                eprintln!("supervisor-spawn-error={error}");
                return 2;
            }
        };
        let stdout = child.stdout.take().expect("piped stdout");
        let reader = std::thread::spawn(move || {
            let mut reader = io::BufReader::new(stdout);
            let mut buffer = [0u8; 64 * 1024];
            let mut total = 0usize;
            loop {
                let count = match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => count,
                    Err(_) => break,
                };
                total += count;
                if total > MAX_OUTPUT_BYTES {
                    break;
                }
            }
            total
        });
        let started = Instant::now();
        let result = loop {
            if reader.is_finished() {
                let bytes = reader.join().unwrap_or(usize::MAX);
                if bytes > MAX_OUTPUT_BYTES {
                    let _ = child.kill();
                    let _ = child.wait();
                    break "output-too-large";
                }
                let status = child.wait();
                break if status.map(|status| status.success()).unwrap_or(false) {
                    "completed"
                } else {
                    "crashed"
                };
            }
            if started.elapsed() > std::time::Duration::from_secs(10) {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                break "timeout";
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        eprintln!(
            "mode={mode} result={result} elapsed_ms={}",
            started.elapsed().as_millis()
        );
        if result == "completed" { 0 } else { 4 }
    }

    struct ComGuard;
    impl ComGuard {
        fn new() -> Result<Self, String> {
            let status = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
            if status.is_err() {
                return Err(format!("com-init: {status:?}"));
            }
            Ok(Self)
        }
    }
    impl Drop for ComGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }

    fn main_impl() -> i32 {
        let mut args = env::args_os();
        let _program = args.next();
        let Some(first) = args.next() else {
            eprintln!("usage: <path> | direct <clsid> <path> | dynamic <path> | discover <extension>");
            return 2;
        };
        if first == "discover" {
            let Some(extension) = args.next() else { return 2 };
            match discover_registered_ifilter(&extension.to_string_lossy()) {
                Ok(registration) => {
                    eprintln!(
                        "extension={} clsid={} dll={} threading_model={}",
                        registration.extension,
                        registration.clsid,
                        registration.dll_path.as_deref().unwrap_or(""),
                        registration.threading_model.as_deref().unwrap_or("")
                    );
                    return 0;
                }
                Err(error) => {
                    eprintln!("status=unsupported reason={error} iid_ifilter={IID_IFILTER}");
                    return 3;
                }
            }
        }
        if first == "dynamic" {
            let Some(path) = args.next() else { return 2 };
            let path = Path::new(&path);
            let started = Instant::now();
            match extract_dynamic(path) {
                Ok((registration, extracted)) => {
                    let _ = io::stdout().write_all(extracted.text.as_bytes());
                    let _ = io::stdout().flush();
                    eprintln!(
                        "status=success extension={} clsid={} dll={} threading_model={} bytes={} chunks={} text_chunks={} elapsed_ms={}",
                        registration.extension,
                        registration.clsid,
                        registration.dll_path.as_deref().unwrap_or(""),
                        registration.threading_model.as_deref().unwrap_or(""),
                        extracted.text.len(),
                        extracted.chunks,
                        extracted.text_chunks,
                        started.elapsed().as_millis()
                    );
                    return 0;
                }
                Err(reason) => {
                    eprintln!("status=failed reason={reason}");
                    return 4;
                }
            }
        }
        if first == "supervise" {
            let Some(mode) = args.next() else { return 2 };
            if !matches!(mode.to_string_lossy().as_ref(), "test-sleep" | "test-output" | "test-crash") {
                return 2;
            }
            return supervise(&mode.to_string_lossy());
        }
        if matches!(first.to_string_lossy().as_ref(), "test-sleep" | "test-output" | "test-crash") {
            return controlled_child(&first.to_string_lossy());
        }
        let (direct_clsid, path) = if first == "direct" {
            let Some(clsid) = args.next() else {
                eprintln!("usage: windows-ifilter-probe direct <clsid> <path>");
                return 2;
            };
            let Some(path) = args.next() else {
                eprintln!("usage: windows-ifilter-probe direct <clsid> <path>");
                return 2;
            };
            (Some(clsid), path)
        } else {
            (None, first)
        };
        let path = Path::new(&path);
        let started = Instant::now();
        let result = match direct_clsid.as_deref() {
            Some(clsid) => extract_direct(&clsid.to_string_lossy(), path),
            None => extract(path),
        };
        match result {
            Ok(extracted) => {
                // stdout is deliberately the helper protocol: UTF-8 text only.
                let _ = io::stdout().write_all(extracted.text.as_bytes());
                let _ = io::stdout().flush();
                eprintln!(
                    "status=success bytes={} chunks={} text_chunks={} elapsed_ms={}",
                    extracted.text.len(),
                    extracted.chunks,
                    extracted.text_chunks,
                    started.elapsed().as_millis()
                );
                0
            }
            Err(reason)
                if reason == "unsupported-null-filter" || reason.starts_with("load-ifilter:") =>
            {
                eprintln!("status=unsupported reason={reason} iid_ifilter={IID_IFILTER}");
                3
            }
            Err(reason) => {
                eprintln!("status=failed reason={reason}");
                4
            }
        }
    }

    pub fn run() -> ! {
        std::process::exit(main_impl())
    }
}

#[cfg(windows)]
fn main() -> ! {
    windows_probe::run()
}
