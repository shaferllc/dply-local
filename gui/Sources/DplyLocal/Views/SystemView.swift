import SwiftUI

/// The System surface: every dpl subsystem on the left with its live status, its
/// real log streaming on the right. Traffic (the access log) shows every request
/// as it happens; the daemon log carries DNS, mail, and dumps events.
///
/// Status comes from the same probes as the Doctor page (`dpl doctor`); the logs
/// are the daemon's own files under `~/.dpl/logs`, tailed live.
struct SystemView: View {
    @EnvironmentObject var store: Store
    @State private var selected: String = "Traffic"

    /// A dpl subsystem: its display name, the doctor check that reports its
    /// status, and — when it has one — the log to tail.
    private struct Subsystem: Identifiable {
        let id: String            // display name
        let statusTitle: String   // doctor check `title` to read status from
        let log: LogSource?
        var name: String { id }
    }

    private enum LogSource {
        case file(String)                 // ~/.dpl/logs/<file>
        case filtered(String, String)     // file, keep only lines containing …
        case fpm                          // the most-recently-active php-fpm pool log
    }

    private let subsystems: [Subsystem] = [
        .init(id: "Traffic",   statusTitle: "HTTP proxy",         log: .file("access.log")),
        .init(id: "Daemon",    statusTitle: "Daemon",             log: .file("dpld.out.log")),
        .init(id: "PHP-FPM",   statusTitle: "Installed versions", log: .fpm),
        .init(id: "DNS",       statusTitle: "DNS responder",      log: .filtered("dpld.out.log", "dns")),
        .init(id: "Mail",      statusTitle: "Mail sink",          log: .filtered("dpld.out.log", "mail")),
        .init(id: "Dumps",     statusTitle: "Debug bridge",       log: .filtered("dpld.out.log", "dumps")),
        .init(id: "TLS",       statusTitle: "Local CA",           log: nil),
    ]

    private var current: Subsystem { subsystems.first { $0.id == selected } ?? subsystems[0] }

    var body: some View {
        HStack(spacing: 0) {
            list.frame(width: 240)
            Divider()
            detail.frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .task { if store.doctorHealth == nil { await store.refreshDoctor() } }
        .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
            Task { await store.refreshDoctor() }
        }
    }

    // MARK: Subsystem list

    private var list: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("SYSTEM").font(.caption.weight(.semibold)).foregroundStyle(.secondary)
                Spacer()
                if let s = store.doctorHealth?.summary {
                    Circle().fill(s.fail > 0 ? .red : (s.warn > 0 ? .orange : Theme.live))
                        .frame(width: 8, height: 8)
                }
            }
            .padding(14)
            Divider()
            ScrollView {
                LazyVStack(spacing: 2) {
                    ForEach(subsystems) { sub in
                        row(sub)
                    }
                }
                .padding(8)
            }
        }
    }

    private func row(_ sub: Subsystem) -> some View {
        let check = status(of: sub)
        return Button {
            selected = sub.id
        } label: {
            HStack(spacing: 10) {
                Circle().fill(check?.status.color ?? .secondary).frame(width: 8, height: 8)
                Text(sub.name).font(.callout)
                Spacer()
            }
            .padding(.vertical, 7).padding(.horizontal, 10)
            .background(RoundedRectangle(cornerRadius: 7)
                .fill(selected == sub.id ? Theme.violet.opacity(0.14) : .clear))
        }
        .buttonStyle(.plain)
    }

    // MARK: Detail

    @ViewBuilder
    private var detail: some View {
        let sub = current
        let check = status(of: sub)
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 10) {
                Text(sub.name).font(.title3.weight(.semibold))
                if let check {
                    Label(check.detail, systemImage: check.status.systemImage)
                        .font(.caption)
                        .foregroundStyle(check.status.color)
                        .labelStyle(.titleAndIcon)
                }
                Spacer()
            }
            .padding(14)
            Divider()
            switch sub.log {
            case .file(let f):
                LogTailView(path: logPath(f), filter: nil)
            case .filtered(let f, let contains):
                LogTailView(path: logPath(f), filter: contains)
            case .fpm:
                if let path = newestFpmLog() {
                    LogTailView(path: path, filter: nil)
                } else {
                    configPane(sub, check: check)
                }
            case nil:
                configPane(sub, check: check)
            }
        }
    }

    /// For subsystems with no live log (TLS, PHP), show their status detail plus
    /// the related checks, so the pane is never empty.
    private func configPane(_ sub: Subsystem, check: DoctorCheck?) -> some View {
        let category = check?.category ?? ""
        let related = store.doctorHealth?.checks(in: category) ?? []
        return ScrollView {
            VStack(alignment: .leading, spacing: 10) {
                ForEach(related) { c in
                    HStack(spacing: 10) {
                        Image(systemName: c.status.systemImage).foregroundStyle(c.status.color)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(c.title).font(.callout)
                            Text(c.detail).font(.caption).foregroundStyle(.secondary)
                        }
                        Spacer()
                    }
                }
            }
            .padding(16)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    // MARK: Helpers

    private func status(of sub: Subsystem) -> DoctorCheck? {
        store.doctorHealth?.checks.first { $0.title == sub.statusTitle }
    }

    private func logPath(_ file: String) -> String {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".dpl/logs/\(file)").path
    }

    /// php-fpm logs live per version + mode at `~/.dpl/php/<key>/fpm-<mode>/fpm.log`.
    /// Show the most-recently-written one — the pool actually serving traffic.
    private func newestFpmLog() -> String? {
        let fm = FileManager.default
        let base = fm.homeDirectoryForCurrentUser.appendingPathComponent(".dpl/php")
        guard let versions = try? fm.contentsOfDirectory(at: base, includingPropertiesForKeys: nil) else { return nil }
        var newest: (path: String, at: Date)?
        for version in versions {
            guard let pools = try? fm.contentsOfDirectory(at: version, includingPropertiesForKeys: nil) else { continue }
            for pool in pools where pool.lastPathComponent.hasPrefix("fpm-") {
                let log = pool.appendingPathComponent("fpm.log").path
                guard let mod = (try? fm.attributesOfItem(atPath: log))?[.modificationDate] as? Date else { continue }
                if newest == nil || mod > newest!.at { newest = (log, mod) }
            }
        }
        return newest?.path
    }
}

/// A live-tailing view of a log file: polls it, strips ANSI colour codes,
/// optionally keeps only lines matching a term, and follows the tail.
struct LogTailView: View {
    let path: String
    var filter: String?
    @State private var lines: [String] = []
    @State private var live = true

    var body: some View {
        VStack(spacing: 0) {
            HStack(spacing: 8) {
                Circle().fill(live ? Theme.live : .secondary).frame(width: 7, height: 7)
                Text(live ? "live" : "paused").font(.caption).foregroundStyle(.secondary)
                Spacer()
                Button(live ? "Pause" : "Resume") { live.toggle() }.buttonStyle(.borderless).font(.caption)
                Text(path).font(.caption2).foregroundStyle(.tertiary).lineLimit(1).truncationMode(.head)
            }
            .padding(.horizontal, 14).padding(.vertical, 8)
            Divider()
            ScrollViewReader { proxy in
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: 1) {
                        if lines.isEmpty {
                            Text("No entries yet. Reload a site to see traffic here.")
                                .font(.system(.caption, design: .monospaced))
                                .foregroundStyle(.tertiary)
                                .padding(.top, 12)
                        }
                        ForEach(Array(lines.enumerated()), id: \.offset) { _, line in
                            Text(line)
                                .font(.system(.caption, design: .monospaced))
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                        Color.clear.frame(height: 1).id("bottom")
                    }
                    .padding(12)
                }
                // id: path — cancels and restarts when the subsystem switches, so
                // the previous log stops polling.
                .task(id: path) { await follow(proxy) }
            }
        }
    }

    /// Consume the event-driven tail stream. Each element is a full snapshot the
    /// follower emits only when the file actually changed, so there's no busy
    /// poll and every update is real — we just assign and follow the tail. The
    /// stream reads and parses off the main thread; SwiftUI state is only touched
    /// here, on the main actor.
    private func follow(_ proxy: ScrollViewProxy) async {
        for await snapshot in LogTailStream(path: path, filter: filter).snapshots() {
            if Task.isCancelled { break }
            guard live else { continue }
            lines = snapshot
            proxy.scrollTo("bottom", anchor: .bottom)
        }
    }
}

/// An event-driven file tail. Reads the file's tail once, then uses a vnode
/// `DispatchSource` to wake only when the file is written — and reads just the
/// bytes appended since last time, never the whole tail again. That replaces a
/// 1.5s busy-poll that re-read and re-parsed 256KB on every tick with work that
/// happens only when a line is actually logged (near-zero cost on an idle log).
///
/// Rotation and truncation are handled: a shrink re-reads the tail from the new
/// end, and a rename/delete/revoke reopens the path (logs rotate under the same
/// name). All file work runs on a utility queue; snapshots are delivered through
/// an `AsyncStream` the view awaits.
final class LogTailStream: @unchecked Sendable {
    private let path: String
    private let filter: String?
    private let queue = DispatchQueue(label: "io.dply.logtail", qos: .utility)

    private var source: (any DispatchSourceFileSystemObject)?
    private var evtFd: Int32 = -1
    private var offset: UInt64 = 0
    private var lines: [String] = []
    private var cont: AsyncStream<[String]>.Continuation?

    init(path: String, filter: String?) {
        self.path = path
        self.filter = filter
    }

    /// A stream of full tail snapshots, emitted only when the content changes.
    func snapshots() -> AsyncStream<[String]> {
        AsyncStream(bufferingPolicy: .bufferingNewest(1)) { continuation in
            queue.async { [weak self] in self?.start(continuation) }
            continuation.onTermination = { [weak self] _ in
                self?.queue.async { self?.stop() }
            }
        }
    }

    // MARK: On the utility queue

    private func start(_ continuation: AsyncStream<[String]>.Continuation) {
        cont = continuation
        open()
    }

    /// Open (or reopen) the file: seed with the current tail, remember the end
    /// offset, and arm a vnode source. If the file isn't there yet — a log not
    /// created until the first request — retry shortly; that's the only polling
    /// left, and only while the file is absent.
    private func open() {
        lines = LogTail.read(path: path, filter: filter)
        cont?.yield(lines)

        let fd = Darwin.open(path, O_EVTONLY)
        guard fd >= 0 else {
            queue.asyncAfter(deadline: .now() + 2) { [weak self] in
                guard let self, self.cont != nil else { return }
                self.open()
            }
            return
        }
        evtFd = fd
        offset = (try? FileHandle(forReadingAtPath: path)?.seekToEnd()) ?? 0

        let src = DispatchSource.makeFileSystemObjectSource(
            fileDescriptor: fd,
            eventMask: [.write, .extend, .delete, .rename, .revoke],
            queue: queue
        )
        src.setEventHandler { [weak self] in self?.handle(src.data) }
        src.setCancelHandler { Darwin.close(fd) }
        source = src
        src.resume()
    }

    private func handle(_ event: DispatchSource.FileSystemEvent) {
        // Rotated away (renamed/deleted/revoked): drop this source and reopen the
        // path, which a rotator will have recreated.
        if !event.isDisjoint(with: [.delete, .rename, .revoke]) {
            teardownSource()
            open()
            return
        }
        guard let handle = FileHandle(forReadingAtPath: path) else { return }
        defer { try? handle.close() }
        let size = (try? handle.seekToEnd()) ?? 0

        if size < offset {
            // Truncated in place: re-read the tail from the new end.
            lines = LogTail.read(path: path, filter: filter)
            offset = size
            cont?.yield(lines)
            return
        }
        guard size > offset else { return }
        try? handle.seek(toOffset: offset)
        let data = (try? handle.readToEnd()) ?? Data()
        // Consume only through the last newline; a line still being written stays
        // unread until its newline lands, so we never split or duplicate it.
        guard let lastNL = data.lastIndex(of: 0x0A) else { return }
        let complete = data[...lastNL]
        offset += UInt64(complete.count)
        guard let text = String(data: Data(complete), encoding: .utf8) else { return }
        let added = LogTail.parse(text, filter: filter)
        guard !added.isEmpty else { return }
        lines = Array((lines + added).suffix(LogTail.keep))
        cont?.yield(lines)
    }

    private func teardownSource() {
        source?.cancel()
        source = nil
        evtFd = -1
    }

    private func stop() {
        teardownSource()
        cont?.finish()
        cont = nil
    }
}

/// The off-main-thread log reader. Reads only the tail of the file, filters and
/// strips ANSI on just the lines it keeps — never the whole file.
enum LogTail {
    private static let ansi = try? NSRegularExpression(pattern: "\u{1B}\\[[0-9;]*m")
    private static let tailBytes: UInt64 = 256 * 1024
    static let keep = 600

    static func read(path: String, filter: String?) -> [String] {
        guard let handle = FileHandle(forReadingAtPath: path) else { return [] }
        defer { try? handle.close() }

        // Read only the last `tailBytes` — a live tail never needs the whole log.
        let size = (try? handle.seekToEnd()) ?? 0
        let start = size > tailBytes ? size - tailBytes : 0
        try? handle.seek(toOffset: start)
        let data = (try? handle.readToEnd()) ?? Data()
        guard var text = String(data: data, encoding: .utf8) else { return [] }

        // If we seeked into the middle of a line, drop that partial first line.
        if start > 0, let nl = text.firstIndex(of: "\n") {
            text = String(text[text.index(after: nl)...])
        }
        return Array(parse(text, filter: filter).suffix(keep))
    }

    /// Split a chunk of complete log lines, keep only those matching `filter`,
    /// and strip ANSI on just the survivors. Shared by the initial tail read and
    /// the incremental append reads.
    static func parse(_ text: String, filter: String?) -> [String] {
        var rows = text.split(separator: "\n").map(String.init)
        if let f = filter, !f.isEmpty {
            rows = rows.filter { $0.range(of: f, options: .caseInsensitive) != nil }
        }
        return rows.map(strip)
    }

    private static func strip(_ s: String) -> String {
        guard let ansi else { return s }
        let range = NSRange(s.startIndex..., in: s)
        return ansi.stringByReplacingMatches(in: s, range: range, withTemplate: "")
    }
}
