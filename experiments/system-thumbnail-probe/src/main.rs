#[cfg(unix)]
use md5::{Digest, Md5};
use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Debug)]
enum ThumbnailError {
    Unavailable,
    Provider(String),
    InvalidOutput(String),
    Io(io::Error),
}

impl From<io::Error> for ThumbnailError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[derive(Debug)]
struct Thumbnail {
    path: PathBuf,
    width: u32,
    height: u32,
    cached: bool,
}

fn file_uri(path: &Path) -> io::Result<String> {
    let absolute = fs::canonicalize(path)?;
    let mut uri = String::from("file://");
    for byte in absolute.to_string_lossy().bytes() {
        if byte.is_ascii_alphanumeric() || b"/-._~".contains(&byte) {
            uri.push(byte as char);
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    Ok(uri)
}

fn png_dimensions(path: &Path) -> Result<(u32, u32), ThumbnailError> {
    let bytes = fs::read(path)?;
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return Err(ThumbnailError::InvalidOutput(
            "provider did not produce a PNG thumbnail".into(),
        ));
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0 || height == 0 {
        return Err(ThumbnailError::InvalidOutput("empty thumbnail".into()));
    }
    Ok((width, height))
}

fn wait_with_timeout(child: &mut Child, timeout: Duration) -> io::Result<std::process::ExitStatus> {
    let started = Instant::now();
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "thumbnail provider timeout",
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn thumbnail_via_helper(path: &Path, max_size: u32) -> Result<Thumbnail, ThumbnailError> {
    let mut child = Command::new(env::current_exe()?)
        .args(["--worker", &path.to_string_lossy(), &max_size.to_string()])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    wait_with_timeout(&mut child, Duration::from_secs(15))
        .map_err(|error| ThumbnailError::Provider(error.to_string()))?;
    let output = child.wait_with_output()?;
    let line = String::from_utf8_lossy(&output.stdout);
    let mut fields = line.trim().split('\t');
    match fields.next() {
        Some("ok") => {
            let output_path = PathBuf::from(fields.next().ok_or(ThumbnailError::Unavailable)?);
            let width = fields
                .next()
                .and_then(|value| value.parse().ok())
                .ok_or(ThumbnailError::Unavailable)?;
            let height = fields
                .next()
                .and_then(|value| value.parse().ok())
                .ok_or(ThumbnailError::Unavailable)?;
            Ok(Thumbnail {
                path: output_path,
                width,
                height,
                cached: false,
            })
        }
        Some("unavailable") => Err(ThumbnailError::Unavailable),
        _ => Err(ThumbnailError::Provider("helper failed".into())),
    }
}

#[cfg(unix)]
fn linux_mime(path: &Path) -> Result<String, ThumbnailError> {
    let output = Command::new("gio")
        .args(["info", "--attributes=standard::content-type", "--"])
        .arg(path)
        .output()?;
    if !output.status.success() {
        return Err(ThumbnailError::Provider(
            "gio could not identify MIME".into(),
        ));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.split_once("standard::content-type:")
        .and_then(|(_, value)| value.split_whitespace().next())
        .map(str::to_string)
        .ok_or(ThumbnailError::Unavailable)
}

#[cfg(unix)]
fn thumbnail_cache_candidates(uri: &str) -> Vec<PathBuf> {
    let mut hasher = Md5::new();
    hasher.update(uri.as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    let cache = env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")));
    let Some(cache) = cache else {
        return Vec::new();
    };
    ["normal", "large", "x-large"]
        .into_iter()
        .map(|size| {
            cache
                .join("thumbnails")
                .join(size)
                .join(format!("{hash}.png"))
        })
        .collect()
}

#[cfg(unix)]
fn thumbnailer_entries() -> Vec<(String, String, Vec<String>)> {
    let mut result = Vec::new();
    for directory in ["/usr/share/thumbnailers", "/usr/local/share/thumbnailers"] {
        let Ok(entries) = fs::read_dir(directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|x| x.to_str()) != Some("thumbnailer") {
                continue;
            }
            let Ok(text) = fs::read_to_string(path) else {
                continue;
            };
            let mut exec = None;
            let mut try_exec = None;
            let mut mime_types = Vec::new();
            for line in text.lines() {
                if let Some(value) = line.strip_prefix("Exec=") {
                    exec = Some(value.to_string());
                }
                if let Some(value) = line.strip_prefix("TryExec=") {
                    try_exec = Some(value.to_string());
                }
                if let Some(value) = line.strip_prefix("MimeType=") {
                    mime_types.extend(
                        value
                            .split(';')
                            .filter(|x| !x.is_empty())
                            .map(str::to_string),
                    );
                }
            }
            let Some(exec) = exec else { continue };
            if let Some(command) = try_exec {
                if !command.is_empty() && Command::new(&command).arg("--version").output().is_err()
                {
                    continue;
                }
            }
            result.push((exec, mime_types.join(";"), mime_types));
        }
    }
    result
}

#[cfg(unix)]
fn expand_exec(
    template: &str,
    path: &Path,
    uri: &str,
    output: &Path,
    size: u32,
) -> Option<(String, Vec<String>)> {
    let tokens: Vec<&str> = template.split_whitespace().collect();
    let (program, rest) = tokens.split_first()?;
    let mut args = Vec::new();
    for token in rest {
        let value = token
            .replace("%i", &path.to_string_lossy())
            .replace("%u", uri)
            .replace("%o", &output.to_string_lossy())
            .replace("%s", &size.to_string());
        if !value.starts_with('%') {
            args.push(value);
        }
    }
    Some(((*program).to_string(), args))
}

#[cfg(unix)]
fn thumbnail_linux(path: &Path, max_size: u32) -> Result<Thumbnail, ThumbnailError> {
    let uri = file_uri(path)?;
    for cached in thumbnail_cache_candidates(&uri) {
        if cached.is_file() {
            if let Ok((width, height)) = png_dimensions(&cached) {
                return Ok(Thumbnail {
                    path: cached,
                    width,
                    height,
                    cached: true,
                });
            }
        }
    }
    let mime = linux_mime(path)?;
    let temp = env::temp_dir().join(format!(
        "system-thumbnail-{}-{}.png",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| ThumbnailError::Provider(error.to_string()))?
            .as_nanos()
    ));
    for (exec, _all, mimes) in thumbnailer_entries() {
        if !mimes.iter().any(|candidate| candidate == &mime) {
            continue;
        }
        let Some((program, args)) = expand_exec(&exec, path, &uri, &temp, max_size) else {
            continue;
        };
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let status = wait_with_timeout(&mut child, Duration::from_secs(10))
            .map_err(|error| ThumbnailError::Provider(error.to_string()))?;
        if !status.success() {
            return Err(ThumbnailError::Provider(format!("provider exit {status}")));
        }
        let dimensions = png_dimensions(&temp)?;
        return Ok(Thumbnail {
            path: temp,
            width: dimensions.0,
            height: dimensions.1,
            cached: false,
        });
    }
    Err(ThumbnailError::Unavailable)
}

#[cfg(windows)]
fn thumbnail_windows(path: &Path, max_size: u32) -> Result<Thumbnail, ThumbnailError> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::{Interface, PCWSTR};
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO,
        BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        IShellItem, IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK,
        SIIGBF_THUMBNAILONLY,
    };

    unsafe {
        let com_status = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if com_status.is_err() {
            return Err(ThumbnailError::Provider(format!(
                "COM initialization failed: {com_status:?}"
            )));
        }
        let result = (|| {
            let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)
                .map_err(|e| ThumbnailError::Provider(e.to_string()))?;
            let factory: IShellItemImageFactory = item
                .cast()
                .map_err(|e| ThumbnailError::Provider(e.to_string()))?;
            let bitmap = factory
                .GetImage(
                    SIZE {
                        cx: max_size as i32,
                        cy: max_size as i32,
                    },
                    SIIGBF_THUMBNAILONLY | SIIGBF_BIGGERSIZEOK,
                )
                .map_err(|_| ThumbnailError::Unavailable)?;
            let dc = CreateCompatibleDC(None);
            if dc.0.is_null() {
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                return Err(ThumbnailError::Io(io::Error::last_os_error()));
            }
            let mut bitmap_info = BITMAP::default();
            if GetObjectW(
                bitmap.into(),
                std::mem::size_of::<BITMAP>() as i32,
                Some(&mut bitmap_info as *mut _ as *mut std::ffi::c_void),
            ) == 0
            {
                let _ = DeleteDC(dc);
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                return Err(ThumbnailError::Provider("GetObjectW failed".into()));
            }
            let width = bitmap_info.bmWidth.unsigned_abs();
            let height = bitmap_info.bmHeight.unsigned_abs();
            if width == 0 || height == 0 {
                let _ = DeleteDC(dc);
                let _ = DeleteObject(HGDIOBJ(bitmap.0));
                return Err(ThumbnailError::InvalidOutput(
                    "empty Shell thumbnail".into(),
                ));
            }
            let mut info = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width as i32,
                    biHeight: -(height as i32),
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0 as u32,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut pixels = vec![0u8; width as usize * height as usize * 4];
            let copied = GetDIBits(
                dc,
                bitmap,
                0,
                height,
                Some(pixels.as_mut_ptr() as *mut std::ffi::c_void),
                &mut info,
                DIB_RGB_COLORS,
            );
            let _ = DeleteDC(dc);
            let _ = DeleteObject(HGDIOBJ(bitmap.0));
            if copied == 0 {
                return Err(ThumbnailError::Provider("GetDIBits failed".into()));
            }
            for pixel in pixels.chunks_exact_mut(4) {
                pixel.swap(0, 2);
            }
            let output = env::temp_dir().join(format!(
                "system-thumbnail-{}-{}.png",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|error| ThumbnailError::Provider(error.to_string()))?
                    .as_nanos()
            ));
            let file = fs::File::create(&output)?;
            let mut encoder = png::Encoder::new(file, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut png_writer = encoder
                .write_header()
                .map_err(|e| ThumbnailError::Provider(e.to_string()))?;
            png_writer
                .write_image_data(&pixels)
                .map_err(|e| ThumbnailError::Provider(e.to_string()))?;
            Ok(Thumbnail {
                path: output,
                width,
                height,
                cached: false,
            })
        })();
        CoUninitialize();
        result
    }
}

fn thumbnail(path: &Path, max_size: u32) -> Result<Thumbnail, ThumbnailError> {
    #[cfg(unix)]
    {
        thumbnail_linux(path, max_size)
    }
    #[cfg(windows)]
    {
        thumbnail_windows(path, max_size)
    }
}

fn print_result(path: &Path, started: Instant) {
    match thumbnail(path, 256) {
        Ok(result) => println!(
            "file={} result=thumbnail thumbnail_path={} cached={} width={} height={} elapsed_ms={}",
            path.display(),
            result.path.display(),
            result.cached,
            result.width,
            result.height,
            started.elapsed().as_millis()
        ),
        Err(ThumbnailError::Unavailable) => println!(
            "file={} result=unavailable elapsed_ms={}",
            path.display(),
            started.elapsed().as_millis()
        ),
        Err(error) => println!(
            "file={} result=error kind={error:?} elapsed_ms={}",
            path.display(),
            started.elapsed().as_millis()
        ),
    }
}

fn print_machine_result(path: &Path, max_size: u32) {
    match thumbnail(path, max_size) {
        Ok(result) => println!(
            "ok\t{}\t{}\t{}",
            result.path.display(),
            result.width,
            result.height
        ),
        Err(ThumbnailError::Unavailable) => println!("unavailable"),
        Err(_) => println!("error"),
    }
}

fn main() {
    let mut args = env::args().skip(1);
    match args.next().as_deref() {
        Some("thumbnail") => {
            let Some(path) = args.next() else {
                eprintln!("usage: thumbnail PATH [MAX_SIZE]");
                return;
            };
            let path = PathBuf::from(path);
            let started = Instant::now();
            print_result(&path, started);
        }
        Some("helper") => {
            let Some(path) = args.next() else { eprintln!("usage: helper PATH [MAX_SIZE]"); return };
            let max_size = args.next().and_then(|value| value.parse().ok()).unwrap_or(256);
            let started = Instant::now();
            match thumbnail_via_helper(&PathBuf::from(path), max_size) {
                Ok(result) => println!("result=thumbnail width={} height={} elapsed_ms={}", result.width, result.height, started.elapsed().as_millis()),
                Err(ThumbnailError::Unavailable) => println!("result=unavailable elapsed_ms={}", started.elapsed().as_millis()),
                Err(error) => println!("result=error kind={error:?} elapsed_ms={}", started.elapsed().as_millis()),
            }
        }
        Some("--worker") => {
            let Some(path) = args.next() else { return };
            let max_size = args.next().and_then(|value| value.parse().ok()).unwrap_or(256);
            print_machine_result(&PathBuf::from(path), max_size);
        }
        Some("providers") => {
            #[cfg(unix)]
            for (exec, _, mimes) in thumbnailer_entries() {
                println!("provider={} mimes={}", exec, mimes.join(","));
            }
            #[cfg(windows)]
            println!("provider=IShellItemImageFactory isolation=system-shell");
        }
        _ => eprintln!("usage: system-thumbnail-probe providers | thumbnail PATH [MAX_SIZE] | helper PATH [MAX_SIZE]"),
    }
}
