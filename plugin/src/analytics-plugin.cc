// libnix-analytics: A Nix daemon plugin that taps into the Logger to emit
// structured build events over a Unix domain socket.
//
// The plugin wraps nix::logger with an AnalyticsLogger that intercepts
// startActivity/stopActivity/result calls, serializes them as JSON, and
// sends them to the analytics daemon (nix-analyticsd).
//
// Build with:
//   g++ -shared -fPIC -std=c++23 -O2 \
//     $(pkg-config --cflags nix-main nix-store nix-util) \
//     -o libnix-analytics.so analytics-plugin.cc \
//     $(pkg-config --libs nix-main nix-store nix-util)

#include <nix/util/logging.hh>
#include <nix/util/config-global.hh>

#include <sys/socket.h>
#include <sys/un.h>
#include <sys/uio.h>
#include <arpa/inet.h>
#include <unistd.h>
#include <link.h>
#include <cstring>
#include <chrono>
#include <fstream>
#include <mutex>
#include <string>
#include <unordered_map>
#include <unordered_set>

using namespace nix;

// -- Configuration --

struct AnalyticsSettings : Config
{
    Setting<std::string> analyticsSocket{
        this,
        "/run/nix-analytics/events.sock",
        "analytics-socket",
        R"(
          Path to the Unix domain socket where build analytics events are sent.
          The nix-analyticsd daemon should be listening on this socket.
        )"};
};

// Heap-allocated to avoid static destruction order fiasco in forked children.
// nix-daemon forks per-connection children that call exit(), triggering all
// static destructors.  If our GlobalConfig-registered objects destruct before
// nix's own (e.g. FileTransferSettings), they corrupt shared state causing
// double-free / segfault.  "Leaking" is safe: the OS reclaims everything.
static AnalyticsSettings & analyticsSettings = *new AnalyticsSettings();
static auto * rAnalyticsSettings = new GlobalConfig::Register(&analyticsSettings);

// -- Socket connection --

class SocketWriter
{
    int fd = -1;
    std::mutex mutex;

public:
    SocketWriter() = default;

    ~SocketWriter()
    {
        if (fd >= 0)
            close(fd);
    }

    bool ensureConnected()
    {
        if (fd >= 0)
            return true;

        fd = socket(AF_UNIX, SOCK_STREAM | SOCK_NONBLOCK | SOCK_CLOEXEC, 0);
        if (fd < 0)
            return false;

        struct sockaddr_un addr;
        memset(&addr, 0, sizeof(addr));
        addr.sun_family = AF_UNIX;

        auto path = analyticsSettings.analyticsSocket.get();
        if (path.size() >= sizeof(addr.sun_path)) {
            close(fd);
            fd = -1;
            return false;
        }
        strncpy(addr.sun_path, path.c_str(), sizeof(addr.sun_path) - 1);

        if (connect(fd, (struct sockaddr *)&addr, sizeof(addr)) < 0) {
            close(fd);
            fd = -1;
            return false;
        }

        return true;
    }

    void send(const std::string & json)
    {
        std::lock_guard<std::mutex> lock(mutex);

        if (!ensureConnected())
            return;

        // Length-prefixed message: [4 bytes BE length][JSON payload]
        uint32_t len = htonl(json.size());

        // Use writev for atomic-ish write of header + payload.
        struct iovec iov[2];
        iov[0].iov_base = &len;
        iov[0].iov_len = sizeof(len);
        iov[1].iov_base = const_cast<char *>(json.data());
        iov[1].iov_len = json.size();

        ssize_t written = writev(fd, iov, 2);
        if (written < 0) {
            // Connection lost, reset.
            close(fd);
            fd = -1;
        }
    }
};

// Heap-allocated — same rationale as AnalyticsSettings above.
static SocketWriter & socketWriter = *new SocketWriter();

// -- Command line --

/// Read /proc/self/cmdline and return it as a space-separated string.
static std::string readCommandLine()
{
    std::ifstream f("/proc/self/cmdline", std::ios::binary);
    if (!f)
        return {};

    std::string raw((std::istreambuf_iterator<char>(f)),
                     std::istreambuf_iterator<char>());

    // /proc/self/cmdline is NUL-separated. Replace NULs with spaces,
    // then trim the trailing space.
    for (auto & c : raw)
        if (c == '\0')
            c = ' ';

    while (!raw.empty() && raw.back() == ' ')
        raw.pop_back();

    return raw;
}

/// The command line of this nix process, read once at plugin init.
static std::string nixCommandLine;

// -- Helpers --

static uint64_t nowMicros()
{
    auto now = std::chrono::system_clock::now();
    auto us = std::chrono::duration_cast<std::chrono::microseconds>(
        now.time_since_epoch());
    return us.count();
}

// Simple JSON escaping for strings.
static std::string jsonEscape(const std::string & s)
{
    std::string out;
    out.reserve(s.size() + 8);
    for (char c : s) {
        switch (c) {
        case '"': out += "\\\""; break;
        case '\\': out += "\\\\"; break;
        case '\n': out += "\\n"; break;
        case '\r': out += "\\r"; break;
        case '\t': out += "\\t"; break;
        default:
            if (static_cast<unsigned char>(c) < 0x20) {
                char buf[8];
                snprintf(buf, sizeof(buf), "\\u%04x", (unsigned)c);
                out += buf;
            } else {
                out += c;
            }
        }
    }
    return out;
}

// -- Analytics Logger --

class AnalyticsLogger : public Logger
{
    std::unique_ptr<Logger> inner;

    // State for detecting cached derivations: a Realise activity that stops
    // without ever spawning a child Build/Substitute means the drv was already
    // in the store.
    std::mutex stateMutex;
    std::unordered_map<ActivityId, std::string> realiseActivities; // id → drv_path
    std::unordered_set<ActivityId> realiseWithChild;               // had a Build/Substitute child

public:
    explicit AnalyticsLogger(std::unique_ptr<Logger> inner) : inner(std::move(inner)) {}

    void log(Verbosity lvl, std::string_view s) override
    {
        inner->log(lvl, s);
    }

    void logEI(const ErrorInfo & ei) override
    {
        inner->logEI(ei);
    }

    void warn(const std::string & msg) override
    {
        inner->warn(msg);
    }

    bool isVerbose() override { return inner->isVerbose(); }
    void stop() override { inner->stop(); }
    void pause() override { inner->pause(); }
    void resume() override { inner->resume(); }

    void writeToStdout(std::string_view s) override
    {
        inner->writeToStdout(s);
    }

    std::optional<char> ask(std::string_view s) override
    {
        return inner->ask(s);
    }

    void setPrintBuildLogs(bool b) override
    {
        inner->setPrintBuildLogs(b);
    }

    void startActivity(
        ActivityId act,
        Verbosity lvl,
        ActivityType type,
        const std::string & s,
        const Fields & fields,
        ActivityId parent) override
    {
        inner->startActivity(act, lvl, type, s, fields, parent);

        // Extract drv_path from fields if this is a build activity.
        std::string drvPath;
        if (!fields.empty() && fields[0].type == Field::tString)
            drvPath = fields[0].s;

        std::string cmdField;
        if (parent == 0 && !nixCommandLine.empty())
            cmdField = ",\"command_line\":\"" + jsonEscape(nixCommandLine) + "\"";

        std::string pidField = ",\"nix_pid\":" + std::to_string(getpid());

        std::string json = "{\"type\":\"ActivityStarted\""
            ",\"timestamp_us\":" + std::to_string(nowMicros()) +
            ",\"activity_id\":" + std::to_string(act) +
            ",\"activity_type\":" + std::to_string((int)type) +
            ",\"description\":\"" + jsonEscape(s) + "\""
            ",\"drv_path\":" + (drvPath.empty() ? "null" : "\"" + jsonEscape(drvPath) + "\"") +
            ",\"parent_id\":" + std::to_string(parent) +
            pidField +
            cmdField +
            "}";

        socketWriter.send(json);

        // Track Realise activities for cached derivation detection.
        if (type == actRealise && !drvPath.empty()) {
            std::lock_guard<std::mutex> lock(stateMutex);
            realiseActivities[act] = drvPath;
        }

        // If a Build/Substitute starts under a tracked Realise, mark
        // that Realise as having a child (i.e. not cached).
        if ((type == actBuild || type == actSubstitute) && parent != 0) {
            std::lock_guard<std::mutex> lock(stateMutex);
            if (realiseActivities.count(parent))
                realiseWithChild.insert(parent);
        }
    }

    void stopActivity(ActivityId act) override
    {
        inner->stopActivity(act);

        // Detect cached derivations: a Realise that stops without ever
        // spawning a child Build/Substitute means the drv was already
        // in the store. Emit DrvCached BEFORE ActivityStopped so the
        // daemon can still walk the parent chain.
        {
            std::lock_guard<std::mutex> lock(stateMutex);
            auto it = realiseActivities.find(act);
            if (it != realiseActivities.end()) {
                if (realiseWithChild.count(act) == 0) {
                    std::string cached = "{\"type\":\"DrvCached\""
                        ",\"timestamp_us\":" + std::to_string(nowMicros()) +
                        ",\"activity_id\":" + std::to_string(act) +
                        ",\"drv_path\":\"" + jsonEscape(it->second) + "\""
                        "}";
                    socketWriter.send(cached);
                }
                realiseActivities.erase(it);
                realiseWithChild.erase(act);
            }
        }

        std::string json = "{\"type\":\"ActivityStopped\""
            ",\"timestamp_us\":" + std::to_string(nowMicros()) +
            ",\"activity_id\":" + std::to_string(act) +
            "}";

        socketWriter.send(json);
    }

    void result(ActivityId act, ResultType type, const Fields & fields) override
    {
        inner->result(act, type, fields);

        switch (type) {
        case resSetPhase:
            if (!fields.empty() && fields[0].type == Field::tString) {
                std::string json = "{\"type\":\"PhaseChanged\""
                    ",\"timestamp_us\":" + std::to_string(nowMicros()) +
                    ",\"activity_id\":" + std::to_string(act) +
                    ",\"phase\":\"" + jsonEscape(fields[0].s) + "\""
                    "}";
                socketWriter.send(json);
            }
            break;

        case resBuildLogLine:
            if (!fields.empty() && fields[0].type == Field::tString) {
                std::string json = "{\"type\":\"LogLine\""
                    ",\"timestamp_us\":" + std::to_string(nowMicros()) +
                    ",\"activity_id\":" + std::to_string(act) +
                    ",\"text\":\"" + jsonEscape(fields[0].s) + "\""
                    "}";
                socketWriter.send(json);
            }
            break;

        case resProgress:
            if (fields.size() >= 4) {
                std::string json = "{\"type\":\"Progress\""
                    ",\"timestamp_us\":" + std::to_string(nowMicros()) +
                    ",\"activity_id\":" + std::to_string(act) +
                    ",\"done\":" + std::to_string(fields[0].i) +
                    ",\"expected\":" + std::to_string(fields[1].i) +
                    ",\"running\":" + std::to_string(fields[2].i) +
                    ",\"failed\":" + std::to_string(fields[3].i) +
                    "}";
                socketWriter.send(json);
            }
            break;

        case resPostBuildLogLine:
            if (!fields.empty() && fields[0].type == Field::tString) {
                std::string json = "{\"type\":\"PostBuildLogLine\""
                    ",\"timestamp_us\":" + std::to_string(nowMicros()) +
                    ",\"activity_id\":" + std::to_string(act) +
                    ",\"text\":\"" + jsonEscape(fields[0].s) + "\""
                    "}";
                socketWriter.send(json);
            }
            break;

        default:
            break;
        }
    }
};

// -- Version mismatch detection --

struct NixLibScanResult
{
    int count = 0;
    std::string paths;
};

static int countNixutilCallback(struct dl_phdr_info * info, size_t, void * data)
{
    auto * result = static_cast<NixLibScanResult *>(data);
    std::string name = info->dlpi_name ? info->dlpi_name : "";
    if (name.find("libnixutil") != std::string::npos) {
        result->count++;
        if (!result->paths.empty())
            result->paths += ", ";
        result->paths += name;
    }
    return 0;
}

/// Returns true if multiple copies of libnixutil are loaded (SONAME mismatch).
static bool detectDuplicateNixLibs(std::string & details)
{
    NixLibScanResult scan;
    dl_iterate_phdr(countNixutilCallback, &scan);
    if (scan.count > 1) {
        details = scan.paths;
        return true;
    }
    return false;
}

// -- Plugin entry point --

extern "C" void nix_plugin_entry()
{
    // Check for nix version mismatch (SONAME conflict).
    // If the plugin was compiled against a different nix version than the
    // running daemon, the dynamic linker loads two copies of libnixutil with
    // different SONAMEs.  This causes duplicate global state (GlobalConfig,
    // Settings, etc.) leading to double-free crashes when forked children exit.
    {
        std::string details;
        if (detectDuplicateNixLibs(details)) {
            fprintf(stderr,
                "\n"
                "WARNING: nix-analytics plugin DISABLED — nix version mismatch detected!\n"
                "  Plugin compiled against: nix %s\n"
                "  Loaded libnixutil copies: %s\n"
                "  This causes duplicate global state and will crash nix-daemon.\n"
                "  Rebuild the plugin against the same nix version as your daemon.\n"
                "\n",
                NIX_ANALYTICS_COMPILED_NIX_VERSION,
                details.c_str());
            return;  // Bail out — do NOT wrap the logger
        }
    }

    // Capture the nix command line for display in the TUI.
    nixCommandLine = readCommandLine();

    // Wrap the existing global logger.
    // nix::logger is a unique_ptr<Logger> — we move the original into our
    // wrapper so there is a single ownership chain:
    //   nix::logger owns AnalyticsLogger owns originalLogger
    if (nix::logger) {
        auto wrapper = std::make_unique<AnalyticsLogger>(std::move(nix::logger));
        nix::logger = std::move(wrapper);
    } else {
        fprintf(stderr, "nix-analytics: WARNING — nix::logger is null, cannot wrap!\n");
    }
}
