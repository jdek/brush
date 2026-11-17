//! Integration tests for opening files through a host's `FileOpener`.

#![cfg(test)]
#![allow(clippy::panic_in_result_fn)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use brush_builtins::{BuiltinSet, ShellBuilderExt as _};
use brush_core::extensions::{
    DefaultErrorFormatter, DefaultProcessLauncher, DefaultTaskSpawner, FileAccess, FileOpener,
    ShellExtensionsImpl,
};

/// Opens files as asked, but for those named `forbidden`; records each open it
/// was asked for.
#[derive(Clone, Default)]
struct RecordingOpener {
    opens: Arc<Mutex<Vec<(PathBuf, FileAccess)>>>,
}

impl RecordingOpener {
    fn take(&self) -> Vec<(PathBuf, FileAccess)> {
        std::mem::take(&mut *self.opens.lock().unwrap())
    }
}

impl FileOpener for RecordingOpener {
    fn open(
        &self,
        path: &Path,
        options: &std::fs::OpenOptions,
        access: FileAccess,
    ) -> Result<std::fs::File, std::io::Error> {
        self.opens.lock().unwrap().push((path.to_owned(), access));
        if path.file_name().is_some_and(|n| n == "forbidden") {
            return Err(std::io::ErrorKind::PermissionDenied.into());
        }
        options.open(path)
    }
}

type Host = ShellExtensionsImpl<
    DefaultErrorFormatter,
    DefaultProcessLauncher,
    DefaultTaskSpawner,
    RecordingOpener,
>;

/// Runs `script` in `dir`, in a shell that opens files with `opener`,
/// returning its exit status.
async fn run(opener: &RecordingOpener, dir: &Path, script: &str) -> Result<u8> {
    let mut shell = brush_core::Shell::builder_with_extensions::<Host>()
        .default_builtins(BuiltinSet::BashMode)
        .file_opener(opener.clone())
        .working_dir(dir.to_owned())
        .build()
        .await?;

    let params = shell.default_exec_params();
    let _ = shell
        .run_string(script, &brush_core::SourceInfo::default(), &params)
        .await?;

    Ok(shell.last_exit_status())
}

#[tokio::test]
async fn redirections_are_opened_by_the_hosts_opener() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let opener = RecordingOpener::default();

    let status = run(
        &opener,
        &root,
        "echo one > out; echo two >> out; read -r line < out; echo both &> all; : <> rw",
    )
    .await?;

    assert_eq!(status, 0);
    assert_eq!(
        opener.take(),
        [
            (root.join("out"), FileAccess::Write),
            (root.join("out"), FileAccess::Write),
            (root.join("out"), FileAccess::Read),
            (root.join("all"), FileAccess::Write),
            (root.join("rw"), FileAccess::ReadWrite),
        ]
    );
    assert_eq!(std::fs::read_to_string(root.join("out"))?, "one\ntwo\n");
    Ok(())
}

#[tokio::test]
async fn a_file_the_opener_refuses_is_not_opened() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let opener = RecordingOpener::default();

    // The command of a redirection that fails is not run; the script goes on.
    let status = run(&opener, &root, "echo secret > forbidden; echo after > out").await?;

    assert_eq!(status, 0);
    assert!(!root.join("forbidden").exists());
    assert_eq!(std::fs::read_to_string(root.join("out"))?, "after\n");
    Ok(())
}

#[tokio::test]
async fn sourced_scripts_are_opened_by_the_hosts_opener() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    std::fs::write(root.join("lib.sh"), "exit 7\n")?;
    let opener = RecordingOpener::default();

    let status = run(&opener, &root, "source ./lib.sh").await?;

    assert_eq!(status, 7);
    assert_eq!(opener.take(), [(root.join("lib.sh"), FileAccess::Read)]);
    Ok(())
}
