use std::process::exit;

use libc::{pid_t, setsid};
use rustix::{
    fs::Mode,
    process::{chdir, umask},
};

/// Daemonize (from: advanced programming in the unix environment)
pub fn daemon_double_fork() {
    do_fork();

    let sid = unsafe { setsid() };
    if sid < 0 {
        eprintln!("setsid failed");
        exit(-1);
    }

    // cannot be killed by parent
    unsafe {
        libc::signal(libc::SIGHUP, libc::SIG_IGN);
    }

    // really shake them off our tail
    do_fork();

    // no risk of unmounting
    chdir("/").unwrap();

    // clear umask
    umask(Mode::empty());

    // redirect the stds to dev null
    redirect_std_fds_to_devnull();
}

fn redirect_std_fds_to_devnull() {
    use std::{fs::OpenOptions, os::unix::io::AsRawFd};

    let devnull = OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/null")
        .expect("failed to open /dev/null");

    let fd = devnull.as_raw_fd();
    unsafe {
        libc::dup2(fd, 0);
        libc::dup2(fd, 1);
        libc::dup2(fd, 2);
    }
}

fn do_fork() {
    let pid: pid_t = unsafe { libc::fork() };

    match pid {
        p if p < 0 => {
            eprintln!("unable to fork");
            exit(-1);
        }
        0 => {}       // child
        _ => exit(0), // parent
    }
}
