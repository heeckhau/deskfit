//! Re-launch the current executable as its own "responsible process".
//!
//! macOS charges privacy permissions (such as Bluetooth) to the app that
//! launched a helper. Stream Deck.app has no Bluetooth usage description, so
//! the helper disclaims that responsibility and is judged by its own embedded
//! Info.plist instead. This uses the private libSystem function
//! `responsibility_spawnattrs_setdisclaim`, as LLDB and Qt Creator do.

use std::ffi::{CString, OsString};
use std::os::unix::ffi::OsStrExt;
use std::ptr;

use anyhow::{Context, Result, anyhow, bail};

type SetDisclaim = unsafe extern "C" fn(*mut libc::posix_spawnattr_t, libc::c_int) -> libc::c_int;

unsafe extern "C" {
    static environ: *const *mut libc::c_char;
}

/// Spawn the current executable with `args`, wait for it, and return its exit code.
/// The child inherits stdin, stdout and stderr; SIGTERM, SIGINT and SIGHUP
/// sent to this process are forwarded to it.
pub fn spawn_disclaimed(args: &[OsString]) -> Result<i32> {
    let exe = std::env::current_exe().context("locating current executable")?;
    let path = CString::new(exe.as_os_str().as_bytes())?;
    let mut argv_owned = vec![path.clone()];
    for arg in args {
        argv_owned.push(CString::new(arg.as_bytes())?);
    }
    let mut argv: Vec<*mut libc::c_char> = argv_owned.iter().map(|a| a.as_ptr() as *mut _).collect();
    argv.push(ptr::null_mut());

    let name = c"responsibility_spawnattrs_setdisclaim";
    let sym = unsafe { libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr()) };
    if sym.is_null() {
        bail!("responsibility_spawnattrs_setdisclaim is unavailable");
    }
    let set_disclaim: SetDisclaim = unsafe { std::mem::transmute(sym) };

    unsafe {
        // Block the forwarded signals here (threads spawned later inherit
        // this) so a thread can sigwait for them; the child gets defaults.
        let mut forwarded: libc::sigset_t = std::mem::zeroed();
        let mut empty: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut forwarded);
        libc::sigemptyset(&mut empty);
        for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(&mut forwarded, sig);
        }
        check(
            libc::pthread_sigmask(libc::SIG_BLOCK, &forwarded, ptr::null_mut()),
            "pthread_sigmask",
        )?;

        let mut attr: libc::posix_spawnattr_t = ptr::null_mut();
        check(libc::posix_spawnattr_init(&mut attr), "posix_spawnattr_init")?;
        let result = (|| {
            check(set_disclaim(&mut attr, 1), "responsibility_spawnattrs_setdisclaim")?;
            check(libc::posix_spawnattr_setsigmask(&mut attr, &empty), "posix_spawnattr_setsigmask")?;
            check(
                libc::posix_spawnattr_setsigdefault(&mut attr, &forwarded),
                "posix_spawnattr_setsigdefault",
            )?;
            check(
                libc::posix_spawnattr_setflags(
                    &mut attr,
                    (libc::POSIX_SPAWN_SETSIGMASK | libc::POSIX_SPAWN_SETSIGDEF) as libc::c_short,
                ),
                "posix_spawnattr_setflags",
            )?;
            let mut pid: libc::pid_t = 0;
            check(
                libc::posix_spawn(&mut pid, path.as_ptr(), ptr::null(), &attr, argv.as_ptr(), environ),
                "posix_spawn",
            )?;
            std::thread::spawn(move || {
                loop {
                    let mut sig = 0;
                    if libc::sigwait(&forwarded, &mut sig) == 0 {
                        libc::kill(pid, sig);
                    }
                }
            });
            wait(pid)
        })();
        libc::posix_spawnattr_destroy(&mut attr);
        result
    }
}

fn check(rc: libc::c_int, what: &str) -> Result<()> {
    if rc == 0 {
        Ok(())
    } else {
        Err(anyhow!("{what}: {}", std::io::Error::from_raw_os_error(rc)))
    }
}

fn wait(pid: libc::pid_t) -> Result<i32> {
    let mut status = 0;
    loop {
        let rc = unsafe { libc::waitpid(pid, &mut status, 0) };
        if rc == pid {
            break;
        }
        let err = std::io::Error::last_os_error();
        if err.kind() != std::io::ErrorKind::Interrupted {
            return Err(err).context("waitpid");
        }
    }
    if libc::WIFEXITED(status) {
        Ok(libc::WEXITSTATUS(status))
    } else if libc::WIFSIGNALED(status) {
        Ok(128 + libc::WTERMSIG(status))
    } else {
        Ok(1)
    }
}
