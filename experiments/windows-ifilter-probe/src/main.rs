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
    use std::io::{self, Write};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::time::Instant;

    use windows::core::{Interface, PCWSTR, PWSTR};
    use windows::Win32::Storage::IndexServer::{
        IFilter, LoadIFilter, CHUNK_TEXT, FILTER_E_END_OF_CHUNKS, FILTER_E_NO_MORE_TEXT,
        FILTER_S_LAST_TEXT, IFILTER_INIT_CANON_PARAGRAPHS, IFILTER_INIT_CANON_SPACES,
        IFILTER_INIT_INDEXING_ONLY, STAT_CHUNK,
    };
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

    const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;
    const MAX_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
    const IID_IFILTER: &str = "{89BCB740-6119-101A-BCB7-00DD010655AF}";

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(std::iter::once(0)).collect()
    }

    fn extract(path: &Path) -> Result<String, String> {
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
        let Some(path) = args.next() else {
            eprintln!("usage: windows-ifilter-probe <path>");
            return 2;
        };
        let path = Path::new(&path);
        let started = Instant::now();
        match extract(path) {
            Ok(text) => {
                // stdout is deliberately the helper protocol: UTF-8 text only.
                let _ = io::stdout().write_all(text.as_bytes());
                let _ = io::stdout().flush();
                eprintln!(
                    "status=success bytes={} elapsed_ms={}",
                    text.len(),
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
