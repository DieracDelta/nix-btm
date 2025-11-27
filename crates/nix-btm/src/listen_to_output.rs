// ideally in some point in the future, we have two options
//
// - morally better option: expose a port on nix and dump this info so we don't
//   have to strace it.
// This is better because it doesn't involve elevated permissions
//
//-
// we implement strace/dtruss for only write syscalls in a cross
// platform manner. That is, for {x86, aarc64}-{linux,darwin}
// as it stands right now, we'll just use strace and dtruss because they're
// easier This is worse because it involves elevated perms on both platforms
//

use sysinfo::Pid;

pub fn listen_to_write_syscalls(pid: &Pid) {
    #[cfg(target_os = "linux")]
    let mut output = {
        let mut tmp = std::process::Command::new("strace");
        tmp.arg("-e")
            .arg("write")
            .arg("-s")
            .arg("1000000")
            .arg("-p")
            .arg(pid.to_string());
        tmp
    };

    #[cfg(target_os = "macos")]
    let output = Command::new("dtruss")
        .arg("-t")
        .arg("write")
        .arg("-p")
        .arg(pid.to_string());

    output.output().expect("Failed to execute command {output}");
}
