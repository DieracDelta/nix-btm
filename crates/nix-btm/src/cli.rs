static HELP_STR_SOCKET: &str = "
    The fully qualified path of the socket to read from. See the README for \
                                more details. Without this flag, the Eagle \
                                Eye view will not work because it will be \
                                unable to view the nix daemon's state. \
                                Example value: \"/tmp/nixbtm.sock\"
";

#[derive(clap::Parser)]
#[command(
    name = "nix-btm",
    version,
    about = "nix-btm",
    long_about = "A top-like client for nix that can either run in \
                  conjunction with itself as a corresponding daemon or as a \
                  standalone program with more limited functionality"
)]
pub enum Args {
    Daemon {
        #[arg(
            long,
            short = 'f',
            value_name = "DAEMONIZE",
            help = "Run in background (double-fork). Example value: false",
            default_value = "false"
        )]
        daemonize: bool,

        #[arg(
            long,
            short,
            value_name = "JSON_FILE_PATH",
            help = HELP_STR_SOCKET,
            default_value = "/tmp/nixbtm.sock"
            )]
        nix_json_file_path: Option<String>,

        #[arg(
            long,
            short,
            value_name = "SOCKET_PATH",
            help = "socket path of daemon",
            default_value = "/tmp/nix-daemon.sock"
        )]
        daemon_socket_path: String,

        #[arg(
            long,
            short = 'l',
            value_name = "LOG_PATH",
            help = "Optional log path value. If not provided, logs will \
                    placed in /tmp/nixbtm-daemon-$PID.log"
        )]
        daemon_log_path: Option<String>,
    },
    Client {
        #[arg(
            long,
            short,
            value_name = "DAEMON_SOCKET_PATH",
            help = HELP_STR_SOCKET,
            default_value = "/tmp/nix-daemon.sock"
        )]
        daemon_socket_path: Option<String>,
        #[arg(
            long,
            short = 'l',
            value_name = "LOG_PATH",
            help = "Optional log path value. If not provided, logs will \
                    placed in /tmp/nixbtm-client-$PID.log"
        )]
        client_log_path: Option<String>,
    },
    Standalone {
        #[arg(
            long,
            short,
            value_name = "JSON_FILE_PATH",
            help = HELP_STR_SOCKET,
            default_value = "/tmp/nixbtm.sock"
            )]
        nix_json_file_path: Option<String>,
        #[arg(
            long,
            short,
            value_name = "LOG_PATH",
            help = "Optional log path value. If not provided, logs will \
                    placed in /tmp/nixbtm-standalone-$PID.log",
            default_value = "None"
        )]
        standalone_log_path: Option<String>,
    },
}
