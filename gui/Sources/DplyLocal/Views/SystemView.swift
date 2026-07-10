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
    }

    private let subsystems: [Subsystem] = [
        .init(id: "Traffic",   statusTitle: "HTTP proxy",         log: .file("access.log")),
        .init(id: "Daemon",    statusTitle: "Daemon",             log: .file("dpld.out.log")),
        .init(id: "DNS",       statusTitle: "DNS responder",      log: .filtered("dpld.out.log", "dns")),
        .init(id: "Mail",      statusTitle: "Mail sink",          log: .filtered("dpld.out.log", "mail")),
        .init(id: "Dumps",     statusTitle: "Debug bridge",       log: .filtered("dpld.out.log", "dumps")),
        .init(id: "TLS",       statusTitle: "Local CA",           log: nil),
        .init(id: "PHP",       statusTitle: "Installed versions", log: nil),
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
}

/// A live-tailing view of a log file: polls it, strips ANSI colour codes,
/// optionally keeps only lines matching a term, and follows the tail.
struct LogTailView: View {
    let path: String
    var filter: String?
    @State private var lines: [String] = []
    @State private var live = true

    // The pattern needs a real ESC byte, not the text "\u{1B}" — ICU rejects the
    // brace form. `try?` so a bad pattern degrades to "don't strip", never a crash.
    private static let ansi = try? NSRegularExpression(pattern: "\u{1B}\\[[0-9;]*m")

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
                .task(id: path) { await follow(proxy) }
            }
        }
    }

    private func follow(_ proxy: ScrollViewProxy) async {
        while !Task.isCancelled {
            if live {
                load()
                proxy.scrollTo("bottom", anchor: .bottom)
            }
            try? await Task.sleep(for: .seconds(1.5))
        }
    }

    private func load() {
        guard let raw = try? String(contentsOfFile: path, encoding: .utf8) else {
            lines = []
            return
        }
        var out = raw.split(separator: "\n").map { stripAnsi(String($0)) }
        if let f = filter, !f.isEmpty {
            out = out.filter { $0.range(of: f, options: .caseInsensitive) != nil }
        }
        lines = Array(out.suffix(500))
    }

    private func stripAnsi(_ s: String) -> String {
        guard let ansi = Self.ansi else { return s }
        let range = NSRange(s.startIndex..., in: s)
        return ansi.stringByReplacingMatches(in: s, range: range, withTemplate: "")
    }
}
