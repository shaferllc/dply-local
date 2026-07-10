import SwiftUI

/// First-run setup wizard: welcome → requirements → trusted `.test` →
/// autostart → first site → done. Shown once (tracked by `didOnboard`) and
/// re-openable from the Status panel.
struct OnboardingView: View {
    @EnvironmentObject var store: Store
    @Binding var isPresented: Bool
    @AppStorage("didOnboard") private var didOnboard = false

    @State private var step = 0
    @State private var doctorLines: [String] = []
    @State private var reqs: [SetupRequirement] = []
    @State private var autostart = false
    @State private var didSetup = false
    @State private var settingUp = false
    @State private var firstSite: String?

    private let last = 5

    var body: some View {
        VStack(spacing: 0) {
            content
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .padding(28)
            Divider()
            footer
        }
        .frame(width: 580, height: 540)
        .task {
            reqs = await store.checkSetupRequirements()
            doctorLines = await store.doctor()
            autostart = await store.daemonStatus()
        }
    }

    // MARK: Steps

    @ViewBuilder private var content: some View {
        switch step {
        case 0: welcome
        case 1: requirements
        case 2: setup
        case 3: autostartStep
        case 4: firstSiteStep
        default: done
        }
    }

    private var welcome: some View {
        stepShell(icon: "bolt.horizontal.fill", title: "Welcome to Dply Local",
                  subtitle: "A fast, native local PHP dev environment.") {
            VStack(alignment: .leading, spacing: 12) {
                bullet("globe", "Serve your projects at http://<name>.test")
                bullet("bolt.fill", "php-fpm speed, multiple PHP versions")
                bullet("lock.fill", "Trusted HTTPS, mail capture, databases")
                bullet("ladybug", "Debug with dumps() — right in this app")
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private var requirements: some View {
        stepShell(icon: "checklist", title: "Set up your Mac",
                  subtitle: "What a fresh macOS needs for PHP development.") {
            VStack(alignment: .leading, spacing: 10) {
                if reqs.isEmpty {
                    ProgressView().controlSize(.small)
                }
                ForEach(reqs) { r in
                    HStack(spacing: 10) {
                        Image(systemName: r.ok ? "checkmark.circle.fill" : "circle")
                            .foregroundStyle(r.ok ? AnyShapeStyle(.green) : AnyShapeStyle(.secondary))
                        VStack(alignment: .leading, spacing: 1) {
                            Text(r.name).font(.callout.weight(.medium))
                            Text(r.detail).font(.caption2).foregroundStyle(.secondary)
                        }
                        Spacer()
                        if r.ok {
                            Text("Installed").font(.caption).foregroundStyle(.green)
                        } else {
                            Button("Install…") { store.runInTerminal(r.installCommand) }
                                .buttonStyle(.borderedProminent).tint(Theme.violet).controlSize(.small)
                        }
                    }
                }
                Text("Installs open in Terminal. Re-open this step to re-check after they finish.")
                    .font(.caption2).foregroundStyle(.secondary).padding(.top, 2)
            }
        }
    }

    private var setup: some View {
        stepShell(icon: "lock.shield", title: "Clean .test + trusted HTTPS",
                  subtitle: "One-time privileged setup (optional).") {
            VStack(alignment: .leading, spacing: 12) {
                Text("This routes the .test domain, hands ports 80 and 443 to the dpl daemon, and trusts a local certificate authority — so your sites load at **http://name.test** (no port) with a padlock.")
                    .font(.callout)
                Text("Skip it and your sites still work at http://name.test:8080.")
                    .font(.caption).foregroundStyle(.secondary)
                Button {
                    Task {
                        settingUp = true
                        didSetup = await store.runSetup()
                        settingUp = false
                    }
                } label: {
                    if settingUp {
                        Label("Setting up…", systemImage: "hourglass")
                    } else {
                        Label(didSetup ? "Setup complete" : "Run Setup…",
                              systemImage: didSetup ? "checkmark.circle.fill" : "lock.shield")
                    }
                }
                .buttonStyle(.borderedProminent).tint(Theme.violet)
                .disabled(settingUp || didSetup)
                Text("macOS will ask for your password. Nothing leaves this window.")
                    .font(.caption2).foregroundStyle(.secondary)
            }
        }
    }

    private var autostartStep: some View {
        stepShell(icon: "power", title: "Run in the background",
                  subtitle: "Keep the daemon and menu bar always available.") {
            Toggle(isOn: $autostart) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Start Dply Local on login").font(.callout.weight(.medium))
                    Text("Installs a background service so your sites are always ready.")
                        .font(.caption).foregroundStyle(.secondary)
                }
            }
            .toggleStyle(.switch)
            .onChange(of: autostart) { _, on in
                Task { await store.daemonAction(on ? "install" : "uninstall") }
            }
        }
    }

    private var firstSiteStep: some View {
        stepShell(icon: "folder.badge.plus", title: "Add your first site",
                  subtitle: "Point Dply Local at a project (optional).") {
            VStack(alignment: .leading, spacing: 12) {
                if let s = firstSite {
                    Label("Linked \(s).test", systemImage: "checkmark.circle.fill").foregroundStyle(.green)
                }
                HStack {
                    Button { pick(park: false) } label: { Label("Link a project…", systemImage: "link") }
                        .buttonStyle(.borderedProminent).tint(Theme.violet)
                    Button { pick(park: true) } label: { Label("Park a folder…", systemImage: "folder") }
                }
                Text("Linking serves one project; parking serves every subfolder of a directory.")
                    .font(.caption).foregroundStyle(.secondary)
            }
        }
    }

    private var done: some View {
        stepShell(icon: "checkmark.seal.fill", title: "You're all set!",
                  subtitle: "A few things to know.") {
            VStack(alignment: .leading, spacing: 12) {
                bullet("menubar.arrow.up.rectangle", "The menu bar shows your PHP version — click it to switch or manage sites.")
                bullet("ladybug", "Add `shaferllc/dumps` to a project and call dumps($x) to debug here.")
                bullet("sidebar.left", "Everything lives in the sidebar: Sites, Services, Mail, Dumps, Status.")
            }
        }
    }

    // MARK: Chrome

    private func stepShell<C: View>(icon: String, title: String, subtitle: String, @ViewBuilder content: () -> C) -> some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(spacing: 14) {
                GradientTile(systemImage: icon, size: 52)
                VStack(alignment: .leading, spacing: 3) {
                    Text(title).font(.title2.weight(.bold))
                    Text(subtitle).font(.callout).foregroundStyle(.secondary)
                }
            }
            content()
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var footer: some View {
        HStack {
            // Progress dots.
            HStack(spacing: 6) {
                ForEach(0...last, id: \.self) { i in
                    Circle().fill(i == step ? AnyShapeStyle(Theme.brand) : AnyShapeStyle(Color.secondary.opacity(0.3)))
                        .frame(width: 7, height: 7)
                }
            }
            Spacer()
            if step > 0 {
                Button("Back") { step -= 1 }
            }
            if step < last {
                Button("Skip") { finish() }
                Button(step == 0 ? "Get Started" : "Next") { step += 1 }
                    .buttonStyle(.borderedProminent).tint(Theme.violet).keyboardShortcut(.defaultAction)
            } else {
                Button("Finish") { finish() }
                    .buttonStyle(.borderedProminent).tint(Theme.violet).keyboardShortcut(.defaultAction)
            }
        }
        .padding(16)
    }

    private func bullet(_ icon: String, _ text: String) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: icon).foregroundStyle(Theme.violet).frame(width: 20)
            Text(text).font(.callout)
            Spacer(minLength: 0)
        }
    }

    private func healthRow(_ line: String) -> some View {
        let mark = line.first.map(String.init) ?? ""
        let color: Color = mark == "✓" ? .green : (mark == "✗" ? .red : (mark == "⚠" ? .orange : .secondary))
        return HStack(alignment: .top, spacing: 8) {
            Text(mark).foregroundStyle(color).frame(width: 14)
            Text(line.dropFirst(mark.isEmpty ? 0 : 1).trimmingCharacters(in: .whitespaces)).font(.callout)
            Spacer(minLength: 0)
        }
    }

    private func pick(park: Bool) {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.prompt = park ? "Park" : "Link"
        if panel.runModal() == .OK, let url = panel.url {
            Task {
                if park { await store.parkLocal(path: url.path) } else { await store.linkLocal(path: url.path) }
                firstSite = url.lastPathComponent.lowercased()
            }
        }
    }

    private func finish() {
        didOnboard = true
        isPresented = false
    }
}
