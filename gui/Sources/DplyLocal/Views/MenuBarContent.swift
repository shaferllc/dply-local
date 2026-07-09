import SwiftUI
import AppKit

/// The status-bar menu — modeled on PhpMon: the active PHP version + switcher,
/// PHP configuration (php.ini, phpinfo, extensions, Xdebug), Composer, sites,
/// services, and first-aid. All driven by the same `dpl` engine + the active
/// PHP binary.
struct MenuBarContent: View {
    @EnvironmentObject var store: Store
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Text("GLOBAL VERSION: PHP \(store.phpFull ?? store.defaultPhp ?? "—")")

        // Version switcher.
        ForEach(Array(store.phpVersions.enumerated()), id: \.offset) { i, v in
            let version = v.cell(["version"])
            let isDefault = v.dig("default") == .bool(true)
            Button {
                Task { await store.usePhp(version: version, site: nil) }
            } label: {
                Text("\(isDefault ? "✓ " : "")Switch to PHP \(version)  (\(formula(v)))")
            }
            .keyboardShortcut(KeyEquivalent(Character("\(i + 1)")), modifiers: [])
            .disabled(isDefault)
        }

        Divider()

        // Xdebug step-debug quick toggle — front and center.
        if store.activeXdebug.stepDebug {
            Button {
                Task { await store.toggleActiveXdebug() }
            } label: {
                Label("Stop Debugging (Xdebug on)", systemImage: "ladybug.fill")
            }
        } else if store.activeXdebug.loaded || store.activeXdebug.disabledButInstalled {
            Button {
                Task { await store.toggleActiveXdebug() }
            } label: {
                Label("Start Debugging (Xdebug)", systemImage: "ladybug")
            }
        } else {
            Button("Install Xdebug…") { openManager() }
        }

        Divider()

        // Memory limits (read from the active php.ini).
        if !store.phpMemory.isEmpty {
            Text(store.phpMemory.map { "\($0.0): \($0.1)" }.joined(separator: "  ·  "))
        }

        Menu("PHP Configuration") {
            Button("Locate php.ini") { store.openPhpIni() }.keyboardShortcut("c")
            Button("Show phpinfo…") { store.showPhpInfo() }.keyboardShortcut("i")
            Menu("Extensions") {
                if store.phpExtensions.isEmpty { Text("None detected").disabled(true) }
                ForEach(store.phpExtensions) { ext in
                    Button {
                        Task { await store.toggleExtension(ext) }
                    } label: {
                        Text("\(ext.enabled ? "✓ " : "  ")\(ext.name)")
                    }
                }
            }
            if let xd = store.xdebugExt {
                Button(xd.enabled ? "Disable Xdebug" : "Enable Xdebug") {
                    Task { await store.toggleExtension(xd) }
                }
            } else {
                Text("Xdebug not installed").disabled(true)
            }
            Divider()
            Button("PHP Version Manager…") { openManager() }
        }

        Menu("Composer") {
            Button("Locate Global Composer (~/.composer)") { store.openComposerFolder() }
            Button("Update Global Dependencies…") { store.updateComposerGlobal() }
        }

        Divider()

        // Sites.
        Menu("Sites (\(store.localSites.count))") {
            if store.localSites.isEmpty { Text("No sites").disabled(true) }
            ForEach(store.localSites) { site in
                let name = site.cell(["name"])
                let path = site.cell(["path"])
                let isProxy = site.cell(["source"]) == "proxy"
                Menu(site.cell(["host"])) {
                    Button("Open in Browser") { store.openURL(site.cell(["url"])) }
                    if !isProxy {
                        Button("Open Folder") { store.openFolder(path) }
                        Button("Open in Editor") { store.openFolderInEditor(path) }
                        if site.dig("source") == .string("linked") {
                            let secure = site.dig("secure") == .bool(true)
                            Divider()
                            Button(secure ? "Disable HTTPS" : "Secure (HTTPS)") {
                                Task { await store.setLocalSecure(name: name, secure: !secure) }
                            }
                        }
                    }
                }
            }
        }

        // Services.
        Menu("Services") {
            ForEach(store.dbServices) { s in
                let name = s.cell(["name"])
                let running = s.dig("running") == .bool(true)
                if s.dig("external") == .bool(true) {
                    Text("\(name.capitalized) — running (DBngin)").disabled(true)
                } else if s.dig("installed") == .bool(true) {
                    Button("\(running ? "Stop" : "Start") \(name.capitalized)") {
                        Task { await store.serviceAction(running ? "stop" : "start", name: name) }
                    }
                } else {
                    Text("\(name.capitalized) — not installed").disabled(true)
                }
            }
        }

        Menu("First Aid & Services") {
            Button("Open ~/.dpl folder") { store.openDplFolder() }
            Button("Restart daemon") { Task { await store.restartDaemon() } }
            Button("Run setup (trusted .test)…") { store.runSetupInTerminal() }
        }

        Divider()

        Button("Open DplyLocal") {
            // Ensure the window exists, then activate the app and explicitly
            // raise the window — openWindow alone won't front an already-open
            // (but buried) window on recent macOS.
            NSApp.setActivationPolicy(.regular)
            openWindow(id: "main")
            NSApp.activate(ignoringOtherApps: true)
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) {
                NSApp.activate(ignoringOtherApps: true)
                for window in NSApp.windows where window.canBecomeMain {
                    window.makeKeyAndOrderFront(nil)
                    window.orderFrontRegardless()
                }
            }
        }
        Button("Refresh") { Task { await store.refreshMenuData() } }
        SettingsLink { Text("Settings…") }.keyboardShortcut(",")

        Divider()

        Button("Quit DplyLocal") { NSApp.terminate(nil) }.keyboardShortcut("q")
    }

    /// Open the main window on the PHP Version Manager section.
    private func openManager() {
        store.section = .php
        Task { await store.loadPhpCatalog() }
        NSApp.setActivationPolicy(.regular)
        openWindow(id: "main")
        NSApp.activate(ignoringOtherApps: true)
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.05) {
            NSApp.activate(ignoringOtherApps: true)
            for window in NSApp.windows where window.canBecomeMain {
                window.makeKeyAndOrderFront(nil)
                window.orderFrontRegardless()
            }
        }
    }

    /// The Homebrew-ish formula label for a version (e.g. `php@8.4`, or `php`
    /// for the linked default).
    private func formula(_ v: Row) -> String {
        let version = v.cell(["version"])
        let source = v.cell(["source"])
        if source == "dbngin" { return "dbngin \(version)" }
        return v.dig("default") == .bool(true) ? "php" : "php@\(version)"
    }
}
