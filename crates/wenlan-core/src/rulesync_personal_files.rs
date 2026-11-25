// SPDX-License-Identifier: Apache-2.0
//! Native global command and subagent files staged through the pinned renderer.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsStr,
    io,
    path::{Component, Path, PathBuf},
};

use anyhow::{ensure, Context, Result};
use cap_fs_ext::{DirEntryExt as _, MetadataExt as _};
use cap_std::fs::Metadata;
use serde_yaml::{Mapping, Value};

use crate::store::format::{normalize_line_ends, split_frontmatter};

pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// `sharing_paths` reason for a tool that has no folder for this kind of file.
pub const NO_PERSONAL_DESTINATION: &str =
    "This tool has no supported personal destination for this file type.";

pub const NON_ASCII_PERSONAL_NAME: &str =
    "the file or directory has no verified ASCII name. Its case aliases cannot be checked safely";

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum PersonalFeature {
    Commands,
    Subagents,
}

impl PersonalFeature {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Commands => "commands",
            Self::Subagents => "subagents",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct PersonalFileLocation {
    pub tool: &'static str,
    pub feature: PersonalFeature,
    pub directory: &'static str,
    suffix: &'static str,
    pub nested: bool,
}

impl PersonalFileLocation {
    const fn markdown(
        tool: &'static str,
        feature: PersonalFeature,
        directory: &'static str,
        nested: bool,
    ) -> Self {
        Self {
            tool,
            feature,
            directory,
            nested,
            suffix: ".md",
        }
    }

    pub fn file_name_matches(self, name: &str) -> bool {
        !name.starts_with('.') && name.ends_with(self.suffix)
    }

    pub fn name_at(self, path: &Path) -> Option<PathBuf> {
        let relative = path.strip_prefix(self.directory).ok()?;
        if !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        {
            return None;
        }
        let parent = relative.parent()?;
        if !self.nested && !parent.as_os_str().is_empty() {
            return None;
        }
        let name = relative.file_name()?.to_str()?.strip_suffix(self.suffix)?;
        if !portable_component(name)
            || !parent
                .iter()
                .all(|part| part.to_str().is_some_and(portable_component))
        {
            return None;
        }
        Some(parent.join(name))
    }

    fn path_for(self, name: &Path) -> Option<PathBuf> {
        let parent = name.parent()?;
        if !self.nested && !parent.as_os_str().is_empty() {
            return None;
        }
        let leaf = name.file_name()?.to_str()?;
        Some(
            Path::new(self.directory)
                .join(parent)
                .join(format!("{leaf}{}", self.suffix)),
        )
    }

    fn is_toml(self) -> bool {
        self.suffix == ".toml"
    }

    /// Stage a native file as a rulesync source, with the original bytes kept by
    /// the caller. Unparseable or unrepresentable files can still be copied.
    pub fn stage(self, path: &Path, bytes: &[u8]) -> Result<(PathBuf, Vec<u8>)> {
        let name = self.name_at(path).context("invalid personal file name")?;
        let document = Document::read(self, bytes)?;
        let mut source = Mapping::new();
        source.insert("targets".into(), serde_yaml::to_value([self.tool])?);
        let mut native = document.fields;
        let description = native
            .remove("description")
            .unwrap_or(Value::String(String::new()));
        source.insert("description".into(), description);
        if self.feature == PersonalFeature::Subagents {
            let name = native.remove("name").unwrap_or_else(|| {
                Value::String(
                    name.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .into_owned(),
                )
            });
            source.insert("name".into(), name);
        }
        source.insert(self.tool.into(), Value::Mapping(native));
        let base = name
            .file_name()
            .and_then(|name| name.to_str())
            .context("invalid personal file name")?;
        let path = Path::new(self.feature.as_str())
            .join(name.parent().context("invalid personal file name")?)
            .join(format!("{base}.md"));
        let yaml = serde_yaml::to_string(&source)?;
        Ok((
            path,
            format!("---\n{yaml}---\n{}", document.body).into_bytes(),
        ))
    }
}

use PersonalFeature::{Commands, Subagents};

pub const LOCATIONS: &[PersonalFileLocation] = &[
    PersonalFileLocation::markdown("claudecode", Commands, ".claude/commands", true),
    PersonalFileLocation::markdown("claudecode", Subagents, ".claude/agents", true),
    PersonalFileLocation::markdown("codexcli", Commands, ".codex/prompts", false),
    PersonalFileLocation {
        tool: "codexcli",
        feature: Subagents,
        directory: ".codex/agents",
        suffix: ".toml",
        nested: false,
    },
    PersonalFileLocation {
        tool: "geminicli",
        feature: Commands,
        directory: ".gemini/commands",
        suffix: ".toml",
        nested: true,
    },
    PersonalFileLocation::markdown("geminicli", Subagents, ".gemini/agents", false),
    PersonalFileLocation::markdown("cursor", Commands, ".cursor/commands", false),
    PersonalFileLocation::markdown("cursor", Subagents, ".cursor/agents", false),
    PersonalFileLocation::markdown("antigravity-ide", Subagents, ".gemini/config/agents", false),
    PersonalFileLocation {
        tool: "copilotcli",
        feature: Subagents,
        directory: ".copilot/agents",
        suffix: ".agent.md",
        nested: false,
    },
    PersonalFileLocation::markdown("opencode", Commands, ".config/opencode/commands", true),
    PersonalFileLocation::markdown("opencode", Subagents, ".config/opencode/agents", true),
    // OpenCode still discovers both legacy spellings. Prefer the plural
    // locations above only when creating a new command or agent.
    PersonalFileLocation::markdown("opencode", Commands, ".config/opencode/command", true),
    PersonalFileLocation::markdown("opencode", Subagents, ".config/opencode/agent", true),
    PersonalFileLocation::markdown("musecode", Subagents, ".config/muse/agents", false),
];

fn portable_component(part: &str) -> bool {
    !part.is_empty() && !part.starts_with('.') && !part.contains(['\\', ':'])
}

pub fn location_of(path: &Path) -> Option<PersonalFileLocation> {
    LOCATIONS
        .iter()
        .copied()
        .find(|spec| spec.name_at(path).is_some())
}

pub fn is_personal_path(path: &Path) -> bool {
    LOCATIONS
        .iter()
        .any(|location| path.starts_with(location.directory))
}

/// A single directory entry looked up by the filesystem. Two spellings that
/// resolve to this identity are aliases. Multiple hard links are distinct
/// entries and must continue through the normal link-safety checks.
pub fn directory_entry_identity(metadata: &Metadata) -> Option<(u64, u64)> {
    let inode = metadata.ino();
    (inode != 0 && (metadata.is_dir() || metadata.nlink() == 1)).then(|| (metadata.dev(), inode))
}

/// A spelling supplied by the caller can alias an on-disk Unicode name.
pub fn require_ascii_entry_name(
    parent: &cap_std::fs::Dir,
    name: &OsStr,
    opened: &Metadata,
) -> io::Result<()> {
    if name.is_ascii() {
        if directory_is_case_sensitive(parent) {
            return Ok(());
        }
        for entry in parent.entries()? {
            let entry = entry?;
            if entry.file_name().eq_ignore_ascii_case(name) {
                // Listing metadata has no volume or file index on Windows, and
                // cap-fs-ext panics when asked for them. Open the entry instead.
                let metadata = entry.full_metadata()?;
                if opened.ino() != 0
                    && (opened.dev(), opened.ino()) == (metadata.dev(), metadata.ino())
                {
                    return Ok(());
                }
            }
        }
    }
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        NON_ASCII_PERSONAL_NAME,
    ))
}

/// Use a proven case-sensitive directory to allow distinct case-only names.
/// When the filesystem cannot attest to its lookup rules, reserve the alias.
#[cfg(target_os = "macos")]
pub fn directory_is_case_sensitive(dir: &cap_std::fs::Dir) -> bool {
    use std::os::fd::AsRawFd as _;

    // SAFETY: dir owns the open descriptor for the duration of this query.
    unsafe { libc::fpathconf(dir.as_raw_fd(), libc::_PC_CASE_SENSITIVE) == 1 }
}

#[cfg(target_os = "linux")]
pub fn directory_is_case_sensitive(dir: &cap_std::fs::Dir) -> bool {
    use rustix::fs::{fstatfs, ioctl_getflags};

    const FS_CASEFOLD_FL: u32 = 0x4000_0000;
    // A cap-std directory on Linux can be O_PATH, which cannot query flags.
    let Ok(file) = dir.open(".") else {
        return false;
    };
    let Ok(stat) = fstatfs(&file) else {
        return false;
    };
    match stat.f_type {
        libc::TMPFS_MAGIC
        | libc::BTRFS_SUPER_MAGIC
        | libc::EXT4_SUPER_MAGIC
        | libc::F2FS_SUPER_MAGIC
        | libc::OVERLAYFS_SUPER_MAGIC => {
            ioctl_getflags(&file).is_ok_and(|flags| flags.bits() & FS_CASEFOLD_FL == 0)
        }
        _ => false,
    }
}

#[cfg(windows)]
pub fn directory_is_case_sensitive(dir: &cap_std::fs::Dir) -> bool {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Storage::FileSystem::{
        FileCaseSensitiveInfo, GetFileInformationByHandleEx, FILE_CASE_SENSITIVE_INFO,
    };

    const FILE_CS_FLAG_CASE_SENSITIVE_DIR: u32 = 1;
    let mut info = FILE_CASE_SENSITIVE_INFO::default();
    // SAFETY: the directory handle is open and info is an output buffer of the given size.
    let ok = unsafe {
        GetFileInformationByHandleEx(
            dir.as_raw_handle(),
            FileCaseSensitiveInfo,
            std::ptr::from_mut(&mut info).cast(),
            std::mem::size_of_val(&info) as u32,
        )
    };
    ok != 0 && info.Flags & FILE_CS_FLAG_CASE_SENSITIVE_DIR != 0
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
pub fn directory_is_case_sensitive(_dir: &cap_std::fs::Dir) -> bool {
    false
}

pub fn alternate_paths(path: &Path) -> Vec<PathBuf> {
    let Some(location) = location_of(path) else {
        return Vec::new();
    };
    let Some(name) = location.name_at(path) else {
        return Vec::new();
    };
    LOCATIONS
        .iter()
        .filter(|other| {
            other.tool == location.tool
                && other.feature == location.feature
                && other.directory != location.directory
        })
        .filter_map(|other| other.path_for(&name))
        .collect()
}

struct Document {
    fields: Mapping,
    body: String,
}

impl Document {
    fn read(location: PersonalFileLocation, bytes: &[u8]) -> Result<Self> {
        let text = std::str::from_utf8(bytes)?.trim_start_matches('\u{feff}');
        ensure!(!text.contains('\0'), "the file contains a NUL");
        if location.is_toml() {
            let mut fields: toml::Table = text.parse()?;
            let instructions = match location.feature {
                Commands => "prompt",
                Subagents => "developer_instructions",
            };
            let body = fields
                .remove(instructions)
                .and_then(|instructions| instructions.as_str().map(str::to_owned))
                .with_context(|| format!("the native file has no {instructions} text"))?;
            return Ok(Self {
                fields: serde_yaml::from_value(serde_yaml::to_value(fields)?)?,
                body,
            });
        }
        let text = normalize_line_ends(text);
        Ok(match split_frontmatter(&text)? {
            Some(split) => Self {
                fields: split.frontmatter,
                body: split.body.to_owned(),
            },
            None => Self {
                fields: Mapping::new(),
                body: text.into_owned(),
            },
        })
    }
}

/// A shared format has the same meaning in the destination, including its
/// invocation and permission fields. A common extension alone is insufficient.
pub fn sharing_paths(
    path: &Path,
    bytes: &[u8],
    enrolled_tools: &BTreeMap<String, String>,
) -> (BTreeSet<PathBuf>, BTreeMap<String, &'static str>) {
    let mut paths = BTreeSet::from([path.to_owned()]);
    let mut skipped = BTreeMap::new();
    let Some(source) = location_of(path) else {
        return (paths, skipped);
    };
    let Some(name) = source.name_at(path) else {
        return (paths, skipped);
    };
    let document = Document::read(source, bytes);
    let directive = if source.feature != Commands {
        None
    } else if bytes.contains(&b'$') {
        Some("Left unmanaged: '$' can introduce argument placeholders. The command stays in its originating tool.")
    } else if bytes.contains(&b'!') {
        Some("Left unmanaged: '!' can introduce shell or context directives. The command stays in its originating tool.")
    } else {
        None
    };
    for tool in enrolled_tools
        .keys()
        .filter(|tool| tool.as_str() != source.tool)
    {
        let destination = (|| {
            let target = LOCATIONS
                .iter()
                .copied()
                .find(|target| target.tool == tool.as_str() && target.feature == source.feature)
                .ok_or(NO_PERSONAL_DESTINATION)?;
            if !path.as_os_str().is_ascii() {
                return Err(NON_ASCII_PERSONAL_NAME);
            }
            if let Some(reason) = directive {
                return Err(reason);
            }
            let document = document
                .as_ref()
                .map_err(|_| "The source could not be read as a portable command or subagent.")?;
            check_compatibility(source, target, &name, document)?;
            target
                .path_for(&name)
                .ok_or("This tool does not support this nested name.")
        })();
        match destination {
            Ok(path) => {
                paths.insert(path);
            }
            Err(reason) => {
                skipped.insert(tool.clone(), reason);
            }
        }
    }
    (paths, skipped)
}

fn check_compatibility(
    source: PersonalFileLocation,
    target: PersonalFileLocation,
    name: &Path,
    document: &Document,
) -> std::result::Result<(), &'static str> {
    let text_fields = |allowed: &[&str]| {
        document.fields.iter().all(|(key, value)| {
            key.as_str().is_some_and(|key| allowed.contains(&key)) && value.is_string()
        })
    };
    match source.feature {
        Commands => {
            if source.is_toml() || target.is_toml() {
                return Err("Gemini CLI uses its own native TOML command format.");
            }
            if name.components().count() > 1 {
                return Err("Other tools can give a nested command a different invocation name.");
            }
            let hints = |tool| matches!(tool, "claudecode" | "codexcli");
            let fields: &[&str] = if hints(source.tool) && hints(target.tool) {
                &["description", "argument-hint"]
            } else {
                &["description"]
            };
            if !text_fields(fields) {
                return Err(
                    "The tools do not interpret the command's frontmatter fields the same way.",
                );
            }
            // Treat possible shell and file directives conservatively, even
            // inside code examples. Tools preprocess the body before sending it.
            if document.body.contains('@')
                || (matches!(source.tool, "claudecode") || matches!(target.tool, "claudecode"))
                    && document.body.contains("ultrathink")
            {
                return Err("The tools can interpret shell or context directives differently.");
            }
        }
        Subagents => {
            if document
                .fields
                .get("name")
                .and_then(Value::as_str)
                .is_some_and(|name| name.contains(':'))
            {
                return Err(
                    "Its name contains a colon, which Claude Code reserves for plugin subagents.",
                );
            }
            // OpenCode's mode and permissions model is specific to that tool.
            if source.is_toml()
                || target.is_toml()
                || source.tool == "opencode"
                || target.tool == "opencode"
            {
                return Err("The tools use different subagent formats.");
            }
            if document
                .fields
                .get("name")
                .and_then(Value::as_str)
                .is_none_or(|name| name.trim().is_empty())
                || document
                    .fields
                    .get("description")
                    .and_then(Value::as_str)
                    .is_none_or(|description| description.trim().is_empty())
                || !text_fields(&["name", "description"])
            {
                return Err("The subagent's frontmatter is specific to its tool.");
            }
        }
    }
    Ok(())
}

pub fn require_plain_personal_directory(dir: &cap_std::fs::Dir) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        require_plain_unix_acl(&dir.open(".")?)
    }
    #[cfg(windows)]
    {
        // Windows checks the descriptor of the entry it replaces. Traversing
        // a directory does not require its permissions to have a given shape.
        let _ = dir;
        Ok(())
    }
}

pub fn require_plain_personal_file(
    file: &cap_std::fs::File,
    metadata: &Metadata,
) -> io::Result<()> {
    if cfg!(windows) && metadata.permissions().readonly() {
        return Err(readonly_personal_file());
    }
    if metadata.nlink() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "the file has multiple hard links. Its link will be kept in place",
        ));
    }
    #[cfg(unix)]
    require_plain_unix_acl(file)?;
    #[cfg(windows)]
    crate::rulesync_personal::check_windows_replacement(file)?;
    Ok(())
}

pub fn readonly_personal_file() -> io::Error {
    io::Error::new(
        io::ErrorKind::Unsupported,
        "Windows cannot safely replace this read-only personal configuration file",
    )
}

#[cfg(target_os = "macos")]
pub fn require_plain_macos_link(parent: &cap_std::fs::Dir, name: &Path) -> io::Result<()> {
    use rustix::fs::{openat, Mode, OFlags};

    if name.file_name() != Some(name.as_os_str()) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "expected a link name",
        ));
    }
    // O_SYMLINK opens the leaf link itself. The parent is an open directory
    // and name is a single component, so no part of this open follows a link.
    let link = openat(
        parent,
        name,
        OFlags::CLOEXEC | OFlags::NONBLOCK | OFlags::from_bits_retain(libc::O_SYMLINK as _),
        Mode::empty(),
    )?;
    let link = cap_std::fs::File::from_std(link.into());
    if !link.metadata()?.is_symlink() {
        return Err(io::Error::other(
            "the path changed while checking the link ACL",
        ));
    }
    require_plain_unix_acl(&link)
}

/// Leave access controls in place when mode bits cannot represent them.
#[cfg(unix)]
pub fn require_plain_unix_acl(file: &cap_std::fs::File) -> std::io::Result<()> {
    use std::io;

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    {
        #[cfg(target_os = "macos")]
        if has_macos_acl(file)? {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the path has an extended ACL. It stays unmanaged",
            ));
        }
        // Query the opened file, including ACL xattrs supplied by network
        // filesystems. A failed query leaves the file unmanaged as well.
        let count = rustix::fs::flistxattr_query(file)?;
        if count == 0 {
            return Ok(());
        }
        let mut names = vec![0; count];
        let count = rustix::fs::flistxattr(file, &mut names[..])?;
        if names[..count].split(|byte| *byte == 0).any(|name| {
            name.windows(3)
                .any(|part| part.eq_ignore_ascii_case(b"acl"))
        }) {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "the path has an ACL extended attribute. It stays unmanaged",
            ));
        }
        Ok(())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = file;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "could not verify the path's ACL. It stays unmanaged",
        ))
    }
}

#[cfg(target_os = "macos")]
fn has_macos_acl(file: &cap_std::fs::File) -> std::io::Result<bool> {
    use std::{ffi::c_void, io, os::fd::AsRawFd as _};

    unsafe extern "C" {
        fn acl_get_fd(fd: libc::c_int) -> *mut c_void;
        fn acl_free(acl: *mut c_void) -> libc::c_int;
    }

    // SAFETY: file keeps the checked descriptor open. acl_get_fd returns an
    // allocated extended ACL on Darwin, or NULL on failure or when absent.
    let acl = unsafe { acl_get_fd(file.as_raw_fd()) };
    if acl.is_null() {
        let error = io::Error::last_os_error();
        return match error.raw_os_error() {
            Some(libc::ENOENT | libc::ENOATTR | libc::ENOTSUP) => Ok(false),
            _ => Err(error),
        };
    }
    // SAFETY: this non-null value was allocated by acl_get_fd above.
    unsafe { acl_free(acl) };
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rulesync_harness_retro_stays_with_its_originating_tool() {
        let tools = LOCATIONS
            .iter()
            .map(|loc| (loc.tool.to_owned(), String::new()))
            .collect();
        let source = Path::new(".claude/commands/harness-retro.md");
        let command = include_bytes!("../tests/fixtures/rulesync/harness-retro.md");
        assert_eq!(
            sharing_paths(source, command, &tools).0,
            BTreeSet::from([source.to_owned()])
        );
    }

    #[test]
    fn rulesync_round5_leaves_directive_commands_native() {
        let tools = ["claudecode", "opencode", "cursor"]
            .into_iter()
            .map(|tool| (tool.to_owned(), String::new()))
            .collect();
        let source = Path::new(".claude/commands/check.md");
        for text in [
            "Echo $ARGUMENTS.\n",
            "Check the file at \"$S/all.json\".\n",
            "---\ndescription: Check $name\n---\nReview the changes.\n",
            "Run !`git status`.\n",
            "Run ! git status.\n",
        ] {
            let (paths, skipped) = sharing_paths(source, text.as_bytes(), &tools);
            assert_eq!(paths, BTreeSet::from([source.to_owned()]), "{text}");
            assert!(skipped["opencode"].contains("stays in its originating tool"));
            assert!(skipped["cursor"].contains("stays in its originating tool"));
        }
        let (paths, skipped) = sharing_paths(source, b"Review the changes.\n", &tools);
        assert!(paths.contains(Path::new(".config/opencode/commands/check.md")));
        assert!(paths.contains(Path::new(".cursor/commands/check.md")));
        assert!(skipped.is_empty());
    }

    #[test]
    fn rulesync_command_arguments_stay_with_their_originating_tool() {
        let tools = ["claudecode", "opencode", "codexcli", "cursor"]
            .into_iter()
            .map(|tool| (tool.to_owned(), String::new()))
            .collect();
        let claude = Path::new(".claude/commands/check.md");
        let opencode = Path::new(".config/opencode/commands/check.md");
        let codex = Path::new(".codex/prompts/check.md");
        let cursor = Path::new(".cursor/commands/check.md");
        for (source, body) in [
            (claude, "Use $ARGUMENTS."),
            (opencode, "Use $ARGUMENTS."),
            (codex, "Use $ARGUMENTS."),
            (claude, "Use $ARGUMENTS[0]."),
            (claude, r"Keep \$ARGUMENTS literal."),
            (opencode, "Use $ARGUMENTS_suffix."),
            (claude, "Use ${CLAUDE_SESSION_ID}."),
            (opencode, "Use ${CLAUDE_SESSION_ID}."),
            (claude, "Use $0 and $1."),
            (opencode, "Use $10."),
            (codex, "Use $9 or $FILE."),
            (cursor, "Use $$."),
            (claude, "Use $local and ${HOME}."),
        ] {
            assert_eq!(
                sharing_paths(source, body.as_bytes(), &tools).0,
                BTreeSet::from([source.to_owned()]),
                "{}: {body}",
                source.display()
            );
        }
    }

    #[test]
    fn rulesync_round4_keeps_codex_placeholder_prefixes_native() {
        let tools = [("codexcli".to_owned(), String::new())].into();
        let source = Path::new(".cursor/commands/check.md");
        for body in ["Keep $10.", "Keep $1suffix.", "Keep $NAMEsuffix."] {
            let (paths, skipped) = sharing_paths(source, body.as_bytes(), &tools);
            assert_eq!(paths, BTreeSet::from([source.to_owned()]), "{body}");
            assert!(skipped["codexcli"].contains("argument placeholders"));
        }
    }

    #[test]
    fn rulesync_round4_keeps_executable_command_directives_native() {
        let tools = ["claudecode", "opencode"]
            .into_iter()
            .map(|tool| (tool.to_owned(), String::new()))
            .collect();
        let source = Path::new(".codex/prompts/check.md");
        for body in [
            "Review.\n```!\n:\n```\n",
            "Review !`:`.",
            "Review (@README.md).",
        ] {
            let (paths, skipped) = sharing_paths(source, body.as_bytes(), &tools);
            assert_eq!(paths, BTreeSet::from([source.to_owned()]), "{body}");
            assert!(skipped["claudecode"].contains("shell or context directives"));
        }
    }

    #[test]
    fn personal_file_paths_are_relative_to_the_macos_linux_and_windows_home() {
        let paths = [
            ("claudecode", Commands, ".claude/commands/review.md"),
            ("claudecode", Subagents, ".claude/agents/review.md"),
            ("codexcli", Commands, ".codex/prompts/review.md"),
            ("codexcli", Subagents, ".codex/agents/review.toml"),
            ("geminicli", Commands, ".gemini/commands/review.toml"),
            ("geminicli", Subagents, ".gemini/agents/review.md"),
            ("cursor", Commands, ".cursor/commands/review.md"),
            ("cursor", Subagents, ".cursor/agents/review.md"),
            (
                "antigravity-ide",
                Subagents,
                ".gemini/config/agents/review.md",
            ),
            ("copilotcli", Subagents, ".copilot/agents/review.agent.md"),
            ("opencode", Commands, ".config/opencode/commands/review.md"),
            ("opencode", Subagents, ".config/opencode/agents/review.md"),
            ("musecode", Subagents, ".config/muse/agents/review.md"),
        ];
        for home in ["/Users/Fixture", "/home/fixture", "C:/Users/Fixture"] {
            let home = Path::new(home);
            for (tool, feature, expected) in paths {
                let location = location_of(Path::new(expected)).unwrap();
                assert_eq!((location.tool, location.feature), (tool, feature));
                let path = home.join(location.path_for(Path::new("review")).unwrap());
                let relative = path.strip_prefix(home).unwrap();
                assert_eq!(relative, Path::new(expected), "home {home:?}");
                assert_eq!(location.name_at(relative), Some("review".into()));
            }
        }
        assert!(location_of(Path::new(".claude/commands/git/review.md")).is_some());
        assert!(location_of(Path::new(".gemini/commands/git/review.toml")).is_some());
        assert!(location_of(Path::new(".config/opencode/commands/git/review.md")).is_some());
        assert!(location_of(Path::new(".cursor/commands/git/review.md")).is_none());
        assert!(location_of(Path::new(".claude/commands/../agents/review.md")).is_none());
        assert!(location_of(Path::new(".codex/agents/review.md")).is_none());
        assert!(location_of(Path::new(".cursorignore")).is_none());
    }

    #[test]
    fn personal_file_sharing_uses_compatible_native_fields_and_namespaces() {
        let tools = LOCATIONS
            .iter()
            .map(|loc| (loc.tool.to_owned(), String::new()))
            .collect();
        let agent = Path::new(".claude/agents/build.md");
        let portable = b"---\nname: build\ndescription: Build checklist\n---\nReview.\n";
        let (paths, _) = sharing_paths(agent, portable, &tools);
        assert!(paths.contains(Path::new(".cursor/agents/build.md")));
        assert!(paths.contains(Path::new(".copilot/agents/build.agent.md")));
        assert!(!paths.contains(Path::new(".codex/agents/build.toml")));
        assert!(!paths.contains(Path::new(".config/opencode/agents/build.md")));
        let restricted =
            b"---\nname: build\ndescription: Build checklist\ntools: Read\n---\nReview.\n";
        assert_eq!(
            sharing_paths(agent, restricted, &tools).0,
            BTreeSet::from([agent.to_owned()])
        );
        let nested = Path::new(".claude/commands/git/check.md");
        assert_eq!(
            sharing_paths(nested, b"Check $ARGUMENTS.\n", &tools).0,
            BTreeSet::from([nested.to_owned()])
        );
        let root = Path::new(".claude/commands/check.md");
        assert!(sharing_paths(root, b"Check the output.\n", &tools)
            .0
            .contains(Path::new(".codex/prompts/check.md")));
        let (_, skipped) = sharing_paths(root, b"Check $FILE.\n", &tools);
        assert!(skipped["codexcli"].contains("argument placeholders"));
        let inline = b"Read !`git diff` before running.\n";
        assert_eq!(
            sharing_paths(root, inline, &tools).0,
            BTreeSet::from([root.to_owned()])
        );
        assert!(!sharing_paths(root, b"Review.\n", &tools)
            .0
            .contains(Path::new(".gemini/commands/check.toml")));
        let nested = Path::new(".config/opencode/commands/git/check.md");
        assert_eq!(
            sharing_paths(nested, b"Check the output.\n", &tools).0,
            BTreeSet::from([nested.to_owned()])
        );
    }
}
