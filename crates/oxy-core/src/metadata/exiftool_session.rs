//! A batch owns one ExifTool child and joins both pipe readers before it exits.
//! The owning core worker remains live while any native operation is unfinished.
use super::{MetadataError, exiftool_command};
use std::{
    ffi::OsString,
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Stdio},
    sync::mpsc::{self, Receiver, Sender},
    thread::JoinHandle,
};

enum Message {
    Line(bool, String),
    Closed(bool),
    Error(String),
}

pub(super) struct Session {
    child: Child,
    input: Option<ChildStdin>,
    output: Receiver<Message>,
    readers: Vec<JoinHandle<()>>,
    sequence: u64,
    broken: bool,
    executable: Option<PathBuf>,
}

#[derive(Debug)]
pub(super) struct WriteError {
    pub error: MetadataError,
    pub uncertain: bool,
}

impl From<MetadataError> for WriteError {
    fn from(error: MetadataError) -> Self {
        Self {
            error,
            uncertain: false,
        }
    }
}

impl Session {
    pub fn start(executable: Option<&Path>) -> Result<Self, MetadataError> {
        let mut child = exiftool_command(executable)
            .args(["-stay_open", "True", "-@", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(MetadataError::Io)?;
        let input = child.stdin.take();
        let (sender, output) = mpsc::channel();
        let mut session = Self {
            child,
            input,
            output,
            readers: Vec::new(),
            sequence: 0,
            broken: false,
            executable: executable.map(Path::to_owned),
        };
        let stdout = session.child.stdout.take().expect("piped stdout");
        let stderr = session.child.stderr.take().expect("piped stderr");
        session
            .readers
            .push(reader(stdout, false, sender.clone()).map_err(MetadataError::Io)?);
        session
            .readers
            .push(reader(stderr, true, sender).map_err(MetadataError::Io)?);
        Ok(session)
    }

    pub fn is_broken(&self) -> bool {
        self.broken
    }
    pub fn is_for(&self, executable: Option<&Path>) -> bool {
        self.executable.as_deref() == executable
    }

    /// Line-oriented argfiles cannot represent newlines or non-Unicode names.
    /// Those rare inputs use the existing argv transport instead.
    pub fn supports(args: &[OsString]) -> bool {
        args.iter().all(|arg| {
            arg.to_str()
                .is_some_and(|value| !value.contains(['\r', '\n']) && value.trim() == value)
        })
    }

    pub fn execute(&mut self, args: &[OsString]) -> Result<(), WriteError> {
        if self.broken {
            return Err(
                MetadataError::Read("ExifTool session is no longer available".into()).into(),
            );
        }
        self.sequence += 1;
        let ready = format!("{{ready{}}}", self.sequence);
        let status_prefix = format!("oxy-status-{}:", self.sequence);
        let mut payload = String::from("-charset\nfilename=UTF8\n");
        for arg in args {
            payload.push_str(arg.to_str().expect("validated argfile argument"));
            payload.push('\n');
        }
        payload.push_str(&format!(
            "-echo4\n{status_prefix}${{status}}\n-execute{}\n",
            self.sequence
        ));
        let input = self.input.as_mut().expect("live session stdin");
        if let Err(error) = input
            .write_all(payload.as_bytes())
            .and_then(|()| input.flush())
        {
            self.broken = true;
            return Err(WriteError {
                error: MetadataError::Io(error),
                uncertain: true,
            });
        }
        let mut reached_ready = false;
        let mut status = None;
        let mut diagnostics = String::new();
        while !reached_ready || status.is_none() {
            match self.output.recv() {
                Ok(Message::Line(false, line)) if line == ready => reached_ready = true,
                Ok(Message::Line(true, line)) if line.starts_with(&status_prefix) => {
                    status = line[status_prefix.len()..].parse::<i32>().ok();
                    if status.is_none() {
                        break;
                    }
                }
                Ok(Message::Line(true, line)) => {
                    // Bound diagnostics independently of the child's output volume.
                    if diagnostics.len() < 8192 {
                        diagnostics.push_str(&line);
                        diagnostics.push('\n');
                    }
                }
                Ok(Message::Line(false, _)) => {}
                Ok(Message::Closed(stderr)) => {
                    diagnostics.push_str(if stderr {
                        "stderr closed"
                    } else {
                        "stdout closed"
                    });
                    break;
                }
                Ok(Message::Error(error)) => {
                    diagnostics.push_str(&error);
                    break;
                }
                Err(error) => {
                    diagnostics.push_str(&error.to_string());
                    break;
                }
            }
        }
        if !reached_ready || status.is_none() {
            self.broken = true;
            return Err(WriteError {
                error: MetadataError::Read(format!(
                    "ExifTool exited before confirming the write; inspect the file before retrying. {diagnostics}"
                )),
                uncertain: true,
            });
        }
        if status != Some(0) {
            return Err(WriteError {
                error: MetadataError::Read(diagnostics.trim().to_owned()),
                uncertain: true,
            });
        }
        Ok(())
    }
}

fn reader(
    input: impl Read + Send + 'static,
    stderr: bool,
    sender: Sender<Message>,
) -> std::io::Result<JoinHandle<()>> {
    std::thread::Builder::new()
        .name("exiftool-pipe".into())
        .spawn(move || {
            for line in BufReader::new(input).lines() {
                let message = match line {
                    Ok(line) => Message::Line(stderr, line),
                    Err(error) => Message::Error(error.to_string()),
                };
                if sender.send(message).is_err() {
                    return;
                }
            }
            let _ = sender.send(Message::Closed(stderr));
        })
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(mut input) = self.input.take() {
            let _ = input.write_all(b"-stay_open\nFalse\n");
            let _ = input.flush();
        }
        // Do not report an active native write as cancelled/joined. Runtime
        // shutdown can time out while this owned worker waits for the child.
        let _ = self.child.wait();
        for reader in self.readers.drain(..) {
            let _ = reader.join();
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::{fs, os::unix::fs::PermissionsExt};

    fn script(directory: &Path, body: &str) -> std::path::PathBuf {
        let path = directory.join("fake-exiftool");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn drains_stderr_correlates_status_and_keeps_the_child_after_a_file_error() {
        let temp = tempfile::tempdir().unwrap();
        let path = script(
            temp.path(),
            r#"
while IFS= read -r arg; do
  case "$arg" in
    -stay_open) IFS= read -r flag; [ "$flag" = False ] && exit 0 ;;
    -execute*)
      id=${arg#-execute}
      count=0
      while [ "$count" -lt 2000 ]; do printf 'diagnostic line %s\n' "$count" >&2; count=$((count + 1)); done
      status=0
      [ "$id" = 2 ] && status=1
      printf 'oxy-status-%s:%s\n' "$id" "$status" >&2
      printf '{ready%s}\n' "$id"
      ;;
  esac
done
"#,
        );
        let mut session = Session::start(Some(&path)).unwrap();
        let pid = session.child.id();
        session.execute(&[]).unwrap();
        assert!(session.execute(&[]).unwrap_err().uncertain);
        assert!(!session.is_broken());
        session.execute(&[]).unwrap();
        assert_eq!(session.child.id(), pid);
    }

    #[test]
    fn lost_reply_is_uncertain_and_never_replayed() {
        let temp = tempfile::tempdir().unwrap();
        let path = script(
            temp.path(),
            "while IFS= read -r arg; do case \"$arg\" in -execute*) exit 0 ;; esac; done",
        );
        let mut session = Session::start(Some(&path)).unwrap();
        assert!(session.execute(&[]).unwrap_err().uncertain);
        assert!(session.is_broken());
        assert!(session.execute(&[]).is_err());
    }
}
