use super::access::ReadLease;
use crate::error::AppError;
use axum::http::StatusCode;
use std::{
    collections::HashMap,
    env,
    ffi::OsString,
    io,
    path::{Path, PathBuf},
    process::{Output, Stdio},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
};

const INPUT_FORMATS: &str =
    "mov,matroska,webm,mpegts,avi,asf,flv,rm,ogg,mp3,aac,wav,flac,mpeg,m4v,h264,hevc";

fn input_options(protocols: &str) -> [&str; 4] {
    [
        "-protocol_whitelist",
        protocols,
        "-format_whitelist",
        INPUT_FORMATS,
    ]
}

#[derive(Clone, Default)]
pub struct MediaTools {
    ffmpeg: Option<PathBuf>,
    ffprobe: Option<PathBuf>,
    search_path: OsString,
}

impl MediaTools {
    pub fn from_path(data: &HashMap<String, PathBuf>) -> Self {
        let roots: Vec<_> = data
            .values()
            .filter_map(|path| path.canonicalize().ok())
            .collect();
        let path = env::var_os("PATH").unwrap_or_default();
        let dirs: Vec<_> = env::split_paths(&path)
            .filter(|path| path.is_absolute())
            .filter_map(|path| path.canonicalize().ok())
            .filter(|dir| !roots.iter().any(|root| inside_media_root(dir, root)))
            .collect();
        Self {
            ffmpeg: find_tool("ffmpeg", &dirs, &roots),
            ffprobe: find_tool("ffprobe", &dirs, &roots),
            search_path: env::join_paths(&dirs).unwrap_or_default(),
        }
    }

    pub fn encoder(&self) -> Result<&Path, AppError> {
        self.ffmpeg.as_deref().ok_or_else(tools_unavailable)
    }

    fn command(&self, program: &Path) -> Command {
        let mut command = Command::new(program);
        command.env("PATH", &self.search_path).current_dir(
            program
                .parent()
                .expect("tool path must have a parent directory"),
        );
        command
    }
}

fn find_tool(name: &str, dirs: &[PathBuf], roots: &[PathBuf]) -> Option<PathBuf> {
    let name = format!("{name}{}", env::consts::EXE_SUFFIX);
    dirs.iter()
        .filter_map(|dir| dir.join(&name).canonicalize().ok())
        .find(|file| {
            if roots.iter().any(|root| inside_media_root(file, root)) {
                return false;
            }
            let Ok(metadata) = file.metadata() else {
                return false;
            };
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 == 0 {
                    return false;
                }
            }
            metadata.is_file()
        })
}

fn inside_media_root(file: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    return Path::new(&file.to_string_lossy().to_lowercase())
        .starts_with(root.to_string_lossy().to_lowercase());
    #[cfg(not(windows))]
    file.starts_with(root)
}

pub fn ffmpeg_command(tools: &MediaTools) -> Result<Command, AppError> {
    let mut command = tools.command(tools.encoder()?);
    command.args([
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-threads",
        "2",
        "-filter_threads",
        "1",
    ]);
    Ok(command)
}

pub fn ffmpeg(tools: &MediaTools, input: &Path, seek: Option<f64>) -> Result<Command, AppError> {
    let mut command = ffmpeg_input(tools, "file,pipe", seek)?;
    command.arg(input);
    Ok(command)
}

pub fn ffmpeg_file(
    tools: &MediaTools,
    file: std::fs::File,
    seek: Option<f64>,
) -> Result<Command, AppError> {
    let mut command = ffmpeg_input(tools, "fd", seek)?;
    command.arg("fd:").stdin(file);
    Ok(command)
}

fn ffmpeg_input(
    tools: &MediaTools,
    protocols: &str,
    seek: Option<f64>,
) -> Result<Command, AppError> {
    let mut command = ffmpeg_command(tools)?;
    command.args(input_options(protocols));
    if let Some(seek) = seek {
        // 保留裁切前的解码数据，让滤镜能使用仍在显示的上一帧。
        command.args(["-ss", &seek.to_string(), "-noaccurate_seek"]);
    }
    command.arg("-i");
    Ok(command)
}

pub fn ffprobe(tools: &MediaTools) -> Result<Command, AppError> {
    let mut command = tools.command(tools.ffprobe.as_deref().ok_or_else(tools_unavailable)?);
    command
        .args(["-v", "error"])
        .args(input_options("file,pipe"));
    Ok(command)
}

pub fn process_error(error: io::Error) -> AppError {
    if error.kind() == io::ErrorKind::NotFound {
        return tools_unavailable();
    }
    error.into()
}

fn tools_unavailable() -> AppError {
    AppError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        "FFmpeg tools are unavailable on the server",
    )
}

pub struct ToolProcess {
    child: Option<Child>,
    read: Option<ReadLease>,
}

impl ToolProcess {
    pub fn start(command: &mut Command, read: &ReadLease) -> Result<Self, AppError> {
        let child = command.kill_on_drop(true).spawn().map_err(process_error)?;
        Ok(Self {
            child: Some(child),
            read: Some(read.clone()),
        })
    }

    pub fn child(&mut self) -> &mut Child {
        self.child
            .as_mut()
            .expect("media process has not been reaped")
    }

    pub fn set_read(&mut self, read: Option<ReadLease>) {
        self.read = read;
    }

    pub async fn stop(mut self) -> Result<(), AppError> {
        self.child().kill().await.map_err(process_error)?;
        self.child.take();
        Ok(())
    }

    pub async fn output(&mut self) -> Result<Output, AppError> {
        let child = self.child();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let (status, stdout, stderr) =
            tokio::try_join!(child.wait(), drain(stdout), drain(stderr))?;
        self.child.take();
        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }
}

impl Drop for ToolProcess {
    fn drop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        let _ = child.start_kill();
        // 终止请求不等于进程已退出；保留读许可直到 Windows 关闭文件句柄。
        if let Some(read) = self.read.take() {
            tokio::spawn(async move {
                let _read = read;
                if let Err(error) = child.wait().await {
                    eprintln!("Could not reap media process: {error}");
                }
            });
        }
    }
}

async fn drain(stream: Option<impl AsyncRead + Unpin>) -> io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    if let Some(mut stream) = stream {
        stream.read_to_end(&mut bytes).await?;
    }
    Ok(bytes)
}

pub async fn run(
    command: &mut Command,
    program: &str,
    read: &ReadLease,
) -> Result<Vec<u8>, AppError> {
    let output = ToolProcess::start(
        command
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped()),
        read,
    )?
    .output()
    .await?;
    check_output(&output, program, !output.stdout.is_empty())?;
    Ok(output.stdout)
}

pub async fn run_with_audio(
    command: &mut Command,
    audio: &[u8],
    read: &ReadLease,
) -> Result<Vec<u8>, AppError> {
    let command = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = ToolProcess::start(command, read)?;
    let mut stdin = child.child().stdin.take().expect("ffmpeg stdin is piped");
    let write = async move { stdin.write_all(audio).await };
    let (written, output) = tokio::join!(write, child.output());
    let output = output?;
    check_output(&output, "ffmpeg", !output.stdout.is_empty())?;
    written?;
    Ok(output.stdout)
}

pub fn check_output(output: &Output, program: &str, has_data: bool) -> Result<(), AppError> {
    if !output.status.success() || !has_data {
        let details = String::from_utf8_lossy(&output.stderr);
        if let Some(line) = details.lines().next() {
            eprintln!("{program}: {line}");
        }
        return Err(AppError::new(
            StatusCode::BAD_GATEWAY,
            "Media conversion failed",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_tools_inside_media_roots() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("media");
        let trusted = directory.path().join("tools");
        let name = format!("ffprobe{}", env::consts::EXE_SUFFIX);
        for folder in [&root, &trusted] {
            std::fs::create_dir(folder).unwrap();
            let file = folder.join(&name);
            std::fs::write(&file, []).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o700)).unwrap();
            }
        }
        assert_eq!(
            find_tool(
                "ffprobe",
                &[root.clone(), trusted.clone()],
                &[root.canonicalize().unwrap()]
            ),
            Some(trusted.join(name).canonicalize().unwrap())
        );
    }
}
