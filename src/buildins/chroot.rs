//! src/buildins/chroot.rs
//! Manages the chroot environment for secure package building.


use std::collections::HashSet;
use std::ffi::CString;
use std::io;
use std::os::unix::process::ExitStatusExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use colored::*;
use nix::mount::{mount, umount2, MntFlags, MsFlags};
use nix::sched::{unshare, CloneFlags};
use nix::sys::wait::{waitpid, WaitStatus};
use nix::unistd::{chdir, chroot, fork, setgid, setuid, ForkResult, Gid, Uid};


/// Represents a chroot environment.
pub struct ChrootEnv {
    root_path: PathBuf,
}

// Helper to convert nix::sys::wait::WaitStatus to std::process::ExitStatus
fn wait_status_to_exit_status(status: WaitStatus) -> ExitStatus {
    match status {
        WaitStatus::Exited(_, code) => ExitStatus::from_raw(code << 8),
        WaitStatus::Signaled(_, signal, _) => ExitStatus::from_raw(signal as i32),
        _ => ExitStatus::from_raw(1), // Should not happen in simple cases
    }
}

impl ChrootEnv {
    /// Creates a new chroot environment at the specified path.
    pub fn new<P: AsRef<Path>>(path: P) -> Self {
        ChrootEnv {
            root_path: path.as_ref().to_path_buf(),
        }
    }


    /// Prepares the chroot directory by finding binaries in PATH and copying them with their dependencies.
    pub fn prepare(&self) -> io::Result<()> {
        println!("{}", "Setting up chroot environment... (requires sudo)".yellow());
        std::fs::create_dir_all(&self.root_path)?;

        // 1. Create essential directories
        let dirs = ["bin", "usr/bin", "lib", "lib64", "proc", "dev", "sys", "etc", "build", "tmp"];
        for dir in dirs.iter() {
            std::fs::create_dir_all(self.root_path.join(dir))?;
        }
        let tmp_path = self.root_path.join("tmp");
        if let Ok(meta) = std::fs::metadata(&tmp_path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o1777);
            let _ = std::fs::set_permissions(&tmp_path, perms);
        }






                // Copy host DNS configuration (for networking inside chroot)
        let host_resolv = Path::new("/etc/resolv.conf");
        let chroot_resolv = self.root_path.join("etc/resolv.conf");
        if host_resolv.exists() {
            if let Err(e) = std::fs::copy(host_resolv, &chroot_resolv) {
                eprintln!("{} could not copy resolv.conf: {}", "Warning:".yellow(), e);
            }
        } else {
            eprintln!("{} host /etc/resolv.conf not found; DNS may fail inside chroot", "Warning:".yellow());
        }

        // 2. Define binaries needed for building
        let binaries_to_find = [
            "bash", "sh", "env", "make", "gcc", "g++", "cargo", "meson",
            "ninja", "cmake", "git", "scons", "python", "ld"
        ];
        




        // 3. Find and copy them with dependencies
        let mut copied_files = HashSet::new();
        for bin_name in &binaries_to_find {
            println!("  Resolving dependencies for '{}'...", bin_name);
            match self.copy_binary_with_deps(bin_name, &mut copied_files) {
                Ok(_) => {},
                Err(e) => println!("    {} Could not resolve '{}': {}", "Warning:".yellow(), bin_name, e),
            }
        }

        println!("{}", "Chroot environment prepared.".green());
        Ok(())
    }

    /// Finds a binary, its library dependencies (via ldd), and copies them into the chroot.
    fn copy_binary_with_deps(&self, bin_name: &str, copied_files: &mut HashSet<PathBuf>) -> io::Result<()> {
        // Find the binary's full path
        let output = Command::new("which").arg(bin_name).output()?;
        if !output.status.success() {
            return Err(io::Error::new(io::ErrorKind::NotFound, format!("'{}' not found in PATH", bin_name)));
        }
        let bin_path_str = String::from_utf8(output.stdout).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let bin_path = PathBuf::from(bin_path_str.trim());

        // Get dependencies using ldd
        let ldd_output = Command::new("ldd").arg(&bin_path).output()?;
        let mut files_to_copy = vec![bin_path];

        if ldd_output.status.success() {
            let ldd_str = String::from_utf8(ldd_output.stdout).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
            for line in ldd_str.lines() {
                let parts: Vec<&str> = line.split_whitespace().collect();
                // Handle lines like:
                // /lib64/ld-linux-x86-64.so.2 (0x...)
                // libsomthing.so.6 => /lib/x86_64-linux-gnu/libsomething.so.6 (0x...)
                let path_to_lib = if line.contains("=>") && parts.len() >= 3 {
                    Some(parts[2])
                } else if !line.contains("=>") && parts.len() >= 2 && parts[0].starts_with('/') {
                    Some(parts[0])
                } else {
                    None
                };
                
                if let Some(p) = path_to_lib {
                    files_to_copy.push(PathBuf::from(p));
                }
            }
        }

        // Copy all found files (binary + libs) into the chroot
        for file_path in files_to_copy {
            if !copied_files.contains(&file_path) {
                let dest_path = self.root_path.join(file_path.strip_prefix("/").unwrap());
                if let Some(parent) = dest_path.parent() {
                    std::fs::create_dir_all(parent)?;
                }




                
                if file_path.exists() {
                    std::fs::copy(&file_path, &dest_path)?;
                    //println!("    Copied {}", file_path.display());
                    copied_files.insert(file_path);
                }
            }
        }



        Ok(())
    }

    /// Runs a command inside the prepared chroot environment using fork, unshare, and chroot.
    /// **Warning:** This function must be run with root privileges.
    pub fn run_command(&self, command: &str, args: &[&str], cwd: Option<&Path>) -> io::Result<ExitStatus> {
        if nix::unistd::geteuid().as_raw() != 0 {
            return Err(io::Error::new(io::ErrorKind::PermissionDenied, "run_command requires root privileges (chroot)"));
        }
        let c_args: Vec<CString> = args.iter().map(|a| CString::new(*a).unwrap()).collect();

        match unsafe { fork() } {
            Ok(ForkResult::Parent { child, .. }) => {
                // Parent process: wait for the child to finish
                let wait_status = waitpid(child, None)
                    .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;
                Ok(wait_status_to_exit_status(wait_status))
            }
            Ok(ForkResult::Child) => {
                // --- Child Process ---
                // This code runs in the child. If anything fails, we exit with a non-zero code.
                
                // 1. Unshare namespaces
                unshare(CloneFlags::CLONE_NEWNS | CloneFlags::CLONE_NEWPID | CloneFlags::CLONE_NEWUTS)
                    .unwrap_or_else(|e| {
                        eprintln!("Fatal: unshare failed: {}", e);
                        std::process::exit(101);
                    });

                // 2. Make mounts private to avoid leaking mounts to the host
                mount(
                    None::<&str>,
                    Path::new("/"),
                    None::<&str>,
                    MsFlags::MS_REC | MsFlags::MS_PRIVATE,
                    None::<&str>,
                ).unwrap_or_else(|e| {
                    eprintln!("Fatal: mount propagation change failed: {}", e);
                    std::process::exit(106);
                });

                // 3. Bind-mount /dev and /sys into the chroot root (in this new mount namespace)
                let dev_src = Path::new("/dev");
                let dev_dst = self.root_path.join("dev");
                if let Err(e) = mount(
                    Some(dev_src),
                    &dev_dst,
                    None::<&str>,
                    MsFlags::MS_BIND | MsFlags::MS_REC,
                    None::<&str>,
                ) {
                    eprintln!("{} bind-mount /dev failed: {}", "Warning:".yellow(), e);
                } else {
                    let _ = mount(
                        Some(dev_src),
                        &dev_dst,
                        None::<&str>,
                        MsFlags::MS_REMOUNT | MsFlags::MS_BIND | MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
                        None::<&str>,
                    );
                }

                let sys_src = Path::new("/sys");
                let sys_dst = self.root_path.join("sys");
                if sys_dst.exists() {
                    if let Err(e) = mount(
                        Some(sys_src),
                        &sys_dst,
                        None::<&str>,
                        MsFlags::MS_BIND | MsFlags::MS_REC,
                        None::<&str>,
                    ) {
                        eprintln!("{} bind-mount /sys failed: {}", "Warning:".yellow(), e);
                    } else {
                        // Remount read-only
                        let _ = mount(
                            Some(sys_src),
                            &sys_dst,
                            None::<&str>,
                            MsFlags::MS_REMOUNT | MsFlags::MS_BIND | MsFlags::MS_RDONLY | MsFlags::MS_NOSUID | MsFlags::MS_NOEXEC,
                            None::<&str>,
                        );
                    }
                }

                // 4. Mount /proc for the new PID namespace
                let proc_path = self.root_path.join("proc");
                mount(
                    Some("proc"),
                    &proc_path,
                    Some("proc"),
                    MsFlags::MS_NOEXEC | MsFlags::MS_NOSUID | MsFlags::MS_NODEV,
                    None::<&str>,
                ).unwrap_or_else(|e| {
                    eprintln!("Fatal: mount /proc failed: {}", e);
                    std::process::exit(102);
                });
                
                // 5. Chroot into the new root directory
                chroot(&self.root_path)
                    .unwrap_or_else(|e| {
                        eprintln!("Fatal: chroot failed: {}", e);
                        std::process::exit(103);
                    });
                
                // 6. Change directory to the new root
                chdir("/").unwrap_or_else(|e| {
                    eprintln!("Fatal: chdir to / failed: {}", e);
                    std::process::exit(104);
                });

                // 7. Drop privileges
                // Using 'nobody' user (often UID/GID 65534) or a fallback
                let nobody_uid = Uid::from_raw(65534);
                let nobody_gid = Gid::from_raw(65534);
                if let Err(e) = setgid(nobody_gid) {
                    eprintln!("Fatal: setgid failed: {}", e);
                    std::process::exit(108);
                }
                if let Err(e) = setuid(nobody_uid) {
                    eprintln!("Fatal: setuid failed: {}", e);
                    std::process::exit(109);
                }

                // 8. Optionally change to the requested working directory
                if let Some(dir) = cwd {
                    if !dir.is_absolute() {
                        eprintln!("Fatal: cwd must be absolute inside chroot");
                        std::process::exit(110);
                    }
                    chdir(dir).unwrap_or_else(|e| {
                        eprintln!("Fatal: chdir to {} failed: {}", dir.display(), e);
                        std::process::exit(111);
                    });
                }

                // 9. Resolve command path inside chroot and execute without relying on PATH
                let resolved = if command.starts_with('/') || command.contains('/') {
                    command.to_string()
                } else {
                    let cand_usr = format!("/usr/bin/{}", command);
                    if std::path::Path::new(&cand_usr).exists() {
                        cand_usr
                    } else {
                        let cand_bin = format!("/bin/{}", command);
                        if std::path::Path::new(&cand_bin).exists() {
                            cand_bin
                        } else {
                            command.to_string()
                        }
                    }
                };

                let c_command = CString::new(resolved.clone()).unwrap();
                let mut argv: Vec<&std::ffi::CStr> = Vec::with_capacity(1 + c_args.len());
                argv.push(c_command.as_c_str());
                for a in &c_args {
                    argv.push(a.as_c_str());
                }
                let exec_result = nix::unistd::execv(c_command.as_c_str(), &argv);
                
                // execv only returns if there's an error
                let errno = exec_result.err().unwrap();
                eprintln!("Fatal: execv of '{}' failed: {}", resolved, errno);
                std::process::exit(105);
            }
            Err(e) => {
                // Fork failed
                Err(io::Error::new(io::ErrorKind::Other, format!("fork failed: {}", e)))
            }
        }
    }

    /// Cleans up the chroot environment. (Requires sudo)
    pub fn cleanup(&self) -> io::Result<()> {
        println!("{}", "Cleaning up chroot environment... (requires sudo)".yellow());
        // Attempt to unmount common mounts inside chroot path (best-effort)
        for name in ["proc", "dev", "sys"] {
            let p = self.root_path.join(name);
            if p.exists() {
                if let Err(e) = umount2(&p, MntFlags::MNT_DETACH) {
                    eprintln!("{} could not unmount /{}: {}", "Warning:".yellow(), name, e);
                }
            }
        }

        std::fs::remove_dir_all(&self.root_path)?;
        Ok(())
    }
}
