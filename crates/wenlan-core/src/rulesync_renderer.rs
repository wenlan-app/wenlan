// SPDX-License-Identifier: Apache-2.0
//! Invoke the pinned rulesync renderer inside a private staging tree.

use anyhow::{bail, ensure, Context, Result};
use sha2::{Digest, Sha256};
use std::cell::OnceCell;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek};
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

pub const PINNED_VERSION: &str = "43.11.0";

#[derive(Debug, thiserror::Error)]
#[error("rulesync rejected the skill's frontmatter")]
pub struct SkillRejected;

struct Pin {
    name: &'static str,
    sha256: &'static str,
}

fn pin_for(os: &str, arch: &str) -> Result<Pin> {
    // Release assets and their digests from v43.11.0's SHA256SUMS.
    let (name, sha256) = match (os, arch) {
        ("macos", "aarch64") => (
            "rulesync-darwin-arm64",
            "174f73789629f9f70e527eaf05db96701e8094a723eace2c7b3b47b661434683",
        ),
        ("macos", "x86_64") => (
            "rulesync-darwin-x64",
            "027356378eb535541786c5c071bc622b23a4556634990cdf507d21d1e3337cec",
        ),
        ("linux", "aarch64") => (
            "rulesync-linux-arm64",
            "b8c57a76ffb82c90c5b86b615efcb2b0576aa1c109d8dcae762410128e65591b",
        ),
        ("linux", "x86_64") => (
            "rulesync-linux-x64",
            "df60ca38f6cf7786c27138a64f70fcafb000548d66de200c00b0e2ac8633b6d2",
        ),
        ("windows", "x86_64") => (
            "rulesync-windows-x64.exe",
            "5527c44d24b073b40bb4a7a8e22149588132e830371b50293fae929ed10ea361",
        ),
        _ => bail!("no rulesync {PINNED_VERSION} binary pin for {os}/{arch}"),
    };
    Ok(Pin { name, sha256 })
}

/// Release filename to look for in Wenlan's local binary directory.
pub fn pinned_binary_name() -> Result<&'static str> {
    Ok(pin_for(std::env::consts::OS, std::env::consts::ARCH)?.name)
}

const DOWNLOAD_BASE: &str = "https://github.com/dyoshikawa/rulesync/releases/download";

/// Return `dir/<pinned name>`, fetching the pinned release asset first when it
/// is missing. The run-time SHA-256 check still applies to the returned file.
pub fn ensure_pinned_binary(dir: &Path) -> Result<PathBuf> {
    let pin = pin_for(std::env::consts::OS, std::env::consts::ARCH)?;
    let base = match std::env::var("WENLAN_RULESYNC_DOWNLOAD_BASE") {
        Ok(base) if !base.is_empty() => format!("{}/v{PINNED_VERSION}", base.trim_end_matches('/')),
        _ => format!("{DOWNLOAD_BASE}/v{PINNED_VERSION}"),
    };
    fetch_pinned(dir, &base, &pin)
}

fn fetch_pinned(dir: &Path, base: &str, pin: &Pin) -> Result<PathBuf> {
    let path = dir.join(pin.name);
    if path.exists() {
        return Ok(path);
    }
    let url = format!("{base}/{}", pin.name);
    let fail = |error: &dyn std::fmt::Display| {
        anyhow::anyhow!(
            "Could not fetch rulesync {PINNED_VERSION} ({error}); set WENLAN_RULESYNC_BINARY to a copy of {}.",
            pin.name
        )
    };
    fs::create_dir_all(dir).map_err(|error| fail(&error))?;
    let mut temp = tempfile::NamedTempFile::new_in(dir).map_err(|error| fail(&error))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|error| fail(&error))?;
    let mut response = client
        .get(&url)
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|error| fail(&error.without_url()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = response.read(&mut buffer).map_err(|error| fail(&error))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
        io::Write::write_all(&mut temp, &buffer[..read]).map_err(|error| fail(&error))?;
    }
    ensure!(
        format!("{:x}", digest.finalize()) == pin.sha256,
        "Could not fetch rulesync {PINNED_VERSION} (the download did not match its checksum); set WENLAN_RULESYNC_BINARY to a copy of {}.",
        pin.name
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        temp.as_file()
            .set_permissions(fs::Permissions::from_mode(0o755))
            .map_err(|error| fail(&error))?;
    }
    temp.persist(&path).map_err(|error| fail(&error.error))?;
    Ok(path)
}

pub fn new_stage(parent: &Path) -> Result<tempfile::TempDir> {
    let mut builder = tempfile::Builder::new();
    builder.prefix("rulesync-stage-");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        builder.permissions(fs::Permissions::from_mode(0o700));
    }
    builder
        .tempdir_in(parent)
        .context("create private rulesync stage")
}

/// Renders in one operation can execute the same verified private snapshot.
/// The installed executable is opened on first use. Each invocation still
/// uses fresh input, output, and writable scratch directories.
pub struct RenderSession<'a> {
    source: &'a Path,
    verified: OnceCell<Result<VerifiedBinary, String>>,
}

struct VerifiedBinary {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl<'a> RenderSession<'a> {
    pub fn new(source: &'a Path) -> Self {
        Self {
            source,
            verified: OnceCell::new(),
        }
    }

    pub fn render(
        &self,
        stage: &Path,
        input_roots: &[PathBuf],
        output: &Path,
        global: bool,
        targets: &[String],
        features: &[&str],
    ) -> Result<()> {
        validate_selection(targets, features)?;
        ensure!(
            global
                || features
                    .iter()
                    .all(|feature| matches!(*feature, "rules" | "skills")),
            "wenlan renders commands and subagents in global scope only"
        );
        render_staged(
            self,
            stage,
            input_roots,
            output,
            Selection {
                global,
                targets,
                features,
            },
        )
    }

    fn binary(&self, parent: &Path) -> Result<&Path> {
        self.verified
            .get_or_init(|| self.verify(parent).map_err(|error| format!("{error:#}")))
            .as_ref()
            .map(|binary| binary.path.as_path())
            .map_err(|error| anyhow::anyhow!(error.clone()))
    }

    fn verify(&self, parent: &Path) -> Result<VerifiedBinary> {
        let pin = pin_for(std::env::consts::OS, std::env::consts::ARCH)?;
        let dir = new_stage(parent)?;
        let path = dir.path().join(pin.name);
        copy_verified_binary(self.source, &path, &pin)?;
        Ok(VerifiedBinary { _dir: dir, path })
    }
}

/// Render one or more staged source trees into an empty staged output directory.
/// `stage` must be a private directory owned by the caller for this operation.
/// Its disjoint input and output trees must already exist. Discard the output
/// directory on error because rulesync generation can fail after writing files.
pub fn render(
    binary: &Path,
    stage: &Path,
    input_roots: &[PathBuf],
    output: &Path,
    global: bool,
    targets: &[String],
    features: &[&str],
) -> Result<()> {
    RenderSession::new(binary).render(stage, input_roots, output, global, targets, features)
}

/// The explicit local settings preview is the only caller that can render
/// these features. Rulesync diagnostics can include snippets of the input.
pub(crate) fn render_settings(
    binary: &Path,
    stage: &Path,
    input: &Path,
    output: &Path,
    features: &[&str],
) -> Result<()> {
    ensure!(
        !features.is_empty()
            && features
                .iter()
                .all(|feature| matches!(*feature, "mcp" | "hooks" | "permissions")),
        "expected explicit settings features"
    );
    render_staged(
        &RenderSession::new(binary),
        stage,
        &[input.to_owned()],
        output,
        Selection {
            global: true,
            targets: &["claudecode".into(), "codexcli".into()],
            features,
        },
    )
}

struct Selection<'a> {
    global: bool,
    targets: &'a [String],
    features: &'a [&'a str],
}

fn render_staged(
    renderer: &RenderSession<'_>,
    stage: &Path,
    input_roots: &[PathBuf],
    output: &Path,
    selection: Selection<'_>,
) -> Result<()> {
    let (stage, input_roots, output) = validate_roots(stage, input_roots, output)?;
    let runtime = tempfile::Builder::new()
        .prefix("rulesync-runtime-")
        .tempdir_in(&stage)
        .context("create private rulesync runtime directory")?;
    let work = runtime.path().join("work");
    fs::create_dir(&work).context("create rulesync scratch directory")?;
    let binary = renderer.binary(stage.parent().context("rulesync stage has no parent")?)?;

    let mut command = renderer_command(binary, &stage, &output, &work)?;
    configure_generate(
        &mut command,
        &input_roots,
        &output,
        &work,
        selection.global,
        selection.targets,
        selection.features,
    );
    run_generate(&mut command, selection.features)?;
    validate_tree(&output)?;
    Ok(())
}

const INVALID_FRONTMATTER: &[u8] = b"Invalid frontmatter in ";

fn run_generate(command: &mut Command, features: &[&str]) -> Result<()> {
    // Only recognize the pin's package diagnostic. The renderer and the
    // sandbox launcher can both exit nonzero, and their text stays private.
    let mut child = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("launch the staged rulesync renderer")?;
    let mut diagnostic = Vec::new();
    let mut stderr = child.stderr.take().context("read renderer diagnostics")?;
    let read = (&mut stderr)
        .take(INVALID_FRONTMATTER.len() as u64)
        .read_to_end(&mut diagnostic)
        .and_then(|_| io::copy(&mut stderr, &mut io::sink()));
    drop(stderr);
    let status = child
        .wait()
        .context("wait for the staged rulesync renderer")?;
    read.context("read renderer diagnostics")?;
    if features == ["skills"] && is_skill_rejection(status.code(), &diagnostic) {
        return Err(SkillRejected.into());
    }
    ensure!(status.success(), "rulesync generation failed ({status})");
    Ok(())
}

fn is_skill_rejection(code: Option<i32>, diagnostic: &[u8]) -> bool {
    code == Some(1) && diagnostic.starts_with(INVALID_FRONTMATTER)
}

fn validate_selection(targets: &[String], features: &[&str]) -> Result<()> {
    ensure!(
        !targets.is_empty()
            && targets.iter().all(|target| {
                let mut chars = target.bytes();
                chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
                    && chars.all(|c| c.is_ascii_alphanumeric() || c == b'-')
            }),
        "rulesync needs explicit tool target names"
    );
    ensure!(
        !features.is_empty()
            && features
                .iter()
                .all(|feature| matches!(*feature, "rules" | "skills" | "commands" | "subagents")),
        "rulesync supports rendering rules, skills, commands and subagents"
    );
    Ok(())
}

fn validate_roots(
    stage: &Path,
    input_roots: &[PathBuf],
    output: &Path,
) -> Result<(PathBuf, Vec<PathBuf>, PathBuf)> {
    ensure!(!input_roots.is_empty(), "rulesync needs an input root");
    ensure!(
        stage.is_absolute() && !stage.components().any(|part| part == Component::ParentDir),
        "rulesync stage needs an absolute path without parent traversal"
    );
    let metadata = require_directory(stage)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        ensure!(
            metadata.permissions().mode() & 0o077 == 0,
            "rulesync stage must be private: {}",
            stage.display()
        );
    }
    #[cfg(not(unix))]
    let _ = metadata;

    let canonical_stage = stage.canonicalize().context("resolve rulesync stage")?;
    let output = staged_directory(stage, &canonical_stage, output)?;
    ensure!(
        fs::read_dir(&output)?.next().is_none(),
        "rulesync output directory must be empty: {}",
        output.display()
    );
    let mut inputs: Vec<PathBuf> = Vec::with_capacity(input_roots.len());
    for root in input_roots {
        let input = staged_directory(stage, &canonical_stage, root)?;
        ensure!(
            std::iter::once(&output)
                .chain(&inputs)
                .all(|other| !input.starts_with(other) && !other.starts_with(&input)),
            "rulesync stage trees overlap at {}",
            input.display()
        );
        validate_tree(&input)?;
        inputs.push(input);
    }
    Ok((canonical_stage, inputs, output))
}

fn require_directory(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("inspect rulesync staging directory {}", path.display()))?;
    ensure!(
        metadata.is_dir() && !is_windows_reparse_point(&metadata),
        "expected a regular staging directory: {}",
        path.display()
    );
    Ok(metadata)
}

fn is_windows_reparse_point(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

fn staged_directory(stage: &Path, canonical_stage: &Path, path: &Path) -> Result<PathBuf> {
    let relative = path
        .strip_prefix(stage)
        .or_else(|_| path.strip_prefix(canonical_stage))
        .with_context(|| {
            format!(
                "staging directory is outside the rulesync stage: {}",
                path.display()
            )
        })?;
    ensure!(
        !relative.as_os_str().is_empty()
            && relative
                .components()
                .all(|part| matches!(part, Component::Normal(_))),
        "invalid rulesync staging directory: {}",
        path.display()
    );
    let mut directory = canonical_stage.to_path_buf();
    for part in relative {
        directory.push(part);
        require_directory(&directory)?;
    }
    let directory = directory
        .canonicalize()
        .with_context(|| format!("resolve rulesync staging directory {}", directory.display()))?;
    ensure!(
        directory.starts_with(canonical_stage) && directory != canonical_stage,
        "directory escaped the rulesync stage: {}",
        directory.display()
    );
    ensure!(
        directory.to_str().is_some(),
        "rulesync needs UTF-8 staging paths"
    );
    Ok(directory)
}

fn validate_tree(directory: &Path) -> Result<()> {
    require_directory(directory)?;
    for entry in fs::read_dir(directory)
        .with_context(|| format!("read rulesync staging directory {}", directory.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        ensure!(
            entry.file_name().to_str().is_some(),
            "rulesync needs UTF-8 filenames: {}",
            path.display()
        );
        let metadata = fs::symlink_metadata(&path)?;
        ensure!(
            !is_windows_reparse_point(&metadata),
            "reparse point in rulesync stage: {}",
            path.display()
        );
        if metadata.is_dir() {
            validate_tree(&path)?;
        } else {
            ensure!(
                metadata.is_file(),
                "non-regular file in rulesync stage: {}",
                path.display()
            );
            #[cfg(unix)]
            {
                use std::os::unix::fs::MetadataExt;
                ensure!(
                    metadata.nlink() == 1,
                    "hardlink in rulesync stage: {}",
                    path.display()
                );
            }
        }
    }
    Ok(())
}

fn open_binary(path: &Path) -> Result<File> {
    #[cfg(unix)]
    let file = {
        use rustix::fs::{open, Mode, OFlags};
        File::from(open(
            path,
            OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?)
    };
    #[cfg(not(unix))]
    let file = File::open(path)?;
    ensure!(
        file.metadata()?.is_file(),
        "rulesync binary is not a regular file"
    );
    Ok(file)
}

fn copy_verified_binary(source: &Path, destination: &Path, pin: &Pin) -> Result<()> {
    let mut source = open_binary(source)
        .with_context(|| format!("open local pinned rulesync binary {}", source.display()))?;
    let mut copy = OpenOptions::new()
        .create_new(true)
        .read(true)
        .write(true)
        .open(destination)?;
    io::copy(&mut source, &mut copy).context("stage local rulesync binary")?;
    copy.rewind()?;
    let mut digest = Sha256::new();
    io::copy(&mut copy, &mut digest).context("hash staged rulesync binary")?;
    ensure!(
        format!("{:x}", digest.finalize()) == pin.sha256,
        "rulesync {PINNED_VERSION} checksum mismatch for {}",
        pin.name
    );
    // Verify and execute the same private snapshot, even if the installed file
    // is replaced between verification and launch.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        copy.set_permissions(fs::Permissions::from_mode(0o500))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn renderer_command(binary: &Path, stage: &Path, output: &Path, runtime: &Path) -> Result<Command> {
    use std::ffi::OsString;
    use std::fmt::Write as _;

    const SANDBOX_EXEC: &str = "/usr/bin/sandbox-exec";
    const PROFILE_START: &str = "(version 1)
        (allow default)
        (deny network*)
        (deny process-exec (require-not (literal (param \"RENDERER\"))))
        (deny file-read* (require-all
            (require-not (subpath (param \"STAGE_ROOT\")))
            (require-not (literal (param \"RENDERER\")))
            (require-not (literal (param \"RENDERER_DIR\")))
            (require-not (subpath \"/usr/lib\"))
            (require-not (subpath \"/System/Library/dyld\"))
            (require-not (subpath \"/private/var/db/dyld\"))
            (require-not (subpath \"/System/Cryptexes/OS/System/Library/dyld\"))
            (require-not (subpath \"/System/Volumes/Preboot/Cryptexes/OS/System/Library/dyld\"))
            (require-not (subpath \"/usr/share/icu\"))
            (require-not (literal \"/System/Library/CoreServices/SystemVersion.plist\"))
            (require-not (literal \"/dev/null\"))
            (require-not (literal \"/dev/random\"))
            (require-not (literal \"/dev/urandom\"))
";
    const PROFILE_END: &str = "        ))
        (deny file-write* (require-all
            (require-not (subpath (param \"OUTPUT_ROOT\")))
            (require-not (subpath (param \"RUNTIME_ROOT\")))
            (require-not (literal \"/dev/null\"))))";
    ensure!(
        Path::new(SANDBOX_EXEC).is_file(),
        "macOS rulesync offline sandbox is unavailable"
    );
    let mut command = Command::new(SANDBOX_EXEC);
    let mut profile = String::from(PROFILE_START);
    // The renderer needs to read the directories along the canonical stage
    // path. Each literal exception applies to the directory itself.
    for (index, ancestor) in stage.ancestors().skip(1).enumerate() {
        let name = format!("STAGE_ANCESTOR_{index}");
        writeln!(
            profile,
            "            (require-not (literal (param \"{name}\")))"
        )
        .expect("append Seatbelt filter to profile");
        let mut param = OsString::from(name);
        param.push("=");
        param.push(ancestor);
        command.arg("-D").arg(param);
    }
    profile.push_str(PROFILE_END);
    command.args(["-p", &profile]);
    for (name, path) in [
        ("STAGE_ROOT=", stage),
        ("OUTPUT_ROOT=", output),
        ("RUNTIME_ROOT=", runtime),
        ("RENDERER=", binary),
        (
            "RENDERER_DIR=",
            binary.parent().context("staged renderer has no parent")?,
        ),
    ] {
        let mut param = OsString::from(name);
        param.push(path);
        command.arg("-D").arg(param);
    }
    command.arg(binary);
    Ok(command)
}

#[cfg(target_os = "linux")]
fn renderer_command(
    binary: &Path,
    _stage: &Path,
    output: &Path,
    _runtime: &Path,
) -> Result<Command> {
    use std::sync::OnceLock;

    static NETWORK_NAMESPACE: OnceLock<Option<(&str, &str)>> = OnceLock::new();
    let namespace = NETWORK_NAMESPACE.get_or_init(|| {
        let unshare = ["/usr/bin/unshare", "/bin/unshare"]
            .into_iter()
            .find(|path| Path::new(path).is_file())?;
        let probe = ["/usr/bin/true", "/bin/true"]
            .into_iter()
            .find(|path| Path::new(path).is_file())?;
        ["-n", "-Urn"]
            .into_iter()
            .find(|flag| {
                Command::new(unshare)
                    .args([flag, "--", probe])
                    .current_dir(output)
                    .env_clear()
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success())
            })
            .map(|flag| (unshare, flag))
    });
    if let Some((unshare, flag)) = namespace {
        let mut command = Command::new(unshare);
        command.args([*flag, "--"]).arg(binary);
        return Ok(command);
    }
    Ok(Command::new(binary))
}

#[cfg(target_os = "windows")]
fn renderer_command(
    binary: &Path,
    _stage: &Path,
    _output: &Path,
    _runtime: &Path,
) -> Result<Command> {
    Ok(Command::new(binary))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn renderer_command(
    _binary: &Path,
    _stage: &Path,
    _output: &Path,
    _runtime: &Path,
) -> Result<Command> {
    bail!("no supported rulesync renderer on {}", std::env::consts::OS)
}

fn configure_generate(
    command: &mut Command,
    input_roots: &[PathBuf],
    output: &Path,
    runtime: &Path,
    global: bool,
    targets: &[String],
    features: &[&str],
) {
    let home = if global { output } else { runtime };
    configure_environment(command, home, output, runtime);
    command.arg("generate");
    if global {
        command.arg("--global");
    }
    // The CLI treats commas in --output-roots as separators. Its cwd is an
    // empty directory, which also prevents loading rulesync settings.
    command
        .args(["--output-roots", "."])
        .arg("--targets")
        .arg(targets.join(","))
        .arg("--features")
        .arg(features.join(","))
        .arg("--input-roots")
        .args(input_roots);
}

fn configure_environment(command: &mut Command, home: &Path, output: &Path, runtime: &Path) {
    command
        .current_dir(output)
        .env_clear()
        .env("HOME", home)
        .env("HOME_DIR", home)
        .env("USERPROFILE", home)
        .env("TMPDIR", runtime)
        .env("DO_NOT_TRACK", "1")
        .stdin(Stdio::null());
    #[cfg(windows)]
    {
        command
            .env("APPDATA", home)
            .env("LOCALAPPDATA", home)
            .env("TEMP", runtime)
            .env("TMP", runtime);
        if let Some(system_root) = std::env::var_os("SystemRoot") {
            command.env("SystemRoot", system_root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    const RENDERER_CHILD_MODE: &str = "WENLAN_TEST_RENDERER_CHILD_MODE";

    fn stub_server(
        body: &'static [u8],
        hits: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) -> String {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut request = [0u8; 4096];
                let _ = stream.read(&mut request);
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn stub_pin(body: &[u8]) -> Pin {
        let digest = format!("{:x}", Sha256::digest(body));
        Pin {
            name: "rulesync-test",
            sha256: Box::leak(digest.into_boxed_str()),
        }
    }

    #[test]
    fn fetches_verifies_and_places_a_missing_binary_once() {
        let dir = tempfile::tempdir().unwrap();
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let base = stub_server(b"fake rulesync", hits.clone());
        let pin = stub_pin(b"fake rulesync");
        let path = fetch_pinned(dir.path(), &base, &pin).unwrap();
        assert_eq!(path, dir.path().join("rulesync-test"));
        assert_eq!(fs::read(&path).unwrap(), b"fake rulesync");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o755
            );
        }
        assert_eq!(fetch_pinned(dir.path(), &base, &pin).unwrap(), path);
        assert_eq!(hits.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn refuses_a_download_with_the_wrong_digest_and_leaves_no_file() {
        let dir = tempfile::tempdir().unwrap();
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let base = stub_server(b"tampered", hits);
        let error = fetch_pinned(dir.path(), &base, &stub_pin(b"fake rulesync")).unwrap_err();
        let message = format!("{error:#}");
        assert!(message.contains("checksum"), "{message}");
        assert!(message.contains("WENLAN_RULESYNC_BINARY"), "{message}");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[test]
    fn a_failed_connection_leaves_no_file_and_names_the_override() {
        let dir = tempfile::tempdir().unwrap();
        let error = fetch_pinned(dir.path(), "http://127.0.0.1:1", &stub_pin(b"x")).unwrap_err();
        assert!(format!("{error:#}").contains("WENLAN_RULESYNC_BINARY"));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn generate_failure_keeps_child_diagnostics_private() {
        let root = tempfile::tempdir().unwrap();
        let input = root.path().join("rule.md");
        let source = "private rule fixture for renderer diagnostics";
        fs::write(&input, source).unwrap();
        let mut command = Command::new("/bin/sh");
        command
            .current_dir(root.path())
            .env_clear()
            .args(["-c", "/bin/cat \"$1\"; /bin/cat \"$1\" >&2; exit 7", "sh"])
            .arg(input);
        let error = run_generate(&mut command, &["rules"])
            .unwrap_err()
            .to_string();
        assert!(error.contains('7'), "{error}");
        assert!(!error.contains(source), "{error}");
    }

    #[test]
    fn only_pinned_package_errors_are_recoverable() {
        assert!(is_skill_rejection(
            Some(1),
            b"Invalid frontmatter in SKILL.md"
        ));
        assert!(!is_skill_rejection(Some(1), b"unshare: unshare failed"));
        assert!(!is_skill_rejection(
            Some(71),
            b"sandbox-exec: sandbox_apply"
        ));
        assert!(!is_skill_rejection(
            None,
            b"Invalid frontmatter in SKILL.md"
        ));
    }

    #[test]
    #[ignore = "requires the local pinned rulesync binary"]
    fn render_session_reuses_only_its_verified_snapshot() {
        let binary = PathBuf::from(std::env::var_os("WENLAN_TEST_RULESYNC_BINARY").unwrap());
        let parent = new_stage(&std::env::temp_dir()).unwrap();
        let installed = parent.path().join("test-rulesync");
        fs::copy(&binary, &installed).unwrap();
        let session = RenderSession::new(&installed);
        let verified = session.binary(parent.path()).unwrap().to_owned();
        assert_ne!(verified, installed);

        fs::remove_file(&installed).unwrap();
        fs::write(&installed, b"different executable").unwrap();
        assert_eq!(session.binary(parent.path()).unwrap(), verified);
        assert_eq!(
            fs::metadata(&verified).unwrap().len(),
            fs::metadata(&binary).unwrap().len()
        );
        let err = RenderSession::new(&installed)
            .binary(parent.path())
            .unwrap_err();
        assert!(err.to_string().contains("checksum mismatch"));
        drop(session);
        assert!(!verified.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires a native macOS session allowed to apply a Seatbelt profile"]
    fn macos_sandbox_allows_staged_reads_and_denies_unrelated_reads() {
        let parent = tempfile::tempdir().unwrap();
        let temp = new_stage(parent.path()).unwrap();
        let stage = temp.path().canonicalize().unwrap();
        let input = stage.join("input");
        let output = stage.join("output");
        let work = stage.join("work");
        for dir in [&input, &output, &work] {
            fs::create_dir(dir).unwrap();
        }
        let inside = input.join("rule.md");
        let outside = parent.path().join("unrelated-home-rule.md");
        fs::write(&inside, "staged rule fixture\n").unwrap();
        fs::write(&outside, "unrelated rule fixture\n").unwrap();
        let binary = stage.join("probe");
        fs::copy("/bin/cat", &binary).unwrap();
        // A copied platform binary keeps Apple's signature and is killed at
        // launch; an ad-hoc signature lets the copy run as the probe.
        let signed = Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-"])
            .arg(&binary)
            .status()
            .unwrap();
        assert!(signed.success(), "ad-hoc signing the probe failed");
        let result = renderer_command(&binary, &stage, &output, &work)
            .unwrap()
            .env_clear()
            .current_dir(&output)
            .args([&inside, &outside])
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&result.stdout),
            "staged rule fixture\n",
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(!result.status.success());
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires the local pinned rulesync binary and a macOS session that permits sandbox-exec"]
    fn macos_sandbox_renders_personal_rules_with_pinned_binary() {
        let binary = PathBuf::from(
            std::env::var_os("WENLAN_TEST_RULESYNC_BINARY")
                .expect("set WENLAN_TEST_RULESYNC_BINARY to the local pinned release executable"),
        );
        let home = tempfile::tempdir().unwrap();
        let data = home.path().join("Wenlan \"測試\"").join("rulesync");
        fs::create_dir_all(&data).unwrap();
        let rule = "Keep the personal rules fixture in staging.";
        for target in crate::rulesync_personal::PERSONAL_TARGETS {
            let stage = new_stage(&data).unwrap();
            let input = stage.path().join("master");
            let output = stage.path().join("output");
            fs::create_dir(&output).unwrap();
            crate::rulesync_personal::write_root_rule(&input, rule).unwrap();
            render(
                &binary,
                stage.path(),
                &[input],
                &output,
                true,
                &[target.rules.renderer.into()],
                &["rules"],
            )
            .unwrap_or_else(|error| panic!("{}: {error:#}", target.id));
            assert!(fs::read_to_string(output.join(target.rules.staged))
                .unwrap()
                .contains(rule));
        }
    }

    #[test]
    fn renderer_child_fixture() {
        let Some(home) = std::env::var_os(RENDERER_CHILD_MODE).map(PathBuf::from) else {
            return;
        };
        for name in ["HOME", "HOME_DIR", "USERPROFILE"] {
            assert_eq!(std::env::var_os(name).as_deref(), Some(home.as_os_str()));
        }
        assert_eq!(std::env::var("DO_NOT_TRACK").as_deref(), Ok("1"));
        for name in ["OPENAI_API_KEY", "GITHUB_TOKEN", "SSH_AUTH_SOCK", "PATH"] {
            assert!(std::env::var_os(name).is_none(), "inherited {name}");
        }
        assert_eq!(
            std::env::current_dir().unwrap().canonicalize().unwrap(),
            home
        );
        #[cfg(windows)]
        for name in ["APPDATA", "LOCALAPPDATA"] {
            assert_eq!(std::env::var_os(name).as_deref(), Some(home.as_os_str()));
        }
        #[cfg(target_os = "linux")]
        if let Some(parent_namespace) = std::env::var_os("WENLAN_TEST_PARENT_NET_NS") {
            let child_namespace = fs::read_link("/proc/self/ns/net").unwrap();
            assert_ne!(child_namespace.as_os_str(), parent_namespace);
        }
        println!("verified renderer child environment");
    }

    #[test]
    fn renderer_child_uses_only_allowlisted_environment() {
        let temp = new_stage(&std::env::temp_dir()).unwrap();
        let stage = temp.path().canonicalize().unwrap();
        let output = stage.join("output with spaces 測試");
        let work = stage.join("work");
        fs::create_dir(&output).unwrap();
        fs::create_dir(&work).unwrap();
        let binary = std::env::current_exe().unwrap();
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        let mut command = renderer_command(&binary, &stage, &output, &work).unwrap();
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let mut command = Command::new(&binary);
        command
            .env("OPENAI_API_KEY", "fixture-key")
            .env("GITHUB_TOKEN", "fixture-token")
            .env("SSH_AUTH_SOCK", "fixture-socket");
        configure_environment(&mut command, &output, &output, &work);
        command.env(RENDERER_CHILD_MODE, &output).args([
            "--exact",
            "rulesync_renderer::tests::renderer_child_fixture",
            "--nocapture",
        ]);
        #[cfg(target_os = "linux")]
        if command.get_program() != binary.as_os_str() {
            let parent_namespace = fs::read_link("/proc/self/ns/net").unwrap();
            command.env("WENLAN_TEST_PARENT_NET_NS", parent_namespace);
        }
        let result = command.output().unwrap();
        let stdout = String::from_utf8_lossy(&result.stdout);
        assert!(
            result.status.success() && stdout.contains("verified renderer child environment"),
            "{stdout}{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }

    #[test]
    #[ignore = "requires a local verified rulesync release binary and a supported native launcher"]
    fn renders_rules_with_local_pinned_binary() {
        let binary = PathBuf::from(std::env::var_os("WENLAN_TEST_RULESYNC_BINARY").unwrap());
        let temp = new_stage(&std::env::temp_dir()).unwrap();
        let input = temp.path().join("input with spaces 測試");
        crate::rulesync_personal::write_root_rule(&input, "A staged rendering fixture.").unwrap();
        for global in [false, true] {
            let output = temp.path().join(format!("output,{global}"));
            fs::create_dir(&output).unwrap();
            render(
                &binary,
                temp.path(),
                std::slice::from_ref(&input),
                &output,
                global,
                &["codexcli".into()],
                &["rules"],
            )
            .unwrap();
            let relative = if global {
                ".codex/AGENTS.md"
            } else {
                "AGENTS.md"
            };
            assert!(fs::read_to_string(output.join(relative))
                .unwrap()
                .contains("A staged rendering fixture."));
        }
    }

    #[test]
    fn generate_only_uses_staging_paths_and_environment() {
        let stage = new_stage(&std::env::temp_dir()).unwrap();
        let output = stage.path().join("output,with spaces");
        let runtime = stage.path().join("runtime");
        let inputs = [stage.path().join("team"), stage.path().join("personal")];
        for global in [false, true] {
            let mut command = Command::new("unused-launcher");
            configure_generate(
                &mut command,
                &inputs,
                &output,
                &runtime,
                global,
                &["claudecode".into()],
                &["rules", "skills"],
            );
            let home = if global { &output } else { &runtime };
            assert_eq!(command.get_current_dir(), Some(output.as_path()));
            let mut expected_env = std::collections::BTreeMap::from([
                (OsStr::new("DO_NOT_TRACK"), Some(OsStr::new("1"))),
                (OsStr::new("HOME"), Some(home.as_os_str())),
                (OsStr::new("HOME_DIR"), Some(home.as_os_str())),
                (OsStr::new("USERPROFILE"), Some(home.as_os_str())),
                (OsStr::new("TMPDIR"), Some(runtime.as_os_str())),
            ]);
            #[cfg(windows)]
            let system_root = std::env::var_os("SystemRoot");
            #[cfg(windows)]
            {
                expected_env.extend([
                    (OsStr::new("APPDATA"), Some(home.as_os_str())),
                    (OsStr::new("LOCALAPPDATA"), Some(home.as_os_str())),
                    (OsStr::new("TEMP"), Some(runtime.as_os_str())),
                    (OsStr::new("TMP"), Some(runtime.as_os_str())),
                ]);
                if let Some(root) = system_root.as_deref() {
                    expected_env.insert(OsStr::new("SystemRoot"), Some(root));
                }
            }
            #[cfg(not(windows))]
            let _ = &mut expected_env;
            assert_eq!(
                command
                    .get_envs()
                    .collect::<std::collections::BTreeMap<_, _>>(),
                expected_env
            );
            let mut expected = vec![OsStr::new("generate")];
            if global {
                expected.push(OsStr::new("--global"));
            }
            expected.extend(
                [
                    "--output-roots",
                    ".",
                    "--targets",
                    "claudecode",
                    "--features",
                    "rules,skills",
                    "--input-roots",
                ]
                .map(OsStr::new),
            );
            expected.extend(inputs.iter().map(|path| path.as_os_str()));
            assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
        }
    }

    #[test]
    fn refuses_settings_and_option_like_target_arguments() {
        assert!(validate_selection(&["--delete".into()], &["rules"]).is_err());
        assert!(validate_selection(&["claudecode,codexcli".into()], &["rules"]).is_err());
        assert!(validate_selection(&["*".into()], &["rules"]).is_err());
        for feature in ["mcp", "hooks", "permissions"] {
            assert!(validate_selection(&["claudecode".into()], &["rules", feature]).is_err());
        }
        assert!(validate_selection(&["claudecode".into()], &["rules", "skills"]).is_ok());
    }

    #[test]
    fn refuses_outside_overlapping_and_reused_stage_paths() {
        let stage = new_stage(&std::env::temp_dir()).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let input = stage.path().join("input");
        let output = stage.path().join("output");
        fs::create_dir(&input).unwrap();
        fs::create_dir(&output).unwrap();
        assert!(validate_roots(stage.path(), std::slice::from_ref(&input), &output).is_ok());
        assert!(
            validate_roots(stage.path(), std::slice::from_ref(&input), outside.path()).is_err()
        );
        assert!(validate_roots(stage.path(), std::slice::from_ref(&input), &input).is_err());
        fs::write(output.join("rulesync.jsonc"), b"{\"delete\":true}").unwrap();
        assert!(validate_roots(stage.path(), &[input], &output).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_ancestors_and_linked_input_files() {
        use std::os::unix::fs::symlink;

        let stage = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let input = stage.path().join("input");
        let output = stage.path().join("output");
        fs::create_dir_all(outside.path().join("nested")).unwrap();
        fs::write(outside.path().join("outside.md"), "outside").unwrap();
        fs::create_dir(&input).unwrap();
        fs::create_dir(&output).unwrap();
        let link = stage.path().join("linked");
        symlink(outside.path(), &link).unwrap();
        assert!(validate_roots(stage.path(), &[link.join("nested")], &output).is_err());
        assert!(validate_roots(
            stage.path(),
            std::slice::from_ref(&input),
            &link.join("nested")
        )
        .is_err());
        symlink(outside.path().join("outside.md"), input.join("rule.md")).unwrap();
        assert!(validate_roots(stage.path(), std::slice::from_ref(&input), &output).is_err());
        fs::remove_file(input.join("rule.md")).unwrap();
        fs::hard_link(outside.path().join("outside.md"), input.join("rule.md")).unwrap();
        assert!(validate_roots(stage.path(), &[input], &output).is_err());
    }

    #[test]
    fn copies_then_verifies_the_snapshot_to_be_executed() {
        let stage = tempfile::tempdir().unwrap();
        let source = stage.path().join("binary");
        let destination = stage.path().join("verified");
        fs::write(&source, b"abc").unwrap();
        let pin = Pin {
            name: "test-pin",
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        };
        copy_verified_binary(&source, &destination, &pin).unwrap();
        fs::write(&source, b"different").unwrap();
        assert_eq!(fs::read(destination).unwrap(), b"abc");
        let error =
            copy_verified_binary(&source, &stage.path().join("unverified"), &pin).unwrap_err();
        assert!(error.to_string().contains("checksum mismatch"));
    }
}
