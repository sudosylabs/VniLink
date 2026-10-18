//! The shells a Windows computer can offer, found the way VS Code finds its
//! terminal profiles (platform/terminal/node/terminalProfiles.ts and
//! base/node/powershell.ts): PowerShell first, then Windows PowerShell, Git
//! Bash, Command Prompt and WSL distributions. Cygwin, MSYS2 and Cmder are left
//! out; their default paths sit at the root of a drive any user can write to.
use std::{
    ffi::OsString,
    path::{Path, PathBuf},
};
use vnilink::terminal::ShellChoice;

/// What detection reads from the computer; tests supply their own.
pub(crate) trait ShellHost {
    fn env(&self, name: &str) -> Option<OsString>;
    fn is_file(&self, path: &Path) -> bool;
    fn list_dir(&self, path: &Path) -> Vec<String>;
    /// `wsl.exe -l -q` output, or nothing when it failed or took too long.
    fn wsl_list(&self, wsl: &Path) -> Option<Vec<u8>>;
}

fn env_path(host: &dyn ShellHost, name: &str) -> Option<PathBuf> {
    host.env(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn join(base: &Path, parts: &[&str]) -> PathBuf {
    parts
        .iter()
        .fold(base.to_path_buf(), |path, part| path.join(part))
}

/// Program Files directories, without repeats: the native one first.
fn program_files(host: &dyn ShellHost) -> Vec<PathBuf> {
    let mut bases = Vec::new();
    for name in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"] {
        if let Some(base) = env_path(host, name).filter(|base| !bases.contains(base)) {
            bases.push(base);
        }
    }
    bases
}

/// PowerShell 7 or later, in VS Code's search order without preview builds:
/// the highest numbered install under Program Files, the Store package, the
/// .NET global tool, then Scoop.
fn find_pwsh(host: &dyn ShellHost) -> Option<PathBuf> {
    for base in program_files(host) {
        let root = base.join("PowerShell");
        let newest = host
            .list_dir(&root)
            .into_iter()
            .filter_map(|name| name.parse::<u32>().ok())
            .max();
        if let Some(version) = newest {
            let pwsh = join(&root, &[&version.to_string(), "pwsh.exe"]);
            if host.is_file(&pwsh) {
                return Some(pwsh);
            }
        }
    }
    if let Some(apps) =
        env_path(host, "LOCALAPPDATA").map(|local| join(&local, &["Microsoft", "WindowsApps"]))
    {
        let mut packages: Vec<_> = host
            .list_dir(&apps)
            .into_iter()
            .filter(|name| name.starts_with("Microsoft.PowerShell_"))
            .collect();
        packages.sort();
        if let Some(pwsh) = packages
            .into_iter()
            .map(|package| join(&apps, &[&package, "pwsh.exe"]))
            .find(|pwsh| host.is_file(pwsh))
        {
            return Some(pwsh);
        }
    }
    let home = env_path(host, "USERPROFILE")?;
    [
        join(&home, &[".dotnet", "tools", "pwsh.exe"]),
        join(&home, &["scoop", "apps", "pwsh", "current", "pwsh.exe"]),
    ]
    .into_iter()
    .find(|pwsh| host.is_file(pwsh))
}

/// Git for Windows' bash: next to `git.exe` on PATH, then the usual install
/// directories, then Scoop.
fn find_git_bash(host: &dyn ShellHost) -> Option<PathBuf> {
    let mut bases = Vec::new();
    let path = host.env("PATH").unwrap_or_default();
    if let Some(git) = path
        .to_string_lossy()
        .split(';')
        .filter(|dir| !dir.is_empty())
        .map(|dir| PathBuf::from(dir).join("git.exe"))
        .find(|git| host.is_file(git))
    {
        // <base>\Git\cmd\git.exe: the install's parent directory.
        if let Some(base) = git.parent().and_then(Path::parent).and_then(Path::parent) {
            bases.push(base.to_path_buf());
        }
    }
    bases.extend(program_files(host));
    if let Some(local) = env_path(host, "LOCALAPPDATA") {
        bases.push(local.join("Programs"));
    }
    let mut candidates = Vec::new();
    for base in bases {
        candidates.push(join(&base, &["Git", "bin", "bash.exe"]));
        candidates.push(join(&base, &["Git", "usr", "bin", "bash.exe"]));
        candidates.push(join(&base, &["usr", "bin", "bash.exe"]));
    }
    if let Some(home) = env_path(host, "USERPROFILE") {
        candidates.push(join(
            &home,
            &["scoop", "apps", "git", "current", "bin", "bash.exe"],
        ));
        candidates.push(join(
            &home,
            &[
                "scoop",
                "apps",
                "git-with-openssh",
                "current",
                "bin",
                "bash.exe",
            ],
        ));
    }
    candidates.into_iter().find(|bash| host.is_file(bash))
}

/// Distribution names from `wsl.exe -l -q`, which writes UTF-16LE. Docker's
/// internal distributions are not shells.
pub(crate) fn parse_wsl_list(output: &[u8]) -> Vec<String> {
    let units: Vec<u16> = output
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .collect();
    String::from_utf16_lossy(&units)
        .lines()
        .map(|line| line.trim_matches(|c: char| c.is_whitespace() || c == '\0' || c == '\u{feff}'))
        .filter(|name| {
            !name.is_empty()
                && name.len() <= 64
                && !name.starts_with("docker-desktop")
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        })
        .map(str::to_owned)
        .collect()
}

fn choice(
    id: impl Into<String>,
    label: impl Into<String>,
    program: PathBuf,
    args: &[&str],
) -> ShellChoice {
    ShellChoice {
        id: id.into(),
        label: label.into(),
        program,
        args: args.iter().map(OsString::from).collect(),
    }
}

pub(crate) fn detect(host: &dyn ShellHost) -> Vec<ShellChoice> {
    let mut shells = Vec::new();
    let Some(system32) = env_path(host, "SystemRoot")
        .or_else(|| env_path(host, "windir"))
        .map(|root| root.join("System32"))
    else {
        return shells;
    };
    if let Some(pwsh) = find_pwsh(host) {
        shells.push(choice("pwsh", "PowerShell", pwsh, &[]));
    }
    let windows_powershell = join(&system32, &["WindowsPowerShell", "v1.0", "powershell.exe"]);
    if host.is_file(&windows_powershell) {
        shells.push(choice(
            "windows-powershell",
            "Windows PowerShell",
            windows_powershell,
            &[],
        ));
    }
    if let Some(bash) = find_git_bash(host) {
        shells.push(choice("git-bash", "Git Bash", bash, &["--login", "-i"]));
    }
    let cmd = system32.join("cmd.exe");
    if host.is_file(&cmd) {
        shells.push(choice("cmd", "Command Prompt", cmd, &[]));
    }
    let wsl = system32.join("wsl.exe");
    if host.is_file(&wsl) {
        for distro in host
            .wsl_list(&wsl)
            .map(|output| parse_wsl_list(&output))
            .unwrap_or_default()
        {
            shells.push(choice(
                format!("wsl:{distro}"),
                format!("{distro} (WSL)"),
                wsl.clone(),
                &["-d", &distro],
            ));
        }
    }
    shells
}

/// What "automatic" offers, as VS Code's default: PowerShell when installed,
/// then Windows PowerShell, then Command Prompt.
pub(crate) fn automatic(shells: &[ShellChoice]) -> Option<&ShellChoice> {
    ["pwsh", "windows-powershell", "cmd"]
        .into_iter()
        .find_map(|id| shells.iter().find(|shell| shell.id == id))
}

/// A quoted program and its arguments, as `CommandLineToArgvW` splits them:
/// backslashes are literal except before a quote, where they are escaped.
pub(crate) fn command_line(program: &str, args: &[&str]) -> String {
    let mut line = format!("\"{program}\"");
    for arg in args {
        line.push(' ');
        if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\u{b}', '"']) {
            line.push_str(arg);
            continue;
        }
        line.push('"');
        let mut backslashes = 0;
        for c in arg.chars() {
            match c {
                '\\' => backslashes += 1,
                '"' => {
                    line.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                    line.push('"');
                    backslashes = 0;
                }
                _ => {
                    line.extend(std::iter::repeat_n('\\', backslashes));
                    line.push(c);
                    backslashes = 0;
                }
            }
        }
        line.extend(std::iter::repeat_n('\\', backslashes * 2));
        line.push('"');
    }
    line
}

/// The real computer: environment, file system and a bounded `wsl.exe` run.
#[cfg(windows)]
pub(crate) struct SystemShellHost;

#[cfg(windows)]
impl ShellHost for SystemShellHost {
    fn env(&self, name: &str) -> Option<OsString> {
        std::env::var_os(name)
    }
    fn is_file(&self, path: &Path) -> bool {
        std::fs::metadata(path).is_ok_and(|meta| meta.is_file())
    }
    fn list_dir(&self, path: &Path) -> Vec<String> {
        std::fs::read_dir(path)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .filter_map(|entry| entry.file_name().into_string().ok())
                    .collect()
            })
            .unwrap_or_default()
    }
    fn wsl_list(&self, wsl: &Path) -> Option<Vec<u8>> {
        use std::{io::Read, os::windows::process::CommandExt, process::Stdio};
        /// No console window flashes while the list is read.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let mut child = std::process::Command::new(wsl)
            .args(["-l", "-q"])
            .env("WSL_UTF8", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .ok()?;
        // As VS Code: a WSL that does not answer within a second offers nothing.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(20))
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        };
        let mut output = Vec::new();
        child.stdout.take()?.read_to_end(&mut output).ok()?;
        status.success().then_some(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    #[derive(Default)]
    struct FakeHost {
        env: HashMap<&'static str, String>,
        files: HashSet<PathBuf>,
        dirs: HashMap<PathBuf, Vec<String>>,
        wsl: Option<Vec<u8>>,
    }
    impl ShellHost for FakeHost {
        fn env(&self, name: &str) -> Option<OsString> {
            self.env.get(name).map(OsString::from)
        }
        fn is_file(&self, path: &Path) -> bool {
            self.files.contains(path)
        }
        fn list_dir(&self, path: &Path) -> Vec<String> {
            self.dirs.get(path).cloned().unwrap_or_default()
        }
        fn wsl_list(&self, _: &Path) -> Option<Vec<u8>> {
            self.wsl.clone()
        }
    }
    fn path(parts: &[&str]) -> PathBuf {
        join(Path::new(parts[0]), &parts[1..])
    }
    fn windows() -> FakeHost {
        let mut host = FakeHost::default();
        host.env.insert("SystemRoot", "C:".into());
        host.env.insert("ProgramFiles", "PF".into());
        host.env.insert("ProgramW6432", "PF".into());
        host.env.insert("ProgramFiles(x86)", "PF86".into());
        host.env.insert("LOCALAPPDATA", "LOCAL".into());
        host.env.insert("USERPROFILE", "HOME".into());
        host.files.insert(path(&["C:", "System32", "cmd.exe"]));
        host.files.insert(path(&[
            "C:",
            "System32",
            "WindowsPowerShell",
            "v1.0",
            "powershell.exe",
        ]));
        host
    }
    fn wsl_output(text: &str) -> Vec<u8> {
        text.encode_utf16().flat_map(u16::to_le_bytes).collect()
    }
    fn ids(shells: &[ShellChoice]) -> Vec<&str> {
        shells.iter().map(|shell| shell.id.as_str()).collect()
    }

    #[test]
    fn a_bare_windows_offers_windows_powershell_by_default_never_cmd_first() {
        let shells = detect(&windows());
        assert_eq!(ids(&shells), ["windows-powershell", "cmd"]);
        assert_eq!(automatic(&shells).unwrap().id, "windows-powershell");
        assert_eq!(shells[1].label, "Command Prompt");
    }

    #[test]
    fn the_newest_powershell_install_is_offered_and_automatic() {
        let mut host = windows();
        host.dirs.insert(
            path(&["PF", "PowerShell"]),
            vec!["6".into(), "7".into(), "7-preview".into(), "notes".into()],
        );
        host.files
            .insert(path(&["PF", "PowerShell", "7", "pwsh.exe"]));
        host.files
            .insert(path(&["PF", "PowerShell", "6", "pwsh.exe"]));
        let shells = detect(&host);
        assert_eq!(ids(&shells), ["pwsh", "windows-powershell", "cmd"]);
        assert_eq!(
            shells[0].program,
            path(&["PF", "PowerShell", "7", "pwsh.exe"])
        );
        assert_eq!(automatic(&shells).unwrap().id, "pwsh");
    }

    #[test]
    fn powershell_is_found_in_the_store_dotnet_tools_and_scoop_in_that_order() {
        let mut host = windows();
        let store = path(&["LOCAL", "Microsoft", "WindowsApps"]);
        host.dirs.insert(
            store.clone(),
            vec!["Microsoft.PowerShell_8wekyb3d8bbwe".into(), "Other".into()],
        );
        host.files.insert(
            store
                .join("Microsoft.PowerShell_8wekyb3d8bbwe")
                .join("pwsh.exe"),
        );
        host.files
            .insert(path(&["HOME", ".dotnet", "tools", "pwsh.exe"]));
        assert_eq!(
            detect(&host)[0].program,
            store
                .join("Microsoft.PowerShell_8wekyb3d8bbwe")
                .join("pwsh.exe")
        );
        host.files.retain(|file| !file.starts_with(&store));
        assert_eq!(
            detect(&host)[0].program,
            path(&["HOME", ".dotnet", "tools", "pwsh.exe"])
        );
        host.files
            .remove(&path(&["HOME", ".dotnet", "tools", "pwsh.exe"]));
        host.files.insert(path(&[
            "HOME", "scoop", "apps", "pwsh", "current", "pwsh.exe",
        ]));
        assert_eq!(detect(&host)[0].id, "pwsh");
    }

    #[test]
    fn git_bash_is_found_beside_git_on_path_then_in_program_files() {
        let mut host = windows();
        host.files
            .insert(path(&["D:", "Tools", "Git", "cmd", "git.exe"]));
        host.files
            .insert(path(&["D:", "Tools", "Git", "bin", "bash.exe"]));
        host.files.insert(path(&["PF", "Git", "bin", "bash.exe"]));
        host.env.insert(
            "PATH",
            format!("C:;{}", path(&["D:", "Tools", "Git", "cmd"]).display()),
        );
        let shells = detect(&host);
        let bash = shells.iter().find(|shell| shell.id == "git-bash").unwrap();
        assert_eq!(
            bash.program,
            path(&["D:", "Tools", "Git", "bin", "bash.exe"])
        );
        assert_eq!(bash.args, ["--login", "-i"]);
        host.env.remove("PATH");
        assert_eq!(
            detect(&host)
                .iter()
                .find(|shell| shell.id == "git-bash")
                .unwrap()
                .program,
            path(&["PF", "Git", "bin", "bash.exe"])
        );
    }

    #[test]
    fn wsl_distributions_are_offered_last_without_docker() {
        let mut host = windows();
        host.files.insert(path(&["C:", "System32", "wsl.exe"]));
        host.wsl = Some(wsl_output(
            "Ubuntu-24.04\r\ndocker-desktop\r\n\r\nDebian\r\nbad name\r\n",
        ));
        let shells = detect(&host);
        assert_eq!(
            ids(&shells),
            [
                "windows-powershell",
                "cmd",
                "wsl:Ubuntu-24.04",
                "wsl:Debian"
            ]
        );
        assert_eq!(shells[2].label, "Ubuntu-24.04 (WSL)");
        assert_eq!(shells[2].args, ["-d", "Ubuntu-24.04"]);
        host.wsl = None;
        assert_eq!(ids(&detect(&host)), ["windows-powershell", "cmd"]);
    }

    #[test]
    fn arguments_are_quoted_as_windows_splits_them() {
        assert_eq!(
            command_line(r"C:\Program Files\Git\bin\bash.exe", &["--login", "-i"]),
            r#""C:\Program Files\Git\bin\bash.exe" --login -i"#
        );
        assert_eq!(
            command_line("wsl.exe", &["-d", "My Distro"]),
            r#""wsl.exe" -d "My Distro""#
        );
        // An unquoted trailing backslash is literal; a quoted one is doubled before the closing quote.
        assert_eq!(
            command_line("p", &[r#"a"b"#, r"c:\dir\", r"c:\my dir\", ""]),
            r#""p" "a\"b" c:\dir\ "c:\my dir\\" """#
        );
        assert_eq!(command_line("p", &[r"x\\y"]), r#""p" x\\y"#);
    }
}
