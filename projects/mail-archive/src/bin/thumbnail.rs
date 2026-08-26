#[cfg(target_os = "linux")]
use serde_json::Value;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::thread;
#[cfg(any(target_os = "linux", target_os = "windows"))]
use std::time::{Duration, Instant};

static PREVIEW_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug)]
pub enum PreviewError {
    Disabled,
    Unavailable,
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    Provider(String),
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    Timeout,
    Io(String),
}

impl std::fmt::Display for PreviewError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Disabled => formatter.write_str("prévisualisations désactivées"),
            Self::Unavailable => formatter.write_str("aucun provider de miniature disponible"),
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            Self::Provider(error) => write!(formatter, "provider de miniature : {error}"),
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            Self::Timeout => formatter.write_str("le provider de miniature a expiré"),
            Self::Io(error) => write!(formatter, "prévisualisation impossible : {error}"),
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn wait_with_timeout(child: &mut Child, timeout: Duration) -> Result<(), PreviewError> {
    let started = Instant::now();
    loop {
        if child
            .try_wait()
            .map_err(|error| PreviewError::Io(error.to_string()))?
            .is_some()
        {
            return Ok(());
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PreviewError::Timeout);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn run_helper(program: &Path, args: &[&str]) -> Result<Output, PreviewError> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| PreviewError::Io(error.to_string()))?;
    wait_with_timeout(&mut child, Duration::from_secs(15))?;
    child
        .wait_with_output()
        .map_err(|error| PreviewError::Io(error.to_string()))
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn executable_candidates(name: &str, variable: &str) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = env::var_os(variable) {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(executable) = env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join(name));
        }
    }
    candidates.push(PathBuf::from(name));
    candidates
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn first_executable(candidates: impl IntoIterator<Item = PathBuf>) -> Option<PathBuf> {
    let path_entries = env::var_os("PATH")
        .map(|path| env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    candidates.into_iter().find_map(|candidate| {
        if candidate.is_file() {
            return Some(candidate);
        }
        if candidate.components().count() == 1 {
            return path_entries
                .iter()
                .map(|directory| directory.join(&candidate))
                .find(|path| path.is_file());
        }
        None
    })
}

fn output_path(directory: &Path) -> Result<PathBuf, PreviewError> {
    fs::create_dir_all(directory).map_err(|error| PreviewError::Io(error.to_string()))?;
    let serial = PREVIEW_COUNTER.fetch_add(1, Ordering::Relaxed);
    Ok(directory.join(format!("preview-{}-{serial}.png", std::process::id())))
}

#[cfg(any(target_os = "linux", target_os = "windows"))]
fn valid_output(path: &Path) -> Result<PathBuf, PreviewError> {
    let bytes = fs::read(path).map_err(|error| PreviewError::Io(error.to_string()))?;
    if bytes.len() < 24 || &bytes[..8] != b"\x89PNG\r\n\x1a\n" || &bytes[12..16] != b"IHDR" {
        return Err(PreviewError::Provider("sortie PNG invalide".into()));
    }
    Ok(path.to_path_buf())
}

#[cfg(target_os = "linux")]
fn json_output(stdout: &[u8]) -> Option<PathBuf> {
    let value: Value = serde_json::from_slice(stdout).ok()?;
    (value.get("result")?.as_str()? == "thumbnail")
        .then(|| value.get("output_path")?.as_str().map(PathBuf::from))
        .flatten()
}

#[cfg(target_os = "linux")]
fn machine_output(stdout: &[u8]) -> Option<PathBuf> {
    let line = std::str::from_utf8(stdout).ok()?.trim();
    let mut fields = line.split('\t');
    if fields.next()? != "ok" {
        return None;
    }
    Some(PathBuf::from(fields.next()?))
}

#[cfg(target_os = "linux")]
fn run_kio(input: &Path, output: &Path, size: u32) -> Result<PathBuf, PreviewError> {
    let mut candidates = executable_candidates(
        "memoria-kio-thumbnail-helper",
        "MEMORIA_KIO_THUMBNAIL_HELPER",
    );
    if let Ok(executable) = env::current_exe() {
        if let Some(directory) = executable.parent() {
            candidates.push(directory.join("kio-thumbnail-probe"));
        }
    }
    candidates.push(PathBuf::from("kio-thumbnail-probe"));
    let Some(helper) = first_executable(candidates) else {
        return Err(PreviewError::Unavailable);
    };
    let input = input.to_string_lossy().into_owned();
    let output_string = output.to_string_lossy().into_owned();
    let size = size.to_string();
    let result = run_helper(&helper, &["preview", &input, &size, &output_string])?;
    if !result.status.success() {
        return Err(PreviewError::Provider("KIO a refusé la miniature".into()));
    }
    let Some(path) = json_output(&result.stdout) else {
        return Err(PreviewError::Provider("réponse KIO illisible".into()));
    };
    valid_output(&path)
}

#[cfg(target_os = "linux")]
fn run_freedesktop(input: &Path, size: u32) -> Result<PathBuf, PreviewError> {
    let Some(probe) = first_executable(executable_candidates(
        "system-thumbnail-probe",
        "MEMORIA_THUMBNAIL_PROBE",
    )) else {
        return Err(PreviewError::Unavailable);
    };
    let input_string = input.to_string_lossy().into_owned();
    let size = size.to_string();
    let result = run_helper(&probe, &["thumbnail", &input_string, &size])?;
    if !result.status.success() {
        return Err(PreviewError::Provider(
            "provider freedesktop indisponible".into(),
        ));
    }
    let Some(path) = machine_output(&result.stdout) else {
        return Err(PreviewError::Unavailable);
    };
    valid_output(&path)
}

pub fn preview_attachment(
    input: &Path,
    output_directory: &Path,
    max_size: u32,
) -> Result<PathBuf, PreviewError> {
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let _ = (input, max_size);
    if env::var_os("MEMORIA_DISABLE_SYSTEM_PREVIEWS").is_some() {
        return Err(PreviewError::Disabled);
    }
    let output = output_path(output_directory)?;
    #[cfg(target_os = "linux")]
    {
        let kde = env::var("XDG_CURRENT_DESKTOP")
            .map(|value| value.to_ascii_lowercase().contains("kde"))
            .unwrap_or(false)
            || env::var_os("KDE_FULL_SESSION").is_some();
        if kde && run_kio(input, &output, max_size).is_ok() {
            return valid_output(&output);
        }
        if let Ok(path) = run_freedesktop(input, max_size) {
            fs::copy(&path, &output).map_err(|error| PreviewError::Io(error.to_string()))?;
            let _ = fs::remove_file(path);
            return valid_output(&output);
        }
    }
    #[cfg(windows)]
    {
        if let Some(helper) = first_executable(executable_candidates(
            "memoria-windows-thumbnail-helper",
            "MEMORIA_WINDOWS_THUMBNAIL_HELPER",
        )) {
            let input_string = input.to_string_lossy().into_owned();
            let output_string = output.to_string_lossy().into_owned();
            let size = max_size.to_string();
            let result = run_helper(
                &helper,
                &["thumbnail", &input_string, &size, &output_string],
            )?;
            if result.status.success() {
                return valid_output(&output);
            }
        }
    }
    let _ = fs::remove_file(&output);
    Err(PreviewError::Unavailable)
}

// This file is included as a private module by `mail-archive-app`. Cargo also
// discovers files directly under `src/bin`; keep the incidental target
// harmless when it is compiled on its own.
#[allow(dead_code)]
fn main() {}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn parses_helper_outputs_without_exposing_content() {
        assert_eq!(
            json_output(br#"{"result":"thumbnail","output_path":"/tmp/a.png"}"#),
            Some(PathBuf::from("/tmp/a.png"))
        );
        assert_eq!(
            machine_output(b"ok\t/tmp/a.png\t256\t192\n"),
            Some(PathBuf::from("/tmp/a.png"))
        );
    }
}
