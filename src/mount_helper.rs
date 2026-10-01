use std::{
    ffi::{CStr, CString},
    io::{BufRead, BufReader, Write},
    path::PathBuf,
    process::{Command, Stdio},
};

use clap::Parser;
use fuser::{Config, MountOption, Session, SessionACL};

use crate::{Result, api::MyboxApiClient, auth::TokenStore, fuse::MyboxFs};

#[derive(Debug, Parser)]
#[command(name = "mount.myboxfs", about = "Mount MYBOX for a local user")]
struct HelperArgs {
    source: String,
    mountpoint: PathBuf,
    #[arg(short = 'o', action = clap::ArgAction::Append)]
    options: Vec<String>,
    #[arg(short = 'r')]
    read_only: bool,
    #[arg(short = 'w', conflicts_with = "read_only")]
    read_write: bool,
    #[arg(short = 'n')]
    no_mtab: bool,
    #[arg(short = 'v')]
    verbose: bool,
    #[arg(short = 'f')]
    fake: bool,
    #[arg(short = 's')]
    sloppy: bool,
    #[arg(hide = true, long)]
    daemon_child: bool,
}

#[derive(Debug)]
struct MountSettings {
    config: Config,
    token_file: Option<PathBuf>,
    foreground: bool,
}

impl MountSettings {
    fn parse(options: &[String]) -> Result<Self> {
        let mut config = Config::default();
        config.mount_options = vec![
            MountOption::DefaultPermissions,
            MountOption::FSName("myboxfs".into()),
            MountOption::Subtype("myboxfs".into()),
            MountOption::NoSuid,
            MountOption::NoDev,
        ];
        let mut settings = Self {
            config,
            token_file: None,
            foreground: false,
        };
        for option in options.iter().flat_map(|options| options.split(',')) {
            let mount_option = match option {
                "ro" => Some(MountOption::RO),
                "rw" => Some(MountOption::RW),
                "exec" => Some(MountOption::Exec),
                "noexec" => Some(MountOption::NoExec),
                "atime" => Some(MountOption::Atime),
                "noatime" => Some(MountOption::NoAtime),
                "sync" => Some(MountOption::Sync),
                "async" => Some(MountOption::Async),
                "dirsync" => Some(MountOption::DirSync),
                "allow_other" => {
                    settings.config.acl = SessionACL::All;
                    None
                }
                "allow_root" => {
                    settings.config.acl = SessionACL::RootAndOwner;
                    None
                }
                "foreground" => {
                    settings.foreground = true;
                    None
                }
                ""
                | "defaults"
                | "default_permissions"
                | "nosuid"
                | "nodev"
                | "auto"
                | "noauto"
                | "user"
                | "users"
                | "nouser"
                | "_netdev"
                | "nofail" => None,
                value
                    if value.starts_with("x-")
                        || value.starts_with("comment=")
                        || value.starts_with("user=") =>
                {
                    None
                }
                value if value.starts_with("token_file=") => {
                    let path = PathBuf::from(&value[11..]);
                    if !path.is_absolute() {
                        return Err("token_file must be an absolute path".into());
                    }
                    settings.token_file = Some(path);
                    None
                }
                _ => {
                    return Err(
                        "unsupported MYBOX mount option; see README for supported options".into(),
                    );
                }
            };
            if let Some(mount_option) = mount_option {
                settings.config.mount_options.retain(|existing| {
                    !matches!(
                        (existing, &mount_option),
                        (
                            MountOption::RO | MountOption::RW,
                            MountOption::RO | MountOption::RW
                        ) | (
                            MountOption::Exec | MountOption::NoExec,
                            MountOption::Exec | MountOption::NoExec
                        ) | (
                            MountOption::Atime | MountOption::NoAtime,
                            MountOption::Atime | MountOption::NoAtime
                        ) | (
                            MountOption::Sync | MountOption::Async,
                            MountOption::Sync | MountOption::Async
                        )
                    )
                });
                settings.config.mount_options.push(mount_option);
            }
        }
        Ok(settings)
    }
}

struct LocalUser {
    name: CString,
    uid: libc::uid_t,
    gid: libc::gid_t,
    home: PathBuf,
}

impl LocalUser {
    fn lookup(name: &str) -> Result<Self> {
        let name = CString::new(name)?;
        let entry = unsafe { libc::getpwnam(name.as_ptr()) };
        if entry.is_null() {
            return Err("fstab source must name an existing local user".into());
        }
        let entry = unsafe { &*entry };
        use std::os::unix::ffi::OsStrExt;
        let home = PathBuf::from(std::ffi::OsStr::from_bytes(unsafe {
            CStr::from_ptr(entry.pw_dir).to_bytes()
        }));
        if !home.is_absolute() {
            return Err("local user's home directory must be absolute".into());
        }
        Ok(Self {
            name,
            uid: entry.pw_uid,
            gid: entry.pw_gid,
            home,
        })
    }

    fn assume_identity(&self) -> Result<()> {
        if unsafe { libc::geteuid() } == 0 {
            if unsafe { libc::initgroups(self.name.as_ptr(), self.gid) } != 0
                || unsafe { libc::setgid(self.gid) } != 0
                || unsafe { libc::setuid(self.uid) } != 0
            {
                return Err(std::io::Error::last_os_error().into());
            }
        } else if unsafe { libc::geteuid() } != self.uid || unsafe { libc::getegid() } != self.gid {
            return Err("only root can mount MYBOX as another local user".into());
        }
        Ok(())
    }

    fn token_path(&self, token_file: Option<PathBuf>) -> PathBuf {
        token_file.unwrap_or_else(|| self.home.join(".config/myboxfs/token"))
    }
}

fn start_session(
    args: &HelperArgs,
    settings: MountSettings,
) -> Result<Session<MyboxFs<MyboxApiClient>>> {
    let user = LocalUser::lookup(&args.source)?;
    user.assume_identity()?;
    let token = TokenStore::new(user.token_path(settings.token_file))
        .load()?
        .ok_or("no token for the selected user; run `myboxfs login` as that user")?;
    let filesystem = MyboxFs::new(MyboxApiClient::new(token));
    Ok(Session::new(
        filesystem,
        &args.mountpoint,
        &settings.config,
    )?)
}

pub fn run() -> Result<()> {
    let args = HelperArgs::parse();
    if !args.mountpoint.is_absolute() {
        return Err("mountpoint must be an absolute path".into());
    }
    let mut settings = MountSettings::parse(&args.options)?;
    settings
        .config
        .mount_options
        .retain(|option| !matches!(option, MountOption::FSName(_)));
    settings
        .config
        .mount_options
        .push(MountOption::FSName(args.source.clone()));
    if args.read_only || args.read_write {
        settings
            .config
            .mount_options
            .retain(|option| !matches!(option, MountOption::RO | MountOption::RW));
        settings.config.mount_options.push(if args.read_only {
            MountOption::RO
        } else {
            MountOption::RW
        });
    }
    if args.fake {
        LocalUser::lookup(&args.source)?;
        return Ok(());
    }
    if args.daemon_child {
        let result = (|| -> Result<_> {
            if unsafe { libc::setsid() } == -1 {
                return Err(std::io::Error::last_os_error().into());
            }
            let session = start_session(&args, settings)?;
            crate::logging::init(true)?;
            Ok(session)
        })();
        let session = match result {
            Ok(session) => session,
            Err(error) => {
                println!("ERROR: {error}");
                return Err(error);
            }
        };
        let null = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/null")?;
        println!("READY");
        std::io::stdout().flush()?;
        use std::os::fd::AsRawFd;
        for descriptor in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
            if unsafe { libc::dup2(null.as_raw_fd(), descriptor) } == -1 {
                return Err(std::io::Error::last_os_error().into());
            }
        }
        session.run()?;
        return Ok(());
    }
    if settings.foreground {
        crate::logging::init(false)?;
        start_session(&args, settings)?.run()?;
        return Ok(());
    }
    let mut child = Command::new(std::env::current_exe()?)
        .args(std::env::args_os().skip(1))
        .arg("--daemon-child")
        .current_dir("/")
        .env_remove("TMPDIR")
        .env_remove("TMP")
        .env_remove("TEMP")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    let mut status = String::new();
    BufReader::new(child.stdout.take().ok_or("missing mount startup pipe")?)
        .read_line(&mut status)?;
    if status.trim() != "READY" {
        let exit = child.wait()?;
        return Err(if status.is_empty() {
            format!("MYBOX mount helper exited before mounting: {exit}")
        } else {
            status.trim().to_owned()
        }
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_util_linux_arguments() {
        let args = HelperArgs::try_parse_from([
            "mount.myboxfs",
            "alice",
            "/mnt/mybox",
            "-o",
            "rw,nosuid,nodev,_netdev,user=alice",
            "-n",
            "-v",
            "-s",
        ])
        .unwrap();
        assert_eq!(args.source, "alice");
        assert_eq!(args.mountpoint, PathBuf::from("/mnt/mybox"));
        MountSettings::parse(&args.options).unwrap();
    }

    #[test]
    fn translates_options_and_last_flag_wins() {
        let settings = MountSettings::parse(&[
            "defaults,ro,noexec,allow_root,token_file=/home/alice/token,_netdev,nofail,x-systemd.automount".into(),
            "rw,exec,foreground".into(),
        ]).unwrap();
        assert_eq!(settings.config.acl, SessionACL::RootAndOwner);
        assert!(settings.config.mount_options.contains(&MountOption::RW));
        assert!(!settings.config.mount_options.contains(&MountOption::RO));
        assert!(settings.config.mount_options.contains(&MountOption::Exec));
        assert!(!settings.config.mount_options.contains(&MountOption::NoExec));
        assert!(
            settings
                .config
                .mount_options
                .contains(&MountOption::DefaultPermissions)
        );
        assert_eq!(
            settings.token_file,
            Some(PathBuf::from("/home/alice/token"))
        );
        assert!(settings.foreground);
    }

    #[test]
    fn rejects_unsafe_or_unknown_options_without_echoing_secrets() {
        for option in [
            "token=secret",
            "token_file=relative",
            "uid=1000",
            "suid",
            "dev",
            "unknown",
        ] {
            let error = MountSettings::parse(&[option.into()])
                .unwrap_err()
                .to_string();
            assert!(!error.contains("secret"));
        }
    }

    #[test]
    fn selects_users_home_instead_of_root_environment() {
        let user = LocalUser {
            name: CString::new("alice").unwrap(),
            uid: 1000,
            gid: 1000,
            home: "/home/alice".into(),
        };
        assert_eq!(
            user.token_path(None),
            PathBuf::from("/home/alice/.config/myboxfs/token")
        );
        assert_eq!(
            user.token_path(Some("/custom/token".into())),
            PathBuf::from("/custom/token")
        );
    }

    #[test]
    fn fake_mount_is_distinct_from_foreground() {
        let args =
            HelperArgs::try_parse_from(["mount.myboxfs", "alice", "/mnt/mybox", "-f"]).unwrap();
        assert!(args.fake);
        assert!(!MountSettings::parse(&args.options).unwrap().foreground);
    }
}
