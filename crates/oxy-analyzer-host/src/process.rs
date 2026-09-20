//! First-party subprocess transport. This is fault isolation, not an OS sandbox.
use crate::{AnalyzeInput, AnalyzeOutput, Analyzer, AnalyzerError, BuiltInFaceAnalyzer};
use oxy_domain::{
    AnalyzerDescriptor, AnalyzerPackManifest, AnalyzerWorkerCommand, AnalyzerWorkerInit,
    AnalyzerWorkerReply, AnalyzerWorkerRequest, FaceAnalyzerSettings,
};
use oxy_runtime::CancellationToken;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, ChildStdout, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

const HOST_API: u32 = 2;
const MAX_CONTROL: usize = 4 * 1024 * 1024;
const MAX_PIXELS: usize = 512 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(120);

fn failed(error: impl std::fmt::Display) -> AnalyzerError {
    AnalyzerError::Failed(error.to_string())
}

pub fn read_frame(reader: &mut impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let size = u32::from_le_bytes(header) as usize;
    if size > limit {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "analyzer frame exceeds limit",
        ));
    }
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn write_frame(writer: &mut impl Write, bytes: &[u8]) -> io::Result<()> {
    let size = u32::try_from(bytes.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "analyzer frame exceeds u32"))?;
    writer.write_all(&size.to_le_bytes())?;
    writer.write_all(bytes)?;
    writer.flush()
}

fn digest(path: &Path) -> Result<String, AnalyzerError> {
    let mut file = File::open(path).map_err(failed)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let len = file.read(&mut buffer).map_err(failed)?;
        if len == 0 {
            break;
        }
        hash.update(&buffer[..len]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

/// Resolve only an application-owned pack; external plugin installation is unsupported.
pub fn verify_pack(root: &Path) -> Result<PathBuf, AnalyzerError> {
    let root = root.canonicalize().map_err(failed)?;
    let manifest: AnalyzerPackManifest =
        serde_json::from_slice(&std::fs::read(root.join("manifest.json")).map_err(failed)?)
            .map_err(failed)?;
    if manifest.id != "faces"
        || manifest.version != "1"
        || manifest.host_api != HOST_API
        || manifest.target_os != std::env::consts::OS
        || manifest.target_arch != std::env::consts::ARCH
    {
        return Err(failed("incompatible first-party analyzer pack"));
    }
    if Path::new(&manifest.entrypoint).components().count() != 1 {
        return Err(failed("invalid pack entrypoint"));
    }
    let binary = root
        .join(&manifest.entrypoint)
        .canonicalize()
        .map_err(failed)?;
    if binary.parent() != Some(root.as_path()) || digest(&binary)? != manifest.entrypoint_sha256 {
        return Err(failed("analyzer binary integrity check failed"));
    }
    Ok(binary)
}

pub fn verify_models(root: &Path) -> Result<oxy_faces::FaceModelPaths, AnalyzerError> {
    let root = root.canonicalize().map_err(failed)?;
    for model in &oxy_faces::managed_model_manifest().models {
        let path = root.join(&model.file).canonicalize().map_err(failed)?;
        if path.parent() != Some(root.as_path()) || digest(&path)? != model.sha256 {
            return Err(failed("face model integrity check failed"));
        }
    }
    oxy_faces::managed_model_paths(&root)
        .ok_or_else(|| failed("managed face models are incomplete"))
}

struct Worker {
    child: Child,
    input: Mutex<ChildStdin>,
    output: ChildStdout,
    next_request: u64,
}
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub struct ProcessAnalyzer {
    descriptor: AnalyzerDescriptor,
    worker: Mutex<Worker>,
}

/// Monitor blocking pipe I/O from another thread so cancellation can kill a hung child.
fn exchange<T: Send>(
    worker: &mut Worker,
    cancel: &CancellationToken,
    operation: impl FnOnce(&Mutex<ChildStdin>, &mut ChildStdout) -> Result<T, AnalyzerError> + Send,
) -> Result<T, AnalyzerError> {
    std::thread::scope(|scope| {
        let (send, receive) = mpsc::sync_channel(1);
        let input = &worker.input;
        let output = &mut worker.output;
        scope.spawn(move || {
            let _ = send.send(operation(input, output));
        });
        let start = Instant::now();
        let mut cancelled_at = None;
        let mut cancel_sent = false;
        loop {
            if cancel.is_cancelled() {
                let at = cancelled_at.get_or_insert_with(Instant::now);
                if !cancel_sent && worker.next_request > 0 {
                    if let Ok(mut writer) = worker.input.try_lock() {
                        let command = AnalyzerWorkerCommand::Cancel {
                            request_id: worker.next_request,
                        };
                        if let Ok(bytes) = serde_json::to_vec(&command) {
                            let _ = write_frame(&mut *writer, &bytes);
                        }
                        cancel_sent = true;
                    }
                }
                if at.elapsed() >= Duration::from_secs(2) {
                    let _ = worker.child.kill();
                    let _ = worker.child.wait();
                    return Err(AnalyzerError::Cancelled);
                }
            }
            if start.elapsed() >= TIMEOUT {
                let _ = worker.child.kill();
                let _ = worker.child.wait();
                return Err(failed("analyzer timed out"));
            }
            match receive.recv_timeout(Duration::from_millis(25)) {
                Ok(result) => {
                    return if cancel.is_cancelled() {
                        Err(AnalyzerError::Cancelled)
                    } else {
                        result
                    };
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(failed("analyzer transport stopped"));
                }
            }
        }
    })
}

impl ProcessAnalyzer {
    pub fn load(
        pack_root: &Path,
        model_root: &Path,
        settings: FaceAnalyzerSettings,
        cancel: &CancellationToken,
    ) -> Result<Self, AnalyzerError> {
        if cancel.is_cancelled() {
            return Err(AnalyzerError::Cancelled);
        }
        let binary = verify_pack(pack_root)?;
        verify_models(model_root)?;
        let mut command = Command::new(binary);
        command
            .current_dir(pack_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = command.spawn().map_err(failed)?;
        let input = child
            .stdin
            .take()
            .ok_or_else(|| failed("missing analyzer stdin"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| failed("missing analyzer stdout"))?;
        let mut worker = Worker {
            child,
            input: Mutex::new(input),
            output,
            next_request: 0,
        };
        let init = AnalyzerWorkerInit {
            host_api: HOST_API,
            model_directory: model_root.canonicalize().map_err(failed)?,
            settings,
        };
        let descriptor: AnalyzerDescriptor = exchange(&mut worker, cancel, |input, output| {
            write_frame(
                &mut *input
                    .lock()
                    .map_err(|_| failed("analyzer stdin poisoned"))?,
                &serde_json::to_vec(&init).map_err(failed)?,
            )
            .map_err(failed)?;
            serde_json::from_slice(&read_frame(output, MAX_CONTROL).map_err(failed)?)
                .map_err(failed)
        })?;
        if descriptor.id != "faces"
            || descriptor.version != "1"
            || descriptor.analysis_fingerprint.is_empty()
            || descriptor.feature_fingerprint.is_empty()
            || !(1..=4096).contains(&descriptor.feature_dimension)
        {
            return Err(failed("invalid analyzer handshake"));
        }
        Ok(Self {
            descriptor,
            worker: Mutex::new(worker),
        })
    }
}

impl Analyzer for ProcessAnalyzer {
    fn descriptor(&self) -> &AnalyzerDescriptor {
        &self.descriptor
    }
    fn analyze(
        &self,
        input: &AnalyzeInput<'_>,
        cancel: &CancellationToken,
    ) -> Result<AnalyzeOutput, AnalyzerError> {
        input.validate()?;
        if input.pixels.len() > MAX_PIXELS {
            return Err(AnalyzerError::InvalidInput);
        }
        if cancel.is_cancelled() {
            return Err(AnalyzerError::Cancelled);
        }
        let mut worker = self
            .worker
            .lock()
            .map_err(|_| failed("analyzer mutex poisoned"))?;
        worker.next_request += 1;
        let request = AnalyzerWorkerRequest {
            request_id: worker.next_request,
            asset_id: input.asset_id.into(),
            source_revision: input.source_revision.into(),
            width: input.width,
            height: input.height,
        };
        exchange(&mut worker, cancel, |writer, reader| {
            {
                let mut writer = writer
                    .lock()
                    .map_err(|_| failed("analyzer stdin poisoned"))?;
                let command = AnalyzerWorkerCommand::Analyze {
                    request: request.clone(),
                };
                write_frame(&mut *writer, &serde_json::to_vec(&command).map_err(failed)?)
                    .map_err(failed)?;
                write_frame(&mut *writer, input.pixels).map_err(failed)?;
            }
            let reply: AnalyzerWorkerReply =
                serde_json::from_slice(&read_frame(reader, MAX_CONTROL).map_err(failed)?)
                    .map_err(failed)?;
            if reply.request_id != request.request_id {
                return Err(failed("mismatched analyzer reply"));
            }
            if let Some(error) = reply.error {
                return Err(failed(error));
            }
            if reply.regions.len() > 512 {
                return Err(failed("too many analyzer regions"));
            }
            let feature_bytes = self
                .descriptor
                .feature_dimension
                .checked_mul(4)
                .ok_or_else(|| failed("invalid feature dimension"))?;
            let payload_limit = 512usize
                .checked_mul(feature_bytes)
                .ok_or_else(|| failed("feature payload limit overflow"))?;
            let bytes = read_frame(reader, payload_limit).map_err(failed)?;
            if bytes.len() != reply.regions.len() * feature_bytes {
                return Err(failed("invalid feature payload"));
            }
            let features = bytes
                .chunks_exact(feature_bytes)
                .map(|feature| {
                    feature
                        .chunks_exact(4)
                        .map(|word| f32::from_le_bytes(word.try_into().expect("four bytes")))
                        .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let ids: std::collections::HashSet<_> =
                reply.regions.iter().map(|region| &region.id).collect();
            if ids.len() != reply.regions.len()
                || reply.regions.iter().any(|region| region.id.is_empty())
                || features.iter().flatten().any(|value| !value.is_finite())
                || reply.regions.iter().any(|region| {
                    !region.rect.is_display_normalized()
                        || !region.score.is_finite()
                        || !(0.0..=1.0).contains(&region.score)
                        || region.landmarks.iter().any(|point| !point.is_valid())
                })
            {
                return Err(failed("invalid analyzer output"));
            }
            Ok(AnalyzeOutput {
                regions: reply.regions,
                features,
            })
        })
    }
}

pub fn serve_worker() -> Result<(), AnalyzerError> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    let init: AnalyzerWorkerInit =
        serde_json::from_slice(&read_frame(&mut input, MAX_CONTROL).map_err(failed)?)
            .map_err(failed)?;
    if init.host_api != HOST_API {
        return Err(failed("incompatible host API"));
    }
    let analyzer =
        BuiltInFaceAnalyzer::load(&verify_models(&init.model_directory)?, init.settings)?;
    write_frame(
        &mut output,
        &serde_json::to_vec(analyzer.descriptor()).map_err(failed)?,
    )
    .map_err(failed)?;
    drop(input);
    let (send, receive) = mpsc::sync_channel(1);
    let active = Arc::new(Mutex::new(None::<(u64, CancellationToken)>));
    let reader_active = Arc::clone(&active);
    std::thread::spawn(move || {
        let mut input = io::stdin().lock();
        while let Ok(bytes) = read_frame(&mut input, MAX_CONTROL) {
            let Ok(command) = serde_json::from_slice::<AnalyzerWorkerCommand>(&bytes) else {
                break;
            };
            match command {
                AnalyzerWorkerCommand::Cancel { request_id } => {
                    if let Some((id, token)) =
                        &*reader_active.lock().expect("active request poisoned")
                    {
                        if *id == request_id {
                            token.cancel();
                        }
                    }
                }
                AnalyzerWorkerCommand::Analyze { request } => {
                    let Ok(pixels) = read_frame(&mut input, MAX_PIXELS) else {
                        break;
                    };
                    let token = CancellationToken::default();
                    *reader_active.lock().expect("active request poisoned") =
                        Some((request.request_id, token.clone()));
                    if send.send((request, pixels, token)).is_err() {
                        break;
                    }
                }
            }
        }
        if let Some((_, token)) = &*reader_active.lock().expect("active request poisoned") {
            token.cancel();
        }
    });
    while let Ok((request, pixels, token)) = receive.recv() {
        let analyzed = analyzer.analyze(
            &AnalyzeInput {
                asset_id: &request.asset_id,
                source_revision: &request.source_revision,
                width: request.width,
                height: request.height,
                pixels: &pixels,
            },
            &token,
        );
        let (regions, features, error) = match analyzed {
            Ok(result) => (result.regions, result.features, None),
            Err(_) => (Vec::new(), Vec::new(), Some("analysisFailed".into())),
        };
        let reply = AnalyzerWorkerReply {
            request_id: request.request_id,
            regions,
            error,
        };
        write_frame(&mut output, &serde_json::to_vec(&reply).map_err(failed)?).map_err(failed)?;
        if reply.error.is_none() {
            let bytes: Vec<u8> = features
                .iter()
                .flatten()
                .flat_map(|value| value.to_le_bytes())
                .collect();
            write_frame(&mut output, &bytes).map_err(failed)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    #[test]
    fn cancelled_hung_child_is_killed_and_reaped() {
        let mut child = Command::new("/bin/sleep")
            .arg("30")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut worker = Worker {
            input: Mutex::new(child.stdin.take().unwrap()),
            output: child.stdout.take().unwrap(),
            child,
            next_request: 0,
        };
        let cancel = CancellationToken::default();
        cancel.cancel();
        let start = Instant::now();
        let result = exchange(&mut worker, &cancel, |_, output| {
            read_frame(output, 1024).map_err(failed)
        });
        assert!(matches!(result, Err(AnalyzerError::Cancelled)));
        assert!(start.elapsed() < Duration::from_secs(4));
        assert!(worker.child.try_wait().unwrap().is_some());
    }
    #[cfg(unix)]
    #[test]
    fn crashed_child_fails_without_hanging_host() {
        let mut child = Command::new("/usr/bin/false")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut worker = Worker {
            input: Mutex::new(child.stdin.take().unwrap()),
            output: child.stdout.take().unwrap(),
            child,
            next_request: 0,
        };
        assert!(
            exchange(&mut worker, &CancellationToken::default(), |_, output| {
                read_frame(output, 1024).map_err(failed)
            })
            .is_err()
        );
    }
    #[test]
    fn rejects_oversized_and_truncated_frames() {
        assert!(read_frame(&mut &u32::MAX.to_le_bytes()[..], 1024).is_err());
        assert!(read_frame(&mut &[4, 0, 0, 0, 1][..], 1024).is_err());
    }
}
